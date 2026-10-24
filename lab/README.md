# RustDesk transport lab

End-to-end test: two official RustDesk clients (headless, Xvfb) connect through a
baseline or modern `hbbs`/`hbbr`, and the lab reads from the controlling side's
log which connection type was actually used.

## Prerequisites

- Docker 29 with compose v2+, used via `sudo docker ...` (passwordless sudo).
- Internet access only for building images (cargo, apt, alpine) and for downloading
  client `.deb`s that are not in `lab/.cache/<ver>/` yet. The lab networks themselves
  are `internal: true` (no internet), and no ports are published on the host.
- Images (built on demand by `run-lab.sh`):
  - `rustdesk-server-baseline:873230e`: upstream master `873230e` + hbb_common `69cea8d`,
    exported with `git archive` to `lab/out/src-baseline/` and built with
    `lab/docker/Dockerfile.server-baseline` (a copy of
    `docker-classic/Dockerfile.multistage` with lab-only cache ids; see NOTES).
  - modern server: `rustdesk-server-modern:dev` (default; built by `run-all-tests.sh`
    from the committed HEAD, by `--build-modern` from the working tree, or by the server
    engineer), `-s` selects another tag.
  - `rdlab-client:<ver>-<sciter|flutter>`: ubuntu 24.04 + official `.deb` + Xvfb.
  - `rdlab-router:latest` (only for the `lan`/`nat` topologies): alpine + iptables.
  - `coturn/coturn:latest`: local STUN server.

## All tests in one go: `run-all-tests.sh`

Prerequisites: an Ubuntu (or any Linux) host with Docker and sudo for `docker`; no
host Rust needed. Internet is needed for crates.io/rustup/apt during the builds.

```sh
lab/run-all-tests.sh                    # everything, client 1.5.0
lab/run-all-tests.sh -v 1.5.1           # another "latest" client (old cases stay on 1.4.9)
lab/run-all-tests.sh --skip 1,2         # only level 3
lab/run-all-tests.sh --skip 3           # only cargo levels
lab/run-all-tests.sh --cases flat,lan-webrtc   # level 3 subset (case or topology names)
```

- Level 1: in a `rust:1.93-bookworm` container, on a `git archive` export of the
  committed HEAD + its hbb_common commit (uncommitted changes are not tested; the commit
  is printed and stored in `run.txt`). It runs `cargo build --release` and
  `cargo clippy --all-targets`. Clippy is also run on the baseline export (873230e +
  69cea8d), and any diagnostic that is new in HEAD (compared without line numbers) fails.
- Level 2: `cargo test --no-fail-fast`, reported per test binary. A debug `hbbs` of the
  baseline is built first and passed as `HBBS_BASELINE_BIN`. `tests/compat.rs` is run
  once more uncaptured and fails the level if it printed "skipped".
- Level 3: builds `rustdesk-server-modern:dev` from the same export
  (`docker-classic/Dockerfile.multistage`), rebuilds the baseline image, and runs the
  whole lab matrix (`run-lab.sh`, all topologies).
- Result: `lab/out/all-<run-id>/all-tests-summary.txt` (one PASS/FAIL line per item),
  with `level1/`, `level2/` and `level3/lab/<case>/` logs next to it. The exit code is
  non-zero if anything failed.
- Duration: about 25-30 min cold (cargo caches empty) and less with warm caches. The
  lab matrix alone takes about 7 min. Containers and networks are removed at the end. The
  cargo caches stay in the docker volumes `rdlab-cargo-registry`, `rdlab-cargo-git`,
  `rdlab-target-head` and `rdlab-target-baseline`; `--purge-caches` deletes them.

## Run

```sh
lab/run-lab.sh --list                         # show the case matrix
lab/run-lab.sh baseline baseline-allopts      # the cases that pass against the original hbbs
lab/run-lab.sh                                # all cases (modern ones use rustdesk-server-modern:dev)
lab/run-lab.sh lan                            # all cases of one topology
lab/run-lab.sh -s rustdesk-server-modern:foo udp-only
lab/run-lab.sh --build-modern                 # build the modern image from the working tree, then run
lab/run-lab.sh --build-baseline baseline      # rebuild the baseline image
lab/run-lab.sh --keep webrtc-only             # leave the stack running afterwards
lab/run-lab.sh --variant flutter baseline     # Flutter package instead of sciter
lab/run-lab.sh --topology nat --ct-liberal baseline   # run a flat case behind NAT routers
```

Exit code 0 only if every selected case passed. A summary table is printed at the end
and saved as `lab/out/<run-id>/summary.txt`. One case takes about 13-15 s (more when an
image has to be built; a WebRTC offer against a server without key exchange waits
18 s for the exchange).

Every case runs in its own topology (case field 2). `--topology T` forces one. If a modern image is missing, the script polls for it
for `--wait-modern` seconds (default 600) and then fails those cases.

Tear down a kept stack:
`sudo docker compose -p rdlab -f lab/compose.yaml [-f lab/compose.nat.yaml] down -v`

## Cases

All expectations live in the `CASES` table at the top of `run-lab.sh` (`;`-separated:
name ; topology ; server ; clients ; hbbs env ; controller options ; expected winner ;
expected offer ; hbbs log check). `lab/run-lab.sh --list` prints it. `--expect CASE=REGEX`
overrides an expected winner for one run. Clients are `<controller>/<controlled>`, each
`latest` (= `-v`, default 1.5.0) or `old` (1.4.9). "direct" below means
TCP|UDP|IPv6|WebRTC.

| case | topo | server | clients | controller options | expected |
|---|---|---|---|---|---|
| `baseline` | flat | baseline | latest | defaults (non-public server: UDP/IPv6/WebRTC off) | TCP, offer TCP |
| `baseline-allopts` | flat | baseline | latest | all four `enable-*` = Y | TCP (the original hbbs only gives TCP/relay) |
| `modern-off` | flat | modern | latest | udp/ipv6/webrtc = N | TCP, like baseline |
| `modern-switches-off` | flat | modern, `SECURE_TCP=N IPV6_PUNCH=N WEBRTC=N UDP_PUNCH=N` | latest | all Y | TCP like baseline-allopts, hbbs log without `secured, version` |
| `udp-only` / `ipv6-only` / `webrtc-only` | flat | modern | latest | `enable-tcp-punch=N` + that one = Y | UDP / IPv6 / WebRTC; webrtc-only also needs `secured, version 1` in the hbbs log |
| `all-on` | flat | modern | latest | all Y | direct |
| `old-client` | flat | modern | old | defaults | TCP |
| `baseline-old-allopts` / `old-client-allopts` | flat | baseline / modern | old | all Y | direct |
| `mixed-old-controlled` / `mixed-old-controller` | flat | modern | latest/old, old/latest | all Y | direct |
| `client-1.4.0` / `baseline-client-1.4.0` | flat | modern / baseline | 1.4.0 | defaults | TCP (1.4.0 logs no type: Relay if hbbr paired the session, else TCP) |
| `mixed-1.4.0-controller` / `mixed-1.4.0-controlled` | flat | modern | 1.4.0/latest, latest/1.4.0 | defaults / all Y | TCP / direct |
| `webrtc-old-controlled` / `baseline-webrtc-old-controlled` | flat | modern / baseline | latest/old | tcp-punch N, webrtc Y | connects; Relay allowed |
| `lan-baseline` / `lan-default` | lan | baseline / modern | latest | defaults | TCP, hbbs log `Fetch local addr` |
| `lan-webrtc` | lan | modern | latest | tcp-punch N, webrtc Y | WebRTC, hbbs log `WebRTC offer` |
| `lan-all-on` | lan | modern | latest | all Y | direct |
| `nat-baseline` | nat | baseline | latest | defaults | Relay (strict conntrack, see Topologies) |
| `nat-udp-only` / `nat-ipv6-only` / `nat-webrtc-only` | nat | modern | latest | as the flat ones | UDP / IPv6 / WebRTC |
| `nat-all-on` | nat | modern | latest | all Y | UDP, IPv6 or WebRTC |

hbbs feature switches go into the hbbs env field (space separated `KEY=VALUE`); they end
up in `lab/out/<run>/<case>/hbbs.env`, which also sets `RUST_LOG=debug`.

Pass criterion: the controlling client's first
`<elapsed> used to establish <TYPE> connection` line matches the expected winner; where
set, one of its `#1 <transports> punch attempt` lines matches the expected offer and the
hbbs log check matches; and the session comes up (`peer info` within 60 s). The KX column
lists the key exchange versions hbbs completed with any client in that case
(`secured, version N`).

hbbr (unchanged upstream code) has a pairing race: when both relay requests of a
session arrive at the same moment, neither is paired (`make_pair_` in
`src/relay_server.rs`). A case that fails and shows this race in the hbbr log is run
once more; the table then notes "2nd attempt after hbbr pairing race", and the first
attempt stays in `lab/out/<run>/<case>.attempt1/`.

## Topologies

- `flat` (default): hbbs (198.18.10.10), hbbr (.11), STUN (.3), controlled (.21) and
  controller (.22) on one dual-stack "WAN" network `rdlab_wan`
  (198.18.10.0/24, 2001:db8:10::/64, internal). No NAT, so every punch is trivial; this
  shows which transport the client and server *negotiate*.
- `lan`: both clients in one LAN (192.168.20.0/24, 2001:db8:20::/64) behind one
  router, so hbbs sees the same public IPv4 for both and takes its same-intranet path
  (FetchLocalAddr). Client-to-client traffic stays in the LAN, so the router's conntrack
  mode does not matter (no `--ct-liberal` needed).
- `nat`: each client in its own LAN (192.168.11.0/24 / 192.168.12.0/24 plus
  2001:db8:11::/64 / 2001:db8:12::/64) behind a router container: IPv4 MASQUERADE,
  IPv6 routed with a stateful firewall, unsolicited WAN input dropped. With Linux'
  default strict TCP conntrack the TCP punch fails and the session ends up on the relay;
  `--ct-liberal` makes the routers accept it (see NOTES); then use
  `--expect nat-baseline=TCP`.

## Logs

`lab/out/<run-id>/<case>/`:

- `case.txt`: images, options, hbbs key, controlled ID, result.
- `established.txt`, `punch-attempts.txt`: the relevant controller log lines.
- `controller/logs/RustDesk/<connect|flutter_ffi>/*.log`: controlling client log
  (sciter writes to `connect/`, Flutter to `flutter_ffi/`).
- `controlled/logs/RustDesk/server/*.log`: controlled side (`rustdesk --server`).
- `controller/rustdesk/`, `controlled/rustdesk/`: the clients' config dirs
  (`RustDesk2.toml`, `RustDesk_local.toml`, `peers/<id>.toml`).
- `*.stdout.log`: container output of hbbs, hbbr, stun, both clients (and routers).
- `hbbs.env`: environment passed to hbbs.

Build logs: `lab/out/build-*.log`.

## Changing the client version

`lab/run-lab.sh -v 1.4.9 baseline` uses client 1.4.9 for all "default" cases. Missing
packages are downloaded from
`https://github.com/rustdesk/rustdesk/releases/download/<ver>/rustdesk-<ver>-x86_64[-sciter].deb`
into `lab/.cache/<ver>/`, and `rdlab-client:<ver>-<variant>` is built. A case can pin
its own version in the client field (`1.4.9`, or `<controller ver>/<controlled ver>`).
