//! This module implements an interface to the BM13xx chips.

use bitvec::prelude::*;
use bytes::{Buf, BufMut, BytesMut};
use std::io;
use strum::FromRepr;
use tokio_util::codec::{Decoder, Encoder};

use crate::tracing::prelude::*;
use crc::*;

mod crc;

#[derive(FromRepr, Copy, Clone)]
#[repr(u8)]
pub enum RegisterAddress {
    ChipAddress = 0x00,
    // MiscControl = 0x18,
    // FastUartConfiguration = 0x28,
    // Pll1Parameter = 0x60,
    // VersionRolling = 0xa4,
    RegA8 = 0xa8,
}

pub enum Register {
    ChipAddress {
        chip_id: u16,
        core_count: u8,
        address: u8,
    },
    RegA8 {
        unknown: u32,
    },
}

impl Register {
    fn decode(address: RegisterAddress, bytes: &[u8; 4]) -> Register {
        match address {
            RegisterAddress::ChipAddress => Register::ChipAddress {
                chip_id: u16::from_be_bytes(bytes[0..2].try_into().unwrap()),
                core_count: u8::from_be(bytes[2]),
                address: u8::from_be(bytes[3]),
            },
            RegisterAddress::RegA8 => Register::RegA8 { 
                unknown: u32::from_be_bytes(*bytes)
            },
        }
    }
}

#[repr(u8)]
enum CommandFlagsType {
    // Job = 1,
    Command = 2,
}

#[repr(u8)]
enum CommandFlagsCmd {
    // SetAddress = 0,
    WriteRegisterOrJob = 1,
    ReadRegister = 2,
    // ChainInactive = 3,
}

pub enum Command {
    ReadRegister {
        all: bool,
        chip_address: u8,
        register_address: RegisterAddress,
    },
    WriteRegister {
        all: bool,
        chip_address: u8,
        register: Register,
    },
}


impl Command {
    fn build_flags(typ: CommandFlagsType, all: bool, cmd: CommandFlagsCmd) -> u8 {
        let mut flags = 0u8;
        let field = flags.view_bits_mut::<Lsb0>();
        field[5..7].store(typ as u8);
        field[4..5].store(all as u8);
        field[0..4].store(cmd as u8);
        flags
    }

    fn encode(&self, dst: &mut BytesMut) {
        match self {
            Command::ReadRegister { all, chip_address, register_address } => {
                dst.put_u8(Self::build_flags(
                    CommandFlagsType::Command,
                    *all,
                    CommandFlagsCmd::ReadRegister,
                ));

                const FLAGS_LEN: u8 = 1;
                const CHIP_ADDR_LEN: u8 = 1;
                const REG_ADDR_LEN: u8 = 1;
                const LENGTH_FIELD_LEN: u8 = 1;
                const CRC_LEN: u8 = 1;
                const TOTAL_LEN: u8 = FLAGS_LEN + LENGTH_FIELD_LEN + CHIP_ADDR_LEN + REG_ADDR_LEN + CRC_LEN;
                
                dst.put_u8(TOTAL_LEN);
                dst.put_u8(*chip_address);
                dst.put_u8(*register_address as u8);
            }
            Command::WriteRegister { all, chip_address, register } => {
                dst.put_u8(Self::build_flags(
                    CommandFlagsType::Command,
                    *all,
                    CommandFlagsCmd::WriteRegisterOrJob,
                ));

                const FLAGS_LEN: u8 = 1;
                const CHIP_ADDR_LEN: u8 = 1;
                const REG_ADDR_LEN: u8 = 1;
                const REG_DATA_LEN: u8 = 4;
                const LENGTH_FIELD_LEN: u8 = 1;
                const CRC_LEN: u8 = 1;
                const TOTAL_LEN: u8 = FLAGS_LEN + LENGTH_FIELD_LEN + CHIP_ADDR_LEN + REG_ADDR_LEN + REG_DATA_LEN + CRC_LEN;

                dst.put_u8(TOTAL_LEN);
                dst.put_u8(*chip_address);
                
                match register {
                    Register::ChipAddress { chip_id, core_count, address } => {
                        dst.put_u8(RegisterAddress::ChipAddress as u8);
                        dst.put_u16(*chip_id);
                        dst.put_u8(*core_count);
                        dst.put_u8(*address);
                    }
                    Register::RegA8 { unknown } => {
                        dst.put_u8(RegisterAddress::RegA8 as u8);
                        dst.put_u32(*unknown);
                    }
                }
            }
        }
    }
}

#[derive(FromRepr)]
#[repr(u8)]
enum ResponseType {
    ReadRegister = 0,
    Nonce = 4,
}

pub enum Response {
    ReadRegister {
        chip_address: u8,
        register: Register,
    },
    Nonce,
}

impl Response {
    fn decode(bytes: &mut BytesMut, is_version_rolling: bool) -> Result<Response, String> {
        let type_and_crc = bytes[bytes.len() - 1].view_bits::<Lsb0>();
        let type_repr = type_and_crc[5..].load::<u8>();

        match ResponseType::from_repr(type_repr) {
            Some(ResponseType::ReadRegister) => {
                let value: &[u8; 4] = &bytes.split_to(4)[..].try_into().unwrap();
                let chip_address = bytes.get_u8();
                let register_address_repr = bytes.get_u8();

                if let Some(register_address) = RegisterAddress::from_repr(register_address_repr) {
                    let register = Register::decode(register_address, value);
                    Ok(Response::ReadRegister {
                        chip_address,
                        register,
                    })
                } else {
                    Err(format!(
                        "unknown register address 0x{:x}.",
                        register_address_repr
                    ))
                }
            }
            Some(ResponseType::Nonce) => {
                panic!("not implemented")
            }
            None => Err(format!("unknown response type 0x{:x}.", type_repr)),
        }
    }
}

#[derive(Default)]
pub struct FrameCodec {
    // Controls whether to use the alternative frame format required when version rolling
    // is enabled. When true, uses version rolling compatible format. (default: false)
    version_rolling: bool,
}

impl Encoder<Command> for FrameCodec {
    type Error = io::Error;

    fn encode(&mut self, command: Command, dst: &mut BytesMut) -> Result<(), Self::Error> {
        const COMMAND_PREAMBLE: &[u8] = &[0x55, 0xaa];
        dst.put_slice(COMMAND_PREAMBLE);

        command.encode(dst);

        let crc = crc5(&dst[2..]);
        dst.put_u8(crc);

        Ok(())
    }
}

impl Decoder for FrameCodec {
    type Item = Response;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Return Ok(Item) with a valid frame, or Ok(None) if to be called again, potentially with
        // more data. Returning an Error causes the stream to be terminated, so don't do that.
        //
        // There are three cases:
        //
        // 1. More data needed
        // 2. Invalid frame
        // 3. Valid frame
        //
        // In the case of an invalid frame, consume the first byte and request another call by
        // returning Ok(None). In the case of a valid frame, consume that frame's worth of bytes.

        const PREAMBLE: &[u8] = &[0xaa, 0x55];
        const NONROLLING_FRAME_LEN: usize = PREAMBLE.len() + 7;
        const ROLLING_FRAME_LEN: usize = PREAMBLE.len() + 9;
        const CALL_AGAIN: Result<Option<Response>, io::Error> = Ok(None);

        let frame_len = if self.version_rolling {
            ROLLING_FRAME_LEN
        } else {
            NONROLLING_FRAME_LEN
        };

        if src.len() < frame_len {
            return CALL_AGAIN;
        }

        let mut prospect = src.clone(); // avoid consuming real buffer as we provisionally parse

        if prospect.get_u8() != PREAMBLE[0] {
            src.advance(1);
            return CALL_AGAIN;
        }

        if prospect.get_u8() != PREAMBLE[1] {
            src.advance(1);
            return CALL_AGAIN;
        }

        if !crc5_is_valid(&prospect[..]) {
            src.advance(1);
            return CALL_AGAIN;
        } else {
            src.advance(frame_len);
        }

        match Response::decode(&mut prospect, self.version_rolling) {
            Ok(response) => Ok(Some(response)),
            Err(msg) => {
                warn!(msg);
                CALL_AGAIN
            }
        }
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn read_register() {
        assert_frame_eq(
            Command::ReadRegister {
                all: true,
                chip_address: 0,
                register_address: RegisterAddress::ChipAddress,
            },
            &[0x55, 0xaa, 0x52, 0x05, 0x00, 0x00, 0x0a],
        );
    }

    #[test]
    fn write_register_chip_address() {
        assert_frame_eq(
            Command::WriteRegister {
                all: false,
                chip_address: 0x01,
                register: Register::ChipAddress {
                    chip_id: 0x1370,
                    core_count: 0x00,
                    address: 0x01,
                },
            },
            &[0x55, 0xaa, 0x41, 0x09, 0x01, 0x00, 0x13, 0x70, 0x00, 0x01, 0x0a],
        );
    }

    fn assert_frame_eq(cmd: Command, expect: &[u8]) {
        let mut codec = FrameCodec::default();
        let mut frame = BytesMut::new();
        codec.encode(cmd, &mut frame).unwrap();
        if frame != expect {
            panic!(
                "mismatch!\nexpected: {}\nactual: {}",
                as_hex(expect),
                as_hex(&frame[..])
            )
        }
    }

    fn as_hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<String>>()
            .join(" ")
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;

    #[test]
    fn read_register() {
        let wire = &[0xaa, 0x55, 0x13, 0x70, 0x00, 0x00, 0x00, 0x00, 0x06];
        let response = decode_frame(wire).unwrap();

        let Response::ReadRegister {
            chip_address,
            register,
        } = response
        else {
            panic!();
        };

        assert_eq!(chip_address, 0x00);

        let Register::ChipAddress {
            chip_id,
            core_count,
            address,
        } = register
        else {
            panic!();
        };

        assert_eq!(chip_id, 0x1370);
        assert_eq!(core_count, 0x00);
        assert_eq!(address, 0x00);
    }

    fn decode_frame(frame: &[u8]) -> Option<Response> {
        let mut buf = BytesMut::from(frame);
        let mut codec = FrameCodec::default();
        codec.decode(&mut buf).unwrap()
    }
}

// Bytes go out on the wire least-significant byte first.
// Multi-byte fields are sent most-significant byte first, i.e., big-endian.
