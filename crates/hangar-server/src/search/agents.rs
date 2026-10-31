// crates/hangar-server/src/search/agents.rs
//! Conversas de Codex, Pi, omp e Kimi no índice. A descoberta é a do Arquivo (`archive_list`) e as
//! linhas passam pelo parser do histórico dele, então o trecho abre no ponto. A conversa entra na
//! pasta do Claude do cwd dela, como no Arquivo; sem cwd, fica fora (o Arquivo também não a lista).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use super::ingest::{Found, Stat};
use crate::accounts::environment::AccountEnvironment;
use crate::archive_list::Unread;

/// Os transcripts dos outros agentes e o que não pôde ser varrido agora (o índice mantém o que já
/// tem lá).
pub(super) fn discover(env: &AccountEnvironment) -> (BTreeMap<String, Found>, Vec<Unread>) {
    // O índice é a fonte do que ele mesmo guarda: o cabeçalho sai sempre do disco.
    let (mut others, mut unread) = crate::archive_list::agent_conversations(env, &|_| None);
    match crate::archive_list::codex_conversations(env, &|_| None) {
        Ok((codex, bad)) => {
            unread.extend(bad);
            for c in codex {
                let Some(owner) = c.codex.as_ref() else { continue };
                match crate::list::local_transfers::thread_phase(&env.home, &owner.home.to_string_lossy(), &c.sid) {
                    Ok(None) => others.push(c),
                    // O transferido do Claude sai inteiro: o histórico dele é composto com a conversa de
                    // origem e não se lê do offset; a parte Claude já está no índice pelo transcript dela.
                    Ok(Some(_)) => {}
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
        let Ok(md) = std::fs::metadata(&c.path) else { continue };
        if !md.is_file() { continue; }
        // Pi e omp: a conta sai do modelo lido nas linhas (`ingest`).
        let account = match (c.provider, &c.codex) {
            ("codex", Some(owner)) => format!("codex:{}", crate::list::links::resolve_lenient(&owner.home).to_string_lossy()),
            ("kimi", _) => kimi.clone(),
            _ => String::new(),
        };
        out.entry(c.path.to_string_lossy().into_owned()).or_insert(Found {
            project, sid: c.sid, stat: Stat::of(&md), provider: c.provider, config_dir: None, account,
            codex_account: c.codex.map(|owner| owner.id), cwd: c.cwd,
        });
    }
    (out, unread)
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
