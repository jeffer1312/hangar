use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use ring::{hmac, rand::{SecureRandom, SystemRandom}};
use serde::{Deserialize, Serialize};
use crate::{AuthAttempt, AuthDecision, AuthError, Origins, Profile, ReceiptContext, AUTH_ABI, AUTH_STORE, MAX_FAILS, MAX_ORIGINS};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    pub abi: u32,
    pub store_version: u32,
    pub mode: String,
    pub state_path: PathBuf,
    pub parent_epoch: String,
    pub expected_token: String,
    pub receipt_key: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Store { version: u32, parent_epoch: String, origins: Origins }

pub struct SharedLimiter { path: PathBuf, epoch: String, expected: Vec<u8>, key: hmac::Key }
impl SharedLimiter {
    pub fn open(config: AuthConfig) -> Result<Self, AuthError> {
        if config.abi != AUTH_ABI || config.store_version != AUTH_STORE { return Err(AuthError::AbiMismatch); }
        if config.parent_epoch.len() != 32 || !config.parent_epoch.bytes().all(|b| b.is_ascii_hexdigit())
            || !config.state_path.is_absolute() || !matches!(config.mode.as_str(), "create" | "attach") {
            return Err(AuthError::InvalidInput);
        }
        let key = unhex(&config.receipt_key).ok_or(AuthError::InvalidInput)?;
        if key.len() != 32 { return Err(AuthError::InvalidInput); }
        let context = Self { path: config.state_path, epoch: config.parent_epoch,
            expected: config.expected_token.into_bytes(), key: hmac::Key::new(hmac::HMAC_SHA256, &key) };
        let _guard = context.lock()?;
        if config.mode == "create" {
            if context.path.exists() { return Err(AuthError::InvalidInput); }
            context.publish(&Store { version: AUTH_STORE, parent_epoch: context.epoch.clone(), origins: Origins::default() })?;
        } else { context.read()?; }
        Ok(context)
    }
    fn lock(&self) -> Result<File, AuthError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let file = options.open(self.path.with_extension("lock")).map_err(|_| AuthError::StoreUnavailable)?;
        file.lock().map_err(|_| AuthError::StoreUnavailable)?;
        Ok(file)
    }
    fn read(&self) -> Result<Store, AuthError> {
        if fs::metadata(&self.path).map_err(|_| AuthError::StoreUnavailable)?.len() > (1 << 20) {
            return Err(AuthError::InvalidInput);
        }
        let raw = fs::read(&self.path).map_err(|_| AuthError::StoreUnavailable)?;
        let store: Store = serde_json::from_slice(&raw).map_err(|_| AuthError::InvalidInput)?;
        if store.version != AUTH_STORE { return Err(AuthError::AbiMismatch); }
        if store.parent_epoch != self.epoch { return Err(AuthError::EpochMismatch); }
        if store.origins.0.len() > MAX_ORIGINS || store.origins.0.iter().enumerate().any(|(i, (origin, hits))|
            hits.len() > MAX_FAILS || hits.iter().any(|t| !t.is_finite())
            || store.origins.0[..i].iter().any(|(other, _)| other == origin)) { return Err(AuthError::InvalidInput); }
        Ok(store)
    }
    fn publish(&self, store: &Store) -> Result<(), AuthError> {
        let parent = self.path.parent().ok_or(AuthError::InvalidInput)?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| AuthError::StoreUnavailable)?;
        let raw = serde_json::to_vec(store).map_err(|_| AuthError::InvalidInput)?;
        file.write_all(&raw).map_err(|_| AuthError::StoreUnavailable)?;
        file.persist(&self.path).map_err(|_| AuthError::StoreUnavailable)?;
        Ok(())
    }
    pub fn transaction<T>(&self, op: impl FnOnce(&mut Origins) -> T) -> Result<T, AuthError> {
        let _guard = self.lock()?;
        let mut store = self.read()?;
        let previous = store.origins.clone();
        let result = op(&mut store.origins);
        if store.origins != previous { self.publish(&store)?; }
        Ok(result)
    }
    pub fn evaluate(&self, attempt: &AuthAttempt) -> Result<AuthDecision, AuthError> {
        self.transaction(|origins| Ok::<_, AuthError>(origins.evaluate(&self.expected, attempt, now()?)))?
    }
    pub fn evaluate_at(&self, attempt: &AuthAttempt, now: f64) -> Result<AuthDecision, AuthError> {
        if !now.is_finite() { return Err(AuthError::InvalidInput); }
        self.transaction(|origins| origins.evaluate(&self.expected, attempt, now))
    }
    pub fn token_matches(&self, token: Option<&[u8]>) -> bool {
        use subtle::ConstantTimeEq;
        token.is_some_and(|t| bool::from(t.ct_eq(self.expected.as_slice())))
    }
    pub fn receipt(&self, context: &ReceiptContext) -> Result<String, AuthError> {
        let mut nonce = [0u8; 16];
        SystemRandom::new().fill(&mut nonce).map_err(|_| AuthError::StoreUnavailable)?;
        let payload = Receipt { version: AUTH_ABI, parent_epoch: self.epoch.clone(), nonce: hex(&nonce), profile: context.profile };
        let payload = serde_json::to_vec(&payload).map_err(|_| AuthError::InvalidInput)?;
        let mac = hmac::sign(&self.key, &receipt_message(&payload, context)?);
        Ok(format!("{}.{}", hex(&payload), hex(mac.as_ref())))
    }
    pub fn verify_receipt(&self, context: &ReceiptContext, receipt: &[u8]) -> Result<AuthDecision, AuthError> {
        // Uma porta pública nunca adquire a confiança do listener privado por cabeçalho.
        if context.connection_kind != "private_upstream" { return Ok(AuthDecision::UNAUTHORIZED); }
        let Ok(receipt) = std::str::from_utf8(receipt) else { return Ok(AuthDecision::UNAUTHORIZED) };
        let Some((payload, signature)) = receipt.split_once('.') else { return Ok(AuthDecision::UNAUTHORIZED) };
        let (Some(payload), Some(signature)) = (unhex(payload), unhex(signature)) else { return Ok(AuthDecision::UNAUTHORIZED) };
        let Ok(value) = serde_json::from_slice::<Receipt>(&payload) else { return Ok(AuthDecision::UNAUTHORIZED) };
        if value.version != AUTH_ABI || value.parent_epoch != self.epoch || value.profile != context.profile {
            return Ok(AuthDecision::UNAUTHORIZED);
        }
        if hmac::verify(&self.key, &receipt_message(&payload, context)?, &signature).is_err() { return Ok(AuthDecision::UNAUTHORIZED); }
        // Também valida a época do arquivo: um recibo não esconde uma autoridade quebrada.
        self.transaction(|_| ())?;
        Ok(AuthDecision::OWNER)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt { version: u32, parent_epoch: String, nonce: String, profile: Profile }
fn receipt_message(payload: &[u8], context: &ReceiptContext) -> Result<Vec<u8>, AuthError> {
    serde_json::to_vec(&(payload, context)).map_err(|_| AuthError::InvalidInput)
}
pub fn now() -> Result<f64, AuthError> {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|t| t.as_secs_f64()).map_err(|_| AuthError::StoreUnavailable)
}
pub fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }
fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() || s.len() % 2 != 0 { return None; }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i+2], 16).ok()).collect()
}
