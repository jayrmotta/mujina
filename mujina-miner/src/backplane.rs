//! Backplane for board communication and lifecycle management.
//!
//! The Backplane acts as the communication substrate between mining boards and
//! the scheduler. Like a hardware backplane, it provides connection points for
//! boards to plug into, routes events between components, and manages board
//! lifecycle (hotplug, emergency shutdown, etc.).

use crate::board::{Board, BoardDescriptor};
use crate::error::Result;
use crate::hash_thread::{HashThread, ThreadRemovalSignal};
use crate::transport::{TransportEvent, UsbDeviceInfo};
use std::collections::HashMap;
use tokio::sync::{mpsc, watch};

/// Board registry that uses inventory to find registered boards.
pub struct BoardRegistry;

impl BoardRegistry {
    /// Find a board descriptor that can handle this USB device.
    pub fn find_descriptor(&self, vid: u16, pid: u16) -> Option<&'static BoardDescriptor> {
        inventory::iter::<BoardDescriptor>().find(|desc| desc.vid == vid && desc.pid == pid)
    }

    /// Create a board from USB device info.
    pub async fn create_board(&self, device: UsbDeviceInfo) -> Result<Box<dyn Board + Send>> {
        let desc = self
            .find_descriptor(device.vid, device.pid)
            .ok_or_else(|| {
                crate::error::Error::Other(format!(
                    "No board registered for {:04x}:{:04x}",
                    device.vid, device.pid
                ))
            })?;

        tracing::info!("Creating {} board from USB device", desc.name);
        (desc.create_fn)(device).await
    }
}

/// Backplane that connects boards to the scheduler.
///
/// Acts as the communication substrate between mining boards and the work
/// scheduler. Boards plug into the backplane, which routes their events and
/// manages their lifecycle.
pub struct Backplane {
    registry: BoardRegistry,
    /// Boards with their removal signals for lifecycle management
    boards: HashMap<String, (Box<dyn Board + Send>, watch::Sender<ThreadRemovalSignal>)>,
    event_rx: mpsc::Receiver<TransportEvent>,
    /// Channel to send hash threads to the scheduler
    scheduler_tx: mpsc::Sender<Vec<Box<dyn HashThread>>>,
}

impl Backplane {
    /// Create a new backplane.
    pub fn new(
        event_rx: mpsc::Receiver<TransportEvent>,
        scheduler_tx: mpsc::Sender<Vec<Box<dyn HashThread>>>,
    ) -> Self {
        Self {
            registry: BoardRegistry,
            boards: HashMap::new(),
            event_rx,
            scheduler_tx,
        }
    }

    /// Run the backplane event loop.
    pub async fn run(&mut self) -> Result<()> {
        while let Some(event) = self.event_rx.recv().await {
            match event {
                TransportEvent::Usb(usb_event) => {
                    self.handle_usb_event(usb_event).await?;
                } // Future: handle other transport types
            }
        }

        Ok(())
    }

    /// Handle USB transport events.
    async fn handle_usb_event(
        &mut self,
        event: crate::transport::usb::TransportEvent,
    ) -> Result<()> {
        use crate::transport::usb::TransportEvent;

        match event {
            TransportEvent::UsbDeviceConnected(device_info) => {
                let vid = device_info.vid;
                let pid = device_info.pid;
                tracing::info!("USB device connected: {:04x}:{:04x}", vid, pid);

                // Try to create a board from this USB device
                match self.registry.create_board(device_info).await {
                    Ok(mut board) => {
                        let board_info = board.board_info();
                        let board_id = board_info
                            .serial_number
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string());

                        tracing::info!("Created {} board (serial: {})", board_info.model, board_id);

                        // Create hash threads from the board
                        match board.create_hash_threads().await {
                            Ok((threads, removal_tx)) => {
                                tracing::info!(
                                    "Created {} hash thread(s) from board {}",
                                    threads.len(),
                                    board_id
                                );

                                // Store board with removal signal for lifecycle management
                                self.boards.insert(board_id.clone(), (board, removal_tx));

                                // Send threads to scheduler
                                if let Err(e) = self.scheduler_tx.send(threads).await {
                                    tracing::error!("Failed to send threads to scheduler: {}", e);
                                } else {
                                    tracing::info!(
                                        "Threads from board {} sent to scheduler",
                                        board_id
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to create hash threads from board {}: {}",
                                    board_id,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to create board for device {:04x}:{:04x}: {}",
                            vid,
                            pid,
                            e
                        );
                    }
                }
            }
            TransportEvent::UsbDeviceDisconnected { device_path } => {
                tracing::info!("USB device disconnected: {}", device_path);
                // TODO: Remove board from active boards and notify scheduler
            }
        }

        Ok(())
    }
}
