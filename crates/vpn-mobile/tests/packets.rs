use base64::{engine::general_purpose::STANDARD, Engine as _};
use boringtun::{
    noise::{Tunn, TunnResult},
    x25519::{PublicKey, StaticSecret},
};
use vpn_api_types::peer::PeerRegisterResponse;
use vpn_mobile::{
    engine::{Engine, Packet},
    policy,
};
use vpn_obfs::{Codec, Direction, Mode, ReplayCache};

fn config(public: &str, mode: Option<&str>) -> PeerRegisterResponse {
    let mut v = serde_json::json!({"vpn_ip":"10.8.0.2","vpn_subnet":"10.8.0.0/24","server_public_key":public,"server_endpoint":"vpn.example.com:51820","allowed_routes":["192.168.0.0/16"],"dns":{"mode":"global","server":"10.8.0.1"}});
    if let Some(mode) = mode {
        v["transport"] = serde_json::json!({"protocol":"obfs-v1","mode":mode,"endpoint":"vpn.example.com:47358","psk":STANDARD.encode([7;32]),"path_mtu":1500});
    }
    serde_json::from_value(v).unwrap()
}
#[test]
fn real_wireguard_packets_and_replay_in_both_modes() {
    for mode in [None, Some("low-overhead-v1"), Some("paranoid-v1")] {
        let private = StaticSecret::from([1; 32]);
        let server_private = StaticSecret::from([2; 32]);
        let cfg = config(
            &STANDARD.encode(PublicKey::from(&server_private).as_bytes()),
            mode,
        );
        let mut mobile = Engine::new(&STANDARD.encode(private.to_bytes()), &cfg).unwrap();
        let mut server = Tunn::new(
            server_private,
            PublicKey::from(&private),
            None,
            Some(25),
            2,
            None,
        );
        let codec = |direction| {
            mode.map(|m| {
                Codec::new(
                    &[7; 32],
                    if m == "low-overhead-v1" {
                        Mode::LowOverheadV1
                    } else {
                        Mode::ParanoidV1
                    },
                    direction,
                    1472,
                )
                .unwrap()
            })
        };
        let incoming = codec(Direction::ClientToServer);
        let outgoing = codec(Direction::ServerToClient);
        let mut replay = ReplayCache::new();
        let decode = |wire: Vec<u8>, replay: &mut ReplayCache| {
            incoming
                .as_ref()
                .map(|c| c.decode(&wire, replay).unwrap())
                .unwrap_or(wire)
        };
        let encode = |wire: &[u8]| {
            outgoing
                .as_ref()
                .map(|c| c.encode(wire).unwrap())
                .unwrap_or(wire.to_vec())
        };
        let mut buffer = [0u8; 65536];
        let initiation = network(mobile.process(3, &[]).unwrap());
        let response = match server.decapsulate(None, &decode(initiation, &mut replay), &mut buffer)
        {
            TunnResult::WriteToNetwork(v) => encode(v),
            other => panic!("{other:?}"),
        };
        let keepalive = network(mobile.process(1, &response).unwrap());
        assert!(matches!(
            server.decapsulate(None, &decode(keepalive, &mut replay), &mut buffer),
            TunnResult::Done
        ));
        assert!(mobile.stats().handshake_seconds.is_some());
        if mode.is_some() {
            assert!(mobile.process(1, &response).unwrap().is_empty());
        }
        for len in [21, 84, 1280, 1360] {
            let mut original = vec![0; len];
            original[0] = 0x45;
            original[2..4].copy_from_slice(&(len as u16).to_be_bytes());
            original[8] = 64;
            original[9] = 17;
            original[12..16].copy_from_slice(&[10, 8, 0, 2]);
            original[16..20].copy_from_slice(&[10, 8, 0, 1]);
            let wire = network(mobile.process(0, &original).unwrap());
            let wire = decode(wire, &mut replay);
            match server.decapsulate(None, &wire, &mut buffer) {
                TunnResult::WriteToTunnelV4(v, _) => assert_eq!(v, original),
                other => panic!("{other:?}"),
            }
            let mut padded = original.clone();
            padded.resize(len.div_ceil(16) * 16, 0);
            let response = match server.encapsulate(&padded, &mut buffer) {
                TunnResult::WriteToNetwork(v) => encode(v),
                other => panic!("{other:?}"),
            };
            assert_eq!(
                mobile.process(1, &response).unwrap(),
                vec![Packet::Tunnel(original)]
            );
            assert!(mobile.process(1, &response).unwrap().is_empty());
        }
        assert!(mobile.process(0, &[0; 1500]).is_err());
        assert!(mobile.process(1, &[0; 31]).unwrap().is_empty());
    }
}
fn network(packets: Vec<Packet>) -> Vec<u8> {
    packets
        .into_iter()
        .find_map(|p| match p {
            Packet::Network(b) => Some(b),
            _ => None,
        })
        .expect("network packet")
}
#[test]
fn policy_preserves_vpn_and_rejects_unsafe_inputs() {
    let (_, public) = vpn_mobile::engine::keypair();
    let mut c = config(&public, Some("paranoid-v1"));
    c.local_route_bypass = serde_json::from_value(serde_json::json!([{"local_subnets":["192.168.1.0/24"],"excluded_routes":["192.168.0.0/16","10.8.0.0/24"]}])).unwrap();
    let p = policy::plan(&c, &["192.168.1.7".into()]).unwrap();
    assert_eq!(p.routes, vec!["10.8.0.0/24"]);
    c.allowed_routes = vec!["0.0.0.0/0".into()];
    assert!(policy::plan(&c, &[]).is_err());
    c.allowed_routes = vec![];
    c.dns.as_mut().unwrap().server = "8.8.8.8".into();
    assert!(policy::plan(&c, &[]).is_err());
    c.dns = None;
    c.transport.as_mut().unwrap().path_mtu = 576;
    assert!(policy::plan(&c, &[]).is_err());
}
#[test]
fn api_error_and_legacy_null_contract() {
    assert_eq!(
        vpn_mobile::session::unwrap_response(r#"{"code":0,"data":null}"#).unwrap(),
        serde_json::Value::Null
    );
    assert!(vpn_mobile::session::unwrap_response(r#"{"code":1007}"#).is_err());
    assert!(vpn_mobile::session::session_invalid(1007));
    assert!(!vpn_mobile::session::session_invalid(5000));
}
