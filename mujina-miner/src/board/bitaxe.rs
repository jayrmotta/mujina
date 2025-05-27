use std::time::Duration;
use tokio::{io::AsyncWriteExt, time};
use tokio_serial::SerialStream;

/// Bitaxe Gamma hashboard abstraction.
///
/// The Bitaxe Gamma running bitaxe-raw firmware provides a control interface for managing the
/// hashboard, including GPIO reset control and board initialization sequences.
pub struct Board {
    /// Serial control channel for board management commands
    control: SerialStream,
}

impl Board {
    /// Creates a new Board instance with the provided control serial stream.
    ///
    /// # Arguments
    /// * `control` - Serial stream for sending board control commands
    ///
    /// # Returns
    /// A new Board instance ready for hardware operations
    pub fn new(control: SerialStream) -> Self {
        Board { control }
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
