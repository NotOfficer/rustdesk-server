# Reviews

Every finding of the independent reviews, whether it was valid, and the fix.
Reviewers were fresh agents (Opus 5.5, effort xhigh) that got only the
task's Context and Requirements, the branch and the diff range. Full reports
were kept outside the repo.

## Feature review: key exchange (`a409ba9..e2cb3a0`)

No critical or major findings. The reviewer confirmed the version 1 exchange
against the 1.5.0 client (`key_exchange()`, common.rs:2157), version 0 for
1.4.x, no exchange on 21115/console/WebSocket, and both switches.

| # | Finding | Verdict | Fix |
|---|---|---|---|
| KX-1 | The offer is sent before the client speaks. The controlled side's short-lived plaintext connections (TCP punch `PunchHoleSent`, `LocalAddr`) close without reading it, so the close becomes a TCP reset; on packet loss, or on a server OS that drops unread data on a reset (Windows), the message can be lost. Proven: 100/100 such closes were resets with `SECURE_TCP=Y`, 0/100 with `N`. | Valid (minor) | `3b49556`: hbbs waits up to 100 ms for the client to speak first and only then offers the exchange. Test `speaking_first_gets_no_offer`. |
| KX-2 | Docs overstate `SECURE_TCP=N` ("restores the old behavior", but the 64 KiB frame limit stays), and with `-k <public key>` the switch has no effect without any hint. | Valid (minor, docs) | `3b49556`: docs corrected, warning logged at start. Test `without_a_private_key_nothing_is_offered`. |
| KX-3 | Test gaps: no test where the controlled side drops its socket right after sending; `LocalAddr` and `RelayResponse` to an encrypted controller untested; the test client's handshake is a hand-written copy that could drift. | Valid (minor) | `3b49556`: tests for drop-without-read on the TCP punch, `LocalAddr` and `RelayResponse` paths. The copy is checked against the real 1.5.0 client in the lab (WebRTC needs version 1). |

## Feature review: IPv6 (`e2cb3a0..1eebd55`)

| # | Finding | Verdict | Fix |
|---|---|---|---|
| IPV6-1 | 1.4.x controllers send `socket_addr_v6` even when they force a relay (`/r`, force-always-relay, proxy; 1.4.9 client.rs:451). hbbs forwarded it without `force_relay`, so the controlled side started IPv6 and the session could go direct, bypassing the relay. | Valid (major) | `b5207d7`: no v6 and no `udp_port` when `force_relay` is set. Test `controller_forced_relay_drops_v6_and_udp`. |
| IPV6-2 | The v6 address is chosen by the controller and the peer sends punch probes to it; non-global targets (`::1`, `fe80::`, `ff02::1`, ULA, NAT64, v4-mapped) and unlimited requests were forwarded. | Valid (major) | `b5207d7`: only global unicast (2000::/3) with a port, at most 60 per controller IP in a burst (1/s). Residual risk documented in `docs/modern-transports.md` §3.1a. Tests `only_global_v6_targets_are_forwarded`, `v6_forwards_are_limited_per_controller`. |
| IPV6-3 | A 1.4.x controller's IPv6 attempt (`punch_udp`, 20 s) holds up the relay fallback (`select_ok` waits for all attempts) when IPv6 does not work; the original server never forwarded v6, so relay came sooner. Verified in 1.4.9 client.rs:4309 and common.rs:2508. | Valid (minor) | `b5207d7`: controller v6 only forwarded from version 1.5.0 on (overflow-free version parsing). Test `v6_only_from_controllers_since_1_5_0`. |
| IPV6-4 | Test gaps: no `force_relay` case, no non-global targets, `drop(ws.next())` did nothing. | Valid (minor) | `b5207d7`: tests added, dead line removed. |

## Feature review: WebRTC (`1eebd55..d86ba5a`)

No critical or major findings. The reviewer confirmed offer/answer placement,
the session key derivation (DTLS fingerprint of the offer), ICE routing in
both directions, the kept controller connection and the same-intranet case
against the 1.5.0 client.

| # | Finding | Verdict | Fix |
|---|---|---|---|
| WEBRTC-1 | The per-IP route limit compares full IPv6 addresses, so one host with a /64 could fill all 10,000 routes (WebRTC/UDP off for everyone, failing closed); the 120 s lifetime is per route, a new request opens a new one. | Valid (minor) | `5fcae81`: routes counted per /64 for IPv6. Docs state that a new request opens a new route. Unit test `route_limit_counts_an_ipv6_prefix_as_one_controller`. |
| WEBRTC-2 | A controlled 1.5.0 client without the server's key aborts before sending its answer (`secure_tcp_required(...)?`, rendezvous_mediator.rs:1172). With WebRTC on and TCP punch off on the controller (no fallback round), the connection fails where the original server relayed. Verified in the client code. | Valid (minor) | `5fcae81`: from the third re-sent request without any reply, the offer is dropped, so the peer answers with a plain punch and the controller can relay. Tests `unanswered_retries_lose_the_offer`, `answered_retries_keep_the_offer`. Documented that WebRTC needs the key on both sides. |
| WEBRTC-3 | The design doc claims a checked decoder for network addresses, but `PunchHoleSent`, `LocalAddr` and `RelayResponse` still used `AddrMangle::decode` (panics on overflow in debug builds; wraps in release). It also described `UDP_PUNCH` before it existed at that commit. | Valid (minor) | `5fcae81`: those three paths use the checked decoder. `UDP_PUNCH` was implemented in `88db509`. |
| WEBRTC-4 | Test gaps: no IPv6 controller (18-byte addresses), re-sent requests only unit-tested, no `IceCandidate` on the WebSocket port, no IPv6 route-limit case, no real-client run. | Valid (minor) | `5fcae81`: tests `ipv6_controller_gets_answer_and_ice`, `answered_retries_keep_the_offer`, `websocket_ice_candidate_closes_as_before`, the /64 unit test. Real clients: lab (see REPORT.md). |

## Feature review: UDP hole punching (`d86ba5a..88db509`)

No critical or major findings. The reviewer confirmed: UDP `TestNatResponse`
carries only the port (no `ConfigUpdate`, at most 7 bytes), `udp_port` is
passed through, three UDP `PunchHoleSent` copies yield exactly one
`PunchHoleResponse { is_udp }` over the controller's TCP connection, nothing
goes back over UDP, and the licence key gates every route.

| # | Finding | Verdict | Fix |
|---|---|---|---|
| UDP-1 | A full limiter table (65,536 sources, fillable from one IPv6 /48 without spoofing) makes every packet from a new source scan the whole table (355 µs vs 88 ns measured) in the single I/O loop. | Valid (minor) | `da09859`: the sweep runs at most once per refill time. Unit test checks that a second sweep within that time is skipped. |
| UDP-2 | One /64 could fill the route table (per-address limit), and each new route counted the whole table under the lock. | Valid (minor); the /64 part was already fixed by WEBRTC-1 | `da09859`: per-source counters instead of a scan, sweeps at most once per second. Remaining exhaustion needs thousands of open TCP connections from many /64s (documented in DECISIONS.md). |
| UDP-3 | A UDP `PunchHoleSent` from another host than the registration (NAT pool, multi-WAN) was dropped entirely, WebRTC answer included, while the TCP path accepts the same answer from any IP. A controller without a fallback round got no reply until its retries ran out. | Valid (minor) | `da09859`: answered without `is_udp`. Test `hole_sent_from_another_host_is_not_punched`. |
| UDP-4 | Docs claim "UDP works" for 1.4.9, but 1.4.1-1.4.9 may send a `udp_port` taken from STUN on another socket. | Valid (docs) | `da09859`: docs corrected; the unchanged fallback round covers it. |
| UDP-5 | The `.dockerignore` repair is hidden in the UDP commit, so reverting that commit would break the Docker build again. | Valid (hygiene) | Not rewritten: the branch history stays; the commit message names the change. Switching UDP off is done with `UDP_PUNCH=N`, not a revert. |
| UDP-6 | Test gaps: UDP + WebRTC in one round, licence key with `udp_port`, a 1.4.0 controlled side answering over TCP, limiter cost. | Valid (minor) | `da09859`: tests `udp_ipv6_and_webrtc_in_one_round`, `wrong_licence_key_opens_no_udp_route`, `old_controlled_side_answers_over_tcp`, sweep unit test. |

## Final review loop

### Round 1 (`873230e..1a5c571`)

No critical or major findings. Confirmed: key exchange v0/v1 against
1.5.0/1.4.9/1.4.0, session key derivation, field numbers, older clients skip
the extra `KeyExchange`, licence key on every new path, bounds, logging, no
`unwrap()` on network data.

| # | Finding | Verdict | Fix |
|---|---|---|---|
| FINAL-1-1 | The 100 ms offer delay is shorter than a retransmission of the controlled side's lost first segment (200 ms minimum RTO), so under packet loss the offer can still meet the closed socket and the reply is reset away. | Valid (minor) | `79080f9`: delay raised to 500 ms (documented cost: 500 ms for key-exchanging connections). |
| FINAL-1-2 | About 2000 spoofed `TestNatRequest`s per second keep the global budget empty, so genuine controllers get no answer and lose UDP punching (fail closed). Proven with 15,000 loopback sources. | Valid (minor) | `79080f9`: sources with an open TCP connection to hbbs (address proven) skip the global budget; global limit configurable (`UDP_TEST_NAT_LIMIT`). Test `connected_controllers_keep_their_nat_test` (fails without the fix). |
| FINAL-1-3 | The switches only turn off on `N`; `false`, `0`, `no`, `off` silently leave a feature on. | Valid (minor) | `79080f9`: those values turn it off, other values log a warning. Tests `switch_values_*`. |
| FINAL-1-4 | Docs claim clients that skip the exchange "send at once", but 1.5.0 controllers wait up to ~400 ms for their UDP NAT test and may see the offer first; no test covered that order. | Valid (docs/test) | `79080f9`: docs corrected, test `offer_after_half_a_second_then_plaintext`. |

### Round 2 (`873230e..a7de31a`, now including the lab)

No critical or major findings. Confirmed again: key exchange, IPv6, WebRTC,
UDP paths against 1.5.0/1.4.9/1.4.1/1.4.0; the differential test passes
against the original binary; licence key, logging, robustness, database and
key files unchanged.

| # | Finding | Verdict | Fix |
|---|---|---|---|
| FINAL-2-1 | (a) The limiter checks "table full" before `proven`, and spoofed sources get entries even when the global budget is empty, so a ~33k pps spoofed flood keeps the table full and refuses proven controllers too. Proven with a unit test. (b) The client starts its UDP NAT test before the TCP connect and waits only rtt/2 afterwards, so under a flood high-latency controllers are not rescued. | (a) Valid (minor); (b) valid, residual | `9c6f140`: unproven new sources only get an entry while the global budget has tokens (table can no longer be filled by spoofing); proven sources skip the full check. Unit tests `spoofed_sources_cannot_fill_the_test_nat_table`, `source_table_is_bounded`; test `nat_test_retry_after_the_connect_is_answered` in the client's order. (b) documented as residual risk (falls back to TCP/relay, the original behavior). |
| FINAL-2-2 | One licence holder with an IPv6 /48 can fill the global route table (157 /64s × 64 routes) and the IPv6-forward limiter table, so other users get no UDP/WebRTC/IPv6. Proven with a unit test. | Valid (minor) | `9c6f140`: a full route table evicts the oldest route of the source with clearly the most routes; the IPv6 forward limiter (licensed, non-spoofable sources) lets unknown sources pass when full. Unit test `a_full_table_takes_from_the_heaviest_source`. |
| FINAL-2-3 | 500 ms is not enough on high-latency lossy links: the first RTO after the handshake is about 3 × RTT, so for RTT above ~250 ms the offer can still win and reset the reply; the doc's timing argument was wrong. | Valid (minor) | `9c6f140`: delay = 4 × kernel-measured handshake RTT + 100 ms (500 ms to 3 s, `TCP_INFO`), docs corrected. Unit tests `kx_offer_delay_follows_the_round_trip`, `tcp_rtt_reads_the_handshake`. |
| FINAL-2-4 | The lab never runs a real 1.4.0 client, although 1.4.0 has its own code paths. | Valid (test gap) | `a9745e1`: lab cases `client-1.4.0`, `baseline-client-1.4.0`, `mixed-1.4.0-controller`, `mixed-1.4.0-controlled` (signed-in clients need an API server and are covered by the integration tests only). |
