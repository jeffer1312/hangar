//! O que o sonho lembra entre rodadas: até onde leu cada sessão, o gasto do mês e o último resultado
//! de cada projeto. Só a máquina que sonha tem este arquivo.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Key,
    Account,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectState {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub last_dream_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_cost_usd: f64,
    #[serde(default)]
    pub last_run_id: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for ProjectState {
    fn default() -> Self {
        Self { enabled: true, mode: Mode::Key, last_dream_at: None, last_cost_usd: 0.0, last_run_id: None, last_error: None }
    }
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RootEntry {
    pub root: String,
    pub project: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Month {
    /// `AAAA-MM`.
    pub month: String,
    pub key_usd: f64,
    pub account_usd: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub text: String,
    pub conflicts_with: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectState>,
    /// `máquina/caminho do transcript` → eventos já lidos.
    #[serde(default)]
    pub cursors: BTreeMap<String, usize>,
    /// `máquina/cwd` → raiz e projeto, gravado na primeira vez: a worktree apagada continua sendo
    /// do mesmo projeto.
    #[serde(default)]
    pub roots: BTreeMap<String, RootEntry>,
    /// Projeto → projeto em que ele se junta (correção feita na tela).
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub month: Month,
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
    /// Última rodada noturna: o relógio conta o próximo horário a partir dela, mesmo depois de reiniciar.
    #[serde(default)]
    pub last_round_at: Option<DateTime<Utc>>,
}

impl State {
    /// Arquivo ausente é estado novo; ilegível é erro, nunca reescrito por cima.
    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        hangar_core::fs::atomic_write(path, &bytes)
    }

    /// Gasto do mês corrente; na virada do mês começa do zero.
    pub fn month_at(&mut self, now: DateTime<Utc>) -> &mut Month {
        let current = now.format("%Y-%m").to_string();
        if self.month.month != current {
            self.month = Month { month: current, key_usd: 0.0, account_usd: 0.0 };
        }
        &mut self.month
    }

    pub fn canonical<'a>(&'a self, project: &'a str) -> &'a str {
        self.aliases.get(project).map_or(project, String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn missing_file_is_new_state_and_corrupt_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dream").join("state.json");
        assert_eq!(State::load(&path).unwrap(), State::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{quebrado").unwrap();
        assert!(State::load(&path).is_err());
    }

    #[test]
    fn round_trip_and_month_rollover() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        let mut state = State::default();
        state.month_at(Utc.with_ymd_and_hms(2026, 11, 30, 23, 0, 0).unwrap()).key_usd = 12.5;
        state.cursors.insert("notebook/a.jsonl".into(), 40);
        state.save(&path).unwrap();
        let mut back = State::load(&path).unwrap();
        assert_eq!(back, state);
        assert_eq!(back.month_at(Utc.with_ymd_and_hms(2026, 11, 30, 23, 59, 0).unwrap()).key_usd, 12.5);
        assert_eq!(back.month_at(Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap()).key_usd, 0.0);
    }

    #[test]
    fn project_defaults_to_enabled_with_key() {
        let p: ProjectState = serde_json::from_str("{}").unwrap();
        assert!(p.enabled);
        assert_eq!(p.mode, Mode::Key);
    }
}
