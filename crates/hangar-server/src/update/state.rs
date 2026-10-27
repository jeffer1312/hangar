//! `estado.json` da atualização: o mesmo arquivo, os mesmos campos e o mesmo jeito de gravar do
//! motor `git` (`backend/app/atualizar.py`): os clientes (nativo, Expo, PWA) leem por polling e
//! não sabem qual motor escreveu.
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Linhas guardadas no `log` do estado, como o `_TETO_LOG` do Python.
const LOG_CAP: usize = 400;

/// Etapas do motor de pacote, na ordem. O rótulo é o que a tela mostra.
pub const STEPS: &[(&str, &str)] = &[
    ("baixar", "Baixando a versão nova"),
    ("passos", "Aplicando o que a versão pede"),
    ("trocar", "Trocando para a versão nova"),
    ("reiniciar", "Reiniciando o servidor"),
];

/// `<config do Claude>/.hangar-update/`, a pasta do `_base()` do Python (`CLAUDE_CONFIG_DIR` ou
/// `~/.claude`). Lá moram `estado.json` e o registro `aplicados.json` dos passos.
pub fn state_dir(claude_base: &Path) -> PathBuf { claude_base.join(".hangar-update") }

#[derive(Debug, Clone)]
pub struct StateFile {
    path: PathBuf,
}

impl StateFile {
    pub fn new(state_dir: &Path) -> StateFile { StateFile { path: state_dir.join("estado.json") } }

    pub fn path(&self) -> &Path { &self.path }

    /// `{}` quando não existe ou não é um objeto, como o `estado()` do Python.
    pub fn read(&self) -> Map<String, Value> {
        std::fs::read(&self.path).ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|v| match v { Value::Object(m) => Some(m), _ => None })
            .unwrap_or_default()
    }

    /// Mescla `fields` no que já está gravado e carimba `ts`. Falha de escrita não derruba quem
    /// chamou: vai ao log, e a próxima escrita leva o estado inteiro.
    pub fn write(&self, fields: Value) {
        let mut current = self.read();
        if let Value::Object(fields) = fields { current.extend(fields); }
        current.insert("ts".into(), Value::String(now()));
        let result = self.path.parent().map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| crate::runtime::queue::atomic_write(&self.path, &serde_json::to_vec(&Value::Object(current)).unwrap_or_default()));
        if let Err(e) = result {
            tracing::warn!("update: estado.json não gravado: {e}");
        }
    }

    /// Marca a etapa atual; `passo`/`total` alimentam a barra e `etapa_inicio` o relógio da tela.
    pub fn step(&self, key: &str) {
        let index = STEPS.iter().position(|(k, _)| *k == key).unwrap_or(0);
        self.write(serde_json::json!({"fase": "rodando", "etapa": key, "passo": index + 1, "total": STEPS.len(),
            "texto": STEPS[index].1, "etapa_inicio": now()}));
    }

    /// Uma linha por item no `log` que a tela mostra, com o teto do Python.
    pub fn log(&self, text: &str) {
        let mut lines: Vec<Value> = self.read().remove("log").and_then(|v| match v { Value::Array(a) => Some(a), _ => None })
            .unwrap_or_default();
        lines.extend(text.lines().map(|l| Value::String(l.to_owned())));
        let skip = lines.len().saturating_sub(LOG_CAP);
        self.write(serde_json::json!({"log": lines.split_off(skip)}));
    }

    pub fn warn(&self, text: &str) {
        let mut warnings: Vec<Value> = self.read().remove("avisos").and_then(|v| match v { Value::Array(a) => Some(a), _ => None })
            .unwrap_or_default();
        warnings.push(Value::String(text.to_owned()));
        self.write(serde_json::json!({"avisos": warnings}));
    }
}

/// O `datetime.now().astimezone().isoformat(timespec="seconds")` do Python.
fn now() -> String { chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string() }
