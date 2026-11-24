//! O esquema do cofre de sincronização dos clientes (`frontend/src/lib/sync.ts`, `syncCrypto.ts` do Expo):
//! PBKDF2-SHA256 da senha, HKDF em dois ramos (`cp-auth` vai à máquina, `cp-enc` decifra o cofre) e
//! AES-256-GCM com a tag no fim de `data`. A máquina que guarda o convidado cifra com o mesmo código.
use std::num::NonZeroU32;

use base64::{Engine, engine::general_purpose::STANDARD};
use ring::rand::{SecureRandom, SystemRandom};
use ring::{aead, hkdf, pbkdf2};
use serde::{Deserialize, Serialize};

/// O mínimo que os clientes aceitam no `prelogin`.
pub const ITERATIONS: u32 = 600_000;

pub struct Keys {
    /// O que o cliente manda no login, em base64.
    pub auth_hash: String,
    pub enc_key: [u8; 32],
}

/// `{iv, data}` em base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    pub iv: String,
    pub data: String,
}

/// # Panics
///
/// Never: 32 bytes sempre cabem no HKDF-SHA256.
pub fn derive_keys(password: &str, salt: &[u8], iterations: u32) -> Keys {
    let mut master = [0u8; 32];
    let iterations = NonZeroU32::new(iterations).unwrap_or(NonZeroU32::MIN);
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, iterations, salt, password.as_bytes(), &mut master);
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(&master);
    let branch = |info: &[u8]| {
        let mut out = [0u8; 32];
        prk.expand(&[info], hkdf::HKDF_SHA256).and_then(|okm| okm.fill(&mut out)).expect("32 bytes cabem no HKDF-SHA256");
        out
    };
    Keys { auth_hash: STANDARD.encode(branch(b"cp-auth")), enc_key: branch(b"cp-enc") }
}

/// # Errors
///
/// Sem fonte de aleatoriedade para o `iv`, ou a cifra recusou a chave.
pub fn encrypt(key: &[u8; 32], plain: &[u8]) -> std::io::Result<Blob> {
    let iv = random::<12>()?;
    let mut data = plain.to_vec();
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).map_err(|_| std::io::Error::other("chave inválida"))?);
    key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(iv), aead::Aad::empty(), &mut data)
        .map_err(|_| std::io::Error::other("cifra falhou"))?;
    Ok(Blob { iv: STANDARD.encode(iv), data: STANDARD.encode(data) })
}

/// `None`: chave errada, base64 torto ou cofre adulterado.
pub fn decrypt(key: &[u8; 32], blob: &Blob) -> Option<Vec<u8>> {
    let iv: [u8; 12] = STANDARD.decode(&blob.iv).ok()?.try_into().ok()?;
    let mut data = STANDARD.decode(&blob.data).ok()?;
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).ok()?);
    key.open_in_place(aead::Nonce::assume_unique_for_key(iv), aead::Aad::empty(), &mut data).ok().map(|plain| plain.to_vec())
}

/// # Errors
///
/// O sistema não deu bytes aleatórios.
pub fn random<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    SystemRandom::new().fill(&mut bytes).map_err(|_| std::io::Error::other("sem fonte de aleatoriedade"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use ring::hmac;

    use super::*;

    #[test]
    fn sealed_blob_opens_with_the_same_key_only() {
        let key = [7u8; 32];
        let blob = encrypt(&key, b"[1,2]").unwrap();
        assert_eq!(decrypt(&key, &blob).as_deref(), Some(&b"[1,2]"[..]));
        assert!(decrypt(&[8u8; 32], &blob).is_none());
    }

    #[test]
    fn derivation_matches_the_rfc_hkdf_with_empty_salt() {
        // HKDF com sal vazio é HMAC com chave de zeros (RFC 5869 §2.2), como faz o WebCrypto.
        let keys = derive_keys("senha", b"0123456789abcdef", 1);
        let mut master = [0u8; 32];
        pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::MIN, b"0123456789abcdef", b"senha", &mut master);
        let prk = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &[0u8; 32]), &master);
        let okm = |info: &[u8]| hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, prk.as_ref()), &[info, &[1u8][..]].concat());
        assert_eq!(keys.auth_hash, STANDARD.encode(okm(b"cp-auth").as_ref()));
        assert_eq!(&keys.enc_key[..], okm(b"cp-enc").as_ref());
    }
}
