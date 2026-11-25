//! Onde o `agy` guarda cada coisa. A conta padrão é o `~/.gemini` (login no chaveiro do sistema); conta adicional
//! é uma pasta própria usada como `GEMINI_HOME`.
use std::path::{Path, PathBuf};

pub const DATA_DIR: &str = "antigravity-cli";
/// Marca da pasta de conta adicional criada pelo Hangar (`~/.antigravity-<nome>`).
pub const MARKER: &str = ".hangar-antigravity-conta";
pub const ACCOUNT_PREFIX: &str = ".antigravity-";

/// Pasta de conta adicional: nome `.antigravity-<nome>` com a marca do Hangar dentro.
pub fn is_account_dir(dir: &Path) -> bool {
    dir.is_absolute()
        && dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.len() > ACCOUNT_PREFIX.len() && n.starts_with(ACCOUNT_PREFIX))
        && dir.join(MARKER).is_file()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// `None` = conta padrão (`~/.gemini`).
    pub gemini_home: Option<PathBuf>,
}

impl Account {
    pub fn extra(dir: PathBuf) -> Self { Self { gemini_home: Some(dir) } }

    pub fn is_default(&self) -> bool { self.gemini_home.is_none() }

    pub fn gemini_dir(&self, home: &Path) -> PathBuf {
        self.gemini_home.clone().unwrap_or_else(|| home.join(".gemini"))
    }

    pub fn data_dir(&self, home: &Path) -> PathBuf { self.gemini_dir(home).join(DATA_DIR) }

    /// `None` para id que não é UUID: o id vem de hook e de spec, e vira parte de caminho.
    pub fn transcript_path(&self, home: &Path, conversation: &str) -> Option<PathBuf> {
        transcript_in(&self.brain_dir(home), conversation)
    }

    pub fn brain_dir(&self, home: &Path) -> PathBuf { self.data_dir(home).join("brain") }

    pub fn hooks_path(&self, home: &Path) -> PathBuf { self.gemini_dir(home).join("config").join("hooks.json") }

    pub fn settings_path(&self, home: &Path) -> PathBuf { self.data_dir(home).join("settings.json") }

    /// Os servidores MCP do usuário (`agy mcp add` grava aqui).
    pub fn mcp_config_path(&self, home: &Path) -> PathBuf { self.gemini_dir(home).join("config").join("mcp_config.json") }
}

/// UUID em minúsculas (`8-4-4-4-12`), o formato que o `agy` usa para conversa.
pub fn valid_conversation_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    parts.len() == 5
        && parts.iter().zip([8, 4, 4, 4, 12]).all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// A conta padrão e as adicionais (`~/.antigravity-<nome>` com a marca do Hangar), nessa ordem.
pub fn known_accounts(home: &Path) -> Vec<Account> {
    let mut extra: Vec<PathBuf> = std::fs::read_dir(home).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| is_account_dir(p)).collect();
    extra.sort();
    std::iter::once(Account::default()).chain(extra.into_iter().map(Account::extra)).collect()
}

/// A pasta de trabalho da conversa pelo `history.jsonl` do CLI (`{"workspace", "conversationId", …}` por mensagem
/// digitada): o transcript não guarda a pasta. A última linha da conversa vale.
pub fn workspace_of(account: &Account, home: &Path, conversation: &str) -> Option<String> {
    let text = std::fs::read_to_string(account.data_dir(home).join("history.jsonl")).ok()?;
    text.lines().rev().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        (value["conversationId"] == conversation).then(|| value["workspace"].as_str().map(str::to_owned)).flatten()
    })
}

/// O id da conversa a partir do caminho do transcript (`brain/<id>/.system_generated/logs/transcript_full.jsonl`).
pub fn conversation_of_transcript(path: &Path) -> Option<String> {
    let id = path.parent()?.parent()?.parent()?.file_name()?.to_str()?;
    valid_conversation_id(id).then(|| id.to_owned())
}

/// A pasta `brain` da conta e o id da conversa a partir do caminho do transcript dela.
pub fn place_of_transcript(path: &Path) -> Option<(PathBuf, String)> {
    let conversation = conversation_of_transcript(path)?;
    Some((path.parent()?.parent()?.parent()?.parent()?.to_path_buf(), conversation))
}

/// O transcript de uma conversa dentro da pasta `brain`; `None` para id que não é UUID.
pub fn transcript_in(brain: &Path, conversation: &str) -> Option<PathBuf> {
    valid_conversation_id(conversation).then(|| brain.join(conversation).join(".system_generated/logs/transcript_full.jsonl"))
}

/// Onde o `agy` registra os subagentes que a conversa lançou (`<id>.json` cada um).
pub fn subagents_dir(brain: &Path, conversation: &str) -> PathBuf { brain.join(conversation).join(".system_generated/subagents") }

/// Os subagentes que a conversa lançou, em ordem. Sem a pasta não há subagente; outra falha de leitura volta
/// como erro, para não parecer conversa sem subagentes.
pub fn subagent_ids(brain: &Path, conversation: &str) -> std::io::Result<Vec<String>> {
    if !valid_conversation_id(conversation) { return Ok(Vec::new()); }
    let entries = match std::fs::read_dir(subagents_dir(brain, conversation)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        if let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")).filter(|id| valid_conversation_id(id)) {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

/// O descritor (`{typeName, role}`) do subagente `child` lançado pela conversa `parent`; `None` quando `child`
/// não é subagente dela. O `agy` grava o descritor como objeto ou como texto JSON.
pub fn subagent_descriptor(brain: &Path, parent: &str, child: &str) -> Option<serde_json::Value> {
    if !valid_conversation_id(parent) || !valid_conversation_id(child) { return None; }
    let text = std::fs::read_to_string(subagents_dir(brain, parent).join(format!("{child}.json"))).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(match &value["subagentDescriptor"] {
        serde_json::Value::String(raw) => serde_json::from_str(raw).unwrap_or_default(),
        other => other.clone(),
    })
}

/// O nome do subagente para a tela: `role`, senão `typeName`.
pub fn subagent_name(descriptor: &serde_json::Value) -> Option<String> {
    ["role", "typeName"].iter().find_map(|key| descriptor[*key].as_str().filter(|s| !s.trim().is_empty())).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "904fd70e-847b-49ed-9999-a13d7a4d9a98";

    #[test]
    fn the_default_account_lives_in_dot_gemini() {
        let home = Path::new("/home/u");
        let path = Account::default().transcript_path(home, ID).unwrap();
        assert_eq!(path, Path::new("/home/u/.gemini/antigravity-cli/brain/904fd70e-847b-49ed-9999-a13d7a4d9a98/.system_generated/logs/transcript_full.jsonl"));
        assert_eq!(conversation_of_transcript(&path).as_deref(), Some(ID));
        assert_eq!(Account::default().hooks_path(home), Path::new("/home/u/.gemini/config/hooks.json"));
    }

    #[test]
    fn a_missing_subagent_folder_is_no_subagent_and_an_unreadable_one_is_an_error() {
        let brain = tempfile::tempdir().unwrap();
        assert_eq!(subagent_ids(brain.path(), ID).unwrap(), Vec::<String>::new());
        std::fs::create_dir_all(brain.path().join(ID).join(".system_generated")).unwrap();
        std::fs::write(subagents_dir(brain.path(), ID), "não é pasta").unwrap();
        assert!(subagent_ids(brain.path(), ID).is_err());
    }

    #[test]
    fn an_extra_account_uses_its_own_folder() {
        let account = Account::extra(PathBuf::from("/home/u/.antigravity-trabalho"));
        assert_eq!(account.settings_path(Path::new("/home/u")), Path::new("/home/u/.antigravity-trabalho/antigravity-cli/settings.json"));
    }

    #[test]
    fn the_workspace_comes_from_the_last_history_line_of_the_conversation() {
        let home = tempfile::tempdir().unwrap();
        let data = Account::default().data_dir(home.path());
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("history.jsonl"), format!(
            "{{\"display\":\"a\",\"workspace\":\"/w/old\",\"conversationId\":\"{ID}\"}}\n\
             {{\"display\":\"b\",\"workspace\":\"/w/other\",\"conversationId\":\"00000000-0000-0000-0000-000000000000\"}}\n\
             {{\"display\":\"c\",\"workspace\":\"/w/new\",\"conversationId\":\"{ID}\"}}\n")).unwrap();
        assert_eq!(workspace_of(&Account::default(), home.path(), ID).as_deref(), Some("/w/new"));
        assert_eq!(workspace_of(&Account::default(), home.path(), "11111111-1111-1111-1111-111111111111"), None);
    }

    #[test]
    fn a_conversation_id_that_is_not_a_uuid_never_becomes_a_path() {
        assert!(Account::default().transcript_path(Path::new("/h"), "../../etc").is_none());
        assert!(!valid_conversation_id("904FD70E-847b-49ed-9999-a13d7a4d9a98"));
        assert!(valid_conversation_id(ID));
    }
}
