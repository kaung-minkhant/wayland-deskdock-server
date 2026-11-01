//! Linux evdev key codes to the Android key codes the DeskDock companion
//! expects in family-2 key events.
//!
//! The Android codes follow the original server's `ab.a(awtKey, location)`
//! table (see `analysis/cfr/ab.java`). Printable keys also carry the produced
//! character for a US layout, as the original does for unmodified typing.
//! Deliberate deviations: the apostrophe (the original table yields 0) is
//! APOSTROPHE (75), and the keypad dot is NUMPAD_DOT (158, original 159).

use evdev::KeyCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mapped {
    /// `android.view.KeyEvent` key code.
    pub android: u32,
    /// Characters produced without and with Shift, for printable keys.
    pub chars: Option<(char, char)>,
}

const fn key(android: u32) -> Mapped {
    Mapped {
        android,
        chars: None,
    }
}

const fn printable(android: u32, plain: char, shifted: char) -> Mapped {
    Mapped {
        android,
        chars: Some((plain, shifted)),
    }
}

pub fn lookup(code: KeyCode) -> Option<Mapped> {
    Some(match code {
        KeyCode::KEY_0 => printable(7, '0', ')'),
        KeyCode::KEY_1 => printable(8, '1', '!'),
        KeyCode::KEY_2 => printable(9, '2', '@'),
        KeyCode::KEY_3 => printable(10, '3', '#'),
        KeyCode::KEY_4 => printable(11, '4', '$'),
        KeyCode::KEY_5 => printable(12, '5', '%'),
        KeyCode::KEY_6 => printable(13, '6', '^'),
        KeyCode::KEY_7 => printable(14, '7', '&'),
        KeyCode::KEY_8 => printable(15, '8', '*'),
        KeyCode::KEY_9 => printable(16, '9', '('),
        KeyCode::KEY_A => printable(29, 'a', 'A'),
        KeyCode::KEY_B => printable(30, 'b', 'B'),
        KeyCode::KEY_C => printable(31, 'c', 'C'),
        KeyCode::KEY_D => printable(32, 'd', 'D'),
        KeyCode::KEY_E => printable(33, 'e', 'E'),
        KeyCode::KEY_F => printable(34, 'f', 'F'),
        KeyCode::KEY_G => printable(35, 'g', 'G'),
        KeyCode::KEY_H => printable(36, 'h', 'H'),
        KeyCode::KEY_I => printable(37, 'i', 'I'),
        KeyCode::KEY_J => printable(38, 'j', 'J'),
        KeyCode::KEY_K => printable(39, 'k', 'K'),
        KeyCode::KEY_L => printable(40, 'l', 'L'),
        KeyCode::KEY_M => printable(41, 'm', 'M'),
        KeyCode::KEY_N => printable(42, 'n', 'N'),
        KeyCode::KEY_O => printable(43, 'o', 'O'),
        KeyCode::KEY_P => printable(44, 'p', 'P'),
        KeyCode::KEY_Q => printable(45, 'q', 'Q'),
        KeyCode::KEY_R => printable(46, 'r', 'R'),
        KeyCode::KEY_S => printable(47, 's', 'S'),
        KeyCode::KEY_T => printable(48, 't', 'T'),
        KeyCode::KEY_U => printable(49, 'u', 'U'),
        KeyCode::KEY_V => printable(50, 'v', 'V'),
        KeyCode::KEY_W => printable(51, 'w', 'W'),
        KeyCode::KEY_X => printable(52, 'x', 'X'),
        KeyCode::KEY_Y => printable(53, 'y', 'Y'),
        KeyCode::KEY_Z => printable(54, 'z', 'Z'),
        KeyCode::KEY_COMMA => printable(55, ',', '<'),
        KeyCode::KEY_DOT => printable(56, '.', '>'),
        KeyCode::KEY_GRAVE => printable(68, '`', '~'),
        KeyCode::KEY_MINUS => printable(69, '-', '_'),
        KeyCode::KEY_EQUAL => printable(70, '=', '+'),
        KeyCode::KEY_LEFTBRACE => printable(71, '[', '{'),
        KeyCode::KEY_RIGHTBRACE => printable(72, ']', '}'),
        KeyCode::KEY_BACKSLASH => printable(73, '\\', '|'),
        KeyCode::KEY_SEMICOLON => printable(74, ';', ':'),
        KeyCode::KEY_APOSTROPHE => printable(75, '\'', '"'),
        KeyCode::KEY_SLASH => printable(76, '/', '?'),
        KeyCode::KEY_SPACE => printable(62, ' ', ' '),

        KeyCode::KEY_ESC => key(111),
        KeyCode::KEY_TAB => key(61),
        KeyCode::KEY_ENTER => key(66),
        KeyCode::KEY_BACKSPACE => key(67),
        KeyCode::KEY_DELETE => key(112),
        KeyCode::KEY_INSERT => key(124),
        // Like the original, Home is Android's HOME key, not MOVE_HOME.
        KeyCode::KEY_HOME => key(3),
        KeyCode::KEY_END => key(123),
        KeyCode::KEY_PAGEUP => key(92),
        KeyCode::KEY_PAGEDOWN => key(93),
        KeyCode::KEY_UP => key(19),
        KeyCode::KEY_DOWN => key(20),
        KeyCode::KEY_LEFT => key(21),
        KeyCode::KEY_RIGHT => key(22),
        KeyCode::KEY_COMPOSE => key(82),
        KeyCode::KEY_SYSRQ => key(120),
        KeyCode::KEY_PAUSE => key(127),
        KeyCode::KEY_SCROLLLOCK => key(116),
        KeyCode::KEY_NUMLOCK => key(143),
        KeyCode::KEY_CAPSLOCK => key(115),

        KeyCode::KEY_LEFTSHIFT => key(59),
        KeyCode::KEY_RIGHTSHIFT => key(60),
        KeyCode::KEY_LEFTALT => key(57),
        KeyCode::KEY_RIGHTALT => key(58),
        KeyCode::KEY_LEFTCTRL => key(113),
        KeyCode::KEY_RIGHTCTRL => key(114),
        KeyCode::KEY_LEFTMETA => key(117),
        KeyCode::KEY_RIGHTMETA => key(118),

        KeyCode::KEY_F1 => key(131),
        KeyCode::KEY_F2 => key(132),
        KeyCode::KEY_F3 => key(133),
        KeyCode::KEY_F4 => key(134),
        KeyCode::KEY_F5 => key(135),
        KeyCode::KEY_F6 => key(136),
        KeyCode::KEY_F7 => key(137),
        KeyCode::KEY_F8 => key(138),
        KeyCode::KEY_F9 => key(139),
        KeyCode::KEY_F10 => key(140),
        KeyCode::KEY_F11 => key(141),
        KeyCode::KEY_F12 => key(142),

        KeyCode::KEY_KP0 => key(144),
        KeyCode::KEY_KP1 => key(145),
        KeyCode::KEY_KP2 => key(146),
        KeyCode::KEY_KP3 => key(147),
        KeyCode::KEY_KP4 => key(148),
        KeyCode::KEY_KP5 => key(149),
        KeyCode::KEY_KP6 => key(150),
        KeyCode::KEY_KP7 => key(151),
        KeyCode::KEY_KP8 => key(152),
        KeyCode::KEY_KP9 => key(153),
        KeyCode::KEY_KPSLASH => key(154),
        KeyCode::KEY_KPASTERISK => key(155),
        KeyCode::KEY_KPMINUS => key(156),
        KeyCode::KEY_KPPLUS => key(157),
        KeyCode::KEY_KPDOT => key(158),
        KeyCode::KEY_KPENTER => key(66),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_use_android_codes() {
        assert_eq!(lookup(KeyCode::KEY_A).unwrap().android, 29);
        assert_eq!(lookup(KeyCode::KEY_Z).unwrap().android, 54);
        assert_eq!(lookup(KeyCode::KEY_0).unwrap().android, 7);
        assert_eq!(lookup(KeyCode::KEY_9).unwrap().android, 16);
    }

    #[test]
    fn printable_keys_carry_characters() {
        assert_eq!(lookup(KeyCode::KEY_A).unwrap().chars, Some(('a', 'A')));
        assert_eq!(lookup(KeyCode::KEY_1).unwrap().chars, Some(('1', '!')));
        assert_eq!(lookup(KeyCode::KEY_ENTER).unwrap().chars, None);
    }

    #[test]
    fn control_keys_match_original_table() {
        assert_eq!(lookup(KeyCode::KEY_ENTER).unwrap().android, 66);
        assert_eq!(lookup(KeyCode::KEY_BACKSPACE).unwrap().android, 67);
        assert_eq!(lookup(KeyCode::KEY_ESC).unwrap().android, 111);
        assert_eq!(lookup(KeyCode::KEY_LEFTSHIFT).unwrap().android, 59);
        assert_eq!(lookup(KeyCode::KEY_F1).unwrap().android, 131);
    }

    #[test]
    fn unmapped_keys_are_none() {
        assert_eq!(lookup(KeyCode::KEY_VOLUMEUP), None);
    }
}
