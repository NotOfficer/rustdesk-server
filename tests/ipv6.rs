// IPv6 P2P: socket_addr_v6 travels between the peers in every message the
// 1.5.0 client uses it in (client.rs _start_inner/connect, rendezvous_mediator.rs
// handle_punch_hole/handle_intranet/create_relay).
mod common;

use common::*;
use hbb_common::{bytes::Bytes, rendezvous_proto::*, tokio};
use std::net::SocketAddr;

fn v6(s: &str) -> Bytes {
    mangle(s.parse::<SocketAddr>().unwrap())
}

const A_V6: &str = "[2001:db8::a]:40001";
const B_V6: &str = "[2001:db8::b]:40002";

async fn request(hbbs: &Hbbs, ctl: &mut Tcp, id: &str, v6_addr: Bytes) {
    let mut req = punch_hole_request(id, &hbbs.key);
    req.socket_addr_v6 = v6_addr;
    ctl.send(&msg(|m| m.set_punch_hole_request(req))).await;
}

async fn punch_hole_sent(hbbs: &Hbbs, ph_socket_addr: Bytes, id: &str, v6_addr: Bytes) -> Tcp {
    let mut reply = Tcp::connect(hbbs, PEER_IP, Kx::None).await;
    reply
        .send(&msg(|m| {
            m.set_punch_hole_sent(PunchHoleSent {
                socket_addr: ph_socket_addr,
                id: id.to_owned(),
                nat_type: NatType::ASYMMETRIC.into(),
                version: "1.5.0".to_owned(),
                socket_addr_v6: v6_addr,
                ..Default::default()
            })
        }))
        .await;
    reply
}

#[tokio::test]
async fn punch_hole_carries_v6_both_ways() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    request(&hbbs, &mut ctl, "peer01", v6(A_V6)).await;
    let ph = peer.expect(3000, pick_punch_hole).await.unwrap();
    assert_eq!(demangle(&ph.socket_addr_v6), A_V6.parse().unwrap());
    let _reply = punch_hole_sent(&hbbs, ph.socket_addr, "peer01", v6(B_V6)).await;
    let res = ctl.expect(3000, pick_punch_hole_response).await.unwrap();
    assert_eq!(demangle(&res.socket_addr_v6), B_V6.parse().unwrap());
    assert!(!res.socket_addr.is_empty());
}

#[tokio::test]
async fn same_intranet_carries_v6_both_ways() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    // same public IP as the peer: FetchLocalAddr instead of PunchHole
    let mut ctl = Tcp::connect(&hbbs, PEER_IP, Kx::None).await;
    request(&hbbs, &mut ctl, "peer01", v6(A_V6)).await;
    let fla = peer.expect(3000, pick_fetch_local_addr).await.unwrap();
    assert_eq!(demangle(&fla.socket_addr_v6), A_V6.parse().unwrap());
    // like handle_intranet_: LocalAddr on a new plaintext connection
    let mut reply = Tcp::connect(&hbbs, PEER_IP, Kx::None).await;
    let local = reply.local_addr();
    reply
        .send(&msg(|m| {
            m.set_local_addr(LocalAddr {
                id: "peer01".to_owned(),
                socket_addr: mangle(demangle(&fla.socket_addr)),
                local_addr: mangle(local),
                version: "1.5.0".to_owned(),
                socket_addr_v6: v6(B_V6),
                ..Default::default()
            })
        }))
        .await;
    let res = ctl.expect(3000, pick_punch_hole_response).await.unwrap();
    assert!(res.is_local());
    assert_eq!(demangle(&res.socket_addr), local);
    assert_eq!(demangle(&res.socket_addr_v6), B_V6.parse().unwrap());
}

async fn relay_response(hbbs: &Hbbs, ph_socket_addr: Bytes, v6_addr: Bytes) -> Tcp {
    // like create_relay(initiate = true)
    let mut reply = Tcp::connect(hbbs, PEER_IP, Kx::None).await;
    let mut rr = RelayResponse {
        socket_addr: ph_socket_addr,
        uuid: "uuid-relay-1".to_owned(),
        relay_server: "relay.example:21117".to_owned(),
        version: "1.5.0".to_owned(),
        socket_addr_v6: v6_addr,
        ..Default::default()
    };
    rr.set_id("peer01".to_owned());
    reply.send(&msg(|m| m.set_relay_response(rr))).await;
    reply
}

#[tokio::test]
async fn relay_response_carries_v6() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    request(&hbbs, &mut ctl, "peer01", v6(A_V6)).await;
    let ph = peer.expect(3000, pick_punch_hole).await.unwrap();
    let _reply = relay_response(&hbbs, ph.socket_addr, v6(B_V6)).await;
    let rr = ctl.expect(3000, pick_relay_response).await.unwrap();
    assert_eq!(demangle(&rr.socket_addr_v6), B_V6.parse().unwrap());
    assert_eq!(rr.uuid, "uuid-relay-1");
    assert!(!rr.pk().is_empty());
}

#[tokio::test]
async fn switched_off_drops_v6_everywhere() {
    for (envs, args) in [
        (vec![("IPV6_PUNCH", "N")], vec![]),
        (vec![], vec!["--ipv6-punch", "n"]),
    ] {
        let hbbs = Hbbs::start(&envs, &args).await;
        let peer = Peer::register(&hbbs, "peer01").await;
        let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
        request(&hbbs, &mut ctl, "peer01", v6(A_V6)).await;
        let ph = peer.expect(3000, pick_punch_hole).await.unwrap();
        assert!(ph.socket_addr_v6.is_empty());
        let _reply = punch_hole_sent(&hbbs, ph.socket_addr.clone(), "peer01", v6(B_V6)).await;
        let res = ctl.expect(3000, pick_punch_hole_response).await.unwrap();
        assert!(res.socket_addr_v6.is_empty());

        let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
        request(&hbbs, &mut ctl, "peer01", v6(A_V6)).await;
        let ph = peer.expect(3000, pick_punch_hole).await.unwrap();
        let _reply = relay_response(&hbbs, ph.socket_addr, v6(B_V6)).await;
        let rr = ctl.expect(3000, pick_relay_response).await.unwrap();
        assert!(rr.socket_addr_v6.is_empty());

        let mut ctl = Tcp::connect(&hbbs, PEER_IP, Kx::None).await;
        request(&hbbs, &mut ctl, "peer01", v6(A_V6)).await;
        let fla = peer.expect(3000, pick_fetch_local_addr).await.unwrap();
        assert!(fla.socket_addr_v6.is_empty());
    }
}

#[tokio::test]
async fn forced_relay_drops_v6() {
    let hbbs = Hbbs::start(&[("ALWAYS_USE_RELAY", "Y")], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    request(&hbbs, &mut ctl, "peer01", v6(A_V6)).await;
    let ph = peer.expect(3000, pick_punch_hole).await.unwrap();
    assert!(ph.socket_addr_v6.is_empty());
    assert_eq!(ph.nat_type.enum_value(), Ok(NatType::SYMMETRIC));
    let _reply = relay_response(&hbbs, ph.socket_addr, v6(B_V6)).await;
    let rr = ctl.expect(3000, pick_relay_response).await.unwrap();
    assert!(rr.socket_addr_v6.is_empty());
}

#[tokio::test]
async fn malformed_v6_is_dropped() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    for bad in [vec![1u8; 17], vec![1u8; 19], vec![0xffu8; 4000], mangle("1.2.3.4:5".parse().unwrap()).to_vec()] {
        let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
        request(&hbbs, &mut ctl, "peer01", bad.clone().into()).await;
        let ph = peer.expect(3000, pick_punch_hole).await.unwrap();
        assert!(ph.socket_addr_v6.is_empty());
        let _reply = punch_hole_sent(&hbbs, ph.socket_addr, "peer01", bad.into()).await;
        let res = ctl.expect(3000, pick_punch_hole_response).await.unwrap();
        assert!(res.socket_addr_v6.is_empty());
    }
}

#[tokio::test]
async fn websocket_controller_gets_no_v6() {
    use hbb_common::{
        futures_util::{SinkExt, StreamExt},
        protobuf::Message as _,
    };
    let hbbs = Hbbs::start(&[], &[]).await;
    let peer = Peer::register(&hbbs, "peer01").await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}", hbbs.ws_addr()))
        .await
        .unwrap();
    let mut req = punch_hole_request("peer01", &hbbs.key);
    req.socket_addr_v6 = v6(A_V6);
    ws.send(tungstenite::Message::Binary(
        msg(|m| m.set_punch_hole_request(req)).write_to_bytes().unwrap(),
    ))
    .await
    .unwrap();
    let ph = peer.expect(3000, pick_punch_hole).await.unwrap();
    assert!(ph.socket_addr_v6.is_empty());
    drop(ws.next());
}
