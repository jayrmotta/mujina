use bitvec::prelude::*;
use crc_all::Crc;
use std::io;
use bytes::{BytesMut, BufMut};
use tokio_util::codec::Encoder;

#[repr(u8)]
pub enum Register {
    ChipAddress = 0,
}

pub enum Command {
    ReadRegister { all: bool, address: u8, register: Register },
}

struct CommandFieldBuilder {
    field: u8,
}

#[repr(u8)]
enum CommandFieldType {
    Job = 1,
    Command = 2,
}

#[repr(u8)]
enum CommandFieldCmd {
    SetAddress = 0,
    WriteRegisterOrJob = 1,
    ReadRegister = 2,
    ChainInactive = 3,
}

impl CommandFieldBuilder {
    fn new() -> Self {
        Self { field: 0 }
    }

    fn with_type(mut self, command_type: CommandFieldType) -> Self {
        let view = self.field.view_bits_mut::<Lsb0>();
        view[5..7].store(command_type as u8);
        self
    }

    fn with_type_for_command(self, command: &Command) -> Self {
        self.with_type(
            match command {
                Command::ReadRegister {..} => CommandFieldType::Command,
            }
        )
    }

    fn with_all(mut self, all: &bool) -> Self {
        let view = self.field.view_bits_mut::<Lsb0>();
        view[4..5].store(*all as u8);
        self
    }

    fn with_all_for_command(self, command: &Command) -> Self {
        self.with_all(
            match command {
                Command::ReadRegister {all, ..} => &all,
            }
        )
    }

    fn with_cmd(mut self, cmd: CommandFieldCmd) -> Self {
        let view = self.field.view_bits_mut::<Lsb0>();
        view[0..4].store(cmd as u8);
        self
    }

    fn with_cmd_for_command(self, command: &Command) -> Self {
        self.with_cmd(
            match command {
                Command::ReadRegister {..} => CommandFieldCmd::ReadRegister,
            }
        )
    }

    fn for_command(self, command: &Command) -> Self {
        self.with_type_for_command(command)
            .with_all_for_command(command)
            .with_cmd_for_command(command)
    }

    fn build(self) -> u8 {
        return self.field;
    }
}

fn crc5_usb(bytes: &[u8]) -> u8 {
    const POLYNOMIAL: u8 = 0x05;
    const WIDTH: usize = 5;
    const INITIAL: u8 = 0x1f;
    const XOR: u8 = 0;
    const REFLECT: bool = false;
    let mut crc5_usb = Crc::<u8>::new(POLYNOMIAL, WIDTH, INITIAL, XOR, REFLECT);

    crc5_usb.update(bytes);
    crc5_usb.finish()
}

pub struct FrameCodec;

impl Encoder<Command> for FrameCodec {
    type Error = io::Error;

    fn encode(&mut self, command: Command, dst: &mut BytesMut) -> Result<(), Self::Error> {
        const COMMAND_PREAMBLE: &[u8] = &[0x55, 0xaa];
        dst.put_slice(COMMAND_PREAMBLE);

        let command_field = CommandFieldBuilder::new().for_command(&command).build();
        dst.put_u8(command_field);


        match command {
            Command::ReadRegister { all: _, address, register } => {
                const LENGTH: u8 = 5;
                dst.put_u8(LENGTH);
                dst.put_u8(address);
                dst.put_u8(register as u8);
            }
        }

        let crc = crc5_usb(&dst[2..]);
        dst.put_u8(crc);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<String>>()
            .join(" ")
    }

    fn assert_frame(cmd: Command, expect: &[u8]) {
        let mut codec = FrameCodec;
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

    #[test]
    fn test_read_all() {
        assert_frame(
            Command::ReadRegister {
                all: true,
                address: 0,
                register: Register::ChipAddress,
            },
            &[0x55, 0xaa, 0x52, 0x05, 0x00, 0x00, 0x0a],
        );
    }
}
