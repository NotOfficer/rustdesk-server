#!/usr/bin/env bash
# RustDesk transport lab: runs official RustDesk clients (headless, Xvfb) against
# a baseline and a modern hbbs/hbbr and checks which connection type the
# controlling side actually used. See lab/README.md.
set -uo pipefail

LAB="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$LAB/.." && pwd)"
PROJECT=rdlab

# --------------------------------------------------------------------------
# Defaults
# --------------------------------------------------------------------------
CLIENT_VERSION=1.5.0                 # "latest" side
OLD_VERSION=1.4.9                    # what "old" means in the case table
VARIANT=sciter                       # sciter | flutter (see NOTES.md)
BASELINE_COMMIT=873230e
BASELINE_SUBMODULE=69cea8dafee147848ae88702029f4bf7df7224c3
BASELINE_IMAGE="rustdesk-server-baseline:${BASELINE_COMMIT}"
MODERN_IMAGE="rustdesk-server-modern:dev"
BUILD_MODERN=0
BUILD_BASELINE=0
KEEP=0
TIMEOUT=90                           # seconds to wait for the "used to establish" line
SESSION_WAIT=60                      # seconds to wait for the session (peer info) afterwards;
                                     # covers one client retry after an hbbr pairing race (NOTES)
WAIT_MODERN=600                      # seconds to wait for a missing modern image
PASSWORD="rdlab-Passw0rd"
CPUS=2
TOPOLOGY_OVERRIDE=""                 # empty: every case runs in its own topology
ROUTER_IMAGE="rdlab-router:latest"
CT_LIBERAL=0                         # nat/lan: routers' nf_conntrack_tcp_be_liberal
HBBS_LOG_LEVEL=debug                 # RUST_LOG of hbbs
OUT=""
SELECTED=()
declare -A EXPECT_OVERRIDE=()

# --------------------------------------------------------------------------
# Case matrix: the one place for expectations. Fields are separated by ";":
#   name ; topology ; server ; clients ; hbbs env ; controller options ;
#   expected winner ; expected offer ; hbbs log check
# topology: flat | lan | nat (compose.yaml [+ compose.<topology>.yaml])
# server:   baseline | modern
# clients:  <controller>/<controlled>, each "latest" (= --client-version) or "old"
#           (= 1.4.9) or an explicit version; a single value means both sides
# hbbs env: space separated KEY=VALUE (feature switches such as WEBRTC=N)
# options:  space separated key=value for the controller's RustDesk_local.toml, "-" = none
# expected winner: regex on <TYPE> of the first "used to establish <TYPE> connection"
#                  (TCP, UDP, IPv6, WebRTC, Relay, WebSocket)
# expected offer:  regex one of the "#1 <transports> punch attempt" lines must match
#                  (a WebRTC offer round and a TCP fallback round can both appear), "-" = none
# hbbs log check:  regex the hbbs log must contain, "!regex" = must not contain, "-" = none
# Every case also requires the session to come up (controller got the peer info).
# --------------------------------------------------------------------------
ALL_N="enable-udp-punch=N enable-ipv6-punch=N enable-webrtc=N"
ALL_Y="enable-tcp-punch=Y enable-udp-punch=Y enable-ipv6-punch=Y enable-webrtc=Y"
UDP_ONLY="enable-tcp-punch=N enable-udp-punch=Y enable-ipv6-punch=N enable-webrtc=N"
IPV6_ONLY="enable-tcp-punch=N enable-udp-punch=N enable-ipv6-punch=Y enable-webrtc=N"
WEBRTC_ONLY="enable-tcp-punch=N enable-udp-punch=N enable-ipv6-punch=N enable-webrtc=Y"
DIRECT="TCP|UDP|IPv6|WebRTC"
ANY="TCP|UDP|IPv6|WebRTC|Relay"
SWITCHES_OFF="SECURE_TCP=N IPV6_PUNCH=N WEBRTC=N UDP_PUNCH=N"
CASES=(
  # --- flat: one WAN network, no NAT
  "baseline;flat;baseline;latest;;-;TCP;TCP;Punch hole"
  "baseline-allopts;flat;baseline;latest;;${ALL_Y};TCP;TCP;-"
  "modern-off;flat;modern;latest;;${ALL_N};TCP;TCP;-"
  "modern-switches-off;flat;modern;latest;${SWITCHES_OFF};${ALL_Y};TCP;TCP;!secured, version"
  "udp-only;flat;modern;latest;;${UDP_ONLY};UDP;UDP;-"
  "ipv6-only;flat;modern;latest;;${IPV6_ONLY};IPv6;IPv6;-"
  "webrtc-only;flat;modern;latest;;${WEBRTC_ONLY};WebRTC;WebRTC;secured, version 1"
  "all-on;flat;modern;latest;;${ALL_Y};${DIRECT};-;-"
  "old-client;flat;modern;old;;-;TCP;TCP;-"
  "baseline-old-allopts;flat;baseline;old;;${ALL_Y};${DIRECT};-;-"
  "old-client-allopts;flat;modern;old;;${ALL_Y};${DIRECT};-;-"
  "mixed-old-controlled;flat;modern;latest/old;;${ALL_Y};${DIRECT};-;-"
  "mixed-old-controller;flat;modern;old/latest;;${ALL_Y};${DIRECT};-;-"
  "webrtc-old-controlled;flat;modern;latest/old;;${WEBRTC_ONLY};${ANY};-;-"
  "baseline-webrtc-old-controlled;flat;baseline;latest/old;;${WEBRTC_ONLY};${ANY};-;-"
  # client 1.4.0 (no UDP/IPv6/WebRTC fields, logs no connection type)
  "client-1.4.0;flat;modern;1.4.0;;-;TCP;-;-"
  "baseline-client-1.4.0;flat;baseline;1.4.0;;-;TCP;-;-"
  "mixed-1.4.0-controller;flat;modern;1.4.0/latest;;-;TCP;-;-"
  "mixed-1.4.0-controlled;flat;modern;latest/1.4.0;;${ALL_Y};${DIRECT};-;-"
  # --- lan: both clients behind ONE router (same public IP -> same-intranet path)
  "lan-baseline;lan;baseline;latest;;-;TCP;TCP;Fetch local addr"
  "lan-default;lan;modern;latest;;-;TCP;TCP;Fetch local addr"
  "lan-webrtc;lan;modern;latest;;${WEBRTC_ONLY};WebRTC;WebRTC;WebRTC offer"
  "lan-all-on;lan;modern;latest;;${ALL_Y};${DIRECT};-;-"
  # --- nat: each client behind its own router (strict Linux conntrack by default)
  "nat-baseline;nat;baseline;latest;;-;Relay;TCP;-"
  "nat-udp-only;nat;modern;latest;;${UDP_ONLY};UDP;UDP;-"
  "nat-ipv6-only;nat;modern;latest;;${IPV6_ONLY};IPv6;IPv6;-"
  "nat-webrtc-only;nat;modern;latest;;${WEBRTC_ONLY};WebRTC;WebRTC;-"
  "nat-all-on;nat;modern;latest;;${ALL_Y};UDP|IPv6|WebRTC;-;-"
)
# Notes on expectations:
# - all-on / *-allopts / mixed-*: every direct transport races; whichever is fastest
#   wins, so any direct type passes (only Relay fails).
# - webrtc-old-controlled: a 1.4.9 controlled side cannot answer WebRTC; with TCP punch
#   off the 1.5.0 controller may end on the relay. Only the session is required.
# - nat-baseline: with strict conntrack the TCP punch fails (see NOTES), so Relay.
#   With --ct-liberal it becomes TCP: use --expect nat-baseline=TCP.
# - nat-all-on: TCP cannot win through strict conntrack routers.

read_case() {
  IFS=';' read -r C_NAME C_TOPO C_SERVER C_CLIENT C_HBBS_ENV C_OPTS C_EXPECT C_OFFER C_HBBS_CHECK <<<"$1"
}

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "ERROR: $*"; exit 2; }

usage() {
  cat <<EOF
Usage: $0 [options] [case|topology ...]

Selection: case names, or a topology name (flat, lan, nat) for all its cases.
Default: all cases.

Options:
  -v, --client-version VER   "latest" client version (default: $CLIENT_VERSION; "old" = $OLD_VERSION)
      --variant V            client package: sciter | flutter (default: $VARIANT)
  -s, --server-image TAG     modern server image (default: $MODERN_IMAGE)
      --baseline-image TAG   baseline server image (default: $BASELINE_IMAGE)
  -c, --cases a,b,...        same as positional selection
      --build-modern         build the modern image (-s tag) from the current working tree
                             (docker-classic/Dockerfile.multistage)
      --build-baseline       (re)build the baseline image from $BASELINE_COMMIT
      --topology T           run all selected cases in topology T (flat | lan | nat)
                             instead of each case's own
      --ct-liberal           lan/nat routers: liberal TCP conntrack window tracking
                             (nf_conntrack_tcp_be_liberal=1). Env RDLAB_V6_FIREWALL=0 opens
                             the routers' IPv6 firewall, RDLAB_WAN_INPUT=reject makes them
                             answer unsolicited WAN packets with RST/ICMP
      --expect CASE=REGEX    override the expected winner of a case (repeatable)
      --out DIR              result directory (default: lab/out/<run-id>)
      --keep                 leave the last case's stack running
  -t, --timeout SEC          wait for the connection result (default: $TIMEOUT)
      --wait-modern SEC      poll this long for a missing modern image (default: $WAIT_MODERN)
  -l, --list                 list cases and exit
  -h, --help
EOF
}

# --------------------------------------------------------------------------
# Args
# --------------------------------------------------------------------------
while [ $# -gt 0 ]; do
  case "$1" in
    -v|--client-version) CLIENT_VERSION="$2"; shift 2 ;;
    --variant) VARIANT="$2"; shift 2 ;;
    -s|--server-image) MODERN_IMAGE="$2"; shift 2 ;;
    --baseline-image) BASELINE_IMAGE="$2"; shift 2 ;;
    -c|--cases) IFS=',' read -r -a _c <<<"$2"; SELECTED+=("${_c[@]}"); shift 2 ;;
    --build-modern) BUILD_MODERN=1; shift ;;
    --build-baseline) BUILD_BASELINE=1; shift ;;
    --keep) KEEP=1; shift ;;
    --topology) TOPOLOGY_OVERRIDE="$2"; shift 2 ;;
    --ct-liberal) CT_LIBERAL=1; shift ;;
    --expect) EXPECT_OVERRIDE["${2%%=*}"]="${2#*=}"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    -t|--timeout) TIMEOUT="$2"; shift 2 ;;
    --wait-modern) WAIT_MODERN="$2"; shift 2 ;;
    -l|--list) LIST=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) die "unknown option $1" ;;
    *) SELECTED+=("$1"); shift ;;
  esac
done
if [ "${LIST:-0}" = 1 ]; then
  for c in "${CASES[@]}"; do
    read_case "$c"
    printf '%-31s %-4s %-8s clients=%-11s hbbs-env=[%s] opts=[%s] expect=%s offer=%s hbbs-log=%s\n' \
      "$C_NAME" "$C_TOPO" "$C_SERVER" "$C_CLIENT" "$C_HBBS_ENV" "$C_OPTS" "$C_EXPECT" "$C_OFFER" "$C_HBBS_CHECK"
  done
  exit 0
fi
case "$VARIANT" in sciter|flutter) ;; *) die "--variant must be sciter or flutter" ;; esac
case "$TOPOLOGY_OVERRIDE" in ""|flat|lan|nat) ;; *) die "--topology must be flat, lan or nat" ;; esac

RUN_CASES=()
if [ ${#SELECTED[@]} -eq 0 ] || [ "${SELECTED[*]}" = "all" ]; then
  RUN_CASES=("${CASES[@]}")
else
  for want in "${SELECTED[@]}"; do
    found=""
    for c in "${CASES[@]}"; do
      read_case "$c"
      if [ "$C_NAME" = "$want" ] || [ "$C_TOPO" = "$want" ]; then RUN_CASES+=("$c"); found=1; fi
    done
    [ -n "$found" ] || die "unknown case or topology '$want' (see --list)"
  done
fi

# --------------------------------------------------------------------------
# Docker helpers (lab objects only: project rdlab, images rdlab-*/rustdesk-server-*)
# --------------------------------------------------------------------------
declare -a DENV=()
COMPOSE_FILES=(-f "$LAB/compose.yaml")
TOPO=flat
dc() { sudo env "${DENV[@]}" docker compose -p "$PROJECT" "${COMPOSE_FILES[@]}" "$@"; }
image_exists() { sudo docker image inspect "$1" >/dev/null 2>&1; }

set_topology() { # flat | lan | nat
  TOPO="$1"
  COMPOSE_FILES=(-f "$LAB/compose.yaml")
  [ "$TOPO" != flat ] && COMPOSE_FILES+=(-f "$LAB/compose.$TOPO.yaml")
}

routers_of() { case "$1" in nat) echo "router1 router2" ;; lan) echo "router1" ;; *) echo "" ;; esac; }

lock() {
  mkdir -p "$LAB/out"
  exec 9>"$LAB/out/.lock"
  flock -n 9 || die "another run-lab.sh is running (lab/out/.lock)"
}

teardown() {
  # remove every container of the project, whatever overlay created it
  sudo docker ps -aq --filter "label=com.docker.compose.project=$PROJECT" \
    | xargs -r sudo docker rm -f -v >/dev/null 2>&1 || true
  dc down -v --remove-orphans -t 3 >/dev/null 2>&1 || true
  sudo docker network ls -q --filter "label=com.docker.compose.project=$PROJECT" \
    | xargs -r sudo docker network rm >/dev/null 2>&1 || true
}

on_exit() {
  local rc=$?
  if [ "$KEEP" != 1 ]; then teardown; fi
  exit $rc
}

# --------------------------------------------------------------------------
# Images
# --------------------------------------------------------------------------
build_baseline() {
  local src="$LAB/out/src-baseline"
  log "exporting $BASELINE_COMMIT (+ hbb_common ${BASELINE_SUBMODULE:0:7}) to $src"
  rm -rf "$src" && mkdir -p "$src/libs/hbb_common"
  git -C "$REPO" archive "$BASELINE_COMMIT" | tar -x -C "$src" || die "git archive failed"
  git -C "$REPO/libs/hbb_common" archive "$BASELINE_SUBMODULE" | tar -x -C "$src/libs/hbb_common" \
    || die "git archive of hbb_common failed"
  [ -f "$REPO/.dockerignore" ] && cp "$REPO/.dockerignore" "$src/"
  # git archive stamps files with the commit time; give them a fresh mtime so cargo
  # never takes stale artifacts from a cache for them.
  find "$src" -exec touch {} +
  log "building $BASELINE_IMAGE (lab/docker/Dockerfile.server-baseline, log: out/build-baseline.log)"
  sudo docker build --build-arg CARGO_BUILD_JOBS=8 \
    -f "$LAB/docker/Dockerfile.server-baseline" -t "$BASELINE_IMAGE" "$src" \
    >"$LAB/out/build-baseline.log" 2>&1 || die "baseline build failed, see lab/out/build-baseline.log"
}

build_modern() {
  log "building $MODERN_IMAGE from the working tree (log: out/build-modern.log)"
  sudo docker build --build-arg CARGO_BUILD_JOBS=8 \
    -f "$REPO/docker-classic/Dockerfile.multistage" -t "$MODERN_IMAGE" "$REPO" \
    >"$LAB/out/build-modern.log" 2>&1 || die "modern build failed, see lab/out/build-modern.log"
}

wait_modern() {
  image_exists "$MODERN_IMAGE" && return 0
  log "modern image $MODERN_IMAGE not found, polling up to ${WAIT_MODERN}s"
  local end=$((SECONDS + WAIT_MODERN))
  while [ $SECONDS -lt $end ]; do
    sleep 15
    image_exists "$MODERN_IMAGE" && return 0
  done
  return 1
}

deb_name() { # ver variant
  if [ "$2" = sciter ]; then echo "rustdesk-$1-x86_64-sciter.deb"; else echo "rustdesk-$1-x86_64.deb"; fi
}

client_image() { echo "rdlab-client:$1-$VARIANT"; }

# Revision of the client image recipe; an image built from older files is rebuilt.
client_rev() { cat "$LAB/docker/Dockerfile.client" "$LAB/docker/client-entrypoint.sh" | sha256sum | cut -c1-16; }

ensure_router() {
  local rev
  rev="$(cat "$LAB/docker/Dockerfile.router" "$LAB/docker/router.sh" | sha256sum | cut -c1-16)"
  if [ "$(sudo docker image inspect -f '{{index .Config.Labels "rdlab.router.rev"}}' "$ROUTER_IMAGE" 2>/dev/null)" = "$rev" ]; then
    return 0
  fi
  log "building $ROUTER_IMAGE (log: out/build-router.log)"
  sudo docker build -f "$LAB/docker/Dockerfile.router" --label "rdlab.router.rev=$rev" \
    -t "$ROUTER_IMAGE" "$LAB/docker" >"$LAB/out/build-router.log" 2>&1 \
    || die "router image build failed, see lab/out/build-router.log"
}

ensure_client() { # ver
  local ver="$1" img deb dir rev
  img="$(client_image "$ver")"
  rev="$(client_rev)"
  if [ "$(sudo docker image inspect -f '{{index .Config.Labels "rdlab.client.rev"}}' "$img" 2>/dev/null)" = "$rev" ]; then
    return 0
  fi
  deb="$(deb_name "$ver" "$VARIANT")"
  dir="$LAB/.cache/$ver"
  mkdir -p "$dir"
  if [ ! -s "$dir/$deb" ]; then
    log "downloading $deb"
    curl -fL --retry 3 -o "$dir/$deb.part" \
      "https://github.com/rustdesk/rustdesk/releases/download/$ver/$deb" \
      && mv "$dir/$deb.part" "$dir/$deb" || { rm -f "$dir/$deb.part"; die "download of $deb failed"; }
  fi
  log "building $img (log: out/build-client-$ver-$VARIANT.log)"
  sudo docker build -f "$LAB/docker/Dockerfile.client" --build-context "debs=$dir" \
    --build-arg "RD_DEB=$deb" --build-arg "RDLAB_CLIENT_REV=$rev" -t "$img" "$LAB/docker" \
    >"$LAB/out/build-client-$ver-$VARIANT.log" 2>&1 || die "client image build failed, see lab/out/build-client-$ver-$VARIANT.log"
}

resolve_ver() { # latest|old|<ver>
  case "$1" in latest|default) echo "$CLIENT_VERSION" ;; old) echo "$OLD_VERSION" ;; *) echo "$1" ;; esac
}

# --------------------------------------------------------------------------
# One case
# --------------------------------------------------------------------------
copy_out() { # service path dest
  local cid
  cid="$(dc ps -a -q "$1" 2>/dev/null | head -1)"
  [ -n "$cid" ] || return 0
  mkdir -p "$3"
  sudo docker cp "$cid:$2" - 2>/dev/null | tar -x -C "$3" 2>/dev/null || true
}

controller_logs() {
  dc exec -T controller sh -c 'find /root/.local/share/logs -name "*.log" -exec cat {} + 2>/dev/null' 2>/dev/null
}

collect() { # dir
  local d="$1" s svcs
  svcs="hbbs hbbr stun controlled controller $(routers_of "$TOPO")"
  [ "$TOPO" != flat ] && svcs="$svcs hbbs-routes hbbr-routes stun-routes"
  for s in $svcs; do
    dc logs --no-color -t "$s" >"$d/$s.stdout.log" 2>&1 || true
  done
  for s in controller controlled; do
    copy_out "$s" /root/.local/share/logs "$d/$s"
    copy_out "$s" /root/.config/rustdesk "$d/$s"
    copy_out "$s" /tmp/rdlab "$d/$s"
  done
}

RESULTS=()
FAILED=0

record() { # name topo server clients expect offer got session kx secs verdict note
  RESULTS+=("$(printf '%s\x1f' "$@")")
  [ "${11}" = PASS ] || FAILED=1
  printf '%s\t' "$@" | sed 's/\t$//' >>"$OUT/results.tsv"
  echo >>"$OUT/results.tsv"
}

run_case() {
  read_case "$1"
  [ -n "${EXPECT_OVERRIDE[$C_NAME]+x}" ] && C_EXPECT="${EXPECT_OVERRIDE[$C_NAME]}"
  local name="$C_NAME" dir server_img ctl_ver cted_ver t0 key id got offer session kx verdict note=""
  local topo="${TOPOLOGY_OVERRIDE:-$C_TOPO}"
  set_topology "$topo"
  dir="$OUT/$name"
  mkdir -p "$dir"
  t0=$SECONDS
  log "=== case $name ($topo)"

  if [ "$C_SERVER" = baseline ]; then server_img="$BASELINE_IMAGE"; else server_img="$MODERN_IMAGE"; fi
  ctl_ver="$(resolve_ver "${C_CLIENT%%/*}")"
  cted_ver="$(resolve_ver "${C_CLIENT#*/}")"
  local clients="$ctl_ver/$cted_ver"
  local opts="$C_OPTS"; [ "$opts" = "-" ] && opts=""

  if ! image_exists "$server_img"; then
    record "$name" "$topo" "$server_img" "$clients" "$C_EXPECT" "-" "-" "-" "-" "$((SECONDS - t0))" FAIL "server image missing"
    log "server image $server_img missing"
    return
  fi
  ensure_client "$ctl_ver"; ensure_client "$cted_ver"
  t0=$SECONDS   # image downloads and builds do not count as case time
  [ "$topo" != flat ] && ensure_router

  {
    echo "RUST_LOG=$HBBS_LOG_LEVEL"
    for kv in $C_HBBS_ENV; do echo "$kv"; done
  } >"$dir/hbbs.env"
  cat >"$dir/case.txt" <<EOF
case=$name
topology=$topo (ct_liberal=$CT_LIBERAL)
server_image=$server_img ($(sudo docker image inspect -f '{{.Id}} created {{.Created}}' "$server_img" 2>/dev/null))
controller_client=$(client_image "$ctl_ver")
controlled_client=$(client_image "$cted_ver")
hbbs_env=$C_HBBS_ENV
controller_local_options=$opts
expected_winner=$C_EXPECT
expected_offer=$C_OFFER
hbbs_log_check=$C_HBBS_CHECK
EOF

  DENV=(
    "RDLAB_SERVER_IMAGE=$server_img"
    "RDLAB_HBBS_ENV_FILE=$dir/hbbs.env"
    "RDLAB_CLIENT_IMAGE_CONTROLLER=$(client_image "$ctl_ver")"
    "RDLAB_CLIENT_IMAGE_CONTROLLED=$(client_image "$cted_ver")"
    "RDLAB_CONTROLLER_LOCAL_OPTS=$opts"
    "RDLAB_PASSWORD=$PASSWORD"
    "RDLAB_CPUS=$CPUS"
    "RDLAB_ROUTER_IMAGE=$ROUTER_IMAGE"
    "RDLAB_CT_LIBERAL=$CT_LIBERAL"
    "RDLAB_V6_FIREWALL=${RDLAB_V6_FIREWALL:-1}"
    "RDLAB_WAN_INPUT=${RDLAB_WAN_INPUT:-drop}"
  )
  teardown
  dc up -d hbbs hbbr stun >"$dir/compose-up.log" 2>&1 || note="compose up (servers) failed"
  local routers n_routers
  routers="$(routers_of "$topo")"
  if [ -n "$routers" ] && [ -z "$note" ]; then
    n_routers=$(wc -w <<<"$routers")
    # shellcheck disable=SC2086
    dc up -d $routers hbbs-routes hbbr-routes stun-routes >>"$dir/compose-up.log" 2>&1 \
      || note="compose up (routers) failed"
    for _ in $(seq 1 30); do
      # shellcheck disable=SC2086
      [ "$(dc logs $routers 2>/dev/null | grep -c 'rdlab-router: ready')" -ge "$n_routers" ] && break
      sleep 1
    done
  fi

  # hbbs prints its freshly generated public key ("Key: ...") on first start.
  key=""
  for _ in $(seq 1 30); do
    key="$(dc logs --no-color hbbs 2>/dev/null | grep -o 'Key: [A-Za-z0-9+/=]*' | head -1 | cut -d' ' -f2)"
    [ -n "$key" ] && break
    sleep 1
  done
  [ -z "$key" ] && note="${note:-no key in hbbs log}"
  echo "hbbs_key=$key" >>"$dir/case.txt"
  DENV+=("RDLAB_KEY=$key")

  id=""
  if [ -z "$note" ]; then
    dc up -d controlled >>"$dir/compose-up.log" 2>&1 || note="compose up (controlled) failed"
    for _ in $(seq 1 60); do
      id="$(dc exec -T controlled cat /tmp/rdlab/id 2>/dev/null | tr -d '[:space:]')"
      [ -n "$id" ] && break
      sleep 1
    done
    [ -z "$id" ] && note="${note:-controlled side has no ID}"
  fi
  echo "controlled_id=$id" >>"$dir/case.txt"
  DENV+=("RDLAB_PEER_ID=$id")

  got=""; offer=""; session="no"; kx="-"
  if [ -z "$note" ]; then
    dc up -d controller >>"$dir/compose-up.log" 2>&1 || note="compose up (controller) failed"
    local end=$((SECONDS + TIMEOUT)) logs=""
    while [ $SECONDS -lt $end ]; do
      logs="$(controller_logs)"
      got="$(grep -o 'used to establish [^ ]* connection' <<<"$logs" | head -1 | awk '{print $4}')"
      # 1.4.0 logs no type ("used to establish connection"); resolved from hbbr below
      [ -z "$got" ] && grep -q 'used to establish connection' <<<"$logs" && got=untyped
      [ -n "$got" ] && break
      sleep 1
    done
    if [ -n "$got" ]; then
      # the session: login done and peer info received (relay retries can take a while)
      end=$((SECONDS + SESSION_WAIT))
      while [ $SECONDS -lt $end ]; do
        grep -q 'peer info supported_encoding\|handle_peer_info' <<<"$logs" && { session=yes; break; }
        sleep 1
        logs="$(controller_logs)"
      done
      sleep 2
      logs="$(controller_logs)"
    else
      note="${note:-no 'used to establish' line within ${TIMEOUT}s}"
    fi
    # A 1.5.0 controller with a WebRTC offer sends two requests in parallel (the offer
    # round and a pure TCP fallback round), so there can be several "#1" lines.
    offer="$(grep -o '#1 [^ ]* punch attempt' <<<"$logs" | awk '{print $2}' | awk '!seen[$0]++' | paste -sd, -)"
    grep -oE 'used to establish ([^ ]+ )?connection.*' <<<"$logs" >"$dir/established.txt" || true
    grep -o '#[0-9] [^ ]* punch attempt.*' <<<"$logs" >"$dir/punch-attempts.txt" || true
  fi

  collect "$dir"
  if [ "$got" = untyped ]; then
    # 1.4.0 knows only TCP as direct type; a relayed session shows up in hbbr
    if grep -q 'got paired' "$dir/hbbr.stdout.log" 2>/dev/null; then got=Relay; else got=TCP; fi
    note="${note:+$note; }type from hbbr log (1.4.0 logs none)"
  fi
  # key exchange versions hbbs completed ("Tcp connection from .. secured, version N")
  kx="$(grep -o 'secured, version [0-9]*' "$dir/hbbs.stdout.log" 2>/dev/null | awk '{print "v"$3}' | sort -u | paste -sd, -)"
  kx="${kx:--}"

  # visible side effects: reconnect rounds and hbbr's pairing race (two "New relay request"
  # for one uuid, no "got paired": both requests arrived at the same moment)
  local rounds unpaired
  rounds=$(grep -c . "$dir/established.txt" 2>/dev/null || echo 0)
  [ "${rounds:-0}" -gt 1 ] && note="${note:+$note; }$rounds connection rounds"
  unpaired=$(grep -o 'New relay request [^ ]*' "$dir/hbbr.stdout.log" 2>/dev/null | awk '{print $4}' | sort | uniq -c \
    | awk '$1 > 1 {print $2}' | while read -r u; do grep -q "Relayrequest $u .*got paired" "$dir/hbbr.stdout.log" || echo "$u"; done | wc -l)
  [ "${unpaired:-0}" -gt 0 ] && note="${note:+$note; }hbbr pairing race: $unpaired uuid(s) never paired"

  verdict=PASS
  if [ -z "$got" ] || ! [[ "$got" =~ ^(${C_EXPECT})$ ]]; then
    verdict=FAIL; [ -n "$got" ] && note="${note:+$note; }got $got, expected /$C_EXPECT/"
  fi
  if [ "$C_OFFER" != "-" ]; then
    local o ok=""
    for o in ${offer//,/ }; do [[ "$o" =~ ^(${C_OFFER})$ ]] && ok=1; done
    if [ -z "$ok" ]; then verdict=FAIL; note="${note:+$note; }offer '$offer' !~ /$C_OFFER/"; fi
  fi
  if [ "$session" != yes ] && [ -n "$got" ]; then
    verdict=FAIL; note="${note:+$note; }no session within ${SESSION_WAIT}s"
  fi
  if [ "$C_HBBS_CHECK" != "-" ] && [ -n "$C_HBBS_CHECK" ]; then
    if [ "${C_HBBS_CHECK:0:1}" = "!" ]; then
      if grep -qE -- "${C_HBBS_CHECK:1}" "$dir/hbbs.stdout.log"; then
        verdict=FAIL; note="${note:+$note; }hbbs log has /${C_HBBS_CHECK:1}/"
      fi
    elif ! grep -qE -- "$C_HBBS_CHECK" "$dir/hbbs.stdout.log"; then
      verdict=FAIL; note="${note:+$note; }hbbs log lacks /$C_HBBS_CHECK/"
    fi
  fi
  # hbbr's pairing race is pre-existing upstream behavior (hbbr is unchanged, see
  # DECISIONS.md); a case that failed with it gets one more attempt, marked as such
  if [ "$verdict" = FAIL ] && [ "${unpaired:-0}" -gt 0 ] && [ -z "${CASE_RETRY:-}" ]; then
    log "case $name: failed after hbbr's pairing race; retrying once (attempt 1 kept in $dir.attempt1)"
    rm -rf "$dir.attempt1"; mv "$dir" "$dir.attempt1"
    teardown
    CASE_RETRY="$note" run_case "$1"
    return
  fi
  [ -n "${CASE_RETRY:-}" ] && note="2nd attempt after hbbr pairing race${note:+; $note}"
  record "$name" "$topo" "$server_img" "$clients" "$C_EXPECT" "${offer:--}" "${got:--}" "$session" "$kx" "$((SECONDS - t0))" "$verdict" "$note"
  {
    echo "offered=$offer"
    echo "got=$got"
    echo "session=$session"
    echo "kx=$kx"
    echo "verdict=$verdict"
    echo "note=$note"
    echo "seconds=$((SECONDS - t0))"
  } >>"$dir/case.txt"
  log "case $name: offered=${offer:--} got=${got:--} session=$session kx=$kx -> $verdict ${note:+($note)}"
}

# --------------------------------------------------------------------------
# Main
# --------------------------------------------------------------------------
lock
trap on_exit EXIT
trap 'exit 130' INT TERM

RUN_ID="$(date +%Y%m%d-%H%M%S)"
[ -n "$OUT" ] || OUT="$LAB/out/$RUN_ID"
mkdir -p "$OUT"
: >"$OUT/results.tsv"
log "run $RUN_ID -> $OUT (client latest=$CLIENT_VERSION old=$OLD_VERSION $VARIANT, modern=$MODERN_IMAGE)"

need_baseline=0; need_modern=0
for c in "${RUN_CASES[@]}"; do
  read_case "$c"
  [ "$C_SERVER" = baseline ] && need_baseline=1
  [ "$C_SERVER" = modern ] && need_modern=1
done
if [ $need_baseline = 1 ] || [ $BUILD_BASELINE = 1 ]; then
  if [ $BUILD_BASELINE = 1 ] || ! image_exists "$BASELINE_IMAGE"; then build_baseline; fi
fi
if [ $BUILD_MODERN = 1 ]; then build_modern; fi
if [ $need_modern = 1 ]; then
  wait_modern || log "modern image $MODERN_IMAGE still missing; modern cases will fail"
fi

for i in "${!RUN_CASES[@]}"; do
  run_case "${RUN_CASES[$i]}"
  if [ "$KEEP" = 1 ] && [ "$i" -eq $((${#RUN_CASES[@]} - 1)) ]; then
    log "--keep: stack of the last case left running (sudo docker compose -p rdlab ${COMPOSE_FILES[*]} down -v)"
  else
    teardown
  fi
done

FMT='%-31s %-4s %-32s %-11s %-20s %-20s %-7s %-7s %-5s %4s  %-6s %s\n'
{
  printf "$FMT" CASE TOPO SERVER CLIENTS EXPECT OFFERED GOT SESSION KX SECS RESULT NOTE
  for r in "${RESULTS[@]}"; do
    IFS=$'\x1f' read -r n tp s cl e o g se kx t v no <<<"$r"
    printf "$FMT" "$n" "$tp" "$s" "$cl" "$e" "$o" "$g" "$se" "$kx" "$t" "$v" "$no"
  done
} | tee "$OUT/summary.txt"
echo "logs: $OUT"
exit $FAILED
