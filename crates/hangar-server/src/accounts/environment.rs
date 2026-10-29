//! Contexto capturado uma vez; conta adicional não herda identidade da principal.
use super::catalog::Account;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Só para teste: chamado com o caminho logo antes da leitura real de um destino Pi/omp.
#[doc(hidden)]
pub type SecondaryBarrier = Arc<dyn Fn(&Path) + Send + Sync>;

/// Só para teste: saída de `list-sessions` no lugar do multiplexador real (`Err` = sem resposta).
#[doc(hidden)]
pub type TerminalFixture = Arc<dyn Fn() -> Result<String, ()> + Send + Sync>;

#[derive(Clone)]
pub struct AccountEnvironment {
    pub home: PathBuf,
    pub claude_base: PathBuf,
    pub codex_default: PathBuf,
    pub claude_fixed: String,
    pub base: BTreeMap<String, String>,
    original_order: Vec<String>,
    #[doc(hidden)]
    pub secondary_barrier: Option<SecondaryBarrier>,
    #[doc(hidden)]
    pub terminal_fixture: Option<TerminalFixture>,
}

impl AccountEnvironment {
    pub fn capture() -> Self {
        Self::from_pairs(std::env::vars())
    }
    pub fn from_map(base: BTreeMap<String, String>) -> Self { Self::from_pairs(base) }
    pub fn from_pairs<K:Into<String>,V:Into<String>>(pairs:impl IntoIterator<Item=(K,V)>)->Self {
        let mut base=BTreeMap::new();let mut original_order=Vec::new();
        for (name,value) in pairs {
            let name=name.into();
            if name=="HANGAR_AUTH_CONTEXT"{continue;}
            if !base.contains_key(&name){original_order.push(name.clone());}
            base.insert(name,value.into());
        }
        let home = PathBuf::from(
            base.get(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .or_else(|| base.get("HOME"))
                .cloned()
                .unwrap_or_default(),
        );
        let path = |name: &str, fallback: &str| {
            base.get(name)
                .filter(|v| !v.is_empty())
                .map(|v| expand(v, &home))
                .unwrap_or_else(|| home.join(fallback))
        };
        Self {
            claude_base: path("CLAUDE_CONFIG_DIR", ".claude"),
            codex_default: path("CODEX_HOME", ".codex"),
            claude_fixed: base
                .get("CP_CLAUDE_CONFIG_DIRS")
                .cloned()
                .unwrap_or_default(),
            home,
            base,
            original_order,
            secondary_barrier: None,
            terminal_fixture: None,
        }
    }

    /// Valores atuais, na ordem capturada; inclusões posteriores seguem a ordem do mapa.
    pub fn ordered_pairs(&self)->Vec<(String,String)> {
        self.original_order.iter().filter_map(|name|self.base.get(name).map(|value|(name.clone(),value.clone())))
            .chain(self.base.iter().filter(|(name,_)| !self.original_order.contains(name)).map(|(name,value)|(name.clone(),value.clone())))
            .filter(|(name,_)|name!="HANGAR_AUTH_CONTEXT").collect()
    }

    /// Raízes do omp resolvidas só pelo home e pelo ambiente capturados, sem criar nada.
    pub fn omp_directories(&self) -> Result<super::secondary_auth::OmpDirectories, &'static str> {
        super::secondary_auth::omp_directories(&self.home, &self.base)
    }

    pub fn codex(&self, account: &Account) -> BTreeMap<String, String> {
        static AUTH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"^(?:OPENAI|CODEX|CHATGPT)_.+(?:(?:KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL)(?:_FILE)?|ACCOUNT_ID|ORG_ID|PROJECT_ID)$").unwrap()
        });
        let mut env = self.base.clone();
        env.retain(|key, _| {
            let key = key.to_ascii_uppercase();
            !crate::terminal_process::PRIVATE_ENV_KEYS.contains(&key.as_str())
        });
        if !account.is_default {
            env.retain(|key, _| {
                let key = key.to_ascii_uppercase();
                !AUTH.is_match(&key)
                    && !matches!(
                        key.as_str(),
                        "OPENAI_BASE_URL"
                            | "OPENAI_API_BASE"
                            | "OPENAI_ORGANIZATION"
                            | "OPENAI_ORG_ID"
                            | "OPENAI_PROJECT"
                            | "OPENAI_PROJECT_ID"
                            | "OPENAI_ENDPOINT"
                            | "CODEX_HOME"
                            | "CODEX_CONFIG_HOME"
                            | "CODEX_SQLITE_HOME"
                            | "HOME"
                            | "USERPROFILE"
                            | "XDG_CONFIG_HOME"
                            | "XDG_DATA_HOME"
                            | "XDG_STATE_HOME"
                            | "XDG_CACHE_HOME"
                    )
            });
        }
        env.insert("HOME".into(), self.home.to_string_lossy().into());
        env.insert("USERPROFILE".into(), self.home.to_string_lossy().into());
        env.insert("CODEX_HOME".into(), account.home.to_string_lossy().into());
        env
    }
}

pub fn expand(value: &str, home: &Path) -> PathBuf {
    if value == "~" {
        home.into()
    } else if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        home.join(rest)
    } else {
        value.into()
    }
}
