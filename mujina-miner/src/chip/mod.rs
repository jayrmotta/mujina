pub(crate) mod bm13xx;

use async_trait::async_trait;
use std::error::Error;
use std::fmt;

/// Represents a mining ASIC chip.
///
/// A chip receives mining jobs and returns nonces when it finds valid shares.
#[async_trait]
pub trait Chip: Send {
    /// Configure the chip with initial parameters.
    ///
    /// This typically includes setting PLL frequency, enabling cores,
    /// configuring version rolling, etc.
    async fn configure(&mut self) -> Result<(), ChipError>;
    
    /// Send a mining job to this chip.
    ///
    /// The chip will begin hashing immediately upon receiving the job.
    async fn send_job(&mut self, job: &MiningJob) -> Result<(), ChipError>;
    
    /// Poll for any nonces found by this chip.
    ///
    /// Returns immediately with any available nonces. Does not block
    /// waiting for nonces to be found.
    async fn poll_nonces(&mut self) -> Result<Vec<NonceResult>, ChipError>;
    
    /// Get chip information
    fn chip_info(&self) -> &ChipInfo;
    
    /// Get current chip statistics
    fn stats(&self) -> ChipStats;
}

/// Information about a chip
#[derive(Debug, Clone)]
pub struct ChipInfo {
    /// Chip model identifier (e.g., 0x1370 for BM1370)
    pub chip_id: u16,
    /// Number of hashing cores
    pub core_count: u32,
    /// Chip address on the serial bus
    pub address: u8,
    /// Whether the chip supports version rolling
    pub supports_version_rolling: bool,
}

/// Runtime statistics for a chip
#[derive(Debug, Clone, Default)]
pub struct ChipStats {
    /// Total number of nonces found
    pub nonces_found: u64,
    /// Total number of jobs sent
    pub jobs_sent: u64,
    /// Current frequency in MHz
    pub frequency_mhz: Option<u32>,
    /// Current temperature in Celsius
    pub temperature_c: Option<f32>,
}

/// A mining job to be processed by a chip
#[derive(Debug, Clone)]
pub struct MiningJob {
    /// Job ID for tracking
    pub job_id: u64,
    /// Block header data to hash
    pub header: [u8; 80],
    /// Target difficulty
    pub target: [u8; 32],
    /// Starting nonce value
    pub nonce_start: u32,
    /// Nonce range to search
    pub nonce_range: u32,
}

/// Result of finding a valid nonce
#[derive(Debug, Clone)]
pub struct NonceResult {
    /// Job ID this nonce is for
    pub job_id: u64,
    /// The nonce value found
    pub nonce: u32,
    /// Hash of the block with this nonce
    pub hash: [u8; 32],
}

/// Chip-specific errors
#[derive(Debug)]
pub enum ChipError {
    /// Communication error with chip
    Communication(String),
    /// Chip not responding
    Timeout,
    /// Invalid response from chip
    InvalidResponse(String),
    /// Configuration error
    Configuration(String),
    /// Other error
    Other(Box<dyn Error + Send + Sync>),
}

impl fmt::Display for ChipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChipError::Communication(msg) => write!(f, "Communication error: {}", msg),
            ChipError::Timeout => write!(f, "Chip timeout"),
            ChipError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            ChipError::Configuration(msg) => write!(f, "Configuration error: {}", msg),
            ChipError::Other(err) => write!(f, "Chip error: {}", err),
        }
    }
}

impl Error for ChipError {}
