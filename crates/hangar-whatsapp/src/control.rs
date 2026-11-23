//! Contrato C1 (`control.json` e `control.key` na pasta `--data`) e as assinaturas HMAC do C2.
//!
//! Pedido: `X-Hangar-Signature: <ts>.<hex>`, `hex = HMAC-SHA256(key, METHOD\nURI\nTS\nhex(sha256(body)))`.
//! Resposta: `X-Hangar-Signature: <hex>`, `hex = HMAC-SHA256(key, TS_DO_PEDIDO\nSTATUS\nhex(sha256(body)))`.
use std::path::Path;

use hangar_managed_binary::hex;
use serde::Deserialize;

pub const SIGNATURE_HEADER: &str = "x-hangar-signature";

/// `control.json`, reescrito pelo binário a cada subida antes de aceitar conexão.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Control {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub api: u32,
    pub port: u16,
    pub pid: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Arquivo ausente: o binário nunca subiu nesta pasta.
    Missing,
    Invalid(String),
}

/// Lê `control.json`; arquivo ausente é [`ReadError::Missing`].
pub fn read_control(data: &Path) -> Result<Control, ReadError> {
    let raw = match std::fs::read(data.join("control.json")) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadError::Missing),
        Err(e) => return Err(ReadError::Invalid(e.to_string())),
    };
    let control: Control = serde_json::from_slice(&raw).map_err(|e| ReadError::Invalid(format!("control.json: {e}")))?;
    if control.port == 0 || control.pid == 0 {
        return Err(ReadError::Invalid("control.json sem porta ou pid".into()));
    }
    Ok(control)
}

/// A chave do HMAC são os 32 bytes do hex de `control.key`, nunca o texto.
pub fn read_key(data: &Path) -> Result<Vec<u8>, ReadError> {
    let text = match std::fs::read_to_string(data.join("control.key")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadError::Missing),
        Err(e) => return Err(ReadError::Invalid(e.to_string())),
    };
    parse_key(&text).ok_or_else(|| ReadError::Invalid("control.key não tem 64 dígitos hexadecimais".into()))
}

pub fn parse_key(text: &str) -> Option<Vec<u8>> {
    let key = hex::decode(text.trim())?;
    (key.len() == 32).then_some(key)
}

/// O processo do `control.json` ainda existe; sem ele, a porta pode ser de outro programa.
pub fn alive(control: &Control) -> bool {
    hangar_core::terminal_process::pid_alive(control.pid).unwrap_or(false)
}

/// Cabeçalho do pedido: `<ts>.<hex>`. `uri` é o caminho mais `?query` crua, igual à linha do pedido.
pub fn sign_request(key: &[u8], method: &str, uri: &str, ts: i64, body: &[u8]) -> String {
    let ts = ts.to_string();
    let mac = mac(key, &[method, uri, &ts, &body_hash(body)]);
    format!("{ts}.{mac}")
}

pub fn sign_response(key: &[u8], ts: &str, status: u16, body: &[u8]) -> String {
    mac(key, &[ts, &status.to_string(), &body_hash(body)])
}

/// Confere em tempo constante a assinatura que o binário pôs na resposta ao pedido de `ts`.
pub fn verify_response(key: &[u8], ts: &str, status: u16, body: &[u8], header: &str) -> bool {
    let Some(tag) = hex::decode(header.trim()) else { return false };
    let message = [ts, &status.to_string(), &body_hash(body)].join("\n");
    ring::hmac::verify(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key), message.as_bytes(), &tag).is_ok()
}

fn mac(key: &[u8], parts: &[&str]) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    hex::encode(ring::hmac::sign(&key, parts.join("\n").as_bytes()).as_ref())
}

fn body_hash(body: &[u8]) -> String {
    hex::sha256(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> {
        (0u8..32).collect()
    }

    /// Valores calculados à parte, com `openssl dgst -sha256 -mac HMAC -macopt hexkey:000102…1f`
    /// sobre as mesmas linhas; não saem deste código.
    #[test]
    fn signatures_match_an_independent_vector() {
        assert_eq!(
            sign_request(&key(), "GET", "/v1/messages?chat=5564999999999&limit=2", 1_763_800_000, b""),
            "1763800000.292f24140938d5e9eb440f45c9237190e7a0ad1e66b047a0f409c64ad6415c0a"
        );
        assert_eq!(
            sign_request(&key(), "POST", "/v1/pair", 1_763_800_000, br#"{"phone":"5564999999999"}"#),
            "1763800000.d1d17ab17a612ea6d0602bd344c4a4c7348ef8ab044f1a49e78481cd656e1cc5"
        );
        let response = "21819ffeb0447a37d6c4cf9dbee658d69a1777f1015f4a1f78b7eb1b1d203129";
        assert_eq!(sign_response(&key(), "1763800000", 200, br#"{"ok":true}"#), response);
        assert!(verify_response(&key(), "1763800000", 200, br#"{"ok":true}"#, response));
    }

    #[test]
    fn a_response_signature_binds_the_timestamp_status_and_body() {
        let good = sign_response(&key(), "17", 200, b"{}");
        assert!(verify_response(&key(), "17", 200, b"{}", &good));
        assert!(!verify_response(&key(), "18", 200, b"{}", &good), "outro pedido");
        assert!(!verify_response(&key(), "17", 201, b"{}", &good), "outro status");
        assert!(!verify_response(&key(), "17", 200, b"{ }", &good), "outro corpo");
        assert!(!verify_response(&key(), "17", 200, b"{}", ""), "sem assinatura");
        assert!(!verify_response(&key(), "17", 200, b"{}", "zz"), "não é hex");
        let mut other = key();
        other[0] = 1;
        assert!(!verify_response(&other, "17", 200, b"{}", &good), "outra chave");
        // O 401 de pedido sem ts legível vem assinado com ts vazio.
        let empty = sign_response(&key(), "", 401, b"x");
        assert!(verify_response(&key(), "", 401, b"x", &empty));
    }

    #[test]
    fn the_key_is_the_decoded_bytes_not_the_text() {
        let text = format!("{}\n", hex::encode(&key()));
        assert_eq!(parse_key(&text), Some(key()));
        assert_eq!(parse_key("abcd\n"), None, "curta");
        assert_eq!(parse_key(&"g".repeat(64)), None, "não é hex");
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_key(dir.path()), Err(ReadError::Missing));
        std::fs::write(dir.path().join("control.key"), text).unwrap();
        assert_eq!(read_key(dir.path()).unwrap(), key());
    }

    #[test]
    fn control_json_needs_port_and_pid() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_control(dir.path()), Err(ReadError::Missing));
        std::fs::write(dir.path().join("control.json"), r#"{"version":1,"api":1,"port":40123,"pid":77}"#).unwrap();
        assert_eq!(read_control(dir.path()).unwrap(), Control { version: 1, api: 1, port: 40123, pid: 77 });
        std::fs::write(dir.path().join("control.json"), r#"{"version":1,"api":1,"port":0,"pid":77}"#).unwrap();
        assert!(matches!(read_control(dir.path()), Err(ReadError::Invalid(_))));
        std::fs::write(dir.path().join("control.json"), "{").unwrap();
        assert!(matches!(read_control(dir.path()), Err(ReadError::Invalid(_))));
    }
}
