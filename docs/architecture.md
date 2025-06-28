# Mujina Miner Architecture

This document describes the high-level architecture of mujina-miner, an
async Bitcoin mining software built on Tokio.

## Overview

Mujina-miner is organized as a single Rust crate with well-defined modules
that separate concerns while maintaining simplicity. The architecture is
fully async using Tokio for concurrent I/O operations.

### Key Dependencies

- **tokio**: Async runtime for concurrent I/O operations
- **rust-bitcoin**: Core Bitcoin types and utilities
- **tokio-serial**: Async serial port communication
- **axum**: HTTP server framework (future)
- **tracing**: Structured logging and diagnostics

## Module Structure

```
src/
├── bin/              # Binary entry points
│   ├── minerd.rs     # mujina-minerd - Main daemon
│   ├── cli.rs        # mujina-cli - Command line interface
│   └── tui.rs        # mujina-tui - Terminal UI
├── lib.rs            # Library root
├── error.rs          # Common error types
├── types.rs          # Core types (Job, Share, Nonce, etc.)
├── config.rs         # Configuration loading and validation
├── daemon.rs         # Daemon lifecycle management
├── board/            # Mining board abstractions
├── chip/             # ASIC chip protocols
├── transport/        # USB/Serial communication layer
├── control/          # Hashboard control protocols
├── hal/              # Hardware abstraction layer
├── drivers/          # Peripheral device drivers
├── pool/             # Mining pool connectivity
├── scheduler.rs      # Work scheduling and distribution
├── job_generator.rs  # Local job generation (testing/solo)
├── api/              # HTTP API and WebSocket
├── api_client/       # Shared API client library
│   ├── mod.rs        # Client implementation
│   └── types.rs      # API DTOs and models
└── tracing.rs        # Logging and observability
```

## Module Descriptions

### Core Modules

#### `bin/minerd.rs`
The main daemon binary entry point. Handles:
- Signal handling (SIGINT/SIGTERM)
- Tokio runtime initialization
- Graceful shutdown coordination
- Top-level task spawning

#### `bin/cli.rs`
Command-line interface for controlling the miner:
- Uses the `api_client` module to communicate with the daemon
- Provides commands for status, configuration, pool management
- Suitable for scripting and automation

#### `bin/tui.rs`
Terminal user interface for interactive monitoring:
- Uses the `api_client` module to communicate with the daemon
- Built with ratatui for rich terminal graphics
- Real-time hashrate graphs and statistics
- Keyboard-driven interface for operators
- Connects via API WebSocket for live updates

#### `error.rs` (new)
Centralized error types using `thiserror`. Provides a unified `Error` enum
for the entire crate with conversions from underlying error types.

#### `types.rs` (new)
Core domain types shared across modules. This module re-exports commonly
used types from rust-bitcoin and defines mining-specific types. Using
rust-bitcoin provides battle-tested implementations of Bitcoin primitives
while avoiding reinventing fundamental types.

#### `config.rs` (new)
Configuration management:
- TOML file parsing with serde
- Config validation
- Hot-reload support via file watching
- Default values and config merging

#### `daemon.rs` (new)
Daemon lifecycle management:
- systemd notification support
- PID file handling
- Resource cleanup
- Health monitoring

### Hardware Communication Layer

The hardware communication layer is organized in four distinct levels, each
with a specific responsibility. This design enables maximum code reuse and
testability.

```
┌──────────────────────────────────────────────────────────────┐
│                     Board Implementation                     │
│   orchestrates all components for a specific board model     │
└──────────────────────────────────────────────────────────────┘
               │                               │                
               │Peripherals                    │ASIC Chain      
               │                               │                
┌─────────────────────────────┐ ┌──────────────────────────────┐
│           Drivers           │ │        ASIC Protocols        │
│     board support chips     │ │   ┌──────────────────────┐   │
│ ┌──────┐ ┌───────┐┌───────┐ │ │   │     BM13xx Family    │   │
│ │ TMP75│ │INA260 ││EMC2101│ │ │   │  ┌──────┐ ┌──────┐   │   │
│ └───┬──┘ └───┬───┘└───┬───┘ │ │   │  │BM1370│ │BM1362│   │   │
└─────────────────────────────┘ │   │  └───┬──┘ └───┬──┘   │   │
      │        │        │       │   └──────────────────────┘   │
      └────────┼────────┘       └──────────────────────────────┘
               │                           └────┬───┘           
        ┌─────────────┐                         │               
        │ HAL Traits  │                         │               
┌───────└─────────────┘───────┐                 │               
│        HAL Adapters         │                 │               
│ ┌──────────┐ ┌────────────┐ │                 │               
│ │I2cOverCtl│ │GpioOverCtrl│ │                 │               
│ └────┬─────┘ └───────┬────┘ │                 │               
└─────────────────────────────┘                 │               
       └───────┬───────┘                        │               
               │                                │               
    ┌─────────────────────┐           ┌──────────────────┐      
    │   Control Channel   │           │   Data Channel   │      
    │  control  protocol  │           │  direct  serial  │      
    └─────────────────────┘           └──────────────────┘      
              │                                 │               
              └────────────────┬────────────────┘               
                               │                                
┌──────────────────────────────────────────────────────────────┐
│                          Transport                           │
│                    USB/serial abstraction                    │
│    ┌─────────────────────┐       ┌──────────────────────┐    │
│    │   Control Channel   │       │     Data Channel     │    │
│    │    /dev/ttyACM0     │       │     /dev/ttyACM1     │    │
│    └─────────────────────┘       └──────────────────────┘    │
└──────────────────────────────────────────────────────────────┘
```

#### Key Insight: Two Separate Communication Paths

Mining boards typically have two distinct communication paths:

1. **Control Channel** (via control protocol): For board management
   - Temperature sensors, fan control, power monitoring
   - Reset lines, LED control
   - Uses protocols like bitaxe-raw over USB serial

2. **Data Channel** (direct to ASICs): For mining operations
   - Sending work to ASIC chips
   - Receiving nonces from ASIC chips
   - Uses chip-specific protocols (BM13xx, etc.)
   - Often a separate serial port connected directly to the ASIC chain

#### `transport/`
Raw communication with hardware devices. This layer handles:
- USB device discovery and enumeration
- Opening and configuring serial ports
- Managing dual-channel devices (control + data channels)
- No protocol knowledge - just raw byte streams

#### `control/`
Protocol implementations for hashboard control channels. This layer:
- Implements specific packet formats (e.g., bitaxe-raw's 7-byte header)
- Provides protocol operations: GPIO control, ADC readings, I2C passthrough
- Handles command/response sequencing and error checking
- Translates high-level operations into protocol packets

#### `hal/`
Hardware Abstraction Layer providing standard async traits. This layer:
- Defines traits like `I2c`, `Gpio`, `Adc` that drivers can use
- Provides adapters that implement these traits over control protocols
- Enables drivers to work with any underlying transport
- Allows the same driver to work with native Linux I2C or I2C-over-serial

#### `drivers/`
Reusable device drivers for mining peripherals. These drivers:
- Are generic over HAL traits (work with any `I2c` implementation)
- Provide high-level APIs for specific chips
- Handle device-specific registers and protocols
- Can be tested with mock HAL implementations

#### `chip/` (ASIC protocols)
ASIC chip communication protocols - the heart of mining operations:
- Implements protocols for different ASIC families (BM13xx, etc.)
- Handles work distribution and nonce collection
- Manages chip initialization, frequency control, and status
- Communicates directly via the data channel serial port
- Each chip family has its own protocol implementation

### Example: EmberOne Board Implementation

Here's how these layers work together in practice:

```rust
// board/ember_one.rs
use crate::transport::DualSerialTransport;
use crate::control::bitaxe_raw::BitaxeRawControl;
use crate::hal::adapters::I2cOverControl;
use crate::drivers::{TMP75, INA260};
use crate::chip::bm13xx::{BM1370, ChipChain};

pub struct EmberOneBoard {
    transport: DualSerialTransport,
    control: BitaxeRawControl,
    asic_chain: ChipChain<BM1370>,
    temp_sensor: TMP75<I2cOverControl>,
    power_monitor: INA260<I2cOverControl>,
}

impl EmberOneBoard {
    pub async fn new(control_port: &str, data_port: &str) -> Result<Self> {
        // 1. Create transport layer (dual serial ports)
        let transport = DualSerialTransport::open(control_port, data_port)
            .await
            .context("Failed to open serial ports")?;
        
        // 2. Create control protocol handler for board management
        let mut control = BitaxeRawControl::new(transport.control_channel());
        
        // 3. Initialize the board via control channel
        control.set_gpio(ASIC_RESET_PIN, false).await?; // Reset ASICs
        tokio::time::sleep(Duration::from_millis(100)).await;
        control.set_gpio(ASIC_RESET_PIN, true).await?;  // Release reset
        
        // 4. Create ASIC chain on the data channel
        let asic_chain = ChipChain::<BM1370>::new(
            transport.data_channel(),
            1  // Single chip on EmberOne
        );
        
        // 5. Initialize ASICs
        asic_chain.enumerate_chips().await?;
        asic_chain.set_frequency(500.0).await?; // 500 MHz
        
        // 6. Create HAL adapter for board peripherals
        let i2c = I2cOverControl::new(&mut control);
        
        // 7. Create drivers for board support chips
        let temp_sensor = TMP75::new(i2c.clone(), 0x48);
        let power_monitor = INA260::new(i2c, 0x40);
        
        Ok(Self {
            transport,
            control,
            asic_chain,
            temp_sensor,
            power_monitor,
        })
    }
    
    pub async fn send_work(&mut self, job: MiningJob) -> Result<()> {
        // Send mining work directly to ASICs via data channel
        self.asic_chain.send_job(0, job).await
    }
    
    pub async fn check_for_nonces(&mut self) -> Result<Vec<Nonce>> {
        // Poll ASICs for any found nonces
        self.asic_chain.read_nonces().await
    }
    
    pub async fn read_diagnostics(&mut self) -> Result<Diagnostics> {
        // Read from board peripherals via control channel
        let temp = self.temp_sensor.read_temperature().await?;
        let power = self.power_monitor.read_power().await?;
        let hashrate = self.asic_chain.estimate_hashrate();
        
        Ok(Diagnostics { temp, power, hashrate })
    }
}
```

This architecture provides several key benefits:

1. **Clear separation**: ASICs communicate via data channel, peripherals via control
2. **Reusability**: Drivers work with any HAL implementation, ASIC protocols work with any serial port
3. **Testability**: Each layer can be tested in isolation with mocks
4. **Flexibility**: New boards can mix and match components:
   - Different ASIC chips (BM1370, BM1397, etc.)
   - Different control protocols (bitaxe-raw, custom protocols)
   - Different peripheral chips (various temp sensors, power monitors)
5. **Maintainability**: Clear boundaries between transport, protocols, and business logic

### Mining Logic

#### `board/`
**Existing module - expanded scope**

Mining board abstractions that compose all hardware elements:
- `Board` trait defining the interface for all mining boards
- `bitaxe.rs` - Original Bitaxe board implementation
- `ember_one.rs` - EmberOne board using layered architecture
- `generic_usb.rs` - Auto-detecting USB boards
- Manages: chip chains, cooling, power delivery, monitoring

#### `chip/`
**Existing module - unchanged location**

ASIC chip protocols and implementations:
- Current: `bm13xx/` family with protocol documentation
- Future: Other ASIC families
- Handles: work distribution, nonce collection, frequency control

#### `pool/`
Mining pool client implementations:
- `traits.rs` - `PoolClient` trait
- `stratum_v1.rs` - Stratum v1 protocol (most common)
- `stratum_v2.rs` - Stratum v2 protocol (future)
- `manager.rs` - Pool failover and switching logic
- Handles: work fetching, share submission, difficulty adjustments

#### `scheduler.rs`
**Existing module - enhanced**

Orchestrates the mining operation:
- Receives work from pools
- Distributes work to boards/chips
- Collects and routes shares
- Implements work scheduling strategies
- Manages board lifecycle

#### `job_generator.rs`
**Existing module - unchanged**

Local job generation for testing and solo mining:
- Generates valid block templates
- Updates timestamp/nonce fields
- Useful for hardware testing without pools

### API and Observability

#### `api/`
HTTP API server (new):
- Built on Axum (async web framework)
- RESTful endpoints for status, control, configuration
- WebSocket support for real-time updates
- OpenTelemetry integration
- Prometheus metrics endpoint

#### `tracing.rs`
**Existing module - unchanged**

Structured logging and observability:
- tracing subscriber setup
- journald or stdout output
- Log level configuration
- Performance tracing spans

## Data Flow

```
Mining Pool <--[Stratum]--> pool::PoolClient
                                   |
                                   v
                            scheduler::Scheduler
                                   |
                    +--------------+--------------+
                    |                             |
                    v                             v
             board::Board                  board::Board
                    |                             |
                    v                             v
          chip::BM13xxChip              chip::BM13xxChip
                    |                             |
                    v                             v
    transport::SerialPort          transport::SerialPort
```

## Async Patterns

All I/O operations are async using Tokio:
- Serial communication uses `tokio-serial`
- HTTP server uses `axum` (built on Tokio)
- Background tasks use `tokio::spawn`
- Graceful shutdown via `CancellationToken`
- Concurrent operations via `TaskTracker`

## Extension Points

The architecture supports extension through several mechanisms:

1. **New Board Types**: Implement the `Board` trait
2. **New Chip Families**: Add modules under `chip/`
3. **New Pool Protocols**: Implement `PoolClient` trait
4. **New Control Protocols**: Implement `ControlProtocol` trait
5. **Custom Schedulers**: Pluggable scheduling strategies
6. **Additional Drivers**: Add I2C/SPI device drivers

## Configuration

Configuration is managed through TOML files with hot-reload support:
- `/etc/mujina/mujina.toml` - System configuration
- Board-specific settings
- Pool credentials and priorities
- Temperature limits and safety settings
- API server configuration

## Security Considerations

- No hardcoded credentials
- TLS support for API endpoints
- Privilege dropping after startup
- Isolated board control (no direct chip access from API)
- Rate limiting on API endpoints

## User Interfaces

Mujina-miner provides multiple interfaces for different use cases:

### Web Application (Separate Repository)
The primary user interface is a modern web application that lives in a
separate repository (`mujina-web`):
- Built with modern web technologies (React/Vue/Svelte)
- Communicates exclusively through the HTTP API
- Provides rich visualizations and easy configuration
- Suitable for remote management
- Can be served by any web server or CDN

**Repository**: `github.com/mujina/mujina-web` (example)

### Command Line Interface (CLI)
Included in this repository as `mujina-cli`:
- Direct API client for automation and scripting
- Supports all daemon operations
- JSON output mode for parsing
- Configuration file management

### Terminal User Interface (TUI)
Included in this repository as `mujina-tui`:
- Interactive terminal dashboard
- Real-time monitoring without web browser
- Ideal for SSH sessions
- Keyboard shortcuts for common operations

### API Client Library
The `api_client` module provides:
- Rust types for all API requests/responses
- Async HTTP client using reqwest
- WebSocket support for real-time data
- Shared between CLI and TUI
- Could be published as separate crate for third-party tools

## Repository Structure

This repository contains the core miner daemon and terminal-based tools:
```
mujina-miner/
├── Cargo.toml
├── README.md
├── docs/
│   ├── architecture.md    # This file
│   ├── api.md            # API documentation
│   └── deployment.md     # Installation guide
├── configs/
│   └── example.toml      # Example configuration
├── src/                  # Rust source code
├── systemd/
│   └── mujina-minerd.service
└── debian/               # Debian packaging
```

The web interface lives in a separate repository to allow:
- Independent development cycles
- Different programming languages
- Separate CI/CD pipelines
- Alternative web UIs from the community
