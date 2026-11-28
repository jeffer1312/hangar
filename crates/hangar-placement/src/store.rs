//! `~/.hangar/placement.json`: ausente vale o padrão; ilegível é erro, nunca o padrão calado, para a
//! próxima gravação não apagar as regras do dono sem ele ver.

use std::io;
use std::path::{Path, PathBuf};

use chrono_tz::Tz;

use crate::rules::{hhmm, PlacementConfig};

pub fn path(home: &Path) -> PathBuf {
    home.join(".hangar").join("placement.json")
}

pub fn load(home: &Path) -> io::Result<PlacementConfig> {
    match hangar_core::fs::read_replaced(&path(home)) {
        Ok(raw) => serde_json::from_slice(&raw).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(PlacementConfig::default()),
        Err(error) => Err(error),
    }
}

/// Grava inteiro, sem ler antes: é a gravação explícita que recupera um arquivo ilegível.
pub fn save(home: &Path, config: &PlacementConfig) -> Result<(), SaveError> {
    validate(config).map_err(SaveError::Invalid)?;
    let path = path(home);
    let parent = path.parent().ok_or_else(|| SaveError::Io(io::Error::other("sem pasta")))?;
    std::fs::create_dir_all(parent).map_err(SaveError::Io)?;
    let bytes = serde_json::to_vec_pretty(config).map_err(|e| SaveError::Io(io::Error::other(e)))?;
    hangar_core::fs::atomic_write(&path, &bytes).map_err(SaveError::Io)
}

#[derive(Debug)]
pub enum SaveError {
    Invalid(Invalid),
    Io(io::Error),
}

/// O campo recusado, como `schedule_rules[0].tz`.
#[derive(Debug, PartialEq, Eq)]
pub struct Invalid {
    pub field: String,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "campo inválido: {}", self.field)
    }
}

/// Recusa o que a escolha ignoraria calada (`ScheduleRule::covers` não vale com fuso ou hora ilegível).
pub fn validate(config: &PlacementConfig) -> Result<(), Invalid> {
    let bad = |field: String| Err(Invalid { field });
    let limits = &config.limits;
    if limits.mem_free_pct > 100 {
        return bad("limits.mem_free_pct".into());
    }
    if limits.cpu_pct > 100 {
        return bad("limits.cpu_pct".into());
    }
    if limits.working == 0 {
        return bad("limits.working".into());
    }
    if limits.open == 0 {
        return bad("limits.open".into());
    }
    for (i, rule) in config.project_rules.iter().enumerate() {
        if rule.project.0.trim().is_empty() {
            return bad(format!("project_rules[{i}].project"));
        }
        if rule.server.trim().is_empty() {
            return bad(format!("project_rules[{i}].server"));
        }
    }
    for (i, rule) in config.schedule_rules.iter().enumerate() {
        if rule.days.iter().any(|d| !(1..=7).contains(d)) {
            return bad(format!("schedule_rules[{i}].days"));
        }
        if hhmm(&rule.from).is_none() {
            return bad(format!("schedule_rules[{i}].from"));
        }
        if hhmm(&rule.to).is_none() {
            return bad(format!("schedule_rules[{i}].to"));
        }
        if rule.tz.parse::<Tz>().is_err() {
            return bad(format!("schedule_rules[{i}].tz"));
        }
        if rule.avoid.trim().is_empty() {
            return bad(format!("schedule_rules[{i}].avoid"));
        }
    }
    for (server, dir) in &config.clone_dirs {
        if server.trim().is_empty() || dir.trim().is_empty() {
            return bad(format!("clone_dirs.{server}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{AgentMode, Limits, ProjectRule, ScheduleRule, UiMode};
    use hangar_core::project::ProjectId;

    fn schedule() -> ScheduleRule {
        ScheduleRule { days: vec![1, 5], from: "09:00".into(), to: "18:00".into(), tz: "America/Sao_Paulo".into(), avoid: "nb".into() }
    }

    fn custom() -> PlacementConfig {
        PlacementConfig {
            ui_mode: UiMode::Ask,
            agent_mode: AgentMode::Off,
            limits: Limits { mem_free_pct: 20, working: 4, open: 8, cpu_pct: 70 },
            project_rules: vec![ProjectRule { project: ProjectId("git:github.com/a/b".into()), server: "pc".into() }],
            schedule_rules: vec![schedule()],
            clone_dirs: [("nb".to_owned(), "/home/j/Projetos".to_owned())].into(),
        }
    }

    #[test]
    fn missing_file_is_the_spec_defaults() {
        let home = tempfile::tempdir().unwrap();
        let config = load(home.path()).unwrap();
        assert_eq!((config.ui_mode, config.agent_mode), (UiMode::Suggest, AgentMode::Auto));
        assert_eq!(config.limits, Limits { mem_free_pct: 15, working: 6, open: 10, cpu_pct: 80 });
        assert!(config.project_rules.is_empty() && config.schedule_rules.is_empty() && config.clone_dirs.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let home = tempfile::tempdir().unwrap();
        save(home.path(), &custom()).unwrap();
        assert_eq!(load(home.path()).unwrap(), custom());
    }

    #[test]
    fn unreadable_file_is_an_error_and_an_explicit_save_replaces_it() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(path(home.path()).parent().unwrap()).unwrap();
        std::fs::write(path(home.path()), b"{\"limits\": [").unwrap();
        assert!(load(home.path()).is_err(), "nunca o padrão no lugar do arquivo estragado");
        save(home.path(), &custom()).unwrap();
        assert_eq!(load(home.path()).unwrap(), custom());
    }

    #[test]
    fn invalid_config_is_refused_without_writing() {
        let home = tempfile::tempdir().unwrap();
        let cases: [(&str, fn(&mut PlacementConfig)); 8] = [
            ("schedule_rules[0].tz", |c| c.schedule_rules[0].tz = "Lua/Base".into()),
            ("schedule_rules[0].from", |c| c.schedule_rules[0].from = "9h".into()),
            ("schedule_rules[0].to", |c| c.schedule_rules[0].to = "24:00".into()),
            ("schedule_rules[0].days", |c| c.schedule_rules[0].days = vec![0]),
            ("limits.mem_free_pct", |c| c.limits.mem_free_pct = 101),
            ("limits.working", |c| c.limits.working = 0),
            ("limits.open", |c| c.limits.open = 0),
            ("project_rules[0].server", |c| c.project_rules[0].server = " ".into()),
        ];
        for (field, change) in cases {
            let mut config = custom();
            change(&mut config);
            match save(home.path(), &config) {
                Err(SaveError::Invalid(invalid)) => assert_eq!(invalid.field, field),
                other => panic!("{field}: {other:?}"),
            }
        }
        assert!(!path(home.path()).exists(), "recusa não cria o arquivo");
        assert_eq!(validate(&custom()), Ok(()));
    }
}
