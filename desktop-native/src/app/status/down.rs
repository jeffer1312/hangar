//! Servidor parado (spec, bloco "Servidor parado"; mock, estado "Servidor parado"). Com o servidor selecionado
//! nesta máquina fora do ar, a página Status lê o serviço pelo `hangar-local-server` e oferece subir e consertar;
//! servidor de outra máquina fora só ganha o aviso, sem Subir. Leitura e ações rodam fora da thread da janela.
use super::*;
use super::super::device::DeviceReply;
use super::model::fill;
use hangar_local_server::{DEFAULT_PORT, Failure as LocalFailure, Listeners, LocalServer, LocalState, Os, PortHolder, Versions};

/// Entre leituras enquanto o serviço diz que está subindo: o gerenciador não avisa quando decide. O próprio
/// crate encerra a subida em `STARTING_LIMIT`, então a repetição tem fim.
const STARTING_POLL: Duration = Duration::from_millis(1500);

// ---------------------------------------------------------------------------------------------
// Modelo

/// Uma leitura do servidor desta máquina.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct Probe {
    pub state: LocalState,
    pub versions: Versions,
    pub port: u16,
    /// Subindo: alguém já escuta na porta.
    pub port_open: bool,
    /// Parado sem motivo registrado: as últimas linhas do serviço, que só o `Failure` traria.
    pub output: Vec<String>,
}

/// O que a pessoa já pediu nesta queda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Attempt { None, Running, Done }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase { Stopped, Starting, Failed }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step { Done, Running, Waiting }

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Fix { FreePort(PortHolder), Listeners, UseVersion(String), Reinstall }

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FixRow { pub fix: Fix, pub title: String, pub detail: String, pub button: String, pub primary: bool }

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DownView {
    pub phase: Phase,
    pub title: String,
    /// Parado: o motivo curto, depois de "Não responde"; Não subiu: o que aconteceu.
    pub detail: Option<String>,
    pub steps: Vec<(Step, String)>,
    pub fixes: Vec<FixRow>,
    pub output: Vec<String>,
    /// Sem serviço instalado não há o que subir: só o conserto.
    pub can_start: bool,
}

fn capitalize(text: String) -> String {
    let mut chars = text.chars();
    match chars.next() { Some(first) => first.to_uppercase().chain(chars).collect(), None => text }
}

/// "a unit hangar-backend.service", com o artigo de cada idioma.
fn service_name(os: Os) -> String {
    use hangar_local_server::service::{LAUNCHD_LABEL, TASK, UNIT};
    match os {
        Os::Linux => fill(tr("status_down_service_linux"), &[("name", UNIT)]),
        Os::Windows => fill(tr("status_down_service_windows"), &[("name", TASK)]),
        Os::Macos => fill(tr("status_down_service_macos"), &[("name", LAUNCHD_LABEL)]),
        Os::Other => tr("status_down_service_other"),
    }
}

fn with_detail(text: String, detail: &str) -> String {
    if detail.trim().is_empty() { text } else { format!("{text} ({})", detail.trim()) }
}

/// O motivo das falhas sem conserto próprio, em minúscula: entra no meio da frase.
fn reason(failure: &LocalFailure, os: Os) -> String {
    let service = service_name(os);
    match failure.code.as_str() {
        "service_exit" => fill(tr("status_down_reason_exit"), &[("service", &service), ("code", &failure.detail)]),
        "service_failed" => with_detail(fill(tr("status_down_reason_failed"), &[("service", &service)]), &failure.detail),
        "service_unreadable" => with_detail(tr("status_down_reason_unreadable"), &failure.detail),
        "server_exited" => with_detail(tr("status_down_reason_exited"), &failure.detail),
        "server_failed" => with_detail(tr("status_down_reason_server_failed"), &failure.detail),
        code => with_detail(fill(tr("status_down_reason_other"), &[("code", code)]), &failure.detail),
    }
}

/// A troca de versão só existe com pacote, no Linux e no Windows, e com outra versão inteira.
fn switchable(os: Os, versions: &Versions) -> Option<&str> {
    (matches!(os, Os::Linux | Os::Windows) && !versions.dev_mode).then_some(versions.previous.as_deref()).flatten()
}

/// O assistente de instalação só existe no Linux e no Windows.
fn reinstall(os: Os, primary: bool) -> Option<FixRow> {
    matches!(os, Os::Linux | Os::Windows).then(|| FixRow { fix: Fix::Reinstall, title: tr("status_down_fix_reinstall"),
        detail: tr("status_down_fix_reinstall_detail"), button: tr("status_down_fix_reinstall_button"), primary })
}

fn holder_name(holder: &PortHolder) -> String {
    if holder.name.is_empty() { tr("status_down_unknown_process") } else { holder.name.clone() }
}

fn failed(title: String, detail: Option<String>, fixes: Vec<FixRow>, output: Vec<String>) -> DownView {
    DownView { phase: Phase::Failed, title, detail, steps: Vec::new(), fixes, output, can_start: true }
}

fn stopped(reason: Option<String>) -> DownView {
    DownView { phase: Phase::Stopped, title: tr("status_down_stopped_title"), detail: reason, steps: Vec::new(), fixes: Vec::new(),
        output: Vec::new(), can_start: true }
}

/// O painel para uma leitura; `None` com o servidor de pé (a página normal volta).
pub(super) fn down_model(probe: &Probe, os: Os, attempt: Attempt) -> Option<DownView> {
    let port = probe.port.to_string();
    let starting = |service: Step, open: Step, prepare: Step| DownView {
        phase: Phase::Starting, title: tr("status_down_starting"), detail: None,
        steps: vec![(service, tr("status_down_step_service")), (open, fill(tr("status_down_step_port"), &[("port", &port)])),
            (prepare, tr("status_down_step_prepare"))],
        fixes: Vec::new(), output: Vec::new(), can_start: false,
    };
    // A ação ainda roda: o pedido ao gerenciador é o primeiro passo.
    if attempt == Attempt::Running { return Some(starting(Step::Running, Step::Waiting, Step::Waiting)); }
    match &probe.state {
        LocalState::Running => None,
        LocalState::Starting if probe.port_open => Some(starting(Step::Done, Step::Done, Step::Running)),
        LocalState::Starting => Some(starting(Step::Done, Step::Running, Step::Waiting)),
        LocalState::NotInstalled => Some(DownView { can_start: false, ..failed(tr("status_down_unit_title"),
            Some(capitalize(fill(tr("status_down_unit_detail"), &[("service", &service_name(os))]))), reinstall(os, true).into_iter().collect(), Vec::new()) }),
        LocalState::Stopped { failure: None } if attempt == Attempt::None => Some(stopped(None)),
        LocalState::Stopped { failure: None } => Some(failed(tr("status_down_clean_title"), Some(tr("status_down_clean_detail")), Vec::new(),
            probe.output.clone())),
        LocalState::Stopped { failure: Some(failure) } => Some(failure_view(failure, probe, os, attempt)),
    }
}

fn failure_view(failure: &LocalFailure, probe: &Probe, os: Os, attempt: Attempt) -> DownView {
    let output = failure.output.clone();
    let previous = switchable(os, &probe.versions);
    match failure.code.as_str() {
        "port_unavailable" => {
            let port = probe.port.to_string();
            let title = fill(tr("status_down_port_title"), &[("port", &port)]);
            let listeners = FixRow { fix: Fix::Listeners, title: tr("status_down_fix_listeners"),
                detail: fill(tr("status_down_fix_listeners_detail"), &[("port", &port)]), button: tr("status_down_fix_listeners_button"), primary: false };
            let Some(holder) = &failure.port_holder else { return failed(title, Some(tr("status_down_port_hidden")), vec![listeners], output) };
            let (name, pid) = (holder_name(holder), holder.pid.to_string());
            let params = [("name", name.as_str()), ("pid", pid.as_str())];
            let detail = fill(tr(if holder.same_user { "status_down_port_holder" } else { "status_down_port_holder_other" }), &params);
            // Processo de outro usuário: daqui não se encerra.
            let mut fixes = if holder.same_user {
                vec![FixRow { fix: Fix::FreePort(holder.clone()), title: fill(tr("status_down_fix_free"), &params),
                    detail: fill(tr("status_down_fix_free_detail"), &params), button: tr("status_down_fix_free_button"), primary: false }]
            } else { Vec::new() };
            fixes.push(listeners);
            failed(title, Some(detail), fixes, output)
        }
        code @ ("launch_no_server" | "launch_no_python") => {
            let title = tr(if code == "launch_no_server" { "status_down_no_server_title" } else { "status_down_no_python_title" });
            let mut detail = fill(tr("status_down_missing"), &[("path", &failure.detail)]);
            let mut fixes = Vec::new();
            if let Some(version) = previous {
                detail = format!("{detail} {}", fill(tr("status_down_previous_whole"), &[("version", version)]));
                fixes.push(FixRow { fix: Fix::UseVersion(version.to_owned()), title: fill(tr("status_down_fix_back"), &[("version", version)]),
                    detail: fill(tr("status_down_fix_back_detail"), &[("version", version)]), button: tr("status_down_fix_back_button"), primary: true });
            }
            fixes.extend(reinstall(os, fixes.is_empty()));
            failed(title, Some(detail), fixes, output)
        }
        "launch_no_current" => {
            let path = if failure.detail.is_empty() { "~/.hangar/current" } else { failure.detail.as_str() };
            let mut fixes = Vec::new();
            if let Some(version) = previous {
                // Sem `current` legível, a versão oferecida é a mais nova instalada.
                let why = tr(if probe.versions.current.is_none() { "status_down_fix_point_newest" } else { "status_down_fix_point_previous" });
                fixes.push(FixRow { fix: Fix::UseVersion(version.to_owned()), title: fill(tr("status_down_fix_point"), &[("version", version)]),
                    detail: why, button: tr("status_down_fix_point_button"), primary: true });
            }
            fixes.extend(reinstall(os, fixes.is_empty()));
            failed(tr("status_down_current_title"), Some(fill(tr("status_down_current_detail"), &[("path", path)])), fixes, output)
        }
        // Sem conserto próprio: antes de alguém tentar, é só o motivo da parada; depois, a falha com a saída.
        _ if attempt == Attempt::None => stopped(Some(reason(failure, os))),
        _ => failed(fill(tr("status_down_failed_title"), &[("reason", &reason(failure, os))]),
            (!output.is_empty()).then(|| tr("status_down_failed_detail")), Vec::new(), output),
    }
}

/// "Não responde há 2 min": desde quando a lista do servidor selecionado caiu.
pub(super) fn silent_for(elapsed: Duration) -> String {
    match elapsed.as_secs() / 60 {
        0 => tr("status_down_silent_moment"),
        minutes @ 1..60 => fill(tr("status_down_silent_minutes"), &[("n", &minutes.to_string())]),
        minutes => fill(tr("status_down_silent_hours"), &[("n", &(minutes / 60).to_string())]),
    }
}

/// O servidor selecionado está fora do ar. A lista também cai com 503 quando só os fatos dela falharam: de outra
/// máquina, só a conexão do retrato que também falhou confirma. `stream` é `None` sem essa conexão aberta (página
/// fechada), e então vale a lista; aberta, `Some(falhou)`.
pub(super) fn is_down(list_online: bool, list_error: bool, loopback: bool, stream: Option<bool>) -> bool {
    !list_online && list_error && (loopback || stream.is_none_or(|failed| failed))
}

/// Quem escuta na porta, para o diálogo do "Ver processos…": linhas (nome, pid, usuário) e as notas abaixo delas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListenersView { pub rows: Vec<[String; 3]>, pub notes: Vec<String> }

pub(super) fn listeners_model(listeners: &Listeners, port: u16) -> ListenersView {
    let rows = listeners.holders.iter().map(|holder| [holder_name(holder), fill(tr("status_down_listeners_pid"), &[("pid", &holder.pid.to_string())]),
        tr(if holder.same_user { "status_down_listeners_you" } else { "status_down_listeners_other_user" })]).collect();
    let mut notes = Vec::new();
    match &listeners.error {
        Some(error) => notes.push(fill(tr("status_down_listeners_error"), &[("detail", error)])),
        None if listeners.is_empty() => notes.push(fill(tr("status_down_listeners_none"), &[("port", &port.to_string())])),
        None => {}
    }
    if listeners.hidden { notes.push(tr("status_down_listeners_hidden")); }
    // Daqui só se encerra processo do próprio usuário: o diálogo diz por que não há o que fazer com os outros.
    if listeners.holders.iter().any(|holder| !holder.same_user) { notes.push(tr("status_down_listeners_other_note")); }
    ListenersView { rows, notes }
}

/// Frase de uma ação que falhou, pelo código do `ActionError`.
pub(super) fn action_error_text(code: &str, detail: &str) -> String {
    let key = format!("status_down_error_{code}");
    let text = tr(&key);
    if text == key { fill(tr("status_down_error_other"), &[("code", code), ("detail", detail)]) } else { fill(text, &[("detail", detail)]) }
}

// ---------------------------------------------------------------------------------------------
// Estado e tarefas

/// A queda do servidor selecionado: começa quando a lista cai e acaba quando ela volta ou o servidor troca.
pub(in crate::app) struct Down {
    key: String,
    /// Só com o servidor nesta máquina.
    local: Option<Local>,
    /// Redesenha a página a cada meio minuto, para o "há N min" andar; some com a queda.
    tick: Option<Task<()>>,
}

struct Local {
    port: u16,
    probe: Option<Probe>,
    tried: bool,
    /// Uma ação roda; o painel mostra a subida até a primeira leitura depois dela.
    busy: bool,
    error: Option<String>,
    generation: u64,
    task: Option<JoinHandle<()>>,
}

impl Drop for Local {
    fn drop(&mut self) { if let Some(task) = self.task.take() { task.abort(); } }
}

#[derive(Debug, Clone)]
enum Action { Start, FreePort(PortHolder), UseVersion(String) }

pub(in crate::app) enum LocalReply {
    Probe(Box<Probe>),
    /// A ação acabou; com erro, o código e o dado dele.
    Done(Option<(String, String)>),
}

fn read(server: &LocalServer) -> Probe {
    let state = server.probe();
    let port_open = state == LocalState::Starting && !server.listeners().is_empty();
    let output = if state == (LocalState::Stopped { failure: None }) { server.output() } else { Vec::new() };
    Probe { versions: server.versions(), port: server.port, port_open, output, state }
}

fn act(server: &LocalServer, action: &Action) -> Result<(), hangar_local_server::ActionError> {
    match action {
        Action::Start => server.start().map(drop),
        Action::FreePort(holder) => { server.free_port(holder)?; server.start().map(drop) }
        Action::UseVersion(version) => server.use_version(version).map(drop),
    }
}

/// Faz a ação (se houver) e lê o servidor; enquanto ele sobe, lê de novo. `StillStarting` segue pela leitura.
async fn local_task<F>(home: std::path::PathBuf, port: u16, action: Option<Action>, generation: u64, done: F)
where F: Fn(DeviceReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static {
    let send = |reply| done(DeviceReply::Status(0, Feed::Local(generation, reply)));
    if let Some(action) = action {
        let home = home.clone();
        let result = tokio::task::spawn_blocking(move || act(&LocalServer::new(&home, port), &action)).await;
        let error = match result {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some((e.code().to_owned(), e.detail())),
            Err(e) => Some(("local_server_io".to_owned(), e.to_string())),
        };
        send(LocalReply::Done(error)).await;
    }
    loop {
        let home = home.clone();
        let probe = match tokio::task::spawn_blocking(move || read(&LocalServer::new(&home, port))).await {
            Ok(probe) => probe,
            // A leitura não voltou: sem o erro, o painel ficaria girando em "Lendo" para sempre.
            Err(e) => return send(LocalReply::Done(Some(("local_server_io".to_owned(), e.to_string())))).await,
        };
        let starting = probe.state == LocalState::Starting;
        send(LocalReply::Probe(Box::new(probe))).await;
        if !starting { return; }
        tokio::time::sleep(STARTING_POLL).await;
    }
}

impl Hangar {
    /// O servidor selecionado não responde (`is_down`).
    fn server_down(&self) -> bool {
        let Some(api) = &self.api else { return false };
        let page = &self.device.status;
        is_down(self.list_online, self.list_error.is_some(), api.is_loopback(), page.stream.as_ref().map(|_| page.stream_failed))
    }

    /// Abre ou fecha a queda pelo estado da lista. Chamado no desenho da navegação e da página.
    pub(super) fn status_down_sync(&mut self) {
        let wanted = self.api.as_ref().filter(|_| self.server_down()).map(|api| (api.identity(), api.is_loopback()));
        let open = self.device.status.down.as_ref().map(|down| down.key.clone());
        match (wanted, open) {
            (None, None) => {}
            (None, Some(_)) => self.device.status.down = None,
            (Some((key, _)), Some(open)) if open == key => {}
            (Some((key, loopback)), _) => {
                let port = url::Url::parse(&key).ok().and_then(|url| url.port_or_known_default()).unwrap_or(DEFAULT_PORT);
                let local = loopback.then(|| Local { port, probe: None, tried: false, busy: false, error: None, generation: 0, task: None });
                self.device.status.down = Some(Down { key, local, tick: None });
                if loopback { self.local_run(None); }
            }
        }
    }

    fn local_mut(&mut self) -> Option<&mut Local> { self.device.status.down.as_mut().and_then(|down| down.local.as_mut()) }

    fn local_run(&mut self, action: Option<Action>) {
        let (done, runtime) = (self.device_send_later(), self.runtime.clone());
        let home = std::env::home_dir();
        let Some(local) = self.local_mut() else { return };
        if let Some(task) = local.task.take() { task.abort(); }
        local.generation = super::next_generation();
        if action.is_some() { (local.busy, local.tried, local.error) = (true, true, None); }
        let Some(home) = home else {
            local.busy = false;
            local.error = Some(action_error_text("local_server_io", "HOME"));
            return;
        };
        let (generation, port) = (local.generation, local.port);
        local.task = Some(runtime.spawn(local_task(home, port, action, generation, done)));
    }

    pub(super) fn receive_local(&mut self, generation: u64, reply: LocalReply, cx: &mut Context<Self>) {
        let Some(local) = self.local_mut() else { return };
        if local.generation != generation { return; }
        let mut back = false;
        match reply {
            LocalReply::Done(error) => {
                // Falhou: nada mais vai tirar o painel da subida (ação recusada ou leitura que não voltou).
                if error.is_some() { local.busy = false; }
                local.error = error.map(|(code, detail)| action_error_text(&code, &detail));
            }
            LocalReply::Probe(probe) => {
                back = probe.state == LocalState::Running && local.probe.as_ref().is_none_or(|p| p.state != LocalState::Running);
                local.probe = Some(*probe);
                local.busy = false;
            }
        }
        // De pé: a página normal volta sem esperar a espera crescente da conexão do retrato.
        if back && self.settings == Some(Page::Status) { self.status_retry(cx); }
        cx.notify();
    }

    /// A cor e o texto do ponto da navegação para o servidor desta máquina; `None` de pé. Antes da primeira leitura, parado.
    pub(super) fn local_dot(&self) -> Option<Option<(Hsla, String)>> {
        let local = self.device.status.down.as_ref()?.local.as_ref()?;
        if local.busy { return Some(Some((theme::warning(), tr("status_nav_starting")))); }
        Some(match local.probe.as_ref().map(|p| &p.state) {
            Some(LocalState::Running) => None,
            Some(LocalState::Starting) => Some((theme::warning(), tr("status_nav_starting"))),
            _ => Some((theme::danger(), tr("status_nav_down"))),
        })
    }

    fn local_start(&mut self, cx: &mut Context<Self>) { self.local_action(Action::Start, cx); }

    fn local_action(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.local_mut().is_none_or(|local| local.busy) { return; }
        self.local_run(Some(action));
        cx.notify();
    }

    /// `ok`: o rótulo do botão da linha, repetido no confirmar.
    fn local_fix(&mut self, fix: Fix, ok: String, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        match fix {
            Fix::Reinstall => self.open_setup_from_menu(window, cx),
            Fix::Listeners => self.show_listeners(window, cx),
            Fix::FreePort(holder) => {
                let (name, pid) = (holder_name(&holder), holder.pid.to_string());
                let params = [("name", name.as_str()), ("pid", pid.as_str())];
                chrome::confirm_alert(window, cx, fill(tr("status_down_free_confirm_title"), &params), fill(tr("status_down_free_confirm_body"), &params),
                    ok, ButtonVariant::Danger, move |_, cx| {
                        let _ = this.update(cx, |this, cx| this.local_action(Action::FreePort(holder.clone()), cx));
                        true
                    });
            }
            Fix::UseVersion(version) => {
                let current = self.device.status.down.as_ref().and_then(|d| d.local.as_ref()).and_then(|l| l.probe.as_ref())
                    .and_then(|p| p.versions.current.clone());
                let body = match &current {
                    Some(current) => fill(tr("status_down_version_confirm_body"), &[("version", &version), ("current", current)]),
                    None => fill(tr("status_down_version_confirm_body_new"), &[("version", &version)]),
                };
                chrome::confirm_alert(window, cx, fill(tr("status_down_version_confirm_title"), &[("version", &version)]), body,
                    ok, ButtonVariant::Danger, move |_, cx| {
                        let _ = this.update(cx, |this, cx| this.local_action(Action::UseVersion(version.clone()), cx));
                        true
                    });
            }
        }
    }

    /// "Ver processos…": quem escuta na porta agora. O diálogo abre lendo; a leitura roda fora da thread da janela.
    fn show_listeners(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt;
        let Some(port) = self.local_mut().map(|local| local.port) else { return };
        let read: std::rc::Rc<std::cell::RefCell<Option<Listeners>>> = Default::default();
        match std::env::home_dir() {
            Some(home) => {
                let (done, result) = tokio::sync::oneshot::channel();
                self.runtime.spawn_blocking(move || { let _ = done.send(LocalServer::new(&home, port).listeners()); });
                let read = read.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let listeners = result.await.unwrap_or_else(|e| Listeners { error: Some(e.to_string()), ..Listeners::default() });
                    *read.borrow_mut() = Some(listeners);
                    let _ = this.update_in(cx, |_, window, _| window.refresh());
                }).detach();
            }
            None => *read.borrow_mut() = Some(Listeners { error: Some("HOME".to_owned()), ..Listeners::default() }),
        }
        let title = fill(tr("status_down_listeners_title"), &[("port", &port.to_string())]);
        window.open_dialog(cx, move |dialog, _, _| super::super::popup::dialog(dialog).w(px(440.)).title(title.clone())
            .child(listeners_body(port, read.borrow().as_ref()))
            .footer(div().flex().justify_end().child(Button::new("status-listeners-close").label(tr("close"))
                .on_click(|_, window, cx| window.close_dialog(cx)))));
    }

    // -----------------------------------------------------------------------------------------
    // Desenho

    /// O corpo da página com o servidor fora; `None` deixa a página normal (de pé, ou sem queda).
    pub(super) fn status_down_body(&mut self, cx: &mut Context<Self>) -> Option<Vec<AnyElement>> {
        self.status_down_sync();
        let down = self.device.status.down.as_mut()?;
        if down.tick.is_none() {
            down.tick = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(Duration::from_secs(30)).await;
                let Ok(()) = this.update(cx, |this, cx| if this.settings == Some(Page::Status) { cx.notify(); }) else { return };
            }));
        }
        let down = self.device.status.down.as_ref()?;
        let Some(local) = &down.local else { return Some(vec![self.remote_down(cx)]) };
        let Some(probe) = &local.probe else {
            // A primeira leitura não voltou: o erro e o ler de novo, nunca o "Lendo" parado.
            return Some(vec![match local.error.clone() { Some(error) => self.read_failed(error, cx), None => checking() }]);
        };
        let attempt = if local.busy { Attempt::Running } else if local.tried { Attempt::Done } else { Attempt::None };
        let view = down_model(probe, Os::current(), attempt)?;
        let (error, busy) = (local.error.clone(), local.busy);
        Some(self.render_down(view, error, busy, cx))
    }

    /// Outra máquina: daqui não se sobe nada.
    fn remote_down(&self, cx: &mut Context<Self>) -> AnyElement {
        let color = theme::danger();
        let server = self.server_label(cx);
        let body = match self.list_down_since {
            Some(since) => fill(tr("status_down_remote_body"), &[("silent", &silent_for(since.elapsed()))]),
            None => tr_shared("status_down_body", &[]),
        };
        settings_box().flex_row().items_start().gap(px(16.)).p(px(16.))
            .child(big_icon(chrome::small_icon(IconName::Close, 24., color).into_any_element(), color))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal()
                    .child(tr_shared("status_down_title", &[("server", &server)])))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(body))
                .child(div().mt(px(9.)).child(Button::new("status-down-remote-retry").outline().small().icon(IconName::RotateCw).label(tr("status_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.status_retry(cx))))))
            .into_any_element()
    }

    /// A leitura do servidor desta máquina não voltou.
    fn read_failed(&self, error: String, cx: &mut Context<Self>) -> AnyElement {
        let color = theme::danger();
        settings_box().flex_row().items_start().gap(px(16.)).p(px(16.)).border_color(color.opacity(0.45))
            .child(big_icon(chrome::small_icon(IconName::TriangleAlert, 24., color).into_any_element(), color))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(tr("status_down_read_failed_title")))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(error))
                .child(div().mt(px(9.)).child(Button::new("status-down-read-retry").outline().small().icon(IconName::RotateCw).label(tr("status_retry"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(local) = this.local_mut() { (local.error, local.probe) = (None, None); }
                        this.local_run(None);
                        cx.notify();
                    })))))
            .into_any_element()
    }

    /// "Não responde" e, com o instante da queda, há quanto tempo.
    fn silent(&self) -> String {
        self.list_down_since.map_or_else(|| tr("status_down_silent"), |since| silent_for(since.elapsed()))
    }

    fn render_down(&self, view: DownView, error: Option<String>, busy: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (icon, color) = match view.phase {
            Phase::Stopped => (chrome::small_icon(IconName::Close, 24., theme::danger()).into_any_element(), theme::danger()),
            Phase::Starting => (spinner("status-down-spin", 22., theme::warning(), view.title.clone()).into_any_element(), theme::warning()),
            Phase::Failed => (chrome::small_icon(IconName::TriangleAlert, 24., theme::danger()).into_any_element(), theme::danger()),
        };
        let sub = match view.phase {
            Phase::Stopped => Some(match &view.detail { Some(reason) => format!("{} · {reason}", self.silent()), None => self.silent() }),
            _ => view.detail.clone(),
        };
        let steps = (!view.steps.is_empty()).then(|| div().id("status-down-steps").role(Role::Status).mt(px(12.)).flex().flex_col().gap(px(8.))
            .text_size(px(12.5)).children(view.steps.iter().enumerate().map(|(i, (step, label))| {
                let mark = match step {
                    Step::Done => chrome::small_icon(IconName::Check, 13., theme::success()).into_any_element(),
                    Step::Running => spinner(&format!("status-down-step-{i}"), 13., theme::muted(), tr("status_down_step_running")).into_any_element(),
                    Step::Waiting => div().size(px(13.)).flex().items_center().justify_center()
                        .child(div().size(px(7.)).rounded_full().border_1().border_color(theme::border_strong())).into_any_element(),
                };
                div().flex().items_center().gap(px(8.)).text_color(if *step == Step::Done { theme::text() } else { theme::muted() })
                    .child(mark).child(label.clone())
            })));
        let error = error.map(|text| div().mt(px(10.)).flex().items_start().gap(px(6.)).text_size(px(12.5)).text_color(theme::danger())
            .child(div().pt(px(2.)).child(chrome::small_icon(IconName::TriangleAlert, 13., theme::danger())))
            .child(div().flex_1().min_w_0().whitespace_normal().child(text)));
        let button = match view.phase {
            Phase::Stopped => Some(Button::new("status-down-start").primary().small().icon(IconName::Play).label(tr("status_down_start"))
                .disabled(busy || !view.can_start).on_click(cx.listener(|this, _, _, cx| this.local_start(cx)))),
            Phase::Failed if view.can_start => Some(Button::new("status-down-retry").outline().small().icon(IconName::RotateCw)
                .label(tr("status_down_retry")).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.local_start(cx)))),
            _ => None,
        };
        let head = settings_box().flex_row().items_start().gap(px(16.)).p(px(16.))
            .when(view.phase == Phase::Failed, |el| el.border_color(theme::danger().opacity(0.45)))
            .child(big_icon(icon, color))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(view.title.clone()))
                .children(sub.map(|text| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text)))
                .children(steps)
                .children(error)
                .children(button.map(|button| div().mt(px(12.)).child(button))));
        let mut out = vec![head.into_any_element()];
        if !view.fixes.is_empty() || !view.output.is_empty() { out.push(self.render_fixes(&view, busy, cx)); }
        out
    }

    fn render_fixes(&self, view: &DownView, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let rows = view.fixes.iter().enumerate().map(|(i, row)| {
            let (fix, ok) = (row.fix.clone(), row.button.clone());
            // Largura mínima, não fixa: o rótulo em inglês ("See processes…") não corta.
            let button = Button::new(SharedString::from(format!("status-down-fix-{i}"))).small().min_w(px(122.)).label(row.button.clone()).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| this.local_fix(fix.clone(), ok.clone(), window, cx)));
            let button = match (&row.fix, row.primary) {
                // O contorno vermelho do mock (`.btn.dng`): encerra um processo, não é o caminho de sempre.
                (Fix::FreePort(_), _) => button.custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::danger())
                    .hover(theme::hover()).active(theme::hover())).border_1().border_color(theme::danger().opacity(0.45)),
                (_, true) => button.primary(),
                (_, false) => button.outline(),
            };
            div().flex().items_center().gap(px(12.)).when(i > 0, |el| el.py(px(11.)).border_t_1().border_color(theme::border()))
                .when(i == 0, |el| el.pb(px(11.)))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).whitespace_normal().child(row.title.clone()))
                    .child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(row.detail.clone())))
                .child(div().flex_shrink_0().child(button))
        });
        let output = (!view.output.is_empty()).then(|| {
            let text = view.output.join("\n");
            div().mt(px(10.)).flex().flex_col().gap(px(6.))
                .child(div().flex().items_center().gap(px(8.))
                    .child(div().text_size(px(11.5)).text_color(theme::faint()).child(tr("status_down_output")))
                    .child(div().ml_auto().child(Button::new("status-down-copy").ghost().small().icon(IconName::Copy).label(tr("status_down_copy"))
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))))))
                .child(div().px(px(12.)).py(px(10.)).rounded(px(8.)).bg(theme::inset()).font_family(theme::MONO).text_size(px(11.5))
                    .line_height(px(17.)).text_color(theme::muted()).whitespace_normal()
                    .children(view.output.iter().map(|line| div().child(line.clone()))))
        });
        settings_box().px(px(16.)).py(px(14.))
            .when(!view.fixes.is_empty(), |el| el
                .child(div().mb(px(10.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr("status_down_fix_title")))
                .children(rows))
            .children(output)
            .into_any_element()
    }
}

fn big_icon(icon: AnyElement, color: Hsla) -> Div {
    div().size(px(52.)).flex_shrink_0().rounded_full().bg(color.opacity(0.18)).flex().items_center().justify_center().child(icon)
}

/// O `Spinner` com nome para o leitor de tela: sozinho é só um ícone girando.
fn spinner(id: &str, size: f32, color: Hsla, label: String) -> Stateful<Div> {
    div().id(SharedString::from(format!("{id}-label"))).role(Role::ProgressIndicator).aria_label(label).flex_shrink_0()
        .child(chrome::Spinner::new(SharedString::from(id.to_owned()), IconName::LoaderCircle, px(size), color))
}

/// Antes da primeira leitura do serviço.
fn checking() -> AnyElement {
    settings_box().flex_row().items_center().gap(px(12.)).p(px(16.))
        .child(spinner("status-down-checking", 16., theme::muted(), tr("status_down_checking")))
        .child(div().text_size(px(13.)).text_color(theme::muted()).child(tr("status_down_checking")))
        .into_any_element()
}

/// O corpo do diálogo "Ver processos…"; `None` enquanto a leitura roda.
fn listeners_body(port: u16, read: Option<&Listeners>) -> AnyElement {
    let Some(listeners) = read else {
        return div().id("status-listeners-loading").role(Role::Status).flex().items_center().gap(px(10.)).text_size(px(13.)).text_color(theme::muted())
            .child(spinner("status-listeners-spin", 14., theme::muted(), tr("status_down_listeners_loading")))
            .child(tr("status_down_listeners_loading")).into_any_element();
    };
    let view = listeners_model(listeners, port);
    let rows = (!view.rows.is_empty()).then(|| div().id("status-listeners").role(Role::List).flex().flex_col()
        .children(view.rows.into_iter().enumerate().map(|(i, [name, pid, user])| div().id(SharedString::from(format!("status-listener-{i}")))
            .role(Role::ListItem).aria_label(format!("{name}, {pid}, {user}")).flex().items_center().gap(px(12.)).py(px(7.))
            .when(i > 0, |el| el.border_t_1().border_color(theme::border()))
            .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).child(name))
            .child(div().flex_shrink_0().text_color(theme::muted()).child(pid))
            .child(div().flex_shrink_0().text_color(theme::muted()).child(user)))));
    div().flex().flex_col().gap(px(10.)).text_size(px(13.))
        .children(rows)
        .children(view.notes.into_iter().map(|note| div().text_color(theme::muted()).whitespace_normal().child(note)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(state: LocalState) -> Probe {
        Probe { state, versions: Versions { current: Some("0.1.0.10824".into()), previous: Some("0.1.0.10790".into()),
            installed: vec!["0.1.0.10824".into(), "0.1.0.10790".into()], dev_mode: false }, port: 8765, port_open: false, output: Vec::new() }
    }

    fn stopped_with(code: &str, detail: &str, holder: Option<PortHolder>) -> Probe {
        probe(LocalState::Stopped { failure: Some(LocalFailure { code: code.into(), detail: detail.into(), port_holder: holder,
            output: vec!["linha 1".into(), "linha 2".into()] }) })
    }

    fn holder(same_user: bool) -> PortHolder { PortHolder { pid: 48211, name: "python3".into(), same_user, start_time: 1_700_000_000 } }

    fn fixes(view: &DownView) -> Vec<Fix> { view.fixes.iter().map(|row| row.fix.clone()).collect() }

    fn steps(view: &DownView) -> Vec<Step> { view.steps.iter().map(|(step, _)| *step).collect() }

    #[test]
    fn running_leaves_the_normal_page() {
        assert_eq!(down_model(&probe(LocalState::Running), Os::Linux, Attempt::Done), None);
    }

    #[test]
    fn clean_stop_offers_start_and_after_a_try_shows_the_output() {
        crate::i18n::english_in_this_thread(false);
        let view = down_model(&probe(LocalState::Stopped { failure: None }), Os::Linux, Attempt::None).unwrap();
        assert_eq!((view.phase, view.detail.as_deref(), view.can_start), (Phase::Stopped, None, true));
        assert_eq!(view.title, tr("status_down_stopped_title"));
        let mut tried = probe(LocalState::Stopped { failure: None });
        tried.output = vec!["saiu".into()];
        let view = down_model(&tried, Os::Linux, Attempt::Done).unwrap();
        assert_eq!((view.phase, view.output.clone()), (Phase::Failed, vec!["saiu".to_owned()]));
        assert!(view.fixes.is_empty() && view.can_start);
    }

    #[test]
    fn starting_steps_follow_the_port_and_the_running_action() {
        let view = down_model(&probe(LocalState::Starting), Os::Linux, Attempt::Done).unwrap();
        assert_eq!((view.phase, steps(&view)), (Phase::Starting, vec![Step::Done, Step::Running, Step::Waiting]));
        let mut open = probe(LocalState::Starting);
        open.port_open = true;
        assert_eq!(steps(&down_model(&open, Os::Linux, Attempt::None).unwrap()), [Step::Done, Step::Done, Step::Running]);
        // A ação ainda roda: a leitura antiga (uma falha) não volta à tela.
        let view = down_model(&stopped_with("port_unavailable", "python3", Some(holder(true))), Os::Linux, Attempt::Running).unwrap();
        assert_eq!((view.phase, steps(&view)), (Phase::Starting, vec![Step::Running, Step::Waiting, Step::Waiting]));
        assert!(view.steps[1].1.contains("8765"));
    }

    #[test]
    fn missing_service_only_offers_the_installer_where_it_exists() {
        crate::i18n::english_in_this_thread(false);
        let view = down_model(&probe(LocalState::NotInstalled), Os::Linux, Attempt::None).unwrap();
        assert_eq!((view.phase, view.can_start, fixes(&view)), (Phase::Failed, false, vec![Fix::Reinstall]));
        assert!(view.detail.unwrap().contains("hangar-backend.service"));
        assert!(down_model(&probe(LocalState::NotInstalled), Os::Windows, Attempt::None).unwrap().detail.unwrap().contains("hangar-backend"));
        assert!(fixes(&down_model(&probe(LocalState::NotInstalled), Os::Macos, Attempt::None).unwrap()).is_empty());
    }

    #[test]
    fn busy_port_frees_only_a_process_of_this_user() {
        crate::i18n::english_in_this_thread(false);
        let view = down_model(&stopped_with("port_unavailable", "python3", Some(holder(true))), Os::Linux, Attempt::None).unwrap();
        assert_eq!((view.phase, fixes(&view)), (Phase::Failed, vec![Fix::FreePort(holder(true)), Fix::Listeners]));
        assert!(view.title.contains("8765"));
        assert!(view.detail.as_deref().unwrap().contains("python3") && view.detail.as_deref().unwrap().contains("48211"));
        assert!(view.fixes[0].title.contains("python3"));
        assert!(view.fixes[1].detail.contains("8765") && !view.fixes[1].primary);
        assert_eq!(view.output, ["linha 1", "linha 2"]);
        // De outro usuário, ou sem dono visível: só ver quem escuta, nunca encerrar.
        let other = down_model(&stopped_with("port_unavailable", "python3", Some(holder(false))), Os::Linux, Attempt::None).unwrap();
        assert_eq!(fixes(&other), [Fix::Listeners]);
        let hidden = down_model(&stopped_with("port_unavailable", "", None), Os::Linux, Attempt::None).unwrap();
        assert_eq!((hidden.phase, fixes(&hidden)), (Phase::Failed, vec![Fix::Listeners]));
        assert_eq!(hidden.detail.as_deref(), Some(tr("status_down_port_hidden").as_str()));
    }

    #[test]
    fn listeners_dialog_names_the_user_and_explains_other_users() {
        crate::i18n::english_in_this_thread(false);
        let mine = PortHolder { pid: 1, name: String::new(), same_user: true, start_time: 0 };
        let view = listeners_model(&Listeners { holders: vec![holder(false), mine], hidden: true, error: None }, 8765);
        assert_eq!(view.rows, [
            ["python3".to_owned(), "pid 48211".to_owned(), tr("status_down_listeners_other_user")],
            [tr("status_down_unknown_process"), "pid 1".to_owned(), tr("status_down_listeners_you")],
        ]);
        assert_eq!(view.notes, [tr("status_down_listeners_hidden"), tr("status_down_listeners_other_note")]);
        let only_mine = listeners_model(&Listeners { holders: vec![holder(true)], ..Listeners::default() }, 8765);
        assert!(only_mine.notes.is_empty());
        let none = listeners_model(&Listeners::default(), 8765);
        assert!(none.rows.is_empty() && none.notes.len() == 1 && none.notes[0].contains("8765"));
        // A consulta falhou: o erro, nunca "ninguém escuta".
        let failed = listeners_model(&Listeners { error: Some("ss: not found".into()), ..Listeners::default() }, 8765);
        assert_eq!(failed.notes.len(), 1);
        assert!(failed.notes[0].contains("ss: not found"));
    }

    #[test]
    fn silent_time_counts_minutes_then_hours() {
        crate::i18n::english_in_this_thread(true);
        assert_eq!(silent_for(Duration::from_secs(59)), tr("status_down_silent_moment"));
        assert_eq!(silent_for(Duration::from_secs(120)), "Not answering for 2 min");
        assert_eq!(silent_for(Duration::from_secs(59 * 60 + 59)), "Not answering for 59 min");
        assert_eq!(silent_for(Duration::from_secs(2 * 3600 + 600)), "Not answering for 2 h");
        crate::i18n::english_in_this_thread(false);
        assert_eq!(silent_for(Duration::from_secs(120)), "Não responde há 2 min");
    }

    #[test]
    fn a_remote_server_is_down_only_when_the_status_stream_also_failed() {
        // Lista de pé ou sem erro: nunca parado.
        assert!(!is_down(true, true, false, None));
        assert!(!is_down(false, false, true, None));
        // Outra máquina com a página aberta: o 503 da lista sozinho não basta, o retrato ainda chega.
        assert!(!is_down(false, true, false, Some(false)));
        assert!(is_down(false, true, false, Some(true)));
        // Página fechada (o ponto da navegação): vale a lista.
        assert!(is_down(false, true, false, None));
        // Esta máquina: quem decide é a leitura do serviço.
        assert!(is_down(false, true, true, Some(false)));
    }

    #[test]
    fn missing_server_points_back_to_the_previous_version() {
        crate::i18n::english_in_this_thread(false);
        let path = "/home/x/.hangar/versions/0.1.0.10824/bin/hangar-server";
        let view = down_model(&stopped_with("launch_no_server", path, None), Os::Linux, Attempt::None).unwrap();
        assert_eq!(fixes(&view), [Fix::UseVersion("0.1.0.10790".into()), Fix::Reinstall]);
        assert!(view.fixes[0].primary && !view.fixes[1].primary);
        assert!(view.detail.as_deref().unwrap().contains(path) && view.detail.as_deref().unwrap().contains("0.1.0.10790"));
        let python = down_model(&stopped_with("launch_no_python", "/x/python", None), Os::Windows, Attempt::None).unwrap();
        assert_eq!(fixes(&python), [Fix::UseVersion("0.1.0.10790".into()), Fix::Reinstall]);
        assert_eq!(python.title, tr("status_down_no_python_title"));
        // macOS não roda pacote; modo desenvolvedor não tem versões.
        assert!(fixes(&down_model(&stopped_with("launch_no_server", path, None), Os::Macos, Attempt::None).unwrap()).is_empty());
        let mut dev = stopped_with("launch_no_server", path, None);
        dev.versions.dev_mode = true;
        let view = down_model(&dev, Os::Linux, Attempt::None).unwrap();
        assert_eq!(fixes(&view), [Fix::Reinstall]);
        assert!(view.fixes[0].primary);
    }

    #[test]
    fn broken_current_offers_the_newest_installed() {
        crate::i18n::english_in_this_thread(false);
        let mut broken = stopped_with("launch_no_current", "/home/x/.hangar/current", None);
        broken.versions.current = None;
        broken.versions.previous = Some("0.1.0.10824".into());
        let view = down_model(&broken, Os::Linux, Attempt::None).unwrap();
        assert_eq!(view.title, tr("status_down_current_title"));
        assert_eq!(fixes(&view), [Fix::UseVersion("0.1.0.10824".into()), Fix::Reinstall]);
        assert_eq!(view.fixes[0].detail, tr("status_down_fix_point_newest"));
        broken.versions.previous = None;
        assert_eq!(fixes(&down_model(&broken, Os::Linux, Attempt::None).unwrap()), [Fix::Reinstall]);
    }

    #[test]
    fn failures_without_a_fix_are_a_reason_until_someone_tries() {
        crate::i18n::english_in_this_thread(false);
        for (code, detail) in [("service_exit", "3"), ("service_failed", "failed/failed"), ("service_unreadable", "timeout"),
            ("server_exited", "101"), ("server_failed", "panic"), ("identity_conflict", "")] {
            let stopped = down_model(&stopped_with(code, detail, None), Os::Linux, Attempt::None).unwrap();
            assert_eq!(stopped.phase, Phase::Stopped, "{code}");
            assert!(stopped.can_start && stopped.fixes.is_empty() && stopped.output.is_empty(), "{code}");
            let reason = stopped.detail.clone().unwrap();
            assert!(reason.contains(detail), "{code}: {reason}");
            let failed = down_model(&stopped_with(code, detail, None), Os::Linux, Attempt::Done).unwrap();
            assert_eq!(failed.phase, Phase::Failed, "{code}");
            assert!(failed.title.contains(&reason), "{code}: {}", failed.title);
            assert_eq!(failed.output, ["linha 1", "linha 2"], "{code}");
        }
        let exit = down_model(&stopped_with("service_exit", "3", None), Os::Linux, Attempt::None).unwrap();
        assert!(exit.detail.unwrap().contains("hangar-backend.service"));
        let unknown = down_model(&stopped_with("identity_conflict", "", None), Os::Linux, Attempt::None).unwrap();
        assert!(unknown.detail.unwrap().contains("identity_conflict"));
    }

    #[test]
    fn every_action_error_code_has_a_sentence_in_both_languages() {
        let dicts = [include_str!("../../../../messages/pt.json"), include_str!("../../../../messages/en.json")]
            .map(|s| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(s).unwrap());
        let codes = ["local_server_unsupported", "local_server_dev_mode", "local_server_not_installed", "local_server_service_unreadable",
            "update_locked", "local_server_running", "local_server_unknown_version", "local_server_no_binary", "local_server_holder_changed",
            "local_server_other_user", "local_server_terminate_refused", "local_server_port_still_held", "local_server_current_changed",
            "local_server_version_mismatch", "local_server_command_failed", "local_server_io"];
        for code in codes {
            assert!(dicts.iter().all(|d| d.contains_key(&format!("native_status_down_error_{code}"))), "{code}");
        }
        crate::i18n::english_in_this_thread(false);
        assert!(action_error_text("local_server_no_binary", "0.1.0.1").contains("0.1.0.1"));
        let unknown = action_error_text("brand_new", "x");
        assert!(unknown.contains("brand_new") && unknown.contains('x'));
    }
}
