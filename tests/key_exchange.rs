// Server key exchange on TCP 21116, replayed like the client's key_exchange()
// (1.5.0: version 1, 1.4.x: version 0) and like clients that skip it.
mod common;

use common::*;
use hbb_common::{
    futures_util::{SinkExt, StreamExt},
    protobuf::Message as _,
    rendezvous_proto::*,
    tcp::FramedStream,
    tokio,
};
use sodiumoxide::crypto::sign;

async fn unknown_id_round_trip(tcp: &mut Tcp, hbbs: &Hbbs) {
    tcp.send(&msg(|m| {
        m.set_punch_hole_request(punch_hole_request("nobody1", &hbbs.key))
    }))
    .await;
    let res = tcp
        .expect(3000, pick_punch_hole_response)
        .await
        .expect("no PunchHoleResponse");
    assert_eq!(
        res.failure.enum_value(),
        Ok(punch_hole_response::Failure::ID_NOT_EXIST)
    );
}

#[tokio::test]
async fn version_1_client_gets_encrypted_responses() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let mut tcp = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::V1).await;
    assert!(tcp.secured);
    unknown_id_round_trip(&mut tcp, &hbbs).await;
    assert!(hbbs.log().contains("secured, version 1"));
}

#[tokio::test]
async fn version_0_client_gets_encrypted_responses() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let mut tcp = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::Legacy).await;
    assert!(tcp.secured);
    unknown_id_round_trip(&mut tcp, &hbbs).await;
    assert!(hbbs.log().contains("secured, version 0"));
}

#[tokio::test]
async fn plaintext_client_skips_the_offer() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let mut tcp = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    unknown_id_round_trip(&mut tcp, &hbbs).await;
}

#[tokio::test]
async fn offer_is_the_first_frame_and_signed() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let mut conn = FramedStream::new(hbbs.tcp_addr(), None, 3000).await.unwrap();
    let first = conn.next_timeout(3000).await.unwrap().unwrap();
    let Some(rendezvous_message::Union::KeyExchange(ex)) = parse(&first).unwrap().union else {
        panic!("first frame is not a KeyExchange");
    };
    assert_eq!(ex.version, 1);
    let pk = sign::verify(&ex.keys[0], &hbbs.pk).unwrap();
    assert_eq!(pk.len(), 32);
    assert_ne!(pk[31] & 0x80, 0);
    assert!(sign::verify(&ex.signed_params, &hbbs.pk).is_ok());
    // a client configured with another key cannot verify it
    let (other, _) = sign::gen_keypair();
    let mut tcp = Tcp::connect_to(hbbs.tcp_addr(), &other, CONTROLLER_IP, Kx::None).await;
    let err = tcp.secure(&other, Kx::V1).await.unwrap_err();
    assert!(err.contains("Signature mismatch"));
}

#[tokio::test]
async fn switched_off_sends_nothing_first() {
    let hbbs = Hbbs::start(&[("SECURE_TCP", "N")], &[]).await;
    let mut conn = FramedStream::new(hbbs.tcp_addr(), None, 3000).await.unwrap();
    assert!(conn.next_timeout(700).await.is_none());
    let mut tcp = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    unknown_id_round_trip(&mut tcp, &hbbs).await;
    assert!(hbbs.log().contains("SECURE_TCP=N"));
}

#[tokio::test]
async fn cli_flag_switches_it_off() {
    let hbbs = Hbbs::start(&[], &["--secure-tcp", "N"]).await;
    let mut conn = FramedStream::new(hbbs.tcp_addr(), None, 3000).await.unwrap();
    assert!(conn.next_timeout(700).await.is_none());
}

#[tokio::test]
async fn nat_test_port_has_no_key_exchange() {
    let hbbs = Hbbs::start(&[], &[]).await;
    // loopback connections to port-1 are console commands, so use another local address
    let Ok(ip) = local_ip_address::local_ip() else {
        eprintln!("no non-loopback address, skipped");
        return;
    };
    let addr = std::net::SocketAddr::new(ip, hbbs.nat_addr().port());
    let mut conn = FramedStream::new(addr, None, 3000).await.unwrap();
    assert!(conn.next_timeout(700).await.is_none());
    conn.send(&msg(|m| m.set_test_nat_request(TestNatRequest::default())))
        .await
        .unwrap();
    let res = conn.next_timeout(3000).await.unwrap().unwrap();
    assert!(matches!(
        parse(&res).unwrap().union,
        Some(rendezvous_message::Union::TestNatResponse(_))
    ));
}

#[tokio::test]
async fn websocket_has_no_key_exchange() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let url = format!("ws://{}", hbbs.ws_addr());
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let silent =
        tokio::time::timeout(std::time::Duration::from_millis(700), ws.next()).await;
    assert!(silent.is_err(), "the server sent something first on WebSocket");
    let req = msg(|m| m.set_punch_hole_request(punch_hole_request("nobody1", &hbbs.key)));
    ws.send(tungstenite::Message::Binary(req.write_to_bytes().unwrap()))
        .await
        .unwrap();
    let res = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tungstenite::Message::Binary(bytes) = res else {
        panic!("unexpected WebSocket message");
    };
    let res = parse(&bytes).and_then(|m| m.union).and_then(pick_punch_hole_response);
    assert_eq!(
        res.unwrap().failure.enum_value(),
        Ok(punch_hole_response::Failure::ID_NOT_EXIST)
    );
}

#[tokio::test]
async fn broken_reply_closes_the_connection() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let mut tcp = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    tcp.send(&msg(|m| {
        m.set_key_exchange(KeyExchange {
            keys: vec![vec![1u8; 32].into(), vec![2u8; 48].into()],
            version: 1,
            ..Default::default()
        })
    }))
    .await;
    assert!(tcp.closed_within(3000).await);
}

#[tokio::test]
async fn garbage_after_the_exchange_closes_the_connection() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let mut tcp = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::V1).await;
    // bypass the client's encryption: a frame the server cannot decrypt
    tcp.conn.send_bytes(vec![7u8; 40].into()).await.unwrap();
    assert!(tcp.closed_within(3000).await);
}

#[tokio::test]
async fn a_late_key_exchange_is_not_accepted() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let mut tcp = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    // a plaintext first frame keeps the connection plaintext
    unknown_id_round_trip(&mut tcp, &hbbs).await;
    // a KeyExchange now is just an unknown message, which closes it as before
    tcp.send(&msg(|m| {
        m.set_key_exchange(KeyExchange {
            keys: vec![vec![1u8; 32].into(), vec![2u8; 48].into()],
            ..Default::default()
        })
    }))
    .await;
    assert!(tcp.closed_within(3000).await);
}

#[tokio::test]
async fn oversized_frame_closes_the_connection() {
    let hbbs = Hbbs::start(&[], &[]).await;
    let mut tcp = Tcp::connect(&hbbs, CONTROLLER_IP, Kx::None).await;
    tcp.send_raw(vec![0u8; 70 * 1024]).await;
    assert!(tcp.closed_within(3000).await);
}

// TCP punch over an encrypted controller connection: the controlled side's
// plaintext PunchHoleSent reaches the controller encrypted, with the signed pk.
#[tokio::test]
async fn tcp_punch_with_an_encrypted_controller() {
    for kx in [Kx::V1, Kx::Legacy, Kx::None] {
        let hbbs = Hbbs::start(&[], &[]).await;
        let peer = Peer::register(&hbbs, "peer01").await;
        let mut ctl = Tcp::connect(&hbbs, CONTROLLER_IP, kx).await;
        ctl.send(&msg(|m| {
            m.set_punch_hole_request(punch_hole_request("peer01", &hbbs.key))
        }))
        .await;
        let ph = peer.expect(3000, pick_punch_hole).await.expect("no PunchHole");
        assert_eq!(demangle(&ph.socket_addr), ctl.local_addr());
        // like handle_punch_hole: new connection, send_raw without reading
        let mut reply = Tcp::connect(&hbbs, PEER_IP, Kx::None).await;
        reply
            .send(&msg(|m| {
                m.set_punch_hole_sent(PunchHoleSent {
                    socket_addr: ph.socket_addr.clone(),
                    id: "peer01".to_owned(),
                    nat_type: NatType::ASYMMETRIC.into(),
                    version: "1.5.0".to_owned(),
                    ..Default::default()
                })
            }))
            .await;
        let res = ctl
            .expect(3000, pick_punch_hole_response)
            .await
            .expect("no PunchHoleResponse");
        assert_eq!(demangle(&res.socket_addr), reply.local_addr());
        let id_pk = IdPk::parse_from_bytes(&sign::verify(&res.pk, &hbbs.pk).unwrap()).unwrap();
        assert_eq!(id_pk.id, "peer01");
        assert_eq!(&id_pk.pk[..], &peer.pk.0[..]);
        assert!(!res.is_udp);
    }
}
