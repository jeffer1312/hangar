//! Configuração vinda do ambiente que o Python monta ao subir o filho.
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::auth::TrustedHosts;

#[derive(Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub upstream: SocketAddr,
    pub internal_secret: String,
    pub auth_token: String,
    pub log_path: Option<PathBuf>,
    pub trusted: TrustedHosts,
}

/// Manual para o token e o segredo nunca saírem num `{:?}` de log ou de pânico.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("listen", &self.listen)
            .field("upstream", &self.upstream)
            .field("internal_secret", &"<oculto>")
            .field("auth_token", &"<oculto>")
            .field("log_path", &self.log_path)
            .field("trusted", &self.trusted)
            .finish()
    }
}

impl Config {
    pub fn runtime_instance() -> Result<Option<String>, String> {
        let value = std::env::var("HANGAR_RUNTIME_INSTANCE").ok();
        if value.as_ref().is_some_and(|value|value.is_empty() || value.len() > 128
            || !value.bytes().all(|byte|byte.is_ascii_alphanumeric() || matches!(byte,b'_' | b'-'))) {
            return Err("identidade privada do runtime inválida".into());
        }
        Ok(value)
    }

    pub fn from_env() -> Result<Config, String> {
        if let Some(raw) = std::env::var_os("HANGAR_AUTH_CONTEXT") {
            let raw = raw.to_str().ok_or_else(|| "auth_store_invalid".to_owned())?;
            let config = serde_json::from_str::<hangar_auth::AuthConfig>(raw).map_err(|_| "auth_store_invalid".to_owned())?;
            if config.mode != "attach" { return Err("auth_store_invalid".into()); }
            if std::env::var("CP_AUTH_TOKEN").ok().as_deref() != Some(config.expected_token.as_str()) {
                return Err("auth_context_changed".into());
            }
            hangar_auth::SharedLimiter::open(config).map_err(|e| e.to_string())?;
        }
        Config::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
        let required = |k: &str| get(k).filter(|v| !v.is_empty()).ok_or_else(|| format!("{k} ausente"));
        let addr = |k: &str| -> Result<SocketAddr, String> {
            let v = required(k)?;
            v.parse().map_err(|_| format!("{k} inválido: {v}"))
        };
        Ok(Config {
            listen: addr("HANGAR_SERVER_LISTEN")?,
            upstream: addr("HANGAR_SERVER_UPSTREAM")?,
            internal_secret: required("HANGAR_INTERNAL_SECRET")?,
            auth_token: required("CP_AUTH_TOKEN")?,
            log_path: get("HANGAR_SERVER_LOG").filter(|v| !v.is_empty()).map(PathBuf::from),
            // Mesmo padrão do Python: só um proxy desta máquina reescreve o cliente.
            trusted: TrustedHosts::parse(
                &get("CP_FORWARDED_ALLOW_IPS").unwrap_or_else(|| "127.0.0.1".into()),
            ),
        })
    }
}

/// Porta do convidado e convites. Fora do `Config` porque nenhum teste do dono precisa delas.
#[derive(Clone, Debug, Default)]
pub struct GuestEnv {
    /// Ausente = sem porta do convidado.
    pub listen: Option<SocketAddr>,
    /// `shares.json` do Python; ausente = sem convites no Rust.
    pub shares_file: Option<PathBuf>,
}

impl GuestEnv {
    pub fn from_env() -> Result<GuestEnv, String> {
        GuestEnv::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<GuestEnv, String> {
        let var = |k: &str| get(k).filter(|v| !v.is_empty());
        let listen = match var("HANGAR_SERVER_GUEST_LISTEN") {
            Some(v) => Some(v.parse().map_err(|_| format!("HANGAR_SERVER_GUEST_LISTEN inválido: {v}"))?),
            None => None,
        };
        Ok(GuestEnv { listen, shares_file: var("HANGAR_SHARES_FILE").map(PathBuf::from) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    const BASE: [(&str, &str); 4] = [
        ("HANGAR_SERVER_LISTEN", "0.0.0.0:8765"),
        ("HANGAR_SERVER_UPSTREAM", "127.0.0.1:41234"),
        ("HANGAR_INTERNAL_SECRET", "ab12"),
        ("CP_AUTH_TOKEN", "tok"),
    ];

    #[test]
    fn reads_env_with_defaults() {
        let cfg = Config::from_lookup(env(&BASE)).unwrap();
        assert_eq!(cfg.listen, "0.0.0.0:8765".parse().unwrap());
        assert_eq!(cfg.upstream, "127.0.0.1:41234".parse().unwrap());
        assert_eq!(cfg.log_path, None);
        assert!(cfg.trusted.contains("127.0.0.1"));
        assert!(!cfg.trusted.contains("192.0.2.1"));
    }

    #[test]
    fn debug_hides_token_and_secret() {
        let shown = format!("{:?}", Config::from_lookup(env(&BASE)).unwrap());
        assert!(!shown.contains("ab12") && !shown.contains("\"tok\""), "{shown}");
        assert!(shown.contains("41234"));
    }

    #[test]
    fn missing_or_bad_values_are_errors() {
        let sem_segredo: Vec<_> = BASE.iter().copied().filter(|(k, _)| *k != "HANGAR_INTERNAL_SECRET").collect();
        assert!(Config::from_lookup(env(&sem_segredo)).unwrap_err().contains("HANGAR_INTERNAL_SECRET"));
        let mut torto = BASE.to_vec();
        torto[0] = ("HANGAR_SERVER_LISTEN", "nao-e-endereco");
        assert!(Config::from_lookup(env(&torto)).unwrap_err().contains("HANGAR_SERVER_LISTEN"));
    }
}
