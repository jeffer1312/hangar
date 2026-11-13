//! `PUT /api/computer-control`: liga ou desliga o MCP. Tudo que recusa vem antes da primeira gravação.
use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use hangar_core::pycompat::truthy;

use crate::files::{known_from, read_entry, read_object};
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

/// Grava (ligado) ou tira (desligado) a entrada em todos os `.claude.json`.
pub fn save(paths: &Paths, facts: &Facts, aux: &Aux, body: &SaveRequest) -> Result<(), Error> {
    if body.jev_key.as_deref().is_some_and(|key| !key.trim().is_empty()) {
        return Err(Error::plain(400, "erro_computer_control_jev_key_moved", "a chave do Jev do Computer Use agora é a da página Jev"));
    }
    if !body.enabled {
        return remove_entry(paths);
    }
    // O modo local saiu: só o binário instalado pelo Hangar.
    if body.mode == Some(Mode::Local) {
        return Err(Error::plain(
            400,
            "erro_computer_control_update_app",
            "Atualize o app: o Computer Use agora é instalado e ligado sozinho",
        ));
    }
    // Na ordem do Python, para o mesmo arquivo ruim aparecer primeiro.
    let main = read_entry(paths.main_file(), false)?;
    known_from(paths, main.entry)?;
    let install = read_object(&paths.install_json(), false)?;
    if !(install.get("tag").is_some_and(|tag| truthy(tag)) && paths.binary().is_file()) {
        return Err(Error::plain(400, "erro_computer_control_not_installed", "instale a versão publicada antes de usar esse modo"));
    }
    let agent = body.agent_config.trim();
    if !Path::new(agent).is_file() {
        return Err(Error {
            status: 400,
            code: "erro_computer_control_agent",
            msg: format!("o arquivo {agent} não existe"),
            params: json!({"file": agent}),
        });
    }
    reject_own_model(body, aux, &facts.cliproxy_keys)?;
    write_entry(paths, &Entry {
        command: paths.binary().to_string_lossy().into_owned(),
        agent_config: agent.to_owned(),
        jev: facts.jev.clone(),
    })
}

/// Modelo diferente do auxiliar sumiria calado: volta erro com os campos, na ordem do corpo.
fn reject_own_model(body: &SaveRequest, aux: &Aux, cliproxy_keys: &[String]) -> Result<(), Error> {
    let sent = [
        ("llm_url", base_url(body.llm_url.trim()), &aux.base_url),
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

/// O `sem_chat_completions` do Python: tira o `/chat/completions` do caminho, mantendo a query.
fn base_url(value: &str) -> String {
    let split = value.find(['?', '#']).unwrap_or(value.len());
    let path = value[..split].trim_end_matches('/');
    format!("{}{}", path.strip_suffix("/chat/completions").unwrap_or(path), &value[split..])
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn base_url_keeps_the_query() {
        assert_eq!(base_url("http://x/v1/chat/completions"), "http://x/v1");
        assert_eq!(base_url("https://a.b/v1/chat/completions/?x=1"), "https://a.b/v1?x=1");
        assert_eq!(base_url("http://x/v1/"), "http://x/v1");
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
                let _ = save(&paths, &Facts::default(), &Aux::default(), &body);
            }
        }
    }
}
