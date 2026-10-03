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
    pub fn from_env() -> Result<Config, String> {
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
