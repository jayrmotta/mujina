//! The scheduler module manages the distribution of mining jobs to hash boards
//! and ASIC chips.
//!
//! This is a work-in-progress. It's currently the main and initial place where
//! functionality is added, after which the functionality is refactored out to
//! where it belongs.

use tokio_serial::{self, SerialPortBuilderExt};
use tokio_util::sync::CancellationToken;

use crate::board::{bitaxe::BitaxeBoard, Board, BoardEvent};
use crate::tracing::prelude::*;

const CONTROL_SERIAL: &str = "/dev/ttyACM0";
const DATA_SERIAL: &str = "/dev/ttyACM1";

pub async fn task(running: CancellationToken) {
    trace!("Scheduler task started.");

    // In the future, a DeviceManager would create boards based on USB detection
    // For now, we'll create a single board with known serial ports
    let control_port = tokio_serial::new(CONTROL_SERIAL, 115200)
        .open_native_async()
        .expect("failed to open control serial port");
    
    let data_port = tokio_serial::new(DATA_SERIAL, 115200)
        .open_native_async()
        .expect("failed to open data serial port");
    
    let mut board = BitaxeBoard::new(control_port, data_port);
    
    // Initialize the board (reset + chip discovery)
    let mut event_rx = match board.initialize().await {
        Ok(rx) => {
            info!("Board initialized successfully");
            info!("Found {} chip(s)", board.chip_count());
            rx
        }
        Err(e) => {
            error!("Failed to initialize board: {e}");
            return;
        }
    };
    
    // Main scheduler loop
    info!("Starting mining scheduler");
    
    while !running.is_cancelled() {
        tokio::select! {
            // Handle board events
            Some(event) = event_rx.recv() => {
                match event {
                    BoardEvent::NonceFound(nonce_result) => {
                        info!("Nonce found! Job {} nonce {:#x}", nonce_result.job_id, nonce_result.nonce);
                        // TODO: Submit to pool
                    }
                    BoardEvent::JobComplete { job_id, reason } => {
                        info!("Job {} completed: {:?}", job_id, reason);
                        // TODO: Get new work from pool
                    }
                    BoardEvent::ChipError { chip_address, error } => {
                        error!("Chip {} error: {}", chip_address, error);
                    }
                    BoardEvent::ChipStatusUpdate { chip_address, temperature_c, frequency_mhz } => {
                        trace!("Chip {} status - temp: {:?}°C, freq: {:?}MHz", 
                               chip_address, temperature_c, frequency_mhz);
                    }
                }
            }
            
            // Periodic work fetching (temporary)
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
                trace!("Would fetch new work from pool");
                // TODO: Get work from pool
                // TODO: board.send_job(&job).await?;
            }
            
            // Shutdown
            _ = running.cancelled() => {
                info!("Scheduler shutdown requested");
                break;
            }
        }
    }
    
    trace!("Scheduler task stopped.");
}