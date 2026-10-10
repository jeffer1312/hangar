//! Plano da conversa por voz em modo planejar: um .md por sessão, em `~/.hangar/voz/planos`.
use chrono::{DateTime, Local};
use std::{io, path::PathBuf};

const LIMIT: usize = 200_000;

pub struct PlanFile { pub path: PathBuf }

fn voice_dir() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    home.join(".hangar").join("voz")
}

pub fn plans_dir() -> PathBuf { voice_dir().join("planos") }

/// Pasta própria do organizador: a única que ele grava (é o cwd da thread em `workspace-write`).
pub fn files_dir() -> PathBuf { voice_dir().join("arquivos") }

pub fn new_plan(session: &str, now: DateTime<Local>) -> PlanFile { new_plan_in(&plans_dir(), session, now) }

// Nome livre: plano já entregue ou de outra chamada no mesmo minuto não é sobrescrito.
fn new_plan_in(dir: &std::path::Path, session: &str, now: DateTime<Local>) -> PlanFile {
    let safe: String = session.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    let stem = format!("{safe}-{}", now.format("%Y-%m-%d-%H%M"));
    let mut path = dir.join(format!("{stem}.md"));
    let mut n = 2;
    while path.exists() { path = dir.join(format!("{stem}-{n}.md")); n += 1; }
    PlanFile { path }
}

impl PlanFile {
    /// Arquivo ainda não escrito é plano vazio; qualquer outra falha de leitura sobe.
    pub fn read(&self) -> io::Result<String> {
        match std::fs::read_to_string(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            other => other,
        }
    }

    pub fn write(&self, markdown: &str) -> io::Result<()> {
        if markdown.len() > LIMIT { return Err(io::Error::new(io::ErrorKind::InvalidInput, "plano acima de 200 000 bytes")); }
        if let Some(dir) = self.path.parent() { std::fs::create_dir_all(dir)?; }
        // Tmp na mesma pasta (rename entre pastas não é atômico): o plano não fica pela metade se o app fechar.
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, markdown)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use core::prelude::v1::test;

    #[test]
    fn file_name_is_sanitized_and_dated() {
        let when = chrono::Local.with_ymd_and_hms(2026, 10, 7, 19, 5, 0).unwrap();
        let plan = new_plan("pm/../x y", when);
        assert_eq!(plan.path.file_name().unwrap().to_str().unwrap(), "pm-..-x-y-2026-10-07-1905.md");
    }

    #[test]
    fn taken_name_gets_a_suffix() {
        let dir = std::env::temp_dir().join(format!("voice-plan-name-{}", std::process::id()));
        let when = chrono::Local.with_ymd_and_hms(2026, 10, 7, 19, 5, 0).unwrap();
        let name = |p: &PlanFile| p.path.file_name().unwrap().to_str().unwrap().to_owned();
        let first = new_plan_in(&dir, "s", when);
        assert_eq!(name(&first), "s-2026-10-07-1905.md");
        first.write("a").unwrap();
        let second = new_plan_in(&dir, "s", when);
        assert_eq!(name(&second), "s-2026-10-07-1905-2.md");
        second.write("b").unwrap();
        assert_eq!(name(&new_plan_in(&dir, "s", when)), "s-2026-10-07-1905-3.md");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_error_other_than_missing_is_not_empty() {
        // Um diretório no lugar do arquivo: existe, mas não é legível como texto.
        let dir = std::env::temp_dir().join(format!("voice-plan-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(PlanFile { path: dir.clone() }.read().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_is_atomic_and_readable() {
        let dir = std::env::temp_dir().join(format!("voice-plan-{}", std::process::id()));
        let plan = PlanFile { path: dir.join("p.md") };
        assert_eq!(plan.read().unwrap(), "");
        plan.write("# Plano\n- item").unwrap();
        assert_eq!(plan.read().unwrap(), "# Plano\n- item");
        assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
        assert!(plan.write(&"x".repeat(200_001)).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
