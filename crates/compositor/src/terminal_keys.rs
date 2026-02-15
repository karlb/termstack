//! Shared terminal key-to-bytes conversion
//!
//! Both the Linux (Smithay keysym) and macOS (winit Key) backends convert
//! keyboard input into byte sequences for terminal PTY input. This module
//! provides the shared escape-sequence table so each backend only needs a
//! thin adapter from its native key type to `TerminalKey`.

/// Normalized terminal key — the shared representation that both backends
/// convert their native key types into before generating PTY bytes.
pub enum TerminalKey<'a> {
    /// UTF-8 string slice (from winit `Key::Character`)
    Str(&'a str),
    /// Single Unicode character (from keysym raw value)
    Char(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowRight,
    ArrowLeft,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

/// Map a character to its Ctrl+key code (0x01–0x1F), or `None` if the
/// character doesn't have a standard control code.
pub fn ctrl_char_code(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        'a' => Some(1),
        'b' => Some(2),
        'c' => Some(3),
        'd' => Some(4),
        'e' => Some(5),
        'f' => Some(6),
        'g' => Some(7),
        'h' => Some(8),
        'i' => Some(9),
        'j' => Some(10),
        'k' => Some(11),
        'l' => Some(12),
        'm' => Some(13),
        'n' => Some(14),
        'o' => Some(15),
        'p' => Some(16),
        'q' => Some(17),
        'r' => Some(18),
        's' => Some(19),
        't' => Some(20),
        'u' => Some(21),
        'v' => Some(22),
        'w' => Some(23),
        'x' => Some(24),
        'y' => Some(25),
        'z' => Some(26),
        '[' => Some(27),
        '\\' => Some(28),
        ']' => Some(29),
        '^' => Some(30),
        '_' => Some(31),
        _ => None,
    }
}

/// Convert a normalized terminal key to the byte sequence that should be
/// written to the PTY.
///
/// - `ctrl`: Ctrl modifier is held — maps letter/symbol keys to control codes
/// - `alt`:  Alt modifier is held — prepends ESC (0x1B) to the result
pub fn terminal_key_to_bytes(key: TerminalKey, ctrl: bool, alt: bool) -> Vec<u8> {
    // Handle Ctrl+character → control codes (0x01–0x1F)
    if ctrl {
        let c = match &key {
            TerminalKey::Str(s) => s.chars().next(),
            TerminalKey::Char(c) => Some(*c),
            _ => None,
        };
        if let Some(c) = c {
            if let Some(code) = ctrl_char_code(c) {
                return vec![code];
            }
        }
    }

    let mut result = match key {
        TerminalKey::Str(s) => s.as_bytes().to_vec(),
        TerminalKey::Char(c) => {
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf).as_bytes().to_vec()
        }

        TerminalKey::Enter => vec![b'\r'],
        TerminalKey::Backspace => vec![0x7f],
        TerminalKey::Tab => vec![b'\t'],
        TerminalKey::Escape => vec![0x1b],
        TerminalKey::Space => vec![b' '],

        // Arrow keys
        TerminalKey::ArrowUp => vec![0x1b, b'[', b'A'],
        TerminalKey::ArrowDown => vec![0x1b, b'[', b'B'],
        TerminalKey::ArrowRight => vec![0x1b, b'[', b'C'],
        TerminalKey::ArrowLeft => vec![0x1b, b'[', b'D'],

        // Home/End
        TerminalKey::Home => vec![0x1b, b'[', b'H'],
        TerminalKey::End => vec![0x1b, b'[', b'F'],

        // Page Up/Down
        TerminalKey::PageUp => vec![0x1b, b'[', b'5', b'~'],
        TerminalKey::PageDown => vec![0x1b, b'[', b'6', b'~'],

        // Insert/Delete
        TerminalKey::Insert => vec![0x1b, b'[', b'2', b'~'],
        TerminalKey::Delete => vec![0x1b, b'[', b'3', b'~'],

        // Function keys
        TerminalKey::F1 => vec![0x1b, b'O', b'P'],
        TerminalKey::F2 => vec![0x1b, b'O', b'Q'],
        TerminalKey::F3 => vec![0x1b, b'O', b'R'],
        TerminalKey::F4 => vec![0x1b, b'O', b'S'],
        TerminalKey::F5 => vec![0x1b, b'[', b'1', b'5', b'~'],
        TerminalKey::F6 => vec![0x1b, b'[', b'1', b'7', b'~'],
        TerminalKey::F7 => vec![0x1b, b'[', b'1', b'8', b'~'],
        TerminalKey::F8 => vec![0x1b, b'[', b'1', b'9', b'~'],
        TerminalKey::F9 => vec![0x1b, b'[', b'2', b'0', b'~'],
        TerminalKey::F10 => vec![0x1b, b'[', b'2', b'1', b'~'],
        TerminalKey::F11 => vec![0x1b, b'[', b'2', b'3', b'~'],
        TerminalKey::F12 => vec![0x1b, b'[', b'2', b'4', b'~'],
    };

    // Alt prefix: prepend ESC before the key's bytes
    if alt && !result.is_empty() {
        result.insert(0, 0x1b);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_a_through_z() {
        for (i, c) in ('a'..='z').enumerate() {
            assert_eq!(ctrl_char_code(c), Some((i + 1) as u8));
            assert_eq!(ctrl_char_code(c.to_ascii_uppercase()), Some((i + 1) as u8));
        }
    }

    #[test]
    fn ctrl_symbols() {
        assert_eq!(ctrl_char_code('['), Some(27));
        assert_eq!(ctrl_char_code('\\'), Some(28));
        assert_eq!(ctrl_char_code(']'), Some(29));
        assert_eq!(ctrl_char_code('^'), Some(30));
        assert_eq!(ctrl_char_code('_'), Some(31));
    }

    #[test]
    fn ctrl_char_sends_code() {
        let bytes = terminal_key_to_bytes(TerminalKey::Char('c'), true, false);
        assert_eq!(bytes, vec![3]); // Ctrl+C = ETX
    }

    #[test]
    fn ctrl_str_sends_code() {
        let bytes = terminal_key_to_bytes(TerminalKey::Str("a"), true, false);
        assert_eq!(bytes, vec![1]); // Ctrl+A = SOH
    }

    #[test]
    fn enter_produces_cr() {
        assert_eq!(terminal_key_to_bytes(TerminalKey::Enter, false, false), vec![b'\r']);
    }

    #[test]
    fn arrow_keys() {
        assert_eq!(terminal_key_to_bytes(TerminalKey::ArrowUp, false, false), vec![0x1b, b'[', b'A']);
        assert_eq!(terminal_key_to_bytes(TerminalKey::ArrowDown, false, false), vec![0x1b, b'[', b'B']);
    }

    #[test]
    fn function_keys() {
        assert_eq!(terminal_key_to_bytes(TerminalKey::F1, false, false), vec![0x1b, b'O', b'P']);
        assert_eq!(terminal_key_to_bytes(TerminalKey::F12, false, false), vec![0x1b, b'[', b'2', b'4', b'~']);
    }

    #[test]
    fn alt_prefix() {
        let bytes = terminal_key_to_bytes(TerminalKey::Char('x'), false, true);
        assert_eq!(bytes, vec![0x1b, b'x']);
    }

    #[test]
    fn regular_char() {
        let bytes = terminal_key_to_bytes(TerminalKey::Char('a'), false, false);
        assert_eq!(bytes, vec![b'a']);
    }

    #[test]
    fn regular_str() {
        let bytes = terminal_key_to_bytes(TerminalKey::Str("hello"), false, false);
        assert_eq!(bytes, b"hello".to_vec());
    }

    #[test]
    fn unicode_char() {
        let bytes = terminal_key_to_bytes(TerminalKey::Char('é'), false, false);
        assert_eq!(bytes, "é".as_bytes().to_vec());
    }
}
