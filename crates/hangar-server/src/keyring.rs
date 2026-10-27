//! Chaveiro `login` do GNOME desta máquina: o estado vem do Secret Service e o desbloqueio usa o
//! socket de controle do daemon, o mesmo caminho do PAM. `gnome-keyring-daemon --unlock` não serve:
//! com um daemon de pé ele sobe outro, órfão, e o chaveiro segue trancado.
#[cfg(unix)]
use std::{io::{Read, Write}, os::unix::net::UnixStream, path::Path, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyringState { Unlocked, Locked, Absent }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlockError { Denied, Failed, NoDaemon }

impl UnlockError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Denied => "keyring_wrong_password",
            Self::Failed => "keyring_unlock_failed",
            Self::NoDaemon => "keyring_no_daemon",
        }
    }
}

/// O que a máquina oferece; o teste troca por um falso.
pub trait Backend: Send + Sync {
    /// `None` = não deu para ler agora; quem chama fica com o último estado.
    fn state(&self) -> Option<KeyringState>;
    fn unlock(&self, password: &[u8]) -> Result<(), UnlockError>;
    /// true = o daemon voltou de pé.
    fn restart(&self) -> bool;
}

pub struct Keyring {
    backend: std::sync::RwLock<std::sync::Arc<dyn Backend>>,
    tx: tokio::sync::watch::Sender<KeyringState>,
}

impl Keyring {
    pub fn system() -> Self {
        Self { backend: std::sync::RwLock::new(std::sync::Arc::new(System)), tx: tokio::sync::watch::channel(KeyringState::Absent).0 }
    }

    pub fn set_backend(&self, backend: std::sync::Arc<dyn Backend>) {
        *self.backend.write().unwrap_or_else(|e| e.into_inner()) = backend;
    }

    pub fn current(&self) -> KeyringState { *self.tx.borrow() }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<KeyringState> { self.tx.subscribe() }

    fn backend(&self) -> std::sync::Arc<dyn Backend> { self.backend.read().unwrap_or_else(|e| e.into_inner()).clone() }

    /// Bloqueante.
    pub fn refresh(&self) -> KeyringState { self.read().unwrap_or_else(|| self.current()) }

    /// Bloqueante. `None` = não deu para ler; o último estado publicado fica.
    fn read(&self) -> Option<KeyringState> {
        let Some(state) = self.backend().state() else {
            tracing::warn!(code = "keyring_state_unread", "estado do chaveiro não foi lido; fica o último");
            return None;
        };
        self.tx.send_if_modified(|s| std::mem::replace(s, state) != state);
        Some(state)
    }

    /// Bloqueante. Senha aceita e chaveiro ainda trancado é o daemon preso por um prompt que
    /// falhou: reinicia e manda a senha de novo, uma vez. Leitura que não veio não conta como
    /// trancado: reiniciar à toa trancaria o chaveiro que acabou de abrir.
    pub fn unlock(&self, password: &[u8]) -> Result<KeyringState, UnlockError> {
        let backend = self.backend();
        backend.unlock(password)?;
        match self.read() {
            Some(KeyringState::Locked) => {
                if !backend.restart() { return Err(UnlockError::Failed); }
                backend.unlock(password)?;
                match self.read() {
                    Some(KeyringState::Locked) | None => Err(UnlockError::Failed),
                    Some(state) => Ok(state),
                }
            }
            Some(state) => Ok(state),
            None => Err(UnlockError::Failed),
        }
    }
}

/// O chaveiro de verdade, pelo `busctl` e pelo socket de controle.
struct System;

#[cfg(target_os = "linux")]
impl Backend for System {
    fn state(&self) -> Option<KeyringState> {
        // `--auto-start=no`: perguntar não sobe o daemon; sem daemon não há o que destrancar.
        let mut cmd = std::process::Command::new("busctl");
        cmd.args(["--user", "--auto-start=no", "get-property", "org.freedesktop.secrets",
            "/org/freedesktop/secrets/collection/login", "org.freedesktop.Secret.Collection", "Locked"]);
        // Prazo estourado ou busctl que não rodou: sem leitura. Saída com erro: ninguém no barramento.
        let out = hangar_workspace::process::run_program(&mut cmd, Duration::from_secs(5)).ok()?;
        if out.code != 0 { return Some(KeyringState::Absent); }
        parse_locked(&out.stdout).map(|locked| if locked { KeyringState::Locked } else { KeyringState::Unlocked })
    }

    fn unlock(&self, password: &[u8]) -> Result<(), UnlockError> {
        control_unlock(&control_socket().ok_or(UnlockError::NoDaemon)?, password)
    }

    // Prazos somados abaixo dos 45 s que os clientes esperam pela resposta.
    fn restart(&self) -> bool {
        let mut cmd = std::process::Command::new("systemctl");
        cmd.args(["--user", "restart", "gnome-keyring-daemon.service"]);
        match hangar_workspace::process::run_program(&mut cmd, Duration::from_secs(10)) {
            Ok(out) if out.code == 0 => {}
            Ok(out) => {
                tracing::warn!(code = "keyring_restart_failed", exit = out.code, "reinício do gnome-keyring-daemon recusado");
                return false;
            }
            Err(error) => {
                tracing::warn!(code = "keyring_restart_failed", %error, "reinício do gnome-keyring-daemon não terminou");
                return false;
            }
        }
        // O daemon novo cria o socket logo depois de o systemd dar o serviço por iniciado.
        let Some(socket) = control_socket() else { return false };
        for _ in 0..20 {
            if socket.exists() { return true; }
            std::thread::sleep(Duration::from_millis(100));
        }
        socket.exists()
    }
}

#[cfg(target_os = "linux")]
fn control_socket() -> Option<std::path::PathBuf> {
    std::env::var_os("GNOME_KEYRING_CONTROL").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join("keyring")))
        .map(|dir| dir.join("control"))
}

#[cfg(not(target_os = "linux"))]
impl Backend for System {
    fn state(&self) -> Option<KeyringState> { Some(KeyringState::Absent) }
    fn unlock(&self, _password: &[u8]) -> Result<(), UnlockError> { Err(UnlockError::NoDaemon) }
    fn restart(&self) -> bool { false }
}

/// Relê o estado de tempos em tempos: o daemon pode cair, reiniciar ou ser aberto pelo login gráfico.
pub fn spawn(keyring: std::sync::Arc<Keyring>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let k = keyring.clone();
            let _ = tokio::task::spawn_blocking(move || k.refresh()).await;
        }
    })
}

#[derive(serde::Deserialize)]
struct UnlockBody { password: zeroize::Zeroizing<String> }

/// Só o dono pelo token, e o corpo nunca segue ao Python: não há repasse nesta rota.
pub async fn unlock_route(
    axum::extract::State(st): axum::extract::State<std::sync::Arc<crate::routes::AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    req: axum::extract::Request,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use crate::session_write::detail;
    let headers = req.headers().clone();
    let mut resp = 'r: {
        if crate::routes::on_guest_port(&req) || headers.contains_key("x-hangar-guest") {
            break 'r detail(StatusCode::FORBIDDEN, "keyring_forbidden", "só o dono destranca o chaveiro", serde_json::Value::Null);
        }
        let (fwd, owner) = crate::routes::gate(&st, peer, &req);
        if !owner {
            st.auth.record_fail(&fwd.client_ip);
            break 'r detail(StatusCode::UNAUTHORIZED, "unauthorized", "token do dono ausente ou errado", serde_json::Value::Null);
        }
        let Ok(bytes) = axum::body::to_bytes(req.into_body(), 4 * 1024).await else {
            break 'r detail(StatusCode::UNPROCESSABLE_ENTITY, "keyring_bad_request", "corpo inválido", serde_json::Value::Null);
        };
        let mut raw = bytes.try_into_mut().unwrap_or_else(|b| b[..].into());
        let parsed = serde_json::from_slice::<UnlockBody>(&raw).ok();
        zeroize::Zeroize::zeroize(raw.as_mut());
        let Some(body) = parsed.filter(|b| !b.password.is_empty()) else {
            break 'r detail(StatusCode::UNPROCESSABLE_ENTITY, "keyring_bad_request", "falta a senha", serde_json::Value::Null);
        };
        let keyring = st.list.keyring.clone();
        match tokio::task::spawn_blocking(move || keyring.unlock(body.password.as_bytes())).await {
            Ok(Ok(state)) => crate::session_write::json_response(StatusCode::OK, serde_json::json!({"state": state})),
            Ok(Err(e)) => {
                let status = match e { UnlockError::Denied => StatusCode::FORBIDDEN, UnlockError::NoDaemon => StatusCode::SERVICE_UNAVAILABLE,
                    UnlockError::Failed => StatusCode::CONFLICT };
                st.diag.report("rust.keyring_unlock_failed", "", e.code(), "desbloqueio do chaveiro recusado");
                detail(status, e.code(), "o chaveiro não abriu", serde_json::Value::Null)
            }
            Err(_) => detail(StatusCode::INTERNAL_SERVER_ERROR, "keyring_unlock_failed", "o chaveiro não abriu", serde_json::Value::Null),
        }
    };
    crate::routes::cors(&headers, resp.headers_mut());
    resp
}

#[cfg(unix)]
const OP_UNLOCK: u32 = 1;
#[cfg(unix)]
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(unix)]
/// Pacote do `gkr-pam`: um byte de credencial (o daemon confere o dono por `SO_PEERCRED`), depois
/// `[tamanho][op][tamanho da senha][senha]` em big-endian; a resposta é `[8][resultado]`.
pub fn control_unlock(socket: &Path, password: &[u8]) -> Result<(), UnlockError> {
    let mut conn = UnixStream::connect(socket).map_err(|_| UnlockError::NoDaemon)?;
    let _ = conn.set_read_timeout(Some(IO_TIMEOUT));
    let _ = conn.set_write_timeout(Some(IO_TIMEOUT));
    let len = u32::try_from(password.len()).map_err(|_| UnlockError::Failed)?;
    let mut packet = zeroize::Zeroizing::new(Vec::with_capacity(13 + password.len()));
    packet.push(0);
    packet.extend_from_slice(&(12 + len).to_be_bytes());
    packet.extend_from_slice(&OP_UNLOCK.to_be_bytes());
    packet.extend_from_slice(&len.to_be_bytes());
    packet.extend_from_slice(password);
    conn.write_all(&packet).map_err(|_| UnlockError::Failed)?;
    let mut reply = [0u8; 8];
    conn.read_exact(&mut reply).map_err(|_| UnlockError::Failed)?;
    match (u32::from_be_bytes(reply[..4].try_into().unwrap()), u32::from_be_bytes(reply[4..].try_into().unwrap())) {
        (8, 0) => Ok(()),
        (8, 1) => Err(UnlockError::Denied),
        (8, 3) => Err(UnlockError::NoDaemon),
        _ => Err(UnlockError::Failed),
    }
}

pub fn parse_locked(busctl_output: &str) -> Option<bool> {
    match busctl_output.trim() {
        "b true" => Some(true),
        "b false" => Some(false),
        _ => None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{os::unix::net::UnixListener, path::PathBuf};

    /// Daemon falso: lê o pacote do cliente e responde `result`.
    fn fake_daemon(dir: &Path, result: u32) -> (PathBuf, std::thread::JoinHandle<Vec<u8>>) {
        let path = dir.join("control");
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut cred = [0u8; 1];
            conn.read_exact(&mut cred).unwrap();
            let mut len = [0u8; 4];
            conn.read_exact(&mut len).unwrap();
            let mut rest = vec![0u8; u32::from_be_bytes(len) as usize - 4];
            conn.read_exact(&mut rest).unwrap();
            conn.write_all(&[0, 0, 0, 8]).unwrap();
            conn.write_all(&result.to_be_bytes()).unwrap();
            [&cred[..], &len, &rest].concat()
        });
        (path, handle)
    }

    #[test]
    fn unlock_sends_the_pam_packet_and_maps_the_daemon_answer() {
        let dir = tempfile::tempdir().unwrap();
        let (path, daemon) = fake_daemon(dir.path(), 0);
        assert_eq!(control_unlock(&path, b"abc"), Ok(()));
        // byte de credencial, tamanho 15, op 1, senha com tamanho 3
        assert_eq!(daemon.join().unwrap(), [&[0u8][..], &[0, 0, 0, 15], &[0, 0, 0, 1], &[0, 0, 0, 3], b"abc"].concat());

        let dir = tempfile::tempdir().unwrap();
        let (path, daemon) = fake_daemon(dir.path(), 1);
        assert_eq!(control_unlock(&path, b"x"), Err(UnlockError::Denied));
        daemon.join().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let (path, daemon) = fake_daemon(dir.path(), 3);
        assert_eq!(control_unlock(&path, b"x"), Err(UnlockError::NoDaemon));
        daemon.join().unwrap();
    }

    #[test]
    fn unlock_without_socket_is_no_daemon() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(control_unlock(&dir.path().join("control"), b"x"), Err(UnlockError::NoDaemon));
    }

    /// Contra um gnome-keyring de verdade, isolado (HOME, XDG_RUNTIME_DIR e barramento temporários),
    /// com o `login` trancado e prompt já falhado: `KR_PW` é a senha certa.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "precisa de um gnome-keyring isolado; ver o roteiro do teste manual"]
    fn system_backend_against_an_isolated_daemon() {
        let pw = std::env::var("KR_PW").unwrap();
        let k = Keyring::system();
        assert_eq!(k.refresh(), KeyringState::Locked);
        assert_eq!(k.unlock(b"errada"), Err(UnlockError::Denied));
        assert_eq!(k.unlock(pw.as_bytes()), Ok(KeyringState::Unlocked));
        assert_eq!(k.current(), KeyringState::Unlocked);
    }

    /// Leitura que não veio (busctl estourou o prazo sob carga) não é "sem chaveiro": o aviso fica.
    #[test]
    fn an_unread_state_keeps_the_last_one() {
        struct Script(std::sync::Mutex<Vec<Option<KeyringState>>>);
        impl Backend for Script {
            fn state(&self) -> Option<KeyringState> { self.0.lock().unwrap().remove(0) }
            fn unlock(&self, _: &[u8]) -> Result<(), UnlockError> { Ok(()) }
            fn restart(&self) -> bool { true }
        }
        let k = Keyring::system();
        k.set_backend(std::sync::Arc::new(Script(std::sync::Mutex::new(vec![Some(KeyringState::Locked), None, Some(KeyringState::Absent)]))));
        assert_eq!(k.refresh(), KeyringState::Locked);
        assert_eq!(k.refresh(), KeyringState::Locked);
        assert_eq!(k.current(), KeyringState::Locked);
        assert_eq!(k.refresh(), KeyringState::Absent);
    }

    /// Senha aceita e leitura que não veio: não reinicia o daemon (isso o trancaria de novo) e não
    /// diz que abriu.
    #[test]
    fn an_unread_state_after_unlock_does_not_restart_the_daemon() {
        struct Unread(std::sync::atomic::AtomicU32);
        impl Backend for Unread {
            fn state(&self) -> Option<KeyringState> { None }
            fn unlock(&self, _: &[u8]) -> Result<(), UnlockError> { Ok(()) }
            fn restart(&self) -> bool { self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst); true }
        }
        let backend = std::sync::Arc::new(Unread(std::sync::atomic::AtomicU32::new(0)));
        let k = Keyring::system();
        k.set_backend(backend.clone());
        k.tx.send_replace(KeyringState::Locked);
        assert_eq!(k.unlock(b"x"), Err(UnlockError::Failed));
        assert_eq!(backend.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn locked_property_is_read_from_busctl_output() {
        assert_eq!(parse_locked("b true\n"), Some(true));
        assert_eq!(parse_locked("b false\n"), Some(false));
        assert_eq!(parse_locked("Failed to get property"), None);
    }
}
