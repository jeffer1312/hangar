//! Token do Help já configurado pelo plugin Promedico, lido sem copiar nem exportar: arquivos de
//! credenciais do plugin (fusão por data), depois o bloco `env` do settings do Claude Code, depois o
//! ambiente. É a mesma precedência do `claude_settings.py` do plugin.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const KEY: &str = "HELP_TOKEN";
const FILE: &str = "credenciais.json";
const DATES: &str = "_atualizado_em";

/// Onde procurar; os campos vazios seguem o ambiente, como no plugin.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    /// `--config-dir` ou `PMEDICO_CONFIG_DIR`: a única pasta de credenciais quando presente.
    pub config_dir: Option<PathBuf>,
    /// `--claude-config-dir` ou `CLAUDE_CONFIG_DIR`: a pasta do `settings.json`.
    pub claude_dir: Option<PathBuf>,
}

fn home() -> PathBuf { std::env::home_dir().unwrap_or_default() }

fn env_dir(name: &str) -> Option<PathBuf> { std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from) }

impl Sources {
    fn candidates(&self) -> Vec<PathBuf> {
        if let Some(dir) = self.config_dir.clone().or_else(|| env_dir("PMEDICO_CONFIG_DIR")) { return vec![dir.join(FILE)]; }
        let dirs: Vec<PathBuf> = if cfg!(windows) {
            let profile = env_dir("USERPROFILE").unwrap_or_else(home);
            [Some(profile.join(".pmedico")), env_dir("APPDATA").map(|dir| dir.join("pmedico")), env_dir("LOCALAPPDATA").map(|dir| dir.join("pmedico"))]
                .into_iter().flatten().collect()
        } else {
            let config = env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"));
            vec![config.join("pmedico"), home().join(".local").join("share").join("pmedico"), home().join(".pmedico")]
        };
        dirs.into_iter().map(|dir| dir.join(FILE)).collect()
    }

    fn settings(&self) -> PathBuf {
        self.claude_dir.clone().or_else(|| env_dir("CLAUDE_CONFIG_DIR")).unwrap_or_else(|| home().join(".claude")).join("settings.json")
    }

    /// Lido a cada consulta: o login novo do plugin vale sem reiniciar o conector.
    pub fn token(&self) -> Option<String> {
        from_files(&self.candidates())
            .or_else(|| read_json(&self.settings()).and_then(|value| text(value.pointer(&format!("/env/{KEY}")))))
            .or_else(|| std::env::var(KEY).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty()))
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned)
}

/// Arquivo trocado de forma atômica pelo plugin: a leitura tolera a troca no Windows.
fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&hangar_core::fs::read_replaced(path).ok()?).ok().filter(Value::is_object)
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().filter(|meta| meta.is_file()).and_then(|meta| meta.modified().ok())
}

fn seconds(time: SystemTime) -> f64 { time.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

/// Vence o valor não vazio de data mais recente (`_atualizado_em`, senão a do arquivo); no empate,
/// o primeiro na ordem de leitura, que vai do arquivo modificado mais recentemente ao mais antigo.
fn from_files(candidates: &[PathBuf]) -> Option<String> {
    let mut existing: Vec<(usize, &PathBuf, f64)> = candidates.iter().enumerate()
        .filter_map(|(index, path)| modified(path).map(|time| (index, path, seconds(time)))).collect();
    existing.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
    let mut best: Option<(String, f64)> = None;
    for (_, path, file_date) in existing {
        let Some(data) = read_json(path) else { continue };
        let Some(value) = text(data.get(KEY)) else { continue };
        let date = data.pointer(&format!("/{DATES}/{KEY}")).and_then(Value::as_str)
            .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
            .map(|date| date.timestamp_millis() as f64 / 1000.0)
            .unwrap_or(file_date);
        if best.as_ref().is_none_or(|(_, current)| date > *current) { best = Some((value, date)); }
    }
    best.map(|(value, _)| value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(dir: &Path, value: Value) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(FILE);
        std::fs::write(&path, value.to_string()).unwrap();
        path
    }

    #[test]
    fn newest_dated_value_wins_across_folders_and_empty_never_wins() {
        let root = tempfile::tempdir().unwrap();
        let old = write(&root.path().join("a"), json!({KEY: "antigo", DATES: {KEY: "2026-01-01T00:00:00+00:00"}}));
        let new = write(&root.path().join("b"), json!({KEY: "novo", DATES: {KEY: "2026-11-12T10:00:00-03:00"}}));
        let empty = write(&root.path().join("c"), json!({KEY: "  ", DATES: {KEY: "2027-01-01T00:00:00+00:00"}}));
        assert_eq!(from_files(&[old.clone(), new.clone(), empty.clone()]).as_deref(), Some("novo"));
        assert_eq!(from_files(&[empty, old]).as_deref(), Some("antigo"));
    }

    #[test]
    fn explicit_config_dir_is_the_only_candidate() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("so-esta"), json!({KEY: "daqui"}));
        let sources = Sources { config_dir: Some(root.path().join("so-esta")), claude_dir: Some(root.path().join("vazia")) };
        assert_eq!(sources.candidates(), [root.path().join("so-esta").join(FILE)]);
        assert_eq!(sources.token().as_deref(), Some("daqui"));
    }
}
