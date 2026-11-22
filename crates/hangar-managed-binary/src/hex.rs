//! Hex minúsculo, como o `checksums.txt` das releases e as assinaturas HMAC.
const DIGITS: &[u8; 16] = b"0123456789abcdef";

pub fn encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// `None` com tamanho ímpar ou caractere que não é hex.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

pub fn sha256(data: &[u8]) -> String {
    encode(ring::digest::digest(&ring::digest::SHA256, data).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_a_known_digest() {
        assert_eq!(encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(decode("000FA5ff"), Some(vec![0x00, 0x0f, 0xa5, 0xff]));
        assert_eq!(decode("abc"), None, "ímpar");
        assert_eq!(decode("zz"), None);
        // `printf abc | sha256sum`.
        assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
