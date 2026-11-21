//! Assistente do CLIProxyAPI: instalar (ou aproveitar o proxy que já existe), conectar a conta ChatGPT ou Claude pelo
//! login do proxy e criar o modelo do Claude Code. Cada resposta traz o número da vez: a de outra vez não mexe nesta.
use super::*;
use super::keys::{engine_id, field, help, provider_badge, tone};
use crate::app::settings::Disclosure;
use gpui_kit::component::{IndexPath, radio::Radio, select::{Select, SelectEvent, SelectState}, searchable_list::SearchableVec,
    stepper::{Stepper, StepperItem}, switch::Switch};

mod relay;

/// O endereço do catálogo: só dá a cor e as letras do selo.
const BADGE_URL: &str = "http://127.0.0.1:8317";
/// As quatro etapas do instalador, na ordem do `step` do servidor.
const INSTALL_STEPS: [(&str, &str); 4] = [("download", "cliproxy_wizard_item_download"), ("config", "cliproxy_wizard_item_config"),
    ("service", "cliproxy_wizard_item_service"), ("start", "cliproxy_wizard_item_start")];
const STEPS: [&str; 3] = ["cliproxy_wizard_step_install", "cliproxy_wizard_step_connect", "cliproxy_wizard_step_name"];

type Picker = Entity<SelectState<SearchableVec<ModelChoice>>>;

#[derive(Clone, Copy, PartialEq)]
enum WizardStep { Install, Connect, Name }

/// A assinatura que entra pelo proxy.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Brand { ChatGpt, Claude }

impl Brand {
    /// O `provider` do login e o `type` da conta no proxy.
    fn provider(self) -> &'static str { match self { Self::ChatGpt => "codex", Self::Claude => "claude" } }
    fn name(self) -> &'static str { match self { Self::ChatGpt => "ChatGPT", Self::Claude => "Claude" } }
    fn prefix(self) -> &'static str { match self { Self::ChatGpt => "GPT", Self::Claude => "Claude" } }
    fn of(kind: &str) -> Option<Self> { match kind { "codex" => Some(Self::ChatGpt), "claude" => Some(Self::Claude), _ => None } }
}

/// Para que serve a leitura do `GET /api/engines/cliproxy`.
#[derive(Clone, Copy)]
pub(in crate::app) enum Purpose { Entry, AfterLogin, Models }

#[derive(Clone, Default, Deserialize)]
struct JobError { code: String }

#[derive(Clone, Deserialize)]
struct Job { #[serde(default)] kind: String, step: String, done: bool, error: Option<JobError> }

#[derive(Clone, Default, Deserialize)]
struct InstallState {
    #[serde(default)] state: String,
    #[serde(default)] running: bool,
    version: Option<String>,
    latest: Option<String>,
    handover: Option<super::proxy_update::Handover>,
    sessions: Option<usize>,
    job: Option<Job>,
}

/// O que o passo Instalar pede ao servidor.
#[derive(Clone, Copy)]
enum Start { Follow, Install, Adopt { confirm: bool } }

#[derive(Clone, Deserialize)]
struct Connected { #[serde(rename = "type")] kind: String, #[serde(default)] email: String, binding: Option<String>, prefix: Option<String> }

#[derive(Clone, Deserialize)]
struct ProxyModel { id: String, context_length: Option<u64> }

#[derive(Clone, Deserialize)]
struct ProxyStatus {
    #[serde(default)] found: bool,
    base_url: Option<String>,
    #[serde(default)] models: Vec<ProxyModel>,
    error: Option<String>,
    #[serde(default)] connected: Vec<Connected>,
    install: Option<InstallState>,
}

impl ProxyStatus {
    /// O proxy responde e lista modelos: o motor pode nascer dele.
    fn usable(&self) -> bool { self.found && self.base_url.is_some() && self.error.is_none() }
}

#[derive(Clone, PartialEq)]
enum LoginPhase {
    Pick,
    Opening(Brand),
    Waiting { brand: Brand, url: String, state: String },
    /// O proxy disse "ok": falta reler a lista para saber qual conta entrou.
    Resolving(Brand),
    Connected { brand: Brand, email: String },
}

pub(in crate::app) enum CliproxyReply {
    Read(u64, Purpose, Result<Value, Failure>),
    InstallFailed(u64, Failure),
    Progress(u64, Result<Value, Failure>),
    LoginStarted(u64, Brand, Result<Value, Failure>),
    LoginStatus(u64, Result<Value, Failure>),
    /// A volta do login entregue ao proxy, colada ou pelo `relay`; vale só para o login de mesmo `state`.
    Callback(String, Result<Value, Failure>),
    KeySaved(u64, Brand, Result<Value, Failure>),
    /// A senha antiga ficou só como hash no proxy: o Hangar gerou outra.
    KeyReset(u64, Brand, Result<Value, Failure>),
    /// Nome e id do motor gravado.
    Saved(u64, String, String, Result<Value, Failure>),
}

/// O assistente aberto no lugar da lista.
pub(super) struct CliproxyWizard {
    step: WizardStep,
    turn: u64,
    task: Option<JoinHandle<()>>,
    /// Última leitura do proxy; `None` = a primeira ainda não chegou.
    status: Option<Result<ProxyStatus, String>>,
    reading: bool,
    installing: bool,
    /// Proxy de fora: passar para o Hangar (o marcado de início) ou usar como está.
    adopt: bool,
    job: Option<Job>,
    install_error: Option<String>,
    login: LoginPhase,
    login_error: Option<String>,
    polling: bool,
    /// Contas do proxy antes do login: a que aparecer de novo é a que entrou.
    before: Vec<Connected>,
    login_account: Option<Connected>,
    selected_binding: Option<String>,
    other_open: bool,
    relay: Option<relay::Relay>,
    callback: Entity<InputState>,
    sending: bool,
    callback_error: Option<String>,
    /// Falta a senha de gerenciamento (ou a guardada foi recusada) para abrir o login desta conta.
    key: Option<(Brand, bool)>,
    mgmt: Entity<InputState>,
    key_saving: bool,
    name: Option<Entity<InputState>>,
    picker: Option<Picker>,
    models: Option<Vec<ProxyModel>>,
    models_error: Option<String>,
    model: String,
    fast: bool,
    name_error: Option<String>,
    saving: bool,
    save_error: Option<String>,
    /// Nome e id do motor criado: o passo final.
    saved: Option<(String, String)>,
    _subscriptions: Vec<Subscription>,
}

impl Drop for CliproxyWizard {
    // Tarefa do tokio não morre com o handle: sem isto a leitura seguiria depois do assistente fechado.
    fn drop(&mut self) { if let Some(task) = self.task.take() { task.abort(); } }
}

/// A mesma regra do servidor (`allows_fast` em `engines/catalog.rs`): `gpt-` seguido de dígito, sem o prefixo do provedor.
fn allows_fast(model: &str) -> bool {
    model.rsplit('/').next().unwrap_or("").strip_prefix("gpt-").is_some_and(|s| s.starts_with(|c: char| c.is_ascii_digit()))
}

pub(crate) fn default_name(brand: Brand, email: &str) -> String {
    match email.split('@').next().map(str::trim).filter(|local| !local.is_empty()) {
        Some(local) => format!("{} {local}", brand.prefix()),
        None => brand.prefix().to_owned(),
    }
}

/// Sem janela própria o Claude Code assume 1M; com o limite real do modelo, compacta antes de o modelo recusar.
/// A margem de 5% é a mesma que o Codex usa (`effective_context_window_percent`).
fn safe_context(limit: u64) -> u64 { limit / 100 * 95 }

/// A conta que entrou: a do tipo pedido que não estava lá antes; relogar a mesma cai na primeira do tipo.
fn new_email(before: &[Connected], after: &[Connected], kind: &str) -> String {
    let mut of_kind = after.iter().filter(|a| a.kind == kind);
    let fresh = after.iter().filter(|a| a.kind == kind).find(|a| !before.iter().any(|b| b.kind == kind && b.email == a.email));
    fresh.or_else(|| of_kind.next()).map(|a| a.email.clone()).unwrap_or_default()
}

/// O primeiro modelo da assinatura conectada; sem nenhum dela, o primeiro da lista.
fn default_model(models: &[ProxyModel], brand: Brand) -> Option<String> {
    let wanted = |id: &str| match brand { Brand::ChatGpt => allows_fast(id), Brand::Claude => id.to_lowercase().contains("claude") };
    models.iter().find(|m| wanted(&m.id)).or(models.first()).map(|m| m.id.clone())
}

fn step_index(step: &str) -> usize { INSTALL_STEPS.iter().position(|(name, _)| *name == step).unwrap_or(0) }

fn known_code(code: &str) -> Option<String> {
    let key = format!("cliproxy_wizard_err_{code}");
    let text = tr(&key);
    (text != key).then_some(text)
}

pub(super) fn code_text(code: &str) -> String {
    known_code(code).unwrap_or_else(|| tr("cliproxy_wizard_err_unknown").replace("{code}", code))
}

/// Recusa com código conhecido diz o que fazer; o resto é a frase de sempre (sem resposta, login perdido).
fn failure_text(error: &Failure) -> String {
    error.code.as_deref().and_then(known_code).unwrap_or_else(|| Hangar::setting_failure(error))
}

fn parse_status(result: Result<Value, Failure>) -> Result<ProxyStatus, String> {
    result.map_err(|e| Hangar::failure(&e)).and_then(|v| serde_json::from_value::<ProxyStatus>(v).map_err(|_| tr("invalid_response")))
}

fn brand_logo(brand: Brand, size: f32) -> Div {
    let (color, _) = theme::provider(brand.provider());
    div().size(px(size)).flex_shrink_0().rounded(px(size * 0.27)).bg(color.opacity(0.16)).flex().items_center().justify_center()
        .children(chrome::provider_logo(brand.provider(), size * 0.58, color))
}

pub(crate) fn account_row(brand: Brand, email: &str) -> Div {
    let shown = if email.is_empty() { brand.name().to_owned() } else { email.to_owned() };
    div().flex().items_center().gap(px(10.)).px(px(12.)).py(px(10.)).rounded(px(12.)).border_1().border_color(theme::border())
        .child(brand_logo(brand, 26.))
        .child(div().flex_1().min_w_0().truncate().child(shown))
        .child(div().flex_shrink_0().flex().items_center().gap(px(4.)).text_size(px(12.5)).text_color(theme::success())
            .child(chrome::small_icon(IconName::Check, 13., theme::success())).child(tr("cliproxy_wizard_connected_badge")))
}

fn pill(text: String) -> Div {
    div().px(px(9.)).py(px(3.)).rounded(px(7.)).bg(theme::raised()).text_color(theme::text()).font_weight(FontWeight::MEDIUM).child(text)
}

/// A faixa que mostra como o motor aparece na criação de sessão.
fn preview() -> Div {
    div().flex().flex_wrap().items_center().gap(px(8.)).px(px(11.)).py(px(9.)).rounded(px(10.)).border_1().border_color(theme::border())
        .bg(theme::inset()).text_size(px(12.5)).text_color(theme::muted())
}

fn lead(text: String) -> Div { div().text_size(px(14.)).whitespace_normal().child(text) }

fn footer() -> Div { div().mt(px(6.)).flex().items_center().gap(px(8.)) }

fn spacer() -> Div { div().flex_1() }

/// Os três passos do assistente; `current` é o aberto e `finished` marca todos como feitos.
pub(crate) fn wizard_steps(id: impl Into<ElementId>, current: usize, finished: bool) -> Stepper {
    Stepper::new(id).small().selected_index(current).items(STEPS.iter().enumerate().map(|(index, key)| {
        // O feito leva o ✓ além da cor: cor nunca sozinha.
        StepperItem::new().disabled(index > current).when(index < current || finished, |item| item.icon(IconName::Check))
            .child(div().text_size(px(12.5)).text_color(if index <= current { theme::text() } else { theme::muted() }).child(tr(key)))
    }))
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ItemState { Done, Failed, Running, Waiting }

pub(crate) const INSTALL_ITEMS: usize = INSTALL_STEPS.len();

/// Uma das quatro etapas da instalação, na ordem do `step` do servidor.
pub(crate) fn install_item(index: usize, state: ItemState) -> Div {
    step_item(&format!("cliproxy-wizard-item-{index}"), tr(INSTALL_STEPS[index].1), state)
}

/// Uma etapa com o ícone do estado dela: a da instalação e a da atualização.
pub(crate) fn step_item(id: &str, label: String, state: ItemState) -> Div {
    let (icon, color): (AnyElement, Hsla) = match state {
        ItemState::Done => (chrome::small_icon(IconName::CircleCheck, 16., theme::success()).into_any_element(), theme::text()),
        ItemState::Failed => (chrome::small_icon(IconName::CircleX, 16., theme::danger()).into_any_element(), theme::text()),
        ItemState::Running => (chrome::Spinner::new(SharedString::from(id.to_owned()), IconName::LoaderCircle, px(16.), theme::accent())
            .into_any_element(), theme::text()),
        ItemState::Waiting => (chrome::small_icon(IconName::Circle, 16., theme::faint()).into_any_element(), theme::muted()),
    };
    div().flex().items_center().gap(px(10.)).text_size(px(13.5)).text_color(color)
        .child(div().size(px(18.)).flex_shrink_0().flex().items_center().justify_center().child(icon)).child(label)
}

/// Como o motor vai aparecer na criação de sessão.
pub(crate) fn name_preview(name: &str) -> Div {
    preview().child(tr("cliproxy_wizard_name_preview")).child(pill(if name.is_empty() { "…".into() } else { name.to_owned() }))
}

/// A opção Fast do motor, com o interruptor que quem chama monta.
pub(crate) fn fast_field(switch: Switch) -> Div {
    field(tr("cliproxy_wizard_speeds"),
        div().flex().items_center().gap(px(10.)).px(px(11.)).py(px(9.)).rounded(px(10.)).border_1().border_color(theme::border())
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().font_weight(FontWeight::MEDIUM).child(tr("cliproxy_wizard_fast")))
                .child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(tr("cliproxy_wizard_fast_desc"))))
            .child(switch))
        .child(help(tr("cliproxy_wizard_speeds_help")))
}

fn waiting_line(id: &'static str, text: String) -> Div {
    div().flex().items_center().gap(px(10.)).text_size(px(13.)).text_color(theme::muted())
        .child(chrome::Spinner::new(id, IconName::LoaderCircle, px(15.), theme::accent())).child(text)
}

impl Hangar {
    fn wizard_send(&self) -> impl Fn(CliproxyReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let done = self.accounts_send_later();
        move |reply| done(AccountsReply::Cliproxy(reply))
    }

    /// Nova vez: a tarefa da vez anterior para, e só respostas desta vez valem.
    fn wizard_turn(&mut self) -> Option<(u64, Api)> {
        let api = self.api.clone()?;
        self.accounts.keys_seq += 1;
        let turn = self.accounts.keys_seq;
        let wizard = self.accounts.cliproxy.as_mut()?;
        if let Some(task) = wizard.task.take() { task.abort(); }
        (wizard.turn, wizard.polling) = (turn, false);
        Some((turn, api))
    }

    fn wizard(&mut self, turn: u64) -> Option<&mut CliproxyWizard> { self.accounts.cliproxy.as_mut().filter(|w| w.turn == turn) }

    /// A linha do CLIProxyAPI no "Adicionar…" de modelo.
    pub(super) fn open_cliproxy_wizard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() || self.api.is_none() { return; }
        let callback = cx.new(|cx| InputState::new(window, cx).placeholder(tr("cliproxy_wizard_callback_placeholder")));
        let mgmt = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(tr_shared("cliproxy_senha_placeholder", &[])));
        let subscriptions = vec![
            cx.subscribe_in(&callback, window, |this: &mut Hangar, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => { if let Some(w) = this.accounts.cliproxy.as_mut() { w.callback_error = None; } cx.notify(); }
                InputEvent::PressEnter { .. } => this.wizard_callback(cx),
                _ => {}
            }),
            cx.subscribe_in(&mgmt, window, |this: &mut Hangar, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.wizard_save_key(cx),
                _ => {}
            }),
        ];
        self.accounts.outcome = None;
        self.accounts.cliproxy = Some(CliproxyWizard { step: WizardStep::Install, turn: 0, task: None, status: None, reading: false,
            installing: false, adopt: true, job: None, install_error: None, login: LoginPhase::Pick, login_error: None, polling: false, before: Vec::new(), login_account: None, selected_binding: None,
            other_open: false, relay: None, callback, sending: false, callback_error: None, key: None, mgmt, key_saving: false, name: None, picker: None,
            models: None, models_error: None, model: String::new(), fast: true, name_error: None, saving: false, save_error: None, saved: None,
            _subscriptions: subscriptions });
        self.wizard_read(Purpose::Entry, cx);
    }

    fn close_cliproxy_wizard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts.cliproxy.as_ref().is_some_and(|w| w.saving) { return; }
        self.accounts.cliproxy = None;
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    /// Fim do assistente: as Configurações saem da frente e a nova sessão abre já no motor criado.
    fn wizard_open_session(&mut self, engine: String, window: &mut Window, cx: &mut Context<Self>) {
        self.accounts.cliproxy = None;
        self.close_settings(window, cx);
        self.open_new_session_with_engine(engine, window, cx);
    }

    fn wizard_read(&mut self, purpose: Purpose, cx: &mut Context<Self>) {
        let Some((turn, api)) = self.wizard_turn() else { return };
        let done = self.wizard_send();
        let Some(w) = self.accounts.cliproxy.as_mut() else { return };
        w.reading = true;
        match purpose {
            Purpose::Entry => w.status = None,
            Purpose::Models => (w.models, w.models_error, w.picker) = (None, None, None),
            Purpose::AfterLogin => {}
        }
        w.task = Some(self.runtime.spawn(async move {
            done(CliproxyReply::Read(turn, purpose, api.server_read(&["engines", "cliproxy"], &[], 20).await)).await
        }));
        cx.notify();
    }

    /// Instalar, completar o config do proxy de fora ou passá-lo ao Hangar, e acompanhar a cada segundo até o servidor
    /// dizer que acabou. `Start::Follow` só acompanha a tarefa que já estava em andamento.
    fn wizard_install(&mut self, start: Start, cx: &mut Context<Self>) {
        let starting = !matches!(start, Start::Follow);
        if self.accounts.cliproxy.as_ref().is_none_or(|w| w.installing && starting) { return; }
        let Some((turn, api)) = self.wizard_turn() else { return };
        let done = self.wizard_send();
        let Some(w) = self.accounts.cliproxy.as_mut() else { return };
        (w.installing, w.install_error) = (true, None);
        if starting { w.job = None; }
        w.task = Some(self.runtime.spawn(async move {
            let posted = match start {
                Start::Follow => Ok(Value::Null),
                Start::Install => api.server_post(&["engines", "cliproxy", "install"], 30).await,
                Start::Adopt { confirm } =>
                    api.server_send(reqwest::Method::POST, &["engines", "cliproxy", "adopt"], Some(json!({"confirm": confirm})), 30).await,
            };
            // Outra tarefa já em andamento (409) é a mesma que se quer: segue acompanhando ela.
            if let Err(error) = posted && error.code.as_deref() != Some("erro_cliproxy_ocupado") {
                return done(CliproxyReply::InstallFailed(turn, error)).await;
            }
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let read = api.server_read(&["engines", "cliproxy"], &[], 20).await;
                let running = matches!(&read, Ok(v) if v.pointer("/install/job/done").and_then(Value::as_bool) == Some(false));
                done(CliproxyReply::Progress(turn, read)).await;
                if !running { break; }
            }
        }));
        cx.notify();
    }

    fn wizard_login(&mut self, brand: Brand, window: &mut Window, cx: &mut Context<Self>) {
        let Some((turn, api)) = self.wizard_turn() else { return };
        let done = self.wizard_send();
        let Some(w) = self.accounts.cliproxy.as_mut() else { return };
        w.before = w.status.as_ref().and_then(|s| s.as_ref().ok()).map(|s| s.connected.clone()).unwrap_or_default();
        (w.login, w.login_error, w.callback_error, w.other_open, w.sending, w.relay) = (LoginPhase::Opening(brand), None, None, false, false, None);
        w.callback.update(cx, |input, cx| input.set_value("", window, cx));
        let body = json!({"provider": brand.provider()});
        w.task = Some(self.runtime.spawn(async move {
            done(CliproxyReply::LoginStarted(turn, brand, api.server_send(reqwest::Method::POST, &["engines", "cliproxy", "login"], Some(body), 30).await)).await
        }));
        cx.notify();
    }

    /// Lê o login a cada 2 s até o proxy dizer "ok" ou "erro". Sem resposta do servidor segue tentando, com o aviso à vista.
    fn wizard_poll_login(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.accounts.cliproxy.as_ref().and_then(|w| match &w.login {
            LoginPhase::Waiting { state, .. } => Some(state.clone()), _ => None,
        }) else { return };
        let Some((turn, api)) = self.wizard_turn() else { return };
        let done = self.wizard_send();
        let Some(w) = self.accounts.cliproxy.as_mut() else { return };
        w.polling = true;
        w.task = Some(self.runtime.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let read = api.server_read(&["engines", "cliproxy", "login", &state], &[], 15).await;
                let again = match &read {
                    Ok(v) => v.get("status").and_then(Value::as_str) == Some("wait"),
                    Err(error) => error.status.is_none(),
                };
                done(CliproxyReply::LoginStatus(turn, read)).await;
                if !again { break; }
            }
        }));
        cx.notify();
    }

    /// Outro computador: o endereço em que o navegador parou leva o login até o proxy desta máquina.
    fn wizard_callback(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let done = self.wizard_send();
        let Some(w) = self.accounts.cliproxy.as_mut().filter(|w| !w.sending) else { return };
        let LoginPhase::Waiting { state, .. } = &w.login else { return };
        let state = state.clone();
        let address = w.callback.read(cx).value().trim().to_owned();
        if address.is_empty() { return; }
        if !["http://127.0.0.1:", "http://localhost:"].iter().any(|p| address.starts_with(p)) {
            w.callback_error = Some(tr("cliproxy_wizard_callback_invalid"));
            cx.notify();
            return;
        }
        (w.sending, w.callback_error) = (true, None);
        let body = json!({"redirect_url": address});
        self.runtime.spawn(async move {
            let sent = api.server_send(reqwest::Method::POST, &["engines", "cliproxy", "login", &state, "callback"], Some(body), 30).await;
            done(CliproxyReply::Callback(state, sent)).await
        });
        cx.notify();
    }

    /// O navegador desta máquina volta para `localhost`; com o proxy noutra, o app recebe a volta e a entrega ao servidor.
    fn wizard_relay(&mut self, url: &str, state: &str) {
        let Some(api) = self.api.clone() else { return };
        let relay = relay::target(url).filter(|back| back.state == state).and_then(|back| {
            let listener = relay::bind(&back)?;
            let pages = relay::Pages { done: tr("cliproxy_relay_done"), failed: tr("cliproxy_relay_failed") };
            Some(relay::Relay(self.runtime.spawn(relay::serve(listener, back, api, self.wizard_send(), pages))))
        });
        if let Some(w) = self.accounts.cliproxy.as_mut() { w.relay = relay; }
    }

    /// A senha de gerenciamento fica no servidor; com ela guardada, o login pedido é tentado de novo.
    fn wizard_save_key(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let done = self.wizard_send();
        let Some(w) = self.accounts.cliproxy.as_mut().filter(|w| !w.key_saving) else { return };
        let Some((brand, _)) = w.key else { return };
        let key = w.mgmt.read(cx).value().trim().to_owned();
        if key.is_empty() { return; }
        (w.key_saving, w.login_error) = (true, None);
        let turn = w.turn;
        let body = json!({"management_key": key});
        self.runtime.spawn(async move {
            done(CliproxyReply::KeySaved(turn, brand, api.server_send(reqwest::Method::PUT, &["engines", "cliproxy", "management-key"], Some(body), 30).await)).await
        });
        cx.notify();
    }

    /// Conta que já estava logada no proxy: segue para o nome sem abrir login.
    fn wizard_use_existing(&mut self, brand: Brand, account: Connected, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.wizard_turn();
        let Some(w) = self.accounts.cliproxy.as_mut() else { return };
        w.selected_binding = account.binding;
        (w.login, w.login_error, w.key) = (LoginPhase::Connected { brand, email: account.email }, None, None);
        self.wizard_name_step(window, cx);
    }

    /// Senha que ninguém sabe: o servidor troca por uma nova e guarda; com ela, o login pedido é tentado de novo.
    fn wizard_reset_key(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let done = self.wizard_send();
        let Some(w) = self.accounts.cliproxy.as_mut().filter(|w| !w.key_saving) else { return };
        let Some((brand, _)) = w.key else { return };
        (w.key_saving, w.login_error) = (true, None);
        let turn = w.turn;
        self.runtime.spawn(async move {
            done(CliproxyReply::KeyReset(turn, brand, api.server_post(&["engines", "cliproxy", "management-key", "reset"], 30).await)).await
        });
        cx.notify();
    }

    /// Continuar com a conta conectada: o nome vem do e-mail, e os modelos da última leitura (ou de uma nova).
    fn wizard_name_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(w) = self.accounts.cliproxy.as_mut() else { return };
        let LoginPhase::Connected { brand, email } = w.login.clone() else { return };
        let name = cx.new(|cx| InputState::new(window, cx).default_value(default_name(brand, &email)));
        w._subscriptions.push(cx.subscribe_in(&name, window, |this: &mut Hangar, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => { if let Some(w) = this.accounts.cliproxy.as_mut() { w.name_error = None; } cx.notify(); }
            InputEvent::PressEnter { .. } => this.wizard_save(cx),
            _ => {}
        }));
        name.update(cx, |input, cx| input.focus(window, cx));
        (w.step, w.name, w.name_error, w.save_error) = (WizardStep::Name, Some(name), None, None);
        let ready = w.status.as_ref().and_then(|s| s.as_ref().ok()).filter(|s| s.usable() && !s.models.is_empty()).map(|s| s.models.clone());
        match ready {
            Some(models) => self.wizard_models(models, brand, window, cx),
            None => self.wizard_read(Purpose::Models, cx),
        }
    }

    fn wizard_models(&mut self, mut models: Vec<ProxyModel>, brand: Brand, window: &mut Window, cx: &mut Context<Self>) {
        let Some(w) = self.accounts.cliproxy.as_mut() else { return };
        if brand == Brand::ChatGpt && let Some(binding) = &w.selected_binding {
            let prefix = w.status.as_ref().and_then(|s| s.as_ref().ok()).and_then(|s|
                s.connected.iter().find(|a| a.binding.as_ref() == Some(binding))).and_then(|a| a.prefix.as_deref());
            if let Some(prefix) = prefix {
                let prefix = format!("{prefix}/");
                models.retain_mut(|model| {
                    let Some(id) = model.id.strip_prefix(&prefix) else { return false };
                    model.id = id.to_owned();
                    true
                });
            } else { models.clear(); }
        }
        let current = Some(w.model.clone()).filter(|m| models.iter().any(|x| &x.id == m)).or_else(|| default_model(&models, brand));
        let choices: Vec<ModelChoice> = models.iter().map(|m| ModelChoice { id: m.id.clone(), label: m.id.clone(),
            hint: m.context_length.map(|n| format!("{}k", (n as f64 / 1000.).round())).unwrap_or_default() }).collect();
        let at = current.as_deref().and_then(|id| choices.iter().position(|c| c.id == id)).map(IndexPath::new);
        let searchable = choices.len() > 8;
        let picker = cx.new(|cx| SelectState::new(SearchableVec::new(choices), at, window, cx).searchable(searchable));
        w._subscriptions.push(cx.subscribe_in(&picker, window, |this: &mut Hangar, _, event: &SelectEvent<SearchableVec<ModelChoice>>, _, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            if let Some(w) = this.accounts.cliproxy.as_mut() { w.model = id.clone(); }
            cx.notify();
        }));
        (w.model, w.models, w.picker, w.models_error) = (current.unwrap_or_default(), Some(models), Some(picker), None);
        cx.notify();
    }

    /// Concluir: o mesmo `PUT /api/engines/{id}` do formulário, com a chave do proxy que fica no servidor.
    fn wizard_save(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let done = self.wizard_send();
        let taken = self.engine_names();
        let Some(w) = self.accounts.cliproxy.as_mut().filter(|w| !w.saving && w.saved.is_none() && w.step == WizardStep::Name) else { return };
        let Some(label) = w.name.as_ref().map(|n| n.read(cx).value().trim().to_owned()) else { return };
        let base_url = w.status.as_ref().and_then(|s| s.as_ref().ok()).and_then(|s| s.base_url.clone());
        let (Some(base_url), false) = (base_url, w.model.is_empty()) else { return };
        if label.is_empty() {
            w.name_error = Some(tr("cliproxy_wizard_name_empty"));
            cx.notify();
            return;
        }
        let id = engine_id(&label);
        match &taken {
            // Sem a lista não dá para saber se o nome pisaria em outro modelo.
            None => { w.save_error = Some(tr("accounts_engine_names_unknown")); cx.notify(); return; }
            Some(names) if names.contains(&id) => { w.name_error = Some(tr("accounts_engine_name_taken")); cx.notify(); return; }
            Some(_) => {}
        }
        let mut body = json!({"label": label, "base_url": base_url, "model": w.model, "subagent_model": w.model,
            "use_cliproxy_key": true, "create_only": true, "experimental_betas": true, "fast": w.fast && allows_fast(&w.model)});
        if matches!(&w.login, LoginPhase::Connected { brand: Brand::ChatGpt, .. }) {
            let Some(binding) = &w.selected_binding else {
                w.save_error = Some(tr_shared("cliproxy_choose_connected_account", &[]));
                cx.notify();
                return;
            };
            body["cliproxy_binding"] = json!(binding);
        }
        if let Some(limit) = w.models.as_ref().and_then(|m| m.iter().find(|m| m.id == w.model)).and_then(|m| m.context_length) {
            body["context_window"] = json!(safe_context(limit));
        }
        (w.saving, w.save_error, w.name_error) = (true, None, None);
        let turn = w.turn;
        self.runtime.spawn(async move {
            let saved = api.server_send(reqwest::Method::PUT, &["engines", &id], Some(body), 30).await;
            done(CliproxyReply::Saved(turn, label, id, saved)).await
        });
        cx.notify();
    }

    pub(super) fn receive_cliproxy(&mut self, reply: CliproxyReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            CliproxyReply::Read(turn, purpose, result) => {
                let parsed = parse_status(result);
                let Some(w) = self.wizard(turn) else { return };
                (w.reading, w.task) = (false, None);
                match purpose {
                    Purpose::Entry => {
                        let install = parsed.as_ref().ok().and_then(|s| s.install.clone()).unwrap_or_default();
                        let running_job = install.job.clone().filter(|j| !j.done);
                        // Proxy já respondendo, instalado pelo Hangar ou rodando sem instalação conhecida: não há o que instalar.
                        let ready = parsed.as_ref().is_ok_and(|s| s.usable())
                            && ((install.state == "managed" && install.running) || matches!(install.state.as_str(), "absent" | ""));
                        w.status = Some(parsed);
                        if let Some(job) = running_job {
                            w.job = Some(job);
                            self.wizard_install(Start::Follow, cx);
                        } else if ready {
                            w.step = WizardStep::Connect;
                        }
                    }
                    Purpose::AfterLogin => {
                        let LoginPhase::Resolving(brand) = w.login else { return };
                        let email = parsed.as_ref().ok().map(|s| new_email(&w.before, &s.connected, brand.provider())).unwrap_or_default();
                        if parsed.is_ok() { w.status = Some(parsed); }
                        w.selected_binding = None;
                        let identified = w.login_account.take().filter(|a| a.kind == brand.provider() && a.binding.is_some());
                        if brand == Brand::ChatGpt {
                            if let Some(account) = identified {
                                w.selected_binding = account.binding;
                                w.login = LoginPhase::Connected { brand, email: account.email };
                            } else {
                                w.login = LoginPhase::Pick;
                                w.login_error = Some(tr_shared("cliproxy_choose_connected_account", &[]));
                            }
                        } else {
                            w.login = LoginPhase::Connected { brand, email };
                        }
                    }
                    Purpose::Models => {
                        let LoginPhase::Connected { brand, .. } = w.login else { return };
                        let models = match &parsed {
                            Ok(s) if !s.found || s.base_url.is_none() => Err(tr_shared("cliproxy_missing", &[])),
                            Ok(ProxyStatus { error: Some(erro), base_url: Some(url), .. }) => Err(tr_shared("cliproxy_error", &[("url", url.as_str()), ("erro", erro.as_str())])),
                            Ok(s) => Ok(s.models.clone()),
                            Err(error) => Err(error.clone()),
                        };
                        if parsed.is_ok() { w.status = Some(parsed); }
                        match models {
                            Ok(models) => self.wizard_models(models, brand, window, cx),
                            Err(error) => w.models_error = Some(error),
                        }
                    }
                }
            }
            CliproxyReply::InstallFailed(turn, error) => {
                let Some(w) = self.wizard(turn) else { return };
                (w.installing, w.task) = (false, None);
                // Sessões no proxy: a entrega o reinicia, então pergunta antes, como a atualização.
                if error.code.as_deref() == Some("erro_cliproxy_sessoes_ativas") {
                    let sessions = w.status.as_ref().and_then(|s| s.as_ref().ok())
                        .and_then(|s| s.install.as_ref()).and_then(|i| i.sessions);
                    super::proxy_update::confirm_restart(super::proxy_update::Action::Adopt, sessions, window, cx,
                        |this, cx| this.wizard_install(Start::Adopt { confirm: true }, cx));
                } else {
                    w.install_error = Some(failure_text(&error));
                }
            }
            CliproxyReply::Progress(turn, result) => {
                let parsed = parse_status(result);
                let Some(w) = self.wizard(turn) else { return };
                match parsed {
                    Err(error) => { (w.installing, w.install_error) = (false, Some(error)); }
                    Ok(status) => {
                        let job = status.install.as_ref().and_then(|i| i.job.clone());
                        let adopted = job.as_ref().is_some_and(|job| job.done && job.kind == "adopt");
                        match &job {
                            Some(job) if !job.done => {}
                            Some(Job { error: Some(error), kind, .. }) => {
                                let text = if kind == "adopt" {
                                    let (latest, version) = status.install.as_ref()
                                        .map(|i| (i.latest.clone().unwrap_or_default(), i.version.clone().unwrap_or_default())).unwrap_or_default();
                                    super::proxy_update::adopt_failure_text(&error.code, &latest, &version)
                                } else { code_text(&error.code) };
                                (w.installing, w.install_error) = (false, Some(text));
                            }
                            Some(_) => { (w.installing, w.step, w.login) = (false, WizardStep::Connect, LoginPhase::Pick); }
                            // O servidor reiniciou no meio: o andamento era da memória dele.
                            None => { (w.installing, w.install_error) = (false, Some(tr("cliproxy_wizard_install_lost"))); }
                        }
                        w.job = job;
                        w.status = Some(Ok(status));
                        // A linha do CLIProxyAPI em Contas passa a dizer "Do Hangar".
                        if adopted { self.load_proxy_install(cx); }
                    }
                }
            }
            CliproxyReply::LoginStarted(turn, brand, result) => {
                let Some(w) = self.wizard(turn) else { return };
                w.task = None;
                match result {
                    Ok(value) => {
                        let url = value.get("url").and_then(Value::as_str).and_then(|u| url::Url::parse(u).ok())
                            .filter(|u| u.scheme() == "https" && u.username().is_empty() && u.password().is_none()).map(|u| u.to_string());
                        match (url, value.get("state").and_then(Value::as_str)) {
                            (Some(url), Some(state)) => {
                                (w.login, w.key) = (LoginPhase::Waiting { brand, url: url.clone(), state: state.to_owned() }, None);
                                self.wizard_relay(&url, state);
                                crate::app::links::open_link(&url, true, window, cx);
                                self.wizard_poll_login(cx);
                            }
                            _ => (w.login, w.login_error) = (LoginPhase::Pick, Some(tr("invalid_response"))),
                        }
                    }
                    Err(error) => {
                        w.login = LoginPhase::Pick;
                        match error.code.as_deref() {
                            Some("erro_cliproxy_sem_senha") => w.key = Some((brand, false)),
                            Some("erro_cliproxy_senha_recusada") => w.key = Some((brand, true)),
                            _ => w.login_error = Some(failure_text(&error)),
                        }
                    }
                }
            }
            CliproxyReply::LoginStatus(turn, result) => {
                let Some(w) = self.wizard(turn) else { return };
                let LoginPhase::Waiting { brand, .. } = w.login else { return };
                match result {
                    Ok(value) => match value.get("status").and_then(Value::as_str) {
                        Some("ok") => {
                            w.login_account = serde_json::from_value(value["account"].clone()).ok();
                            (w.polling, w.login, w.login_error, w.relay) = (false, LoginPhase::Resolving(brand), None, None);
                            self.wizard_read(Purpose::AfterLogin, cx);
                        }
                        Some("error") => {
                            let reason = value.get("error").and_then(Value::as_str).filter(|e| !e.is_empty());
                            // O login morreu no proxy: a volta que chegar agora não tem para onde ir.
                            (w.polling, w.relay) = (false, None);
                            w.login_error = Some(match reason {
                                Some(reason) => tr("cliproxy_wizard_login_failed").replace("{reason}", reason),
                                None => tr("cliproxy_wizard_login_failed_plain"),
                            });
                        }
                        _ => w.login_error = None,
                    },
                    Err(error) => {
                        if error.status.is_some() { w.polling = false; }
                        w.login_error = Some(failure_text(&error));
                    }
                }
            }
            CliproxyReply::Callback(state, result) => {
                let Some(w) = self.accounts.cliproxy.as_mut()
                    .filter(|w| matches!(&w.login, LoginPhase::Waiting { state: current, .. } if *current == state)) else { return };
                w.sending = false;
                match result {
                    Ok(_) => {
                        w.callback_error = None;
                        // A leitura tinha parado (erro de antes): o "ok" agora só chega lendo de novo.
                        if !w.polling { self.wizard_poll_login(cx); }
                    }
                    Err(error) => {
                        // Pelo encaminhamento o campo pode estar fechado: o motivo e o colar ficam à vista.
                        w.other_open = true;
                        w.callback_error = Some(if error.status == Some(400) { tr("cliproxy_wizard_callback_invalid") } else { failure_text(&error) });
                    }
                }
            }
            CliproxyReply::KeySaved(turn, brand, result) | CliproxyReply::KeyReset(turn, brand, result) => {
                let Some(w) = self.wizard(turn) else { return };
                w.key_saving = false;
                match result {
                    Ok(_) => {
                        w.key = None;
                        w.mgmt.update(cx, |input, cx| input.set_value("", window, cx));
                        self.wizard_login(brand, window, cx);
                    }
                    Err(error) => w.login_error = Some(failure_text(&error)),
                }
            }
            CliproxyReply::Saved(turn, label, id, result) => {
                // Deu certo, falhou ou ficou incerto: as listas relidas mostram o que o servidor guardou.
                self.load_engines(cx);
                self.load_accounts(false, cx);
                let Some(w) = self.wizard(turn) else { return };
                w.saving = false;
                match result {
                    Ok(_) => w.saved = Some((label, id)),
                    // Outra tela criou o mesmo nome depois da lista lida aqui: o servidor recusa em vez de sobrescrever.
                    Err(error) if error.code.as_deref() == Some("erro_motor_ja_existe") => w.name_error = Some(tr("accounts_engine_name_taken")),
                    Err(error) => w.save_error = Some(if error.uncertain { tr("accounts_engine_save_uncertain") } else { Self::failure(&error) }),
                }
            }
        }
        cx.notify();
    }

    /// O assistente no lugar da lista: cabeçalho, os três passos e o passo atual.
    pub(super) fn render_cliproxy_wizard(&self, cx: &mut Context<Self>) -> Option<Div> {
        let w = self.accounts.cliproxy.as_ref()?;
        let head = div().flex().items_start().gap(px(12.))
            .child(provider_badge(BADGE_URL, "CLIProxyAPI", 40.))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(tr("cliproxy_wizard_title")))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr("cliproxy_wizard_subtitle"))))
            .child(Button::new("cliproxy-wizard-close").ghost().small().icon(IconName::Close).accessibility_label(tr("cliproxy_wizard_close"))
                .disabled(w.saving).on_click(cx.listener(|this, _, window, cx| this.close_cliproxy_wizard(window, cx))));
        let current = match w.step { WizardStep::Install => 0, WizardStep::Connect => 1, WizardStep::Name => 2 };
        let finished = w.saved.is_some();
        let steps = wizard_steps("cliproxy-wizard-steps", current, finished);
        let body = match w.step {
            WizardStep::Install => self.wizard_install_body(w, cx),
            WizardStep::Connect => self.wizard_connect_body(w, cx),
            WizardStep::Name => self.wizard_name_body(w, cx),
        };
        Some(settings_box().mt(px(24.)).p(px(20.)).gap(px(20.)).max_w(px(600.))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| { cx.stop_propagation(); this.close_cliproxy_wizard(window, cx); }))
            .child(head).child(steps).child(div().min_h(px(190.)).flex().flex_col().gap(px(12.)).child(body)))
    }

    fn wizard_install_body(&self, w: &CliproxyWizard, cx: &mut Context<Self>) -> Div {
        let col = div().flex().flex_col().gap(px(12.));
        let status = match &w.status {
            None => return col.child(waiting_line("cliproxy-wizard-reading", tr_shared("cliproxy_detecting", &[]))),
            Some(Err(error)) => return col.child(banner(theme::danger(), IconName::CircleAlert, tr("cliproxy_wizard_read_failed").replace("{reason}", error)))
                .child(footer().child(spacer()).child(Button::new("cliproxy-wizard-reread").outline().small().icon(IconName::RefreshCw)
                    .label(tr("cliproxy_wizard_retry")).loading(w.reading).disabled(w.reading)
                    .on_click(cx.listener(|this, _, _, cx| this.wizard_read(Purpose::Entry, cx))))),
            Some(Ok(status)) => status,
        };
        let external = status.install.as_ref().is_some_and(|i| i.state == "external");
        let failed = w.install_error.clone();
        let action = |id: &'static str, label: &str, cx: &mut Context<Self>| Button::new(id).primary().small().label(tr(label))
            .loading(w.installing).disabled(w.installing).on_click(cx.listener(|this, _, _, cx| this.wizard_install(Start::Install, cx)));
        if external {
            return self.wizard_external_body(w, status.install.as_ref(), cx);
        }
        let at = w.job.as_ref().map(|job| step_index(&job.step));
        let all_done = w.job.as_ref().is_some_and(|job| job.done && job.error.is_none());
        let rows = (0..INSTALL_ITEMS).map(|index| install_item(index, match at {
            Some(_) if all_done => ItemState::Done,
            Some(at) if index < at => ItemState::Done,
            Some(at) if index == at && failed.is_some() => ItemState::Failed,
            Some(at) if index == at && w.installing => ItemState::Running,
            _ => ItemState::Waiting,
        }));
        let (title, button) = match (&failed, w.installing) {
            (Some(_), _) => ("cliproxy_wizard_install_stopped", "cliproxy_wizard_retry"),
            (None, true) => ("cliproxy_wizard_installing", "cliproxy_wizard_installing"),
            (None, false) => ("cliproxy_wizard_lead", "cliproxy_wizard_install"),
        };
        col.child(lead(tr(title)))
            .when(failed.is_none() && !w.installing, |el| el.child(help(tr("cliproxy_wizard_lead_help"))))
            .child(div().mt(px(2.)).flex().flex_col().gap(px(9.)).children(rows))
            .children(failed.map(|error| banner(theme::danger(), IconName::CircleAlert, error)))
            .child(footer().child(spacer())
                .child(Button::new("cliproxy-wizard-cancel").outline().small().label(tr("cancel")).disabled(w.installing)
                    .on_click(cx.listener(|this, _, window, cx| this.close_cliproxy_wizard(window, cx))))
                .child(action("cliproxy-wizard-install", button, cx)))
    }

    /// Proxy de fora: passar para o Hangar (download, troca de serviço e conferência) ou usar como está (só o config).
    fn wizard_external_body(&self, w: &CliproxyWizard, install: Option<&InstallState>, cx: &mut Context<Self>) -> Div {
        let handover = install.and_then(|i| i.handover.as_ref());
        let can_adopt = handover.is_some_and(|h| h.ok);
        let adopt = w.adopt && can_adopt;
        let latest = install.and_then(|i| i.latest.clone()).unwrap_or_default();
        let lead_text = match install.and_then(|i| i.version.as_deref()) {
            Some(version) => tr("cliproxy_wizard_external_version").replace("{version}", version),
            None => tr("cliproxy_wizard_external"),
        };
        let mut col = div().flex().flex_col().gap(px(12.)).child(lead(lead_text));
        // As etapas aparecem só durante a entrega; a que falhou volta às duas escolhas, com o motivo embaixo.
        if adopt && w.installing && w.job.as_ref().is_none_or(|job| job.kind == "adopt") {
            let step = w.job.as_ref().map_or("download", |job| job.step.as_str());
            col = col.child(div().mt(px(2.)).flex().flex_col().gap(px(9.)).children(super::proxy_update::adopt_items(&latest, step)));
        } else {
            let option = |value: bool, title: String, desc: String, enabled: bool, cx: &mut Context<Self>| {
                Radio::new(SharedString::from(format!("cliproxy-wizard-{}", if value { "adopt" } else { "keep" }))).small()
                    .label(title).checked(adopt == value).disabled(w.installing || !enabled)
                    .child(div().text_size(px(12.)).line_height(px(17.)).text_color(theme::muted()).whitespace_normal().child(desc))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(w) = this.accounts.cliproxy.as_mut() { (w.adopt, w.install_error) = (value, None); }
                        cx.notify();
                    }))
            };
            let adopt_desc = match handover {
                Some(h) if !h.ok => code_text(&h.code),
                _ => tr("cliproxy_wizard_adopt_desc"),
            };
            let adopt_title = format!("{} · {}", tr("cliproxy_wizard_adopt_title"), tr("cliproxy_wizard_recommended"));
            col = col.child(div().flex().flex_col().gap(px(10.)).pt(px(2.))
                .child(option(true, adopt_title, adopt_desc, can_adopt, cx))
                .child(option(false, tr("cliproxy_wizard_keep_title"), tr("cliproxy_wizard_keep_desc"), true, cx)));
        }
        let (label, start) = match (adopt, w.install_error.is_some()) {
            (_, true) => ("cliproxy_wizard_retry", if adopt { Start::Adopt { confirm: false } } else { Start::Install }),
            (true, false) => ("cliproxy_wizard_continue", Start::Adopt { confirm: false }),
            (false, false) => (if w.installing { "cliproxy_wizard_continuing" } else { "cliproxy_wizard_continue" }, Start::Install),
        };
        col.children(w.install_error.clone().map(|error| banner(theme::danger(), IconName::CircleAlert, error)))
            .child(footer().child(spacer())
                .child(Button::new("cliproxy-wizard-cancel").outline().small().label(tr("cancel")).disabled(w.installing)
                    .on_click(cx.listener(|this, _, window, cx| this.close_cliproxy_wizard(window, cx))))
                .child(Button::new("cliproxy-wizard-continue").primary().small().label(tr(label)).loading(w.installing).disabled(w.installing)
                    .on_click(cx.listener(move |this, _, _, cx| this.wizard_install(start, cx)))))
    }

    fn wizard_connect_body(&self, w: &CliproxyWizard, cx: &mut Context<Self>) -> Div {
        let col = div().flex().flex_col().gap(px(12.));
        match &w.login {
            LoginPhase::Pick | LoginPhase::Opening(_) => {
                let opening = matches!(w.login, LoginPhase::Opening(_));
                let busy = opening || w.key_saving;
                let card = |brand: Brand, desc: &str, recommended: bool, cx: &mut Context<Self>| {
                    let title = div().w_full().flex().items_center().gap(px(8.)).child(brand_logo(brand, 26.))
                        .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD).child(brand.name()))
                        .when(recommended, |el| el.child(div().flex_shrink_0().px(px(7.)).py(px(1.)).rounded_full().bg(theme::accent_dim())
                            .text_color(theme::accent_text()).text_size(px(11.)).child(tr("cliproxy_wizard_recommended"))));
                    Button::new(SharedString::from(format!("cliproxy-wizard-pick-{}", brand.provider()))).outline().w_full().h_auto()
                        .px(px(14.)).py(px(12.)).rounded(px(12.)).disabled(busy).accessibility_label(brand.name())
                        .child(div().w_full().flex().flex_col().items_start().gap(px(6.)).child(title)
                            .child(div().w_full().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr(desc))))
                        .on_click(cx.listener(move |this, _, window, cx| this.wizard_login(brand, window, cx)))
                };
                let mut col = col.child(lead(tr("cliproxy_wizard_pick_lead"))).child(help(tr("cliproxy_wizard_pick_help")));
                if let Some((_, rejected)) = w.key {
                    let label = tr_shared("cliproxy_senha_placeholder", &[]);
                    let ready = !w.mgmt.read(cx).value().trim().is_empty();
                    col = col.child(div().flex().flex_col().gap(px(8.)).p(px(12.)).rounded(px(10.)).border_1().border_color(theme::border_strong())
                        .child(help(tr(if rejected { "cliproxy_wizard_key_rejected" } else { "cliproxy_wizard_key_needed" })))
                        .child(div().flex().items_center().gap(px(8.))
                            .child(div().flex_1().min_w_0().child(Input::new(&w.mgmt).small().disabled(w.key_saving).aria_label(label)))
                            .child(Button::new("cliproxy-wizard-key-save").outline().small().label(tr("cliproxy_wizard_key_save"))
                                .loading(w.key_saving).disabled(w.key_saving || !ready)
                                .on_click(cx.listener(|this, _, _, cx| this.wizard_save_key(cx)))))
                        .child(div().flex().items_center().gap(px(8.))
                            .child(div().flex_1().min_w_0().child(help(tr("cliproxy_wizard_key_reset_help"))))
                            .child(Button::new("cliproxy-wizard-key-reset").ghost().small().icon(IconName::RefreshCw).label(tr("cliproxy_wizard_key_reset"))
                                .disabled(w.key_saving).on_click(cx.listener(|this, _, _, cx| this.wizard_reset_key(cx))))));
                }
                let existing: Vec<(Brand, Connected)> = w.status.as_ref().and_then(|s| s.as_ref().ok())
                    .map(|s| s.connected.iter().filter_map(|a| Some((Brand::of(&a.kind)?, a.clone()))).collect()).unwrap_or_default();
                let rows = existing.into_iter().enumerate().map(|(index, (brand, account))| {
                    let shown = if account.email.is_empty() { brand.name().to_owned() } else { account.email.clone() };
                    let unavailable = busy || (brand == Brand::ChatGpt && account.binding.is_none());
                    div().flex().items_center().gap(px(10.)).px(px(12.)).py(px(8.)).rounded(px(12.)).border_1().border_color(theme::border())
                        .child(brand_logo(brand, 26.))
                        .child(div().flex_1().min_w_0().truncate().child(shown))
                        .child(Button::new(SharedString::from(format!("cliproxy-wizard-use-{index}"))).outline().small()
                            .label(tr("cliproxy_wizard_use_account")).disabled(unavailable)
                            .on_click(cx.listener(move |this, _, window, cx| this.wizard_use_existing(brand, account.clone(), window, cx))))
                }).collect::<Vec<_>>();
                let has_existing = !rows.is_empty();
                col.children(w.login_error.clone().map(|error| banner(theme::danger(), IconName::CircleAlert, error)))
                    .when(has_existing, |el| el
                        .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(tr("cliproxy_wizard_existing")))
                        .child(div().flex().flex_col().gap(px(8.)).children(rows))
                        .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(tr("cliproxy_wizard_new_account"))))
                    .child(div().mt(px(2.)).flex().gap(px(10.))
                        .child(div().flex_1().min_w_0().child(card(Brand::ChatGpt, "cliproxy_wizard_chatgpt_desc", true, cx)))
                        .child(div().flex_1().min_w_0().child(card(Brand::Claude, "cliproxy_wizard_claude_desc", false, cx))))
                    .when(opening, |el| el.child(waiting_line("cliproxy-wizard-opening", tr("cliproxy_wizard_opening"))))
            }
            LoginPhase::Waiting { brand, url, .. } => {
                let indicator = if w.polling {
                    chrome::Spinner::new("cliproxy-wizard-waiting", IconName::LoaderCircle, px(18.), theme::accent()).into_any_element()
                } else {
                    chrome::small_icon(IconName::CircleAlert, 18., theme::danger()).into_any_element()
                };
                let this = cx.entity().downgrade();
                let other = Disclosure::new("cliproxy-wizard-other", w.other_open, tr("cliproxy_wizard_other_computer"), true)
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| {
                        if let Some(w) = this.accounts.cliproxy.as_mut() { w.other_open = open; }
                        cx.notify();
                    }); });
                let ready = !w.callback.read(cx).value().trim().is_empty();
                let paste = div().flex().flex_col().gap(px(8.)).pt(px(2.))
                    .child(help(tr("cliproxy_wizard_other_help")))
                    .child(div().font_family(theme::MONO).child(Input::new(&w.callback).small().disabled(w.sending).aria_label(tr("cliproxy_wizard_other_computer"))))
                    .children(w.callback_error.clone().map(|error| tone(error, theme::danger())))
                    .child(div().flex().child(Button::new("cliproxy-wizard-callback").outline().small().label(tr("cliproxy_wizard_callback_send"))
                        .loading(w.sending).disabled(w.sending || !ready).on_click(cx.listener(|this, _, _, cx| this.wizard_callback(cx)))));
                let failed = !w.polling && w.login_error.is_some();
                let (brand, open) = (*brand, url.clone());
                col.child(div().p(px(16.)).rounded(px(12.)).border_1().border_color(theme::border()).flex().items_start().gap(px(12.))
                    .child(div().pt(px(1.)).flex_shrink_0().child(indicator))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(6.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(tr("cliproxy_wizard_waiting_title").replace("{brand}", brand.name())))
                        .child(help(tr("cliproxy_wizard_waiting_help")))
                        .child(div().flex().child(other))
                        .when(w.other_open, |el| el.child(paste))
                        .children(w.login_error.clone().map(|error| tone(error, theme::danger())))))
                    .child(footer()
                        .child(Button::new("cliproxy-wizard-back").ghost().small().icon(IconName::ArrowLeft).label(tr("cliproxy_wizard_back"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this.wizard_turn();
                                if let Some(w) = this.accounts.cliproxy.as_mut() { (w.login, w.login_error, w.relay) = (LoginPhase::Pick, None, None); }
                                cx.notify();
                            })))
                        .child(spacer())
                        // Login que já terminou em erro não volta pela mesma página: abre um login novo.
                        .child(Button::new("cliproxy-wizard-reopen").outline().small().icon(IconName::ExternalLink).label(tr("cliproxy_wizard_reopen"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if failed { this.wizard_login(brand, window, cx); } else { crate::app::links::open_link(&open, true, window, cx); }
                            }))))
            }
            LoginPhase::Resolving(_) => col.child(waiting_line("cliproxy-wizard-resolving", tr("cliproxy_wizard_resolving"))),
            LoginPhase::Connected { brand, email } => col.child(lead(tr("cliproxy_wizard_connected_title"))).child(account_row(*brand, email))
                .child(footer()
                    .child(Button::new("cliproxy-wizard-another").ghost().small().icon(IconName::Plus).label(tr("cliproxy_wizard_other_account"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let _ = this.wizard_turn();
                            if let Some(w) = this.accounts.cliproxy.as_mut() { (w.login, w.login_error) = (LoginPhase::Pick, None); }
                            cx.notify();
                        })))
                    .child(spacer())
                    .child(Button::new("cliproxy-wizard-connected-next").primary().small().label(tr("cliproxy_wizard_continue"))
                        .on_click(cx.listener(|this, _, window, cx| this.wizard_name_step(window, cx))))),
        }
    }

    fn wizard_name_body(&self, w: &CliproxyWizard, cx: &mut Context<Self>) -> Div {
        let col = div().flex().flex_col().gap(px(16.));
        let name = w.name.as_ref().map(|n| n.read(cx).value().trim().to_owned()).unwrap_or_default();
        let fast_shown = allows_fast(&w.model);
        if let Some((label, id)) = &w.saved {
            let template = tr("cliproxy_wizard_done_help");
            let at = template.find("{name}");
            let plain = template.replace("{name}", label);
            let strong = HighlightStyle { color: Some(theme::text()), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() };
            let text = StyledText::new(plain).with_highlights(at.map(|at| (at..at + label.len(), strong)));
            return col.child(div().flex().items_center().gap(px(12.))
                    .child(div().size(px(36.)).flex_shrink_0().rounded_full().bg(theme::success().opacity(0.16)).flex().items_center().justify_center()
                        .child(chrome::small_icon(IconName::Check, 18., theme::success())))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(tr("cliproxy_wizard_done_title")))
                        .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text))))
                .child(preview().child(tr("cliproxy_wizard_done_preview")).child(pill(label.clone()))
                    .child(if fast_shown && w.fast { format!("{} · {}", w.model, tr("cliproxy_wizard_fast_available")) } else { w.model.clone() }))
                .child(footer().child(spacer())
                    .child(Button::new("cliproxy-wizard-done").outline().small().label(tr("cliproxy_wizard_close"))
                        .on_click(cx.listener(|this, _, window, cx| this.close_cliproxy_wizard(window, cx))))
                    .child(Button::new("cliproxy-wizard-open-session").primary().small()
                        .label(tr("cliproxy_wizard_open_session").replace("{name}", label))
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, window, cx| this.wizard_open_session(id.clone(), window, cx)
                        }))));
        }
        let (brand, email) = match &w.login { LoginPhase::Connected { brand, email } => (*brand, email.as_str()), _ => (Brand::ChatGpt, "") };
        let busy = w.saving;
        let name_field = match &w.name {
            Some(input) => field(tr("cliproxy_wizard_name"), Input::new(input).disabled(busy).aria_label(tr("cliproxy_wizard_name")))
                .child(name_preview(&name))
                .children(w.name_error.clone().map(|error| tone(error, theme::danger()))),
            None => div(),
        };
        let retry = |cx: &mut Context<Self>| Button::new("cliproxy-wizard-models-retry").outline().small().icon(IconName::RefreshCw)
            .label(tr("cliproxy_wizard_retry")).loading(w.reading).disabled(w.reading || busy)
            .on_click(cx.listener(|this, _, _, cx| this.wizard_read(Purpose::Models, cx)));
        let model: AnyElement = match (&w.models_error, &w.models, &w.picker) {
            (Some(error), _, _) => div().flex().flex_col().gap(px(8.)).child(tone(error.clone(), theme::danger())).child(div().flex().child(retry(cx))).into_any_element(),
            (None, Some(list), _) if list.is_empty() => div().flex().flex_col().gap(px(8.)).child(help(tr("cliproxy_wizard_models_empty")))
                .child(div().flex().child(retry(cx))).into_any_element(),
            (None, Some(_), Some(picker)) => Select::new(picker).disabled(busy).accessibility_label(tr("cliproxy_wizard_model")).into_any_element(),
            _ => waiting_line("cliproxy-wizard-models", tr("cliproxy_wizard_models_loading")).into_any_element(),
        };
        let can_save = !busy && !w.model.is_empty() && w.picker.is_some();
        col.child(account_row(brand, email))
            .child(name_field)
            .child(field(tr("cliproxy_wizard_model"), model).child(help(tr("cliproxy_wizard_model_help"))))
            .when(fast_shown, |el| el.child(fast_field(
                Switch::new("cliproxy-wizard-fast").checked(w.fast).disabled(busy).accessibility_label(tr("cliproxy_wizard_fast"))
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        if let Some(w) = this.accounts.cliproxy.as_mut() { w.fast = *on; }
                        cx.notify();
                    })))))
            .children(w.save_error.clone().map(|error| tone(error, theme::danger())))
            .child(footer().child(spacer()).child(Button::new("cliproxy-wizard-save").primary().small()
                .label(tr(if busy { "cliproxy_wizard_saving" } else { "cliproxy_wizard_finish" })).loading(busy).disabled(!can_save)
                .on_click(cx.listener(|this, _, _, cx| this.wizard_save(cx)))))
    }
}

#[cfg(test)]
mod tests {
    use super::{Brand, Connected, ProxyModel, allows_fast, default_model, default_name, new_email, safe_context};

    fn account(kind: &str, email: &str) -> Connected { Connected { kind: kind.into(), email: email.into(), binding: None, prefix: None } }
    fn model(id: &str) -> ProxyModel { ProxyModel { id: id.into(), context_length: None } }

    #[test]
    fn fast_follows_the_server_rule() {
        assert!(allows_fast("gpt-6.1-sol"));
        assert!(allows_fast("openai/gpt-5"));
        assert!(!allows_fast("gpt-oss-120b"));
        assert!(!allows_fast("claude-opus-5-5"));
        assert!(!allows_fast(""));
    }

    #[test]
    fn the_engine_window_keeps_the_codex_margin() {
        assert_eq!(safe_context(872_000), 828_400);
        assert_eq!(safe_context(500_000), 475_000);
    }

    #[test]
    fn name_comes_from_the_email() {
        assert_eq!(default_name(Brand::ChatGpt, "pessoal@gmail.com"), "GPT pessoal");
        assert_eq!(default_name(Brand::Claude, "eu@exemplo.com"), "Claude eu");
        assert_eq!(default_name(Brand::ChatGpt, ""), "GPT");
    }

    #[test]
    fn the_new_account_is_the_one_that_was_not_there() {
        let before = [account("codex", "a@x.com")];
        let after = [account("codex", "a@x.com"), account("claude", "c@x.com"), account("codex", "b@x.com")];
        assert_eq!(new_email(&before, &after, "codex"), "b@x.com");
        assert_eq!(new_email(&before, &after, "claude"), "c@x.com");
        // Relogar a mesma conta não traz nenhuma nova: fica a do tipo pedido.
        assert_eq!(new_email(&after, &after, "codex"), "a@x.com");
        assert_eq!(new_email(&before, &[], "codex"), "");
    }

    #[test]
    fn default_model_prefers_the_connected_subscription() {
        let list = [model("claude-sonnet-5"), model("gpt-oss-120b"), model("gpt-6.1-sol")];
        assert_eq!(default_model(&list, Brand::ChatGpt).as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(default_model(&list, Brand::Claude).as_deref(), Some("claude-sonnet-5"));
        assert_eq!(default_model(&[model("x")], Brand::Claude).as_deref(), Some("x"));
        assert_eq!(default_model(&[], Brand::ChatGpt), None);
    }
}
