//! Web Push com a mesma chave VAPID do Python (RFC 8291 `aes128gcm` e RFC 8292), pelo `ring`.
use base64::Engine;
use ring::{aead, agreement, hkdf, rand::SystemRandom, signature::{self, KeyPair}};
use serde_json::Value;
use std::collections::BTreeMap;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
/// Tamanho de registro anunciado no cabeçalho; o aviso cabe num registro só.
const RECORD: u32 = 4096;

pub struct Vapid { pub public_b64: String, public: Vec<u8>, private: Vec<u8>, subject: String }

fn decode(text: &str) -> Option<Vec<u8>> { B64.decode(text.trim().trim_end_matches('=')).ok() }

/// `CP_VAPID_*` do ambiente e do `backend/.env`. Sem as duas chaves o Web Push fica desligado, como no Python.
pub fn vapid(env: &BTreeMap<String, String>) -> Option<Vapid> {
    let public_b64 = env.get("CP_VAPID_PUBLIC").map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())?;
    let private = decode(env.get("CP_VAPID_PRIVATE")?)?;
    let subject = env.get("CP_VAPID_SUBJECT").map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
        .unwrap_or_else(|| "mailto:push@hangar.dev.br".to_owned());
    Some(Vapid { public: decode(&public_b64)?, public_b64, private, subject })
}

/// Só a chave pública, para o `GET /api/push/vapid` (vazia = push desligado).
pub fn public_key(env: &BTreeMap<String, String>) -> String {
    env.get("CP_VAPID_PUBLIC").map(|v| v.trim().to_owned()).unwrap_or_default()
}

struct Len(usize);
impl hkdf::KeyType for Len { fn len(&self) -> usize { self.0 } }

fn expand(prk: &hkdf::Prk, info: &[u8], len: usize) -> Result<Vec<u8>, &'static str> {
    let mut out = vec![0; len];
    prk.expand(&[info], Len(len)).and_then(|okm| okm.fill(&mut out)).map_err(|_| "webpush_hkdf")?;
    Ok(out)
}

/// Chave e nonce do conteúdo a partir do segredo ECDH (RFC 8291 §3.3–3.4).
fn content_keys(ecdh: &[u8], auth: &[u8], ua_public: &[u8], as_public: &[u8], salt: &[u8]) -> Result<(Vec<u8>, Vec<u8>), &'static str> {
    let prk_key = hkdf::Salt::new(hkdf::HKDF_SHA256, auth).extract(ecdh);
    let info = [b"WebPush: info\0".as_slice(), ua_public, as_public].concat();
    let ikm = expand(&prk_key, &info, 32)?;
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(&ikm);
    Ok((expand(&prk, b"Content-Encoding: aes128gcm\0", 16)?, expand(&prk, b"Content-Encoding: nonce\0", 12)?))
}

/// Corpo cifrado para a inscrição (`keys.p256dh`, `keys.auth`): cabeçalho + um registro só.
pub fn encrypt(ua_public: &[u8], auth: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
    let rng = SystemRandom::new();
    let mine = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).map_err(|_| "webpush_key")?;
    let as_public = mine.compute_public_key().map_err(|_| "webpush_key")?.as_ref().to_vec();
    let ecdh = agreement::agree_ephemeral(mine, &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public), |s| s.to_vec())
        .map_err(|_| "webpush_peer_key")?;
    let mut salt = [0u8; 16];
    ring::rand::SecureRandom::fill(&rng, &mut salt).map_err(|_| "webpush_rng")?;
    let (cek, nonce) = content_keys(&ecdh, auth, ua_public, &as_public, &salt)?;
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| "webpush_cek")?);
    let mut record = [plaintext, &[2u8]].concat();
    key.seal_in_place_append_tag(aead::Nonce::try_assume_unique_for_key(&nonce).map_err(|_| "webpush_nonce")?, aead::Aad::empty(), &mut record)
        .map_err(|_| "webpush_seal")?;
    let mut out = salt.to_vec();
    out.extend(RECORD.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend(&as_public);
    out.extend(record);
    Ok(out)
}

/// JWT ES256 do VAPID para a origem do endpoint.
pub fn jwt(v: &Vapid, audience: &str, exp: u64) -> Result<String, &'static str> {
    let rng = SystemRandom::new();
    let pair = signature::EcdsaKeyPair::from_private_key_and_public_key(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &v.private, &v.public, &rng)
        .map_err(|_| "webpush_vapid_key")?;
    let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = B64.encode(serde_json::json!({"aud": audience, "exp": exp, "sub": v.subject}).to_string());
    let input = format!("{header}.{claims}");
    let sig = pair.sign(&rng, input.as_bytes()).map_err(|_| "webpush_sign")?;
    debug_assert_eq!(pair.public_key().as_ref(), v.public.as_slice());
    Ok(format!("{input}.{}", B64.encode(sig.as_ref())))
}

pub enum Sent { Ok, Gone, Failed(&'static str) }

/// Um envio. 404/410 = inscrição morta (quem chama poda); outro erro mantém a inscrição.
pub async fn send(client: &reqwest::Client, v: &Vapid, subscription: &Value, payload: &[u8]) -> Sent {
    let (Some(endpoint), Some(p256dh), Some(auth)) = (subscription["endpoint"].as_str(),
        subscription["keys"]["p256dh"].as_str().and_then(decode), subscription["keys"]["auth"].as_str().and_then(decode)) else {
        return Sent::Failed("webpush_subscription_invalid");
    };
    let Ok(url) = reqwest::Url::parse(endpoint) else { return Sent::Failed("webpush_endpoint_invalid") };
    let audience = url.origin().ascii_serialization();
    let exp = (chrono::Utc::now() + chrono::Duration::hours(12)).timestamp() as u64;
    let (body, token) = match (encrypt(&p256dh, &auth, payload), jwt(v, &audience, exp)) {
        (Ok(b), Ok(t)) => (b, t),
        (Err(code), _) | (_, Err(code)) => return Sent::Failed(code),
    };
    let reply = client.post(url).header("TTL", "0").header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("Authorization", format!("vapid t={token}, k={}", v.public_b64)).body(body).send().await;
    match reply {
        Ok(r) if r.status().is_success() => Sent::Ok,
        Ok(r) if matches!(r.status().as_u16(), 404 | 410) => Sent::Gone,
        Ok(_) => Sent::Failed("webpush_status"),
        Err(_) => Sent::Failed("webpush_unreachable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O lado do navegador (RFC 8291): decifra com a chave dele para provar o que o servidor montou.
    fn decrypt(ua: agreement::EphemeralPrivateKey, ua_public: &[u8], auth: &[u8], body: &[u8]) -> Vec<u8> {
        let (salt, rest) = body.split_at(16);
        assert_eq!(u32::from_be_bytes(rest[..4].try_into().unwrap()), RECORD);
        let idlen = rest[4] as usize;
        let (as_public, record) = rest[5..].split_at(idlen);
        let ecdh = agreement::agree_ephemeral(ua, &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, as_public), |s| s.to_vec()).unwrap();
        let (cek, nonce) = content_keys(&ecdh, auth, ua_public, as_public, salt).unwrap();
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
        let mut buf = record.to_vec();
        let plain = key.open_in_place(aead::Nonce::try_assume_unique_for_key(&nonce).unwrap(), aead::Aad::empty(), &mut buf).unwrap();
        assert_eq!(plain.last(), Some(&2), "delimitador do último registro");
        plain[..plain.len() - 1].to_vec()
    }

    #[test]
    fn the_browser_side_reads_what_was_encrypted_for_it() {
        let rng = SystemRandom::new();
        let ua = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let ua_public = ua.compute_public_key().unwrap().as_ref().to_vec();
        let auth = [7u8; 16];
        let plain = r#"{"title":"Casa · s","body":"Terminou"}"#.as_bytes();
        let body = encrypt(&ua_public, &auth, plain).unwrap();
        assert_eq!(decrypt(ua, &ua_public, &auth, &body), plain);
    }

    #[test]
    fn vapid_token_is_signed_by_the_configured_key() {
        let rng = SystemRandom::new();
        let pkcs8 = signature::EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = signature::EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
        // O escalar privado fica nos bytes 36..68 do PKCS#8 que o `ring` gera para P-256.
        let private = pkcs8.as_ref()[36..68].to_vec();
        let public = pair.public_key().as_ref().to_vec();
        let env = BTreeMap::from([("CP_VAPID_PUBLIC".to_owned(), B64.encode(&public)), ("CP_VAPID_PRIVATE".to_owned(), B64.encode(&private))]);
        let v = vapid(&env).unwrap();
        let token = jwt(&v, "https://fcm.googleapis.com", 1_900_000_000).unwrap();
        let (input, sig) = token.rsplit_once('.').unwrap();
        signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, &public).verify(input.as_bytes(), &B64.decode(sig).unwrap()).unwrap();
        let claims: Value = serde_json::from_slice(&B64.decode(input.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims, serde_json::json!({"aud": "https://fcm.googleapis.com", "exp": 1_900_000_000u64, "sub": "mailto:push@hangar.dev.br"}));
    }

    #[test]
    fn without_both_keys_web_push_is_off() {
        assert!(vapid(&BTreeMap::from([("CP_VAPID_PUBLIC".to_owned(), "BK".to_owned())])).is_none());
        assert!(vapid(&BTreeMap::new()).is_none());
    }
}
