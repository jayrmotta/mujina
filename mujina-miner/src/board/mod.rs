pub(crate) mod bitaxe;

use async_trait::async_trait;
use std::error::Error;
use std::fmt;

use crate::chip::{Chip, ChipError};

/// Represents a mining board containing one or more ASIC chips.
/// 
/// A board provides the interface between the host system and mining chips,
/// handling hardware initialization, reset, and chip discovery.
#[async_trait]
pub trait Board: Send {
    /// Reset the board hardware.
    /// 
    /// This typically involves toggling GPIO pins or sending reset commands
    /// to bring the board to a known state.
    async fn reset(&mut self) -> Result<(), BoardError>;
    
    /// Initialize the board and discover connected chips.
    /// 
    /// After initialization, chips should be accessible via `chips()` or `chips_mut()`.
    async fn initialize(&mut self) -> Result<(), BoardError>;
    
    /// Get a reference to all discovered chips on this board.
    fn chips(&self) -> &[Box<dyn Chip>];
    
    /// Get a mutable reference to all discovered chips on this board.
    fn chips_mut(&mut self) -> &mut Vec<Box<dyn Chip>>;
    
    /// Get board identification/info
    fn board_info(&self) -> BoardInfo;
}

/// Information about a board
#[derive(Debug, Clone)]
pub struct BoardInfo {
    /// Board model/type (e.g., "Bitaxe Gamma")
    pub model: String,
    /// Board firmware version if available
    pub firmware_version: Option<String>,
    /// Serial number if available
    pub serial_number: Option<String>,
}

/// Board-specific errors
#[derive(Debug)]
pub enum BoardError {
    /// Hardware initialization failed
    InitializationFailed(String),
    /// Communication error with board
    Communication(std::io::Error),
    /// Chip-related error
    Chip(ChipError),
    /// GPIO or hardware control error
    HardwareControl(String),
    /// Other error
    Other(Box<dyn Error + Send + Sync>),
}

impl fmt::Display for BoardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BoardError::InitializationFailed(msg) => write!(f, "Board initialization failed: {}", msg),
            BoardError::Communication(err) => write!(f, "Board communication error: {}", err),
            BoardError::Chip(err) => write!(f, "Chip error: {}", err),
            BoardError::HardwareControl(msg) => write!(f, "Hardware control error: {}", msg),
            BoardError::Other(err) => write!(f, "Board error: {}", err),
        }
    }
}

impl Error for BoardError {}

impl From<std::io::Error> for BoardError {
    fn from(err: std::io::Error) -> Self {
        BoardError::Communication(err)
    }
}

impl From<ChipError> for BoardError {
    fn from(err: ChipError) -> Self {
        BoardError::Chip(err)
    }
}
