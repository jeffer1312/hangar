//! `hangar-server codex-close <nome>`: a limpeza que o `hangar-codex` do terminal pede quando a sessão
//! que ele abriu acabou. Pede ao servidor (`DELETE /api/sessions/<nome>`); sem servidor, sem token ou
//! com recusa, limpa aqui: derruba o app-server só se o pid ainda é o dele e apaga o arquivo da sessão
//! só se ele ainda é daquela vida (endpoint, pid e chave).
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use percent_encoding::utf8_percent_encode;
use serde_json::Value;

use crate::accounts::environment::AccountEnvironment;
use crate::runtime::codex_sidecar;
use crate::runtime::codex_terminal::{AppServer, app_server};

/// O fechamento no servidor espera o processo sair.
const DELETE_TIMEOUT: Duration = Duration::from_secs(30);

/// `args` sem o nome do subcomando: `[nome]`.
pub fn cli(args: &[String]) -> ExitCode {
    let [name] = args else {
        eprintln!("uso: hangar-server codex-close <nome>");
        return ExitCode::from(2);
    };
    let env = super::pane_environment();
    let home = AccountEnvironment::from_pairs(env.iter().cloned()).home;
    let server = super::prepare::server_of(&env, &home);
    let deleted = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime.block_on(delete(&server, name)),
        Err(_) => false,
    };
    if deleted {
        return ExitCode::SUCCESS;
    }
    match local(&codex_sidecar::dir(&home), name) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hangar-codex: o arquivo da sessão {name} não foi removido ({:?})", error.kind());
            ExitCode::from(1)
        }
    }
}

/// O servidor fecha a sessão inteira (processos, fila, convite); `false` em qualquer falha.
async fn delete(server: &super::prepare::Server, name: &str) -> bool {
    let (Some(token), Ok(port)) = (server.token.as_deref(), server.port.trim().parse::<u16>()) else { return false };
    let Ok(client) = crate::http_client().timeout(DELETE_TIMEOUT).build() else { return false };
    let url = format!("http://127.0.0.1:{port}/api/sessions/{}", utf8_percent_encode(name, super::prepare::QUOTE));
    client.delete(url).bearer_auth(token).send().await.is_ok_and(|response| response.status().is_success())
}

/// Sem o servidor: o lançador normalmente já limpou ao ver a TUI sair; isto cobre o pane derrubado sem
/// ele chegar lá. O pid reaproveitado por outro processo não leva sinal.
pub(crate) fn local(dir: &Path, name: &str) -> std::io::Result<()> {
    let Some(meta) = codex_sidecar::load(&codex_sidecar::path(dir, name)) else { return Ok(()) };
    let endpoint = meta.get("endpoint").and_then(Value::as_str).filter(|endpoint| !endpoint.is_empty());
    let pid = meta.get("app_pid").and_then(Value::as_u64).and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0);
    let (Some(endpoint), Some(pid)) = (endpoint, pid) else { return Ok(()) };
    if app_server(pid, endpoint) == AppServer::Ours {
        terminate(pid);
    }
    let key = meta.get("key").and_then(Value::as_str).filter(|key| !key.is_empty());
    codex_sidecar::delete_if_life(dir, name, endpoint, pid, key).map(|_| ())
}

#[cfg(unix)]
fn terminate(pid: u32) {
    // SAFETY: o pid foi conferido como o app-server deste endereço logo acima.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
}

/// No Windows o Job do lançador já leva o app-server quando o lançador morre.
#[cfg(not(unix))]
fn terminate(_pid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn put(dir: &Path, meta: Value) { codex_sidecar::put(dir, "codex-proj", meta.as_object().unwrap()).unwrap(); }

    /// `hangar-codex__local-cleanup-app-pid-dead`: app-server já morto, o arquivo daquela vida sai.
    #[cfg(unix)]
    #[test]
    fn a_dead_app_server_only_loses_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut dead = std::process::Command::new("true").spawn().unwrap();
        let pid = dead.id();
        dead.wait().unwrap();
        put(dir.path(), json!({"name": "codex-proj", "endpoint": "ws://127.0.0.1:1", "app_pid": pid, "key": "k"}));
        local(dir.path(), "codex-proj").unwrap();
        assert!(!codex_sidecar::path(dir.path(), "codex-proj").exists());
        local(dir.path(), "codex-proj").unwrap();
    }

    /// O pid que agora é de outro programa não leva sinal; o arquivo velho sai igual. Sem endpoint ou
    /// pid não há vida a conferir, e o arquivo fica para o servidor.
    #[test]
    fn a_reused_pid_gets_no_signal() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), json!({"name": "codex-proj", "endpoint": "ws://127.0.0.1:1", "app_pid": std::process::id()}));
        local(dir.path(), "codex-proj").unwrap();
        assert!(!codex_sidecar::path(dir.path(), "codex-proj").exists(), "o teste segue vivo e o arquivo saiu");
        put(dir.path(), json!({"name": "codex-proj", "endpoint": null, "app_pid": null}));
        local(dir.path(), "codex-proj").unwrap();
        assert!(codex_sidecar::path(dir.path(), "codex-proj").exists());
    }

    /// `hangar-codex__delete-api-down-local-cleanup`: o app-server vivo daquele endereço leva SIGTERM.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_live_app_server_of_that_address_is_terminated() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = "ws://127.0.0.1:65000";
        let mut server = std::process::Command::new("/bin/sh").args(["-c", "while :; do sleep 1; done", "app-server", endpoint]).spawn().unwrap();
        // O argv chega ao /proc depois do exec.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while app_server(server.id(), endpoint) != AppServer::Ours && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        put(dir.path(), json!({"name": "codex-proj", "endpoint": endpoint, "app_pid": server.id(), "key": "k"}));
        local(dir.path(), "codex-proj").unwrap();
        let status = server.wait().unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGTERM));
        assert!(!codex_sidecar::path(dir.path(), "codex-proj").exists());
    }
}
