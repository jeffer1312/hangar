//! Configuração do sonho desta máquina (`~/.hangar/dream/config.json`, 0600). Fica fora da
//! configuração compartilhada: a chave só existe na máquina que sonha.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    /// `HH:MM` no fuso da máquina.
    #[serde(default = "default_hour")]
    pub hour: String,
    #[serde(default = "default_project_cap")]
    pub project_cap_usd: f64,
    /// Crédito do mês da chave; vazio = sem teto do mês.
    #[serde(default)]
    pub month_cap_usd: Option<f64>,
    /// Id do servidor que sonha por todos; igual em todas as máquinas.
    #[serde(default)]
    pub machine: String,
    /// A passada que escreve `~/.claude/rules/memoria-do-usuario.md`, lido por toda sessão: nasce
    /// desligada, a pessoa liga depois de ler o que ela gera.
    #[serde(default)]
    pub user_memory: bool,
}

fn default_hour() -> String {
    "01:00".into()
}

fn default_project_cap() -> f64 {
    15.0
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            hour: default_hour(),
            project_cap_usd: default_project_cap(),
            month_cap_usd: None,
            machine: String::new(),
            user_memory: false,
        }
    }
}

impl Config {
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

    /// O que a tela pode gravar; o resto é recusado com o nome do campo.
    pub fn validate(&self) -> Result<(), &'static str> {
        let hour_ok = self.hour.split_once(':').is_some_and(|(h, m)| {
            h.len() == 2 && m.len() == 2 && h.parse::<u8>().is_ok_and(|h| h < 24) && m.parse::<u8>().is_ok_and(|m| m < 60)
        });
        if !hour_ok {
            return Err("hour");
        }
        if !(self.project_cap_usd > 0.0 && self.project_cap_usd.is_finite()) {
            return Err("project_cap_usd");
        }
        if self.month_cap_usd.is_some_and(|c| !(c > 0.0 && c.is_finite())) {
            return Err("month_cap_usd");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c, Config::default());
        assert!(c.validate().is_ok());
        assert_eq!(Config { hour: "24:00".into(), ..Config::default() }.validate(), Err("hour"));
        assert_eq!(Config { hour: "1:00".into(), ..Config::default() }.validate(), Err("hour"));
        assert_eq!(Config { project_cap_usd: 0.0, ..Config::default() }.validate(), Err("project_cap_usd"));
        assert_eq!(Config { month_cap_usd: Some(-1.0), ..Config::default() }.validate(), Err("month_cap_usd"));
    }

    #[test]
    fn saved_file_is_private() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dream").join("config.json");
        let c = Config { api_key: "k".into(), enabled: true, ..Config::default() };
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
