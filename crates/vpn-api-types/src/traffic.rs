//! Administrative traffic statistics. Directions are from the client's perspective.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficCounter {
    pub public_key: String,
    /// Bytes received by the gateway from this client.
    pub rx_bytes: u64,
    /// Bytes sent by the gateway to this client.
    pub tx_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficReport {
    pub generation: String,
    pub sequence: i64,
    pub counters: Vec<TrafficCounter>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TrafficBytes {
    pub upload_bytes: i64,
    pub download_bytes: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrafficPoint {
    pub timestamp: i64,
    pub upload_bytes: i64,
    pub download_bytes: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeerTraffic {
    pub peer_id: String,
    pub device_name: String,
    pub username: String,
    pub vpn_ip: String,
    pub upload_bytes: i64,
    pub download_bytes: i64,
    pub upload_bps: Option<f64>,
    pub download_bps: Option<f64>,
    pub sampled_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrafficSource {
    pub id: String,
    pub name: String,
    pub sampled_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrafficDashboard {
    pub from: i64,
    pub to: i64,
    pub first_sample_at: Option<i64>,
    pub sources: Vec<TrafficSource>,
    pub period: TrafficBytes,
    pub today: TrafficBytes,
    pub total: TrafficBytes,
    pub upload_bps: Option<f64>,
    pub download_bps: Option<f64>,
    pub series: Vec<TrafficPoint>,
    pub peers: Vec<PeerTraffic>,
}
