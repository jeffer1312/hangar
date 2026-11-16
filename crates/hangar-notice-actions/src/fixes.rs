//! Consertos em andamento: qual sessão de conserto saiu de qual aviso e de qual conversa
//! (`~/.hangar/notice-fixes.json`). A sessão nasce solta, sem grupo; é por aqui que a volta acha a origem.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_NOTICE_ID: usize = 200;
const MAX_SLUG: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    pub parent: String,
    pub notice_id: String,
    /// Milissegundos da abertura: cada tentativa tem as próprias notas na conversa.
    pub attempt: u64,
    pub subject: String,
}

/// Por nome da sessão de conserto.
pub type Fixes = BTreeMap<String, Fix>;

pub fn path(hangar_dir: &Path) -> PathBuf {
    hangar_dir.join("notice-fixes.json")
}

/// Ausente é vazio; ilegível é erro, nunca vazio: a próxima gravação apagaria os consertos em andamento.
pub fn load(hangar_dir: &Path) -> io::Result<Fixes> {
    match hangar_core::fs::read_replaced(&path(hangar_dir)) {
        Ok(raw) => serde_json::from_slice(&raw).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Fixes::new()),
        Err(error) => Err(error),
    }
}

pub fn save(hangar_dir: &Path, fixes: &Fixes) -> io::Result<()> {
    std::fs::create_dir_all(hangar_dir)?;
    hangar_core::fs::atomic_write(&path(hangar_dir), &serde_json::to_vec_pretty(fixes).map_err(io::Error::other)?)
}

/// O conserto vivo desse aviso, se houver.
pub fn running<'a>(fixes: &'a Fixes, parent: &str, notice_id: &str) -> Option<&'a str> {
    fixes.iter().find(|(_, fix)| fix.parent == parent && fix.notice_id == notice_id).map(|(child, _)| child.as_str())
}

/// O id vem do cliente e vira parte de id de nota na fila.
pub fn valid_notice_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_NOTICE_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || b":._-".contains(&b))
}

/// `conserto-<hook>`; com o nome em uso, `-2` até `-9`.
pub fn child_name(subject: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    let mut slug = String::new();
    for c in subject.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() { slug.push(c) } else if !slug.ends_with('-') { slug.push('-') }
    }
    let slug: String = slug.trim_matches('-').chars().take(MAX_SLUG).collect();
    let base = if slug.is_empty() { "conserto".to_owned() } else { format!("conserto-{}", slug.trim_end_matches('-')) };
    std::iter::once(base.clone()).chain((2..=9).map(|n| format!("{base}-{n}"))).find(|name| !taken(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(parent: &str, notice: &str) -> Fix {
        Fix { parent: parent.into(), notice_id: notice.into(), attempt: 1, subject: "Stop".into() }
    }

    #[test]
    fn registry_round_trips_and_missing_is_empty() {
        let dir = std::env::temp_dir().join(format!("notice-fixes-{}", std::process::id()));
        assert_eq!(load(&dir).unwrap(), Fixes::new());
        let fixes = Fixes::from([("conserto-stop".to_owned(), fix("origem", "hookfail:a"))]);
        save(&dir, &fixes).unwrap();
        assert_eq!(load(&dir).unwrap(), fixes);
        std::fs::write(path(&dir), "{ quebrado").unwrap();
        assert!(load(&dir).is_err(), "ilegível não vira vazio");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn running_is_per_parent_and_notice() {
        let fixes = Fixes::from([("c".to_owned(), fix("origem", "hookfail:a"))]);
        assert_eq!(running(&fixes, "origem", "hookfail:a"), Some("c"));
        assert_eq!(running(&fixes, "outra", "hookfail:a"), None);
        assert_eq!(running(&fixes, "origem", "hookfail:b"), None);
    }

    #[test]
    fn notice_id_is_checked() {
        assert!(valid_notice_id("hookfail:00000000-0000-4000-8000-000000000002"));
        assert!(valid_notice_id("local-hookfail:1:0"));
        assert!(!valid_notice_id(""));
        assert!(!valid_notice_id("a b"));
        assert!(!valid_notice_id(&"a".repeat(201)));
    }

    #[test]
    fn child_name_is_a_slug_with_a_free_suffix() {
        assert_eq!(child_name("PostToolUse:Bash", |_| false).as_deref(), Some("conserto-posttooluse-bash"));
        assert_eq!(child_name("PostToolUse:Bash", |name| name == "conserto-posttooluse-bash").as_deref(), Some("conserto-posttooluse-bash-2"));
        assert_eq!(child_name("::", |_| false).as_deref(), Some("conserto"));
        assert_eq!(child_name("Stop", |_| true), None);
        assert!(child_name(&"x".repeat(100), |_| false).unwrap().len() <= "conserto-".len() + 40);
    }
}
