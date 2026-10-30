//! Contas Claude: entrar (e renovar o login), sair, remover e adicionar. Cada escrita vai ao servidor uma vez e não
//! se repete sozinha: queda depois de enviar é resultado incerto, dito assim, e a lista relida mostra o que valeu.
//! Uma ação em curso segura os controles das outras, para que a resposta sempre ache quem a pediu.
use super::*;
use gpui_kit::component::WindowExt;
use tokio::sync::oneshot;
use std::{cell::Cell, rc::Rc};
use crate::i18n::tr_shared;

/// Tentativa de login numa conta; enquanto existe, a página mostra os passos no lugar da lista, como no web.
pub(super) struct SignIn {
    /// Nome no disco: é ele que vai para as rotas, nunca o apelido.
    label: String,
    name: String,
    /// Número da tentativa: resposta de outra tentativa não mexe nesta.
    attempt: u64,
    /// O servidor tem (ou pode ter, depois de um início incerto) uma tentativa aberta nesta conta.
    open: bool,
    starting: bool,
    sending: bool,
    stopping: bool,
    /// A tentativa acabou sem login: sobram Voltar e Tentar de novo.
    failed: bool,
    url: Option<String>,
    copied: bool,
    step_failed: bool,
    error: Option<String>,
    /// Conectada: e-mail e plano que o servidor confirmou.
    done: Option<(Option<String>, Option<String>)>,
    code: Entity<InputState>,
    has_code: bool,
    poll: Option<JoinHandle<()>>,
    _subscription: Subscription,
    /// Antes da tentativa: onde abrir a autorização.
    choosing: Option<Choosing>,
    /// O navegador escolhido, repetido em "Tentar de novo".
    target: Option<Value>,
    /// `browser` do passo do login: estado, motivo e máquina.
    browser: Option<Value>,
    view: Option<LoginView>,
    view_focus: FocusHandle,
}

enum Choosing {
    Loading,
    Ready { targets: Vec<Value>, last: Option<Value> },
}

/// A janela do login transmitida da máquina do navegador.
struct LoginView {
    frame: Option<Arc<RenderImage>>,
    size: (f64, f64),
    title: String,
    can_input: bool,
    notice: Option<&'static str>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    last_move: Option<Instant>,
    input: tokio::sync::mpsc::UnboundedSender<Value>,
    frames: JoinHandle<()>,
    sender: JoinHandle<()>,
}

impl Drop for LoginView {
    fn drop(&mut self) {
        self.frames.abort();
        self.sender.abort();
    }
}

/// O quadro decodificado fora da tela, ou o que o servidor disse no lugar dele.
pub(in crate::app) enum ViewReply {
    Frame { image: Arc<RenderImage>, size: (f64, f64), title: String, can_input: bool },
    Notice(&'static str),
    Lost,
}

const VIEW_EVERY: Duration = Duration::from_millis(350);
const VIEW_MOVE_EVERY: Duration = Duration::from_millis(80);

impl Drop for SignIn {
    // Tarefa do tokio não morre com o handle: sem isto a leitura do passo seguiria no servidor anterior.
    fn drop(&mut self) { if let Some(poll) = self.poll.take() { poll.abort(); } }
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::app) enum ChangeKind { SignOut, Remove }

/// Sair ou remover em voo.
pub(super) struct Change { pub(super) id: String, pub(super) kind: ChangeKind }

/// Começar e confirmar levam de volta se a tentativa ainda tinha dono na tela. Sem dono (página fechada, ou a
/// resposta de uma conexão antiga descartada no caminho, que derruba o remetente), a própria tarefa fecha a
/// tentativa no servidor dela.
pub(in crate::app) enum ActionReply {
    Started(u64, Result<Value, Failure>, oneshot::Sender<bool>),
    Step(u64, Result<Value, Failure>),
    Targets(u64, Result<Value, Failure>),
    View(u64, ViewReply),
    Code(u64, Result<Value, Failure>, oneshot::Sender<bool>),
    Stopped(u64, bool, Result<Value, Failure>),
    /// O `Option<bool>` é a escolha de guardar as conversas que a exclusão levou, para repeti-la ao liberar.
    Changed(String, String, ChangeKind, Option<bool>, Result<crate::api::Removed, Failure>),
    Created(String, Result<Value, Failure>),
}

/// `Catalog(true)`: provedores para um modelo do Claude Code; `Catalog(false)`: para uma chave de outro agente.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum AddStep { Choose, Subscriptions, Claude, Catalog(bool) }

/// O diálogo "Adicionar conta…". É uma entidade própria porque o diálogo é desenhado durante o desenho da janela,
/// quando o estado do `Hangar` não pode ser lido.
pub(super) struct AddAccount {
    hangar: WeakEntity<Hangar>,
    step: AddStep,
    name: Entity<InputState>,
    slug: String,
    /// Conta padrão sem login: uma conta nova não resolve, o login vai para ela (como no web).
    base: Option<(String, String)>,
    saving: bool,
    error: Option<String>,
    _subscription: Subscription,
}

/// Uma escrita de login por vez por servidor e conta, também entre conexões: a limpeza de uma tentativa antiga
/// nunca alcança a tentativa nova da mesma conta (ida e volta A → B → A).
// ponytail: o mapa só cresce, uma entrada por conta que já tentou entrar; limpar quando o número incomodar.
fn login_turn(api: &Api, label: &str) -> Arc<tokio::sync::Mutex<()>> {
    static TURNS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = std::sync::OnceLock::new();
    TURNS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
        .entry(format!("{}\n{label}", api.identity())).or_default().clone()
}

/// Fecha a tentativa sem tela. É o único resultado que não aparece (não há tela onde mostrá-lo): a falha vai
/// para o log do app.
async fn cancel_unseen(api: &Api, label: &str) {
    if let Err(error) = api.server_post(&["conta-estado", label, "login", "cancelar"], 15).await {
        eprintln!("login de {label} sem tela não foi fechado no servidor: {}", error.detail);
    }
}

/// Nome da pasta como o servidor aceita (`[a-z0-9][a-z0-9_-]{0,31}`): minúsculas, sem acento, o resto vira hífen.
pub(super) fn account_slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.trim().to_lowercase().chars() {
        let ch = match ch {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a', 'é' | 'è' | 'ê' | 'ë' => 'e', 'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o', 'ú' | 'ù' | 'û' | 'ü' => 'u', 'ç' => 'c', 'ñ' => 'n', ch => ch,
        };
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' { out.push(ch); } else if !out.ends_with('-') { out.push('-'); }
    }
    out.trim_matches('-').chars().take(32).collect::<String>().trim_end_matches('-').to_owned()
}

impl AddAccount {
    fn new(hangar: WeakEntity<Hangar>, step: AddStep, base: Option<(String, String)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(tr("accounts_add_name_placeholder")));
        let subscription = cx.subscribe_in(&name, window, |this: &mut AddAccount, input, event: &InputEvent, window, cx| match event {
            InputEvent::Change => { this.slug = account_slug(&input.read(cx).value()); cx.notify(); }
            InputEvent::PressEnter { .. } => this.create(window, cx),
            _ => {}
        });
        Self { hangar, step, name, slug: String::new(), base, saving: false, error: None, _subscription: subscription }
    }

    fn title(&self) -> String {
        tr(match self.step {
            AddStep::Choose => "accounts_add_title", AddStep::Subscriptions => "accounts_add_subscription", AddStep::Claude => "accounts_add_claude",
            AddStep::Catalog(true) => "accounts_add_model_path", AddStep::Catalog(false) => "accounts_add_key_path",
        })
    }

    fn go(&mut self, step: AddStep, window: &mut Window, cx: &mut Context<Self>) {
        (self.step, self.error) = (step, None);
        if step == AddStep::Claude && self.base.is_none() { self.name.update(cx, |input, cx| input.focus(window, cx)); }
        // O título do diálogo sai do desenho do Hangar.
        let _ = self.hangar.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    fn create(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.step != AddStep::Claude || self.base.is_some() || self.slug.is_empty() { return; }
        (self.saving, self.error) = (true, None);
        let slug = self.slug.clone();
        // O Hangar redesenha junto: é ele que desenha o diálogo, que não fecha enquanto cria.
        if !self.hangar.update(cx, |hangar, cx| { cx.notify(); hangar.create_account(slug) }).unwrap_or(false) {
            (self.saving, self.error) = (false, Some(tr("settings_offline")));
        }
        cx.notify();
    }
}

/// Uma escolha do diálogo: título, o que é e o botão (desligado dizendo "chega na próxima versão" quando ainda não chegou).
fn choice(title: String, description: String, action: Button) -> Div {
    div().flex().items_center().gap(px(12.)).py(px(10.)).border_t_1().border_color(theme::border())
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().font_weight(FontWeight::MEDIUM).child(title))
            .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(description)))
        .child(div().flex_shrink_0().child(action))
}

impl Render for AddAccount {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let saving = self.saving;
        let back = Button::new("accounts-add-back").ghost().small().icon(IconName::ArrowLeft).label(tr("settings_back")).disabled(saving)
            .on_click(cx.listener(|this, _, window, cx| {
                let previous = if this.step == AddStep::Claude { AddStep::Subscriptions } else { AddStep::Choose };
                this.go(previous, window, cx);
            }));
        let body = match self.step {
            AddStep::Choose => div().flex().flex_col()
                .child(choice(tr("accounts_add_subscription"), tr("accounts_add_subscription_desc"),
                    Button::new("accounts-add-pick-subscription").outline().small().label(tr("accounts_add_pick"))
                        .on_click(cx.listener(|this, _, window, cx| this.go(AddStep::Subscriptions, window, cx)))))
                .child(choice(tr("accounts_add_model_path"), tr("accounts_add_model_desc"),
                    Button::new("accounts-add-pick-model").outline().small().label(tr("accounts_add_pick"))
                        .on_click(cx.listener(|this, _, window, cx| this.go(AddStep::Catalog(true), window, cx)))))
                .child(choice(tr("accounts_add_key_path"), tr("accounts_add_key_desc"),
                    Button::new("accounts-add-pick-key").outline().small().label(tr("accounts_add_pick"))
                        .on_click(cx.listener(|this, _, window, cx| this.go(AddStep::Catalog(false), window, cx))))),
            AddStep::Catalog(model) => div().flex().flex_col().children(keys::PROVIDERS.iter().map(|provider| {
                let id = provider.id;
                keys::provider_choice(provider, Button::new(SharedString::from(format!("accounts-add-provider-{id}"))).outline().small()
                    .icon(IconName::Plus).label(tr("accounts_add_connect"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.close_dialog(cx);
                        let _ = this.hangar.update(cx, |hangar, cx| { hangar.accounts.add = None; hangar.open_new_key(id, model, window, cx); });
                    })))
            })),
            AddStep::Subscriptions => div().flex().flex_col()
                .child(choice(tr("accounts_add_claude"), tr("accounts_add_claude_desc"),
                    Button::new("accounts-add-pick-claude").outline().small().icon(IconName::Plus).label(tr("accounts_add_connect"))
                        .on_click(cx.listener(|this, _, window, cx| this.go(AddStep::Claude, window, cx)))))
                .child(choice(tr("accounts_add_codex"), tr("accounts_add_codex_desc"),
                    Button::new("accounts-add-pick-codex").outline().small().icon(IconName::Plus).label(tr("accounts_add_connect"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            window.close_dialog(cx);
                            let _ = this.hangar.update(cx, |hangar, cx| { hangar.accounts.add = None; hangar.start_codex(None, false, window, cx); });
                        })))),
            AddStep::Claude => match self.base.clone() {
                Some((id, name)) => div().flex().flex_col().gap(px(14.))
                    .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal()
                        .child(tr("accounts_add_base_signed_out").replace("{name}", &name)))
                    .child(div().flex().justify_end().child(Button::new("accounts-add-base").primary().small()
                        .label(tr("accounts_add_base_enter").replace("{name}", &name))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.close_dialog(cx);
                            let id = id.clone();
                            let _ = this.hangar.update(cx, |hangar, cx| { hangar.accounts.add = None; hangar.start_sign_in(id, window, cx); });
                        })))),
                None => {
                    let hint = if self.slug.is_empty() { tr("accounts_add_name_hint") } else { tr("accounts_add_folder").replace("{slug}", &self.slug) };
                    div().flex().flex_col().gap(px(8.))
                        .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(tr("accounts_add_name")))
                        .child(Input::new(&self.name).disabled(saving).aria_label(tr("accounts_add_name")))
                        .child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(hint))
                        .child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(tr("accounts_add_login_later")))
                        .children(self.error.clone().map(|error| div().text_size(px(13.)).child(div().text_color(theme::danger()).whitespace_normal().child(error))))
                        .child(div().mt(px(6.)).flex().justify_end().child(Button::new("accounts-add-create").primary().small()
                            .label(tr(if saving { "accounts_add_creating" } else { "accounts_add_create" })).disabled(saving || self.slug.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx)))))
                }
            },
        };
        div().flex().flex_col().gap(px(10.))
            .when(self.step != AddStep::Choose, |el| el.child(div().flex().child(back)))
            .child(body)
    }
}

impl Hangar {
    pub(super) fn find_row(&self, id: &str) -> Option<&Row> { self.accounts.sections.iter().flat_map(|s| &s.rows).find(|r| r.id == id) }

    /// Alguma escrita de conta em voo (login, sair, remover, renomear): as outras esperam.
    pub(super) fn accounts_busy(&self) -> bool {
        self.accounts.sign_in.is_some() || self.accounts.change.is_some() || self.accounts.rename.as_ref().is_some_and(|r| r.saving)
            || self.accounts.form.as_ref().is_some_and(EngineForm::busy) || self.accounts.cookie.as_ref().is_some_and(|c| c.saving)
            || self.accounts.cookie_clearing.is_some() || self.accounts.codex.is_some() || self.accounts.reset.as_ref().is_some_and(|r| r.consuming)
    }

    pub(super) fn start_sign_in(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let Some(row) = self.find_row(&id) else { return };
        let (label, name) = (row.label.clone(), row.name.clone());
        let code = cx.new(|cx| InputState::new(window, cx).placeholder(tr("accounts_login_code_placeholder")));
        let subscription = cx.subscribe_in(&code, window, |this: &mut Hangar, input, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let filled = !input.read(cx).value().trim().is_empty();
                if let Some(s) = this.accounts.sign_in.as_mut() { s.has_code = filled; }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.send_code(cx),
            _ => {}
        });
        self.accounts.outcome = None;
        self.accounts.login_attempt += 1;
        let attempt = self.accounts.login_attempt;
        self.accounts.sign_in = Some(SignIn { label: label.clone(), name, attempt, open: false, starting: false, sending: false, stopping: false,
            failed: false, url: None, copied: false, step_failed: false, error: None, done: None, code, has_code: false, poll: None,
            _subscription: subscription, choosing: Some(Choosing::Loading), target: None, browser: None, view: None, view_focus: cx.focus_handle() });
        let Some(api) = self.api.clone() else { return };
        let done = self.accounts_send_later();
        self.runtime.spawn(async move {
            let result = api.server_read(&["conta-estado", &label, "login", "targets"], &[], 8).await;
            done(AccountsReply::Action(ActionReply::Targets(attempt, result))).await;
        });
        cx.notify();
    }

    fn choose_target(&mut self, target: Option<Value>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        (s.choosing, s.target) = (None, target);
        self.begin_login(window, cx);
    }

    /// Começa (ou recomeça) a tentativa: o servidor abre a janela escondida do login.
    fn begin_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.accounts.login_attempt += 1;
        let attempt = self.accounts.login_attempt;
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        if let Some(poll) = s.poll.take() { poll.abort(); }
        (s.attempt, s.starting, s.failed, s.url, s.copied, s.step_failed, s.error, s.done, s.has_code, s.browser, s.view) =
            (attempt, true, false, None, false, false, None, None, false, None, None);
        s.code.update(cx, |input, cx| input.set_value("", window, cx));
        let (label, target) = (s.label.clone(), s.target.clone());
        let (done, turn) = (self.accounts_send_later(), login_turn(&api, &label));
        self.runtime.spawn(async move {
            let _turn = turn.lock().await;
            let path = ["conta-estado", label.as_str(), "login"];
            let result = match target {
                Some(target) => api.server_send(reqwest::Method::POST, &path, Some(json!({"target": target})), 30).await,
                None => api.server_post(&path, 30).await,
            };
            let opened = result.as_ref().map_or_else(|e| e.uncertain, |_| true);
            let (owner, owned) = oneshot::channel();
            done(AccountsReply::Action(ActionReply::Started(attempt, result, owner))).await;
            if opened && !owned.await.unwrap_or(false) { cancel_unseen(&api, &label).await; }
        });
        cx.notify();
    }

    /// Lê o passo agora e a cada 2 s: o link aparece e, se a autorização voltar pelo navegador, o login conclui sozinho.
    fn poll_login(&mut self) {
        let Some(api) = self.api.clone() else { return };
        let done = self.accounts_send_later();
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        let (attempt, label) = (s.attempt, s.label.clone());
        s.poll = Some(self.runtime.spawn(async move {
            loop {
                let result = api.server_read(&["conta-estado", &label, "login", "passo"], &[], 10).await;
                done(AccountsReply::Action(ActionReply::Step(attempt, result))).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }));
    }

    fn send_code(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let Some(s) = self.accounts.sign_in.as_mut()
            .filter(|s| s.open && !s.starting && !s.sending && !s.stopping && !s.failed && s.done.is_none()) else { return };
        let code = s.code.read(cx).value().trim().to_owned();
        if code.is_empty() { return; }
        (s.sending, s.error) = (true, None);
        let (attempt, label) = (s.attempt, s.label.clone());
        let (done, turn) = (self.accounts_send_later(), login_turn(&api, &label));
        // O servidor segura a confirmação até o CLI reler a conta (até 300 s).
        self.runtime.spawn(async move {
            let _turn = turn.lock().await;
            let result = api.server_send(reqwest::Method::POST, &["conta-estado", &label, "login", "codigo"], Some(json!({"codigo": code})), 310).await;
            let entered = matches!(&result, Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true));
            let (owner, owned) = oneshot::channel();
            done(AccountsReply::Action(ActionReply::Code(attempt, result, owner))).await;
            // Sem tela, a resposta do código decide: entrou, nada a fechar; recusa, erro ou incerteza, fecha uma vez.
            if !entered && !owned.await.unwrap_or(false) { cancel_unseen(&api, &label).await; }
        });
        cx.notify();
    }

    /// Cancelar/Voltar (`restart` falso) ou Tentar de novo: fecha a tentativa aberta no servidor antes.
    fn stop_sign_in(&mut self, restart: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        if s.starting || s.sending || s.stopping { return; }
        if !s.open {
            if restart { self.begin_login(window, cx); } else { self.close_sign_in(window, cx); }
            return;
        }
        (s.stopping, s.error) = (true, None);
        let (attempt, label) = (s.attempt, s.label.clone());
        let (done, turn) = (self.accounts_send_later(), login_turn(&api, &label));
        self.runtime.spawn(async move {
            let _turn = turn.lock().await;
            let result = api.server_post(&["conta-estado", &label, "login", "cancelar"], 15).await;
            done(AccountsReply::Action(ActionReply::Stopped(attempt, restart, result))).await
        });
        cx.notify();
    }

    fn close_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.accounts.sign_in = None;
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    /// Só enquanto a tela está aberta: cada quadro passa por duas máquinas.
    fn open_login_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let (frames_done, input_done) = (self.accounts_send_later(), self.accounts_send_later());
        let runtime = self.runtime.clone();
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        let (attempt, label) = (s.attempt, s.label.clone());
        let (input, mut queue) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let (frame_api, frame_label) = (api.clone(), label.clone());
        let frames = runtime.spawn(async move {
            loop {
                let reply = match frame_api.server_read(&["conta-estado", &frame_label, "login", "view"], &[], 10).await {
                    Ok(frame) => view_reply(&frame),
                    Err(_) => ViewReply::Lost,
                };
                frames_done(AccountsReply::Action(ActionReply::View(attempt, reply))).await;
                tokio::time::sleep(VIEW_EVERY).await;
            }
        });
        // Fila única: clique e tecla têm que chegar na ordem em que foram feitos.
        let sender = runtime.spawn(async move {
            while let Some(event) = queue.recv().await {
                let sent = api.server_send(reqwest::Method::POST, &["conta-estado", &label, "login", "view", "input"], Some(event), 10).await;
                if let Err(error) = sent && error.status == Some(409) {
                    input_done(AccountsReply::Action(ActionReply::View(attempt, ViewReply::Notice("accounts_login_view_no_input")))).await;
                }
            }
        });
        s.view = Some(LoginView { frame: None, size: (1., 1.), title: String::new(), can_input: true, notice: None, bounds: Rc::default(),
            last_move: None, input, frames, sender });
        s.view_focus.focus(window, cx);
        cx.notify();
    }

    fn close_login_view(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.accounts.sign_in.as_mut()
            && let Some(mut view) = s.view.take()
            && let Some(frame) = view.frame.take()
        {
            cx.drop_image(frame, None);
        }
        cx.notify();
    }

    fn view_input(&self, event: Value) {
        if let Some(view) = self.accounts.sign_in.as_ref().and_then(|s| s.view.as_ref()) {
            let _ = view.input.send(event);
        }
    }

    /// Coordenada da imagem desenhada, na medida que o servidor deu para a janela.
    fn view_point(&self, point: Point<Pixels>, clamp: bool) -> Option<(f64, f64)> {
        let view = self.accounts.sign_in.as_ref()?.view.as_ref()?;
        let (x, y) = crate::app::remote_browser::fraction(view.bounds.get(), (view.size.0 as f32, view.size.1 as f32), point, clamp)?;
        Some((f64::from(x) * view.size.0, f64::from(y) * view.size.1))
    }

    fn view_mouse(&self, action: &str, point: Point<Pixels>) {
        if let Some((x, y)) = self.view_point(point, action == "up") {
            self.view_input(json!({"t": "mouse", "action": action, "x": x, "y": y}));
        }
    }

    fn activate_login_view(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        let label = s.label.clone();
        if let Some(view) = s.view.as_mut() { view.notice = None; }
        self.runtime.spawn(async move {
            if let Err(error) = api.server_post(&["conta-estado", &label, "login", "view", "activate"], 15).await {
                eprintln!("[contas] reativar a sessão falhou: {:?}", error.status);
            }
        });
        cx.notify();
    }

    fn finish_login(&mut self, email: Option<String>, plan: Option<String>, cx: &mut Context<Self>) {
        let Some(s) = self.accounts.sign_in.as_mut() else { return };
        if let Some(poll) = s.poll.take() { poll.abort(); }
        s.view = None;
        (s.open, s.sending, s.failed, s.error, s.step_failed, s.done) = (false, false, false, None, false, Some((email, plan)));
        // A cota nova só vem com a leitura forçada; a falha dela aparece na tela de conectada.
        self.accounts.notice = None;
        self.load_accounts(true, cx);
    }

    /// A tentativa perdeu a tela (outra página, página fechada, outro servidor): o estado sai. Parada no meio, fecha
    /// aqui, no servidor atual, na vez da conta. Começando ou com o código em voo, a própria tarefa fecha pela resposta.
    pub(in crate::app) fn accounts_page_left(&mut self) {
        // O que estava sendo digitado sai com a página; uma gravação em voo termina sozinha e relê as listas. O painel do
        // Codex para de ler (como o web ao desmontar): a tentativa e a importação seguem no servidor. A redefinição
        // pedida fica com a chave dela: voltar e repetir tem de ser a mesma tentativa, e a resposta ainda acha quem pediu.
        (self.accounts.form, self.accounts.cookie, self.accounts.codex) = (None, None, None);
        let Some(s) = self.accounts.sign_in.take() else { return };
        let Some(api) = self.api.clone().filter(|_| s.open && !s.starting && !s.sending) else { return };
        let (label, turn) = (s.label.clone(), login_turn(&api, &s.label));
        self.runtime.spawn(async move { let _turn = turn.lock().await; cancel_unseen(&api, &label).await; });
    }

    /// Troca de servidor: a tentativa aberta é do servidor que sai, e é nele que se cancela.
    pub(in crate::app) fn leave_accounts(&mut self) { self.accounts_page_left(); self.accounts.reset = None; }

    pub(super) fn confirm_change(&mut self, id: String, kind: ChangeKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let Some(row) = self.find_row(&id) else { return };
        let (title, description, ok) = match kind {
            ChangeKind::SignOut => (tr("accounts_sign_out_title"), tr("accounts_sign_out_desc"), tr("accounts_sign_out_ok")),
            ChangeKind::Remove => (tr("accounts_remove_title"), tr(row.remove.as_ref().map_or("accounts_remove_desc_key", |r| r.2)), tr("accounts_remove_ok")),
        };
        let title = title.replace("{name}", &row.name);
        // Conta Claude ou Codex: as conversas vão para a conta padrão, a menos que a pessoa desmarque.
        let keep = (kind == ChangeKind::Remove && row.remove.as_ref()
            .is_some_and(|r| matches!(r.0.first().map(String::as_str), Some("claude-configs" | "codex-contas"))))
            .then(|| Rc::new(Cell::new(true)));
        let check = keep.clone().map(|keep| (tr_shared("contas_juntar_conversas", &[]), keep));
        let this = cx.entity().downgrade();
        chrome::confirm_alert_with(window, cx, title, description, check, ok, ButtonVariant::Danger, move |_, cx| {
            let keep = keep.as_ref().map(|keep| keep.get());
            let _ = this.update(cx, |this, cx| this.start_change(id.clone(), kind, keep, false, cx));
            true
        });
    }

    /// `release`: o servidor leva as sessões para outra conta e encerra os processos presos antes de apagar.
    fn start_change(&mut self, id: String, kind: ChangeKind, keep: Option<bool>, release: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.accounts_busy() { return; }
        // A linha pode ter sumido entre a pergunta e o sim: nada a fazer.
        let Some(row) = self.find_row(&id) else { return };
        let (path, seconds) = match kind {
            ChangeKind::SignOut => match &row.sign_out { Some(path) => (path.clone(), 60), None => return },
            ChangeKind::Remove => match &row.remove { Some((path, seconds, _)) => (path.clone(), *seconds), None => return },
        };
        let name = row.name.clone();
        self.accounts.outcome = None;
        self.accounts.change = Some(Change { id: id.clone(), kind });
        let done = self.accounts_send_later();
        self.runtime.spawn(async move {
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let result = match kind {
                ChangeKind::SignOut => api.server_post(&path, seconds).await.map(crate::api::Removed::Done),
                ChangeKind::Remove => {
                    let mut query = match keep { Some(true) => vec![("keep_transcripts", "1")],
                        Some(false) => vec![("keep_transcripts", "0")], None => vec![] };
                    if release { query.push(("release", "1")); }
                    api.remove_account(&path, &query, seconds).await
                }
            };
            done(AccountsReply::Action(ActionReply::Changed(id, name, kind, keep, result))).await
        });
        cx.notify();
    }

    pub(super) fn open_add_account(&mut self, window: &mut Window, cx: &mut Context<Self>) { self.open_add_account_at(AddStep::Choose, window, cx); }

    pub(super) fn open_add_account_at(&mut self, step: AddStep, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_busy() { return; }
        let base = self.accounts.list.ok().and_then(|list| list.iter().find(|c| c.kind == "claude" && c.active && c.logged_in() == Some(false)))
            .map(|c| (c.id.clone(), c.name.clone()));
        let hangar = cx.entity().downgrade();
        let add = cx.new(|cx| AddAccount::new(hangar.clone(), step, base, window, cx));
        self.accounts.outcome = None;
        self.accounts.add = Some(add.clone());
        window.open_dialog(cx, move |dialog, _, cx| {
            let (saving, title) = { let add = add.read(cx); (add.saving, add.title()) };
            let (hangar, confirm) = (hangar.clone(), add.clone());
            // Criando: o diálogo não fecha, para a resposta ter onde aparecer. O Enter do diálogo cria a conta em vez
            // de fechá-lo: fechar sem criar parecia ter dado certo.
            popup::dialog(dialog).w(px(540.)).title(title).child(add.clone()).keyboard(!saving).overlay_closable(!saving).close_button(!saving)
                .on_ok(move |_, window, cx| { confirm.update(cx, |add, cx| add.create(window, cx)); false })
                .on_close(move |_, _, cx| { let _ = hangar.update(cx, |this, _| this.accounts.add = None); })
        });
    }

    /// Falso quando não há conexão para mandar o pedido.
    fn create_account(&mut self, slug: String) -> bool {
        let Some(api) = self.api.clone() else { return false };
        let done = self.accounts_send_later();
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::POST, &["claude-configs"], Some(json!({"nome": slug})), 30).await;
            done(AccountsReply::Action(ActionReply::Created(slug, result))).await
        });
        true
    }

    pub(super) fn receive_action(&mut self, reply: ActionReply, window: &mut Window, cx: &mut Context<Self>) {
        let page_open = self.settings == Some(Page::Accounts);
        match reply {
            ActionReply::Targets(attempt, result) => {
                let Some(s) = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt && s.choosing.is_some()) else { return };
                let targets = result.as_ref().ok().and_then(|reply| reply["targets"].as_array().cloned()).unwrap_or_default();
                // Nada para escolher, ou a lista não veio: segue o caminho de sempre, com o link.
                if targets.is_empty() {
                    self.choose_target(None, window, cx);
                    return;
                }
                let last = result.ok().map(|reply| reply["last"].clone()).filter(|last| !last.is_null());
                s.choosing = Some(Choosing::Ready { targets, last });
            }
            ActionReply::View(attempt, reply) => {
                let Some(view) = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt).and_then(|s| s.view.as_mut()) else {
                    if let ViewReply::Frame { image, .. } = reply { cx.drop_image(image, None); }
                    return;
                };
                match reply {
                    ViewReply::Frame { image, size, title, can_input } => {
                        if let Some(old) = view.frame.replace(image) { cx.drop_image(old, None); }
                        (view.size, view.title, view.can_input, view.notice) = (size, title, can_input, None);
                    }
                    ViewReply::Notice(notice) => view.notice = Some(notice),
                    ViewReply::Lost => view.notice = Some("accounts_login_view_lost"),
                }
            }
            ActionReply::Started(attempt, result, owner) => {
                let s = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt && page_open);
                let _ = owner.send(s.is_some());
                let Some(s) = s else { return };
                s.starting = false;
                match result {
                    Ok(_) => { s.open = true; self.poll_login(); }
                    Err(error) if error.uncertain => (s.open, s.failed, s.error) = (true, true, Some(tr("accounts_login_start_uncertain"))),
                    Err(error) => s.error = Some(Self::failure(&error)),
                }
            }
            ActionReply::Step(attempt, result) => {
                if !page_open { self.accounts_page_left(); return; }
                let Some(s) = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt && s.open && !s.failed) else { return };
                match result {
                    Ok(step) if step.get("etapa").and_then(Value::as_str) == Some("concluido") => {
                        let field = |key: &str| step.get(key).and_then(Value::as_str).map(str::to_owned);
                        self.finish_login(field("email"), field("plano"), cx);
                    }
                    Ok(step) => {
                        s.step_failed = false;
                        s.browser = step.get("browser").filter(|browser| !browser.is_null()).cloned();
                        if let Some(url) = step.get("url").and_then(Value::as_str).filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
                            s.url = Some(url.to_owned());
                        }
                    }
                    // O servidor desistiu da tentativa: perguntar de novo só mostraria "aguardando".
                    Err(error) if error.status == Some(409) => {
                        if let Some(poll) = s.poll.take() { poll.abort(); }
                        (s.failed, s.error) = (true, Some(Self::failure(&error)));
                    }
                    Err(_) => s.step_failed = true,
                }
            }
            ActionReply::Code(attempt, result, owner) => {
                let s = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt && s.done.is_none() && page_open);
                let _ = owner.send(s.is_some());
                let Some(s) = s else {
                    // Sem tela: a tarefa fecha a tentativa se precisar; a lista só muda se a conta entrou.
                    if matches!(&result, Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true)) { self.load_accounts(true, cx); }
                    return;
                };
                s.sending = false;
                match result {
                    Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true) => {
                        let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
                        self.finish_login(field("email"), field("plano"), cx);
                    }
                    other => {
                        if let Some(poll) = s.poll.take() { poll.abort(); }
                        s.failed = true;
                        s.error = Some(match other {
                            Err(error) if error.uncertain => tr("accounts_login_uncertain"),
                            Err(error) => Self::failure(&error),
                            Ok(_) => tr("accounts_login_not_confirmed"),
                        });
                        // A recusa não prova que a conta ficou de fora: a lista relida mostra como o servidor a vê.
                        self.load_accounts(false, cx);
                    }
                }
            }
            ActionReply::Stopped(attempt, restart, result) => {
                let Some(s) = self.accounts.sign_in.as_mut().filter(|s| s.attempt == attempt) else { return };
                s.stopping = false;
                match result {
                    Ok(_) => {
                        if let Some(poll) = s.poll.take() { poll.abort(); }
                        s.open = false;
                        if restart { self.begin_login(window, cx); } else { self.close_sign_in(window, cx); }
                    }
                    // Sem confirmação de que fechou, a tentativa continua valendo e os botões continuam lá.
                    Err(error) => s.error = Some(if error.uncertain { tr("accounts_login_stop_uncertain") } else { Self::failure(&error) }),
                }
            }
            ActionReply::Changed(id, name, kind, keep, result) => {
                self.accounts.change = None;
                let result = match result {
                    // A conta está presa: pergunta se libera, e o sim repete o pedido com `release=1`.
                    Ok(crate::api::Removed::Held(holders)) => {
                        let (title, lines) = super::release_prompt(&name, &holders);
                        let this = cx.entity().downgrade();
                        chrome::confirm_alert(window, cx, title, lines.join("\n"), tr_shared("contas_liberar_confirmar", &[]), ButtonVariant::Danger,
                            move |_, cx| { let _ = this.update(cx, |this, cx| this.start_change(id.clone(), kind, keep, true, cx)); true });
                        cx.notify();
                        return;
                    }
                    Ok(crate::api::Removed::Done(body)) => Ok(body),
                    Err(error) => Err(error),
                };
                let keep = keep == Some(true);
                let ok = result.is_ok();
                let kept = result.as_ref().ok().filter(|_| kind == ChangeKind::Remove)
                    .and_then(|body| super::kept_transcripts_notice(&name, keep, body));
                self.accounts.outcome = Some(match result {
                    Ok(_) if kept.is_some() => (kept.unwrap_or_default(), false),
                    Ok(_) => (tr(if kind == ChangeKind::SignOut { "accounts_signed_out" } else { "accounts_removed" }).replace("{name}", &name), false),
                    Err(error) if error.uncertain => (tr("accounts_change_uncertain").replace("{name}", &name), true),
                    Err(error) => (tr(if kind == ChangeKind::SignOut { "accounts_sign_out_failed" } else { "accounts_remove_failed" })
                        .replace("{name}", &name).replace("{reason}", &Self::failure(&error)), true),
                });
                // O servidor já confirmou: a linha muda agora, e a leitura seguinte traz o resto (a cota, os modelos).
                if ok && let Some(Ok(list)) = self.accounts.list.value.as_mut() {
                    match kind {
                        ChangeKind::SignOut => for c in list.iter_mut().filter(|c| c.id == id) {
                            c.login = Some(Login { state: "ok".into(), logged_in: Some(false), email: None, plan: None, reason: None, refresh_expires_at: None });
                            c.quota = None;
                            if c.kind == "codex" { c.auth_method = Some("none".into()); }
                        },
                        ChangeKind::Remove => list.retain(|c| c.id != id),
                    }
                }
                self.load_accounts(ok && kind == ChangeKind::SignOut, cx);
                if kind == ChangeKind::Remove { self.load_engines(cx); }
            }
            ActionReply::Created(slug, result) => {
                let add = self.accounts.add.clone();
                match (result, add) {
                    (Ok(_), add) => {
                        if add.is_some() { self.accounts.add = None; window.close_dialog(cx); }
                        self.accounts.outcome = Some((tr("accounts_created").replace("{name}", &slug), false));
                    }
                    (Err(error), Some(add)) => add.update(cx, |add, cx| {
                        add.saving = false;
                        add.error = Some(if error.uncertain { tr("accounts_create_uncertain") } else { Self::failure(&error) });
                        cx.notify();
                    }),
                    (Err(error), None) => self.accounts.outcome = Some((if error.uncertain { tr("accounts_create_uncertain") }
                        else { Self::failure(&error) }, true)),
                }
                self.load_accounts(false, cx);
            }
        }
    }

    fn render_login_view(&self, view: &LoginView, machine: &str, cx: &mut Context<Self>) -> Div {
        let Some(s) = self.accounts.sign_in.as_ref() else { return div() };
        let note = |text: String, color: Hsla| div().size_full().flex().items_center().justify_center().p_4()
            .text_size(px(12.)).text_color(color).text_center().whitespace_normal().child(text);
        let bounds = view.bounds.clone();
        let page = div().id("accounts-login-view").relative().w_full().h(px(460.)).overflow_hidden().rounded(px(8.)).bg(theme::background())
            .border_1().border_color(theme::border())
            .track_focus(&s.view_focus).role(Role::Application).aria_label(tr("accounts_login_view_open"))
            .child(canvas(|_, _, _| {}, move |b, _, _, _| bounds.set(b)).absolute().inset_0())
            .map(|el| match &view.frame {
                Some(image) => el.child(img(image.clone()).absolute().inset_0().size_full().object_fit(ObjectFit::Contain)),
                None => el.child(note(tr(view.notice.unwrap_or("accounts_login_view_waiting")), theme::muted())),
            })
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                if let Some(s) = this.accounts.sign_in.as_ref() { s.view_focus.focus(window, cx); }
                this.view_mouse("down", e.position);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, _| this.view_mouse("up", e.position)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, _| this.view_mouse("up", e.position)))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, _| {
                let Some(view) = this.accounts.sign_in.as_mut().and_then(|s| s.view.as_mut()) else { return };
                if view.last_move.is_some_and(|at| at.elapsed() < VIEW_MOVE_EVERY) { return; }
                view.last_move = Some(Instant::now());
                this.view_mouse("move", e.position);
            }))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, _| {
                let Some((x, y)) = this.view_point(e.position, false) else { return };
                let delta = e.delta.pixel_delta(px(17.78));
                if f32::from(delta.y) != 0. { this.view_input(json!({"t": "wheel", "x": x, "y": y, "down": f32::from(delta.y) < 0.})); }
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let keystroke = &e.keystroke;
                cx.stop_propagation();
                if keystroke.modifiers.control && !keystroke.modifiers.alt && keystroke.key == "v" {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) { this.view_input(json!({"t": "text", "text": text})); }
                    return;
                }
                // Sem atalhos: só digitação e as teclas de navegar.
                if keystroke.modifiers.control || keystroke.modifiers.alt || keystroke.modifiers.platform { return; }
                if let Some(key) = view_key(&keystroke.key) {
                    this.view_input(json!({"t": "key", "key": key}));
                } else if let Some(text) = keystroke.key_char.clone().filter(|text| !text.is_empty()) {
                    this.view_input(json!({"t": "text", "text": text}));
                }
            }));
        let session_down = !view.can_input || view.notice == Some("accounts_login_view_no_input");
        div().flex().flex_col().gap(px(8.))
            .child(div().flex().items_center().gap(px(8.))
                .child(Button::new("accounts-login-view-back").outline().xsmall().icon(IconName::ArrowLeft).label(tr("accounts_login_view_back"))
                    .on_click(cx.listener(|this, _, _, _| this.view_input(json!({"t": "back"})))))
                .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(view.title.clone()))
                .child(Button::new("accounts-login-view-close").ghost().xsmall().label(tr("accounts_login_view_close"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_login_view(cx)))))
            .when(session_down, |el| el.child(div().flex().items_center().gap(px(8.))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).text_size(px(12.)).whitespace_normal()
                    .child(div().text_color(theme::warning()).child(tr("accounts_login_view_no_input").replace("{machine}", machine)))
                    .child(div().text_color(theme::muted()).child(tr("accounts_login_view_activate_help"))))
                .child(Button::new("accounts-login-view-activate").outline().xsmall().label(tr("accounts_login_view_activate"))
                    .on_click(cx.listener(|this, _, _, cx| this.activate_login_view(cx))))))
            .when(!session_down && view.frame.is_some(), |el| el.children(view.notice.map(|notice| div().text_size(px(12.)).text_color(theme::warning()).child(tr(notice)))))
            .child(page)
    }

    /// A página no lugar da lista enquanto há uma tentativa de login: os passos do web, um por vez.
    pub(super) fn render_sign_in(&self, cx: &mut Context<Self>) -> Option<Div> {
        let s = self.accounts.sign_in.as_ref()?;
        let (color, _) = theme::provider("claude");
        let avatar = div().size(px(36.)).flex_shrink_0().rounded(px(10.)).bg(color.opacity(0.16)).flex().items_center().justify_center()
            .children(chrome::provider_logo("claude", 20., color));
        let title = if s.done.is_some() { tr("accounts_login_connected") } else { tr("accounts_login_title").replace("{name}", &s.name) };
        let head = div().flex().items_center().gap(px(12.)).child(avatar)
            .child(div().flex().flex_col().gap(px(2.)).min_w_0()
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("accounts_login_server").replace("{server}", &self.server_label(cx)))));
        let line = |text: String, color: Hsla| div().text_size(px(13.)).whitespace_normal().child(div().text_color(color).child(text));
        let footer = || div().mt(px(4.)).flex().justify_end().gap(px(8.));
        let mut panel = settings_box().mt(px(24.)).p(px(20.)).gap(px(16.)).child(head);
        if let Some(choosing) = &s.choosing {
            panel = panel.child(div().flex().flex_col().gap(px(4.))
                .child(div().font_weight(FontWeight::MEDIUM).text_size(px(13.)).child(tr("accounts_login_where_title")))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr("accounts_login_where_help"))));
            match choosing {
                Choosing::Loading => panel = panel.child(line(tr("accounts_login_where_loading"), theme::muted())),
                Choosing::Ready { targets, last } => {
                    let same = |a: &Value, b: &Value| a["machine"] == b["machine"] && a["kind"] == b["kind"] && a["profile"] == b["profile"];
                    let mut list = div().flex().flex_col().gap(px(6.));
                    for (index, target) in targets.iter().enumerate() {
                        let is_last = last.as_ref().is_some_and(|last| same(last, target));
                        let chosen = target.clone();
                        list = list.child(Button::new(("accounts-login-target", index)).w_full().small().label(target_label(target, is_last))
                            .map(|button| if is_last { button.primary() } else { button.outline() })
                            .on_click(cx.listener(move |this, _, window, cx| this.choose_target(Some(chosen.clone()), window, cx))));
                    }
                    panel = panel.child(list.child(Button::new("accounts-login-link-only").ghost().w_full().small().label(tr("accounts_login_where_link"))
                        .on_click(cx.listener(|this, _, window, cx| this.choose_target(None, window, cx)))));
                }
            }
            return Some(panel.child(footer().child(Button::new("accounts-login-where-back").outline().small().label(tr("settings_back"))
                .on_click(cx.listener(|this, _, window, cx| this.close_sign_in(window, cx))))));
        }
        if let Some((email, plan)) = &s.done {
            let fact = |label: &'static str, value: &Option<String>| value.clone().map(|value| div().flex().gap(px(12.)).text_size(px(13.))
                .child(div().w(px(72.)).text_color(theme::muted()).child(tr(label))).child(div().child(value)));
            panel = panel.child(div().flex().items_center().gap(px(8.)).text_size(px(13.))
                    .child(chrome::small_icon(IconName::Check, 15., theme::success()))
                    .child(div().child(tr("accounts_login_ready").replace("{name}", &s.name))))
                .children(fact("accounts_login_email", email)).children(fact("accounts_login_plan", plan))
                .children(self.accounts.notice.as_ref().map(|_| line(tr("accounts_login_refresh_failed"), theme::danger())))
                .child(footer().child(Button::new("accounts-login-done").primary().small().label(tr("accounts_login_done"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_sign_in(window, cx)))));
            return Some(panel);
        }
        panel = panel.children(s.error.clone().map(|error| line(error, theme::danger())))
            .when(s.step_failed, |el| el.child(line(tr("accounts_login_step_failed"), theme::danger())));
        let waiting = s.open && !s.failed;
        let locked = s.sending || s.stopping;
        if waiting && let Some(browser) = &s.browser {
            let machine = browser["machine"].as_str().unwrap_or_default();
            let viewable = s.target.as_ref().is_some_and(|target| target["kind"] == "chrome_profile");
            match browser["state"].as_str() {
                Some("error") => panel = panel.child(line(
                    tr("accounts_login_browser_error").replace("{machine}", machine).replace("{reason}", &tr(browser_reason(browser["reason"].as_str()))),
                    theme::danger(),
                )),
                Some("open") => {
                    panel = panel.child(line(tr("accounts_login_browser_open").replace("{machine}", machine), theme::text()));
                    if let Some(view) = &s.view {
                        panel = panel.child(self.render_login_view(view, machine, cx));
                    } else if viewable {
                        panel = panel.child(div().flex().child(Button::new("accounts-login-view-open").outline().small().icon(IconName::Monitor)
                            .label(tr("accounts_login_view_open"))
                            .on_click(cx.listener(|this, _, window, cx| this.open_login_view(window, cx)))));
                    }
                }
                _ => panel = panel.child(line(tr("accounts_login_browser_opening").replace("{machine}", machine), theme::muted())),
            }
        }
        if s.starting || (waiting && s.url.is_none()) {
            panel = panel.child(line(tr("accounts_login_preparing"), theme::muted()));
        } else if let (true, Some(url)) = (waiting, s.url.clone()) {
            let number = |n: &'static str| div().size(px(22.)).flex_shrink_0().rounded_full().border_1().border_color(theme::border_strong())
                .flex().items_center().justify_center().text_size(px(12.)).text_color(theme::muted()).child(n);
            let step = |n: &'static str, title: &'static str, help: &'static str, body: AnyElement| div().flex().gap(px(12.)).child(number(n))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(6.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(tr(title)))
                    .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr(help)))
                    .child(body));
            let open_url = url.clone();
            let links = div().flex().flex_col().gap(px(6.))
                .child(div().flex().gap(px(8.))
                    .child(Button::new("accounts-login-open").primary().small().icon(IconName::ExternalLink).label(tr("accounts_login_open")).disabled(locked)
                        .on_click(move |_, _, cx| cx.open_url(&open_url)))
                    .child(Button::new("accounts-login-copy").outline().small().icon(IconName::Copy)
                        .label(tr(if s.copied { "accounts_login_copied" } else { "accounts_login_copy" })).disabled(locked)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                            if let Some(s) = this.accounts.sign_in.as_mut() { s.copied = true; }
                            cx.notify();
                        }))))
                .child(div().min_w_0().truncate().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::faint())
                    .child(s.url.clone().unwrap_or_default()));
            let code = Input::new(&s.code).disabled(locked).aria_label(tr("accounts_login_code"));
            panel = panel.child(step("1", "accounts_login_authorize", "accounts_login_authorize_help", links.into_any_element()))
                .child(step("2", "accounts_login_code", "accounts_login_code_help", code.into_any_element()))
                .when(s.sending, |el| el.child(line(tr("accounts_login_confirming"), theme::muted())))
                .child(footer()
                    .child(Button::new("accounts-login-cancel").outline().small().label(tr("cancel")).disabled(locked)
                        .on_click(cx.listener(|this, _, window, cx| this.stop_sign_in(false, window, cx))))
                    .child(Button::new("accounts-login-confirm").primary().small()
                        .label(tr(if s.sending { "accounts_login_confirming_short" } else { "accounts_login_confirm" })).disabled(locked || !s.has_code)
                        .on_click(cx.listener(|this, _, _, cx| this.send_code(cx)))));
            return Some(panel);
        }
        let stuck = s.starting || s.stopping;
        Some(panel.child(footer()
            .child(Button::new("accounts-login-back").outline().small().label(tr("settings_back")).disabled(stuck)
                .on_click(cx.listener(|this, _, window, cx| this.stop_sign_in(false, window, cx))))
            .when(!s.starting && (!s.open || s.failed), |el| el.child(Button::new("accounts-login-retry").primary().small()
                .label(tr(if s.stopping { "accounts_login_stopping" } else { "accounts_retry" })).disabled(s.stopping)
                .on_click(cx.listener(|this, _, window, cx| this.stop_sign_in(true, window, cx)))))))
    }
}

fn target_label(target: &Value, is_last: bool) -> String {
    let machine = target["machine"].as_str().unwrap_or_default();
    let name = match target["kind"].as_str() {
        Some("chrome_profile") => {
            let label = target["label"].as_str().unwrap_or_default();
            target["email"].as_str().map_or_else(|| label.to_owned(), |email| format!("{label} ({email})"))
        }
        _ => tr("accounts_login_where_default"),
    };
    if is_last { tr("accounts_login_where_last").replace("{machine}", machine).replace("{name}", &name) } else { format!("{machine}: {name}") }
}

fn browser_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("erro_login_machine_unreachable") => "accounts_login_reason_unreachable",
        Some("erro_login_port_busy") => "accounts_login_reason_port",
        Some("erro_login_target_missing") => "accounts_login_reason_target",
        _ => "accounts_login_reason_other",
    }
}

fn view_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "enter" => "Enter",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "escape" => "Escape",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "home" => "Home",
        "end" => "End",
        _ => return None,
    })
}

fn view_reply(frame: &Value) -> ViewReply {
    if frame["available"] != true { return ViewReply::Notice("accounts_login_view_unavailable"); }
    if frame["closed"] == true { return ViewReply::Notice("accounts_login_view_closed"); }
    if frame["pending"] == true { return ViewReply::Notice("accounts_login_view_waiting"); }
    let image = frame["d"].as_str()
        .and_then(|data| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).ok())
        .and_then(|bytes| crate::media::decode(&bytes, 4096, 4096, None));
    let Some(image) = image else { return ViewReply::Notice("accounts_login_view_capture") };
    ViewReply::Frame {
        image,
        size: (frame["w"].as_f64().unwrap_or(1.), frame["h"].as_f64().unwrap_or(1.)),
        title: frame["title"].as_str().unwrap_or_default().to_owned(),
        can_input: frame["input"].as_bool().unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::account_slug;
    use super::{view_key, view_reply, ViewReply};
    use serde_json::json;

    #[test]
    fn view_reply_says_why_there_is_no_frame() {
        let notice = |frame| match view_reply(&frame) { ViewReply::Notice(notice) => notice, _ => "outro" };
        assert_eq!(notice(json!({"available": false})), "accounts_login_view_unavailable");
        assert_eq!(notice(json!({"available": true, "closed": true})), "accounts_login_view_closed");
        assert_eq!(notice(json!({"available": true, "pending": true})), "accounts_login_view_waiting");
        assert_eq!(notice(json!({"available": true, "error": "x"})), "accounts_login_view_capture");
    }

    #[test]
    fn only_navigation_keys_go_by_name() {
        assert_eq!(view_key("enter"), Some("Enter"));
        assert_eq!(view_key("a"), None);
        assert_eq!(view_key("f4"), None);
    }

    #[test]
    fn slug_is_what_the_server_accepts() {
        assert_eq!(account_slug("  Trabalho Ção 2 "), "trabalho-cao-2");
        assert_eq!(account_slug("--Nova//conta--"), "nova-conta");
        assert_eq!(account_slug("é"), "e");
        assert_eq!(account_slug("!!!"), "");
        assert_eq!(account_slug(&"a".repeat(40)).len(), 32);
    }
}
