// crates/hangar-server/src/search/agents.rs
//! Conversas de Codex, Pi, omp e Kimi no índice. A descoberta é a do Arquivo (`archive_list`) e as
//! linhas passam pelo parser do histórico dele, então o trecho abre no ponto. A conversa entra na
//! pasta do Claude do cwd dela, como no Arquivo; sem cwd, fica fora (o Arquivo também não a lista).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::ingest::{Found, Stat};
use crate::accounts::environment::AccountEnvironment;
use crate::archive_list::Unread;
use crate::speech::history::transfer_history as transfer;

/// Os transcripts dos outros agentes e o que não pôde ser varrido agora (o índice mantém o que já
/// tem lá).
pub(super) fn discover(env: &AccountEnvironment) -> (BTreeMap<String, Found>, Vec<Unread>) {
    // O índice é a fonte do que ele mesmo guarda: o cabeçalho sai sempre do disco.
    let (mut others, mut unread) = crate::archive_list::agent_conversations(env, &|_| None);
    let mut transfers = HashMap::new();
    match crate::archive_list::codex_conversations(env, &|_| None) {
        Ok((codex, bad)) => {
            unread.extend(bad);
            for c in codex {
                let Some(owner) = c.codex.as_ref() else { continue };
                match crate::list::local_transfers::thread_phase(&env.home, &owner.home.to_string_lossy(), &c.sid) {
                    Ok(None) => others.push(c),
                    // O transferido do Claude entra só depois da fronteira, com os ids do histórico composto;
                    // a parte Claude já está no índice pelo transcript dela.
                    Ok(Some(_)) => match transfer::archive_record(&env.home, &owner.home, &crate::locate::session_key(&c.path.to_string_lossy())) {
                        Ok(record) => {
                            if let Some(record) = record { transfers.insert(c.path.clone(), (record, owner.home.clone())); }
                            others.push(c);
                        }
                        Err(e) => unread.push(Unread::new("codex", Some(&c.path), e)),
                    },
                    Err(e) => unread.push(Unread::new("codex", Some(&c.path), e)),
                }
            }
        }
        Err(e) => unread.push(Unread::new("codex", None, e)),
    }
    let kimi = crate::provider_accounts::kimi_account(env).unwrap_or_default();
    let mut out = BTreeMap::new();
    for c in others {
        let Some(project) = crate::archive_list::project_of(c.cwd.as_deref()) else { continue };
        let Some(md) = metadata(c.provider, &c.path, &mut unread) else { continue };
        if !md.is_file() { continue; }
        // Pi e omp: a conta sai do modelo lido nas linhas (`ingest`).
        let account = match (c.provider, &c.codex) {
            ("codex", Some(owner)) => format!("codex:{}", crate::list::links::resolve_lenient(&owner.home).to_string_lossy()),
            ("kimi", _) => kimi.clone(),
            _ => String::new(),
        };
        let transfer = transfers.remove(&c.path);
        out.entry(c.path.to_string_lossy().into_owned()).or_insert(Found {
            project, sid: c.sid, stat: Stat::of(&md), provider: c.provider, config_dir: None, account,
            codex_account: c.codex.map(|owner| owner.id), cwd: c.cwd, transfer,
        });
    }
    (out, unread)
}

/// O stat do transcript; erro que não é ausência vai a `unread`, e o índice mantém o que tem dele.
fn metadata(provider: &'static str, path: &Path, unread: &mut Vec<Unread>) -> Option<std::fs::Metadata> {
    match std::fs::metadata(path) {
        Ok(md) => Some(md),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound { unread.push(Unread::new(provider, Some(path), e)); }
            None
        }
    }
}

/// As pastas a vigiar: a raiz do Pi, a do omp, a casa do Kimi e `sessions`/`archived_sessions` de
/// cada conta Codex.
pub(super) fn roots(env: &AccountEnvironment) -> Vec<PathBuf> {
    use crate::session_open::{AgentProvider, archive};
    let w = archive::Where { home: &env.home, env: &env.base, server_projects: std::path::Path::new(""), config_dirs: &[], live: &[] };
    let mut out = vec![archive::agent_root(&w, AgentProvider::Pi), archive::agent_root(&w, AgentProvider::Omp),
        crate::session_open::agents::kimi_home(&env.home, &env.base)];
    let service = crate::accounts::AccountService::new(env.clone());
    if let Ok(accounts) = service.snapshot(crate::accounts::Provider::Codex) {
        out.extend(accounts.iter().flat_map(|a| [a.home.join("sessions"), a.home.join("archived_sessions")]));
    }
    out
}

/// O provedor de uma troca de modelo do Pi (`provider`) ou do omp (`model: "<provedor>/<modelo>"`).
pub(super) fn model_provider(line: &[u8]) -> Option<String> {
    let obj: Value = serde_json::from_slice(line).ok()?;
    if obj["type"] != "model_change" { return None; }
    let provider = match obj["model"].as_str().and_then(|m| m.split_once('/')) {
        Some((provider, _)) => provider,
        None => obj["provider"].as_str()?,
    };
    (!provider.is_empty()).then(|| provider.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn stat_that_fails_for_another_reason_keeps_the_file() {
        // Erro que não é ausência vai a `unread`: a passada mantém o que o índice tem do arquivo.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("arquivo");
        std::fs::write(&file, "x").unwrap();
        let mut unread = Vec::new();
        assert!(metadata("pi", &file.join("dentro.jsonl"), &mut unread).is_none());
        assert_eq!(unread.len(), 1);
        assert!(unread[0].covers("pi", &file.join("dentro.jsonl").to_string_lossy()));
        assert!(metadata("pi", &dir.path().join("sumiu.jsonl"), &mut unread).is_none());
        assert_eq!(unread.len(), 1, "ausência não é falha");
        assert!(metadata("pi", &file, &mut unread).is_some());
    }
}
