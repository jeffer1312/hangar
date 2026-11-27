//! Os nomes que a limpeza tira das linhas de log: toda sessão vista hoje e ontem (não só as abertas,
//! senão a linha de uma sessão que não abriu ou fechou levaria o nome), os specs, as contas, o
//! usuário do sistema, esta máquina e as do `peers.json`. Releitura a cada 30 s no máximo; leitura que
//! falha mantém o último conjunto bom.
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::telemetry_scrub::{Names, Words};

pub(crate) const REFRESH: Duration = Duration::from_secs(30);

/// O que não vem da lista de sessões: specs, contas e máquinas.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Local {
    pub(crate) specs: Vec<String>,
    pub(crate) accounts: Vec<String>,
    pub(crate) machines: Vec<String>,
}

/// O tempo é o decorrido desde o início da tarefa: quem chama mede, o teste passa o valor.
#[derive(Default)]
pub(crate) struct NameBook {
    /// Dia UTC → nomes de sessão vistos nos retratos; só os dois dias mais novos.
    seen: BTreeMap<String, BTreeSet<String>>,
    listed: bool,
    local: Option<Local>,
    read_at: Option<Duration>,
    words: Option<Arc<Words>>,
}

impl NameBook {
    /// Hora de reler specs, contas e máquinas (e de pedir um retrato, sem nenhum pronto).
    pub(crate) fn due(&self, now: Duration) -> bool { self.read_at.is_none_or(|at| now.saturating_sub(at) >= REFRESH) }

    /// Os nomes de um retrato da lista.
    pub(crate) fn see(&mut self, today: &str, sessions: impl IntoIterator<Item = String>) {
        let first = !self.listed;
        self.listed = true;
        let day = self.seen.entry(today.to_owned()).or_default();
        let mut grew = false;
        for name in sessions {
            grew |= day.insert(name);
        }
        while self.seen.len() > 2 {
            self.seen.pop_first();
        }
        self.rebuild_if(first || grew);
    }

    /// Resultado de uma releitura; `None` (falhou) mantém o último bom.
    pub(crate) fn read(&mut self, now: Duration, local: Option<Local>) {
        self.read_at = Some(now);
        if let Some(local) = local {
            let changed = self.local.as_ref() != Some(&local);
            self.local = Some(local);
            self.rebuild_if(changed);
        }
    }

    /// `None` enquanto não houve uma leitura boa da lista e das outras fontes: a linha não sai.
    pub(crate) fn words(&self) -> Option<Arc<Words>> { self.words.clone() }

    fn rebuild_if(&mut self, changed: bool) {
        let Some(local) = self.local.as_ref().filter(|_| self.listed && (changed || self.words.is_none())) else { return };
        let mut sessions: BTreeSet<String> = self.seen.values().flatten().cloned().collect();
        sessions.extend(local.specs.iter().cloned());
        let names = Names { sessions: sessions.into_iter().collect(), accounts: local.accounts.clone(), machines: local.machines.clone() };
        self.words = Some(Arc::new(names.words()));
    }
}

/// Specs, contas (pastas `.claude-<nome>` e `.codex-<nome>`, rótulos do `tags.json`), usuário do
/// sistema e máquinas. Pasta que não se lê é `Err`; spec ou `tags.json` ilegível só fica de fora. Bloqueante.
pub(crate) fn read_local(home: &Path, peers: impl IntoIterator<Item = String>, server_id: &str, host: Option<String>) -> io::Result<Local> {
    let mut specs = Vec::new();
    match std::fs::read_dir(crate::session_open::record::dir(home)) {
        Ok(entries) => for entry in entries {
            let raw = std::fs::read(entry?.path()).unwrap_or_default();
            if let Some(name) = serde_json::from_slice::<serde_json::Value>(&raw).ok().and_then(|spec| spec["name"].as_str().map(str::to_owned)) {
                specs.push(name);
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut accounts = Vec::new();
    for entry in std::fs::read_dir(home)? {
        let entry = entry?;
        let file = entry.file_name();
        let Some(file) = file.to_str() else { continue };
        let Some((provider, id)) = [".claude-", ".codex-"].iter().find_map(|p| file.strip_prefix(p).map(|id| (&p[1..], id))) else { continue };
        if entry.file_type()?.is_dir() {
            accounts.push(id.to_owned());
            accounts.push(format!("{provider}{id}"));
        }
    }
    let mut machines: Vec<String> = peers.into_iter().collect();
    machines.push(server_id.to_owned());
    machines.extend(host);
    let tags = std::fs::read(home.join(".hangar").join("tags.json")).ok().and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok());
    if let Some(tags) = tags {
        machines.extend(tags["machine"]["name"].as_str().map(str::to_owned));
        if let Some(labels) = tags["accounts"].as_object() {
            accounts.extend(labels.values().filter_map(|tag| tag["name"].as_str().map(str::to_owned)));
        }
    }
    for key in ["USER", "USERNAME", "LOGNAME"] {
        accounts.extend(std::env::var(key).ok());
    }
    accounts.extend(home.file_name().and_then(|n| n.to_str()).map(str::to_owned));
    Ok(Local { specs, accounts, machines })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag_report::Scrubber;
    use crate::telemetry_scrub::clean;

    fn local(machines: &[&str]) -> Local { Local { machines: machines.iter().map(|m| m.to_string()).collect(), ..Local::default() } }

    fn cleaned(book: &NameBook, text: &str) -> String { clean(&Scrubber::new(None, None), text, &book.words().unwrap()) }

    #[test]
    fn a_session_seen_once_stays_hidden_after_it_closes() {
        let mut book = NameBook::default();
        book.read(Duration::ZERO, Some(local(&["estudio-02"])));
        assert!(book.words().is_none(), "sem nenhum retrato da lista, nada sai");
        book.see("2026-11-26", ["sessao-de-ontem".to_owned()]);
        book.see("2026-11-27", ["sessao-aberta".to_owned()]);
        book.see("2026-11-27", Vec::new());
        assert_eq!(cleaned(&book, "sessao-aberta e sessao-de-ontem fecharam em estudio-02"), "<sessão> e <sessão> fecharam em <máquina>");
        book.see("2026-11-28", ["nova-sessao".to_owned()]);
        assert_eq!(cleaned(&book, "sessao-de-ontem e sessao-aberta"), "sessao-de-ontem e <sessão>", "dois dias guardados");
    }

    #[test]
    fn a_failed_read_keeps_the_last_good_set_and_waits_30s() {
        let mut book = NameBook::default();
        assert!(book.due(Duration::ZERO));
        book.see("2026-11-27", Vec::new());
        book.read(Duration::ZERO, None);
        assert!(book.words().is_none(), "sem conjunto bom, a linha não sai");
        assert!(!book.due(Duration::from_secs(29)));
        assert!(book.due(REFRESH));
        book.read(REFRESH, Some(local(&["estudio-02"])));
        book.read(REFRESH * 2, None);
        assert_eq!(cleaned(&book, "peer estudio-02"), "peer <máquina>");
    }

    #[test]
    fn local_names_come_from_specs_accounts_tags_and_the_home() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("fulano");
        std::fs::create_dir_all(home.join(".hangar/session-specs")).unwrap();
        std::fs::write(home.join(".hangar/session-specs/a.json"), r#"{"key": "a", "name": "sessao-que-nao-abriu"}"#).unwrap();
        std::fs::write(home.join(".hangar/session-specs/b.json"), "{").unwrap();
        std::fs::create_dir_all(home.join(".claude-claude-200-5")).unwrap();
        std::fs::create_dir_all(home.join(".codex-trabalho")).unwrap();
        std::fs::write(home.join(".claude-x.lock"), "").unwrap();
        std::fs::write(home.join(".hangar/tags.json"), r#"{"machine": {"name": "Casa"}, "accounts": {"claude:/x": {"name": "Pessoal"}}}"#).unwrap();
        let local = read_local(&home, ["estudio-02".to_owned()], "lab", Some("notebook-fulano".into())).unwrap();
        assert_eq!(local.specs, ["sessao-que-nao-abriu"]);
        for account in ["claude-200-5", "claude-claude-200-5", "trabalho", "codex-trabalho", "Pessoal", "fulano"] {
            assert!(local.accounts.iter().any(|a| a == account), "{account}: {:?}", local.accounts);
        }
        assert!(!local.accounts.iter().any(|a| a.contains("lock")));
        assert_eq!(local.machines, ["estudio-02", "lab", "notebook-fulano", "Casa"]);
        assert!(read_local(&root.path().join("sem-casa"), Vec::new(), "", None).is_err());
    }
}
