# Lab notes

Findings from building the transport lab, 2026-10-24. Client source: `/srv/repos/rustdesk`
at tag `1.5.0` (hbb_common `229b904`); 1.4.9 read with `git show 1.4.9:<path>`, nothing
checked out there.

## Files

| file | purpose |
|---|---|
| `run-lab.sh` | runner: images, case matrix (`CASES`), per-case stack, result table |
| `compose.yaml` | flat topology: hbbs, hbbr, coturn STUN, controlled, controller |
| `compose.nat.yaml` | overlay for the `nat` topology: two LANs, two routers, route helpers |
| `compose.lan.yaml` | overlay for the `lan` topology: one LAN with both clients, one router |
| `run-all-tests.sh` | levels 1-3 (cargo build/clippy, cargo test, images + lab matrix) |
| `docker/Dockerfile.client`, `docker/client-entrypoint.sh` | headless client image |
| `docker/Dockerfile.server-baseline` (+ `.dockerignore`) | baseline server build (see "Baseline build") |
| `docker/Dockerfile.router`, `docker/router.sh` | NAT router / route helper |

Docker objects created by the lab: compose project `rdlab` (containers `rdlab-*-1`,
networks `rdlab_wan`, `rdlab_lan`, `rdlab_lan1`, `rdlab_lan2`), images
`rustdesk-server-baseline:873230e`, `rustdesk-server-modern:dev` (run-all-tests),
`rdlab-client:<ver>-<variant>`, `rdlab-router:latest`, BuildKit cache mounts
`rdlab-cargo-registry`, `rdlab-cargo-git`, `rdlab-baseline-target`, and for
run-all-tests the containers `rdlab-rust-head` / `rdlab-rust-baseline` (removed at the
end) and the volumes `rdlab-cargo-registry`, `rdlab-cargo-git`, `rdlab-target-head`,
`rdlab-target-baseline` (kept as caches; `--purge-caches`). Teardown only uses
`docker compose -p rdlab down` and `docker network rm` on networks labelled
`com.docker.compose.project=rdlab`. No ports are published, and nothing else on the
host is touched.

## Client facts (1.5.0)

### Config files (Linux, root: `/root/.config/rustdesk/`)
- `RustDesk.toml` = `Config`: `enc_id`, `password` (permanent password, stored hashed:
  sha256(pw+salt), then encrypted; see `libs/hbb_common/src/config/permanent_password.rs`),
  `salt`, `key_pair`. Hand-writing the password is impractical, so the lab uses the CLI.
- `RustDesk2.toml` = `Config2`: top-level `rendezvous_server`, `nat_type`, `serial`, plus
  `[options]` with `custom-rendezvous-server`, `relay-server`, `key`, `ice-servers`, ...
  Written by the entrypoint before the first start.
- `RustDesk_local.toml` = `LocalConfig`: `[options]` with the transport switches
  `enable-tcp-punch`, `enable-udp-punch`, `enable-ipv6-punch`, `enable-webrtc`
  (keys in `libs/base/src/config/keys.rs:129-134`), and `relay-fallback-delay`.
  Values are `'Y'`/`'N'`; `option2bool` for `enable-*` means "!= N".
- `get_local_option` (`src/common.rs:1206`): an *empty* `enable-udp-punch` /
  `enable-ipv6-punch` / `enable-webrtc` reads as `N` unless the rendezvous server is
  `rustdesk.com`/`*.rustdesk.com` (`is_public`). `enable-tcp-punch` is on by default.
  `tcp_punch_allowed()` (`src/client.rs:351`) also forces TCP punch on when the other
  three are all off.
- 1.4.9 has only `enable-udp-punch` / `enable-ipv6-punch` (same "N for non-public"
  rule), no `enable-webrtc` / `enable-tcp-punch`.
- `peers/<id>.toml` (controller) holds `direct_failures`; every case uses fresh client
  containers, so it always starts at 0.

### CLI (`src/core_main.rs`)
- `--password <pw>`, `--option`, `--set-id`, `--config` need `is_installed()` (exe path
  starts with `/usr`, true for the .deb) **and** root, and go over IPC
  (`/tmp/RustDesk-0/ipc`) to a running `--server` process. `--password` prints `Done!`.
  `--get-id` asks the same IPC server.
- `--server`: runs the service part (registration, listening) for the current user;
  works as root in a container with `DISPLAY=:0` (Xvfb). It spawns `--tray`, which
  panics without `libayatana-appindicator3-1` (installed in the image; harmless anyway).
- Connect: sciter `rustdesk --connect <id> <password>` (password is positional,
  `src/ui.rs:126-150`); Flutter `rustdesk --connect <id> --password <pw>` (converted into a
  `rustdesk://connection/new/<id>?password=...` link, sent over dbus to a running instance,
  otherwise handled by the process itself; `core_main_invoke_new_connection`).

### Logs
- `~/.local/share/logs/RustDesk/<name>/rustdesk_rCURRENT.log`, `<name>` = first CLI arg
  without dashes (`connect`, `server`, `password`, `get-id`, ...); Flutter sessions log to
  `flutter_ffi/` (`hbb_common::init_log`, `Config::log_path`). Release builds log at
  `debug` level.
- Offered transports (`src/client.rs:968`):
  `#<n> <transports> punch attempt with <local addr>, id: <peer id>`, where
  `<transports>` is `+`-joined from `UDP`, `TCP`, `IPv6`, `WebRTC` or `Relay` when none.
- Winner: `<elapsed> used to establish <TYPE> connection with <transports> punch`
  (`src/client.rs:1566`, after PunchHoleResponse) or `<elapsed> used to establish <TYPE>
  connection` (`:1187`, after RelayResponse). `<TYPE>` is `TCP`, `UDP`, `IPv6`, `WebRTC`,
  `Relay` or `WebSocket`. 1.4.9 uses the same two formats (`git show 1.4.9:src/client.rs`).
- Caveats:
  - The line is logged *before* `secure_connection`. If a WebRTC winner fails
    the secure handshake, the client silently falls back to relay without another
    "used to establish" line.
  - io_loop relabels WebRTC over IPv6 as `WebRTC/IPv6` only for the UI; that is not
    logged.
  - A session that fails after the transport is up and reconnects logs more lines, so
    `established.txt` lists all of them, and the verdict uses the first.
- Session established: `peer info supported_encoding` (INFO) / `handle_peer_info` (DEBUG).
- With UDP or WebRTC active and TCP punch allowed, the controller runs **two rounds in
  parallel**: the preferred round (UDP/IPv6/WebRTC, no TCP when it carries an offer) and a
  pure TCP fallback round (`src/client.rs:536-586`). So there can be two `#1` lines; the
  OFFERED column lists all distinct ones.

### IPv6 detection
- `test_ipv6` (`src/common.rs:2653`): (1) synchronous route probe: UDP `connect` to
  `[2001:4860:4860::8888]:53`, accept the source address if in `2000::/3`;
  (2) background STUN probe of the 4 default STUN servers (`default_stun_servers()`,
  not `ice-servers`), any v6 result accepted, 5 s cap, cached 60 s.
- An `internal: true` Docker network has **no default route** (only the kernel's
  `unreachable default`), so (1) fails with "Network is unreachable", and the address
  only arrives via STUN a few ms *after* a freshly started `--connect` process has
  already sent its PunchHoleRequest. So IPv6 was never offered. Fix: the client
  entrypoint adds an on-link `default dev eth0` IPv6 route (needs `cap_add: NET_ADMIN`,
  container netns only). Afterwards: `Found public IPv6 address locally: [2001:db8:10::22]:0`.
- `2001:db8::/32` passes the client's filter (only `2000::/3` is checked).

### STUN
- Default ICE/STUN servers (`libs/hbb_common/src/webrtc.rs:164`):
  `stun.cloudflare.com:3478`, `stun.l.google.com:19302`, `stun.antisip.com:3478`,
  `stun.nextcloud.com:443`. These are the only STUN hosts in the client (also used by the IPv6
  probe; the NAT-type test uses hbbs ports 21115/21116, not STUN).
- The lab runs a single coturn (`--stun-only`, main port 3478 plus `--aux-server` on
  19302 and 443, v4 and v6), and `extra_hosts` maps all four names to 198.18.10.3 and
  2001:db8:10::3 in both clients. coturn's `--no-dtls` / `--no-cli` no longer exist in
  the current image. No TURN on purpose, so a WebRTC win must be a direct pair.
  Alternative: the Config2 option `ice-servers` (only affects WebRTC, not the IPv6 probe).

## Headless client image
- ubuntu:24.04 + the .deb (apt resolves its deps) + `xvfb dbus-x11 procps iproute2
  libgl1-mesa-dri libegl1 libgles2 libayatana-appindicator3-1`. `dbus-launch` provides a session bus.
- **Variant: sciter is the default** (`--variant flutter` selectable). Both were tested
  on the `baseline` case and both PASS in ~13 s. Sciter is preferred because it runs a
  single small process per session with no GL. Its log goes to `connect/` and the
  password is a plain CLI argument. Flutter needs software GL (Mesa) and multi-window
  plumbing, and logs dbus/wakelock exceptions (`org.freedesktop.ScreenSaver`). Both
  packages contain the same Rust core, so the transport logic is identical.
- Harmless noise: `AT-SPI` warning, `Cannot load libcuda.so.1`, `/dev/fuse not found`
  (clipboard FUSE), rdev `Failed to send grab command`, `xkbcomp` warnings,
  `HTTP POST to http://<hbbs>:21114/api/...` failures (no API server; the client then
  tries its TCP proxy via 21116).
- Images carry a label `rdlab.client.rev` (hash of Dockerfile + entrypoint);
  `run-lab.sh` rebuilds when it does not match.

## Servers
- hbbs: `hbbs -r 198.18.10.11:21117`, no `-k`. The default key arg `-` generates
  `id_ed25519{,.pub}` in `/root` on first start and *enforces* that key (licence_key
  check). Every case starts a fresh hbbs container, so every case gets a fresh key. The runner reads it
  from the log line `Key: <base64>`, so no volume or bind mount is needed (the image is
  `FROM scratch`, no shell).
- hbbr: plain `hbbr`; without `-k`/`KEY` it does not check the key.
- hbbs gets `RUST_LOG=debug` via the per-case env file, which logs
  `Punch hole "<id>" <peer addr> request from <addr>` and `TCP punch hole response ...`.
  Feature switches go into the same file (4th field of a case).
- Clients address the servers by IPv4 literal (198.18.10.10/.11), so the rendezvous
  path is deterministic v4. IPv6 is only used for the IPv6 punch/WebRTC.

### Baseline build: problems with `docker-classic/Dockerfile.multistage`
1. **Shared cargo target cache.** The Dockerfile's cache mounts use default ids
   (= mount path), so `/src/target` is shared with every other build of that Dockerfile,
   e.g. the modern image. `git archive` gives files the commit time, older than the
   cached artifacts, so cargo (mtime-based freshness) reused the modern tree's hbb_common
   build-script output and failed with `unresolved import protos::message`. The reverse
   can happen too: an older-mtime tree built after another tree in the same cache may
   silently reuse that tree's artifacts. The lab therefore builds the baseline with
   `lab/docker/Dockerfile.server-baseline`, identical except for cache ids `rdlab-*`, and
   touches the exported files. The original Dockerfile is not modified.
   **Risk for the server engineer:** builds of different trees through
   `Dockerfile.multistage` share one target cache.
2. **`.dockerignore` excludes `db_v2.sqlite3*`**, but `.env` sets
   `DATABASE_URL=sqlite://./db_v2.sqlite3` and `src/database.rs` uses `sqlx::query!`
   macros that open it at compile time. Error: `error returned from database: unable to
   open database file`. The lab uses a Dockerfile-specific ignore file
   (`Dockerfile.server-baseline.dockerignore`) that keeps `db_v2.sqlite3` and drops
   only `db_v2.sqlite3-*`. Update 03:15: the server engineer's working-tree
   `.dockerignore` no longer excludes `db_v2.sqlite3`, and `rustdesk-server-modern:wip`/`:dev`
   were built from it. `--build-modern` is implemented but I did not run it.
- Baseline build time: about 2.5 min (cold cache, 8 jobs).

## Results so far

Flat topology, client 1.5.0 sciter. Proof lines from
`out/20261024-025046/{baseline,baseline-allopts}`:

```
baseline (rustdesk-server-baseline:873230e, default options):
  INFO [src/client.rs:968] #1 TCP punch attempt with 198.18.10.22:..., id: ...
  INFO [src/client.rs:1037] TCP Hole Punched ... = 198.18.10.21:...
  INFO [src/client.rs:1566] ...µs used to establish TCP connection with TCP punch
  hbbs: DEBUG [src/rendezvous_server.rs:800] Punch hole "<id>" [::ffff:198.18.10.21]:... request from [::ffff:198.18.10.22]:...
baseline-allopts (all four enable-* = Y):
  DEBUG Found public IPv6 address locally: [2001:db8:10::22]:0
  INFO #1 TCP punch attempt ...                 (only the TCP fallback round reaches the punch)
  INFO ... used to establish TCP connection with TCP punch
  DEBUG UDP NAT test to 198.18.10.10:21116 finished: ..., port=0, ..., success=false
```

The full matrix was also run with the baseline image standing in for the modern one
(`-s rustdesk-server-baseline:873230e`, `out/20261024-025209`), to show every case
runs end to end:

| case | offered | got | note |
|---|---|---|---|
| baseline, baseline-allopts, modern-off, all-on | TCP | TCP | PASS |
| old-client (1.4.9) | TCP | TCP | PASS |
| udp-only | Relay | Relay | old hbbs does not answer the new UDP NAT test (`port=0`) |
| ipv6-only | IPv6 | Relay | v6 offered, but old hbbs does not forward `socket_addr_v6` |
| webrtc-only | Relay | Relay | 18 s: `WebRTC signalling to 198.18.10.10:21116 cannot be encrypted, punching without WebRTC: deadline has elapsed` (no KeyExchange from old hbbs) |

### Modern image (`rustdesk-server-modern:wip`, sha256:edff9e50..., created 03:10:25Z)

Flat, `out/20261024-030550`: **all 8 cases PASS** (13 s each):

| case | offered (`#1 ... punch attempt`) | got (`used to establish ...`) |
|---|---|---|
| modern-off | TCP | TCP connection with TCP punch |
| udp-only | UDP | UDP connection with UDP punch |
| ipv6-only | IPv6 | IPv6 connection with IPv6 punch |
| webrtc-only | WebRTC | WebRTC connection with WebRTC punch (`peer connection state: connected`, `WebRTC punch secure_connection ok`; no TURN in the lab, so a direct pair) |
| all-on | TCP and UDP+IPv6+WebRTC (two rounds) | TCP (fallback round won) |
| old-client (1.4.9) | TCP | TCP connection with TCP punch |

NAT topology, strict conntrack (`out/20261024-032131`): udp-only, ipv6-only,
webrtc-only PASS; all-on got UDP. All TCP-only cases (baseline, baseline-allopts,
modern-off, old-client) end on Relay (expected with strict conntrack, see below).
Several of them show SESSION=no within the 5 s settle time, probably relay retries
(hbbr pairing race below). With `--ct-liberal` (`out/20261024-032356`): all 8 PASS;
all-on got IPv6.

## Bug in hbbr (baseline, and still in modern HEAD f03c7ff: `src/relay_server.rs:471-492`)
`src/relay_server.rs` `make_pair_`: `PEERS.lock().await.remove(&uuid)` and the later
`PEERS.lock().await.insert(uuid, stream)` take the lock separately. When both sides'
`RequestRelay` arrive at the same moment (easy on a sub-ms lab network), both miss,
both insert (the second overwrites the first), nobody is paired, and the client
retries after a timeout. Seen in `udp-only` (stand-in run): two
`New relay request <uuid>` lines per uuid without `got paired`, the 3rd attempt worked
(`direct_failures updated to 1`).

## NAT topology (`--topology nat`)
- Routers: alpine + iptables-nft. IPv4 MASQUERADE; IPv6 routed (no NAT66), stateful
  firewall (`ESTABLISHED,RELATED` in, LAN->WAN out); unsolicited packets to the router's
  WAN address are dropped (`WAN_INPUT=drop`). Docker puts its gateway on `.1`, so the
  routers are `.254`. The clients get default routes via the router (`RD_GW4/6`). WAN
  containers (hbbs, hbbr, stun) need return routes to the LAN v6 prefixes; one-shot
  helpers with `network_mode: service:<x>` + NET_ADMIN add them. These depend on
  `br_netfilter` being *not* loaded on the host (it is not); with it Docker's
  internal-network isolation rules would drop routed traffic.
- Result, baseline: TCP hole punching **fails** with Linux' default strict conntrack.
  The controlled side's punch SYN creates the mapping in router1, the controller's SYN
  is accepted (`SYN_SENT2`), but the controlled listener's SYN-ACK has a different ISN
  than the SYN conntrack saw, so it is `INVALID` and dropped (counter on the explicit
  INVALID rule). Result: Relay. With `--ct-liberal`
  (`net.netfilter.nf_conntrack_tcp_be_liberal=1`): TCP, PASS. With
  `RDLAB_WAN_INPUT=reject` (kernel RSTs) the punch fails immediately.
- Expectations in the case table are for `flat`; use `--expect` for NAT runs.

## Assumptions / conservative choices
- `old-client`: 1.4.9 on **both** sides (case field `1.4.9`; `1.4.9/1.5.0` would mix).
- Modern cases pass no hbbs env (feature switches assumed default-on with `=N` opt-outs).
- `all-on` accepts any direct type (on a flat network the fastest one wins; it is
  timing-dependent). Only Relay fails.
- Permanent password `rdlab-Passw0rd` (`RDLAB_PASSWORD` in compose).
- One lab run at a time (`flock` on `lab/out/.lock`; fixed IPs and project name).
- Cases use separate stacks; `--keep` only keeps the last one.

## Open issues
- `SESSION` is informative; a WebRTC secure-handshake fallback to relay would not be
  caught by the "used to establish" line alone. Compare with the controlled side's log
  (`Connection opened from <addr>`) if needed.
- `--build-modern` is implemented but not run (the brief said not to build the working tree).

## Follow-up (2026-10-24, second round)

### Changes
- Default modern image is now `rustdesk-server-modern:dev`.
- Case table is `;`-separated and has a topology per case (`flat`, `lan`, `nat`) and an
  hbbs log check (`regex`, `!regex` = must not appear). One `run-lab.sh` invocation runs
  cases of all topologies; each case gets its own stack with the right overlay. Selection
  by case or topology name (`run-lab.sh lan`). `--topology` still forces one topology.
- Every case now also requires the session (peer info within `SESSION_WAIT`=60 s after the
  transport line); before it was informative only. 60 s, not 30 s: the hbbr pairing race
  (below) makes the client wait 18 s and reconnect, and a WebRTC offer against the old
  hbbs adds another 18 s key-exchange wait per round. `baseline-webrtc-old-controlled`
  failed once with 30 s (`all-20261024-035931`). The NOTE column now reports
  `N connection rounds` and `hbbr pairing race: K uuid(s) never paired`, so the race stays
  visible even when the case passes.
- KX column: distinct `secured, version N` lines in the hbbs log of the case. It covers
  *all* TCP connections of both clients (the 1.5.0 controlled side also completes the
  exchange), so `modern-off` shows v1 although the controller did not need it; 1.4.9
  clients show v0. The real proof is the `webrtc-only` check (`secured, version 1` required).
- `--out DIR` and `results.tsv` (tab separated, one line per case) for run-all-tests.
- Teardown removes every container labelled with the project, so a stack from another
  overlay (routers) is never left behind.
- The lab's baseline Dockerfile copy is kept (it is proven and its caches are separate);
  the upstream `Dockerfile.multistage` no longer has a target cache, so it would work too.

### Bug found in my router script (fixed)
`router.sh` runs with `set -e`; `add_routes` ended its loop with `[ -n "$r" ] && ip ...`,
which returns 1 for an empty `ROUTES6` (the lan router has none). The script died before
the firewall/`ready` step, the LAN had no working gateway, and the first lan run failed
with `Failed to connect to 198.18.10.10:21116: Please try later` in the controller log.
Now `if ...; then ...; fi`.

### lan topology
One router (198.18.10.31 / 192.168.20.254), both clients in 192.168.20.0/24 +
2001:db8:20::/64. hbbs sees both peers from 198.18.10.31, so `same_intranet` is true.
- `lan-default` (modern) / `lan-baseline`: hbbs logs `Fetch local addr "<id>" ... request
  from ...`; the controller gets `TCP Hole Punched` with the peer's LAN address and
  `used to establish TCP connection with TCP punch`.
- `lan-webrtc`: the modern hbbs sends a PunchHole with the offer instead
  (`WebRTC offer (673 bytes) from ... to "<id>"`), result WebRTC.
- `lan-all-on`: TCP won.
- No `--ct-liberal` needed: client-to-client packets never cross the router; only the
  rendezvous/STUN/relay traffic is NATed.

### Results of the new cases (flat, `out/20261024-034451`; lan `out/20261024-035806`)
| case | offered | got | KX | note |
|---|---|---|---|---|
| modern-switches-off | TCP | TCP | - | no key exchange in the hbbs log, like baseline-allopts |
| baseline-old-allopts | TCP | TCP | - | 1.4.9 both sides, original hbbs |
| old-client-allopts | TCP | TCP | v0 | 1.4.9 both sides, modern hbbs; no UDP/IPv6 offer (see below) |
| mixed-old-controlled | TCP | TCP | v0 | 1.5.0 -> 1.4.9 |
| mixed-old-controller | TCP | TCP | v1 | 1.4.9 -> 1.5.0 |
| webrtc-old-controlled | WebRTC | Relay | v0,v1 | 1.4.9 cannot answer the offer; TCP punch off -> relay, session OK (11 s) |
| baseline-webrtc-old-controlled | Relay | Relay | - | 49 s: offer round waits 18 s for a key exchange the old hbbs never sends |
| lan-baseline / lan-default | TCP | TCP | - / v1 | `Fetch local addr` in hbbs log |
| lan-webrtc | WebRTC | WebRTC | v1 | `WebRTC offer` in hbbs log |
| lan-all-on | TCP | TCP | v1 | |
| nat-baseline | TCP | Relay | - | strict conntrack |
| nat-udp-only / nat-ipv6-only / nat-webrtc-only | UDP / IPv6 / WebRTC | same | v1 | |
| nat-all-on | TCP and UDP+IPv6+WebRTC | UDP | v1 | |

1.4.9 with all options Y offers only `TCP` against the modern hbbs. The UDP NAT test
does succeed (`UDP NAT test to 198.18.10.10:21116 finished: ..., port=33999, ..., success=true`),
but 1.4.9 does not wait for it. The PunchHoleRequest went out 0.2 ms earlier
(`#1 TCP punch attempt`), so `udp_port` was 0. 1.4.9's punch label only knows `UDP`/`TCP`;
its IPv6 address may still be in the request, but the modern hbbs forwards
`socket_addr_v6` only for controllers >= 1.5.0. Whether 1.4.9 ever gets UDP depends on that race
(not tested further). Expectation "direct" holds (TCP).

### run-all-tests.sh
- Rust containers: `rust:1.93-bookworm`, `--cpus 8 --memory 12g`, `CARGO_BUILD_JOBS=8`,
  default bridge network (crates.io/rustup). Sources are streamed in with `tar` and
  extracted with fresh mtimes (`tar -m`), and targets live in named volumes. Nothing is
  bind-mounted, so no root-owned files can appear on the host (this covers the
  coordinator's warning about root-owned `src/version.rs`). TMPDIR stays inside the
  container on disk (`/var/lib/docker`), not the host's tmpfs `/tmp`.
- Clippy compare: `--message-format=short`, diagnostics `file:line:col: warning|error: msg`
  reduced to `file: warning: msg`, duplicates numbered, `comm -23` HEAD vs baseline.
- Level 2 builds the baseline hbbs (debug, `/target-baseline`) in the HEAD container and
  runs `cargo test` with `HBBS_BASELINE_BIN`; `tests/compat.rs` is run again with
  `--nocapture` because the harness hides the output of passing tests, and the item fails
  on "skipped".
- Level 3 builds `rustdesk-server-modern:dev` from the HEAD export (same commit as levels
  1-2) and calls `run-lab.sh --build-baseline -s rustdesk-server-modern:dev --out
  <run>/level3/lab`.

### Clippy findings (level 1)
- HEAD and baseline both have a deny-by-default `clippy::unused_io_amount` error in the
  admin command handler (`stream.write(res.as_bytes())`, HEAD
  `src/rendezvous_server.rs:1418`, baseline `:1144`). Without `--cap-lints warn`, clippy
  stops at the lib and never lints bins/tests, in both trees. With the cap it becomes a
  warning and is compared like the rest.
- The only new diagnostics vs. the baseline are in `libs/hbb_common` (submodule
  69cea8d -> 229b904, upstream code needed by client 1.5.0): `src/sh.rs` "stripping a
  suffix manually" (file does not exist in 69cea8d) and `src/stream.rs` "large size
  difference between variants". They are reported as WARN; `--strict-clippy` makes them
  fail. My choice, because the branch cannot change upstream submodule code. New
  diagnostics in the server's own files fail.

### First full run-all-tests (`out/all-20261024-035931`, 19.5 min, old clippy logic)
Level 1 build PASS; clippy FAIL/FAIL for the two reasons above (fixed in the script
logic, not in the server); level 2 all 11 binaries PASS, compat ran against the original
hbbs (`original: ... / modern: ...` pairs identical); level 3 23 of 24 PASS,
`baseline-webrtc-old-controlled` FAIL (hbbr race + 30 s session wait, see above).

### Final full run-all-tests (`out/all-20261024-041947`, 9m46s with warm caches)
Level 1: build PASS, clippy PASS (HEAD 72 diagnostics, baseline 90), 0 new in server code,
2 new in hbb_common (WARN). Level 2: 11 test binaries PASS, compat ran against the
original hbbs. Level 3: 23/24 PASS; `baseline-webrtc-old-controlled` FAIL. Its hbbr log
shows the pairing race in **both** rounds, each request pair 1-6 µs apart, with no
`got paired`:
```
04:26:31.917362 New relay request c39557d8-... from [::ffff:198.18.10.21]:34723
04:26:31.917361 New relay request c39557d8-... from [::ffff:198.18.10.22]:34017
04:27:08.932360 New relay request 64148ec9-... from [::ffff:198.18.10.22]:42315
04:27:08.932366 New relay request 64148ec9-... from [::ffff:198.18.10.21]:40529
controller: 04:26:49 Connection closed: deadline has elapsed(115)  (and again 04:27:26)
```
Each round there costs 18 s (WebRTC key-exchange wait against the old hbbs) + 18 s
(relay deadline). The modern `webrtc-old-controlled` hit the same race in its first round
(`5478b928-...` unpaired) but recovered within 1 s (`Reset by the peer`, then
`86fd894f-... got paired`). The race code is unchanged in HEAD. I left the case failing
on purpose; it is a server (hbbr) bug, not a lab problem.
