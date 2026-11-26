//! Character encodings of what a device sends and takes (#24).
//!
//! The terminal and the session log speak UTF-8. Older equipment sends
//! ISO 8859-1, Windows-1252 or CP437 (the line drawing of its menus): the
//! session converts what it receives to UTF-8 and what the user types to
//! the device's encoding. All of these are single-byte, so each chunk is
//! converted on its own, and ASCII (including every escape sequence) is
//! untouched.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::workspace::Encoding;

const CP437_HIGH: [char; 128] = [
    '\u{00c7}', '\u{00fc}', '\u{00e9}', '\u{00e2}', '\u{00e4}', '\u{00e0}', '\u{00e5}', '\u{00e7}',
    '\u{00ea}', '\u{00eb}', '\u{00e8}', '\u{00ef}', '\u{00ee}', '\u{00ec}', '\u{00c4}', '\u{00c5}',
    '\u{00c9}', '\u{00e6}', '\u{00c6}', '\u{00f4}', '\u{00f6}', '\u{00f2}', '\u{00fb}', '\u{00f9}',
    '\u{00ff}', '\u{00d6}', '\u{00dc}', '\u{00a2}', '\u{00a3}', '\u{00a5}', '\u{20a7}', '\u{0192}',
    '\u{00e1}', '\u{00ed}', '\u{00f3}', '\u{00fa}', '\u{00f1}', '\u{00d1}', '\u{00aa}', '\u{00ba}',
    '\u{00bf}', '\u{2310}', '\u{00ac}', '\u{00bd}', '\u{00bc}', '\u{00a1}', '\u{00ab}', '\u{00bb}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}',
    '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255d}', '\u{255c}', '\u{255b}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252c}', '\u{251c}', '\u{2500}', '\u{253c}', '\u{255e}', '\u{255f}',
    '\u{255a}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256c}', '\u{2567}',
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}', '\u{2558}', '\u{2552}', '\u{2553}', '\u{256b}',
    '\u{256a}', '\u{2518}', '\u{250c}', '\u{2588}', '\u{2584}', '\u{258c}', '\u{2590}', '\u{2580}',
    '\u{03b1}', '\u{00df}', '\u{0393}', '\u{03c0}', '\u{03a3}', '\u{03c3}', '\u{00b5}', '\u{03c4}',
    '\u{03a6}', '\u{0398}', '\u{03a9}', '\u{03b4}', '\u{221e}', '\u{03c6}', '\u{03b5}', '\u{2229}',
    '\u{2261}', '\u{00b1}', '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00f7}', '\u{2248}',
    '\u{00b0}', '\u{2219}', '\u{00b7}', '\u{221a}', '\u{207f}', '\u{00b2}', '\u{25a0}', '\u{00a0}',
];

const CP1252_C1: [char; 32] = [
    '\u{20ac}', '\u{0081}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008d}', '\u{017d}', '\u{008f}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}', '\u{0153}', '\u{009d}', '\u{017e}', '\u{0178}',
];

impl Encoding {
    /// What the device sent, as UTF-8.
    pub fn decode(self, bytes: &[u8]) -> Vec<u8> {
        if self == Self::Utf8 {
            return bytes.to_vec();
        }
        let mut out = Vec::with_capacity(bytes.len());
        let mut buffer = [0; 4];
        for &byte in bytes {
            if byte < 0x80 {
                out.push(byte);
            } else {
                out.extend_from_slice(self.char_of(byte).encode_utf8(&mut buffer).as_bytes());
            }
        }
        out
    }

    /// What the user typed (UTF-8), as the device takes it. A character the
    /// encoding lacks becomes `?`. Bytes that are not UTF-8 (a terminal's
    /// binary reports) go as they are.
    pub fn encode(self, bytes: &[u8]) -> Vec<u8> {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return bytes.to_vec();
        };
        if self == Self::Utf8 {
            return bytes.to_vec();
        }
        text.chars()
            .map(|c| {
                if c.is_ascii() {
                    c as u8
                } else {
                    self.byte_of(c).unwrap_or(b'?')
                }
            })
            .collect()
    }

    /// The character a byte from 0x80 up stands for.
    fn char_of(self, byte: u8) -> char {
        match self {
            Self::Utf8 | Self::Iso8859_1 => char::from(byte),
            Self::Windows1252 => match byte {
                0x80..=0x9f => CP1252_C1[usize::from(byte - 0x80)],
                _ => char::from(byte),
            },
            Self::Cp437 => CP437_HIGH[usize::from(byte - 0x80)],
        }
    }

    fn byte_of(self, c: char) -> Option<u8> {
        match self {
            Self::Utf8 => None,
            Self::Iso8859_1 => u8::try_from(u32::from(c)).ok(),
            Self::Windows1252 => match u8::try_from(u32::from(c)) {
                Ok(byte) if !(0x80..=0x9f).contains(&byte) => Some(byte),
                _ => CP1252_C1
                    .iter()
                    .position(|known| *known == c)
                    .map(|index| 0x80 + index as u8),
            },
            Self::Cp437 => {
                static REVERSE: OnceLock<HashMap<char, u8>> = OnceLock::new();
                REVERSE
                    .get_or_init(|| {
                        CP437_HIGH
                            .iter()
                            .enumerate()
                            .map(|(index, c)| (*c, 0x80 + index as u8))
                            .collect()
                    })
                    .get(&c)
                    .copied()
            }
        }
    }
}
