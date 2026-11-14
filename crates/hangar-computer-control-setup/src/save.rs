//! `PUT /api/computer-control`: liga ou desliga o MCP. Tudo que recusa vem antes da primeira gravação.
use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use hangar_core::llm_url::base_url;

use crate::files::{installed, known_from, read_entry};
use crate::writer::{Entry, remove_entry, write_entry};
use crate::{Error, Facts, Paths};

/// O corpo da tela (o `ComputerControlBody` do Python); campo a mais é recusado.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveRequest {
    pub enabled: bool,
    #[serde(default)]
    pub mode: Option<Mode>,
    #[serde(default)]
    pub project_dir: String,
    #[serde(default)]
    pub agent_config: String,
    // Cliente antigo ainda manda o modelo: igual ao auxiliar da página Jev passa, diferente volta erro.
    #[serde(default)]
    pub llm_url: String,
    #[serde(default)]
    pub llm_model: String,
    #[serde(default)]
    pub llm_effort: String,
    #[serde(default)]
    pub llm_key: Option<String>,
    /// Cliente antigo: vazio passa, chave digitada volta erro (o Jev vem da página dele).
    #[serde(default)]
    pub jev_key: Option<String>,
    #[serde(default)]
    pub use_cliproxy_key: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Package,
    Local,
}

/// O modelo auxiliar gravado na página Jev (`jev_aux_*`, aparado), como o cliente antigo o compara.
#[derive(Clone, Debug, Default)]
pub struct Aux {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub effort: String,
}

/// Grava (ligado) ou tira (desligado) a entrada em todos os `.claude.json`. `jev` dá o env do Jev e é
/// lido já sob a trava (ver `write_entry`).
pub fn save(
    paths: &Paths,
    facts: &Facts,
    aux: &Aux,
    body: &SaveRequest,
    jev: impl FnOnce() -> Result<Map<String, Value>, Error>,
) -> Result<(), Error> {
    if body.jev_key.as_deref().is_some_and(|key| !key.trim().is_empty()) {
        return Err(Error::plain(400, "erro_computer_control_jev_key_moved", "a chave do Jev do Computer Use agora é a da página Jev"));
    }
    // O modo local saiu: só o binário instalado pelo Hangar. Recusa também ao desligar (D2.2).
    if body.mode == Some(Mode::Local) {
        return Err(Error::plain(
            400,
            "erro_computer_control_update_app",
            "Atualize o app: o Computer Use agora é instalado e ligado sozinho",
        ));
    }
    if !body.enabled {
        return remove_entry(paths);
    }
    // Na ordem do Python, para o mesmo arquivo ruim aparecer primeiro.
    let main = read_entry(paths.main_file(), false)?;
    known_from(paths, main.entry)?;
    if !installed(paths)? {
        return Err(Error::plain(400, "erro_computer_control_not_installed", "instale a versão publicada antes de usar esse modo"));
    }
    let agent = body.agent_config.trim();
    // Só com o alvo `linux` automático do Hyprland o padrão fica vazio, como no registro.
    if !(Path::new(agent).is_file() || (agent.is_empty() && facts.hyprland)) {
        return Err(Error {
            status: 400,
            code: "erro_computer_control_agent",
            msg: format!("o arquivo {agent} não existe"),
            params: json!({"file": agent}),
        });
    }
    reject_own_model(body, aux, &facts.cliproxy_keys)?;
    let entry = Entry { command: paths.binary().to_string_lossy().into_owned(), agent_config: agent.to_owned() };
    write_entry(paths, &entry, jev)
}

/// Modelo diferente do auxiliar sumiria calado: volta erro com os campos, na ordem do corpo. Os dois
/// endereços passam pela mesma normalização: o gravado pode ter a barra final que o cliente não manda.
fn reject_own_model(body: &SaveRequest, aux: &Aux, cliproxy_keys: &[String]) -> Result<(), Error> {
    let stored_url = base_url(&aux.base_url);
    let sent = [
        ("llm_url", base_url(body.llm_url.trim()), &stored_url),
        ("llm_model", body.llm_model.trim().to_owned(), &aux.model),
        ("llm_effort", body.llm_effort.trim().to_owned(), &aux.effort),
        ("llm_key", body.llm_key.as_deref().unwrap_or("").trim().to_owned(), &aux.api_key),
    ];
    let mut moved = Vec::new();
    for (field, value, current) in sent {
        if !value.is_empty() && value != *current {
            moved.push(field);
        }
    }
    if body.use_cliproxy_key && !cliproxy_keys.contains(&aux.api_key) {
        moved.push("use_cliproxy_key");
    }
    if moved.is_empty() {
        return Ok(());
    }
    Err(Error {
        status: 400,
        code: "erro_computer_control_llm_movido",
        msg: "o modelo do Computer Use agora é o modelo auxiliar da página Jev".into(),
        params: json!({"fields": moved.join(",")}),
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// Pacote instalado (`install.json` com tag e binário), sem `.claude.json`.
    fn installed_home() -> (tempfile::TempDir, Paths) {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path().to_owned(), Vec::new());
        std::fs::create_dir_all(paths.targets_dir()).unwrap();
        std::fs::write(paths.binary(), b"\x7fELF").unwrap();
        std::fs::write(paths.install_json(), br#"{"tag": "v2.0.0"}"#).unwrap();
        (home, paths)
    }

    fn request(body: Value) -> SaveRequest {
        serde_json::from_value(body).unwrap()
    }

    /// D2.2: `mode: "local"` recusa também ao desligar, sem guardar nem tirar a entrada.
    #[test]
    fn local_mode_is_refused_even_when_turning_off() {
        let (_home, paths) = installed_home();
        std::fs::write(paths.main_file(), br#"{"mcpServers": {"hangar-computer-control": {"command": "x"}}}"#).unwrap();
        let before = std::fs::read(paths.main_file()).unwrap();
        let body = request(json!({"enabled": false, "mode": "local"}));
        let error = save(&paths, &Facts::default(), &Aux::default(), &body, || Ok(Map::new())).unwrap_err();
        assert_eq!(error.code, "erro_computer_control_update_app");
        assert_eq!(std::fs::read(paths.main_file()).unwrap(), before);
        assert!(!paths.parked_file().exists());
    }

    /// P1: só com o Hyprland (alvo `linux` automático) o padrão vazio liga, sem `HCC_AGENT_CONFIG`.
    #[test]
    fn empty_default_target_turns_on_only_with_hyprland() {
        let (_home, paths) = installed_home();
        let body = request(json!({"enabled": true, "mode": "package", "agent_config": ""}));
        let error = save(&paths, &Facts::default(), &Aux::default(), &body, || Ok(Map::new())).unwrap_err();
        assert_eq!(error.code, "erro_computer_control_agent");
        assert!(!paths.main_file().exists());
        let facts = Facts { hyprland: true, ..Facts::default() };
        save(&paths, &facts, &Aux::default(), &body, || Ok(Map::new())).unwrap();
        let data: Value = serde_json::from_slice(&std::fs::read(paths.main_file()).unwrap()).unwrap();
        let entry = &data["mcpServers"][crate::NAME];
        assert_eq!(entry["command"], json!(paths.binary().to_string_lossy()));
        assert!(entry["env"].get("HCC_AGENT_CONFIG").is_none(), "{entry}");
    }

    /// O endereço gravado com barra final e o do cliente com `/chat/completions` são o mesmo modelo.
    #[test]
    fn the_stored_url_is_normalized_like_the_sent_one() {
        let aux = Aux { base_url: "https://x/v1/".into(), ..Aux::default() };
        let body = request(json!({"enabled": true, "llm_url": "https://x/v1/chat/completions"}));
        reject_own_model(&body, &aux, &[]).unwrap();
        let other = request(json!({"enabled": true, "llm_url": "https://y/v1"}));
        assert_eq!(reject_own_model(&other, &aux, &[]).unwrap_err().code, "erro_computer_control_llm_movido");
    }

    proptest! {
        /// Qualquer corpo: recusa (422 na rota) ou resposta com código, nunca pânico.
        #[test]
        fn save_body_never_panics(text in "\\PC{0,200}", json_body in prop::option::of(any::<(bool, Option<bool>, String, String, String, Option<String>, bool)>())) {
            let home = tempfile::tempdir().unwrap();
            let paths = Paths::new(home.path().to_owned(), Vec::new());
            let _ = serde_json::from_str::<SaveRequest>(&text);
            if let Some((enabled, local, agent, model, url, jev_key, cliproxy)) = json_body {
                let mode = local.map(|local| if local { "local" } else { "package" });
                let raw = json!({"enabled": enabled, "mode": mode, "agent_config": agent, "llm_model": model,
                    "llm_url": url, "jev_key": jev_key, "use_cliproxy_key": cliproxy});
                let body: SaveRequest = serde_json::from_value(raw).unwrap();
                let _ = save(&paths, &Facts::default(), &Aux::default(), &body, || Ok(Map::new()));
            }
        }
    }
}
