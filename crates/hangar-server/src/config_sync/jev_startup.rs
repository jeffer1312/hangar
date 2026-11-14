//! Migração do Jev na subida: o modelo auxiliar antigo, a chave antiga do Computer Use e o repasse ao MCP.
use std::{collections::BTreeSet, path::Path};

use serde_json::{Map, Value, json};

use super::{
    jev::{env_of, write},
    legacy::{AUX, read_object, text},
    runtime::RuntimeConfig,
};
use crate::accounts::environment::AccountEnvironment;
use crate::orchestration::truthy;
use hangar_computer_control_setup as setup;

/// Código da falha no diário e no log; a subida segue.
pub const FAILURE: &str = "jev_startup_migration_failed";
const MARKER: &str = ".hangar/jev-migrated";
const OLD_KEYS: &str = ".hangar/jev-chave-antiga-computer-use.json";
const KEY: &str = "TYPESAFE_API_KEY";

impl RuntimeConfig {
    /// Uma vez por subida. Cada etapa que falha vai ao diário e ao log, e a seguinte roda mesmo assim.
    pub fn startup_jev_migration(&self) {
        // O auxiliar antigo antes do repasse: o repasse reescreve o `LLM_*` de onde ele é lido.
        if let Err(error) = self.migrate_old_aux() {
            report(&self.env, "auxiliar", &error);
        }
        if let Err(error) = self.migrate_old_key() {
            report(&self.env, "chave", &error);
        }
    }

    /// O auxiliar de antes da configuração única vira `jev_aux_*`, para quem lê o arquivo cru.
    fn migrate_old_aux(&self) -> Result<(), String> {
        let current = self.read();
        if AUX.iter().any(|key| current.contains_key(*key)) {
            return Ok(());
        }
        let mut legacy: Map<String, Value> = self
            .legacy_aux(&current, true)?
            .into_iter()
            .filter(|(_, value)| truthy(value))
            .collect();
        if legacy.is_empty() {
            return Ok(());
        }
        // Gravar o auxiliar repassa o Jev ao MCP: com a página vazia, o repasse apagaria a chave que
        // a etapa seguinte ainda ia migrar. Ela vira a do Jev na mesma gravação.
        let key = match self.unmigrated_mcp_key()? {
            Some(key) => {
                legacy.insert("jev_api_key".into(), json!(key));
                key
            }
            None => self.jev_key(),
        };
        self.keep_replaced_keys(&key)?;
        self.apply(&legacy, &BTreeSet::new()).map(drop)
    }

    /// A chave do MCP que a migração única ainda vai levar ao Jev: sem marca e com a página vazia.
    fn unmigrated_mcp_key(&self) -> Result<Option<String>, String> {
        if self.env.home.join(MARKER).exists() || !text(Some(&self.get("jev_api_key"))).is_empty() {
            return Ok(None);
        }
        Ok(string_key(&self.known_env()?))
    }

    /// A chave que só existia no MCP ou no `settings.json` vira a do Jev, uma vez; depois o MCP recebe o Jev.
    fn migrate_old_key(&self) -> Result<(), String> {
        let marker = self.env.home.join(MARKER);
        // Sem a marca, chave apagada na página Jev voltaria do MCP ou do settings.json a cada subida.
        if !marker.exists() {
            if text(Some(&self.get("jev_api_key"))).is_empty() {
                let old = match string_key(&self.known_env()?) {
                    Some(key) => Some(key),
                    None => settings_key(&self.env.home.join(".claude/settings.json"))?,
                };
                if let Some(old) = old {
                    self.keep_replaced_keys(&old)?;
                    let change = Map::from_iter([("jev_api_key".to_owned(), json!(old))]);
                    self.apply(&change, &BTreeSet::new())?;
                    tracing::info!("chave do Jev do Computer Use virou a configuração única do Jev");
                }
            }
            if let Some(parent) = marker.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&marker)
                .map_err(|error| format!("{}: {error}", marker.display()))?;
        }
        self.keep_replaced_keys(&self.jev_key())?;
        self.sync_jev()
    }

    /// Antes de o registro reescrever entradas com o env do Jev: a chave que a migração ainda vai levar à
    /// página não sai do MCP, e toda chave trocada fica guardada, como no repasse.
    pub(crate) fn keep_keys_before_register(&self) -> Result<(), String> {
        if self.unmigrated_mcp_key()?.is_some() {
            return Err("a chave do Jev do Computer Use ainda não migrou para a página Jev".into());
        }
        self.keep_replaced_keys(&self.jev_key())
    }

    /// A chave que o repasse grava no MCP.
    fn jev_key(&self) -> String {
        self.get("jev_api_key").as_str().unwrap_or("").trim().to_owned()
    }

    /// `env` da entrada ativa no `~/.claude.json`, senão da guardada. Lido pelo leitor do Computer Use,
    /// o mesmo do escritor: um arquivo que ele grava nunca fica ilegível aqui.
    fn known_env(&self) -> Result<Map<String, Value>, String> {
        let paths = setup::Paths::new(self.env.home.clone(), Vec::new());
        let main = setup::read_entries(&paths).map_err(|e| e.msg)?.into_iter().next().and_then(|file| file.entry);
        match main {
            Some(entry) => env_of(&entry, &paths.main_file()),
            None => {
                let parked = setup::parked_entry(&paths).map_err(|e| e.msg)?.unwrap_or_default();
                env_of(&parked, &paths.parked_file())
            }
        }
    }

    /// Guarda, por arquivo de origem, a chave do MCP que o próximo repasse troca por `new`.
    fn keep_replaced_keys(&self, new: &str) -> Result<(), String> {
        // `new` vazio também troca: o repasse tira a chave de quem a tem. Origem ilegível fica de fora: o leitor é o mesmo do repasse, que recusa o mesmo arquivo antes de
        // gravar qualquer um; nenhuma chave é trocada sem a antiga guardada.
        let paths = self
            .computer_use_paths()
            .unwrap_or_else(|_| setup::Paths::new(self.env.home.clone(), Vec::new()));
        let main = paths.main_file();
        let mut sources: Vec<(std::path::PathBuf, Map<String, Value>)> = paths
            .config_files()
            .into_iter()
            .filter_map(|path| {
                let account = path != main;
                let file = setup::read_entry(path, account).ok()?;
                file.entry.map(|entry| (file.path, entry))
            })
            .collect();
        if let Ok(Some(entry)) = setup::parked_entry(&paths) {
            sources.push((paths.parked_file(), entry));
        }
        let replaced: Vec<(String, Value)> = sources
            .iter()
            .filter_map(|(path, entry)| {
                let env = env_of(entry, path).ok()?;
                let old = env.get(KEY).filter(|value| truthy(value))?;
                (text(Some(old)) != new).then(|| (path.to_string_lossy().into_owned(), old.clone()))
            })
            .collect();
        let file = self.env.home.join(OLD_KEYS);
        if keep_old_keys(&file, &replaced)? {
            tracing::warn!(file = %file.display(), "chave do Jev do Computer Use trocada pela da página Jev; a antiga ficou guardada");
        }
        Ok(())
    }
}

/// Diário e log da etapa que falhou; nunca derruba quem chama.
pub fn report(env: &AccountEnvironment, step: &'static str, error: &str) {
    tracing::warn!(code = FAILURE, step, %error, "migração do Jev da subida falhou");
    crate::diag::record_blocking(
        crate::diag::diary_dir_in(env).as_deref(),
        &[json!({"evento": "jev.migracao_subida", "nivel": "erro", "etapa": step, "codigo": FAILURE, "detalhe": error})],
        "migração do Jev",
    );
}

/// `env.TYPESAFE_API_KEY` do `settings.json`; `env` que não é objeto não tem chave.
pub fn settings_key(path: &Path) -> Result<Option<String>, String> {
    Ok(read_object(path)?
        .get("env")
        .and_then(Value::as_object)
        .and_then(string_key))
}

/// Acrescenta cada chave trocada à lista do arquivo de origem (`{"<arquivo>": ["<chave>", …]}`), 0600,
/// sem apagar nada do que já estava, inclusive o formato antigo de uma chave só. Diz se gravou.
pub fn keep_old_keys(file: &Path, replaced: &[(String, Value)]) -> Result<bool, String> {
    if replaced.is_empty() {
        return Ok(false);
    }
    let link = std::fs::read_link(file).ok();
    let raw = crate::codex_setup::files::read(file).map_err(|error| error.to_string())?;
    let mut kept = read_object(file)?;
    let mut changed = false;
    for (source, old) in replaced {
        let slot = kept.entry(source.clone()).or_insert_with(|| json!([]));
        if !slot.is_array() {
            let previous = slot.take();
            *slot = json!([previous]);
        }
        if let Some(list) = slot.as_array_mut()
            && !list.contains(old)
        {
            list.push(old.clone());
            changed = true;
        }
    }
    if changed {
        write(file, kept, true, raw.as_deref(), link.as_deref())?;
    }
    Ok(changed)
}

fn string_key(env: &Map<String, Value>) -> Option<String> {
    env.get(KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}
