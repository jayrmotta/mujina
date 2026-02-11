//! API data transfer objects.
//!
//! These types define the API contract shared between the server and
//! clients.

use serde::{Deserialize, Serialize};

/// Full miner state snapshot.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MinerState {
    pub uptime_secs: u64,
    /// Aggregate hashrate in hashes per second.
    pub hashrate: u64,
    pub shares_submitted: u64,
    pub boards: Vec<BoardState>,
    pub sources: Vec<SourceState>,
}

/// Board status.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BoardState {
    pub model: String,
    pub serial: Option<String>,
    pub fans: Vec<Fan>,
    pub temperatures: Vec<TemperatureSensor>,
    pub threads: Vec<ThreadState>,
}

/// Fan status.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Fan {
    pub label: String,
    pub rpm: u32,
    pub percent: u8,
    pub target_percent: u8,
}

/// Temperature sensor reading.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TemperatureSensor {
    pub label: String,
    pub temperature_c: f32,
}

/// Per-thread runtime status.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThreadState {
    pub name: String,
    /// Hashrate in hashes per second.
    pub hashrate: u64,
    pub is_active: bool,
}

/// Job source status.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceState {
    pub name: String,
}
