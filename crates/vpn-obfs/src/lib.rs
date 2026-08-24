//! 独立的 WireGuard UDP 混淆线协议 v1。
//!
//! 本层只隐藏 WireGuard 报文特征；节点身份认证、数据机密性和数据包重放保护仍由
//! WireGuard 提供。协议不兼容 swgp-go，且不包含 session id 或外层序号。

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit as AesKeyInit};
use aes::Aes256;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::{rngs::OsRng, Rng, RngCore};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

/// 握手时间戳允许的最大时钟偏差。
pub const CLOCK_SKEW: Duration = Duration::from_secs(15);
/// 已认证握手 nonce 的保留时间。
pub const REPLAY_TTL: Duration = Duration::from_secs(30);
/// v1 支持的最小 PSK 长度。
pub const PSK_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const BLOCK_LEN: usize = 16;
const MAX_REPLAY_ENTRIES: usize = 65_536;

/// 混淆线协议模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 数据包只加密首个 16 字节；握手包额外做 AEAD 和随机填充。
    LowOverheadV1,
    /// 每个包都做 AEAD，并填充到配置的路径 UDP 上限。
    ParanoidV1,
}

/// 数据流方向，用于 HKDF 密钥隔离。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 客户端发往服务端。
    ClientToServer,
    /// 服务端发往客户端。
    ServerToClient,
}

/// 协议处理错误。错误不包含密钥或载荷内容。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ObfsError {
    /// PSK 长度不合法。
    #[error("混淆 PSK 必须恰好为 32 字节")]
    InvalidKey,
    /// 报文长度不合法。
    #[error("混淆数据报长度不合法")]
    InvalidLength,
    /// WireGuard 报文头、类型或固定长度不合法。
    #[error("WireGuard 数据报格式不合法")]
    InvalidWireGuard,
    /// AEAD 认证失败。
    #[error("混淆数据报认证失败")]
    Authentication,
    /// 握手时间戳超出允许窗口。
    #[error("混淆握手时间戳超出允许窗口")]
    ClockSkew,
    /// 握手 nonce 已使用。
    #[error("检测到重复的混淆握手")]
    Replay,
}

#[derive(Clone)]
struct DerivedKeys {
    aes: Zeroizing<[u8; 32]>,
    aead: Zeroizing<[u8; 32]>,
}

/// 30 秒握手 nonce 重放池。
#[derive(Debug, Default)]
pub struct ReplayCache {
    entries: VecDeque<([u8; NONCE_LEN], Instant)>,
    seen: HashSet<[u8; NONCE_LEN]>,
}

impl ReplayCache {
    /// 创建空重放池。
    pub fn new() -> Self {
        Self::default()
    }

    /// 清除过期项，并在 nonce 未出现时记录它。
    pub fn check_and_insert(&mut self, nonce: [u8; NONCE_LEN], now: Instant) -> bool {
        while self
            .entries
            .front()
            .is_some_and(|(_, seen)| now.duration_since(*seen) > REPLAY_TTL)
        {
            if let Some((expired, _)) = self.entries.pop_front() {
                self.seen.remove(&expired);
            }
        }
        if self.seen.contains(&nonce) {
            return false;
        }
        if self.entries.len() >= MAX_REPLAY_ENTRIES {
            return false;
        }
        self.entries.push_back((nonce, now));
        self.seen.insert(nonce);
        true
    }
}

/// 单方向 v1 编解码器。
#[derive(Clone)]
pub struct Codec {
    mode: Mode,
    keys: DerivedKeys,
    max_datagram: usize,
}

impl std::fmt::Debug for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Codec")
            .field("mode", &self.mode)
            .field("max_datagram", &self.max_datagram)
            .finish_non_exhaustive()
    }
}

impl Codec {
    /// 从 32 字节 PSK 派生指定方向和用途的密钥。
    pub fn new(
        psk: &[u8],
        mode: Mode,
        direction: Direction,
        max_datagram: usize,
    ) -> Result<Self, ObfsError> {
        if psk.len() != PSK_LEN || max_datagram < 128 {
            return Err(if psk.len() != PSK_LEN {
                ObfsError::InvalidKey
            } else {
                ObfsError::InvalidLength
            });
        }
        let hk = Hkdf::<Sha256>::new(Some(b"vpn-obfs-v1"), psk);
        let direction = match direction {
            Direction::ClientToServer => b"client-to-server".as_slice(),
            Direction::ServerToClient => b"server-to-client".as_slice(),
        };
        let mut aes = Zeroizing::new([0u8; 32]);
        let mut aead = Zeroizing::new([0u8; 32]);
        let mut aes_info = Vec::from(direction);
        aes_info.extend_from_slice(b"/aes-first-block");
        let mut aead_info = Vec::from(direction);
        aead_info.extend_from_slice(b"/xchacha20poly1305");
        hk.expand(&aes_info, aes.as_mut())
            .map_err(|_| ObfsError::InvalidKey)?;
        hk.expand(&aead_info, aead.as_mut())
            .map_err(|_| ObfsError::InvalidKey)?;
        Ok(Self {
            mode,
            keys: DerivedKeys { aes, aead },
            max_datagram,
        })
    }

    /// 编码一个已由 boringtun 生成的 WireGuard UDP 数据报。
    pub fn encode(&self, packet: &[u8]) -> Result<Vec<u8>, ObfsError> {
        let packet_type = validate_wireguard(packet, self.max_datagram)?;
        match self.mode {
            Mode::LowOverheadV1 => self.encode_low(packet, packet_type),
            Mode::ParanoidV1 => self.encode_paranoid(packet, packet_type),
        }
    }

    /// 解码并认证数据报。只有认证和 WireGuard 格式校验都成功才返回载荷。
    pub fn decode(&self, datagram: &[u8], replay: &mut ReplayCache) -> Result<Vec<u8>, ObfsError> {
        match self.mode {
            Mode::LowOverheadV1 => self.decode_low(datagram, replay),
            Mode::ParanoidV1 => self.decode_paranoid(datagram, replay),
        }
    }

    fn aes_block(&self, input: &[u8], encrypt: bool) -> [u8; BLOCK_LEN] {
        let cipher = Aes256::new_from_slice(self.keys.aes.as_ref()).expect("fixed AES key");
        let mut block = aes::Block::default();
        block.copy_from_slice(&input[..BLOCK_LEN]);
        if encrypt {
            cipher.encrypt_block(&mut block);
        } else {
            cipher.decrypt_block(&mut block);
        }
        block.into()
    }

    fn encode_low(&self, packet: &[u8], packet_type: u8) -> Result<Vec<u8>, ObfsError> {
        let first = self.aes_block(packet, true);
        if packet_type == 4 {
            let mut out = Vec::with_capacity(packet.len());
            out.extend_from_slice(&first);
            out.extend_from_slice(&packet[BLOCK_LEN..]);
            return Ok(out);
        }
        let fixed = BLOCK_LEN + TAG_LEN + NONCE_LEN + 10;
        if packet.len() + TAG_LEN + NONCE_LEN + 10 > self.max_datagram {
            return Err(ObfsError::InvalidLength);
        }
        let max_padding = self.max_datagram - packet.len() - TAG_LEN - NONCE_LEN - 10;
        let padding_len = OsRng.gen_range(0..=max_padding);
        let mut plaintext = Vec::with_capacity(packet.len() - BLOCK_LEN + padding_len + 10);
        plaintext.extend_from_slice(&packet[BLOCK_LEN..]);
        let old_len = plaintext.len();
        plaintext.resize(old_len + padding_len, 0);
        OsRng.fill_bytes(&mut plaintext[old_len..]);
        plaintext.extend_from_slice(&unix_seconds()?.to_be_bytes());
        plaintext.extend_from_slice(&(packet.len() as u16).to_be_bytes());
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new_from_slice(self.keys.aead.as_ref())
            .map_err(|_| ObfsError::InvalidKey)?;
        let xnonce = XNonce::from(nonce);
        let ciphertext = cipher
            .encrypt(
                &xnonce,
                Payload {
                    msg: &plaintext,
                    aad: &first,
                },
            )
            .map_err(|_| ObfsError::Authentication)?;
        let mut out = Vec::with_capacity(fixed + ciphertext.len());
        out.extend_from_slice(&first);
        out.extend_from_slice(&ciphertext);
        out.extend_from_slice(&nonce);
        Ok(out)
    }

    fn decode_low(&self, datagram: &[u8], replay: &mut ReplayCache) -> Result<Vec<u8>, ObfsError> {
        if datagram.len() < BLOCK_LEN {
            return Err(ObfsError::InvalidLength);
        }
        let first = self.aes_block(datagram, false);
        let packet_type = validate_header(&first)?;
        if packet_type == 4 {
            let mut packet = Vec::with_capacity(datagram.len());
            packet.extend_from_slice(&first);
            packet.extend_from_slice(&datagram[BLOCK_LEN..]);
            validate_wireguard(&packet, self.max_datagram)?;
            return Ok(packet);
        }
        if datagram.len() < BLOCK_LEN + TAG_LEN + NONCE_LEN + 10 {
            return Err(ObfsError::InvalidLength);
        }
        let nonce_offset = datagram.len() - NONCE_LEN;
        let nonce: [u8; NONCE_LEN] = datagram[nonce_offset..]
            .try_into()
            .map_err(|_| ObfsError::InvalidLength)?;
        let cipher = XChaCha20Poly1305::new_from_slice(self.keys.aead.as_ref())
            .map_err(|_| ObfsError::InvalidKey)?;
        let xnonce = XNonce::from(nonce);
        let plaintext = cipher
            .decrypt(
                &xnonce,
                Payload {
                    msg: &datagram[BLOCK_LEN..nonce_offset],
                    aad: &datagram[..BLOCK_LEN],
                },
            )
            .map_err(|_| ObfsError::Authentication)?;
        if plaintext.len() < 10 {
            return Err(ObfsError::InvalidLength);
        }
        let metadata = plaintext.len() - 10;
        let timestamp = u64::from_be_bytes(
            plaintext[metadata..metadata + 8]
                .try_into()
                .map_err(|_| ObfsError::InvalidLength)?,
        );
        check_timestamp(timestamp)?;
        let original_len = u16::from_be_bytes(
            plaintext[metadata + 8..]
                .try_into()
                .map_err(|_| ObfsError::InvalidLength)?,
        ) as usize;
        if original_len < BLOCK_LEN || original_len - BLOCK_LEN > metadata {
            return Err(ObfsError::InvalidLength);
        }
        let mut packet = Vec::with_capacity(original_len);
        packet.extend_from_slice(&first);
        packet.extend_from_slice(&plaintext[..original_len - BLOCK_LEN]);
        validate_wireguard(&packet, self.max_datagram)?;
        if !replay.check_and_insert(nonce, Instant::now()) {
            return Err(ObfsError::Replay);
        }
        Ok(packet)
    }

    fn encode_paranoid(&self, packet: &[u8], packet_type: u8) -> Result<Vec<u8>, ObfsError> {
        let plaintext_len = self.max_datagram - NONCE_LEN - TAG_LEN;
        let metadata_len = if packet_type == 4 { 2 } else { 10 };
        if metadata_len + packet.len() > plaintext_len || packet.len() > u16::MAX as usize {
            return Err(ObfsError::InvalidLength);
        }
        let mut plaintext = vec![0u8; plaintext_len];
        OsRng.fill_bytes(&mut plaintext);
        plaintext[..2].copy_from_slice(&(packet.len() as u16).to_be_bytes());
        let packet_offset = if packet_type == 4 {
            2
        } else {
            plaintext[2..10].copy_from_slice(&unix_seconds()?.to_be_bytes());
            10
        };
        plaintext[packet_offset..packet_offset + packet.len()].copy_from_slice(packet);
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new_from_slice(self.keys.aead.as_ref())
            .map_err(|_| ObfsError::InvalidKey)?;
        let xnonce = XNonce::from(nonce);
        let ciphertext = cipher
            .encrypt(&xnonce, plaintext.as_ref())
            .map_err(|_| ObfsError::Authentication)?;
        let mut out = Vec::with_capacity(self.max_datagram);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    fn decode_paranoid(
        &self,
        datagram: &[u8],
        replay: &mut ReplayCache,
    ) -> Result<Vec<u8>, ObfsError> {
        if datagram.len() != self.max_datagram || datagram.len() < NONCE_LEN + TAG_LEN + 10 {
            return Err(ObfsError::InvalidLength);
        }
        let nonce: [u8; NONCE_LEN] = datagram[..NONCE_LEN]
            .try_into()
            .map_err(|_| ObfsError::InvalidLength)?;
        let cipher = XChaCha20Poly1305::new_from_slice(self.keys.aead.as_ref())
            .map_err(|_| ObfsError::InvalidKey)?;
        let xnonce = XNonce::from(nonce);
        let plaintext = cipher
            .decrypt(&xnonce, &datagram[NONCE_LEN..])
            .map_err(|_| ObfsError::Authentication)?;
        let original_len = u16::from_be_bytes(
            plaintext[..2]
                .try_into()
                .map_err(|_| ObfsError::InvalidLength)?,
        ) as usize;
        if original_len < BLOCK_LEN {
            return Err(ObfsError::InvalidLength);
        }
        if 2 + original_len <= plaintext.len() {
            let packet = &plaintext[2..2 + original_len];
            if validate_wireguard(packet, self.max_datagram).ok() == Some(4) {
                return Ok(packet.to_vec());
            }
        }
        if 10 + original_len > plaintext.len() {
            return Err(ObfsError::InvalidLength);
        }
        let timestamp = u64::from_be_bytes(
            plaintext[2..10]
                .try_into()
                .map_err(|_| ObfsError::InvalidLength)?,
        );
        check_timestamp(timestamp)?;
        let packet = &plaintext[10..10 + original_len];
        let packet_type = validate_wireguard(packet, self.max_datagram)?;
        if packet_type == 4 {
            return Err(ObfsError::InvalidWireGuard);
        }
        if !replay.check_and_insert(nonce, Instant::now()) {
            return Err(ObfsError::Replay);
        }
        Ok(packet.to_vec())
    }
}

/// 严格验证 WireGuard UDP 报文并返回消息类型。
pub fn validate_wireguard(packet: &[u8], max_len: usize) -> Result<u8, ObfsError> {
    if packet.len() < BLOCK_LEN || packet.len() > max_len {
        return Err(ObfsError::InvalidLength);
    }
    let packet_type = validate_header(packet)?;
    let valid = match packet_type {
        1 => packet.len() == 148,
        2 => packet.len() == 92,
        3 => packet.len() == 64,
        4 => packet.len() >= 32 && packet.len().is_multiple_of(16),
        _ => false,
    };
    if valid {
        Ok(packet_type)
    } else {
        Err(ObfsError::InvalidWireGuard)
    }
}

fn validate_header(packet: &[u8]) -> Result<u8, ObfsError> {
    if packet.len() < 4 || packet[1..4] != [0, 0, 0] || !(1..=4).contains(&packet[0]) {
        return Err(ObfsError::InvalidWireGuard);
    }
    Ok(packet[0])
}

fn unix_seconds() -> Result<u64, ObfsError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| ObfsError::ClockSkew)
}

fn check_timestamp(timestamp: u64) -> Result<(), ObfsError> {
    check_timestamp_at(timestamp, unix_seconds()?)
}

fn check_timestamp_at(timestamp: u64, now: u64) -> Result<(), ObfsError> {
    if now.abs_diff(timestamp) > CLOCK_SKEW.as_secs() {
        Err(ObfsError::ClockSkew)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(packet_type: u8) -> Vec<u8> {
        let len = match packet_type {
            1 => 148,
            2 => 92,
            3 => 64,
            4 => 64,
            _ => 16,
        };
        let mut packet = vec![0u8; len];
        packet[0] = packet_type;
        OsRng.fill_bytes(&mut packet[4..]);
        packet
    }

    #[test]
    fn both_modes_round_trip_all_wireguard_types() {
        for mode in [Mode::LowOverheadV1, Mode::ParanoidV1] {
            for packet_type in 1..=4 {
                let original = packet(packet_type);
                let codec = Codec::new(&[7u8; 32], mode, Direction::ClientToServer, 1458).unwrap();
                let encoded = codec.encode(&original).unwrap();
                let decoded = codec.decode(&encoded, &mut ReplayCache::new()).unwrap();
                assert_eq!(decoded, original);
            }
        }
    }

    #[test]
    fn low_data_has_zero_size_overhead_and_paranoid_is_fixed_size() {
        let data = packet(4);
        let low = Codec::new(
            &[1u8; 32],
            Mode::LowOverheadV1,
            Direction::ClientToServer,
            1458,
        )
        .unwrap();
        let paranoid = Codec::new(
            &[1u8; 32],
            Mode::ParanoidV1,
            Direction::ClientToServer,
            1458,
        )
        .unwrap();
        assert_eq!(low.encode(&data).unwrap().len(), data.len());
        assert_eq!(paranoid.encode(&data).unwrap().len(), 1458);
    }

    #[test]
    fn low_data_wire_vector_is_stable() {
        let mut data = vec![0u8; 64];
        data[0] = 4;
        let codec = Codec::new(
            &[1u8; 32],
            Mode::LowOverheadV1,
            Direction::ClientToServer,
            1458,
        )
        .unwrap();
        let encoded = codec.encode(&data).unwrap();
        assert_eq!(
            &encoded[..16],
            &[138, 84, 168, 10, 215, 134, 246, 75, 255, 255, 59, 42, 254, 129, 197, 94]
        );
        assert_eq!(&encoded[16..], &data[16..]);
    }

    #[test]
    fn rejects_tamper_replay_reserved_unknown_and_boundaries() {
        let codec = Codec::new(
            &[9u8; 32],
            Mode::ParanoidV1,
            Direction::ClientToServer,
            1458,
        )
        .unwrap();
        let mut encoded = codec.encode(&packet(1)).unwrap();
        encoded[100] ^= 1;
        assert_eq!(
            codec.decode(&encoded, &mut ReplayCache::new()),
            Err(ObfsError::Authentication)
        );

        let encoded = codec.encode(&packet(1)).unwrap();
        let mut replay = ReplayCache::new();
        codec.decode(&encoded, &mut replay).unwrap();
        assert_eq!(codec.decode(&encoded, &mut replay), Err(ObfsError::Replay));

        let mut bad = packet(4);
        bad[1] = 1;
        assert_eq!(codec.encode(&bad), Err(ObfsError::InvalidWireGuard));
        bad[0] = 9;
        bad[1] = 0;
        assert_eq!(codec.encode(&bad), Err(ObfsError::InvalidWireGuard));
        assert_eq!(codec.encode(&[0u8; 15]), Err(ObfsError::InvalidLength));
    }

    #[test]
    fn low_handshake_rejects_tamper_and_replay() {
        let codec = Codec::new(
            &[9u8; 32],
            Mode::LowOverheadV1,
            Direction::ClientToServer,
            1458,
        )
        .unwrap();
        let encoded = codec.encode(&packet(1)).unwrap();
        let mut tampered = encoded.clone();
        tampered[20] ^= 1;
        assert_eq!(
            codec.decode(&tampered, &mut ReplayCache::new()),
            Err(ObfsError::Authentication)
        );
        let mut replay = ReplayCache::new();
        codec.decode(&encoded, &mut replay).unwrap();
        assert_eq!(codec.decode(&encoded, &mut replay), Err(ObfsError::Replay));
    }

    #[test]
    fn clock_and_replay_windows_include_the_boundary() {
        assert_eq!(check_timestamp_at(985, 1000), Ok(()));
        assert_eq!(check_timestamp_at(1015, 1000), Ok(()));
        assert_eq!(check_timestamp_at(984, 1000), Err(ObfsError::ClockSkew));
        assert_eq!(check_timestamp_at(1016, 1000), Err(ObfsError::ClockSkew));

        let mut replay = ReplayCache::new();
        let now = Instant::now();
        assert!(replay.check_and_insert([1; NONCE_LEN], now));
        assert!(!replay.check_and_insert([1; NONCE_LEN], now + REPLAY_TTL));
        assert!(replay.check_and_insert([1; NONCE_LEN], now + REPLAY_TTL + Duration::from_nanos(1)));
    }

    #[test]
    fn direction_keys_are_separated() {
        let c2s = Codec::new(
            &[3u8; 32],
            Mode::LowOverheadV1,
            Direction::ClientToServer,
            1458,
        )
        .unwrap();
        let s2c = Codec::new(
            &[3u8; 32],
            Mode::LowOverheadV1,
            Direction::ServerToClient,
            1458,
        )
        .unwrap();
        let encoded = c2s.encode(&packet(4)).unwrap();
        assert!(s2c.decode(&encoded, &mut ReplayCache::new()).is_err());
    }

    #[test]
    fn maximum_mtu_packets_fit_without_truncation() {
        let mut low_packet = vec![0u8; 1456];
        low_packet[0] = 4;
        let low = Codec::new(
            &[4u8; 32],
            Mode::LowOverheadV1,
            Direction::ClientToServer,
            1472,
        )
        .unwrap();
        let encoded = low.encode(&low_packet).unwrap();
        assert_eq!(encoded.len(), 1456);
        assert_eq!(
            low.decode(&encoded, &mut ReplayCache::new()).unwrap(),
            low_packet
        );

        let mut paranoid_packet = vec![0u8; 1424];
        paranoid_packet[0] = 4;
        let paranoid = Codec::new(
            &[4u8; 32],
            Mode::ParanoidV1,
            Direction::ClientToServer,
            1472,
        )
        .unwrap();
        let encoded = paranoid.encode(&paranoid_packet).unwrap();
        assert_eq!(encoded.len(), 1472);
        assert_eq!(
            paranoid.decode(&encoded, &mut ReplayCache::new()).unwrap(),
            paranoid_packet
        );
    }
}
