// UDP hole punching as the 1.5.0 client does it: the controller's UDP NAT test
// (client.rs test_udp_uat), udp_port in PunchHoleRequest, the controlled side's
// UDP PunchHoleSent from its punch socket (rendezvous_mediator.rs punch_udp_hole,
// three copies) and the PunchHoleResponse with is_udp to the controller.
// Plus the negative cases that matter for reflection (CVE-2026-30784).
mod common;

use common::*;
use hbb_common::{
    bytes::Bytes,
    protobuf::Message as _,
    rendezvous_proto::*,
    tokio::{self, net::UdpSocket},
};
use sodiumoxide::crypto::sign;
use std::{net::SocketAddr, time::Duration};

async fn udp_socket(ip: std::net::IpAddr) -> UdpSocket {
    UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap()
}

/// Receives raw datagrams for `ms`, returns them parsed.
async fn drain(sock: &UdpSocket, ms: u64) -> Vec<RendezvousMessage> {
    let mut out = vec![];
    let mut buf = vec![0u8; 65536];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    while let Ok(Ok((n, _))) =
        tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await
    {
        if let Some(m) = parse(&buf[..n]) {
            out.push(m);
        }
    }
    out
}

fn test_nat_request() -> Vec<u8> {
    msg(|m| m.set_test_nat_request(TestNatRequest::default()))
        .write_to_bytes()
        .unwrap()
}

#[tokio::test]
async fn udp_test_nat_reports_the_port_only() {
    // a server serial above the request's would add a ConfigUpdate over TCP
    let hbbs = Hbbs::start(&[], &["-s", "5"]).await;
    let sock = udp_socket(CONTROLLER_IP).await;
    let req = test_nat_request();
    sock.send_to(&req, hbbs.udp_addr()).await.unwrap();
    let mut buf = [0u8; 2048];
    let (n, from) = tokio::time::timeout(Duration::from_secs(3), sock.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(from.port(), hbbs.port);
    let Some(rendezvous_message::Union::TestNatResponse(res)) = parse(&buf[..n]).unwrap().union
    else {
        panic!("no TestNatResponse");
    };
    assert_eq!(res.port, sock.local_addr().unwrap().port() as i32);
    assert!(res.cu.is_none());
    // the answer is the port and nothing else: at most 4 bytes more than the request
    assert!(n <= req.len() + 4, "answer {} bytes for a {} byte request", n, req.len());
}

#[tokio::test]
async fn udp_test_nat_is_rate_limited() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let sock = udp_socket(CONTROLLER_IP).await;
    let req = test_nat_request();
    for _ in 0..200 {
        sock.send_to(&req, hbbs.udp_addr()).await.unwrap();
    }
    let answers = drain(&sock, 1000).await.len();
    assert!((20..=25).contains(&answers), "{} answers", answers);
    // another source has its own budget
    let other = udp_socket("127.0.0.3".parse().unwrap()).await;
    other.send_to(&req, hbbs.udp_addr()).await.unwrap();
    assert_eq!(drain(&other, 500).await.len(), 1);
    // and the bucket refills
    tokio::time::sleep(Duration::from_millis(1500)).await;
    for _ in 0..5 {
        sock.send_to(&req, hbbs.udp_addr()).await.unwrap();
    }
    assert_eq!(drain(&sock, 500).await.len(), 5);
}

#[tokio::test]
async fn switched_off_ignores_udp() {
    for (envs, args) in [(vec![("UDP_PUNCH", "N")], vec![]), (vec![], vec!["--udp-punch", "N"])] {
        let hbbs = Hbbs::start(&envs, &args).await;
        let sock = udp_socket(CONTROLLER_IP).await;
        sock.send_to(&test_nat_request(), hbbs.udp_addr()).await.unwrap();
        assert!(drain(&sock, 700).await.is_empty());
        let peer = Peer::register(&hbbs, "peer01").await;
        let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
        let mut req = punch_hole_request("peer01", &hbbs.key);
        req.udp_port = 40000;
        ctl.send(&msg(|m| m.set_punch_hole_request(req))).await;
        let ph = peer.expect(3000, pick_punch_hole).await.unwrap();
        assert_eq!(ph.udp_port, 0);
        let punch = udp_socket(PEER_IP).await;
        send_hole_sent(&punch, &hbbs, &ph.socket_addr, "peer01", None).await;
        assert!(ctl.expect(700, pick_punch_hole_response).await.is_none());
    }
}

async fn send_hole_sent(
    sock: &UdpSocket,
    hbbs: &Hbbs,
    ph_socket_addr: &Bytes,
    id: &str,
    extra: Option<fn(&mut PunchHoleSent)>,
) {
    let mut phs = PunchHoleSent {
        socket_addr: ph_socket_addr.clone(),
        id: id.to_owned(),
        relay_server: "relay.example:21117".to_owned(),
        nat_type: NatType::ASYMMETRIC.into(),
        version: "1.5.0".to_owned(),
        ..Default::default()
    };
    if let Some(f) = extra {
        f(&mut phs);
    }
    let bytes = msg(|m| m.set_punch_hole_sent(phs)).write_to_bytes().unwrap();
    sock.send_to(&bytes, hbbs.udp_addr()).await.unwrap();
}

struct Punch {
    hbbs: Hbbs,
    peer: Peer,
    ctl: Tcp,
    ph: PunchHole,
}

async fn start_udp_punch(kx: Kx) -> Punch {
    let hbbs = Hbbs::start(&[], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    // the UDP NAT test gives the controller its punch port
    let ctl_udp = udp_socket(CONTROLLER_IP).await;
    ctl_udp.send_to(&test_nat_request(), hbbs.udp_addr()).await.unwrap();
    ctl_udp.send_to(&test_nat_request(), hbbs.udp_addr()).await.unwrap();
    let port = match drain(&ctl_udp, 500).await.first().and_then(|m| m.union.clone()) {
        Some(rendezvous_message::Union::TestNatResponse(r)) => r.port,
        other => panic!("no TestNatResponse: {:?}", other),
    };
    let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, kx).await;
    let mut req = punch_hole_request("peer01", &hbbs.key);
    req.udp_port = port;
    ctl.send(&msg(|m| m.set_punch_hole_request(req))).await;
    let ph = peer.expect(3000, pick_punch_hole).await.expect("no PunchHole");
    assert_eq!(ph.udp_port, port);
    Punch { hbbs, peer, ctl, ph }
}

#[tokio::test]
async fn udp_punch_reaches_the_controller_over_tcp() {
    for kx in [Kx::None, Kx::V1] {
        let Punch { hbbs, peer, mut ctl, ph } = start_udp_punch(kx).await;
        // three copies from a fresh socket of the same host, like punch_udp_hole
        let punch = udp_socket(PEER_IP).await;
        for _ in 0..3 {
            send_hole_sent(&punch, &hbbs, &ph.socket_addr, "peer01", None).await;
        }
        let res = ctl.expect(3000, pick_punch_hole_response).await.expect("no response");
        assert!(res.is_udp);
        assert_eq!(demangle(&res.socket_addr), punch.local_addr().unwrap());
        assert_eq!(res.relay_server, "relay.example:21117");
        assert_eq!(res.nat_type(), NatType::ASYMMETRIC);
        let id_pk = IdPk::parse_from_bytes(&sign::verify(&res.pk, &hbbs.pk).unwrap()).unwrap();
        assert_eq!(id_pk.id, "peer01");
        assert_eq!(&id_pk.pk[..], &peer.pk.0[..]);
        // only one answer, and nothing ever comes back over UDP
        assert!(ctl.expect(700, pick_punch_hole_response).await.is_none());
        assert!(drain(&punch, 100).await.is_empty());
    }
}

#[tokio::test]
async fn forged_hole_sent_without_a_route_sends_nothing() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    // a controller that did not ask for UDP
    let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    ctl.send(&msg(|m| m.set_punch_hole_request(punch_hole_request("peer01", &hbbs.key))))
        .await;
    let ph = peer.expect(3000, pick_punch_hole).await.unwrap();
    let forger = udp_socket(PEER_IP).await;
    send_hole_sent(&forger, &hbbs, &ph.socket_addr, "peer01", None).await;
    // and one for an address nobody has a connection from
    let nobody = mangle("127.0.0.9:4444".parse().unwrap());
    send_hole_sent(&forger, &hbbs, &nobody, "peer01", None).await;
    assert!(ctl.expect(700, pick_punch_hole_response).await.is_none());
    assert!(drain(&forger, 300).await.is_empty());
    assert!(drain(&peer.sock, 100).await.is_empty());
}

#[tokio::test]
async fn hole_sent_from_another_host_or_id_is_ignored() {
    let Punch { hbbs, mut ctl, ph, .. } = start_udp_punch(Kx::None).await;
    let stranger = udp_socket("127.0.0.3".parse().unwrap()).await;
    send_hole_sent(&stranger, &hbbs, &ph.socket_addr, "peer01", None).await;
    let punch = udp_socket(PEER_IP).await;
    send_hole_sent(&punch, &hbbs, &ph.socket_addr, "peer02", None).await;
    assert!(ctl.expect(700, pick_punch_hole_response).await.is_none());
    assert!(drain(&stranger, 100).await.is_empty());
    // the real one still works
    send_hole_sent(&punch, &hbbs, &ph.socket_addr, "peer01", None).await;
    let res = ctl.expect(3000, pick_punch_hole_response).await.unwrap();
    assert_eq!(demangle(&res.socket_addr), punch.local_addr().unwrap());
}

#[tokio::test]
async fn garbage_does_not_crash_the_server() {
    let mut hbbs = Hbbs::start(&[], &[]).await;
    let sock = udp_socket(CONTROLLER_IP).await;
    let mut seed = 0x1234_5678u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for i in 0..2000 {
        let len = (next() % 1500) as usize;
        let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        if i % 2 == 0 {
            // structurally valid PunchHoleSent with random fields
            let socket_addr: Vec<u8> = (0..(next() % 20)).map(|_| next() as u8).collect();
            let phs = PunchHoleSent {
                socket_addr: socket_addr.into(),
                id: format!("{}", next()),
                webrtc_sdp_answer: "x".repeat((next() % 5000) as usize),
                socket_addr_v6: bytes.clone().into(),
                ..Default::default()
            };
            bytes = msg(|m| m.set_punch_hole_sent(phs)).write_to_bytes().unwrap();
        }
        sock.send_to(&bytes, hbbs.udp_addr()).await.ok();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(hbbs.is_running());
    // random bytes may happen to be a registration, but nothing reached a controller
    // or peer, and UDP still works
    for m in drain(&sock, 300).await {
        assert!(!matches!(
            m.union,
            Some(rendezvous_message::Union::PunchHoleResponse(_))
                | Some(rendezvous_message::Union::PunchHole(_))
                | Some(rendezvous_message::Union::IceCandidate(_))
        ));
    }
    let fresh = udp_socket(PEER_IP).await;
    fresh.send_to(&test_nat_request(), hbbs.udp_addr()).await.unwrap();
    assert_eq!(drain(&fresh, 700).await.len(), 1);
}

#[tokio::test]
async fn invalid_or_forced_udp_port_is_not_forwarded() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    for port in [-5, 0, 65536, 1 << 30] {
        let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
        let mut req = punch_hole_request("peer01", &hbbs.key);
        req.udp_port = port;
        ctl.send(&msg(|m| m.set_punch_hole_request(req))).await;
        assert_eq!(peer.expect(3000, pick_punch_hole).await.unwrap().udp_port, 0);
    }
    let hbbs = Hbbs::start(&[("ALWAYS_USE_RELAY", "Y")], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    let mut req = punch_hole_request("peer01", &hbbs.key);
    req.udp_port = 40000;
    ctl.send(&msg(|m| m.set_punch_hole_request(req))).await;
    assert_eq!(peer.expect(3000, pick_punch_hole).await.unwrap().udp_port, 0);
}

#[tokio::test]
async fn same_intranet_without_offer_keeps_fetch_local_addr() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    let mut ctl = Tcp::connect(&hbbs, PEER_IP, Kx::None).await;
    let mut req = punch_hole_request("peer01", &hbbs.key);
    req.udp_port = 40000;
    ctl.send(&msg(|m| m.set_punch_hole_request(req))).await;
    assert!(peer.expect(3000, pick_fetch_local_addr).await.is_some());
}

// Waits out the 30 s UDP route TTL while the controller keeps its connection.
#[tokio::test]
async fn udp_routes_expire() {
    let Punch { hbbs, mut ctl, ph, .. } = start_udp_punch(Kx::None).await;
    tokio::time::sleep(Duration::from_secs(20)).await;
    // keeps the connection without touching the route (an unmatched candidate)
    ctl.send(&msg(|m| {
        m.set_ice_candidate(IceCandidate {
            id: "peer01".to_owned(),
            session_key: "k".to_owned(),
            candidate: "c".to_owned(),
            ..Default::default()
        })
    }))
    .await;
    tokio::time::sleep(Duration::from_secs(11)).await;
    let punch = udp_socket(PEER_IP).await;
    send_hole_sent(&punch, &hbbs, &ph.socket_addr, "peer01", None).await;
    assert!(ctl.expect(1000, pick_punch_hole_response).await.is_none());
}
