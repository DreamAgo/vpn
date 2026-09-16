use crate::{policy, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use boringtun::{
    noise::{Tunn, TunnResult},
    x25519::{PublicKey, StaticSecret},
};
use rand::rngs::OsRng;
use serde::Serialize;
use vpn_api_types::peer::{ObfsMode, PeerRegisterResponse};
use vpn_obfs::{Codec, Direction, Mode, ReplayCache};

pub struct Engine {
    tunnel: Tunn,
    tx: Option<Codec>,
    rx: Option<Codec>,
    replay: ReplayCache,
    mtu: usize,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Packet {
    Network(Vec<u8>),
    Tunnel(Vec<u8>),
}
#[derive(Serialize)]
pub struct Stats {
    pub handshake_seconds: Option<u64>,
    pub tx_bytes: usize,
    pub rx_bytes: usize,
}
pub fn keypair() -> (String, String) {
    let private = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&private);
    (
        STANDARD.encode(private.to_bytes()),
        STANDARD.encode(public.as_bytes()),
    )
}
impl Engine {
    pub fn new(private: &str, config: &PeerRegisterResponse) -> Result<Self> {
        let plan = policy::plan(config, &[])?;
        let tunnel = Tunn::new(
            StaticSecret::from(policy::key(private)?),
            PublicKey::from(policy::key(&config.server_public_key)?),
            None,
            Some(25),
            1,
            None,
        );
        let (tx, rx) = if let Some(t) = &config.transport {
            let mode = match t.mode {
                ObfsMode::LowOverheadV1 => Mode::LowOverheadV1,
                ObfsMode::ParanoidV1 => Mode::ParanoidV1,
            };
            let psk = zeroize::Zeroizing::new(policy::key(&t.psk)?);
            let make = |direction| {
                Codec::new(psk.as_ref(), mode, direction, usize::from(t.path_mtu - 28))
                    .map_err(|e| e.to_string())
            };
            (
                Some(make(Direction::ClientToServer)?),
                Some(make(Direction::ServerToClient)?),
            )
        } else {
            (None, None)
        };
        Ok(Self {
            tunnel,
            tx,
            rx,
            replay: ReplayCache::new(),
            mtu: usize::from(plan.mtu),
        })
    }
    /// kind: 0 = plaintext IP, 1 = incoming UDP, 2 = timer, 3 = start handshake.
    /// Returned owned packets must be delivered in order; malformed UDP is dropped.
    pub fn process(&mut self, kind: u8, input: &[u8]) -> Result<Vec<Packet>> {
        if input.len() > 65535 || (kind == 0 && input.len() > self.mtu) {
            return Err("Packet exceeds MTU".into());
        }
        let padded;
        let input = if kind == 0 {
            padded = {
                let mut value = input.to_vec();
                value.resize(input.len().div_ceil(16) * 16, 0);
                value
            };
            padded.as_slice()
        } else {
            input
        };
        let decoded;
        let input = if kind == 1 {
            if let Some(codec) = &self.rx {
                decoded = match codec.decode(input, &mut self.replay) {
                    Ok(v) => v,
                    Err(_) => return Ok(vec![]),
                };
                decoded.as_slice()
            } else {
                input
            }
        } else {
            input
        };
        let mut buffer = vec![0u8; 65536];
        let mut packets = Vec::new();
        let result = match kind {
            0 => self.tunnel.encapsulate(input, &mut buffer),
            1 => self.tunnel.decapsulate(None, input, &mut buffer),
            2 => self.tunnel.update_timers(&mut buffer),
            3 => self.tunnel.format_handshake_initiation(&mut buffer, false),
            _ => return Err("Unknown packet operation".into()),
        };
        Self::collect(&self.tx, result, &mut packets)?;
        // Drain queued packets after a successful handshake.
        if kind == 1 {
            for _ in 0..256 {
                let result = self.tunnel.decapsulate(None, &[], &mut buffer);
                if matches!(result, TunnResult::Done | TunnResult::Err(_)) {
                    break;
                }
                Self::collect(&self.tx, result, &mut packets)?;
            }
        }
        Ok(packets)
    }
    fn collect(
        tx: &Option<Codec>,
        result: TunnResult<'_>,
        packets: &mut Vec<Packet>,
    ) -> Result<()> {
        match result {
            TunnResult::WriteToNetwork(data) => {
                packets.push(Packet::Network(if let Some(codec) = tx {
                    codec.encode(data).map_err(|e| e.to_string())?
                } else {
                    data.to_vec()
                }))
            }
            TunnResult::WriteToTunnelV4(data, _) | TunnResult::WriteToTunnelV6(data, _) => {
                packets.push(Packet::Tunnel(data.to_vec()))
            }
            TunnResult::Done | TunnResult::Err(_) => {}
        }
        Ok(())
    }
    pub fn stats(&self) -> Stats {
        let (handshake, tx, rx, _, _) = self.tunnel.stats();
        Stats {
            handshake_seconds: handshake.map(|d| d.as_secs()),
            tx_bytes: tx,
            rx_bytes: rx,
        }
    }
}
