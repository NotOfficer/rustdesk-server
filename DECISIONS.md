# Decisions

Decisions taken during the work on `feature/modern-transports`, with the
reason. Newest at the bottom of each section.

## Setup

- **Client reference: tag `1.5.0`** (latest stable release on 2026-10-24),
  hbb_common `229b904508364c8997aad0fb5af57effac859f60`. Older tags compared:
  `1.4.0` (hbb_common `6e556f7`) and `1.4.9` (hbb_common `7e1c392`).
- **hbb_common bumped to exactly `229b904`.** The build needed two
  adaptations: `IdPk` moved from `message.proto` (now in the client's
  `libs/base`) into `rendezvous.proto`, and `ed25519` had to be pinned to
  1.5.3 because 1.5.0 accepts `signature` 2.x, which it does not compile
  against.
- **Baseline for "original hbbs"** is upstream `master` at `873230e`, the
  commit this branch starts from (version 1.1.17, newer than the latest
  release 1.1.16). The user agreed.
- **Git**: the user asked to fork the repo. `origin` is now
  `NotOfficer/rustdesk-server`, `upstream` is `rustdesk/rustdesk-server` with
  pushing disabled. The feature branch may be pushed to the fork; no PRs.
- **Docker** is only usable via `sudo`; the user allowed `sudo docker` for the
  lab (compose project `rdlab`) and asked not to change the Docker daemon
  configuration. Rust tooling missing on the host (clippy, musl target) is
  used from the official `rust:1.93` images instead of installing packages.
- **Client packages for the lab** are the official `.deb` files from GitHub,
  not self-built clients: they are what users run.

## Corrections to the prior research

Checked against the client at 1.5.0 (details in `docs/modern-transports.md`).

1. **Key exchange version.** 1.5.0 negotiates version **1**: the server sets
   bit 255 of its signed X25519 key and must send
   `signed_params = sign("rdkx-params" || KxParams{pk, version})`; both sides
   then derive one key per direction. PR #706 only implements version 0, which
   1.5.0 still accepts. There is no rustdesk-server PR #709
   (`git fetch upstream pull/709/head` fails); version 1 came with hbb_common
   PR #614 (`kx-v2`).
2. **When clients use the exchange.** The controller uses
   `secure_tcp_required` only for a round that carries a WebRTC offer, and
   `secure_tcp` only when it has a key **and** an access token or a switch
   code (not merely "signed in"). The controlled side uses
   `secure_tcp_required` for TCP replies carrying a WebRTC answer and for its
   ICE connection. Everything else stays plaintext.
3. **WebRTC vs TCP race.** A round that carries an offer never does a TCP
   punch. TCP runs in a separate offer-less fallback round, raced against the
   preferred round. The practical advice (`enable-tcp-punch=N` to test WebRTC)
   still holds.
4. **ICE routing.** Controller → hbbs candidates carry the target `id` and
   come over the controller's punch connection. Controlled → hbbs candidates
   carry `socket_addr` (the controller's address from `PunchHole`) with an
   empty `id`, over a separate encrypted TCP connection that must stay open
   for all candidates. Both peers match candidates only by `session_key`
   (the DTLS fingerprint of the offer).
5. **UDP `PunchHoleSent`.** The controlled side sends it three times from a
   fresh UDP socket, which it then uses for KCP. It can carry the WebRTC
   answer and `socket_addr_v6` in plaintext. The original server's (now
   removed) UDP handling replied over UDP to the controller's TCP address,
   which was wrong anyway; the controller expects the reply on its TCP
   connection.
6. **1.4.9 and `udp_port`.** 1.4.9 also takes `udp_port` from a STUN probe,
   so it sent `udp_port > 0` even to the original hbbs, which dropped it.
   1.5.0 takes it only from the UDP `TestNatResponse`.
7. **`FetchLocalAddr`** has no WebRTC field. The same-intranet case for an
   offer-carrying request is solved by sending a `PunchHole` instead (see
   below).
8. **IdPk.** The server keeps signing `IdPk { id, pk }` only; the new fields
   `dtls_fingerprint` and `kx_version` belong to the peer-signed identity and
   are ignored by the controller on the server-signed path
   (`decode_id_pk`, common.rs:2270).

## Design decisions

- **Key exchange on every TCP connection on 21116**, sent right after accept,
  as the clients expect. Plaintext first frames are still accepted, because
  most connections never do the exchange. Only the first frame may be a
  `KeyExchange`. Requires the server's signing key (always present unless
  `-k` is a plain public key); otherwise no exchange is offered.
- **WebSocket stays unchanged.** No key exchange, no new fields and no
  `IceCandidate` handling on 21118, as required ("WebSocket clients behave
  unchanged"). WebRTC over WebSocket would be possible by client design but
  is left out on purpose.
- **Same intranet with an offer → `PunchHole`.** `FetchLocalAddr` cannot
  carry the offer. The offer-less fallback round still gets
  `FetchLocalAddr`, so the LAN TCP path is unchanged.
- **Forced relay strips the new fields.** With `ALWAYS_USE_RELAY=Y`, or the
  LAN/WAN mismatch that forces relay, `udp_port`, `socket_addr_v6` and the
  offer are not forwarded, because the controlled side would start IPv6 and
  WebRTC attempts even when its NAT type says relay. This is identical to the
  original server, which never forwarded them.
- **Session binding of ICE.** The route stores the offer's DTLS fingerprint
  (the client's `session_key`). Candidates with another key are dropped. If
  the fingerprint cannot be read from the offer, the offer is not forwarded
  (the client falls back to the other transports). Conservative choice
  against ICE injection; a future client with a different envelope format
  would lose WebRTC rather than get an unprotected route.
- **UDP `PunchHoleSent` source check.** Besides the route, the datagram's
  source IP must match the controlled peer's registered IP (IPv6: same /64,
  because of privacy addresses) for `is_udp`. This stops a third party that
  knows a route from steering the controller's UDP probes to another host.
  A mismatch (NAT pools without paired addresses, multi-WAN) still gets a
  response without `is_udp` (review finding UDP-3), so the WebRTC answer
  arrives and the controller can relay instead of waiting for retries.
- **Route capacity exhausted → degrade.** If no route can be opened, the
  request is forwarded like the original server would (no `udp_port`, no
  offer).
- **TCP frame limit 64 KiB on 21116.** The legacy codec accepted frames up to
  1 GiB. The largest legitimate rendezvous message is a `PunchHoleRequest`
  with an offer and a token (a few KiB).
- **Key exchange offered after 500 ms of silence, longer on slow links**
  (review findings KX-1, FINAL-1-1 and FINAL-2-3; the wait is 4 × the
  handshake round trip + 100 ms, 500 ms to 3 s, read with `TCP_INFO` on
  Linux). The controlled side's plaintext connections close right after
  sending; an unread offer would turn that close into a TCP reset. 100 ms was
  shorter than a retransmission of a lost first segment (200 ms minimum
  RTO), so 500 ms. Costs key-exchanging clients (WebRTC rounds, signed-in
  clients, the controlled side's answer and ICE connections) 500 ms per
  connection.
- **IPv6 only from 1.5.0 controllers** (review finding IPV6-3). 1.4.x
  controllers send `socket_addr_v6`, but their IPv6 attempt can hold up the
  relay fallback for up to 20 s, a regression against the original server.
  1.4.x controllers therefore keep the old behavior; 1.4.x controlled peers
  still take part in IPv6 with a 1.5.0 controller. The version is parsed
  with a local, overflow-free function, because hbb_common's
  `get_version_number` multiplies without bounds on untrusted input.
- **No UDP or IPv6 when the controller sets `force_relay`** (IPV6-1). The
  offer still travels with `force_relay`, because the WebRTC peer then uses a
  relay-only ICE policy.
- **IPv6 targets: global unicast only, rate-limited per controller IP**
  (IPV6-2). A same-/64 check against the controller's TCP address was not
  added: most controllers reach hbbs over IPv4, and multi-homed hosts would
  lose IPv6.
- **Offer dropped from the third unanswered request** (review finding
  WEBRTC-2). A controlled 1.5.0 client without the server's key cannot
  encrypt its answer and replies nothing. A controller with WebRTC on and TCP
  punch off would then fail where the original server relayed. Re-sent
  requests without any reply lose the offer, so the peer answers with a
  plain punch. Cost: if the first `PunchHole` datagram is lost twice, that
  attempt goes without WebRTC.
- **Docker build without a `target` cache.** A shared cargo `target` cache
  mount reuses stale output when another source tree with older file times is
  built (found by the lab, which builds the baseline from `git archive`).
  Only the registry and git caches are kept.
- **Route table exhaustion.** Routes are capped per controller IP/64 (64)
  and in total (10,000), and live only as long as the controller's TCP
  connection. Filling the table therefore needs thousands of open
  connections from many /64s, the same cost as exhausting the server's
  connections in general. A full table fails closed: no UDP/WebRTC for new
  requests, which fall back to TCP/relay.

## Lab and tests

- **Client package**: the sciter `.deb` is the default in the lab (one small
  process, no GL under Xvfb); the Flutter package also passed the baseline
  case. Both are official 1.5.0 release assets.
- **No internet in the lab networks** (`internal: true`). A local coturn
  answers STUN, and the default STUN host names from hbb_common
  `src/webrtc.rs` are pointed at it with `extra_hosts`. No TURN server, so a
  WebRTC win in the lab is always a direct connection.
- **IPv6 in the lab** uses `2001:db8::/32` addresses (accepted by the client
  as global, 2000::/3) in internal Docker networks; the client containers get
  `NET_ADMIN` only to add a default route inside their own network
  namespace, which the client's IPv6 detection needs.
- **NAT topology** (bonus): with Linux's default strict TCP connection
  tracking, the router containers drop the TCP punch, so TCP-only cases end
  on the relay there; `--ct-liberal` lets them through. Real home routers
  differ; the flat topology is the reference for "which transport is
  negotiated".
- **Differential test** `tests/compat.rs`: old-style requests are replayed
  against the original hbbs binary and this one, and the observed replies
  must match (defaults and all switches off).
- **Observed, not changed: hbbr pairing race.** When both relay requests of
  a session arrive at the same moment, `make_pair_` in `src/relay_server.rs`
  can leave both unpaired and the clients retry. Pre-existing upstream
  behavior, outside the scope ("hbbr only if really necessary").
- **TCP-connected sources skip the global NAT-test budget** (FINAL-1-2).
  Spoofed sources could empty it; a source with an open TCP connection is
  proven. The global limit is configurable (`UDP_TEST_NAT_LIMIT`), which also
  makes the behavior testable.
- **Switch values** (FINAL-1-3): `no`, `0`, `false`, `off` also turn a
  feature off, unknown values are logged, so a typo cannot silently leave a
  feature on without notice.
- **Limiter and route table fairness** (FINAL-2-1, FINAL-2-2): spoofed
  sources get no limiter entry while the global budget is empty; a full
  route table evicts the oldest route of the heaviest source; the IPv6
  forward limiter (non-spoofable, licensed sources) lets unknown sources
  pass when its table is full. Remaining: during a sustained flood,
  high-latency controllers lose UDP punching (fallback as with the original
  server).
- **Client 1.4.0 in the lab** (FINAL-2-4): official 1.4.0 package. Its log
  names no connection type, so the lab takes "Relay" from hbbr's
  `got paired` and "TCP" otherwise (the only direct type of 1.4.0).
