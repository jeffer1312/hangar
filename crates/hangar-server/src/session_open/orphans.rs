//! Windows: servidor do psmux vivo que perdeu a ficha em `~/.psmux`. O `ls` não o vê, então a sessão sumia da
//! lista com o agente rodando e segurava o nome (o `new-session` com ele falhava). Vira sessão caída, cujo Reabrir
//! e Dispensar o encerram antes, e o nome dele conta como ocupado.
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use hangar_psmux::Orphan;

pub const CAUSE: &str = "psmux_orphan";

static NAMES: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// O nome que um servidor órfão segura: o psmux recusa outra sessão com ele.
pub fn holds(name: &str) -> bool { NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(name) }

/// O servidor recém-aberto ainda não gravou a ficha: só conta o que já passou disto.
const SETTLE: Duration = Duration::from_secs(60);

fn now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

/// Os servidores órfãos da máquina agora; `None` sem a pasta de dados do psmux.
fn find() -> Option<Vec<Orphan>> {
    let dir = hangar_psmux::data_dir(|key| std::env::var_os(key))?;
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always));
    let now = now() as u64;
    let old = system.processes().iter().filter(|(_, p)| now.saturating_sub(p.start_time()) >= SETTLE.as_secs())
        .map(|(pid, p)| (pid.as_u32(), p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect()));
    Some(hangar_psmux::orphans(&dir, old))
}

/// Cada órfão com spec vira sessão caída, e todos seguram o nome.
pub fn register(home: &Path, found: &[Orphan]) {
    let before = std::mem::replace(&mut *NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        found.iter().map(|o| o.name.clone()).collect());
    // Dispensado há pouco (o encerrar e a busca se cruzaram) não volta nesta passada.
    let since = Instant::now().checked_sub(Duration::from_secs(5)).unwrap_or_else(Instant::now);
    for orphan in found {
        if super::fallen::get(home, &orphan.name).is_some_and(|f| f.orphan_pid == Some(orphan.pid)) { continue; }
        let Some(spec) = super::record::newest_named(home, &orphan.name) else {
            if !before.contains(&orphan.name) {
                tracing::warn!(code = "psmux_orphan_no_spec", session = %orphan.name, pid = orphan.pid,
                    "servidor do psmux sem a ficha e sem registro da sessão; o nome fica ocupado até ele sair");
            }
            continue;
        };
        let fallen = super::fallen::Fallen { key: spec.key, provider: spec.provider, conversation: spec.conversation, permission_mode: None,
            cwd: Some(spec.cwd), died_at: now(), cause: CAUSE.into(), orphan_pid: Some(orphan.pid) };
        if super::fallen::insert_unless_forgotten(home, &orphan.name, since, fallen) {
            tracing::warn!(code = "psmux_orphan_found", session = %orphan.name, pid = orphan.pid, "servidor do psmux sem a ficha; a sessão fica na lista para reabrir");
        }
    }
}

/// Uma passada, a cada tique do restore. Só Windows: no Linux o tmux não perde a ficha do servidor.
pub async fn scan(home: &Path) {
    if !cfg!(windows) { return; }
    match tokio::task::spawn_blocking(find).await {
        Ok(Some(found)) => register(home, &found),
        Ok(None) => {}
        Err(error) => tracing::warn!(code = "psmux_orphan_scan_failed", %error, "a busca dos servidores órfãos do psmux caiu"),
    }
}

/// Encerra o órfão, com o agente dentro, se o pid ainda é o servidor desse nome. Quem decide é a conferência
/// depois, não o código de retorno do `taskkill`.
pub async fn end(pid: u32, name: &str) -> Result<(), &'static str> {
    let alive = || async { tokio::task::spawn_blocking(find).await.ok().flatten().map(|f| f.iter().any(|o| o.pid == pid && o.name == name)) };
    let freed = || { NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(name); Ok(()) };
    if !alive().await.ok_or("psmux_orphan_unknown")? { return freed(); }
    let mut command = tokio::process::Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T", "/F"]).stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    match tokio::time::timeout(Duration::from_secs(10), command.status()).await {
        Err(_) => tracing::warn!(code = "psmux_orphan_kill_timeout", session = %name, pid, "taskkill passou de 10 s"),
        Ok(Err(error)) => tracing::warn!(code = "psmux_orphan_kill_failed", session = %name, pid, %error, "taskkill não rodou"),
        Ok(Ok(status)) if !status.success() => tracing::info!(session = %name, pid, code = ?status.code(), "taskkill saiu com erro; vale a conferência"),
        Ok(Ok(_)) => {}
    }
    match alive().await {
        Some(false) => freed(),
        _ => Err("psmux_orphan_not_ended"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_open::{fallen, record};

    #[test]
    fn an_orphan_with_a_spec_becomes_a_fallen_session_and_holds_its_name() {
        let home = tempfile::tempdir().unwrap();
        let spec = serde_json::from_value(serde_json::json!({"version": 1, "key": "f54d0180be54162a529ce67d5c55114a", "name": "hangar",
            "cwd": "C:\\w\\erro", "provider": "claude", "surface": "terminal", "account": {"kind": "default"}, "account_source": "default",
            "model": "opus", "effort": "high", "service_tier": null, "permission_mode": "bypassPermissions", "subagent_model": null,
            "customizations": null, "jev": true, "read_only": false, "worktree": null, "creator": null,
            "created_at": 1794831567.0, "conversation": "8b190b09-d66f-4069-a5d0-331ce61c866c", "name_auto": true})).unwrap();
        record::write(home.path(), &spec).unwrap();
        register(home.path(), &[Orphan { name: "hangar".into(), pid: 9032 }, Orphan { name: "sem-spec".into(), pid: 7 }]);

        let fallen = fallen::get(home.path(), "hangar").expect("o órfão com spec vira caída");
        assert_eq!((fallen.key.as_str(), fallen.cause.as_str(), fallen.orphan_pid, fallen.conversation.as_deref(), fallen.problem()),
            ("f54d0180be54162a529ce67d5c55114a", CAUSE, Some(9032), Some("8b190b09-d66f-4069-a5d0-331ce61c866c"), fallen::OTHER));
        assert!(fallen::get(home.path(), "sem-spec").is_none(), "sem spec não há o que reabrir");
        assert!(holds("hangar") && holds("sem-spec"), "os dois seguram o nome no psmux");

        register(home.path(), &[]);
        assert!(!holds("hangar"), "encerrado, o nome volta a ficar livre");
    }
}
