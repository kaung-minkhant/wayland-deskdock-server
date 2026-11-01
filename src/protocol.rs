//! DeskDock's length-prefixed TCP framing and the handshake packets that are
//! established by static analysis of the original Java server and Android peer.

use std::error::Error;
use std::fmt;
use std::io::{self, Read, Write};

pub const MAX_FRAME_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    pub width: i32,
    pub height: i32,
    /// Preserved verbatim until dynamic captures establish a semantic name.
    pub value3: i32,
    /// Preserved verbatim until dynamic captures establish a semantic name.
    pub value4: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub package_name: String,
    pub device_id: String,
    pub configuration: Configuration,
}

pub fn read_frame(stream: &mut impl Read) -> Result<Vec<u8>, ProtocolError> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;

    let length = i32::from_be_bytes(length);
    if length < 0 || length as usize > MAX_FRAME_BYTES {
        return Err(ProtocolError::InvalidFrameLength(length));
    }

    let mut payload = vec![0; length as usize];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

pub fn write_frame(stream: &mut impl Write, payload: &[u8]) -> Result<(), ProtocolError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::InvalidFrameLength(payload.len() as i32));
    }

    let length =
        i32::try_from(payload.len()).map_err(|_| ProtocolError::InvalidFrameLength(i32::MAX))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

pub fn decode_configuration(payload: &[u8]) -> Result<Configuration, ProtocolError> {
    if payload.len() != 20 {
        return Err(ProtocolError::InvalidConfigurationLength(payload.len()));
    }

    let values = payload
        .chunks_exact(4)
        .map(|bytes| i32::from_be_bytes(bytes.try_into().expect("exact four-byte chunk")))
        .collect::<Vec<_>>();
    if values[0] != 3 {
        return Err(ProtocolError::UnexpectedPacketFamily(values[0]));
    }

    Ok(Configuration {
        width: values[1],
        height: values[2],
        value3: values[3],
        value4: values[4],
    })
}

pub fn expect_hello(stream: &mut impl Read) -> Result<(), ProtocolError> {
    let hello = read_frame(stream)?;
    if hello == b"HELLO" {
        Ok(())
    } else {
        Err(ProtocolError::UnexpectedHello(hello))
    }
}

pub fn perform_handshake_after_hello(
    stream: &mut (impl Read + Write),
    protocol_version: u32,
) -> Result<Handshake, ProtocolError> {
    write_frame(stream, format!("Server {protocol_version}").as_bytes())?;
    let client_hello = decode_utf8(read_frame(stream)?, "client handshake")?;
    if !client_hello.starts_with("Client") {
        return Err(ProtocolError::UnexpectedClientHello(client_hello));
    }

    let package_name = decode_utf8(read_frame(stream)?, "package name")?;
    let device_id = decode_utf8(read_frame(stream)?, "device identifier")?;
    let configuration = decode_configuration(&read_frame(stream)?)?;

    Ok(Handshake {
        package_name,
        device_id,
        configuration,
    })
}

fn decode_utf8(bytes: Vec<u8>, field: &'static str) -> Result<String, ProtocolError> {
    String::from_utf8(bytes).map_err(|_| ProtocolError::InvalidUtf8(field))
}

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    InvalidFrameLength(i32),
    UnexpectedHello(Vec<u8>),
    UnexpectedClientHello(String),
    InvalidUtf8(&'static str),
    InvalidConfigurationLength(usize),
    InvalidControlPacketLength(usize),
    UnexpectedPacketFamily(i32),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "network I/O error: {error}"),
            Self::InvalidFrameLength(length) => write!(formatter, "invalid frame length: {length}"),
            Self::UnexpectedHello(payload) => {
                write!(formatter, "expected HELLO, received {payload:?}")
            }
            Self::UnexpectedClientHello(value) => {
                write!(formatter, "expected Client handshake, received {value:?}")
            }
            Self::InvalidUtf8(field) => write!(formatter, "{field} is not valid UTF-8"),
            Self::InvalidConfigurationLength(length) => write!(
                formatter,
                "expected 20-byte configuration packet, received {length}"
            ),
            Self::InvalidControlPacketLength(length) => {
                write!(
                    formatter,
                    "invalid family-1 control packet length: {length}"
                )
            }
            Self::UnexpectedPacketFamily(family) => {
                write!(formatter, "expected packet family 3, received {family}")
            }
        }
    }
}

impl Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_signed_big_endian_lengths() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, b"HELLO").unwrap();
        assert_eq!(bytes, b"\0\0\0\x05HELLO");
        assert_eq!(read_frame(&mut bytes.as_slice()).unwrap(), b"HELLO");
    }

    #[test]
    fn rejects_negative_and_too_large_frames() {
        assert!(matches!(
            read_frame(&mut [0xFF, 0xFF, 0xFF, 0xFF].as_slice()),
            Err(ProtocolError::InvalidFrameLength(-1))
        ));
        assert!(matches!(
            read_frame(&mut [0x00, 0x50, 0x00, 0x01].as_slice()),
            Err(ProtocolError::InvalidFrameLength(0x0050_0001))
        ));
    }

    #[test]
    fn decodes_family_three_configuration() {
        let payload = [
            0, 0, 0, 3, // family
            0, 0, 4, 56, // width: 1080
            0, 0, 9, 96, // height: 2400
            0, 0, 0, 1, 0, 0, 0, 2,
        ];
        assert_eq!(
            decode_configuration(&payload).unwrap(),
            Configuration {
                width: 1080,
                height: 2400,
                value3: 1,
                value4: 2,
            }
        );
    }
}
