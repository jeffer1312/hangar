use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardMode {
    Shared,
    Exclusive,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountKey {
    pub provider: Provider,
    pub canonical_home: PathBuf,
}

pub fn normalize_windows_path(value: &str) -> String {
    let value = value.replace('\\', "/");
    let value = if value
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("//?/UNC/"))
    {
        format!("//{}", &value[8..])
    } else {
        value.strip_prefix("//?/").unwrap_or(&value).to_owned()
    };
    value
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .trim_end_matches('/')
        .to_owned()
}

fn resolve_missing(path: &Path) -> io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or(error)?;
            let base = resolve_missing(parent)?;
            if path.ends_with("..") {
                return Ok(base.parent().unwrap_or(&base).to_owned());
            }
            Ok(base.join(
                path.file_name()
                    .ok_or_else(|| io::Error::other("caminho sem nome"))?,
            ))
        }
        Err(error) => Err(error),
    }
}

impl AccountKey {
    pub fn new(provider: Provider, home: &Path) -> io::Result<Self> {
        let absolute = if home.is_absolute() {
            home.to_owned()
        } else {
            std::env::current_dir()?.join(home)
        };
        let canonical = resolve_missing(&absolute)?;
        let text = canonical
            .to_str()
            .ok_or_else(|| io::Error::other("caminho sem UTF-8"))?;
        let canonical_home = if cfg!(windows) {
            normalize_windows_path(text).into()
        } else {
            canonical
        };
        Ok(Self {
            provider,
            canonical_home,
        })
    }

    pub fn digest(&self) -> io::Result<String> {
        let text = self
            .canonical_home
            .to_str()
            .ok_or_else(|| io::Error::other("caminho sem UTF-8"))?;
        let canonical = if cfg!(windows) {
            normalize_windows_path(text)
        } else {
            text.to_owned()
        };
        Ok(key_digest(self.provider, &canonical))
    }
}

pub fn key_digest(provider: Provider, canonical: &str) -> String {
    let bytes = format!("{}\0{}", provider.as_str(), canonical);
    ring::digest::digest(&ring::digest::SHA256, bytes.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageFacts {
    pub complete: bool,
    pub sessions: Vec<String>,
    pub pids: Vec<u32>,
    /// Processos que só herdaram a variável da conta: não a usam, mas impedem apagá-la.
    #[serde(default)]
    pub holders: Vec<u32>,
}

impl UsageFacts {
    pub fn ensure_unused(&self) -> Result<(), &'static str> {
        if !self.complete {
            Err("account_usage_unknown")
        } else if !self.sessions.is_empty() || !self.pids.is_empty() {
            Err("account_in_use")
        } else {
            Ok(())
        }
    }

    pub fn merge(&mut self, other: Self) {
        self.complete &= other.complete;
        self.sessions.extend(other.sessions);
        self.pids.extend(other.pids);
        self.holders.extend(other.holders);
        self.sessions.sort();
        self.sessions.dedup();
        self.pids.sort();
        self.pids.dedup();
        self.holders.sort();
        self.holders.dedup();
    }
}
