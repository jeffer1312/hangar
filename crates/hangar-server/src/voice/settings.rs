//! O que a voz lê do servidor: a trava e o Jev do runtime-config (só leitura) e as escolhas da chamada (arquivo próprio).
use super::{jev, organizer::{ModeModel, ModeModels, DEFAULT_EFFORT}};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Só as vozes que o realtime v3 aceita; as outras derrubam a chamada.
pub const VOICES: [&str; 9] = ["arbor", "breeze", "cove", "ember", "juniper", "maple", "sol", "spruce", "vale"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairDto { #[serde(default)] pub model: Option<String>, #[serde(default = "default_effort")] pub effort: String, #[serde(default)] pub tier: Option<String> }

fn default_effort() -> String { DEFAULT_EFFORT.to_owned() }
fn default_account() -> String { "default".to_owned() }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrganizerDto { pub direct: PairDto, pub plan: PairDto }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingsDto { #[serde(default)] pub voice: Option<String>, #[serde(default = "default_account")] pub codex_account: String, pub organizer: OrganizerDto }

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceSettings { pub voice: Option<String>, pub codex_account: String, pub organizer: ModeModels }

impl Default for VoiceSettings {
    fn default() -> Self { Self { voice: None, codex_account: default_account(), organizer: ModeModels::default() } }
}

impl From<SettingsDto> for VoiceSettings {
    fn from(d: SettingsDto) -> Self {
        let pair = |p: PairDto| ModeModel { model: p.model, effort: p.effort, tier: p.tier };
        Self { voice: d.voice, codex_account: d.codex_account, organizer: ModeModels { direct: pair(d.organizer.direct), plan: pair(d.organizer.plan) } }
    }
}

impl From<&VoiceSettings> for SettingsDto {
    fn from(s: &VoiceSettings) -> Self {
        let pair = |p: &ModeModel| PairDto { model: p.model.clone(), effort: p.effort.clone(), tier: p.tier.clone() };
        Self { voice: s.voice.clone(), codex_account: s.codex_account.clone(), organizer: OrganizerDto { direct: pair(&s.organizer.direct), plan: pair(&s.organizer.plan) } }
    }
}

pub struct Gate { pub enabled: bool, pub jev: Option<jev::Config> }

/// Lido uma vez, por quem monta o hub; o teste passa a própria pasta.
pub fn claude_dir(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).map_or_else(|| home.join(".claude"), PathBuf::from)
}

/// Ausente é normal; ilegível ou quebrado vai ao diário (só o nome e o tipo), senão a trava ou a chave cai calada.
fn read_json(path: &Path) -> Value {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            super::log(format!("voice {name} unreadable json kind={:?}", e.classify()));
            Value::Null
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => { super::log(format!("voice {name} unreadable kind={:?}", e.kind())); Value::Null }
    }
}

/// Bloqueia (lê arquivos).
pub fn read_gate(home: &Path, claude_dir: &Path) -> Gate {
    let runtime = read_json(&claude_dir.join("runtime-config.json"));
    let settings = read_json(&home.join(".claude").join("settings.json"));
    Gate { enabled: runtime["codex_voice_beta"] == Value::Bool(true),
        jev: jev::config_from(&runtime, &settings["env"], std::env::var("TYPESAFE_API_KEY").ok().filter(|v| !v.is_empty())) }
}

pub fn settings_path(home: &Path) -> PathBuf { home.join(".hangar").join("voz").join("config.json") }

pub fn read_settings(home: &Path) -> VoiceSettings {
    serde_json::from_value::<SettingsDto>(read_json(&settings_path(home))).map(Into::into).unwrap_or_default()
}

pub fn write_settings(home: &Path, s: &VoiceSettings) -> std::io::Result<()> {
    let path = settings_path(home);
    std::fs::create_dir_all(path.parent().unwrap_or(home))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&SettingsDto::from(s)).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &path)
}
