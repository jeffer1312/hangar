//! Detecção dos bytes JSON e posições originais, inclusive surrogates isolados.
use std::borrow::Cow;

pub(super) struct Decoded<'a> {
    pub text: Cow<'a, str>,
    replacements: Vec<(usize, usize)>,
}
impl Decoded<'_> {
    pub fn position(&self, rendered: usize) -> usize {
        let mut extra = 0;
        for &(start, length) in &self.replacements {
            if rendered < start { break; }
            if rendered < start + length { return start - extra; }
            extra += length - 1;
        }
        rendered - extra
    }
}

#[derive(Clone, Copy)]
enum Encoding { Utf8, Utf16(bool), Utf32(bool) }
fn detect(bytes: &[u8]) -> (Encoding, usize) {
    use Encoding::*;
    if bytes.starts_with(&[0xff, 0xfe, 0, 0]) { return (Utf32(true), 4); }
    if bytes.starts_with(&[0, 0, 0xfe, 0xff]) { return (Utf32(false), 4); }
    if bytes.starts_with(&[0xff, 0xfe]) { return (Utf16(true), 2); }
    if bytes.starts_with(&[0xfe, 0xff]) { return (Utf16(false), 2); }
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) { return (Utf8, 3); }
    if bytes.len() >= 4 {
        if bytes[0] == 0 { return (if bytes[1] == 0 { Utf32(false) } else { Utf16(false) }, 0); }
        if bytes[1] == 0 { return (if bytes[2] != 0 || bytes[3] != 0 { Utf16(true) } else { Utf32(true) }, 0); }
    } else if bytes.len() == 2 {
        if bytes[0] == 0 { return (Utf16(false), 0); }
        if bytes[1] == 0 { return (Utf16(true), 0); }
    }
    (Utf8, 0)
}

#[derive(Default)]
struct Builder { text: String, characters: usize, replacements: Vec<(usize, usize)> }
impl Builder {
    fn push(&mut self, code: u32) -> Result<(), ()> {
        if (0xd800..=0xdfff).contains(&code) {
            let marker = super::codec::surrogate_marker(code);
            let count = marker.chars().count();
            self.replacements.push((self.characters, count));
            self.characters += count;
            self.text.push_str(&marker);
        } else {
            self.text.push(char::from_u32(code).ok_or(())?);
            self.characters += 1;
        }
        Ok(())
    }
    fn utf8(&mut self, mut bytes: &[u8]) -> Result<(), ()> {
        while !bytes.is_empty() {
            let (valid, error) = match std::str::from_utf8(bytes) {
                Ok(text) => (text, false),
                Err(error) => (std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(|_| ())?, true),
            };
            self.text.push_str(valid);
            self.characters += valid.chars().count();
            bytes = &bytes[valid.len()..];
            if error {
                if bytes.len() < 3 || bytes[0] != 0xed || !(0xa0..=0xbf).contains(&bytes[1]) || !(0x80..=0xbf).contains(&bytes[2]) {
                    return Err(());
                }
                self.push(0xd000 | (u32::from(bytes[1] & 0x3f) << 6) | u32::from(bytes[2] & 0x3f))?;
                bytes = &bytes[3..];
            }
        }
        Ok(())
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<Decoded<'_>, ()> {
    let (encoding, skip) = detect(bytes);
    let bytes = &bytes[skip..];
    if matches!(encoding, Encoding::Utf8) && let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(Decoded { text: Cow::Borrowed(text), replacements: Vec::new() });
    }
    let mut output = Builder::default();
    match encoding {
        Encoding::Utf8 => output.utf8(bytes)?,
        Encoding::Utf16(little) => {
            if bytes.len() % 2 != 0 { return Err(()); }
            let read = |at| {
                let pair = [bytes[at], bytes[at + 1]];
                if little { u16::from_le_bytes(pair) } else { u16::from_be_bytes(pair) }
            };
            let mut at = 0;
            while at < bytes.len() {
                let first = read(at);
                at += 2;
                if (0xd800..=0xdbff).contains(&first) && at < bytes.len() && (0xdc00..=0xdfff).contains(&read(at)) {
                    let second = read(at);
                    at += 2;
                    output.push(0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00)?;
                } else { output.push(u32::from(first))?; }
            }
        }
        Encoding::Utf32(little) => {
            if bytes.len() % 4 != 0 { return Err(()); }
            for chunk in bytes.chunks_exact(4) {
                let chunk: [u8; 4] = chunk.try_into().map_err(|_| ())?;
                output.push(if little { u32::from_le_bytes(chunk) } else { u32::from_be_bytes(chunk) })?;
            }
        }
    }
    Ok(Decoded { text: Cow::Owned(output.text), replacements: output.replacements })
}
