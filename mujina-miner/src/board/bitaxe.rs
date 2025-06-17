use std::time::Duration;
use tokio::{io::AsyncWriteExt, time};
use tokio_serial::SerialStream;
use async_trait::async_trait;
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, FramedWrite};
use futures::sink::SinkExt;

use crate::board::{Board, BoardError, BoardInfo, BoardEvent, JobCompleteReason};
use crate::chip::{ChipInfo, MiningJob};
use crate::chip::bm13xx::{self, BM13xxProtocol};

/// Bitaxe Gamma hashboard abstraction.
///
/// The Bitaxe Gamma running bitaxe-raw firmware provides a control interface for managing the
/// hashboard, including GPIO reset control and board initialization sequences.
pub struct BitaxeBoard {
    /// Serial control channel for board management commands
    control: SerialStream,
    /// Serial data channel for chip communication
    data: SerialStream,
    /// Protocol handler for chip communication
    protocol: BM13xxProtocol,
    /// Discovered chip information (passive record-keeping)
    chip_infos: Vec<ChipInfo>,
    /// Channel for sending board events
    event_tx: Option<tokio::sync::mpsc::Sender<BoardEvent>>,
    /// Current job ID
    current_job_id: Option<u64>,
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
    /// 
    /// # Design Note
    /// In the future, a DeviceManager will create boards when USB devices
    /// are detected (by VID/PID) and pass already-opened serial streams.
    pub fn new(control: SerialStream, data: SerialStream) -> Self {
        BitaxeBoard { 
            control,
            data,
            protocol: BM13xxProtocol::new(false), // TODO: Make version rolling configurable
            chip_infos: Vec::new(),
            event_tx: None,
            current_job_id: None,
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
    
    /// Discover chips connected to this board.
    /// 
    /// Sends broadcast ReadRegister commands and collects responses
    /// to identify all chips on the serial bus.
    async fn discover_chips(&mut self) -> Result<(), BoardError> {
        // Split the data stream for reading and writing
        let (read_stream, write_stream) = tokio::io::split(&mut self.data);
        let mut framed_read = FramedRead::new(read_stream, bm13xx::FrameCodec::default());
        let mut framed_write = FramedWrite::new(write_stream, bm13xx::FrameCodec::default());
        
        // Send a broadcast read to discover chips
        let discover_cmd = BM13xxProtocol::discover_chips();
        
        framed_write.send(discover_cmd).await
            .map_err(|e| BoardError::Communication(e))?;
        
        // Wait a bit for responses
        let timeout = Duration::from_millis(500);
        let deadline = tokio::time::Instant::now() + timeout;
        
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                response = framed_read.next() => {
                    match response {
                        Some(Ok(bm13xx::Response::ReadRegister {
                            chip_address: _,
                            register: bm13xx::Register::ChipAddress { chip_id, core_count, address }
                        })) => {
                            tracing::info!("Discovered chip 0x{chip_id:x} at address {address} with {core_count} cores");
                            
                            let chip_info = ChipInfo {
                                chip_id,
                                core_count: core_count.into(),
                                address,
                                supports_version_rolling: true, // BM1370 supports this
                            };
                            
                            self.chip_infos.push(chip_info);
                        }
                        Some(Ok(_)) => {
                            tracing::warn!("Unexpected response during chip discovery");
                        }
                        Some(Err(e)) => {
                            tracing::error!("Error during chip discovery: {e}");
                        }
                        None => break,
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    break;
                }
            }
        }
        
        if self.chip_infos.is_empty() {
            Err(BoardError::InitializationFailed("No chips discovered".to_string()))
        } else {
            Ok(())
        }
    }
    
    /// Spawn a task to monitor chip responses and emit events
    fn spawn_event_monitor(&mut self) {
        // TODO: Implement event monitoring
        // This task should:
        // 1. Read responses from the data stream
        // 2. Decode using protocol
        // 3. Emit appropriate events (NonceFound, ChipError, etc.)
        // 4. Track job completion (by timeout or chip signal)
        
        tracing::warn!("Event monitor not yet implemented");
    }
}

#[async_trait]
impl Board for BitaxeBoard {
    async fn reset(&mut self) -> Result<(), BoardError> {
        // Use the existing momentary_reset method
        self.momentary_reset().await?;
        Ok(())
    }
    
    async fn initialize(&mut self) -> Result<tokio::sync::mpsc::Receiver<BoardEvent>, BoardError> {
        // Reset the board first
        self.reset().await?;
        
        // Discover connected chips
        self.discover_chips().await?;
        
        tracing::info!("Board initialized with {} chip(s)", self.chip_infos.len());
        
        // Create event channel
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        self.event_tx = Some(tx);
        
        // TODO: Spawn task to monitor chip responses and emit events
        self.spawn_event_monitor();
        
        Ok(rx)
    }
    
    fn chip_count(&self) -> usize {
        self.chip_infos.len()
    }
    
    fn chip_infos(&self) -> &[ChipInfo] {
        &self.chip_infos
    }
    
    async fn send_job(&mut self, job: &MiningJob) -> Result<(), BoardError> {
        // TODO: Implement job distribution
        // 1. Encode job using protocol
        // 2. Send to each chip
        // 3. Track job ID for completion
        // 4. Start completion timer/estimation
        
        // Store current job ID
        self.current_job_id = Some(job.job_id);
        
        // For now, just a placeholder
        tracing::warn!("send_job not yet implemented");
        Ok(())
    }
    
    async fn cancel_job(&mut self, job_id: u64) -> Result<(), BoardError> {
        // TODO: Implement job cancellation
        // This might involve sending a new dummy job or reset command
        
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(BoardEvent::JobComplete {
                job_id,
                reason: JobCompleteReason::Cancelled,
            }).await;
        }
        
        Ok(())
    }
    
    fn board_info(&self) -> BoardInfo {
        BoardInfo {
            model: "Bitaxe Gamma".to_string(),
            firmware_version: Some("bitaxe-raw".to_string()),
            serial_number: None, // Could be read from the board in future
        }
    }
}
