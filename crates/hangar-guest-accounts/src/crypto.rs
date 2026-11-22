//! O lado da máquina no esquema do cofre: verificador guardado, sal falso e o hash publicado no serviço.
//! Chaves e cifra são as dos clientes (`hangar_connect_client::crypto`): quem entra decifra com o mesmo código.
use std::num::NonZeroU32;

use base64::{Engine, engine::general_purpose::STANDARD};
use ring::{digest, hmac, pbkdf2};

pub use hangar_connect_client::crypto::{Blob, ITERATIONS, Keys, decrypt, derive_keys};
pub(crate) use hangar_connect_client::crypto::{encrypt, random};

/// O custo do verificador guardado, o mesmo do `sync-vault.json`.
const VERIFIER_ITERATIONS: u32 = 200_000;

/// `PBKDF2-SHA256(auth_hash, salt, 200 mil)` em base64: o verificador guardado aqui e no `sync-vault.json`.
pub fn verifier(auth_hash: &str, salt: &[u8]) -> String {
    let mut out = [0u8; 32];
    let iterations = NonZeroU32::new(VERIFIER_ITERATIONS).expect("constante positiva");
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, iterations, salt, auth_hash.as_bytes(), &mut out);
    STANDARD.encode(out)
}

pub(crate) fn fake_salt(secret: &[u8], user: &str) -> String {
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, secret), format!("prelogin:{user}").as_bytes());
    STANDARD.encode(&tag.as_ref()[..16])
}

/// O que o serviço do Connect guarda: nunca o nome em texto.
pub fn user_hash(user: &str) -> String {
    let hash = digest::digest(&digest::SHA256, format!("hangar-guest:{}", user.to_ascii_lowercase()).as_bytes());
    hash.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn decode<const N: usize>(text: &str) -> Option<[u8; N]> {
    STANDARD.decode(text).ok()?.try_into().ok()
}

pub(crate) fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_hash_is_sha256_of_the_prefixed_lowercase_name() {
        // Calculado fora: `printf 'hangar-guest:ana.silva' | sha256sum`.
        let expected = "af04996f6b2cce66327d6aa6da8d18b9b1b40ff4a0aeadd45877fc4e4f26b599";
        assert_eq!(user_hash("Ana.Silva"), expected);
        assert_eq!(user_hash("ana.silva"), expected);
    }
}
