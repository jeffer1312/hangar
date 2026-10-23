//! O que a voz lê do servidor: o Jev do runtime-config (só leitura) e as escolhas da chamada (arquivo próprio).
use super::{jev, organizer::{ModeModel, ModeModels, DEFAULT_EFFORT}};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Só as vozes que o realtime v3 aceita; as outras derrubam a chamada.
pub const VOICES: [&str; 9] = ["arbor", "breeze", "cove", "ember", "juniper", "maple", "sol", "spruce", "vale"];
/// As 16 vozes que o Codex 0.163.0 juntou à v3: sem a lista dele, salvar uma delas não é recusado.
pub const V3_ADDED: [&str; 16] = ["aube", "bubbie", "haetsal", "hanul", "himari", "leher", "miko", "neer", "porto", "rindo", "rio",
    "rivage", "selva", "sonna", "tinta", "viola"];

/// Salvar recusa a voz: fora da lista do Codex quando ela foi lida (`codex`), ou, sem ela, fora das vozes v3 conhecidas.
pub fn voice_refused(codex: Option<&[String]>, voice: &str) -> bool {
    match codex {
        Some(list) => !list.iter().any(|v| v == voice),
        None => !VOICES.contains(&voice) && !V3_ADDED.contains(&voice),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairDto { #[serde(default)] pub model: Option<String>, #[serde(default = "default_effort")] pub effort: String, #[serde(default)] pub tier: Option<String> }

fn default_effort() -> String { DEFAULT_EFFORT.to_owned() }
fn default_account() -> String { "default".to_owned() }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrganizerDto { pub direct: PairDto, pub plan: PairDto }

/// Jeito de falar escolhido no cartão da voz. Conversa é o padrão: quem não conhece o app precisa de mais explicação.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style { #[default] Conversational, Direct }

/// `style` ausente no `PUT` (cliente antigo) mantém o gravado; no `GET` vem sempre.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingsDto { #[serde(default)] pub voice: Option<String>, #[serde(default = "default_account")] pub codex_account: String, pub organizer: OrganizerDto,
    #[serde(default)] pub style: Option<Style> }

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceSettings { pub voice: Option<String>, pub codex_account: String, pub organizer: ModeModels, pub style: Style }

/// Escolhas de quem nunca gravou o `config.json`; quem já gravou fica com as suas.
pub const FIRST_VOICE: &str = "sol";
pub const FIRST_MODEL: &str = "gpt-6.1-sol";

impl Default for VoiceSettings {
    fn default() -> Self {
        let pair = |effort: &str| ModeModel { model: Some(FIRST_MODEL.to_owned()), effort: effort.to_owned(), tier: None };
        Self { voice: Some(FIRST_VOICE.to_owned()), codex_account: default_account(),
            organizer: ModeModels { direct: pair("medium"), plan: pair("high") }, style: Style::default() }
    }
}

impl From<SettingsDto> for VoiceSettings {
    fn from(d: SettingsDto) -> Self {
        let pair = |p: PairDto| ModeModel { model: p.model, effort: p.effort, tier: p.tier };
        Self { voice: d.voice, codex_account: d.codex_account, organizer: ModeModels { direct: pair(d.organizer.direct), plan: pair(d.organizer.plan) },
            style: d.style.unwrap_or_default() }
    }
}

impl From<&VoiceSettings> for SettingsDto {
    fn from(s: &VoiceSettings) -> Self {
        let pair = |p: &ModeModel| PairDto { model: p.model.clone(), effort: p.effort.clone(), tier: p.tier.clone() };
        Self { voice: s.voice.clone(), codex_account: s.codex_account.clone(), organizer: OrganizerDto { direct: pair(&s.organizer.direct), plan: pair(&s.organizer.plan) },
            style: Some(s.style) }
    }
}

/// O que a chamada lê do runtime-config: só o Jev. Quem decide se a voz aparece é o Codex e a conta (`routes::settings_reply`).
pub struct Gate { pub jev: Option<jev::Config> }

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

/// O `TYPESAFE_API_KEY` do ambiente, lido uma vez por quem monta o hub; o teste passa `None`.
pub fn env_jev_key() -> Option<String> { std::env::var("TYPESAFE_API_KEY").ok().filter(|v| !v.is_empty()) }

/// Bloqueia (lê arquivos).
pub fn read_gate(home: &Path, claude_dir: &Path, env_key: Option<String>) -> Gate {
    let runtime = read_json(&claude_dir.join("runtime-config.json"));
    let settings = read_json(&home.join(".claude").join("settings.json"));
    Gate { jev: jev::config_from(&runtime, &settings["env"], env_key) }
}

/// Raiz da voz: configuração, pasta própria do organizador e planos.
pub fn voice_dir(home: &Path) -> PathBuf { home.join(".hangar").join("voz") }

pub fn settings_path(home: &Path) -> PathBuf { voice_dir(home).join("config.json") }

pub fn read_settings(home: &Path) -> VoiceSettings {
    let path = settings_path(home);
    if !path.exists() { return VoiceSettings::default(); }
    // Arquivo que existe mas não serve não é "primeira vez": cai no neutro (modelo da conta), não nos padrões novos.
    serde_json::from_value::<SettingsDto>(read_json(&path)).map(Into::into).unwrap_or_else(|e| {
        super::log(format!("voice config.json unusable, using account defaults kind={:?}", e.classify()));
        VoiceSettings { voice: None, codex_account: default_account(), organizer: ModeModels::default(), style: Style::default() }
    })
}

pub fn write_settings(home: &Path, s: &VoiceSettings) -> std::io::Result<()> {
    let path = settings_path(home);
    std::fs::create_dir_all(path.parent().unwrap_or(home))?;
    let bytes = serde_json::to_vec(&SettingsDto::from(s)).map_err(std::io::Error::other)?;
    crate::runtime::queue::atomic_write(&path, &bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn saving_refuses_by_the_codex_list_and_without_it_by_the_known_v3_voices() {
        let codex = vec!["sol".to_owned()];
        assert!(voice_refused(Some(&codex), "aube"), "a lista do Codex manda: um Codex antigo não tem a aube");
        assert!(!voice_refused(Some(&codex), "sol"));
        assert!(!voice_refused(None, "aube") && !voice_refused(None, "sol"), "sem a lista, as v3 conhecidas passam");
        assert!(voice_refused(None, "alloy"), "voz da v2 nunca serve à v3");
    }

    use super::*;

    #[test]
    fn first_call_without_config_uses_sol_and_gpt_6_1() {
        let home = tempfile::tempdir().unwrap();
        let s = read_settings(home.path());
        assert_eq!(s.voice.as_deref(), Some("sol"));
        assert_eq!((s.organizer.direct.model.as_deref(), s.organizer.direct.effort.as_str()), (Some("gpt-6.1-sol"), "medium"));
        assert_eq!((s.organizer.plan.model.as_deref(), s.organizer.plan.effort.as_str()), (Some("gpt-6.1-sol"), "high"));
    }

    #[test]
    fn saved_choices_win_over_first_call_defaults() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(voice_dir(home.path())).unwrap();
        std::fs::write(settings_path(home.path()), r#"{"voice": null, "codex_account": "default",
            "organizer": {"direct": {"model": null, "effort": "low"}, "plan": {"model": "gpt-x", "effort": "xhigh"}}}"#).unwrap();
        let s = read_settings(home.path());
        assert_eq!(s.voice, None);
        assert_eq!((s.organizer.direct.model.as_deref(), s.organizer.direct.effort.as_str()), (None, "low"));
        assert_eq!((s.organizer.plan.model.as_deref(), s.organizer.plan.effort.as_str()), (Some("gpt-x"), "xhigh"));
    }

    #[test]
    fn style_defaults_to_conversation_and_round_trips() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(read_settings(home.path()).style, Style::Conversational, "primeira vez");
        std::fs::create_dir_all(voice_dir(home.path())).unwrap();
        std::fs::write(settings_path(home.path()), r#"{"voice": "sol", "codex_account": "default",
            "organizer": {"direct": {"model": null, "effort": "low"}, "plan": {"model": null, "effort": "high"}}}"#).unwrap();
        assert_eq!(read_settings(home.path()).style, Style::Conversational, "config gravada antes do estilo");
        let mut s = read_settings(home.path());
        s.style = Style::Direct;
        write_settings(home.path(), &s).unwrap();
        assert_eq!(read_settings(home.path()).style, Style::Direct);
        let dto: SettingsDto = serde_json::from_value(serde_json::json!({"organizer": {"direct": {}, "plan": {}}})).unwrap();
        assert_eq!(dto.style, None, "cliente antigo não manda estilo: o PUT mantém o gravado");
        assert_eq!(serde_json::to_value(SettingsDto::from(&s)).unwrap()["style"], "direct");
    }

    #[test]
    fn broken_config_is_not_a_first_call() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(voice_dir(home.path())).unwrap();
        for broken in ["{not json", r#"{"voice": "sol"}"#] {
            std::fs::write(settings_path(home.path()), broken).unwrap();
            let s = read_settings(home.path());
            assert_eq!((s.voice, s.organizer.direct.model, s.organizer.direct.effort.as_str()), (None, None, "low"), "{broken}");
        }
    }
}
