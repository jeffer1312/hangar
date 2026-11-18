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
        Err(_) => Deleted::Unreachable,
    };
    match deleted {
        Deleted::Yes => return ExitCode::SUCCESS,
        Deleted::Refused(reason) => {
            // O servidor respondeu e não fechou (ex.: 500 session_kill_unconfirmed): o arquivo fica para
            // ele achar a sessão de novo, e o motivo aparece.
            eprintln!("hangar-codex: o servidor não fechou a sessão {name}: {reason}");
            return ExitCode::from(1);
        }
        Deleted::Unreachable => {}
    }
    match local(&codex_sidecar::dir(&home), name) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hangar-codex: o arquivo da sessão {name} não foi removido ({:?})", error.kind());
            ExitCode::from(1)
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Deleted {
    Yes,
    /// Sem token, sem porta, sem conexão, sem resposta no prazo, ou 404 (o servidor não conhece a
    /// sessão): a limpeza é local.
    Unreachable,
    /// O servidor respondeu e não fechou: status e código dele.
    Refused(String),
}

/// O servidor fecha a sessão inteira (processos, fila, convite).
pub(crate) async fn delete(server: &super::prepare::Server, name: &str) -> Deleted {
    let (Some(token), Ok(port)) = (server.token.as_deref(), server.port.trim().parse::<u16>()) else { return Deleted::Unreachable };
    let Ok(client) = crate::http_client().timeout(DELETE_TIMEOUT).build() else { return Deleted::Unreachable };
    let url = format!("http://127.0.0.1:{port}/api/sessions/{}", utf8_percent_encode(name, super::prepare::QUOTE));
    let Ok(response) = client.delete(url).bearer_auth(token).send().await else { return Deleted::Unreachable };
    let status = response.status();
    if status.is_success() {
        return Deleted::Yes;
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Deleted::Unreachable;
    }
    let body: Value = response.bytes().await.ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null);
    let code = body["detail"]["code"].as_str().or_else(|| body["detail"].as_str()).unwrap_or("");
    Deleted::Refused(if code.is_empty() { format!("HTTP {}", status.as_u16()) } else { format!("HTTP {} {code}", status.as_u16()) })
}

/// Sem o servidor: o lançador normalmente já limpou ao ver a TUI sair; isto cobre o pane derrubado sem
/// ele chegar lá. O pid reaproveitado por outro processo não leva sinal.
pub(crate) fn local(dir: &Path, name: &str) -> std::io::Result<()> {
    let Some(meta) = codex_sidecar::load(&codex_sidecar::path(dir, name)) else { return Ok(()) };
    let endpoint = meta.get("endpoint").and_then(Value::as_str).filter(|endpoint| !endpoint.is_empty());
    let pid = meta.get("app_pid").and_then(Value::as_u64).and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0);
    let (Some(endpoint), Some(pid)) = (endpoint, pid) else { return Ok(()) };
    if app_server(pid, endpoint) == AppServer::Ours && !terminate(pid, endpoint) {
        // Arquivo apagado com o processo de pé deixaria um app-server escutando que ninguém acha.
        return Err(std::io::Error::other("o app-server da sessão continua vivo"));
    }
    let key = meta.get("key").and_then(Value::as_str).filter(|key| !key.is_empty());
    codex_sidecar::delete_if_life(dir, name, endpoint, pid, key).map(|_| ())
}

/// Prazo para o app-server sair depois do sinal, o mesmo nos dois sistemas.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// SIGTERM e espera ele sair. `false` quando o sinal foi recusado ou ele seguiu vivo no prazo.
#[cfg(unix)]
fn terminate(pid: u32, endpoint: &str) -> bool {
    // SAFETY: o pid foi conferido como o app-server deste endereço logo acima.
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        // ESRCH: saiu entre a conferência e o sinal. EPERM e outros: o processo segue lá.
        return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    // Saiu = o pid não é mais o app-server deste endereço (filho zumbi também não tem mais o argv).
    let deadline = std::time::Instant::now() + EXIT_WAIT;
    while app_server(pid, endpoint) == AppServer::Ours {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// O lançador morto por fora leva o app-server pelo Job; vivo (ou sem Job), só o `TerminateProcess`.
/// `false` quando o processo não saiu dentro do prazo.
#[cfg(windows)]
fn terminate(pid: u32, _endpoint: &str) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};
    // SAFETY: o pid foi conferido como o app-server deste endereço logo acima; o handle é fechado aqui.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            // Sem handle: o processo já saiu entre a conferência e aqui (ou não é mais abrível).
            return app_server_gone(pid);
        }
        TerminateProcess(process, 1);
        let gone = WaitForSingleObject(process, 5_000) == WAIT_OBJECT_0;
        CloseHandle(process);
        gone
    }
}

#[cfg(windows)]
fn app_server_gone(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: só abre para consulta e fecha logo.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return true;
        }
        windows_sys::Win32::Foundation::CloseHandle(process);
        false
    }
}

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

    /// Revisão de falhas caladas: o app-server que ignora o SIGTERM segue vivo no prazo, e o arquivo da
    /// sessão fica (sem ele, ninguém acharia o processo que continua escutando).
    #[cfg(target_os = "linux")]
    #[test]
    fn an_app_server_that_survives_the_signal_keeps_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = "ws://127.0.0.1:65001";
        // O argv já é o do app-server antes do `trap`: o sinal só pode sair depois do marcador.
        let ready = dir.path().join("trap-ready");
        let mut server = std::process::Command::new("/bin/sh")
            .args(["-c", "trap '' TERM; : > \"$2\"; while :; do sleep 1; done", "app-server", endpoint])
            .arg(&ready)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while (!ready.exists() || app_server(server.id(), endpoint) != AppServer::Ours) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        put(dir.path(), json!({"name": "codex-proj", "endpoint": endpoint, "app_pid": server.id(), "key": "k"}));
        let result = local(dir.path(), "codex-proj");
        server.kill().unwrap();
        server.wait().unwrap();
        assert!(result.is_err(), "o processo seguiu vivo e a limpeza disse que deu certo");
        assert!(codex_sidecar::path(dir.path(), "codex-proj").exists());
    }

    async fn answer_once(status: &'static str, body: &'static str) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut request).await;
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, reply.as_bytes()).await;
        });
        port
    }

    /// Revisão de falhas caladas: só servidor fora (ou que não conhece a sessão) leva à limpeza local;
    /// a recusa dele (500 `session_kill_unconfirmed`) volta com o status e o código.
    #[tokio::test]
    async fn only_an_unreachable_server_falls_back_to_the_local_cleanup() {
        let server = |port: u16| super::super::prepare::Server { token: Some("t".into()), port: port.to_string() };
        let port = answer_once("500 Internal Server Error", r#"{"detail":{"code":"session_kill_unconfirmed"}}"#).await;
        assert_eq!(delete(&server(port), "codex-proj").await, Deleted::Refused("HTTP 500 session_kill_unconfirmed".into()));
        let port = answer_once("404 Not Found", "{}").await;
        assert_eq!(delete(&server(port), "codex-proj").await, Deleted::Unreachable);
        let port = answer_once("200 OK", "{}").await;
        assert_eq!(delete(&server(port), "codex-proj").await, Deleted::Yes);
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(delete(&server(dead), "codex-proj").await, Deleted::Unreachable);
        let no_token = super::super::prepare::Server { token: None, port: "1".into() };
        assert_eq!(delete(&no_token, "codex-proj").await, Deleted::Unreachable);
    }

    /// PR-09: no Windows o app-server vivo daquele endereço sai pelo `TerminateProcess` antes do arquivo
    /// sumir (o Python fazia `os.kill`, que lá é `TerminateProcess`).
    #[cfg(windows)]
    #[test]
    fn the_live_app_server_of_that_address_is_terminated_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = "ws://127.0.0.1:65000";
        // O PowerShell junta os argumentos do `-Command`: o `#` faz do resto um comentário, e o argv
        // continua com `app-server` e o endereço como palavras.
        let mut server = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep", "60", "#", "app-server", endpoint])
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while app_server(server.id(), endpoint) != AppServer::Ours && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(app_server(server.id(), endpoint), AppServer::Ours);
        put(dir.path(), json!({"name": "codex-proj", "endpoint": endpoint, "app_pid": server.id(), "key": "k"}));
        local(dir.path(), "codex-proj").unwrap();
        assert!(server.try_wait().unwrap().is_some(), "o app-server saiu antes de o arquivo sumir");
        assert!(!codex_sidecar::path(dir.path(), "codex-proj").exists());
    }
}
