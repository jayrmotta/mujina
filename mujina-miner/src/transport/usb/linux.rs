//! Linux udev-based USB discovery implementation.
//!
//! This module discovers USB devices and monitors for hotplug events using
//! libudev on Linux systems.
//!
//! ## Architecture
//!
//! USB monitoring runs in a dedicated OS thread with its own single-threaded
//! Tokio runtime. This design addresses the fact that udev types are !Send
//! (they contain raw C pointers that libudev requires stay on one thread).
//! By running in a dedicated thread with a single-threaded runtime, we get:
//!
//! - Clean async/await code using tokio-udev
//! - tokio::select! for elegant shutdown handling
//! - No unsafe code or manual polling
//! - Proper blocking behavior (waits for events efficiently)
//!
//! ## Device Discovery
//!
//! The implementation uses udev to:
//! - Enumerate existing USB devices at startup
//! - Extract VID/PID/serial number from device attributes
//! - Find associated serial port (tty) devices
//! - Monitor for add/remove events via async udev socket
//!
//! ## Serial Port Ordering
//!
//! When a USB device has multiple serial ports (e.g., /dev/ttyACM0, /dev/ttyACM1),
//! they are sorted by device node name for consistent ordering across
//! reconnections. This is critical for boards that expect a specific port for
//! control vs data communication.

use super::{TransportEvent, UsbDeviceInfo};
use crate::error::Result;
use crate::tracing::prelude::*;
use futures::stream::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Linux udev-based USB discovery implementation.
pub struct LinuxUdevDiscovery {
    // Future: Add state fields if needed for monitoring
}

impl LinuxUdevDiscovery {
    /// Create a new Linux USB discovery instance.
    pub fn new() -> Result<Self> {
        // Verification happens at first use - if libudev isn't available,
        // the udev::Enumerator or MonitorBuilder calls will fail
        Ok(Self {})
    }

    /// Extract VID, PID, and serial number from a udev device.
    ///
    /// These are typically found in device attributes as hexadecimal strings.
    /// VID and PID are required; serial number is optional.
    fn extract_device_properties(
        &self,
        device: &udev::Device,
    ) -> Result<(u16, u16, Option<String>)> {
        // Extract VID (vendor ID) - typically 4 hex digits like "0403"
        let vid_str = device
            .attribute_value("idVendor")
            .and_then(|v| v.to_str())
            .ok_or_else(|| crate::error::Error::Other("Missing idVendor attribute".to_string()))?;

        let vid = u16::from_str_radix(vid_str, 16)
            .map_err(|e| crate::error::Error::Other(format!("Invalid VID '{}': {}", vid_str, e)))?;

        // Extract PID (product ID) - typically 4 hex digits like "6015"
        let pid_str = device
            .attribute_value("idProduct")
            .and_then(|v| v.to_str())
            .ok_or_else(|| crate::error::Error::Other("Missing idProduct attribute".to_string()))?;

        let pid = u16::from_str_radix(pid_str, 16)
            .map_err(|e| crate::error::Error::Other(format!("Invalid PID '{}': {}", pid_str, e)))?;

        // Extract serial number (optional)
        let serial = device
            .attribute_value("serial")
            .and_then(|v| v.to_str())
            .map(|s| s.to_string());

        trace!(
            "Extracted device properties: VID={:04x}, PID={:04x}, serial={:?}",
            vid,
            pid,
            serial
        );

        Ok((vid, pid, serial))
    }

    /// Find serial port devices (tty) associated with this USB device.
    ///
    /// Returns an ordered list of device paths (e.g., ["/dev/ttyACM0", "/dev/ttyACM1"]).
    /// Ports are sorted by device node name for consistent ordering.
    fn find_serial_ports(&self, device: &udev::Device) -> Result<Vec<String>> {
        let mut ports = Vec::new();

        // Get the device syspath for matching children
        let device_syspath = device.syspath();

        // Create an enumerator to find tty devices
        let mut enumerator = udev::Enumerator::new().map_err(|e| {
            crate::error::Error::Other(format!("Failed to create enumerator: {}", e))
        })?;

        // Look for tty subsystem devices
        enumerator.match_subsystem("tty").map_err(|e| {
            crate::error::Error::Other(format!("Failed to filter by subsystem: {}", e))
        })?;

        // Scan all tty devices and check if they're children of our USB device
        for tty_device in enumerator
            .scan_devices()
            .map_err(|e| crate::error::Error::Other(format!("Failed to scan devices: {}", e)))?
        {
            // Check if this tty device is a descendant of our USB device
            // by walking up the parent chain
            let mut current = Some(tty_device.clone());
            let mut is_child = false;

            while let Some(dev) = current {
                if dev.syspath() == device_syspath {
                    is_child = true;
                    break;
                }
                current = dev.parent();
            }

            if is_child {
                // Get the device node (e.g., /dev/ttyACM0)
                if let Some(devnode) = tty_device.devnode() {
                    if let Some(path_str) = devnode.to_str() {
                        trace!("Found serial port: {}", path_str);
                        ports.push(path_str.to_string());
                    }
                }
            }
        }

        // Sort ports by name for consistent ordering
        // This ensures /dev/ttyACM0 comes before /dev/ttyACM1
        ports.sort();

        debug!(
            "Found {} serial port(s) for device: {:?}",
            ports.len(),
            ports
        );

        Ok(ports)
    }

    /// Build a UsbDeviceInfo from a udev device.
    ///
    /// Combines device properties and associated serial ports into a single
    /// structure that can be sent to the backplane.
    fn build_device_info(&self, device: &udev::Device) -> Result<UsbDeviceInfo> {
        // Extract basic device properties
        let (vid, pid, serial_number) = self.extract_device_properties(device)?;

        // Get device path
        let device_path = device
            .syspath()
            .to_str()
            .ok_or_else(|| crate::error::Error::Other("Invalid device path".to_string()))?
            .to_string();

        // Find associated serial ports
        let serial_ports = self.find_serial_ports(device)?;

        Ok(UsbDeviceInfo {
            vid,
            pid,
            serial_number,
            device_path,
            serial_ports,
        })
    }

    /// Enumerate currently connected USB devices.
    fn enumerate_devices(&self) -> Result<Vec<UsbDeviceInfo>> {
        info!("Enumerating USB devices");

        // Create enumerator for USB devices
        let mut enumerator = udev::Enumerator::new().map_err(|e| {
            crate::error::Error::Other(format!("Failed to create enumerator: {}", e))
        })?;

        // Filter for USB subsystem
        enumerator
            .match_subsystem("usb")
            .map_err(|e| crate::error::Error::Other(format!("Failed to match subsystem: {}", e)))?;

        // Scan devices and build info for each
        // We filter to actual devices (not interfaces) by checking for idVendor/idProduct
        let mut devices = Vec::new();
        for device in enumerator
            .scan_devices()
            .map_err(|e| crate::error::Error::Other(format!("Failed to scan devices: {}", e)))?
        {
            // Skip if this doesn't have idVendor (means it's an interface, not a device)
            if device.attribute_value("idVendor").is_none() {
                continue;
            }

            // Try to build device info, skip devices that fail
            match self.build_device_info(&device) {
                Ok(info) => {
                    info!(
                        "Enumerated USB device: {:04x}:{:04x} at {} ({} serial ports)",
                        info.vid,
                        info.pid,
                        info.device_path,
                        info.serial_ports.len()
                    );
                    devices.push(info);
                }
                Err(e) => {
                    // Log but continue - some USB devices may not have complete info
                    debug!("Skipping device due to error: {}", e);
                }
            }
        }

        info!(
            "Enumeration complete: found {} USB device(s)",
            devices.len()
        );
        Ok(devices)
    }
}

impl super::UsbDiscoveryImpl for LinuxUdevDiscovery {
    fn monitor_blocking(
        self: Box<Self>,
        event_tx: mpsc::Sender<super::super::TransportEvent>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        info!("Starting USB monitoring in dedicated thread");

        // Create a single-threaded Tokio runtime for this thread.
        //
        // Why do this? The udev types are !Send (contain raw C pointers), so they
        // can't be used with Tokio's multi-threaded runtime. However, we CAN use
        // async/await within a single thread. By creating a current_thread runtime
        // here, we get:
        // - Clean async/await code with tokio-udev
        // - tokio::select! for monitoring both events and shutdown
        // - No unsafe code or manual polling
        // - All udev types stay on this thread (satisfying !Send requirement)
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| crate::error::Error::Other(format!("Failed to create runtime: {}", e)))?;

        // Run the async monitoring loop on this thread's runtime
        runtime.block_on(async {
            // Initial enumeration - send Connected events for existing devices
            debug!("Performing initial USB device enumeration");
            for device_info in self.enumerate_devices()? {
                let usb_event = TransportEvent::UsbDeviceConnected(device_info);
                let transport_event = super::super::TransportEvent::Usb(usb_event);

                if event_tx.send(transport_event).await.is_err() {
                    info!("Event receiver dropped during enumeration");
                    return Ok(());
                }
            }

            debug!("Initial enumeration complete, creating async monitor");

            // Create async udev monitor using tokio-udev
            let builder = tokio_udev::MonitorBuilder::new()
                .map_err(|e| crate::error::Error::Other(format!("Failed to create monitor: {}", e)))?
                .match_subsystem("usb")
                .map_err(|e| crate::error::Error::Other(format!("Failed to filter monitor: {}", e)))?;

            let socket = builder
                .listen()
                .map_err(|e| crate::error::Error::Other(format!("Failed to listen: {}", e)))?;

            let mut monitor = tokio_udev::AsyncMonitorSocket::new(socket)
                .map_err(|e| crate::error::Error::Other(format!("Failed to create async socket: {}", e)))?;

            info!("USB monitor created, entering event loop");

            // Event loop using tokio::select! to wait on both events and shutdown
            // This is clean, safe async code with no manual polling required
            loop {
                tokio::select! {
                    // Wait for USB hotplug event
                    event_result = monitor.next() => {
                        let event = match event_result {
                            Some(Ok(e)) => e,
                            Some(Err(e)) => {
                                error!("Error from USB monitor: {}", e);
                                continue;
                            }
                            None => {
                                warn!("USB monitor stream ended");
                                return Ok(());
                            }
                        };

                        let device = event.device();

                        // Build transport event based on event type
                        let transport_event = match event.event_type() {
                            tokio_udev::EventType::Add => {
                                // Skip if device doesn't have VID (it's an interface, not a device)
                                if device.attribute_value("idVendor").is_none() {
                                    continue;
                                }

                                debug!("USB device added: {:?}", device.syspath());

                                match self.build_device_info(&device) {
                                    Ok(device_info) => {
                                        info!(
                                            "USB device connected: {:04x}:{:04x} at {}",
                                            device_info.vid, device_info.pid, device_info.device_path
                                        );
                                        Some(TransportEvent::UsbDeviceConnected(device_info))
                                    }
                                    Err(e) => {
                                        debug!("Failed to build device info for added device: {}", e);
                                        None
                                    }
                                }
                            }

                            tokio_udev::EventType::Remove => {
                                if let Some(syspath) = device.syspath().to_str() {
                                    debug!("USB device removed: {}", syspath);
                                    Some(TransportEvent::UsbDeviceDisconnected {
                                        device_path: syspath.to_string(),
                                    })
                                } else {
                                    None
                                }
                            }

                            _ => {
                                // Ignore other event types (change, bind, unbind, etc.)
                                trace!("Ignoring event type: {:?}", event.event_type());
                                None
                            }
                        };

                        // Send the event if we built one
                        if let Some(usb_event) = transport_event {
                            let transport_event = super::super::TransportEvent::Usb(usb_event);
                            if event_tx.send(transport_event).await.is_err() {
                                info!("Event receiver dropped, exiting USB monitor");
                                return Ok(());
                            }
                        }
                    }

                    // Wait for shutdown signal
                    _ = shutdown.cancelled() => {
                        info!("Shutdown requested, exiting USB monitor");
                        return Ok(());
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_discovery() {
        let discovery = LinuxUdevDiscovery::new();
        assert!(discovery.is_ok());
    }

    #[test]
    #[ignore] // Run with: cargo test --package mujina-miner -- --ignored --nocapture
    fn test_udev_monitor_minimal() {
        // Minimal test to understand why socket.iter() returns None immediately
        println!("Creating udev monitor...");
        let socket = udev::MonitorBuilder::new_kernel()
            .unwrap()
            .match_subsystem("usb")
            .unwrap()
            .listen()
            .unwrap();

        println!("Monitor created, calling iter()...");
        let mut iter = socket.iter();

        println!("Calling iter.next() - this should block...");
        match iter.next() {
            Some(event) => println!("Got event: {:?}", event.event_type()),
            None => println!("iter.next() returned None immediately!"),
        }
    }
}
