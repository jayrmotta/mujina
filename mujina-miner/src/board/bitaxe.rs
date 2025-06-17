use std::time::Duration;
use tokio::{io::AsyncWriteExt, time};
use tokio_serial::SerialStream;
use async_trait::async_trait;

use crate::board::{Board, BoardError, BoardInfo};
use crate::chip::Chip;

/// Bitaxe Gamma hashboard abstraction.
///
/// The Bitaxe Gamma running bitaxe-raw firmware provides a control interface for managing the
/// hashboard, including GPIO reset control and board initialization sequences.
pub struct BitaxeBoard {
    /// Serial control channel for board management commands
    control: SerialStream,
    /// Serial data channel for chip communication
    data: SerialStream,
    /// Discovered chips on this board
    chips: Vec<Box<dyn Chip>>,
}

impl BitaxeBoard {
    /// Creates a new BitaxeBoard instance with the provided serial streams.
    ///
    /// # Arguments
    /// * `control` - Serial stream for sending board control commands
    /// * `data` - Serial stream for chip communication
    ///
    /// # Returns
    /// A new BitaxeBoard instance ready for hardware operations
    pub fn new(control: SerialStream, data: SerialStream) -> Self {
        BitaxeBoard { 
            control,
            data,
            chips: Vec::new(),
        }
    }

    /// Performs a momentary reset of the mining chips via GPIO control.
    ///
    /// This function toggles the reset line low for 100ms, then high for 100ms
    /// to properly reset all connected mining chips.
    ///
    /// # Hardware Protocol
    /// - RSTN_LO: Pulls reset line low (active reset)
    /// - RSTN_HI: Releases reset line high (normal operation)
    /// - 100ms delays ensure proper reset timing for BM13xx chips
    ///
    /// # Errors
    /// Returns an error if serial communication fails during reset sequence
    ///
    /// # TODO
    /// Replace raw byte commands with proper codec and high-level message types
    pub async fn momentary_reset(&mut self) -> Result<(), std::io::Error> {
        const RSTN_LO: &[u8] = &[0x07, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00];
        const RSTN_HI: &[u8] = &[0x07, 0x00, 0x00, 0x00, 0x06, 0x00, 0x01];
        const WAIT: Duration = Duration::from_millis(100);

        self.control.write_all(RSTN_LO).await?;
        self.control.flush().await?;
        time::sleep(WAIT).await;

        self.control.write_all(RSTN_HI).await?;
        self.control.flush().await?;
        time::sleep(WAIT).await;

        Ok(())
    }
}

#[async_trait]
impl Board for BitaxeBoard {
    async fn reset(&mut self) -> Result<(), BoardError> {
        // Use the existing momentary_reset method
        self.momentary_reset().await?;
        Ok(())
    }
    
    async fn initialize(&mut self) -> Result<(), BoardError> {
        // Reset the board first
        self.reset().await?;
        
        // TODO: Implement chip discovery
        // For now, we'll need to:
        // 1. Send ReadRegister commands to discover chips
        // 2. Create BM13xx chip instances for each discovered chip
        // 3. Store them in self.chips
        
        // Placeholder for now
        tracing::info!("Board initialization not yet implemented");
        
        Ok(())
    }
    
    fn chips(&self) -> &[Box<dyn Chip>] {
        &self.chips
    }
    
    fn chips_mut(&mut self) -> &mut Vec<Box<dyn Chip>> {
        &mut self.chips
    }
    
    fn board_info(&self) -> BoardInfo {
        BoardInfo {
            model: "Bitaxe Gamma".to_string(),
            firmware_version: Some("bitaxe-raw".to_string()),
            serial_number: None, // Could be read from the board in future
        }
    }
}
