//! Serialization of DeskDock family-2 outbound input events.
//!
//! These layouts are matched against the original Java server's `cx`, `bq`,
//! `bk`, `bo`, and `bm` packet writers. Events are deliberately typed so input
//! capture backends cannot construct malformed packets ad hoc.

use crate::protocol::{ProtocolError, write_frame};
use std::io::Write;

const FAMILY: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    SetEnabled(bool),
    Key {
        key_code: u32,
        character: u16,
        is_down: bool,
    },
    PointerMove {
        x: u32,
        y: u32,
    },
    PointerButton {
        button: u32,
        is_down: bool,
        x: u32,
        y: u32,
    },
    /// Wheel at a pointer position, as the original `bp(x, y, horizontal,
    /// vertical)`. Positive vertical scrolls up (the original sends
    /// `-getWheelRotation()` from AWT, whose positive rotation is down).
    Scroll {
        x: u32,
        y: u32,
        horizontal: i32,
        vertical: i32,
    },
}

impl Event {
    /// Encode the complete family-2 payload, excluding the outer frame length.
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(18);
        payload.extend_from_slice(&FAMILY.to_be_bytes());

        match self {
            Self::SetEnabled(enabled) => {
                payload.extend([0, u8::from(*enabled)]);
            }
            Self::Key {
                key_code,
                character,
                is_down,
            } => {
                payload.push(2);
                payload.extend_from_slice(&key_code.to_be_bytes());
                payload.extend_from_slice(&character.to_be_bytes());
                payload.push(u8::from(*is_down));
            }
            Self::PointerMove { x, y } => {
                payload.push(3);
                payload.extend_from_slice(&x.to_be_bytes());
                payload.extend_from_slice(&y.to_be_bytes());
                // The original server always emits zero for the third word.
                payload.extend_from_slice(&0_u32.to_be_bytes());
            }
            Self::PointerButton {
                button,
                is_down,
                x,
                y,
            } => {
                payload.push(4);
                payload.extend_from_slice(&button.to_be_bytes());
                payload.push(u8::from(*is_down));
                payload.extend_from_slice(&x.to_be_bytes());
                payload.extend_from_slice(&y.to_be_bytes());
            }
            Self::Scroll {
                x,
                y,
                horizontal,
                vertical,
            } => {
                payload.push(5);
                payload.extend_from_slice(&x.to_be_bytes());
                payload.extend_from_slice(&y.to_be_bytes());
                payload.extend_from_slice(&horizontal.to_be_bytes());
                payload.extend_from_slice(&vertical.to_be_bytes());
            }
        }

        payload
    }
}

pub fn send(stream: &mut impl Write, event: &Event) -> Result<(), ProtocolError> {
    write_frame(stream, &event.encode())
}

/// Family-1 command sent by Android after the handshake. The original desktop
/// server responds by launching its Android-side companion service and then
/// sends command 2 to let Android continue setup.
pub fn send_service_started(stream: &mut impl Write) -> Result<(), ProtocolError> {
    write_frame(stream, &[0, 0, 0, 1, 0, 0, 0, 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_the_service_started_control_packet() {
        let mut bytes = Vec::new();
        send_service_started(&mut bytes).unwrap();
        assert_eq!(bytes, [0, 0, 0, 8, 0, 0, 0, 1, 0, 0, 0, 2]);
    }

    #[test]
    fn encodes_a_pointer_move_like_the_original_server() {
        assert_eq!(
            Event::PointerMove { x: 1080, y: 2400 }.encode(),
            [
                0, 0, 0, 2, // family
                3, // subtype
                0, 0, 4, 56, // x
                0, 0, 9, 96, // y
                0, 0, 0, 0, // original server's reserved word
            ]
        );
    }

    #[test]
    fn encodes_a_key_event_and_outer_frame() {
        let event = Event::Key {
            key_code: 65,
            character: 'A' as u16,
            is_down: true,
        };
        let mut bytes = Vec::new();
        send(&mut bytes, &event).unwrap();
        assert_eq!(
            bytes,
            [
                0, 0, 0, 12, // outer payload length
                0, 0, 0, 2, // family
                2, // subtype
                0, 0, 0, 65, // key code
                0, 65, // UTF-16 code unit
                1,  // down
            ]
        );
    }

    #[test]
    fn encodes_a_scroll_event_like_the_original_bp_packet() {
        assert_eq!(
            Event::Scroll {
                x: 10,
                y: 20,
                horizontal: 0,
                vertical: -1,
            }
            .encode(),
            [
                0, 0, 0, 2, 5, 0, 0, 0, 10, 0, 0, 0, 20, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff
            ]
        );
    }

    #[test]
    fn encodes_a_pointer_button_event() {
        assert_eq!(
            Event::PointerButton {
                button: 1,
                is_down: false,
                x: 10,
                y: 20,
            }
            .encode(),
            [0, 0, 0, 2, 4, 0, 0, 0, 1, 0, 0, 0, 0, 10, 0, 0, 0, 20,]
        );
    }
}
