# mujina-miner

Open source Bitcoin mining software written in Rust for ASIC mining hardware.

## Overview

mujina-miner is a modern, async Rust implementation of Bitcoin mining software
designed to communicate with various Bitcoin mining hash boards via USB serial
interfaces. It's part of the larger Mujina OS project, an open source,
Debian-based embedded Linux distribution optimized for Bitcoin mining hardware.

## Features

- **Modular Architecture**: Clean separation between transport, board control,
  chip communication, and mining logic
- **Async I/O**: Built on Tokio for efficient concurrent operations
- **Multi-Board Support**: Extensible design for different ASIC boards and chips
- **Hardware Abstraction**: Generic traits for GPIO, I2C, and ADC operations
- **Protocol Documentation**: Detailed documentation of mining protocols

## Documentation

### Project Documentation

- [Architecture Overview](docs/architecture.md) - System design and component
  interaction
- [Contributing Guide](CONTRIBUTING.md) - How to contribute to the project
- [Code Style Guide](CODE_STYLE.md) - Coding standards and conventions

### Protocol Documentation

- [BM13xx ASIC Protocol](mujina-miner/src/asic/bm13xx/PROTOCOL.md) - Serial
  protocol for BM13xx series mining chips
- [Bitaxe-Raw Control Protocol](mujina-miner/src/mgmt_protocol/bitaxe_raw/PROTOCOL.md) -
  Management protocol for Bitaxe board peripherals

## Supported Hardware

Currently supported:
- **Bitaxe Gamma** with BM1370 ASIC (via bitaxe-raw firmware)

Planned support:
- Additional Bitaxe variants
- Antminer hash boards
- Other ASIC mining hardware

## Building

```bash
# Build the workspace
cargo build

# Run tests
cargo test

# Run the miner daemon (requires connected hardware)
cargo run --bin mujina-minerd
```

## Running

Ensure your Bitaxe device is connected via USB and appears as `/dev/ttyACM0`
and `/dev/ttyACM1`.

```bash
# Run with default logging
cargo run --bin mujina-minerd

# Run with debug logging
RUST_LOG=mujina_miner=debug cargo run --bin mujina-minerd

# Run with trace logging (shows all serial communication)
RUST_LOG=mujina_miner=trace cargo run --bin mujina-minerd
```

## Architecture

The miner follows a layered architecture:

```
Mining Pool <--[Stratum]--> Scheduler <--> Board Manager <--> Boards
                                |                               |
                                v                               v
                          Job Generator                   ASIC Chips
```

Key components:
- **Transport Layer**: USB device discovery and serial communication
- **Board Manager**: Creates and manages board instances based on VID/PID
- **Board Abstraction**: Hardware-specific implementations (e.g., BitaxeBoard)
- **Chip Protocols**: ASIC communication protocols (e.g., BM13xx)
- **Management Protocols**: Board peripheral control (e.g., bitaxe-raw)
- **Scheduler**: Distributes mining jobs and collects results

## Development Status

This is an active development project. Current focus areas:
- [x] Basic board initialization and chip discovery
- [x] GPIO control for ASIC reset
- [ ] I2C peripheral control (fan, temperature, power monitoring)
- [ ] Stratum pool communication
- [ ] Production-ready error handling and recovery

## License

This project is licensed under the GNU General Public License v3.0 or later -
see the [LICENSE](LICENSE) file for details.

## Contributing

We welcome contributions! Please see our [Contributing Guide](CONTRIBUTING.md)
for details on how to get started.

## Related Projects

- [Mujina OS](https://github.com/mujina-os/mujina-os) - The parent project
- [bitaxe-raw](https://github.com/bitaxeorg/bitaxe-raw) - Firmware for Bitaxe
  boards