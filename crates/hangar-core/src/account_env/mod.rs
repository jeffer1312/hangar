//! Contexto capturado uma vez; conta adicional não herda identidade da principal.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

mod omp;
pub use omp::{OmpDirectories, omp_directories, valid_omp_profile};

/// Pasta de um usuário nomeado, consultada pelo mesmo serviço de nomes do sistema.
#[cfg(unix)]
#[allow(unsafe_code)] // A ABI de nomes não tem equivalente seguro para outro usuário no std.
pub fn named_user_home(user: &str) -> Option<PathBuf> {
    use std::{ffi::{CStr, CString, OsStr}, os::unix::ffi::OsStrExt};
    let name = CString::new(user).ok()?;
    let mut size = 1024;
    loop {
        let mut storage = vec![0u8; size];
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // Os buffers pertencem à chamada e só são lidos após a confirmação do sistema.
        let code = unsafe { libc::getpwnam_r(name.as_ptr(), record.as_mut_ptr(), storage.as_mut_ptr().cast(), storage.len(), &mut result) };
        if code == libc::ERANGE { size = size.checked_mul(2)?; continue; }
        if code != 0 || result.is_null() { return None; }
        let record = unsafe { record.assume_init() };
        if record.pw_dir.is_null() { return None; }
        return Some(PathBuf::from(OsStr::from_bytes(unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes())));
    }
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: String,
    pub home: PathBuf,
    pub is_default: bool,
}

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

    /// `default_claude_json` pelo ambiente capturado.
    pub fn default_claude_json(&self) -> PathBuf {
        default_claude_json(&self.home, self.base.get("CLAUDE_CONFIG_DIR").map(String::as_str))
    }

    /// Raízes do omp resolvidas só pelo home e pelo ambiente capturados, sem criar nada.
    pub fn omp_directories(&self) -> Result<OmpDirectories, &'static str> {
        omp_directories(&self.home, &self.base)
    }

    pub fn codex(&self, account: &Account) -> BTreeMap<String, String> {
        static AUTH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            // `.*`: o sufixo logo depois do prefixo (`CHATGPT_ACCOUNT_ID`) também é identidade da principal.
            regex::Regex::new(r"^(?:OPENAI|CODEX|CHATGPT)_.*(?:(?:KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL)(?:_FILE)?|ACCOUNT_ID|ORG_ID|PROJECT_ID)$").unwrap()
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

/// Marcador de pasta de conta Claude criada pelo Hangar (`~/.claude-<nome>`).
pub const CLAUDE_ACCOUNT_MARKER: &str = ".hangar-conta";

/// O `.claude.json` que a conta padrão do Claude lê. Com `CLAUDE_CONFIG_DIR` no ambiente, o Hangar a
/// exporta às sessões da conta padrão (`config_dir_to_export`) e o CLI lê o arquivo de dentro da pasta,
/// mesmo quando ela é o `~/.claude` e mesmo antes de ele existir (o CLI o cria vazio); sem a variável,
/// o do HOME. Pasta de conta do Hangar nunca é a padrão: é o que o shell de uma sessão dela exporta.
pub fn default_claude_json(home: &Path, config_dir: Option<&str>) -> PathBuf {
    let main = home.join(".claude.json");
    let Some(dir) = config_dir.filter(|dir| !dir.is_empty()).map(|dir| expand(dir, home)) else { return main };
    // Marcador que é link ou pasta não faz conta, como no catálogo (`storage::real_file`).
    let marked = std::fs::symlink_metadata(dir.join(CLAUDE_ACCOUNT_MARKER)).is_ok_and(|m| m.is_file());
    if marked { main } else { dir.join(".claude.json") }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(pairs: &[(&str, &str)]) -> AccountEnvironment {
        AccountEnvironment::from_pairs(pairs.iter().map(|&(k, v)| (k, v)))
    }

    /// Com `CLAUDE_CONFIG_DIR`, mesmo apontando para o `~/.claude`, a conta padrão lê o arquivo de dentro
    /// da pasta, mesmo antes de ele existir; vazia e pasta de conta do Hangar valem o do HOME.
    #[test]
    fn default_claude_json_follows_the_config_dir() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let h = home.to_string_lossy().into_owned();
        let read = |config_dir: Option<&str>| {
            let mut pairs = vec![("HOME", h.as_str())];
            pairs.extend(config_dir.map(|dir| ("CLAUDE_CONFIG_DIR", dir)));
            environment(&pairs).default_claude_json()
        };
        let main = home.join(".claude.json");
        for dir in [".claude", "outra", ".claude-conta"] {
            std::fs::create_dir_all(home.join(dir)).unwrap();
        }
        assert_eq!(read(None), main);
        assert_eq!(read(Some("")), main);
        assert_eq!(read(Some(&format!("{h}/.claude"))), home.join(".claude/.claude.json"), "antes de o CLI criar o arquivo");
        assert_eq!(read(Some("~/outra")), home.join("outra/.claude.json"));
        std::fs::write(home.join(".claude-conta").join(CLAUDE_ACCOUNT_MARKER), "").unwrap();
        assert_eq!(read(Some(&format!("{h}/.claude-conta"))), main, "a pasta de uma conta do Hangar não é a padrão");
    }

    /// A conta adicional não leva identidade nenhuma da principal. O lançador Python deixava passar
    /// `CHATGPT_ACCOUNT_ID`, `OPENAI_ACCOUNT_ID`, `CODEX_ACCOUNT_ID` e `CODEX_PROJECT_ID` (a regra
    /// pedia um caractere entre o prefixo e o sufixo).
    #[test]
    fn an_additional_account_drops_every_identity_of_the_main_one() {
        let inherited = [("HOME", "/h"), ("USERPROFILE", "/h"), ("PATH", "/bin"), ("CHATGPT_ACCOUNT_ID", "acc"),
            ("OPENAI_ACCOUNT_ID", "acc2"), ("CODEX_ACCOUNT_ID", "acc3"), ("CODEX_PROJECT_ID", "proj"), ("OPENAI_API_KEY", "sk"),
            ("CODEX_API_KEY", "k"), ("OPENAI_ORG_ID", "org"), ("CHATGPT_SESSION_TOKEN", "t"), ("OPENAI_BASE_URL", "http://proxy"),
            ("CODEX_SQLITE_HOME", "/x"), ("CODEX_HOME", "/velho"), ("XDG_CONFIG_HOME", "/c"), ("XDG_RUNTIME_DIR", "/run/user/1"),
            ("CP_AUTH_TOKEN", "segredo"), ("TMUX_PANE", "%7")];
        let env = environment(&inherited);
        let extra = env.codex(&Account { id: "x".into(), home: "/h/.codex-x".into(), is_default: false });
        for gone in ["CHATGPT_ACCOUNT_ID", "OPENAI_ACCOUNT_ID", "CODEX_ACCOUNT_ID", "CODEX_PROJECT_ID", "OPENAI_API_KEY", "CODEX_API_KEY",
            "OPENAI_ORG_ID", "CHATGPT_SESSION_TOKEN", "OPENAI_BASE_URL", "CODEX_SQLITE_HOME", "XDG_CONFIG_HOME", "CP_AUTH_TOKEN"] {
            assert!(!extra.contains_key(gone), "{gone} vazou para a conta adicional");
        }
        assert_eq!((extra["HOME"].as_str(), extra["USERPROFILE"].as_str(), extra["CODEX_HOME"].as_str()), ("/h", "/h", "/h/.codex-x"));
        for kept in ["PATH", "XDG_RUNTIME_DIR", "TMUX_PANE"] {
            assert!(extra.contains_key(kept), "{kept}");
        }
        // A padrão herda a identidade do servidor; só a autoridade dele não passa.
        let default = env.codex(&Account { id: "default".into(), home: "/h/.codex".into(), is_default: true });
        assert_eq!(default["CHATGPT_ACCOUNT_ID"], "acc");
        assert_eq!(default["CODEX_HOME"], "/h/.codex");
        assert!(!default.contains_key("CP_AUTH_TOKEN"));
    }
}
