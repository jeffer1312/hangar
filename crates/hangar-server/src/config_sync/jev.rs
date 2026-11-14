//! Publicação do Jev nos registros ativos e guardados do Computer Use. A escrita é a do escritor único
//! da entrada (`hangar_computer_control_setup`); aqui ficam o env do Jev e as contas.
use super::{
    legacy::{AUX, text},
    runtime::RuntimeConfig,
};
use crate::orchestration::truthy;
use hangar_core::llm_url::base_url;
use hangar_computer_control_setup as setup;
use serde_json::{Map, Value, json};
use std::path::Path;
const LLM: [&str; 4] = ["LLM_PROXY_URL", "LLM_MODEL", "LLM_EFFORT", "LLM_PROXY_KEY"];

pub(super) fn env_of(entry: &Map<String, Value>, path: &Path) -> Result<Map<String, Value>, String> {
    match entry.get("env").filter(|v| truthy(v)) {
        None => Ok(Map::new()),
        Some(Value::Object(env)) => Ok(env.clone()),
        _ => Err(format!("env em {} não é um objeto JSON", path.display())),
    }
}
pub(super) fn write(
    path: &Path,
    data: Map<String, Value>,
    private: bool,
    expected: Option<&[u8]>,
    link: Option<&Path>,
) -> Result<(), String> {
    setup::write_json(path, data, private, expected, link).map_err(|e| e.msg)
}
impl RuntimeConfig {
    pub(crate) fn jev_env(&self) -> Map<String, Value> {
        let mut output = Map::new();
        let key = self
            .get("jev_api_key")
            .as_str()
            .unwrap_or("")
            .trim()
            .to_owned();
        if !key.is_empty() {
            let endpoint = self.get("jev_endpoint");
            let model = self.get("jev_model");
            let (endpoint, model) =
                crate::voice::jev::destination(&key, endpoint.as_str(), model.as_str());
            output.insert("TYPESAFE_API_KEY".into(), json!(key));
            output.insert(
                "JEV_ENDPOINT".into(),
                json!(endpoint.unwrap_or_else(|| "https://api.typesafe.ai/v1/systemone".into())),
            );
            output.insert(
                "JEV_MODEL".into(),
                json!(model.unwrap_or_else(|| "jev-latest".into())),
            );
        }
        for (key, target) in [AUX[0], AUX[2], AUX[3], AUX[1]].into_iter().zip(LLM) {
            let value = self.get(key).as_str().unwrap_or("").trim().to_owned();
            if value.is_empty() {
                continue;
            }
            let value = if key == AUX[0] {
                let base = base_url(&value);
                let split = base.find(['?', '#']).unwrap_or(base.len());
                format!("{}/chat/completions{}", &base[..split], &base[split..])
            } else {
                value
            };
            output.insert(target.into(), json!(value));
        }
        output
    }
    /// O auxiliar como o `jev_aux()` do Python: os `jev_aux_*` aparados.
    pub(crate) fn jev_aux(&self) -> setup::Aux {
        let value = |key: &str| text(Some(&self.get(key)));
        setup::Aux {
            base_url: value(AUX[0]),
            api_key: value(AUX[1]),
            model: value(AUX[2]),
            effort: value(AUX[3]),
        }
    }
    /// `~/.claude.json` e o de cada outra conta Claude.
    pub(super) fn computer_use_paths(&self) -> Result<setup::Paths, String> {
        crate::computer_control::account_paths(&crate::accounts::AccountService::new(self.env.clone()))
    }
    pub fn sync_jev(&self) -> Result<(), String> {
        setup::sync_jev(&self.computer_use_paths()?, &self.jev_env()).map_err(|e| e.msg)
    }
}
