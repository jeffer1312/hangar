//! Primeiro acesso pelo Connect, com o mesmo formulário do celular. A convidada entra pela máquina que guarda o cofre
//! dela, para a senha nunca passar pelo serviço.
use super::*;
use super::servers::ServerEntry;
use hangar_connect_client::{AccountSession, CONNECT_SERVICE_URL, Code, ConnectClient, ConnectMachine, DeviceKey, DeviceStatus, MachineEntry,
    ReqwestTransport, SignInResult, VaultServer, install_code, normalize_name};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Enquanto este computador espera a liberação, o status é relido neste intervalo: o serviço não avisa.
const RECHECK: Duration = Duration::from_secs(3);

/// O que fica no disco, só legível pelo dono: o id desta instalação (dá o código da liberação), a conta e, fora dela,
/// a credencial deste computador. Sair apaga a conta e deixa a credencial: entrar de novo na mesma conta não pede
/// outra liberação.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
struct Saved {
    install_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<SavedAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device: Option<SavedDevice>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct SavedAccount { name: String, session: AccountSession }

/// A credencial vale só na conta e no serviço em que nasceu.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct SavedDevice { name: String, service_url: String, key: DeviceKey }

impl Saved {
    /// Guarda a conta; a credencial dela, quando tem, passa a ser a deste computador.
    fn keep(&mut self, account: Option<SavedAccount>) {
        if let Some(account) = &account && let Some(key) = &account.session.device {
            self.device = Some(SavedDevice { name: account.name.clone(), service_url: account.session.service_url.clone(), key: key.clone() });
        }
        self.account = account;
    }

    /// O serviço respondeu o estado da credencial. Revogada não volta: a conta fica (a tela mostra a perda do acesso),
    /// e a próxima entrada nela pede outra liberação.
    fn answered(&mut self, account: SavedAccount, status: DeviceStatus) {
        self.keep(Some(account));
        if status == DeviceStatus::Revoked { self.device = None; }
    }

    fn device_for(&self, name: &str, service_url: &str) -> Option<DeviceKey> {
        self.device.as_ref().filter(|d| d.name == name && d.service_url == service_url).map(|d| d.key.clone())
    }
}

fn saved_path() -> Option<PathBuf> { Some(appearance::dir()?.join("connect-account.json")) }

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

/// Sem gerador seguro não há id, e sem id o pedido não sai: a liberação seria conferida só pelo nome do computador.
/// `Err`: o código para o diário.
fn install_id_from(random: std::io::Result<[u8; 16]>) -> Result<String, &'static str> {
    random.map(|bytes| hex(&bytes)).map_err(|_| Code::ConnectSecureRandomUnavailable.as_str())
}

/// O arquivo guardado. Arquivo que existe e não foi lido nunca passa por primeiro acesso: a conta e a credencial
/// deste computador podem estar nele.
enum Loaded { Missing, Read(Saved), Unreadable(&'static str) }

fn parse_saved(read: std::io::Result<Vec<u8>>) -> Loaded {
    match read {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Loaded::Missing,
        Err(_) => Loaded::Unreadable("read_failed"),
        Ok(bytes) => serde_json::from_slice(&bytes).map_or(Loaded::Unreadable("parse_failed"), Loaded::Read),
    }
}

fn load_saved() -> Loaded {
    match saved_path() {
        Some(path) => parse_saved(std::fs::read(path)),
        None => Loaded::Unreadable("no_config_dir"),
    }
}

/// De onde a entrada começa. `Some(código)`: o arquivo não foi lido, a tela avisa e ele só é regravado quando a pessoa
/// entrar.
fn starting_point(loaded: Loaded) -> (Saved, Option<&'static str>) {
    match loaded {
        Loaded::Read(saved) => (saved, None),
        Loaded::Missing => (Saved::default(), None),
        Loaded::Unreadable(code) => (Saved::default(), Some(code)),
    }
}

/// Gravações em ordem: uma resposta atrasada não grava por cima da conta mais nova.
static WRITTEN: Mutex<u64> = Mutex::new(0);
static NEXT_WRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn write_saved(generation: u64, saved: &Saved) -> std::io::Result<()> {
    let mut last = WRITTEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if generation < *last { return Ok(()); }
    *last = generation;
    let path = saved_path().ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?;
    let dir = path.parent().ok_or_else(|| std::io::Error::other("caminho sem pasta"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        options.mode(0o600);
    }
    let bytes = serde_json::to_vec(saved).map_err(std::io::Error::other)?;
    std::io::Write::write_all(&mut options.open(&tmp)?, &bytes)?;
    std::fs::rename(&tmp, &path)
}

fn device_name() -> String { format!("Hangar desktop {}", super::invite::device_label()) }

type Client = Arc<ConnectClient<ReqwestTransport>>;

fn new_client() -> Result<Client, String> {
    ReqwestTransport::new(reqwest::Client::builder()).map(|transport| Arc::new(ConnectClient::new(transport).on_warning(crate::log_line)))
        .map_err(|error| failure_text(&error, false))
}

/// A frase de cada falha pelo código do cliente. Nome desconhecido e senha recusada têm uma frase só, e rede tem a dela:
/// máquina fora do ar nunca aparece como senha errada.
fn failure_text(error: &hangar_connect_client::Error, creating: bool) -> String {
    let key = match error.code() {
        Code::CredentialsMismatch => "login_credentials_mismatch",
        Code::NetworkFailed => "connect_error_network",
        Code::ConnectTooManyAttempts | Code::SyncTooManyAttempts => "connect_error_limit",
        Code::SyncInsecure | Code::SyncUnavailable | Code::SyncRequestFailed | Code::SyncVaultUnreadable => "login_guest_unavailable",
        Code::ConnectDeviceDenied => "connect_error_denied",
        Code::ConnectApprovalExpired => "connect_error_expired",
        Code::ConnectTokenLost => "connect_error_token_lost",
        Code::ConnectSecureRandomUnavailable => "connect_error_secure_random",
        Code::ConnectDevicePending => "connect_error_device_pending",
        Code::ConnectDeviceRevoked => "connect_error_device_revoked",
        Code::ConnectDeviceRequired => "connect_error_device_required",
        _ => match error.status() {
            Some(401) if !creating => "login_credentials_mismatch",
            Some(409) if creating => "connect_error_name_taken",
            Some(400) if creating => "connect_account_rules",
            Some(403) => "connect_error_inactive",
            Some(status) if status < 500 => "connect_error_refused",
            _ => "connect_error_unavailable",
        },
    };
    tr_shared(key, &[])
}

/// Quem é dono da máquina da convidada, pelo host do Connect (`<máquina>.<conta>.hangar.dev.br`); fora dele, o host.
fn guest_owner(base_url: &str) -> String {
    let host = url::Url::parse(base_url).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_else(|| base_url.to_owned());
    match host.strip_suffix(".hangar.dev.br").and_then(|rest| rest.split_once('.')) {
        Some((_, account)) if !account.is_empty() && !account.contains('.') => tr_shared("login_guest_owner", &[("owner", account)]),
        _ => host,
    }
}

fn enter_label(n: usize) -> String {
    if n == 1 { tr_shared("login_guest_enter_one", &[]) } else { tr_shared("login_guest_enter_many", &[("n", &n.to_string())]) }
}

/// As máquinas que já vêm marcadas: as ligadas que ainda não estão neste app.
pub(crate) fn default_picks(list: &[ConnectMachine], saved: &[ServerEntry]) -> Vec<String> {
    list.iter().filter(|m| m.online && !in_app(saved, &m.host)).map(|m| m.machine_id.clone()).collect()
}

fn in_app(saved: &[ServerEntry], host: &str) -> bool {
    let address = super::servers::norm(&format!("https://{host}"));
    saved.iter().any(|s| super::servers::norm(&s.address) == address || s.connect.as_ref().is_some_and(|c| super::servers::norm(&c.url) == address))
}

/// A entrada de uma máquina no app, da conta ou da convidada: o endereço com a credencial que a entrada deu, como a rota
/// já guarda o Connect salvo como endereço.
fn server_entry(id: String, label: String, address: String, token: String) -> ServerEntry {
    ServerEntry { id, label, address, token, ..Default::default() }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Row { Asking, Waiting, Failed(String), Added }

/// As máquinas que "Entrar" leva: ligadas, marcadas e que ainda não entraram.
fn to_enter<'a>(list: &'a [ConnectMachine], picked: &'a [String], rows: &'a HashMap<String, Row>) -> impl Iterator<Item = &'a ConnectMachine> {
    list.iter().filter(|m| m.online && picked.contains(&m.machine_id) && rows.get(&m.machine_id) != Some(&Row::Added))
}

/// As falhas da rodada, na ordem da lista. Só das marcadas: a de uma rodada anterior, desmarcada depois, já foi dita.
fn failures(list: &[ConnectMachine], picked: &[String], rows: &HashMap<String, Row>) -> Vec<String> {
    list.iter().filter(|m| picked.contains(&m.machine_id)).filter_map(|m| match rows.get(&m.machine_id) {
        Some(Row::Failed(error)) => Some(error.clone()),
        _ => None,
    }).collect()
}

#[derive(Default)]
enum Stage {
    #[default]
    Form,
    /// Este computador espera a liberação da conta; `revoked`: a conta tirou o acesso dele.
    Release { name: String, session: AccountSession, revoked: bool },
    Machines { name: String, session: AccountSession, list: Option<Result<Vec<ConnectMachine>, String>>, picked: Vec<String>, rows: HashMap<String, Row> },
    Guest { user: String, servers: Vec<VaultServer>, picked: Vec<String> },
}

struct Form { user: Entity<InputState>, password: Entity<InputState>, _subscriptions: Vec<Subscription> }

#[derive(Default)]
pub(in crate::app) struct SignIn {
    form: Option<Form>,
    creating: bool,
    others: bool,
    /// Pedido em voo; resposta de outro número é de uma tela que já saiu.
    seq: u64,
    busy: bool,
    error: Option<String>,
    stage: Stage,
    saved: Option<Saved>,
    /// As máquinas que entraram nesta rodada, já gravadas: a primeira abre quando a rodada termina.
    added: Vec<ServerEntry>,
    /// Um cliente para a entrada inteira, releituras da espera incluídas: cada um abre as próprias conexões.
    client: Option<Client>,
    /// A gravação falhou e a tela já disse: as releituras da espera regravam a cada volta e não repetem o aviso.
    save_warned: bool,
}

impl SignIn {
    /// Fora do formulário a entrada segue na tela mesmo depois que a primeira máquina entrou na lista.
    pub(super) fn in_flow(&self) -> bool { !matches!(self.stage, Stage::Form) || self.busy }
}

enum Reply {
    Signed(Result<SignInResult, String>, String),
    Device(Result<(AccountSession, DeviceStatus), String>),
    /// A sessão da conta caiu (401 do serviço): a entrada volta ao formulário.
    Ended,
    Machines(Result<Vec<ConnectMachine>, String>),
    Pending(String),
    Entered(String, Result<MachineEntry, String>),
    Done,
}

/// `install_id`: só o pedido novo o leva; sem ele esse pedido não sai e a falha volta à tela.
async fn check_device(client: Result<Client, String>, session: AccountSession, install_id: Result<String, String>) -> Reply {
    let client = match client { Ok(client) => client, Err(error) => return Reply::Device(Err(error)) };
    let fail = |error: hangar_connect_client::Error| if error.status() == Some(401) { Reply::Ended } else { Reply::Device(Err(failure_text(&error, false))) };
    if let Some(key) = session.device.clone() {
        match client.device_status(&session, &key).await {
            Ok(status) => return Reply::Device(Ok((session, status))),
            // 404: o pedido venceu sem resposta (ou a conta não o conhece): vale um pedido novo.
            Err(error) if error.status() == Some(404) => {}
            Err(error) => return fail(error),
        }
    }
    let install_id = match install_id { Ok(id) => id, Err(error) => return Reply::Device(Err(error)) };
    match client.register_device(&session, &device_name(), Some(install_id.as_str())).await {
        Ok(key) => Reply::Device(Ok((AccountSession { device: Some(key), ..session }, DeviceStatus::Pending))),
        Err(error) => fail(error),
    }
}

impl Hangar {
    /// A entrada pelo Connect aparece no primeiro acesso (nenhuma máquina guardada) e segue até a rodada terminar.
    pub(super) fn sign_in_shown(&self) -> bool { self.sign_in.in_flow() || self.servers.iter().all(|s| s.ephemeral) }

    pub(super) fn sign_in_others_open(&self) -> bool { self.sign_in.others }

    /// Com "Outras formas de entrar" recolhido o endereço não está na tela: o foco vai para "Conta ou usuário", senão
    /// a pessoa digitaria num campo escondido. `false` quando quem chamou deve focar o campo dele.
    pub(super) fn sign_in_takes_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.sign_in_shown() || self.sign_in.others { return false; }
        self.sign_in_ensure(window, cx);
        if let Some(form) = self.sign_in.form.as_ref() { form.user.update(cx, |input, cx| input.focus(window, cx)); }
        true
    }

    fn sign_in_spawn<F, Fut>(&mut self, window: &mut Window, cx: &mut Context<Self>, work: F)
    where F: FnOnce(async_channel::Sender<Reply>) -> Fut, Fut: Future<Output = ()> + Send + 'static {
        self.sign_in.seq += 1;
        let seq = self.sign_in.seq;
        let (tx, rx) = async_channel::unbounded();
        self.runtime.spawn(work(tx));
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(reply) = rx.recv().await {
                if this.update_in(cx, |this, window, cx| this.sign_in_reply(seq, reply, window, cx)).is_err() { break; }
            }
        }).detach();
    }

    fn sign_in_store(&mut self, account: Option<SavedAccount>, window: &mut Window, cx: &mut Context<Self>) {
        self.sign_in_save(|saved| saved.keep(account), true, window, cx);
    }

    fn sign_in_client(&mut self) -> Result<Client, String> {
        if let Some(client) = &self.sign_in.client { return Ok(client.clone()); }
        let client = new_client()?;
        self.sign_in.client = Some(client.clone());
        Ok(client)
    }

    /// `on_screen`: a gravação vem do que a pessoa fez, e a falha também aparece na tela; a da abertura vai só ao diário.
    fn sign_in_save(&mut self, change: impl FnOnce(&mut Saved), on_screen: bool, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.sign_in.saved.get_or_insert_with(|| starting_point(load_saved()).0);
        change(saved);
        let (copy, generation) = (saved.clone(), NEXT_WRITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let (done, written) = futures::channel::oneshot::channel();
        self.runtime.spawn_blocking(move || { let _ = done.send(write_saved(generation, &copy)); });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = written.await else { return };
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => this.sign_in.save_warned = false,
                Err(error) => {
                    crate::log_line(&format!("connect: a conta deste computador não foi guardada codigo={:?}", error.kind()));
                    if on_screen && !this.sign_in.save_warned {
                        this.sign_in.save_warned = true;
                        window.push_notification(Notification::warning(tr_shared("connect_saved_not_written", &[])), cx);
                    }
                }
            });
        }).detach();
    }

    /// O id desta instalação para o pedido que o leva. Sem gerador seguro o pedido não sai: a falha vai ao diário e
    /// volta como a frase da tela.
    fn sign_in_install_id(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        if let Some(id) = self.sign_in.saved.as_ref().map(|s| s.install_id.clone()).filter(|id| !id.is_empty()) { return Ok(id); }
        match install_id_from(hangar_connect_client::crypto::random::<16>()) {
            Ok(id) => {
                let copy = id.clone();
                self.sign_in_save(|saved| saved.install_id = copy, true, window, cx);
                Ok(id)
            }
            Err(code) => {
                crate::log_line(&format!("connect: o id desta instalação não foi gerado codigo={code}"));
                Err(tr_shared("connect_error_secure_random", &[]))
            }
        }
    }

    /// Os campos nascem na primeira vez que a entrada aparece; com a conta guardada, ela retoma de onde parou.
    fn sign_in_ensure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sign_in.form.is_none() {
            let user = cx.new(|cx| InputState::new(window, cx).placeholder(tr_shared("login_account_or_user_hint", &[])));
            let password = cx.new(|cx| InputState::new(window, cx).masked(true));
            let subscriptions = [&user, &password].map(|input| cx.subscribe_in(input, window,
                |this: &mut Hangar, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) { this.sign_in_submit(window, cx); }
                }));
            self.sign_in.form = Some(Form { user, password, _subscriptions: subscriptions.into() });
        }
        if self.sign_in.saved.is_some() { return; }
        let (mut saved, unread) = starting_point(load_saved());
        if let Some(code) = unread {
            crate::log_line(&format!("connect: a conta guardada deste computador não foi lida codigo={code}"));
            self.sign_in.error = Some(tr_shared("connect_saved_unreadable", &[]));
        }
        // Sem gerador agora, o pedido que precisar do id tenta de novo e diz na tela.
        let mut fresh = false;
        if saved.install_id.is_empty() && let Ok(id) = install_id_from(hangar_connect_client::crypto::random::<16>()) {
            (saved.install_id, fresh) = (id, true);
        }
        self.sign_in.saved = Some(saved.clone());
        if fresh && unread.is_none() { self.sign_in_save(|_| {}, false, window, cx); }
        if let Some(account) = saved.account {
            self.sign_in.busy = true;
            self.sign_in_check_device(account.session, window, cx);
        }
    }

    fn sign_in_check_device(&mut self, session: AccountSession, window: &mut Window, cx: &mut Context<Self>) {
        let install_id = self.sign_in_install_id(window, cx);
        let client = self.sign_in_client();
        self.sign_in_spawn(window, cx, move |tx| async move { let _ = tx.send(check_device(client, session, install_id).await).await; });
    }

    fn sign_in_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sign_in.busy || !matches!(self.sign_in.stage, Stage::Form) { return; }
        let Some(form) = self.sign_in.form.as_ref() else { return };
        let (name, password) = (form.user.read(cx).value().trim().to_string(), form.password.read(cx).value().to_string());
        if name.is_empty() || password.is_empty() {
            self.sign_in.error = Some(tr_shared("login_fields_required", &[]));
            cx.notify();
            return;
        }
        // A senha sai do campo assim que o pedido parte.
        form.password.update(cx, |state, cx| state.set_value("", window, cx));
        let creating = self.sign_in.creating;
        (self.sign_in.busy, self.sign_in.error) = (true, None);
        let client = self.sign_in_client();
        self.sign_in_spawn(window, cx, move |tx| async move {
            let user = normalize_name(&name);
            let result = match client {
                Err(error) => Err(error),
                Ok(client) if creating => client.sign_up(CONNECT_SERVICE_URL, &user, &password).await.map(SignInResult::Account)
                    .map_err(|error| failure_text(&error, true)),
                Ok(client) => client.sign_in_anywhere(CONNECT_SERVICE_URL, &user, &password).await.map_err(|error| failure_text(&error, false)),
            };
            let _ = tx.send(Reply::Signed(result, user)).await;
        });
        cx.notify();
    }

    /// Sair: a conta sai do disco (o id desta instalação e a credencial deste computador ficam) e a entrada volta ao
    /// formulário.
    fn sign_in_sign_out(&mut self, notice: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.sign_in.stage, Stage::Guest { .. }) { self.sign_in_store(None, window, cx); }
        self.sign_in.seq += 1;
        (self.sign_in.stage, self.sign_in.busy, self.sign_in.error) = (Stage::Form, false, notice);
        self.sign_in.added = Vec::new();
        cx.notify();
    }

    fn sign_in_load_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Stage::Machines { session, list, .. } = &mut self.sign_in.stage else { return };
        *list = None;
        let session = session.clone();
        let client = self.sign_in_client();
        self.sign_in_spawn(window, cx, move |tx| async move {
            let reply = match client {
                Err(error) => Reply::Machines(Err(error)),
                Ok(client) => match client.list_machines(&session).await {
                    Err(error) if error.status() == Some(401) => Reply::Ended,
                    result => Reply::Machines(result.map_err(|error| failure_text(&error, false))),
                },
            };
            let _ = tx.send(reply).await;
        });
        cx.notify();
    }

    fn sign_in_toggle(&mut self, id: String, on: bool, cx: &mut Context<Self>) {
        let (Stage::Machines { picked, .. } | Stage::Guest { picked, .. }) = &mut self.sign_in.stage else { return };
        picked.retain(|p| *p != id);
        if on { picked.push(id); }
        cx.notify();
    }

    fn sign_in_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sign_in.busy { return; }
        if matches!(self.sign_in.stage, Stage::Guest { .. }) { return self.sign_in_enter_guest(window, cx); }
        if !matches!(&self.sign_in.stage, Stage::Machines { list: Some(Ok(list)), picked, rows, .. } if to_enter(list, picked, rows).next().is_some()) {
            return;
        }
        let install_id = self.sign_in_install_id(window, cx);
        let client = self.sign_in_client();
        let Stage::Machines { session, list: Some(Ok(list)), picked, rows, .. } = &mut self.sign_in.stage else { return };
        let chosen: Vec<ConnectMachine> = to_enter(list, picked, rows).cloned().collect();
        // Sem o id o pedido não sai: cada máquina marcada mostra o porquê.
        let install_id = match install_id {
            Ok(id) => id,
            Err(error) => {
                for machine in &chosen { rows.insert(machine.machine_id.clone(), Row::Failed(error.clone())); }
                cx.notify();
                return;
            }
        };
        for machine in &chosen { rows.insert(machine.machine_id.clone(), Row::Asking); }
        let session = session.clone();
        (self.sign_in.busy, self.sign_in.added) = (true, Vec::new());
        self.sign_in_spawn(window, cx, move |tx| async move {
            let client = match client {
                Ok(client) => client,
                Err(error) => {
                    for machine in &chosen { let _ = tx.send(Reply::Entered(machine.machine_id.clone(), Err(error.clone()))).await; }
                    let _ = tx.send(Reply::Done).await;
                    return;
                }
            };
            let name = device_name();
            let (client, session, name, install_id) = (&client, &session, &name, &install_id);
            let entries = chosen.iter().map(|machine| {
                let tx = tx.clone();
                async move {
                    let (pending, id) = (tx.clone(), machine.machine_id.clone());
                    let notify = id.clone();
                    let result = client.add_machine(session, machine, name, Some(install_id.as_str()), move || { let _ = pending.try_send(Reply::Pending(notify)); }).await;
                    let _ = tx.send(Reply::Entered(id, result.map_err(|error| failure_text(&error, false)))).await;
                }
            });
            futures::future::join_all(entries).await;
            let _ = tx.send(Reply::Done).await;
        });
        cx.notify();
    }

    fn sign_in_enter_guest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Stage::Guest { servers: list, picked, .. } = &self.sign_in.stage else { return };
        let base = super::servers::new_id();
        let entries: Vec<ServerEntry> = list.iter().filter(|s| picked.contains(&s.id)).enumerate()
            .map(|(n, server)| server_entry(format!("{base}{n}"), server.label.clone(), server.base_url.clone(), server.token.clone())).collect();
        let Some(first) = entries.first().cloned() else { return };
        self.merge_servers(entries, cx);
        self.sign_in_finish(first, window, cx);
    }

    /// A rodada terminou com máquina na lista: a entrada some e a primeira abre.
    fn sign_in_finish(&mut self, first: ServerEntry, window: &mut Window, cx: &mut Context<Self>) {
        let (saved, form, client) = (self.sign_in.saved.take(), self.sign_in.form.take(), self.sign_in.client.take());
        self.sign_in = SignIn { saved, form, client, seq: self.sign_in.seq + 1, ..SignIn::default() };
        self.activate_server(first, window, cx);
    }

    fn sign_in_reply(&mut self, seq: u64, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        if seq != self.sign_in.seq { return; }
        match reply {
            Reply::Signed(Err(error), _) => (self.sign_in.busy, self.sign_in.error) = (false, Some(error)),
            Reply::Signed(Ok(SignInResult::Guest { servers, .. }), user) => {
                let picked = servers.iter().map(|s| s.id.clone()).collect();
                (self.sign_in.busy, self.sign_in.stage) = (false, Stage::Guest { user, servers, picked });
            }
            Reply::Signed(Ok(SignInResult::Account(session)), name) => {
                // A credencial deste computador na mesma conta é reaproveitada: registrar outra pediria a liberação de novo.
                let device = self.sign_in.saved.as_ref().and_then(|s| s.device_for(&name, &session.service_url));
                let session = AccountSession { device, ..session };
                self.sign_in_store(Some(SavedAccount { name, session: session.clone() }), window, cx);
                self.sign_in_check_device(session, window, cx);
            }
            Reply::Device(Err(error)) => {
                self.sign_in.busy = false;
                // Na espera a falha de uma releitura não tira a tela; a próxima tenta de novo.
                if matches!(self.sign_in.stage, Stage::Release { .. }) { self.sign_in_recheck(window, cx); } else { self.sign_in.error = Some(error); }
            }
            Reply::Device(Ok((session, status))) => {
                self.sign_in.busy = false;
                let name = self.sign_in.saved.as_ref().and_then(|s| s.account.as_ref()).map(|a| a.name.clone()).unwrap_or_default();
                let account = SavedAccount { name: name.clone(), session: session.clone() };
                self.sign_in_save(|saved| saved.answered(account, status), true, window, cx);
                if status == DeviceStatus::Approved {
                    self.sign_in.stage = Stage::Machines { name, session, list: None, picked: Vec::new(), rows: HashMap::new() };
                    self.sign_in_load_machines(window, cx);
                } else {
                    let revoked = status == DeviceStatus::Revoked;
                    self.sign_in.stage = Stage::Release { name, session, revoked };
                    if !revoked { self.sign_in_recheck(window, cx); }
                }
            }
            Reply::Ended => self.sign_in_sign_out(Some(tr_shared("connect_session_ended", &[])), window, cx),
            Reply::Machines(result) => {
                let saved = self.servers.clone();
                if let Stage::Machines { list, picked, .. } = &mut self.sign_in.stage {
                    if let Ok(machines) = &result { *picked = default_picks(machines, &saved); }
                    *list = Some(result);
                }
            }
            Reply::Pending(id) => if let Stage::Machines { rows, .. } = &mut self.sign_in.stage { rows.insert(id, Row::Waiting); },
            Reply::Entered(id, result) => {
                let row = match result {
                    Ok(entry) => {
                        // O token sai uma vez só: vai para a lista na hora.
                        let server_id = format!("{}{}", super::servers::new_id(), self.sign_in.added.len());
                        let server = server_entry(server_id, entry.label, entry.base_url, entry.token);
                        self.merge_servers(vec![server.clone()], cx);
                        self.sign_in.added.push(server);
                        Row::Added
                    }
                    Err(error) => Row::Failed(error),
                };
                if let Stage::Machines { rows, .. } = &mut self.sign_in.stage { rows.insert(id, row); }
            }
            Reply::Done => {
                self.sign_in.busy = false;
                if let Some(first) = self.sign_in.added.first().cloned() {
                    let failed = match &self.sign_in.stage {
                        Stage::Machines { list: Some(Ok(list)), picked, rows, .. } => failures(list, picked, rows),
                        _ => Vec::new(),
                    };
                    self.sign_in_finish(first, window, cx);
                    // As que não entraram não somem caladas: a primeira já abriu, e o aviso diz quantas faltaram.
                    if !failed.is_empty() { window.push_notification(Notification::warning(failed.join("\n")), cx); }
                }
            }
        }
        cx.notify();
    }

    fn sign_in_recheck(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.sign_in.seq;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(RECHECK).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.sign_in.seq != seq || !this.connection_dialog { return; }
                if let Stage::Release { session, revoked: false, .. } = &this.sign_in.stage {
                    let session = session.clone();
                    this.sign_in_check_device(session, window, cx);
                }
            });
        }).detach();
    }

    /// Pedir de novo depois de a conta tirar o acesso: a credencial antiga sai e nasce outro pedido.
    fn sign_in_request_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Stage::Release { session, .. } = &self.sign_in.stage else { return };
        let session = AccountSession { device: None, ..session.clone() };
        // O pedido novo leva o id; sem ele a espera ignoraria a falha, então ela fica na tela aqui.
        if let Err(error) = self.sign_in_install_id(window, cx) {
            self.sign_in.error = Some(error);
            cx.notify();
            return;
        }
        self.sign_in.busy = true;
        self.sign_in_check_device(session, window, cx);
        cx.notify();
    }

    /// `tag`: o selo da convidada antes do nome.
    fn render_account_top(tag: Option<String>, label: String, cx: &mut Context<Self>) -> Div {
        account_top(tag, label, sign_out_button("sign-in-out").on_click(cx.listener(|this, _, window, cx| this.sign_in_sign_out(None, window, cx))))
    }

    fn render_pick_row(id: String, title: String, subtitle: String, dot: Option<bool>, on: bool, enabled: bool, row: Option<&Row>, cx: &mut Context<Self>) -> Div {
        let check = pick_checkbox(&format!("sign-in-pick-{id}"), title.clone(), on, enabled)
            .on_click(cx.listener(move |this, on: &bool, _, cx| this.sign_in_toggle(id.clone(), *on, cx)));
        pick_row(title, subtitle, dot, row, check)
    }

    /// O alto do cartão de conexão na entrada pelo Connect: o formulário, a espera da liberação ou a escolha das máquinas.
    pub(super) fn render_first_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.sign_in_ensure(window, cx);
        let busy = self.sign_in.busy;
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        let alert = |text: String| div().id("sign-in-error").role(Role::Alert).px_3().py(px(10.)).rounded(px(8.)).bg(theme::danger().alpha(0.12))
            .text_sm().text_color(theme::danger()).whitespace_normal().child(text);
        let card = sign_in_card();
        match &self.sign_in.stage {
            Stage::Form => {
                let Some(form) = self.sign_in.form.as_ref() else { return card };
                let field = |label: String, input: &Entity<InputState>| form_field(label.clone(), Input::new(input).disabled(busy).aria_label(label));
                let creating = self.sign_in.creating;
                card.child(form_heading("sign-in-mark", busy, cx.reduce_motion()))
                    .child(field(tr_shared("login_account_or_user", &[]), &form.user))
                    .child(field(tr("login_senha"), &form.password))
                    .when(creating, |el| el.child(muted(tr_shared("connect_account_rules", &[]))))
                    .when_some(self.sign_in.error.clone(), |el, error| el.child(alert(error)))
                    .child(submit_button("sign-in-go", creating, busy).on_click(cx.listener(|this, _, window, cx| this.sign_in_submit(window, cx))))
                    .child(div().flex().justify_center().child(Button::new("sign-in-mode").ghost().small().disabled(busy)
                        .label(tr_shared(if creating { "connect_have_account" } else { "connect_create_free" }, &[]))
                        .on_click(cx.listener(|this, _, _, cx| {
                            (this.sign_in.creating, this.sign_in.error) = (!this.sign_in.creating, None);
                            cx.notify();
                        }))))
            }
            Stage::Release { name, revoked, .. } => {
                let code = self.sign_in.saved.as_ref().map(|s| s.install_id.as_str()).filter(|id| !id.is_empty()).map(install_code);
                let wait = div().id("sign-in-release").role(Role::Status).flex().flex_col().gap(px(6.)).p_3().rounded(px(10.)).border_1()
                    .border_color(theme::warning()).bg(theme::warning().alpha(0.1)).text_sm().text_color(theme::warning()).whitespace_normal();
                let wait = if *revoked { wait.child(tr_shared("connect_device_revoked_notice", &[])) } else {
                    wait.child(div().font_weight(FontWeight::SEMIBOLD).child(tr_shared("connect_release_waiting", &[])))
                        .child(tr_shared("connect_release_body", &[]))
                };
                card.child(Self::render_account_top(None, tr_shared("connect_signed_in_as", &[("name", name)]), cx))
                    .child(section_heading(tr_shared("connect_release_title_desktop", &[])))
                    .child(wait)
                    .when(!*revoked, |el| match code {
                        Some(code) => el.child(div().text_sm().text_color(theme::muted()).text_center().child(tr_shared("connect_release_code", &[])))
                            .child(div().text_center().font_family(theme::MONO).text_size(px(22.)).child(code)),
                        None => el.child(muted(tr_shared("connect_install_code_no_random", &[]))),
                    })
                    .when(*revoked, |el| el.child(Button::new("sign-in-again").primary().w_full().loading(busy).disabled(busy)
                        .label(tr_shared("connect_device_request_again", &[]))
                        .on_click(cx.listener(|this, _, window, cx| this.sign_in_request_again(window, cx)))))
                    .when_some(self.sign_in.error.clone(), |el, error| el.child(alert(error)))
            }
            Stage::Machines { name, list, picked, rows, .. } => {
                let top = Self::render_account_top(None, tr_shared("connect_signed_in_as", &[("name", name)]), cx);
                let card = card.child(top).child(section_heading(tr_shared("connect_your_machines", &[])));
                let list = match list {
                    None => return card.child(div().id("sign-in-machines-loading").role(Role::Status).child(muted(tr_shared("comum_carregando", &[])))),
                    Some(Err(error)) => return card.child(alert(error.clone()))
                        .child(Button::new("sign-in-machines-retry").outline().label(tr_shared("lista_tentar_novamente", &[]))
                            .on_click(cx.listener(|this, _, window, cx| this.sign_in_load_machines(window, cx)))),
                    Some(Ok(list)) if list.is_empty() => return card.child(muted(tr_shared("connect_machines_empty", &[]))),
                    Some(Ok(list)) => list.clone(),
                };
                let (picked, rows) = (picked.clone(), rows.clone());
                let count = to_enter(&list, &picked, &rows).count();
                let (mut card, added_row) = (card, Row::Added);
                for machine in &list {
                    let added = rows.get(&machine.machine_id) == Some(&added_row) || in_app(&self.servers, &machine.host);
                    let row = rows.get(&machine.machine_id).or(added.then_some(&added_row));
                    card = card.child(Self::render_pick_row(machine.machine_id.clone(), machine.name.clone(), machine_subtitle(machine), Some(machine.online),
                        picked.contains(&machine.machine_id) && machine.online, machine.online && !busy, row, cx));
                }
                card.child(enter_button("sign-in-enter", count, busy).on_click(cx.listener(|this, _, window, cx| this.sign_in_enter(window, cx))))
            }
            Stage::Guest { user, servers, picked } => {
                let top = Self::render_account_top(Some(tr_shared("login_guest_tag", &[])), user.clone(), cx);
                let mut card = card.child(top).child(section_heading(tr_shared("login_guest_machines", &[])));
                if servers.is_empty() { card = card.child(muted(tr_shared("connect_machines_empty", &[]))); }
                let count = servers.iter().filter(|s| picked.contains(&s.id)).count();
                for server in servers.clone() {
                    card = card.child(Self::render_pick_row(server.id.clone(), server.label.clone(), guest_owner(&server.base_url), None,
                        picked.contains(&server.id), true, None, cx));
                }
                card.child(enter_button("sign-in-enter", count, false).on_click(cx.listener(|this, _, window, cx| this.sign_in_enter(window, cx))))
                    .child(div().text_center().text_xs().text_color(theme::muted()).whitespace_normal().child(tr_shared("login_guest_hint", &[])))
            }
        }
    }

    /// "Outras formas de entrar": o cabeçalho que abre e fecha; o conteúdo é o cartão de sempre.
    pub(super) fn render_sign_in_others(&self, cx: &mut Context<Self>) -> Div {
        let open = self.sign_in.others;
        div().pt_3().border_t_1().border_color(theme::glass_border()).flex().justify_center()
            .child(Button::new("sign-in-others").ghost().small().icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                .label(tr_shared("login_other_ways", &[]))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.sign_in.others = !this.sign_in.others;
                    if this.sign_in.others && this.entry.is_none() { this.address.update(cx, |input, cx| input.focus(window, cx)); }
                    cx.notify();
                })))
    }
}

// As partes da tela por dados simples: a miniatura da novidade `entrar-pelo-connect` desenha com as mesmas.

/// O cartão da entrada: as partes abaixo, uma embaixo da outra.
pub(crate) fn sign_in_card() -> Div { div().flex().flex_col().gap_3() }

/// A marca do Connect, o título e a frase do formulário.
pub(crate) fn form_heading(mark_id: &str, busy: bool, reduce_motion: bool) -> Div {
    div().flex().flex_col().gap_3()
        .child(div().flex().justify_center().pt_2().child(super::connect::connect_mark(mark_id, 56., busy, reduce_motion)))
        .child(div().text_center().text_size(px(19.)).font_weight(FontWeight::SEMIBOLD).child(tr_shared("login_hangar_title", &[])))
        .child(div().text_center().text_sm().text_color(theme::muted()).whitespace_normal().child(tr_shared("login_hangar_subtitle", &[])))
}

pub(crate) fn form_field(label: String, input: impl IntoElement) -> Div {
    div().flex().flex_col().gap(px(4.)).child(div().text_xs().text_color(theme::muted()).child(label)).child(input)
}

pub(crate) fn submit_button(id: &'static str, creating: bool, busy: bool) -> Button {
    Button::new(id).primary().w_full().loading(busy).disabled(busy)
        .label(tr_shared(if creating { "connect_sign_up" } else if busy { "login_entrando" } else { "login_entrar" }, &[]))
}

pub(crate) fn section_heading(text: String) -> Div { div().text_base().font_weight(FontWeight::SEMIBOLD).child(text) }

pub(crate) fn sign_out_button(id: &'static str) -> Button { Button::new(id).ghost().small().label(tr_shared("connect_sign_out", &[])) }

/// A conta (ou a convidada, com o selo `tag`) e o Sair.
pub(crate) fn account_top(tag: Option<String>, label: String, sign_out: Button) -> Div {
    div().flex().items_center().gap_2().text_sm().text_color(theme::muted())
        .when_some(tag, |el, tag| el.child(div().px_2().py(px(2.)).rounded_full().bg(theme::accent_dim()).text_xs()
            .text_color(theme::accent_text()).child(tag)))
        .child(label)
        .child(div().flex_1())
        .child(sign_out)
}

pub(crate) fn pick_checkbox(id: &str, title: String, on: bool, enabled: bool) -> Checkbox {
    Checkbox::new(SharedString::from(id.to_owned())).checked(on).disabled(!enabled).accessibility_label(title)
}

/// Uma máquina da escolha: o ponto de ligada (só nas da conta), nome, endereço, o andamento da entrada e a marcação.
pub(crate) fn pick_row(title: String, subtitle: String, dot: Option<bool>, row: Option<&Row>, check: Checkbox) -> Div {
    let state = match row {
        Some(Row::Asking) => Some((tr_shared("login_entrando", &[]), theme::muted())),
        Some(Row::Waiting) => Some((tr_shared("connect_waiting", &[]), theme::warning())),
        Some(Row::Added) => Some((tr_shared("connect_machine_added", &[]), theme::success())),
        Some(Row::Failed(error)) => Some((error.clone(), theme::danger())),
        None => None,
    };
    div().flex().items_center().gap_3().px_3().py(px(10.)).rounded(px(10.)).border_1().border_color(theme::border())
        .when_some(dot, |el, online| el.child(div().size(px(9.)).rounded_full().flex_shrink_0()
            .bg(if online { theme::success() } else { theme::muted() })))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(title))
            .child(div().text_sm().text_color(theme::muted()).truncate().child(subtitle))
            .when_some(state, |el, (text, color)| el.child(div().text_sm().text_color(color).whitespace_normal().child(text))))
        .child(check)
}

pub(crate) fn machine_subtitle(machine: &ConnectMachine) -> String {
    if machine.online { machine.host.clone() } else { tr_shared("connect_machine_off_now", &[]) }
}

pub(crate) fn enter_button(id: &'static str, count: usize, busy: bool) -> Button {
    Button::new(id).primary().w_full().loading(busy).disabled(busy || count == 0).label(enter_label(count))
}

#[cfg(test)]
mod tests {
    use super::{Loaded, Reply, Row, Saved, SavedAccount, check_device, default_picks, failure_text, failures, guest_owner, in_app, install_code,
        install_id_from, new_client, parse_saved, server_entry, starting_point, to_enter};
    use std::collections::HashMap;
    use crate::i18n::tr_shared;
    use hangar_connect_client::{AccountSession, Code, ConnectMachine, DeviceKey, DeviceStatus, Error, MachineEntry};

    #[test]
    fn install_code_matches_the_shared_cases() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("../../../packages/core/src/installCode.fixtures.json")).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            assert_eq!(install_code(case["install_id"].as_str().unwrap()), case["code"].as_str().unwrap());
        }
    }

    #[test]
    fn wrong_name_or_password_has_one_text_and_network_has_its_own() {
        let mismatch = tr_shared("login_credentials_mismatch", &[]);
        assert_eq!(failure_text(&Error::new(Code::CredentialsMismatch, Some(401)), false), mismatch);
        assert_eq!(failure_text(&Error::new(Code::ConnectRequestFailed, Some(401)), false), mismatch);
        let network = failure_text(&Error::network("timeout"), false);
        assert_eq!(network, tr_shared("connect_error_network", &[]));
        assert_ne!(network, mismatch, "máquina fora do ar não pode parecer senha errada");
        assert_eq!(failure_text(&Error::new(Code::SyncVaultUnreadable, None), false), tr_shared("login_guest_unavailable", &[]));
        assert_eq!(failure_text(&Error::new(Code::ConnectRequestFailed, Some(409)), true), tr_shared("connect_error_name_taken", &[]));
        assert_eq!(failure_text(&Error::new(Code::ConnectRequestFailed, Some(502)), false), tr_shared("connect_error_unavailable", &[]));
    }

    #[test]
    fn guest_machine_says_whose_it_is() {
        assert_eq!(guest_owner("https://notebook.jeffersonfelizardo.hangar.dev.br"), tr_shared("login_guest_owner", &[("owner", "jeffersonfelizardo")]));
        assert_eq!(guest_owner("https://casa.tail.ts.net"), "casa.tail.ts.net");
    }

    fn machine(id: &str, online: bool) -> ConnectMachine {
        ConnectMachine { machine_id: id.into(), name: id.into(), host: format!("{id}.jeff.hangar.dev.br"), online }
    }

    #[test]
    fn online_machines_not_in_the_app_come_checked() {
        let list = [machine("notebook", true), machine("casa", true), machine("vps", false)];
        let saved = [crate::app::servers::ServerEntry { address: "https://casa.jeff.hangar.dev.br/".into(), ..Default::default() }];
        assert_eq!(default_picks(&list, &saved), vec!["notebook".to_string()]);
        assert!(in_app(&saved, "casa.jeff.hangar.dev.br"));
    }

    #[test]
    fn entered_machine_is_saved_with_the_device_credential() {
        let entry = MachineEntry { id: "m1".into(), label: "notebook".into(), base_url: "https://notebook.jeff.hangar.dev.br".into(),
            token: "device-token".into(), device_id: "d1".into() };
        let server = server_entry("srv-1".into(), entry.label, entry.base_url, entry.token);
        assert_eq!((server.address.as_str(), server.token.as_str(), server.label.as_str()), ("https://notebook.jeff.hangar.dev.br", "device-token", "notebook"));
        assert!(!server.invite && !server.disabled && server.connect.is_none());
    }

    #[test]
    fn enter_takes_the_picked_online_machines_and_reports_only_this_round() {
        let list = [machine("notebook", true), machine("casa", true), machine("vps", false), machine("escritorio", true)];
        let picked: Vec<String> = ["notebook", "casa", "vps"].map(String::from).into();
        let rows = HashMap::from([("casa".to_string(), Row::Added), ("notebook".to_string(), Row::Failed("rede".into())),
            ("escritorio".to_string(), Row::Failed("antiga".into()))]);
        let ids: Vec<&str> = to_enter(&list, &picked, &rows).map(|m| m.machine_id.as_str()).collect();
        assert_eq!(ids, ["notebook"], "desligada, já dentro e desmarcada ficam fora");
        // A falha da desmarcada é de uma rodada anterior e não volta no aviso.
        assert_eq!(failures(&list, &picked, &rows), ["rede"]);
    }

    #[test]
    fn saved_account_keeps_the_device_and_the_install_id() {
        let session = AccountSession { service_url: "https://connect.hangar.dev.br".into(), token: "t".into(),
            device: Some(DeviceKey { id: "d".into(), secret: "s".into() }) };
        let mut saved = Saved { install_id: "0a1b".into(), ..Saved::default() };
        saved.keep(Some(SavedAccount { name: "jeff".into(), session }));
        let back: Saved = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert!(back == saved);
        // Sem conta, o id desta instalação continua: sair não muda o código da próxima liberação.
        let out: Saved = serde_json::from_str(r#"{"install_id":"0a1b"}"#).unwrap();
        assert!(out.account.is_none() && out.device.is_none() && out.install_id == "0a1b");
    }

    #[test]
    fn a_saved_file_that_exists_but_does_not_read_is_never_a_first_access() {
        assert!(matches!(parse_saved(Err(std::io::ErrorKind::NotFound.into())), Loaded::Missing));
        assert!(matches!(parse_saved(Err(std::io::ErrorKind::PermissionDenied.into())), Loaded::Unreadable("read_failed")));
        assert!(matches!(parse_saved(Ok(b"{nao e json".to_vec())), Loaded::Unreadable("parse_failed")));
        assert!(matches!(parse_saved(Ok(br#"{"install_id":"0a1b"}"#.to_vec())), Loaded::Read(ref saved) if saved.install_id == "0a1b"));
        // Só o arquivo que falta começa calado; o ilegível leva o código para o diário e o aviso para a tela.
        assert_eq!(starting_point(Loaded::Missing).1, None);
        let (saved, unread) = starting_point(Loaded::Unreadable("parse_failed"));
        assert_eq!(unread, Some("parse_failed"));
        assert!(saved.account.is_none() && saved.install_id.is_empty(), "nada do arquivo ilegível é inventado");
        let read = Saved { install_id: "0a1b".into(), ..Saved::default() };
        assert!(starting_point(Loaded::Read(read.clone())) == (read, None));
    }

    #[test]
    fn no_secure_random_is_an_error_never_an_empty_id() {
        assert_eq!(install_id_from(Err(std::io::Error::other("sem fonte"))), Err("connect_secure_random_unavailable"));
        assert_eq!(install_id_from(Ok([0xab; 16])), Ok("ab".repeat(16)));
    }

    #[tokio::test]
    async fn a_new_request_without_the_install_id_is_not_sent() {
        let session = AccountSession { service_url: "http://127.0.0.1:9".into(), token: "t".into(), device: None };
        let client = new_client().expect("cliente do Connect");
        let reply = check_device(Ok(client), session, Err("sem gerador".into())).await;
        // Mandado, o pedido daria a frase da rede, não a do gerador.
        assert!(matches!(reply, Reply::Device(Err(ref error)) if error == "sem gerador"));
    }

    fn key(id: &str) -> DeviceKey { DeviceKey { id: id.into(), secret: format!("s-{id}") } }

    fn account(name: &str, device: Option<DeviceKey>) -> SavedAccount {
        SavedAccount { name: name.into(), session: AccountSession { service_url: "https://connect.hangar.dev.br".into(), token: "t".into(), device } }
    }

    #[test]
    fn signing_out_keeps_the_device_for_the_same_account() {
        let mut saved = Saved { install_id: "0a1b".into(), ..Saved::default() };
        saved.keep(Some(account("jeff", Some(key("d1")))));
        saved.keep(None);
        let back: Saved = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert!(back.account.is_none());
        assert_eq!(back.device_for("jeff", "https://connect.hangar.dev.br"), Some(key("d1")), "sair não apaga a credencial");
        assert_eq!(back.device_for("outra", "https://connect.hangar.dev.br"), None, "outra conta pede a dela");
        assert_eq!(back.device_for("jeff", "https://outro.servico"), None);
        // Entrar sem credencial (conta nova) não apaga a guardada; a registrada depois a troca.
        saved.keep(Some(account("outra", None)));
        assert_eq!(saved.device_for("jeff", "https://connect.hangar.dev.br"), Some(key("d1")));
        saved.keep(Some(account("outra", Some(key("d2")))));
        assert_eq!(saved.device_for("outra", "https://connect.hangar.dev.br"), Some(key("d2")));
    }

    #[test]
    fn a_revoked_device_is_dropped_and_an_approved_one_stays() {
        let mut saved = Saved { install_id: "0a1b".into(), ..Saved::default() };
        saved.answered(account("jeff", Some(key("d1"))), DeviceStatus::Approved);
        saved.keep(None);
        assert_eq!(saved.device_for("jeff", "https://connect.hangar.dev.br"), Some(key("d1")));
        saved.answered(account("jeff", Some(key("d1"))), DeviceStatus::Revoked);
        assert!(saved.account.is_some(), "a conta fica para a tela da perda do acesso");
        saved.keep(None);
        assert_eq!(saved.device_for("jeff", "https://connect.hangar.dev.br"), None, "revogada não é reaproveitada");
        // 404: o pedido novo chega com outra credencial, que troca a antiga.
        saved.answered(account("jeff", Some(key("d2"))), DeviceStatus::Pending);
        assert_eq!(saved.device_for("jeff", "https://connect.hangar.dev.br"), Some(key("d2")));
    }
}
