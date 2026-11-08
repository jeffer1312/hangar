//! Identificador, token aleatório e resumo SHA-256 de credencial.
use base64::Engine;
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};

pub fn hash(token: &[u8]) -> String {
    digest::digest(&digest::SHA256, token)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn random(n: usize) -> Vec<u8> {
    let mut bytes = vec![0; n];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("gerador aleatório do sistema");
    bytes
}
pub fn token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random(32))
}
pub fn id() -> String {
    random(8).iter().map(|b| format!("{b:02x}")).collect()
}
