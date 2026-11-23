//! Chaveiros do GNOME desta máquina: o `login` e o do alias `default` (Chrome, Slack). O estado vem do
//! Secret Service e o desbloqueio do `login` usa o socket de controle do daemon, o mesmo caminho do PAM.
//! `gnome-keyring-daemon --unlock` não serve: com um daemon de pé ele sobe outro, órfão, e o chaveiro
//! segue trancado. O `default` tem a senha guardada no `login` e só abre pelo fluxo do prompter.
#[cfg(unix)]
use std::{io::{Read, Write}, os::unix::net::UnixStream, path::Path, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyringState { Unlocked, Locked, Absent }

impl KeyringState {
    fn from_locked(locked: bool) -> Self { if locked { Self::Locked } else { Self::Unlocked } }
}

/// As duas coleções que os programas usam. Alias `default` apontando para o `login` é lido duas vezes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading { pub login: KeyringState, pub default: KeyringState }

impl Reading {
    const ABSENT: Self = Self { login: KeyringState::Absent, default: KeyringState::Absent };

    /// O que o dono vê: qualquer coleção trancada tranca.
    pub fn combined(self) -> KeyringState {
        use KeyringState::*;
        match (self.login, self.default) {
            (Locked, _) | (_, Locked) => Locked,
            (Unlocked, _) | (_, Unlocked) => Unlocked,
            (Absent, Absent) => Absent,
        }
    }

    /// Login aberto e default trancado: o daemon abre o default com a senha guardada no login.
    pub fn default_waits(self) -> bool { self.login == KeyringState::Unlocked && self.default == KeyringState::Locked }
}

/// Resposta do daemon ao pedido de abrir o `default`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultOpen { Opened, Refused, NoAnswer }

/// Tentativa sem senha: `Later` = sem tela, desbloqueio do dono em curso ou leitura que não veio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unattended { Asked(DefaultOpen), Later, Nothing }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlockError { Denied, Failed, NoDaemon, DefaultLocked }

impl UnlockError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Denied => "keyring_wrong_password",
            Self::Failed => "keyring_unlock_failed",
            Self::NoDaemon => "keyring_no_daemon",
            Self::DefaultLocked => "keyring_default_locked",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Self::DefaultLocked => "o login abriu, mas o chaveiro padrão (Chrome, Slack) continua trancado",
            _ => "o chaveiro não abriu",
        }
    }
}

/// Uma volta do desbloqueio com senha.
enum Step { Done(KeyringState), LoginStuck, DefaultStuck }

/// O que a máquina oferece; o teste troca por um falso.
pub trait Backend: Send + Sync {
    /// `None` = não deu para ler agora; quem chama fica com o último estado.
    fn state(&self) -> Option<Reading>;
    /// Abre o `login` pela senha.
    fn unlock(&self, password: &[u8]) -> Result<(), UnlockError>;
    /// true = o daemon voltou de pé.
    fn restart(&self) -> bool;
    /// true = há tela para o prompter.
    fn display(&self) -> bool;
    /// Pede o `default` pelo prompter e espera a resposta até um prazo.
    fn open_default(&self) -> DefaultOpen;
}

pub struct Keyring {
    backend: std::sync::RwLock<std::sync::Arc<dyn Backend>>,
    /// Backend trocado (teste): a escuta do barramento não publica por cima dele.
    real: std::sync::atomic::AtomicBool,
    tx: tokio::sync::watch::Sender<KeyringState>,
    /// Um desbloqueio por vez: o automático desiste se o do dono estiver em curso.
    gate: std::sync::Mutex<()>,
    /// Este daemon já ficou sem responder ao pedido do `default`: preso por um prompt que falhou.
    stuck: std::sync::atomic::AtomicBool,
    /// A tentativa sem senha já pediu o `default` a este daemon.
    asked: std::sync::atomic::AtomicBool,
}

impl Keyring {
    pub fn system() -> Self {
        Self { backend: std::sync::RwLock::new(std::sync::Arc::new(System::default())), real: true.into(),
            tx: tokio::sync::watch::channel(KeyringState::Absent).0, gate: Default::default(), stuck: false.into(), asked: false.into() }
    }

    pub fn set_backend(&self, backend: std::sync::Arc<dyn Backend>) {
        self.real.store(false, std::sync::atomic::Ordering::Relaxed);
        *self.backend.write().unwrap_or_else(|e| e.into_inner()) = backend;
    }

    fn publish(&self, state: KeyringState) { self.tx.send_if_modified(|s| std::mem::replace(s, state) != state); }

    pub fn current(&self) -> KeyringState { *self.tx.borrow() }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<KeyringState> { self.tx.subscribe() }

    fn backend(&self) -> std::sync::Arc<dyn Backend> { self.backend.read().unwrap_or_else(|e| e.into_inner()).clone() }

    /// Daemon novo no barramento: o que se sabia do anterior não vale.
    pub fn daemon_changed(&self) {
        self.stuck.store(false, std::sync::atomic::Ordering::Relaxed);
        self.asked.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn asked(&self) -> bool { self.asked.load(std::sync::atomic::Ordering::Relaxed) }

    /// Bloqueante.
    pub fn refresh(&self) -> KeyringState { self.read().map(Reading::combined).unwrap_or_else(|| self.current()) }

    /// Bloqueante. `None` = não deu para ler; o último estado publicado fica.
    fn read(&self) -> Option<Reading> {
        let Some(reading) = self.backend().state() else {
            tracing::warn!(code = "keyring_state_unread", "estado do chaveiro não foi lido; fica o último");
            return None;
        };
        self.publish(reading.combined());
        Some(reading)
    }

    /// Bloqueante. Sem senha: pede o `default` uma vez por vida do daemon, sem reiniciá-lo. Reiniciar
    /// trancaria o `login` que o PAM abriu e mataria o prompt de quem estiver diante da tela; com o
    /// daemon preso, o aviso fica e o desbloqueio com senha reinicia.
    pub fn open_default_unattended(&self) -> Unattended {
        let backend = self.backend();
        {
            let Ok(_gate) = self.gate.try_lock() else { return Unattended::Later };
            if self.asked() { return Unattended::Nothing; }
            if !backend.display() { return Unattended::Later; }
            // Leitura que não veio não se repete aqui: o laço de espera viraria consulta periódica.
            if !self.read().is_some_and(Reading::default_waits) { return Unattended::Nothing; }
            self.asked.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        // Fora da trava: o desbloqueio do dono não espera o prazo deste pedido.
        let outcome = self.ask_default(&*backend);
        self.read();
        Unattended::Asked(outcome)
    }

    fn ask_default(&self, backend: &dyn Backend) -> DefaultOpen {
        let outcome = backend.open_default();
        if outcome == DefaultOpen::NoAnswer {
            self.stuck.store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(code = "keyring_default_no_answer", "o daemon não respondeu ao pedido do chaveiro padrão");
        }
        outcome
    }

    /// Bloqueante. Abre o `login` pela senha e depois o `default`. Senha aceita e `login` ainda
    /// trancado, ou `default` sem resposta, é o daemon preso por um prompt que falhou: reinicia e
    /// dá mais uma volta. Leitura que não veio não conta como trancado: reiniciar à toa trancaria o
    /// chaveiro que acabou de abrir.
    pub fn unlock(&self, password: &[u8]) -> Result<KeyringState, UnlockError> {
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        let backend = self.backend();
        // Daemon já visto sem resposta: esperar o prazo de novo só gastaria o tempo do cliente.
        let mut step = self.unlock_once(&*backend, password, self.stuck.load(std::sync::atomic::Ordering::Relaxed))?;
        if !matches!(step, Step::Done(_)) && self.restart(&*backend) {
            step = self.unlock_once(&*backend, password, false)?;
        }
        match step {
            Step::Done(state) => Ok(state),
            Step::LoginStuck => Err(UnlockError::Failed),
            Step::DefaultStuck => Err(UnlockError::DefaultLocked),
        }
    }

    fn unlock_once(&self, backend: &dyn Backend, password: &[u8], skip_ask: bool) -> Result<Step, UnlockError> {
        backend.unlock(password)?;
        let reading = self.read().ok_or(UnlockError::Failed)?;
        if reading.login == KeyringState::Locked { return Ok(Step::LoginStuck); }
        if !reading.default_waits() { return Ok(Step::Done(reading.combined())); }
        if !backend.display() { return Err(UnlockError::DefaultLocked); }
        if skip_ask || self.ask_default(backend) == DefaultOpen::NoAnswer { return Ok(Step::DefaultStuck); }
        match self.read().ok_or(UnlockError::Failed)?.combined() {
            KeyringState::Locked => Err(UnlockError::DefaultLocked),
            state => Ok(Step::Done(state)),
        }
    }

    fn restart(&self, backend: &dyn Backend) -> bool {
        let ok = backend.restart();
        if ok { self.daemon_changed(); }
        ok
    }
}

#[cfg(target_os = "linux")]
const SECRETS: &str = "org.freedesktop.secrets";
#[cfg(target_os = "linux")]
const LOGIN: &str = "/org/freedesktop/secrets/collection/login";
#[cfg(target_os = "linux")]
const DEFAULT_ALIAS: &str = "/org/freedesktop/secrets/aliases/default";
#[cfg(target_os = "linux")]
const CALL_LIMIT: Duration = Duration::from_secs(5);
/// Pedido inteiro do `default`: com a senha guardada no login o prompter responde em menos de um
/// segundo. Dois pedidos e um reinício cabem nos 45 s que os clientes esperam.
#[cfg(target_os = "linux")]
const PROMPT_LIMIT: Duration = Duration::from_secs(10);

/// O chaveiro de verdade, pelo `busctl`, pelo socket de controle e, para o `default`, por uma conexão
/// que fica: prompt sem resposta continua vivo para quem estiver diante da tela, e o fim dele chega
/// pela escuta (`CollectionChanged`).
#[derive(Default)]
struct System {
    #[cfg(target_os = "linux")]
    conn: std::sync::Mutex<Option<zbus::Connection>>,
}

#[cfg(target_os = "linux")]
impl Backend for System {
    fn state(&self) -> Option<Reading> {
        Some(Reading { login: busctl_locked(LOGIN)?, default: busctl_locked(DEFAULT_ALIAS)? })
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

    // O prompter é ativado pelo barramento com o ambiente do systemd do usuário.
    fn display(&self) -> bool { crate::display_env::user_manager_has_display() }

    fn open_default(&self) -> DefaultOpen {
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return DefaultOpen::Refused };
        let cached = self.conn.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let conn = match cached {
            Some(conn) => conn,
            None => match rt.block_on(tokio::time::timeout(CALL_LIMIT, zbus::Connection::session())) {
                Ok(Ok(conn)) => {
                    *self.conn.lock().unwrap_or_else(|e| e.into_inner()) = Some(conn.clone());
                    conn
                }
                _ => {
                    tracing::warn!(code = "keyring_default_bus_failed", "sem conexão ao barramento para abrir o chaveiro padrão");
                    return DefaultOpen::Refused;
                }
            },
        };
        // Um prazo para o pedido inteiro: o daemon preso pode parar em qualquer passo.
        match rt.block_on(async { tokio::time::timeout(PROMPT_LIMIT, prompt_default(&conn)).await.unwrap_or(Ok(DefaultOpen::NoAnswer)) }) {
            Ok(outcome) => outcome,
            Err(error) => {
                tracing::warn!(code = "keyring_default_bus_failed", %error, "pedido do chaveiro padrão falhou no barramento");
                *self.conn.lock().unwrap_or_else(|e| e.into_inner()) = None;
                DefaultOpen::Refused
            }
        }
    }
}

/// Saída com erro: ninguém no barramento ou coleção que não existe.
#[cfg(target_os = "linux")]
fn busctl_locked(path: &str) -> Option<KeyringState> {
    // `--auto-start=no`: perguntar não sobe o daemon; sem daemon não há o que destrancar.
    let mut cmd = std::process::Command::new("busctl");
    cmd.args(["--user", "--auto-start=no", "get-property", SECRETS, path, "org.freedesktop.Secret.Collection", "Locked"]);
    // Prazo estourado ou busctl que não rodou: sem leitura.
    let out = hangar_workspace::process::run_program(&mut cmd, CALL_LIMIT).ok()?;
    if out.code != 0 { return Some(KeyringState::Absent); }
    parse_locked(&out.stdout).map(KeyringState::from_locked)
}

#[cfg(target_os = "linux")]
async fn secret_proxy<'a>(conn: &zbus::Connection, path: &'a str, interface: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
    zbus::proxy::Builder::<zbus::Proxy>::new(conn).destination(SECRETS)?.path(path)?.interface(interface)?
        .cache_properties(zbus::proxy::CacheProperties::No).build().await
}

/// Erro de método é a resposta do daemon; falha da conexão não.
#[cfg(target_os = "linux")]
fn refused(error: zbus::Error) -> zbus::Result<DefaultOpen> {
    match error {
        zbus::Error::MethodError(..) | zbus::Error::FDO(_) => Ok(DefaultOpen::Refused),
        error => Err(error),
    }
}

/// O caminho do Chrome: `Unlock` e, vindo prompt, `Prompt("")` até o `Completed`. `Err` = a conexão falhou.
#[cfg(target_os = "linux")]
async fn prompt_default(conn: &zbus::Connection) -> zbus::Result<DefaultOpen> {
    use futures_util::StreamExt;
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};
    let no_start = || zbus::proxy::MethodFlags::NoAutoStart.into();
    let service = secret_proxy(conn, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").await?;
    let target = (vec![ObjectPath::try_from(DEFAULT_ALIAS)?],);
    let call = service.call_with_flags::<_, _, (Vec<OwnedObjectPath>, OwnedObjectPath)>("Unlock", no_start(), &target);
    let (unlocked, prompt) = match call.await {
        Ok(Some(reply)) => reply,
        Ok(None) => return Ok(DefaultOpen::Refused),
        Err(error) => return refused(error),
    };
    if !unlocked.is_empty() { return Ok(DefaultOpen::Opened); }
    let prompt = prompt.to_string();
    if prompt == "/" { return Ok(DefaultOpen::Refused); }
    let prompt = secret_proxy(conn, prompt.as_str(), "org.freedesktop.Secret.Prompt").await?;
    let mut completed = prompt.receive_signal("Completed").await?;
    if let Err(error) = prompt.call_with_flags::<_, _, ()>("Prompt", no_start(), &("",)).await { return refused(error); }
    Ok(match completed.next().await {
        Some(msg) if matches!(msg.body().deserialize::<(bool, OwnedValue)>(), Ok((false, _))) => DefaultOpen::Opened,
        _ => DefaultOpen::Refused,
    })
}

#[cfg(target_os = "linux")]
fn control_socket() -> Option<std::path::PathBuf> {
    std::env::var_os("GNOME_KEYRING_CONTROL").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join("keyring")))
        .map(|dir| dir.join("control"))
}

#[cfg(not(target_os = "linux"))]
impl Backend for System {
    fn state(&self) -> Option<Reading> { Some(Reading::ABSENT) }
    fn unlock(&self, _password: &[u8]) -> Result<(), UnlockError> { Err(UnlockError::NoDaemon) }
    fn restart(&self) -> bool { false }
    fn display(&self) -> bool { false }
    fn open_default(&self) -> DefaultOpen { DefaultOpen::Refused }
}

/// Lê o estado ao ligar e depois só quando o daemon avisa: troca de dono do nome (queda, religada) ou
/// `CollectionChanged` (trancou, abriu). Nunca por consulta periódica: cada `Get` de um cliente que já
/// saiu do barramento derruba o gnome-keyring-daemon (assertion em `gkd_secret_service_get_pkcs11_session`).
#[cfg(target_os = "linux")]
pub fn spawn(keyring: std::sync::Arc<Keyring>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut warned = false;
        loop {
            let result = watch(&keyring).await;
            tracing::info!(code = "keyring_watch_ended", ok = result.is_ok(), "escuta do chaveiro terminou");
            if let Some(error) = result.as_ref().err().filter(|_| !warned) {
                tracing::warn!(code = "keyring_watch_failed", %error, "escuta do chaveiro parou; religa em 30 s");
            }
            warned = result.is_err();
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    })
}

#[cfg(target_os = "linux")]
async fn watch(keyring: &std::sync::Arc<Keyring>) -> zbus::Result<()> {
    use futures_util::StreamExt;
    let conn = zbus::Connection::session().await?;
    let bus = zbus::fdo::DBusProxy::new(&conn).await?;
    let mut owners = bus.receive_name_owner_changed_with_args(&[(0, SECRETS)]).await?;
    // Todos os sinais do serviço: a coleção `login` pode ser criada depois que o daemon pega o nome.
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.Secret.Service")?.build();
    let mut changes = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;
    const PROPS: &str = "org.freedesktop.DBus.Properties";
    let (login, default) = (secret_proxy(&conn, LOGIN, PROPS).await?, secret_proxy(&conn, DEFAULT_ALIAS, PROPS).await?);
    tracing::info!(code = "keyring_watch_ready", name = ?conn.unique_name(), "escuta do chaveiro ligada");
    // Releituras com atraso, contadas: leitura que falhou, ou daemon recém-subido que ainda não exportou a coleção.
    let (mut again, mut failures): (u32, u32) = (0, 0);
    // Tentativa de abrir o `default` sem senha; a marca `asked` a limita a uma por vida do daemon.
    let mut auto: Option<crate::AbortOnDrop> = None;
    loop {
        if keyring.real.load(std::sync::atomic::Ordering::Relaxed) {
            match locked_state(&bus, &login, &default).await {
                Some(reading) => {
                    keyring.publish(reading.combined());
                    failures = 0;
                    if reading.default_waits() && !keyring.asked() && auto.as_ref().is_none_or(|task| task.0.is_finished()) {
                        auto = Some(crate::AbortOnDrop(tokio::spawn(open_default_when_ready(keyring.clone()))));
                    }
                }
                None => {
                    tracing::warn!(code = "keyring_state_unread", "estado do chaveiro não foi lido; fica o último");
                    failures += 1;
                    // Três falhas seguidas e para: daqui em diante só um sinal relê.
                    if failures < 3 { again = again.max(1); }
                }
            }
        }
        tokio::select! {
            owner = owners.next() => {
                if owner.is_none() { return Err(zbus::Error::Failure("o barramento encerrou o aviso de dono do chaveiro".into())); }
                again = 3;
                auto = None;
                keyring.daemon_changed();
            },
            change = changes.next() => match change {
                None => return Err(zbus::Error::Failure("o barramento encerrou os sinais do chaveiro".into())),
                Some(Err(error)) => return Err(error),
                Some(Ok(_)) => {}
            },
            _ = tokio::time::sleep(Duration::from_secs(2)), if again > 0 => { again -= 1; },
        }
    }
}

/// O login abre no PAM antes de o compositor exportar o display: o prompter sem tela falharia e
/// prenderia o daemon. Espera a tela (e o fim do desbloqueio do dono) por até 10 min, sem ler o
/// Secret Service enquanto espera.
#[cfg(target_os = "linux")]
async fn open_default_when_ready(keyring: std::sync::Arc<Keyring>) {
    for _ in 0..120 {
        let k = keyring.clone();
        match tokio::task::spawn_blocking(move || k.open_default_unattended()).await {
            // estavel: código de produção, não teste; é o intervalo entre olhadas no ambiente do systemd à espera da tela.
            Ok(Unattended::Later) => tokio::time::sleep(Duration::from_secs(5)).await,
            _ => return,
        }
    }
    tracing::info!(code = "keyring_default_no_display", "sem tela para abrir o chaveiro padrão; o aviso fica");
}

/// Sem a coleção ou sem o serviço: nada a destrancar. Acesso negado ou sem resposta não é ausência.
#[cfg(target_os = "linux")]
fn missing(error: &zbus::Error) -> bool {
    use zbus::fdo::Error as Fdo;
    match error {
        zbus::Error::FDO(error) => matches!(**error, Fdo::UnknownObject(_) | Fdo::UnknownInterface(_) | Fdo::UnknownMethod(_)
            | Fdo::ServiceUnknown(_) | Fdo::NameHasNoOwner(_)),
        zbus::Error::MethodError(name, ..) => matches!(name.to_string().as_str(), "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.UnknownInterface" | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "org.freedesktop.Secret.Error.NoSuchObject"),
        _ => false,
    }
}

/// `None` = não deu para ler agora. Sem dono do nome não pergunta: perguntar subiria o daemon.
#[cfg(target_os = "linux")]
async fn locked_state(bus: &zbus::fdo::DBusProxy<'_>, login: &zbus::Proxy<'_>, default: &zbus::Proxy<'_>) -> Option<Reading> {
    let name = zbus::names::BusName::try_from(SECRETS).ok()?;
    if !tokio::time::timeout(CALL_LIMIT, bus.name_has_owner(name)).await.ok()?.ok()? { return Some(Reading::ABSENT); }
    Some(Reading { login: collection_locked(login).await?, default: collection_locked(default).await? })
}

#[cfg(target_os = "linux")]
async fn collection_locked(props: &zbus::Proxy<'_>) -> Option<KeyringState> {
    let reply = tokio::time::timeout(CALL_LIMIT, props.call_with_flags::<_, _, zbus::zvariant::OwnedValue>(
        "Get", zbus::proxy::MethodFlags::NoAutoStart.into(), &("org.freedesktop.Secret.Collection", "Locked"))).await.ok()?;
    match reply {
        Ok(Some(value)) => bool::try_from(value).ok().map(KeyringState::from_locked),
        Err(error) if missing(&error) => Some(KeyringState::Absent),
        _ => None,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn spawn(keyring: std::sync::Arc<Keyring>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { let _ = tokio::task::spawn_blocking(move || keyring.refresh()).await; })
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
        let (fwd, owner) = match crate::routes::gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
        if !owner {
            if let Err(e) = st.auth.record_failure(&fwd.client_ip).await { return crate::auth::unavailable(e); }
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
                    UnlockError::Failed | UnlockError::DefaultLocked => StatusCode::CONFLICT };
                st.diag.report("rust.keyring_unlock_failed", "", e.code(), "desbloqueio do chaveiro recusado");
                detail(status, e.code(), e.text(), serde_json::Value::Null)
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
            fn state(&self) -> Option<Reading> { self.0.lock().unwrap().remove(0).map(|s| Reading { login: s, default: s }) }
            fn unlock(&self, _: &[u8]) -> Result<(), UnlockError> { Ok(()) }
            fn restart(&self) -> bool { true }
            fn display(&self) -> bool { true }
            fn open_default(&self) -> DefaultOpen { DefaultOpen::Opened }
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
            fn state(&self) -> Option<Reading> { None }
            fn unlock(&self, _: &[u8]) -> Result<(), UnlockError> { Ok(()) }
            fn restart(&self) -> bool { self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst); true }
            fn display(&self) -> bool { true }
            fn open_default(&self) -> DefaultOpen { DefaultOpen::Opened }
        }
        let backend = std::sync::Arc::new(Unread(std::sync::atomic::AtomicU32::new(0)));
        let k = Keyring::system();
        k.set_backend(backend.clone());
        k.tx.send_replace(KeyringState::Locked);
        assert_eq!(k.unlock(b"x"), Err(UnlockError::Failed));
        assert_eq!(backend.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// Daemon falso: o `login` abre pela senha `certa`; o `default` só pelo prompter, com tela e o
    /// login aberto; o daemon preso não responde até reiniciar, e reiniciar tranca as duas.
    #[derive(Default)]
    struct Daemon(std::sync::Mutex<Model>);

    #[derive(Default)]
    struct Model { login_locked: bool, default_locked: bool, display: bool, stuck: bool, stuck_after_restart: bool,
        login_sticks: bool, login_sticks_after_restart: bool,
        reads: u32, unlocks: u32, restarts: u32, opens: u32 }

    impl Backend for Daemon {
        fn state(&self) -> Option<Reading> {
            let mut m = self.0.lock().unwrap();
            m.reads += 1;
            Some(Reading { login: KeyringState::from_locked(m.login_locked), default: KeyringState::from_locked(m.default_locked) })
        }
        fn unlock(&self, password: &[u8]) -> Result<(), UnlockError> {
            let mut m = self.0.lock().unwrap();
            m.unlocks += 1;
            if password != b"certa" { return Err(UnlockError::Denied); }
            m.login_locked = m.login_sticks;
            Ok(())
        }
        fn restart(&self) -> bool {
            let mut m = self.0.lock().unwrap();
            m.restarts += 1;
            (m.login_locked, m.default_locked, m.stuck, m.login_sticks) = (true, true, m.stuck_after_restart, m.login_sticks_after_restart);
            true
        }
        fn display(&self) -> bool { self.0.lock().unwrap().display }
        fn open_default(&self) -> DefaultOpen {
            let mut m = self.0.lock().unwrap();
            m.opens += 1;
            if m.stuck { return DefaultOpen::NoAnswer; }
            if m.login_locked || !m.display { return DefaultOpen::Refused; }
            m.default_locked = false;
            DefaultOpen::Opened
        }
    }

    fn with(model: Model) -> (Keyring, std::sync::Arc<Daemon>) {
        let daemon = std::sync::Arc::new(Daemon(std::sync::Mutex::new(model)));
        let k = Keyring::system();
        k.set_backend(daemon.clone());
        (k, daemon)
    }

    /// O PAM abriu o `login` e o `default` do Chrome e do Slack ficou trancado: o aviso aparece.
    #[test]
    fn an_open_login_with_the_default_locked_is_locked() {
        let (k, _) = with(Model { default_locked: true, ..Default::default() });
        assert_eq!(k.refresh(), KeyringState::Locked);
        let (k, _) = with(Model::default());
        assert_eq!(k.refresh(), KeyringState::Unlocked);
    }

    /// Com a tela de pé, a senha guardada no login abre o default sem perguntar.
    #[test]
    fn unattended_opens_the_default_once_the_login_is_open() {
        let (k, daemon) = with(Model { default_locked: true, display: true, ..Default::default() });
        assert_eq!(k.open_default_unattended(), Unattended::Asked(DefaultOpen::Opened));
        assert_eq!(k.current(), KeyringState::Unlocked);
        assert_eq!(daemon.0.lock().unwrap().restarts, 0);
    }

    /// Sem senha, o daemon preso não é reiniciado: isso trancaria o login que o PAM abriu.
    #[test]
    fn unattended_never_restarts_a_stuck_daemon() {
        let (k, daemon) = with(Model { default_locked: true, display: true, stuck: true, ..Default::default() });
        assert_eq!(k.open_default_unattended(), Unattended::Asked(DefaultOpen::NoAnswer));
        assert_eq!(k.current(), KeyringState::Locked);
        assert_eq!(daemon.0.lock().unwrap().restarts, 0);
    }

    /// Sem tela o prompter falharia e prenderia o daemon: nem pede, nem lê o Secret Service.
    #[test]
    fn unattended_without_display_does_not_ask() {
        let (k, daemon) = with(Model { default_locked: true, ..Default::default() });
        assert_eq!(k.open_default_unattended(), Unattended::Later);
        let m = daemon.0.lock().unwrap();
        assert_eq!((m.reads, m.opens), (0, 0));
    }

    #[test]
    fn unattended_does_nothing_with_the_login_locked() {
        let (k, daemon) = with(Model { login_locked: true, default_locked: true, display: true, ..Default::default() });
        assert_eq!(k.open_default_unattended(), Unattended::Nothing);
        assert_eq!(daemon.0.lock().unwrap().opens, 0);
    }

    /// Uma tentativa por vida do daemon: o default trancado à mão depois fica trancado.
    #[test]
    fn unattended_asks_once_per_daemon_life() {
        let (k, daemon) = with(Model { default_locked: true, display: true, stuck: true, ..Default::default() });
        assert_eq!(k.open_default_unattended(), Unattended::Asked(DefaultOpen::NoAnswer));
        assert_eq!(k.open_default_unattended(), Unattended::Nothing);
        assert_eq!(daemon.0.lock().unwrap().opens, 1);
        daemon.0.lock().unwrap().stuck = false;
        k.daemon_changed();
        assert_eq!(k.open_default_unattended(), Unattended::Asked(DefaultOpen::Opened));
    }

    /// Desbloqueio do dono em curso: a tentativa automática volta depois, não desiste de vez.
    #[test]
    fn unattended_comes_back_while_the_password_unlock_runs() {
        let (k, daemon) = with(Model { default_locked: true, display: true, ..Default::default() });
        let gate = k.gate.lock().unwrap();
        assert_eq!(k.open_default_unattended(), Unattended::Later);
        drop(gate);
        assert_eq!(daemon.0.lock().unwrap().opens, 0);
        assert_eq!(k.open_default_unattended(), Unattended::Asked(DefaultOpen::Opened));
    }

    /// Senha digitada no aviso com as duas trancadas: abre o login e depois o default.
    #[test]
    fn the_password_also_opens_the_default() {
        let (k, daemon) = with(Model { login_locked: true, default_locked: true, display: true, ..Default::default() });
        assert_eq!(k.unlock(b"certa"), Ok(KeyringState::Unlocked));
        let m = daemon.0.lock().unwrap();
        assert_eq!((m.opens, m.restarts), (1, 0));
    }

    /// Login aberto pelo PAM e daemon preso: a senha deixa reiniciar, e o daemon novo abre as duas.
    #[test]
    fn the_password_restarts_a_stuck_daemon_and_opens_both() {
        let (k, daemon) = with(Model { default_locked: true, display: true, stuck: true, ..Default::default() });
        assert_eq!(k.unlock(b"certa"), Ok(KeyringState::Unlocked));
        let m = daemon.0.lock().unwrap();
        assert_eq!((m.restarts, m.unlocks), (1, 2));
    }

    /// Daemon que a tentativa automática já viu preso: o desbloqueio reinicia sem esperar o prazo de novo.
    #[test]
    fn a_daemon_seen_stuck_is_restarted_without_waiting_again() {
        let (k, daemon) = with(Model { default_locked: true, display: true, stuck: true, ..Default::default() });
        assert_eq!(k.open_default_unattended(), Unattended::Asked(DefaultOpen::NoAnswer));
        assert_eq!(k.unlock(b"certa"), Ok(KeyringState::Unlocked));
        let m = daemon.0.lock().unwrap();
        assert_eq!((m.opens, m.restarts), (2, 1));
    }

    /// Reinício só uma vez: preso de novo, o aviso fica e o erro diz que o login abriu.
    #[test]
    fn stuck_again_after_restart_reports_the_default_locked() {
        let (k, daemon) = with(Model { default_locked: true, display: true, stuck: true, stuck_after_restart: true, ..Default::default() });
        assert_eq!(k.unlock(b"certa"), Err(UnlockError::DefaultLocked));
        assert_eq!(k.current(), KeyringState::Locked);
        assert_eq!(daemon.0.lock().unwrap().restarts, 1);
    }

    /// Daemon novo que aceita a senha e segue com o login trancado: o erro não diz que o login abriu.
    #[test]
    fn a_login_locked_after_restart_is_not_reported_as_open() {
        let (k, daemon) = with(Model { default_locked: true, display: true, stuck: true, login_sticks_after_restart: true, ..Default::default() });
        assert_eq!(k.unlock(b"certa"), Err(UnlockError::Failed));
        assert_eq!(daemon.0.lock().unwrap().restarts, 1);
    }

    /// Senha errada nunca derruba o daemon de quem abriu o login.
    #[test]
    fn a_wrong_password_never_restarts() {
        let (k, daemon) = with(Model { default_locked: true, display: true, stuck: true, ..Default::default() });
        assert_eq!(k.unlock(b"errada"), Err(UnlockError::Denied));
        assert_eq!(daemon.0.lock().unwrap().restarts, 0);
    }

    /// Sem tela não há prompter: o login abre, o default fica, e nada reinicia.
    #[test]
    fn without_display_the_password_opens_only_the_login() {
        let (k, daemon) = with(Model { login_locked: true, default_locked: true, ..Default::default() });
        assert_eq!(k.unlock(b"certa"), Err(UnlockError::DefaultLocked));
        let m = daemon.0.lock().unwrap();
        assert_eq!((m.login_locked, m.opens, m.restarts), (false, 0, 0));
    }

    #[test]
    fn locked_property_is_read_from_busctl_output() {
        assert_eq!(parse_locked("b true\n"), Some(true));
        assert_eq!(parse_locked("b false\n"), Some(false));
        assert_eq!(parse_locked("Failed to get property"), None);
    }
}
