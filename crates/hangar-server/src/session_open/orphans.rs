//! Windows: servidor do psmux vivo que perdeu a ficha em `~/.psmux`. O `ls` não o vê, então a sessão sumia da
//! lista com o agente rodando e segurava o nome (o `new-session` com ele falhava). Vira sessão caída, cujo Reabrir
//! e Dispensar o encerram antes, e o nome dele conta como ocupado.
use std::collections::BTreeSet;
#[cfg(windows)]
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hangar_psmux::Orphan;

use super::{AgentProvider, SessionSpec, Surface};

pub const CAUSE: &str = "psmux_orphan";

pub(super) fn lifecycle(fallen: &super::fallen::Fallen) -> Option<String> {
    (fallen.cause == CAUSE).then_some(())?;
    Some(format!("o:{}:{}:{}", fallen.key, fallen.orphan_pid?, fallen.orphan_birth?))
}

pub(super) fn accepts_action(fallen: &super::fallen::Fallen, requested: Option<&str>) -> bool {
    if fallen.cause == CAUSE { requested.is_some() && lifecycle(fallen).as_deref() == requested }
    else { requested.is_none() }
}

static NAMES: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// O nome que um servidor órfão segura: o psmux recusa outra sessão com ele.
pub fn holds(name: &str) -> bool { NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(name) }

#[cfg(windows)]
const SETTLE: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct Binding {
    key: String,
    provider: AgentProvider,
    birth: u64,
    descendants: Vec<(u32, u64)>,
}

#[cfg(windows)]
fn find() -> io::Result<Vec<(Orphan, Option<Binding>)>> { find_after(SETTLE) }

#[cfg(windows)]
fn find_after(settle: Duration) -> io::Result<Vec<(Orphan, Option<Binding>)>> {
    use crate::list::discover::{is_aux, provider_from_argv};
    use crate::list::procs::{ProcessView, SystemProcs, vanished};

    let dir = hangar_psmux::data_dir(|key| std::env::var_os(key))
        .ok_or_else(|| io::Error::other("pasta do psmux indisponível"))?;
    let found = hangar_psmux::scan_orphans(&dir, settle)?;
    if found.is_empty() { return Ok(Vec::new()); }

    let procs = SystemProcs::default();
    let mut result = Vec::new();
    for orphan in found {
        let Some(birth) = hangar_psmux::process_birth(orphan.pid)? else { continue; };
        let Ok(descendants) = hangar_psmux::process_tree(orphan.pid, birth) else {
            result.push((orphan, None));
            continue;
        };
        let pids: Vec<_> = descendants.iter().map(|&(pid, _)| i64::from(pid)).collect();
        procs.prefetch(&pids);
        let mut binding: Option<Binding> = None;
        for &(pid, started) in &descendants {
            if hangar_psmux::process_birth(pid)? != Some(started) { continue; }
            let process = i64::from(pid);
            let argv = match procs.argv_checked(process) {
                Ok(argv) => argv,
                Err(error) if vanished(&error) => continue,
                Err(_) => { binding = None; break; }
            };
            let Some(provider) = provider_from_argv(&argv).filter(|_| !is_aux(&argv)).and_then(AgentProvider::parse) else { continue; };
            let env = |name| procs.env_var(process, name).ok().flatten().and_then(|value| value.into_string().ok());
            let mux = env("TMUX").as_deref().and_then(hangar_psmux::pane_server_pid);
            let key = env("CP_SESSION_KEY");
            if hangar_psmux::process_birth(pid)? != Some(started) { continue; }
            let Some(key) = key.filter(|key| !key.is_empty() && mux == Some(orphan.pid)) else {
                binding = None;
                break;
            };
            if binding.as_ref().is_some_and(|previous| previous.key != key) {
                binding = None;
                break;
            }
            if binding.is_none() { binding = Some(Binding { key, provider, birth, descendants: descendants.clone() }); }
        }
        if hangar_psmux::process_birth(orphan.pid)? == Some(birth) { result.push((orphan, binding)); }
    }
    Ok(result)
}

fn proven_spec(home: &Path, orphan: &Orphan, binding: &Binding) -> Option<SessionSpec> {
    super::record::read(home, &binding.key).ok().flatten().filter(|spec| {
        spec.key == binding.key && spec.name == orphan.name && spec.surface == Surface::Terminal && spec.provider == binding.provider
    })
}

fn now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

fn register(home: &Path, found: &[(Orphan, Option<Binding>)], snapshot: &super::restore::Snapshot, diary: Option<&Path>) {
    let before = std::mem::replace(&mut *NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        found.iter().map(|(orphan, _)| orphan.name.clone()).collect());
    // Dispensado há pouco (o encerrar e a busca se cruzaram) não volta nesta passada.
    let since = Instant::now().checked_sub(Duration::from_secs(5)).unwrap_or_else(Instant::now);
    for (orphan, binding) in found {
        let Some((binding, spec)) = binding.as_ref().and_then(|binding| proven_spec(home, orphan, binding).map(|spec| (binding, spec))) else {
            if !before.contains(&orphan.name) {
                report(diary, Some(&orphan.name), "psmux_orphan_unbound", "servidor sem ficha e sem vínculo comprovado com uma sessão do Hangar");
            }
            continue;
        };
        if super::fallen::get(home, &orphan.name).is_some_and(|f| {
            f.key == binding.key && f.orphan_pid == Some(orphan.pid) && f.orphan_birth == Some(binding.birth)
        }) { continue; }
        let entry = snapshot.sessions.get(&spec.key).filter(|entry| entry.name == spec.name && entry.provider == spec.provider);
        let fallen = super::fallen::Fallen { key: spec.key, provider: spec.provider,
            conversation: entry.and_then(|entry| entry.conversation.clone()),
            permission_mode: entry.and_then(|entry| entry.permission_mode.clone()), cwd: Some(spec.cwd),
            died_at: now(), cause: CAUSE.into(), orphan_pid: Some(orphan.pid), orphan_birth: Some(binding.birth),
            orphan_descendants: binding.descendants.clone(), orphan_transcript: entry.and_then(|entry| entry.transcript.clone()) };
        if super::fallen::insert_unless_forgotten(home, &orphan.name, since, fallen) {
            tracing::warn!(code = "psmux_orphan_found", session = %orphan.name, pid = orphan.pid, "servidor do psmux sem a ficha; a sessão fica na lista para reabrir");
        }
    }
}

fn report(diary: Option<&Path>, name: Option<&str>, code: &'static str, detail: &str) {
    tracing::warn!(code, session = name, %detail);
    crate::diag::record_blocking(diary,
        &[serde_json::json!({"evento":"sessao.psmux_orfao", "nivel":"aviso", "sessao":name, "codigo":code, "detalhe":detail})], "varredura psmux");
}

pub async fn scan(_home: &Path, _snapshot: &super::restore::Snapshot) {
    #[cfg(windows)]
    match tokio::task::spawn_blocking(find).await {
        Ok(Ok(found)) => register(_home, &found, _snapshot, crate::diag::diary_dir().as_deref()),
        Ok(Err(_)) | Err(_) => {
            if crate::warn_limit::allow(None, "psmux_orphan_scan_failed") {
                report(crate::diag::diary_dir().as_deref(), None, "psmux_orphan_scan_failed", "não foi possível conferir os servidores órfãos do psmux");
            }
        }
    }
}

pub async fn end(home: &Path, name: &str, fallen: &super::fallen::Fallen) -> Result<(), &'static str> {
    #[cfg(not(windows))]
    { let _ = (home, name, fallen); Err("psmux_orphan_unknown") }
    #[cfg(windows)]
    {
        let (home, name, fallen) = (home.to_owned(), name.to_owned(), fallen.clone());
        tokio::task::spawn_blocking(move || end_checked(&home, &name, &fallen, SETTLE)).await.map_err(|_| "psmux_orphan_unknown")?
    }
}

#[cfg(windows)]
fn end_checked(home: &Path, name: &str, fallen: &super::fallen::Fallen, settle: Duration) -> Result<(), &'static str> {
    let (Some(pid), Some(birth)) = (fallen.orphan_pid, fallen.orphan_birth) else { return Err("psmux_orphan_unknown"); };
    let unknown = |_| "psmux_orphan_unknown";
    let mut lives: BTreeSet<_> = fallen.orphan_descendants.iter().copied().collect();
    if hangar_psmux::process_birth(pid).map_err(unknown)? == Some(birth) {
        let binding = find_after(settle).map_err(unknown)?.into_iter().find_map(|(orphan, binding)| {
            binding.filter(|binding| orphan.pid == pid && orphan.name == name
                && binding.birth == birth && binding.key == fallen.key && binding.provider == fallen.provider)
        }).ok_or("psmux_orphan_unknown")?;
        lives.extend(binding.descendants);
    }
    let mut lives: Vec<_> = lives.into_iter().collect();
    if !super::fallen::remember_orphan_descendants(home, name, fallen, &lives) {
        return Err("psmux_orphan_unknown");
    }
    let registry = hangar_psmux::data_dir(|key| std::env::var_os(key)).ok_or("psmux_orphan_unknown")?;
    if hangar_psmux::process_birth(pid).map_err(unknown)? == Some(birth)
        && registry.join(format!("{name}.port")).try_exists().map_err(unknown)? {
        return Err("psmux_orphan_unknown");
    }
    lives.insert(0, (pid, birth));
    if let Err(error) = hangar_psmux::stop_processes(&lives, Duration::from_secs(10)) {
        report(crate::diag::diary_dir().as_deref(), Some(name), "psmux_orphan_not_ended", &error.to_string());
        return Err("psmux_orphan_not_ended");
    }
    NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(name);
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_test;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_open::{fallen, record};

    #[test]
    fn only_an_orphan_bound_to_the_spec_becomes_actionable() {
        let home = tempfile::tempdir().unwrap();
        let spec = serde_json::from_value(serde_json::json!({"version": 1, "key": "f54d0180be54162a529ce67d5c55114a", "name": "hangar",
            "cwd": "C:\\w\\erro", "provider": "claude", "surface": "terminal", "account": {"kind": "default"}, "account_source": "default",
            "model": "opus", "effort": "high", "service_tier": null, "permission_mode": "bypassPermissions", "subagent_model": null,
            "customizations": null, "jev": true, "read_only": false, "worktree": null, "creator": null,
            "created_at": 1794831567.0, "conversation": "8b190b09-d66f-4069-a5d0-331ce61c866c", "name_auto": true})).unwrap();
        record::write(home.path(), &spec).unwrap();
        let key = "f54d0180be54162a529ce67d5c55114a";
        let conversation = "8b190b09-d66f-4069-a5d0-331ce61c866c";
        let projects = home.path().join("projects");
        let transcript = super::super::claude::transcript(&spec, conversation, &projects);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, "{}\n").unwrap();
        let snapshot = super::super::restore::Snapshot { mux_started: None, sessions: [(key.into(), super::super::restore::Entry {
            name: "hangar".into(), provider: AgentProvider::Claude, conversation: Some(conversation.into()),
            transcript: Some(transcript.to_string_lossy().into_owned()), permission_mode: Some("acceptEdits".into()), missing_since: None, unit: None,
        })].into() };
        let orphan = Orphan { name: "hangar".into(), pid: 9032 };
        let diary = home.path().join("diary");
        register(home.path(), &[(orphan.clone(), None)], &snapshot, Some(&diary));
        assert!(holds("hangar") && fallen::get(home.path(), "hangar").is_none());

        register(home.path(), &[
            (orphan, Some(Binding { key: key.into(), provider: AgentProvider::Claude, birth: 42, descendants: vec![(9033, 43)] })),
            (Orphan { name: "sem-spec".into(), pid: 7 }, None),
        ], &snapshot, Some(&diary));

        let fallen = fallen::get(home.path(), "hangar").expect("o órfão com vínculo vira caída");
        assert_eq!((fallen.key.as_str(), fallen.cause.as_str(), fallen.orphan_pid, fallen.orphan_birth, fallen.problem()),
            (key, CAUSE, Some(9032), Some(42), fallen::OTHER));
        assert_eq!(fallen.conversation.as_deref(), Some("8b190b09-d66f-4069-a5d0-331ce61c866c"));
        assert_eq!(fallen.permission_mode.as_deref(), Some("acceptEdits"));
        let mut entry = fallen.entry("hangar");
        let resolve = |entry: &super::super::restore::Entry| super::super::restore::resolve_orphan(home.path(), &projects, key, entry);
        let resolved = resolve(&entry).unwrap();
        assert_eq!(resolved.resume.as_deref(), Some(conversation));
        assert_eq!(resolved.spec.permission_mode, Some(super::super::PermissionMode::AcceptEdits));
        entry.permission_mode = None;
        assert_eq!(resolve(&entry).err().as_deref(), Some("restore_permission_unconfirmed"));
        entry.conversation = None;
        assert_eq!(resolve(&entry).err().as_deref(), Some("restore_no_conversation"));
        let published = fallen::rows(home.path(), &[]).remove(0).lifecycle_id;
        assert!(accepts_action(&fallen, published.as_deref()));
        assert!(!accepts_action(&fallen, None));
        let replaced = fallen::Fallen { orphan_birth: Some(43), ..fallen.clone() };
        assert!(!accepts_action(&replaced, published.as_deref()));
        let rebound = fallen::Fallen { key: "other".into(), ..fallen };
        assert!(!accepts_action(&rebound, published.as_deref()));
        assert!(fallen::get(home.path(), "sem-spec").is_none(), "sem vínculo não há o que reabrir");
        assert!(holds("hangar") && holds("sem-spec"), "os dois seguram o nome no psmux");

        register(home.path(), &[], &snapshot, Some(&diary));
        assert!(!holds("hangar"), "encerrado, o nome volta a ficar livre");
    }
}
