#!/usr/bin/env bash
# All-in-one test entry point for the rustdesk-server modern transports work.
#
#   Level 1  cargo build --release, cargo clippy --all-targets (new warnings vs. baseline fail)
#   Level 2  cargo test (unit + tests/ protocol integration tests), per test binary
#   Level 3  server images from the same commit + the lab matrix with official clients
#
# Levels 1 and 2 run inside rust:1.93-bookworm containers on a `git archive` export of the
# committed HEAD (plus the hbb_common submodule commit recorded in HEAD), so uncommitted
# changes are never tested by accident and no root-owned files end up in the repo.
# Needs: Docker (via sudo), internet for crates/rustup/apt during builds. See lab/README.md.
set -uo pipefail

LAB="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$LAB/.." && pwd)"

CLIENT_VERSION=1.5.0
VARIANT=sciter
SKIP=""
CASES=()
RUST_IMAGE="rust:1.93-bookworm"
BASELINE_COMMIT=873230e
BASELINE_SUBMODULE=69cea8dafee147848ae88702029f4bf7df7224c3
MODERN_TAG="rustdesk-server-modern:dev"
BUILD_CPUS=8
PURGE_CACHES=0
STRICT_CLIPPY=0

usage() {
  cat <<EOF
Usage: $0 [options]

  -v, --client-version VER   "latest" client version for the lab (default: $CLIENT_VERSION;
                             old-client cases stay on 1.4.9)
      --variant V            client package for the lab: sciter | flutter (default: $VARIANT)
      --skip LEVELS          skip levels, e.g. --skip 1 or --skip 1,2
      --cases a,b,...        level 3: run only these lab cases (or topologies flat,lan,nat);
                             default: the whole matrix (lab/run-lab.sh --list)
      --purge-caches         delete the rdlab-* cargo cache volumes at the end
      --strict-clippy        new clippy diagnostics in libs/hbb_common (the submodule,
                             upstream code) fail level 1 too; default: reported as WARN
  -h, --help

Results: lab/out/<run-id>/all-tests-summary.txt (and per-level logs next to it).
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    -v|--client-version) CLIENT_VERSION="$2"; shift 2 ;;
    --variant) VARIANT="$2"; shift 2 ;;
    --skip) SKIP="$SKIP,$2"; shift 2 ;;
    --cases) IFS=',' read -r -a _c <<<"$2"; CASES+=("${_c[@]}"); shift 2 ;;
    --purge-caches) PURGE_CACHES=1; shift ;;
    --strict-clippy) STRICT_CLIPPY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument $1" >&2; usage >&2; exit 2 ;;
  esac
done
skipped() { [[ ",$SKIP," == *",$1,"* ]]; }

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

RUN_ID="all-$(date +%Y%m%d-%H%M%S)"
OUT="$LAB/out/$RUN_ID"
mkdir -p "$OUT"
exec 8>"$LAB/out/.lock-all"
flock -n 8 || { log "another run-all-tests.sh is running"; exit 2; }
T_START=$SECONDS

ITEMS=()      # level \x1f item \x1f result \x1f detail
FAILED=0
item() { # level item result detail
  ITEMS+=("$1"$'\x1f'"$2"$'\x1f'"$3"$'\x1f'"$4")
  case "$3" in PASS|SKIP|WARN) ;; *) FAILED=1 ;; esac
  log "L$1 $2: $3 ${4:+($4)}"
}

CONTAINERS=()
cleanup() {
  local c
  for c in "${CONTAINERS[@]}"; do sudo docker rm -f -v "$c" >/dev/null 2>&1 || true; done
  if [ "$PURGE_CACHES" = 1 ]; then
    sudo docker volume rm rdlab-cargo-registry rdlab-cargo-git rdlab-target-head rdlab-target-baseline \
      >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# --------------------------------------------------------------------------
# Sources: committed HEAD and the baseline, exported with git archive
# --------------------------------------------------------------------------
COMMIT="$(git -C "$REPO" rev-parse HEAD)"
SUBMODULE="$(git -C "$REPO" ls-tree HEAD libs/hbb_common | awk '{print $3}')"
log "testing commit $COMMIT ($(git -C "$REPO" log -1 --format=%s "$COMMIT")), hbb_common $SUBMODULE"
if [ -n "$(git -C "$REPO" status --porcelain --untracked-files=no 2>/dev/null)" ]; then
  log "note: the working tree has uncommitted changes; they are NOT tested"
fi
{
  echo "commit=$COMMIT"
  echo "subject=$(git -C "$REPO" log -1 --format=%s "$COMMIT")"
  echo "hbb_common=$SUBMODULE"
  echo "baseline=$BASELINE_COMMIT hbb_common=$BASELINE_SUBMODULE"
  echo "client=$CLIENT_VERSION ($VARIANT)"
} >"$OUT/run.txt"

export_tree() { # dir commit submodule
  rm -rf "$1" && mkdir -p "$1/libs/hbb_common"
  git -C "$REPO" archive "$2" | tar -x -C "$1" || return 1
  git -C "$REPO/libs/hbb_common" archive "$3" | tar -x -C "$1/libs/hbb_common" || return 1
  find "$1" -exec touch {} +   # fresh mtimes: cargo must never prefer cached artifacts
}
export_tree "$OUT/src-head" "$COMMIT" "$SUBMODULE" || { log "export of HEAD failed"; exit 2; }
if ! skipped 1 || ! skipped 2; then
  export_tree "$OUT/src-baseline" "$BASELINE_COMMIT" "$BASELINE_SUBMODULE" \
    || { log "export of the baseline failed"; exit 2; }
fi

# --------------------------------------------------------------------------
# Rust containers (default bridge network: the builds need crates.io / rustup)
# --------------------------------------------------------------------------
rust_start() { # name srcdir
  local c="rdlab-rust-$1"
  sudo docker rm -f -v "$c" >/dev/null 2>&1 || true
  sudo docker run -d --name "$c" --cpus "$BUILD_CPUS" --memory 12g \
    -e CARGO_BUILD_JOBS=8 -e CARGO_TARGET_DIR=/target -e CARGO_TERM_COLOR=never \
    -v rdlab-cargo-registry:/usr/local/cargo/registry \
    -v rdlab-cargo-git:/usr/local/cargo/git \
    -v "rdlab-target-$1:/target" \
    -v rdlab-target-baseline:/target-baseline \
    -w /src "$RUST_IMAGE" sleep infinity >/dev/null || return 1
  CONTAINERS+=("$c")
  # -m: extraction time as mtime (see export_tree)
  tar -C "$2" -c . | sudo docker exec -i "$c" sh -c 'mkdir -p /src && tar -x -m -C /src' || return 1
}
rx() { # name command (output to stdout/stderr)
  sudo docker exec "rdlab-rust-$1" bash -c "$2"
}
ensure_clippy() { # name
  rx "$1" 'cargo clippy --version >/dev/null 2>&1 || rustup component add clippy' >"$OUT/level1/rustup-$1.log" 2>&1
}

# clippy diagnostics without line/column, one per line, numbered per duplicate
clippy_norm() { # log
  grep -E '^[^ ]+:[0-9]+:[0-9]+: (warning|error)' "$1" \
    | sed -E 's/^([^ :]+):[0-9]+:[0-9]+: /\1: /' \
    | sort | awk '{n[$0]++; print $0 " #" n[$0]}' | sort
}

if ! skipped 1 || ! skipped 2; then
  log "starting rust container for HEAD"
  mkdir -p "$OUT/level1" "$OUT/level2"
  rust_start head "$OUT/src-head" || { item 1 "rust container" FAIL "docker run failed"; }
fi

# --------------------------------------------------------------------------
# Level 1
# --------------------------------------------------------------------------
if skipped 1; then
  item 1 "level 1" SKIP ""
else
  log "level 1: cargo build --release (HEAD)"
  if rx head 'cargo build --release --locked' >"$OUT/level1/build-release.log" 2>&1; then
    item 1 "cargo build --release" PASS "$(grep -c '^warning' "$OUT/level1/build-release.log") warning lines"
  else
    item 1 "cargo build --release" FAIL "$(grep -m1 -E '^error' "$OUT/level1/build-release.log")"
  fi

  log "level 1: cargo clippy --all-targets (HEAD)"
  ensure_clippy head
  rx head 'cargo clippy --all-targets --locked --message-format=short -- --cap-lints warn' >"$OUT/level1/clippy-head.log" 2>&1
  rc_head=$?

  log "level 1: cargo clippy --all-targets (baseline $BASELINE_COMMIT)"
  if rust_start baseline "$OUT/src-baseline"; then
    ensure_clippy baseline
    rx baseline 'cargo clippy --all-targets --locked --message-format=short -- --cap-lints warn' >"$OUT/level1/clippy-baseline.log" 2>&1
    rc_base=$?
  else
    rc_base=99; : >"$OUT/level1/clippy-baseline.log"
  fi
  clippy_norm "$OUT/level1/clippy-head.log" >"$OUT/level1/clippy-head.norm"
  clippy_norm "$OUT/level1/clippy-baseline.log" >"$OUT/level1/clippy-baseline.norm"
  comm -23 "$OUT/level1/clippy-head.norm" "$OUT/level1/clippy-baseline.norm" >"$OUT/level1/clippy-new.txt"
  n_head=$(wc -l <"$OUT/level1/clippy-head.norm")
  n_base=$(wc -l <"$OUT/level1/clippy-baseline.norm")
  n_new=$(wc -l <"$OUT/level1/clippy-new.txt")
  # --cap-lints warn: deny-by-default lints (e.g. unused_io_amount, already in the
  # baseline) would stop the lib and leave bins/tests unlinted; as warnings they are
  # still compared below. A non-zero exit therefore means a real compile error.
  if [ "$rc_head" = 0 ]; then
    item 1 "cargo clippy --all-targets" PASS "exit 0; HEAD $n_head diagnostics, baseline $n_base (exit $rc_base)"
  else
    item 1 "cargo clippy --all-targets" FAIL "exit $rc_head: $(grep -m1 -E '^error' "$OUT/level1/clippy-head.log")"
  fi
  # server code vs. the hbb_common submodule (upstream code, bumped 69cea8d -> HEAD's commit)
  grep -v '^libs/hbb_common/' "$OUT/level1/clippy-new.txt" >"$OUT/level1/clippy-new-server.txt"
  grep '^libs/hbb_common/' "$OUT/level1/clippy-new.txt" >"$OUT/level1/clippy-new-hbb_common.txt"
  n_new_srv=$(wc -l <"$OUT/level1/clippy-new-server.txt")
  n_new_hbb=$(wc -l <"$OUT/level1/clippy-new-hbb_common.txt")
  if [ "$n_new_srv" = 0 ]; then
    item 1 "clippy: no new warnings (server code)" PASS "0 new (HEAD $n_head, baseline $n_base diagnostics)"
  else
    item 1 "clippy: no new warnings (server code)" FAIL "$n_new_srv new, see level1/clippy-new-server.txt; first: $(head -1 "$OUT/level1/clippy-new-server.txt" | cut -c1-120)"
  fi
  if [ "$n_new_hbb" = 0 ]; then
    item 1 "clippy: no new warnings (hbb_common)" PASS "0 new"
  else
    hbb_res=WARN; [ "$STRICT_CLIPPY" = 1 ] && hbb_res=FAIL
    item 1 "clippy: no new warnings (hbb_common)" "$hbb_res" "$n_new_hbb new in the submodule (${BASELINE_SUBMODULE:0:7} -> ${SUBMODULE:0:7}): $(cut -d' ' -f1 "$OUT/level1/clippy-new-hbb_common.txt" | sort -u | paste -sd' ' -)"
  fi
fi

# --------------------------------------------------------------------------
# Level 2
# --------------------------------------------------------------------------
if skipped 2; then
  item 2 "level 2" SKIP ""
else
  # tests/compat.rs runs old-style message sequences against the original hbbs too; it
  # needs HBBS_BASELINE_BIN (debug build of 873230e + hbb_common 69cea8d, own target dir)
  log "level 2: building the baseline hbbs for tests/compat.rs"
  BASE_BIN=/target-baseline/debug/hbbs
  if tar -C "$OUT/src-baseline" -c . \
       | sudo docker exec -i rdlab-rust-head sh -c 'rm -rf /src-baseline && mkdir -p /src-baseline && tar -x -m -C /src-baseline' \
     && rx head 'cd /src-baseline && CARGO_TARGET_DIR=/target-baseline cargo build --locked --bin hbbs' \
          >"$OUT/level2/build-baseline-hbbs.log" 2>&1; then
    item 2 "baseline hbbs for compat ($BASELINE_COMMIT)" PASS "$BASE_BIN"
  else
    item 2 "baseline hbbs for compat ($BASELINE_COMMIT)" FAIL "see level2/build-baseline-hbbs.log"
  fi
  log "level 2: cargo test (HEAD, HBBS_BASELINE_BIN=$BASE_BIN)"
  rx head "HBBS_BASELINE_BIN=$BASE_BIN cargo test --locked --no-fail-fast" >"$OUT/level2/cargo-test.log" 2>&1
  rc_test=$?
  # one line per test binary: name \t result \t passed \t failed
  awk '
    /^ +Running / {
      if (cur != "" && !done) print cur "\tNO RESULT\t0\t0"
      cur = ($2 == "unittests") ? "unittests " $3 : $2; done = 0; next }
    /^ +Doc-tests / {
      if (cur != "" && !done) print cur "\tNO RESULT\t0\t0"
      cur = "doc-tests " $2; done = 0; next }
    /^test result: / {
      res = ($3 == "ok.") ? "ok" : "FAILED"
      p = $4; f = $6; print cur "\t" res "\t" p "\t" f; done = 1; next }
    END { if (cur != "" && !done) print cur "\tNO RESULT\t0\t0" }
  ' "$OUT/level2/cargo-test.log" >"$OUT/level2/binaries.tsv"
  if [ ! -s "$OUT/level2/binaries.tsv" ]; then
    item 2 "cargo test" FAIL "no test binary ran (exit $rc_test): $(grep -m1 -E '^error' "$OUT/level2/cargo-test.log")"
  else
    while IFS=$'\t' read -r bin res p f; do
      if [ "$res" = ok ]; then
        item 2 "$bin" PASS "$p passed"
      else
        fails="$(grep -E '^test .* \.\.\. FAILED$' "$OUT/level2/cargo-test.log" | awk '{print $2}' | head -3 | paste -sd' ' -)"
        item 2 "$bin" FAIL "$res, $p passed, $f failed${fails:+; e.g. $fails}"
      fi
    done <"$OUT/level2/binaries.tsv"
    if [ "$rc_test" != 0 ] && ! grep -qv $'\tok\t' "$OUT/level2/binaries.tsv"; then
      item 2 "cargo test (exit status)" FAIL "exit $rc_test although every binary reported ok"
    fi
  fi
  # tests/compat.rs is #[ignore]d without the original binary; run it explicitly,
  # uncaptured, to show that it compared against the original hbbs
  if rx head 'test -f /src/tests/compat.rs'; then
    rx head "HBBS_BASELINE_BIN=$BASE_BIN cargo test --locked --test compat -- --ignored --nocapture" \
      >"$OUT/level2/compat-nocapture.log" 2>&1
    rc_compat=$?
    if ! grep -q '^original: ' "$OUT/level2/compat-nocapture.log"; then
      item 2 "compat: ran against the original hbbs" FAIL "no comparison output, see level2/compat-nocapture.log"
    elif [ "$rc_compat" != 0 ]; then
      item 2 "compat: ran against the original hbbs" FAIL "exit $rc_compat, see level2/compat-nocapture.log"
    else
      item 2 "compat: ran against the original hbbs" PASS "$(grep -m1 '^test result' "$OUT/level2/compat-nocapture.log")"
    fi
  else
    item 2 "compat: ran against the original hbbs" FAIL "tests/compat.rs missing in $COMMIT"
  fi
fi

# the rust containers are not needed any more
cleanup; CONTAINERS=()

# --------------------------------------------------------------------------
# Level 3
# --------------------------------------------------------------------------
if skipped 3; then
  item 3 "level 3" SKIP ""
else
  mkdir -p "$OUT/level3"
  log "level 3: building $MODERN_TAG from $COMMIT"
  if sudo docker build --build-arg CARGO_BUILD_JOBS=8 \
      -f "$OUT/src-head/docker-classic/Dockerfile.multistage" -t "$MODERN_TAG" "$OUT/src-head" \
      >"$OUT/level3/build-modern.log" 2>&1; then
    item 3 "image $MODERN_TAG" PASS "$(sudo docker image inspect -f '{{.Id}}' "$MODERN_TAG" | cut -c8-19)"
    log "level 3: lab matrix (baseline image is (re)built by run-lab.sh --build-baseline)"
    "$LAB/run-lab.sh" -v "$CLIENT_VERSION" --variant "$VARIANT" -s "$MODERN_TAG" --build-baseline \
      --wait-modern 0 --out "$OUT/level3/lab" "${CASES[@]}" >"$OUT/level3/run-lab.log" 2>&1
    rc_lab=$?
    if [ -s "$OUT/level3/lab/results.tsv" ]; then
      while IFS=$'\t' read -r n tp s cl e o g se kx t v no; do
        item 3 "$n" "$v" "$tp, clients $cl, offered $o, got $g, session $se, kx $kx, ${t}s${no:+; $no}"
      done <"$OUT/level3/lab/results.tsv"
    else
      item 3 "lab matrix" FAIL "run-lab.sh exit $rc_lab without results, see level3/run-lab.log"
    fi
  else
    item 3 "image $MODERN_TAG" FAIL "docker build failed, see level3/build-modern.log"
  fi
fi

# --------------------------------------------------------------------------
# Summary
# --------------------------------------------------------------------------
ELAPSED=$((SECONDS - T_START))
{
  echo "rustdesk-server tests, commit $COMMIT (hbb_common ${SUBMODULE:0:7}), client $CLIENT_VERSION/$VARIANT"
  echo "run $RUN_ID, $((ELAPSED / 60))m$((ELAPSED % 60))s"
  echo
  printf '%-5s %-36s %-6s %s\n' LEVEL ITEM RESULT DETAIL
  for it in "${ITEMS[@]}"; do
    IFS=$'\x1f' read -r l n r d <<<"$it"
    printf '%-5s %-36s %-6s %s\n' "$l" "$n" "$r" "$d"
  done
  echo
  if [ "$FAILED" = 0 ]; then echo "OVERALL: PASS"; else echo "OVERALL: FAIL"; fi
  echo "(WARN = reported, not counted as failure; SKIP = level skipped)"
} | tee "$OUT/all-tests-summary.txt"
echo "results: $OUT"
exit $FAILED
