use std::{collections::{HashMap, HashSet}, path::PathBuf, sync::Arc, time::{Duration, Instant}};
use gpui_kit::{component::{button::*, checkbox::Checkbox, radio::Radio, scroll::{Scrollbar, ScrollbarMode}, menu::{ContextMenuExt, PopupMenuItem},
    input::{Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Textarea, TextareaState}, text::{TextView, TextViewState}, *}, *};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::assets::IconName;
use tokio::{runtime::Runtime, task::JoinHandle};
use crate::{api::{self, Api, Failure, Source, dto::*, sse::Update}, chat::{Chat, LiveTool}, composer,
    conversation::{self, Item, Tool}, delivery::{DeliveryTracker, SendOutcome, SessionKey}, i18n::tr, theme,
    interaction::{self, Action, Ask, InFlight, Pick}, media::{self, MediaCache, MediaState}, appearance};
use gpui_kit::component::notification::Notification;
use serde_json::{Value, json};

mod backdrop;
mod chrome;
mod controls;
mod follow;
mod settings;
mod side;

actions!(hangar, [FocusComposer, OpenSettings, CopyLastReply]);

const LIVE_THINKING: &str = "__thinking__";
const LIVE_TOOL: &str = "__tool__";
const PREVIEW: &str = "__preview__";
const COLUMN: f32 = 780.;

/// Largura da coluna da conversa e do compositor: a do mock vezes o ajuste de Aparência.
fn column_width() -> f32 { COLUMN * crate::appearance::get().column as f32 / 100. }

/// Põe uma faixa na coluna da conversa: mesma margem e mesma largura das mensagens e do compositor.
fn in_column(el: impl IntoElement) -> Div {
    div().w_full().flex_shrink_0().px(px(36.)).flex().justify_center().child(div().w_full().max_w(px(column_width())).child(el))
}

/// Texto da conversa com a fonte, o tamanho e a entrelinha escolhidos em Aparência.
fn conversation_text(el: Div) -> Div {
    let a = crate::appearance::get();
    el.font_family(if a.font == crate::appearance::Font::Mono { theme::MONO } else { theme::SANS })
        .text_size(px(14. * a.text_size as f32 / 100.)).line_height(relative(1.45 * a.line_height as f32 / 100.))
}
const DETAIL_MAX: usize = 20_000;
const LIVE_THINKING_TAIL: usize = 1_500;

enum Payload {
    Sessions(Result<Vec<SessionInfo>, Failure>),
    Stream(Update),
    History(u64, usize, Result<api::History, Failure>),
    // Mensagem entregue e o texto do campo que a originou (com anexo os dois diferem).
    Sent(SessionKey, String, String, Result<Delivery, Failure>),
    Interrupted(SessionKey, Result<(), Failure>),
    Acted(SessionKey, Action, Result<serde_json::Value, Failure>),
    Files(SessionKey, Vec<Result<Picked, String>>),
    // `None` marca o início do envio daquele anexo.
    UploadStep(SessionKey, u64, Option<Result<Uploaded, Failure>>),
    UploadsDone(SessionKey, String, bool, HashSet<String>),
    Commands(String, Result<Vec<CommandInfo>, Failure>),
    Recent(SessionKey, Result<Vec<UploadFile>, Failure>),
    // Miniatura já decodificada fora da thread da janela; `None` = bytes que não são imagem legível.
    Media(SessionKey, Source, Result<Option<Arc<RenderImage>>, Failure>),
    Saved(SessionKey, bool, Result<PathBuf, String>),
    ConnectionNotSaved(String),
    AppearanceSaved(Result<(), String>),
    // Paleta do papel de parede pedida à conexão atual (`GET /api/desktop/palette`), com o número do pedido.
    DesktopPalette(u64, Result<Value, Failure>),
    // Imagem do fundo já decodificada, com o número do pedido; `None` = a mesma foto que já está na tela.
    Backdrop(u64, Result<Option<(u64, Arc<RenderImage>)>, Failure>),
    // Arquivo escolhido para o fundo Imagem, já validado e copiado.
    BackdropPicked(Result<Arc<RenderImage>, Failure>),
    // Cópia da imagem de fundo apagada (ou o erro do disco).
    BackdropRemoved(Result<(), String>),
    // Resposta amarrada à sessão e ao pedido capturados no gesto, não ao que está na tela na volta.
    Reply(SessionKey, Reply, Result<Value, Failure>),
    Config(Result<Value, Failure>),
    HeadlessPlan(SessionKey, controls::PlanOutcome),
}

#[derive(Clone, Debug, PartialEq)]
enum Reply {
    Catalog(controls::Ctl),
    // Valor aplicado e o que a fonte ao vivo mostrava no gesto.
    Applied(controls::Ctl, String, Option<String>),
    Cost(u64),
    GitFiles,
    Diff(String),
    Shell(String),
    Reload,
    PlanPreview(bool),
    PreSelect(String),
}

pub struct Picked { name: String, bytes: Vec<u8> }

#[derive(Clone)]
struct Attachment { id: u64, name: String, bytes: Arc<Vec<u8>>, image: Option<Arc<Image>>, state: AttachState }

#[derive(Clone, PartialEq)]
enum AttachState { Waiting, Uploading, Uploaded(Uploaded), Failed(String) }

#[derive(Clone, PartialEq)]
enum Confirm { Stop, Destructive(String), Replace(String), Prefill(String), Shortcut(String, side::Shortcut), Reload }

struct Recent { key: SessionKey, files: Option<Result<Vec<UploadFile>, String>> }

struct Envelope { connection: u64, selection: Option<u64>, payload: Payload }
struct RichText { source: String, view: Entity<TextViewState>, _observer: Subscription, touched: u64, row: String }
enum ChatUpdate { Message(ChatEvent), Preview(Preview), State(SessionState), Question(Option<Ask>), Thinking(String), LiveTool(Option<LiveTool>), Reset }

// Formulário da pergunta atual; refeito quando a pergunta (identidade + conteúdo) muda.
#[derive(Default)]
struct AskForm { fingerprint: String, picks: Vec<Pick>, typing: Vec<bool>, inputs: Vec<Entity<InputState>>, _changes: Vec<Subscription> }

#[derive(Clone, Copy)]
enum Live { Thinking, Tool }

pub struct Hangar {
    runtime: Arc<Runtime>,
    tx: async_channel::Sender<Envelope>,
    api: Option<Api>,
    server: Option<String>,
    connection: u64,
    selection: u64,
    revision: u64,
    sessions: Vec<SessionInfo>,
    selected: Option<SessionInfo>,
    chat: Chat,
    list_task: Option<JoinHandle<()>>,
    session_task: Option<JoinHandle<()>>,
    history_task: Option<JoinHandle<()>>,
    address: Entity<InputState>,
    token: Entity<InputState>,
    // Endereço e token desta conexão, gravados só quando o servidor aceitar.
    unsaved_connection: Option<(String, String)>,
    connection_focus: FocusHandle,
    root_focus: FocusHandle,
    composer: Entity<TextareaState>,
    composer_placeholder: String,
    _input_subscription: Subscription,
    connection_dialog: bool,
    list_online: bool,
    chat_online: bool,
    loading: bool,
    history_started: bool,
    history_installed: bool,
    pending_chat: Vec<ChatUpdate>,
    history_limit: usize,
    has_older: bool,
    etag: Option<String>,
    error: Option<String>,
    list_error: Option<String>,
    delivery: DeliveryTracker,
    stopping: HashSet<SessionKey>,
    stop_feedback: HashMap<SessionKey, (String, bool)>,
    drafts: HashMap<SessionKey, String>,
    flight: InFlight,
    action_feedback: HashMap<SessionKey, (String, bool)>,
    ask_form: AskForm,
    plans_dismissed: HashSet<String>,
    // Pergunta do transcript aceita pelo backend, por sessão: vale até o `tool_result`, mesmo trocando de seleção.
    answered_tools: HashSet<(SessionKey, String)>,
    // Id da ferramenta capturado no clique; a resposta HTTP usa este, não a pergunta aberta na volta.
    answering: HashMap<SessionKey, String>,
    // Rolagem por identidade: pergunta ou plano novo volta ao topo; retrato idêntico mantém a posição.
    ask_scroll: (String, ScrollHandle),
    plan_scroll: (String, ScrollHandle),
    plan_view: Option<(String, Entity<TextViewState>)>,
    list_state: ListState,
    follow: follow::Follow,
    row_ids: Vec<String>,
    row_signatures: Vec<String>,
    items: Vec<Item>,
    expanded: HashSet<String>,
    last_message: Option<usize>,
    live_clear_epoch: [u64; 2],
    rich: HashMap<String, RichText>,
    render_tick: u64,
    preview_drop_epoch: u64,
    preview_drop_scheduled: bool,
    visible_preview: Preview,
    preview_tick_epoch: u64,
    preview_tick_scheduled: bool,
    preview_last_tick: Option<Instant>,
    preview_deadline: Option<Instant>,
    // Anexos e rascunho têm a mesma identidade (servidor + sessão + transcript): trocar de sessão não mistura.
    attachments: HashMap<SessionKey, Vec<Attachment>>,
    attach_seq: u64,
    uploading: HashMap<SessionKey, Vec<u64>>,
    commands: HashMap<String, Result<Vec<CommandInfo>, String>>,
    suggest_pick: usize,
    suggest_dismissed: Option<String>,
    command_panel: bool,
    command_search: Entity<InputState>,
    confirm: Option<Confirm>,
    terminal_suggestion: String,
    recent: Option<Recent>,
    media: MediaCache<(SessionKey, Source)>,
    stats: Option<Stats>,
    side: side::Side,
    controls: controls::Controls,
    // Página de configurações aberta por cima da janela inteira; `None` é a janela da conversa.
    settings: Option<settings::Page>,
    settings_ui: settings::SettingsUi,
    appearance_note: Option<String>,
    // Por que o tema Desktop não está pintando com o papel de parede; `None` quando pinta ou não foi escolhido.
    desktop_note: Option<String>,
    // Pedidos numerados: resposta de pedido anterior ao último é descartada.
    palette_seq: u64,
    backdrop_seq: u64,
    // Imagem do fundo na tela (arquivo escolhido ou papel de parede) e a assinatura dos bytes dela.
    backdrop: Option<(u64, Arc<RenderImage>)>,
    // Por que o fundo não desenha a imagem escolhida.
    backdrop_note: Option<String>,
    backdrop_busy: Option<backdrop::BackdropBusy>,
    grain: Arc<RenderImage>,
}

impl Drop for Hangar {
    fn drop(&mut self) {
        for task in [&self.list_task, &self.session_task, &self.history_task].into_iter().flatten() { task.abort(); }
    }
}

impl Hangar {
    pub fn new(runtime: Arc<Runtime>, appearance_error: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::watch_system(window, cx);
        let saved = load_connection();
        let (saved_address, saved_token) = saved.clone().unwrap_or_else(|| ("http://127.0.0.1:8765".into(), String::new()));
        let address = cx.new(|cx| InputState::new(window, cx).default_value(saved_address).placeholder(tr("server")));
        let token = cx.new(|cx| InputState::new(window, cx).masked(true).default_value(saved_token).placeholder(tr("token")));
        if saved.is_some() { cx.defer_in(window, |this: &mut Self, window, cx| this.connect(window, cx)); }
        // A imagem escolhida abre sem depender de conexão; o papel de parede em Vidro vem ao conectar.
        cx.defer_in(window, |this: &mut Self, window, cx| this.refresh_backdrop(window, cx));
        let connection_focus = cx.focus_handle();
        address.update(cx, |input, cx| input.focus(window, cx));
        let composer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 10).submit_on_enter(true));
        let input_subscription = cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
            match event {
                InputEvent::PressEnter { secondary: false, shift: false } if !this.connection_dialog => this.submit(false, false, window, cx),
                // A lista de comandos acompanha o que se digita.
                InputEvent::Change => { this.suggest_pick = 0; cx.notify(); }
                _ => {}
            }
        });
        // Ctrl+L leva ao campo de mensagem; a raiz da janela trata a ação e segura o foco quando nada mais o tem.
        cx.bind_keys([KeyBinding::new("ctrl-l", FocusComposer, None), KeyBinding::new("ctrl-,", OpenSettings, None),
            KeyBinding::new("ctrl-shift-c", CopyLastReply, None)]);
        let settings_ui = settings::SettingsUi::new(window, cx);
        let root_focus = cx.focus_handle();
        let command_search = cx.new(|cx| InputState::new(window, cx).placeholder(tr("commands_search")));
        cx.subscribe(&command_search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        let (tx, rx) = async_channel::bounded::<Envelope>(256);
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(envelope) = rx.recv().await {
                if this.update_in(cx, |this, window, cx| this.receive(envelope, window, cx)).is_err() { break; }
            }
        }).detach();
        // Só para medir sem mouse: HANGAR_NATIVE_CYCLE_SECS seleciona as sessões em rodízio. Sem a variável, nada roda.
        if let Some(every) = std::env::var("HANGAR_NATIVE_CYCLE_SECS").ok().and_then(|s| s.parse::<f64>().ok()).filter(|s| s.is_finite() && *s > 0.) {
            cx.spawn_in(window, async move |this, cx| {
                for turn in 0usize.. {
                    cx.background_executor().timer(Duration::from_secs_f64(every)).await;
                    let alive = this.update_in(cx, |this, window, cx| {
                        let Some(next) = (!this.sessions.is_empty()).then(|| this.sessions[turn % this.sessions.len()].clone()) else { return };
                        eprintln!("cycle {turn} {}", next.name);
                        this.select(next, window, cx);
                    });
                    if alive.is_err() { break; }
                }
            }).detach();
        }
        let list_state = ListState::new(0, ListAlignment::Bottom, px(300.));
        Self::watch_user_scroll(&list_state, cx);
        Self {
            runtime, tx, api: None, server: None, connection: 0, selection: 0, revision: 0, sessions: Vec::new(), selected: None,
            chat: Chat::default(), list_task: None, session_task: None, history_task: None,
            address, token, unsaved_connection: None, connection_focus, root_focus, composer, composer_placeholder: String::new(), _input_subscription: input_subscription,
            connection_dialog: true, list_online: false, chat_online: false, loading: false, history_started: false,
            history_installed: false, pending_chat: Vec::new(),
            history_limit: 400, has_older: false, etag: None, error: None, list_error: None,
            delivery: DeliveryTracker::default(), stopping: HashSet::new(), stop_feedback: HashMap::new(), drafts: HashMap::new(),
            flight: InFlight::default(), action_feedback: HashMap::new(), ask_form: AskForm::default(), plans_dismissed: HashSet::new(), answered_tools: HashSet::new(), answering: HashMap::new(), ask_scroll: Default::default(), plan_scroll: Default::default(), plan_view: None,
            list_state, follow: Default::default(), row_ids: Vec::new(), row_signatures: Vec::new(), items: Vec::new(), expanded: HashSet::new(),
            last_message: None, live_clear_epoch: [0; 2], rich: HashMap::new(), render_tick: 0,
            preview_drop_epoch: 0, preview_drop_scheduled: false,
            visible_preview: Preview::default(), preview_tick_epoch: 0, preview_tick_scheduled: false,
            preview_last_tick: None, preview_deadline: None,
            attachments: HashMap::new(), attach_seq: 0, uploading: HashMap::new(), commands: HashMap::new(),
            suggest_pick: 0, suggest_dismissed: None, command_panel: false, command_search, confirm: None,
            terminal_suggestion: String::new(), recent: None, media: MediaCache::new(), stats: None,
            side: side::Side::default(), controls: controls::Controls::default(),
            settings: None, settings_ui,
            appearance_note: appearance_error.map(|error| tr("settings_not_loaded").replace("{error}", &error)),
            desktop_note: None,
            palette_seq: 0, backdrop_seq: 0, backdrop: None, backdrop_note: None, backdrop_busy: None, grain: crate::media::grain(),
        }
    }

    /// Automático acompanha a preferência do sistema; o Desktop relê a paleta quando a janela volta ao foco.
    fn watch_system(window: &mut Window, cx: &mut Context<Self>) {
        theme::set_system_dark(matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark));
        theme::sync_kit(Some(window), cx);
        cx.observe_window_appearance(window, |this, window, cx| {
            theme::set_system_dark(matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark));
            theme::sync_kit(Some(window), cx);
            this.sync_sliders(window, cx);
            cx.notify();
        }).detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() { return; }
            this.refresh_desktop_palette(cx);
            // O papel de parede muda fora da janela; a volta do foco é quando repintar importa.
            let a = appearance::get();
            if a.background == appearance::Background::Desktop && a.wallpaper == appearance::Wallpaper::Glass { this.refresh_backdrop(window, cx); }
        }).detach();
    }

    /// Pede a paleta do papel de parede quando o tema é Desktop. Sem conexão, diz isso e desenha como Automático.
    pub(super) fn refresh_desktop_palette(&mut self, cx: &mut Context<Self>) {
        if appearance::get().theme != appearance::ThemeMode::Desktop { self.desktop_note = None; return; }
        let Some(api) = self.api.clone() else {
            self.desktop_note = Some(tr("settings_desktop_offline"));
            cx.notify();
            return;
        };
        self.palette_seq += 1;
        let (tx, connection, seq) = (self.tx.clone(), self.connection, self.palette_seq);
        self.runtime.spawn(async move {
            let result = api.desktop_palette().await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::DesktopPalette(seq, result) }).await;
        });
    }

    fn receive_desktop_palette(&mut self, seq: u64, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        // Um pedido mais novo já saiu (outro foco, outro clique): esta resposta pintaria a paleta anterior.
        if seq != self.palette_seq { return; }
        let parsed = result.map_err(|error| match error.status {
            Some(403) => tr("settings_desktop_remote"),
            Some(404) => tr("settings_desktop_missing"),
            _ => Self::failure(&error),
        }).and_then(|value| {
            let dark = value.get("escuro").and_then(Value::as_bool).unwrap_or(true);
            let colors = value.get("cores");
            theme::from_desktop(dark, |name| colors?.get(name)?.as_str()?.strip_prefix('#')
                .and_then(|h| u32::from_str_radix(h, 16).ok()))
                .ok_or_else(|| tr("invalid_response"))
        });
        // Tema trocado enquanto a resposta vinha: ela não vale mais.
        if appearance::get().theme != appearance::ThemeMode::Desktop { return; }
        let note = parsed.as_ref().err().cloned();
        // Aviso fora das configurações só quando o motivo muda: a releitura a cada foco não repete a notificação.
        if let Some(reason) = note.as_ref().filter(|reason| self.desktop_note.as_ref() != Some(*reason)) {
            window.push_notification(Notification::warning(tr("settings_desktop_failed").replace("{reason}", reason)), cx);
        }
        self.desktop_note = note;
        theme::set_desktop(parsed.ok());
        theme::sync_kit(Some(window), cx);
        self.sync_sliders(window, cx);
        cx.notify();
    }

    /// Nome curto do servidor conectado (endereço sem o esquema), para a lista e as configurações.
    fn server_label(&self, cx: &App) -> String {
        let address = self.address.read(cx).value().to_string();
        let host = address.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
        if host.is_empty() { tr("connection") } else { host.to_owned() }
    }

    /// Texto da última resposta do agente na conversa aberta, para o atalho de copiar.
    fn last_reply(&self) -> Option<String> {
        self.chat.events.iter().rev().find(|e| e.kind == "assistant_msg").map(|e| e.body())
    }

    fn failure(error: &Failure) -> String {
        match error.status {
            Some(401 | 403) => tr("auth_error"), Some(429) => tr("rate_limited"),
            _ if error.uncertain => tr("delivery_uncertain"),
            _ => tr(&error.detail),
        }
    }

    // Leitura de arquivo: 403/404 é a política de caminho do backend, e o motivo dele é o que se mostra.
    fn fetch_failure(error: &Failure) -> String {
        match error.status { Some(401) => tr("auth_error"), Some(_) => tr(&error.detail), None => Self::failure(error) }
    }

    fn selected_key(&self) -> Option<SessionKey> {
        SessionKey::new(self.server.as_deref()?, self.selected.as_ref()?)
    }

    fn open_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.connection_dialog = true;
        self.address.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (address, token) = (self.address.read(cx).value().to_string(), self.token.read(cx).value().to_string());
        let api = match Api::new(&address, &token) {
            Ok(api) => api,
            Err(error) => { self.error = Some(Self::failure(&error)); cx.notify(); return; }
        };
        self.unsaved_connection = Some((address, token));
        if let Some(key) = self.selected_key() { self.drafts.insert(key, self.composer.read(cx).value().to_string()); }
        self.connection += 1;
        self.selection += 1;
        self.revision += 1;
        for slot in [&mut self.list_task, &mut self.session_task, &mut self.history_task] { if let Some(t) = slot.take() { t.abort(); } }
        self.api = Some(api.clone());
        self.server = Some(api.identity());
        self.selected = None;
        self.sessions.clear();
        self.chat = Chat::default();
        self.stats = None;
        self.reset_details();
        self.cancel_preview_drop();
        self.clear_visible_preview();
        self.rich.clear();
        // Resposta de imagem da conexão anterior é descartada no filtro; sem limpar, a prévia ficava em "Carregando".
        for image in self.media.clear() { cx.drop_image(image, Some(window)); }
        self.error = None;
        self.list_error = None;
        self.list_online = false;
        self.chat_online = false;
        self.connection_dialog = false;
        self.root_focus.focus(window, cx);
        self.loading = false;
        self.history_started = false;
        self.history_installed = false;
        self.pending_chat.clear();
        self.composer.update(cx, |input, cx| input.set_value("", window, cx));
        self.sync_rows(cx);
        let tx = self.tx.clone();
        let connection = self.connection;
        self.list_task = Some(self.runtime.spawn(async move {
            let result = api.sessions().await;
            let fatal = result.as_ref().err().is_some_and(|e| matches!(e.status, Some(401 | 403)));
            if tx.send(Envelope { connection, selection: None, payload: Payload::Sessions(result) }).await.is_err() || fatal { return; }
            forward_stream(api, None, connection, None, tx).await;
        }));
        self.side.reset_server();
        self.controls = controls::Controls::default();
        if let Some(api) = self.api.clone() {
            let tx = self.tx.clone();
            // Só leitura: a fileira de atalhos vem da config do servidor.
            self.runtime.spawn(async move {
                let result = api.config().await;
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Config(result) }).await;
            });
        }
        self.refresh_desktop_palette(cx);
        let a = appearance::get();
        if a.background == appearance::Background::Desktop && a.wallpaper == appearance::Wallpaper::Glass { self.refresh_backdrop(window, cx); }
        cx.notify();
    }

    fn select(&mut self, session: SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(old) = self.selected_key() { self.drafts.insert(old, self.composer.read(cx).value().to_string()); }
        self.selection += 1;
        self.revision += 1;
        if let Some(t) = self.session_task.take() { t.abort(); }
        if let Some(t) = self.history_task.take() { t.abort(); }
        self.chat = Chat::default();
        self.stats = None;
        self.reset_details();
        self.cancel_preview_drop();
        self.clear_visible_preview();
        self.chat.state.state = session.state.clone();
        self.chat.state.question = session.question.clone();
        self.error = None;
        self.loading = session.readable();
        self.history_started = false;
        self.history_installed = false;
        self.pending_chat.clear();
        self.chat_online = false;
        self.history_limit = 400;
        self.etag = None;
        self.has_older = false;
        self.rich.clear();
        self.row_ids.clear();
        self.list_state.reset(0);
        self.follow_reset();
        let draft = self.server.as_deref().and_then(|server| SessionKey::new(server, &session))
            .and_then(|key| self.drafts.get(&key).cloned()).unwrap_or_default();
        self.composer.update(cx, |input, cx| input.set_value(draft, window, cx));
        self.confirm = None;
        self.terminal_suggestion.clear();
        self.recent = None;
        self.command_panel = false;
        self.suggest_dismissed = None;
        self.side.on_select();
        self.controls.on_select();
        self.selected = Some(session.clone());
        if session.readable() {
            if let Some(api) = self.api.clone() {
                let tx = self.tx.clone();
                let connection = self.connection;
                let selection = self.selection;
                self.session_task = Some(self.runtime.spawn(forward_stream(api, Some(session.name), connection, Some(selection), tx)));
            }
        }
        cx.notify();
    }

    fn load_history(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(session)) = (self.api.clone(), self.selected.as_ref()) else { return; };
        if self.history_task.as_ref().is_some_and(|task| !task.is_finished()) { return; }
        self.history_started = true;
        self.loading = true;
        let name = session.name.clone();
        let (connection, selection, revision, limit) = (self.connection, self.selection, self.revision, self.history_limit);
        let etag = self.etag.clone();
        let tx = self.tx.clone();
        self.history_task = Some(self.runtime.spawn(async move {
            let result = api.history(&name, limit, etag.as_deref()).await;
            let _ = tx.send(Envelope { connection, selection: Some(selection), payload: Payload::History(revision, limit, result) }).await;
        }));
        cx.notify();
    }

    fn receive(&mut self, envelope: Envelope, window: &mut Window, cx: &mut Context<Self>) {
        // Resultados amarrados à identidade da sessão valem mesmo depois de trocar a seleção.
        let payload = match envelope.payload {
            Payload::Sent(key, text, draft, result) => {
                self.receive_sent(key, text, draft, result, window, cx);
                cx.notify();
                return;
            }
            Payload::Files(key, files) => { self.receive_files(key, files); cx.notify(); return; }
            Payload::UploadStep(key, id, result) => { self.receive_upload(key, id, result); cx.notify(); return; }
            Payload::UploadsDone(key, draft, steer, known) => { self.finish_uploads(key, draft, steer, known, cx); cx.notify(); return; }
            Payload::Saved(key, open, result) => {
                let note = match result {
                    Ok(path) if open => { cx.open_with_system(&path); None }
                    Ok(path) => Some((tr("saved").replace("{path}", &path.display().to_string()), false)),
                    Err(error) => Some((error, true)),
                };
                if let Some(note) = note { self.action_feedback.insert(key, note); }
                cx.notify();
                return;
            }
            Payload::AppearanceSaved(result) => {
                // O arquivo é deste computador, não da conexão: vale mesmo depois de trocar de servidor.
                self.appearance_note = result.err().map(|error| tr("settings_not_saved").replace("{error}", &error));
                cx.notify();
                return;
            }
            Payload::ConnectionNotSaved(error) => {
                eprintln!("conexão não gravada: {error}");
                self.list_error = Some(tr("connection_not_saved").replace("{error}", &error));
                cx.notify();
                return;
            }
            Payload::Interrupted(key, result) => {
                self.receive_interrupted(key, result);
                cx.notify();
                return;
            }
            Payload::Acted(key, action, result) => {
                self.receive_acted(key, action, result, window, cx);
                self.sync_rows(cx);
                cx.notify();
                return;
            }
            Payload::Reply(key, reply, result) => {
                if matches!(result, Err(Failure { status: Some(401 | 403), .. })) && self.selected_key().as_ref() == Some(&key)
                    && !matches!(reply, Reply::Diff(_)) { self.open_connection(window, cx); }
                self.receive_reply(key, reply, result, window, cx);
                cx.notify();
                return;
            }
            Payload::HeadlessPlan(key, outcome) => { self.receive_headless_plan(key, outcome); cx.notify(); return; }
            // O fundo é deste computador: o número do pedido decide, não a conexão.
            Payload::Backdrop(seq, result) => { self.receive_backdrop(seq, result, window, cx); return; }
            Payload::BackdropPicked(result) => { self.receive_picked_backdrop(result, window, cx); return; }
            Payload::BackdropRemoved(result) => { self.receive_removed_backdrop(result, window, cx); return; }
            payload => payload,
        };
        if envelope.connection != self.connection { return; }
        if envelope.selection.is_some_and(|selection| selection != self.selection) { return; }
        let is_chat = envelope.selection.is_some();
        match payload {
            Payload::Sessions(Ok(sessions)) => {
                self.list_error = None;
                if let Some((address, token)) = self.unsaved_connection.take() {
                    // Disco fora da thread da janela; só a falha volta.
                    let (connection, tx) = (self.connection, self.tx.clone());
                    self.runtime.spawn(async move {
                        let saved = tokio::task::spawn_blocking(move || save_connection(&address, &token).map_err(|e| e.to_string())).await;
                        if let Err(error) = saved.map_err(|e| e.to_string()).and_then(|r| r) {
                            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::ConnectionNotSaved(error) }).await;
                        }
                    });
                }
                self.replace_sessions(sessions, window, cx);
            }
            Payload::Sessions(Err(error)) => {
                if matches!(error.status, Some(401 | 403)) {
                    self.open_connection(window, cx);
                    self.error = Some(Self::failure(&error));
                }
                self.list_error = Some(Self::failure(&error));
            }
            Payload::Stream(Update::Online) => {
                if is_chat {
                    self.chat_online = true;
                    self.error = None;
                    if !self.history_started { self.load_history(cx); }
                } else { self.list_online = true; }
            }
            Payload::Stream(Update::Offline(error)) => {
                if matches!(error.status, Some(401 | 403)) {
                    self.open_connection(window, cx);
                    self.error = Some(Self::failure(&error));
                }
                if is_chat { self.chat_online = false; self.error = Some(Self::failure(&error)); }
                else { self.list_online = false; self.list_error = Some(Self::failure(&error)); }
            }
            Payload::Stream(Update::Frame(frame)) => {
                let applied = if is_chat { self.accept_chat_frame(&frame.event, frame.data, cx) }
                    else if frame.event == "sessions" {
                        match serde_json::from_value(frame.data) {
                            Ok(sessions) => { self.list_error = None; self.replace_sessions(sessions, window, cx); true }
                            Err(_) => { self.list_error = Some(tr("invalid_response")); false }
                        }
                    } else if frame.event == "list_error" { self.list_error = Some(tr("list_stale")); true }
                    else { true };
                let _ = frame.applied.send(applied);
            }
            Payload::History(revision, limit, result) if revision == self.revision && limit == self.history_limit => {
                self.history_task = None;
                self.loading = false;
                match result {
                    Ok(history) => {
                        self.etag = history.etag;
                        if let Some(events) = history.events {
                            self.has_older = events.len() >= limit;
                            self.chat.merge_history(events);
                            if self.chat.preview.text.is_empty() {
                                self.cancel_preview_drop();
                                self.clear_visible_preview();
                            }
                        }
                        self.history_installed = true;
                        for update in std::mem::take(&mut self.pending_chat) { self.apply_chat_update(update, cx); }
                        self.error = None;
                        self.ensure_commands(false);
                        self.discover_plan();
                    }
                    Err(error) => {
                        if error.status == Some(404) {
                            if let Some(api) = self.api.clone() {
                                let tx = self.tx.clone();
                                let connection = self.connection;
                                self.runtime.spawn(async move {
                                    let result = api.sessions().await;
                                    let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Sessions(result) }).await;
                                });
                            }
                        }
                        self.history_installed = true;
                        for update in std::mem::take(&mut self.pending_chat) { self.apply_chat_update(update, cx); }
                        self.error = Some(Self::failure(&error));
                    }
                }
            }
            Payload::History(..) => return,
            Payload::Commands(cache, result) => { self.commands.insert(cache, result.map_err(|error| Self::failure(&error))); }
            Payload::Recent(key, result) => {
                if let Some(recent) = self.recent.as_mut().filter(|recent| recent.key == key) {
                    recent.files = Some(result.map(|mut files| {
                        files.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
                        files.truncate(20);
                        files
                    }).map_err(|error| Self::failure(&error)));
                }
            }
            Payload::Media(key, source, result) => {
                let state = match result {
                    Ok(Some(image)) => MediaState::Image(image),
                    Ok(None) => MediaState::Failed(tr("media_unreadable")),
                    Err(error) => MediaState::Failed(Self::fetch_failure(&error)),
                };
                for image in self.media.insert((key, source), state) { cx.drop_image(image, Some(window)); }
                let rows = self.row_ids.len();
                if rows > 0 { self.follow_content_changed(cx); self.list_state.remeasure_items(0..rows); }
            }
            Payload::Config(result) => self.side.receive_config(result.map_err(|error| Self::failure(&error))),
            Payload::DesktopPalette(seq, result) => { self.receive_desktop_palette(seq, result, window, cx); return; }
            Payload::Sent(..) | Payload::Interrupted(..) | Payload::Acted(..) | Payload::Files(..) | Payload::UploadStep(..)
                | Payload::UploadsDone(..) | Payload::Saved(..) | Payload::ConnectionNotSaved(..) | Payload::Reply(..) | Payload::HeadlessPlan(..)
                | Payload::AppearanceSaved(..) | Payload::Backdrop(..) | Payload::BackdropPicked(..) | Payload::BackdropRemoved(..) => unreachable!(),
        }
        self.sync_rows(cx);
        cx.notify();
    }

    fn receive_sent(&mut self, key: SessionKey, text: String, draft: String, result: Result<Delivery, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = match &result {
            Ok(delivery) if delivery.delivered => SendOutcome::Delivered,
            Ok(_) => SendOutcome::Queued,
            Err(error) if error.uncertain => SendOutcome::Uncertain,
            Err(error) => SendOutcome::Rejected(Self::failure(error)),
        };
        if !self.delivery.complete(&key, &text, outcome) { return; }
        let current = self.selected_key().as_ref() == Some(&key);
        let confirmed = self.delivery.outcome(&key).is_none();
        if result.is_ok() || confirmed {
            if self.drafts.get(&key).is_some_and(|saved| saved == &draft) { self.drafts.remove(&key); }
            if current && !draft.is_empty() && self.composer.read(cx).value().as_ref() == draft {
                self.composer.update(cx, |input, cx| input.set_value("", window, cx));
            }
            // Só os anexos que subiram nesta mensagem saem; um anexado depois continua no campo.
            if let Some(list) = self.attachments.get_mut(&key) {
                let mut gone = Vec::new();
                list.retain(|a| {
                    let keep = !matches!(&a.state, AttachState::Uploaded(up) if text.contains(&up.path));
                    if !keep { gone.extend(a.image.clone()); }
                    keep
                });
                if list.is_empty() { self.attachments.remove(&key); }
                for image in gone { release_image(image, window, cx); }
            }
        } else {
            if !current && !draft.is_empty() { self.drafts.entry(key.clone()).or_insert(draft); }
            if current && result.as_ref().err().is_some_and(|error| matches!(error.status, Some(401 | 403))) {
                self.open_connection(window, cx);
            }
        }
    }

    fn receive_interrupted(&mut self, key: SessionKey, result: Result<(), Failure>) {
        self.stopping.remove(&key);
        let note = match result { Ok(()) => (tr("stop_requested"), false), Err(error) => (Self::failure(&error), true) };
        self.stop_feedback.insert(key, note);
    }

    fn replace_sessions(&mut self, sessions: Vec<SessionInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let old = self.selected.clone();
        self.sessions = sessions;
        if let Some(old) = old {
            match self.sessions.iter().find(|s| s.name == old.name).cloned() {
                Some(new) if new.jsonl != old.jsonl || new.tracked != old.tracked => self.select(new, window, cx),
                Some(new) => self.selected = Some(new),
                None => {
                    self.selection += 1;
                    if let Some(task) = self.session_task.take() { task.abort(); }
                    if let Some(task) = self.history_task.take() { task.abort(); }
                    self.selected = None;
                    self.chat = Chat::default();
                    self.reset_details();
                    self.cancel_preview_drop();
                    self.clear_visible_preview();
                    self.loading = false;
                    self.chat_online = false;
                    self.error = Some(tr("session_gone"));
                }
            }
        }
    }

    fn accept_chat_frame(&mut self, event: &str, data: serde_json::Value, cx: &mut Context<Self>) -> bool {
        let update = match event {
            "message" | "queue_confirmed" => serde_json::from_value(data).map(ChatUpdate::Message),
            "preview" => serde_json::from_value(data).map(ChatUpdate::Preview),
            "state" => serde_json::from_value(data).map(ChatUpdate::State),
            "ask_question" if data.is_null() => Ok(ChatUpdate::Question(None)),
            "ask_question" => serde_json::from_value(data.clone()).map(|payload| ChatUpdate::Question(Some(Ask::new(payload, &data)))),
            "reset" => Ok(ChatUpdate::Reset),
            "suggest" => {
                self.terminal_suggestion = data.get("text").and_then(Value::as_str).unwrap_or("").to_owned();
                return true;
            }
            "stats" => {
                return match serde_json::from_value::<Option<Stats>>(data) {
                    Ok(stats) => { self.stats = stats; true }
                    Err(_) => { self.error = Some(tr("invalid_response")); false }
                };
            }
            "pensamento" | "ferramenta" => {
                let Some(text) = data.get("text").and_then(|v| v.as_str()) else {
                    self.error = Some(tr("invalid_response"));
                    return false;
                };
                if event == "pensamento" { Ok(ChatUpdate::Thinking(text.to_owned())) }
                else if text.is_empty() { Ok(ChatUpdate::LiveTool(None)) }
                else {
                    #[derive(serde::Deserialize)]
                    struct Wire { nome: Option<String>, #[serde(default)] input: serde_json::Value }
                    serde_json::from_str::<Wire>(text).map(|wire| ChatUpdate::LiveTool(Some(LiveTool {
                        name: wire.nome.unwrap_or_else(|| tr("tool")), input: wire.input,
                    })))
                }
            }
            _ => return true,
        };
        let update = match update {
            Ok(update) => update,
            Err(_) => { self.error = Some(tr("invalid_response")); return false; }
        };
        if !self.history_installed && !matches!(update, ChatUpdate::Reset) {
            self.pending_chat.push(update);
        } else { self.apply_chat_update(update, cx); }
        true
    }

    fn apply_chat_update(&mut self, update: ChatUpdate, cx: &mut Context<Self>) {
        match update {
            ChatUpdate::Message(event) => {
                if event.kind == "user_msg" && !event.queued() {
                    if let (Some(key), Some(text)) = (self.selected_key(), event.text.as_deref()) {
                        self.delivery.confirm_real(&key, &event.id, text);
                    }
                }
                self.chat.apply(event);
                if self.chat.preview.text.is_empty() {
                    self.cancel_preview_drop();
                    self.clear_visible_preview();
                }
            }
            ChatUpdate::Preview(preview) => {
                if self.chat.update_preview(preview) {
                    self.cancel_preview_drop();
                    self.update_visible_preview(cx);
                }
                if self.chat.state.state != "working" { self.defer_preview_drop(cx); }
            }
            ChatUpdate::State(state) => {
                // Turno terminou: o plano do Claude com terminal pode ter mudado de arquivo.
                let finished = self.chat.state.state == "working" && state.state != "working";
                let resumed = self.chat.state.state == "awaiting_input" && state.state == "working";
                self.chat.update_state(state);
                if finished { self.discover_plan(); }
                if resumed { self.controls.clear_plan_preview(); }
                if self.chat.ask.is_none() { self.ask_form = AskForm::default(); }
                if self.chat.state.state == "working" { self.cancel_preview_drop(); }
                else {
                    self.defer_preview_drop(cx);
                    self.defer_live_clear(Live::Thinking, cx);
                    self.defer_live_clear(Live::Tool, cx);
                }
            }
            ChatUpdate::Question(ask) => if self.chat.update_ask(ask) { self.ask_form = AskForm::default(); },
            ChatUpdate::Thinking(text) => {
                if text.is_empty() { self.defer_live_clear(Live::Thinking, cx); }
                else if self.chat.update_live_thinking(text) { self.live_clear_epoch[Live::Thinking as usize] += 1; }
            }
            ChatUpdate::LiveTool(tool) => match tool {
                Some(tool) => if self.chat.update_live_tool(tool) { self.live_clear_epoch[Live::Tool as usize] += 1; },
                None => self.defer_live_clear(Live::Tool, cx),
            },
            ChatUpdate::Reset => {
                // Transcript trocado: as perguntas respondidas desta sessão não valem mais.
                if let Some(key) = self.selected_key() {
                    self.answered_tools.retain(|(owner, _)| owner != &key);
                    self.answering.remove(&key);
                }
                self.revision += 1;
                self.terminal_suggestion.clear();
                if let Some(task) = self.history_task.take() { task.abort(); }
                self.chat = Chat::default();
                self.stats = None;
                self.controls.clear_plan_preview();
                self.reset_details();
                self.cancel_preview_drop();
                self.clear_visible_preview();
                self.pending_chat.clear();
                self.history_started = false;
                self.history_installed = false;
                self.loading = false;
                self.has_older = false;
                self.rich.clear();
                self.row_ids.clear();
                self.list_state.reset(0);
                self.follow_reset();
                self.etag = None;
                self.load_history(cx);
            }
        }
    }

    fn reset_details(&mut self) {
        self.expanded.clear();
        self.ask_form = AskForm::default();
        for epoch in &mut self.live_clear_epoch { *epoch += 1; }
    }

    // Quadro vazio encerra o item em voo; a espera evita piscar entre dois passos seguidos.
    fn defer_live_clear(&mut self, slot: Live, cx: &mut Context<Self>) {
        let active = match slot { Live::Thinking => !self.chat.live_thinking.is_empty(), Live::Tool => self.chat.live_tool.is_some() };
        if !active { return; }
        let (connection, selection, epoch) = (self.connection, self.selection, self.live_clear_epoch[slot as usize]);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    if this.connection != connection || this.selection != selection || this.live_clear_epoch[slot as usize] != epoch { return; }
                    match slot { Live::Thinking => this.chat.live_thinking.clear(), Live::Tool => this.chat.live_tool = None }
                    this.sync_rows(cx);
                    cx.notify();
                });
            }
        }).detach();
    }

    fn toggle(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) { self.expanded.insert(key.clone()); }
        let row = self.row_ids.iter().position(|id| id == &key).or_else(|| {
            let events = &self.chat.events;
            self.items.iter().position(|item| match item {
                Item::Group { tools, .. } => tools.iter().any(|t| events[t.call].id == key),
                Item::Thinking { parts, .. } => parts.iter().any(|&i| events[i].id == key),
                _ => false,
            })
        });
        if let Some(row) = row { self.list_state.remeasure_items(row..row + 1); }
        cx.notify();
    }

    fn cancel_preview_drop(&mut self) {
        self.preview_drop_epoch = self.preview_drop_epoch.wrapping_add(1);
        self.preview_drop_scheduled = false;
    }

    fn defer_preview_drop(&mut self, cx: &mut Context<Self>) {
        if self.preview_drop_scheduled || self.chat.preview.text.is_empty() { return; }
        self.preview_drop_scheduled = true;
        let (connection, selection, epoch) = (self.connection, self.selection, self.preview_drop_epoch);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    if this.connection != connection || this.selection != selection || this.preview_drop_epoch != epoch { return; }
                    this.preview_drop_scheduled = false;
                    if this.chat.state.state == "working" { return; }
                    this.chat.clear_preview();
                    this.clear_visible_preview();
                    this.sync_rows(cx);
                    cx.notify();
                });
            }
        }).detach();
    }

    fn clear_visible_preview(&mut self) {
        self.preview_tick_epoch = self.preview_tick_epoch.wrapping_add(1);
        self.preview_tick_scheduled = false;
        self.preview_last_tick = None;
        self.preview_deadline = None;
        self.visible_preview = Preview::default();
    }

    fn update_visible_preview(&mut self, cx: &mut Context<Self>) {
        let next = &self.chat.preview;
        let extends = self.visible_preview.md == next.md && self.visible_preview.full == next.full
            && self.visible_preview.vivo == next.vivo && next.text.starts_with(&self.visible_preview.text);
        if self.visible_preview.text.is_empty() || next.vivo || cx.reduce_motion() || !extends {
            let next = next.clone();
            self.clear_visible_preview();
            self.visible_preview = next;
            return;
        }
        self.preview_deadline = Some(Instant::now() + Duration::from_millis(1200));
        self.schedule_preview_tick(cx);
    }

    fn schedule_preview_tick(&mut self, cx: &mut Context<Self>) {
        if self.preview_tick_scheduled || self.visible_preview.text == self.chat.preview.text { return; }
        self.preview_tick_scheduled = true;
        let (connection, selection, epoch) = (self.connection, self.selection, self.preview_tick_epoch);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(33)).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    if this.connection != connection || this.selection != selection || this.preview_tick_epoch != epoch { return; }
                    this.preview_tick_scheduled = false;
                    this.advance_visible_preview(cx);
                });
            }
        }).detach();
    }

    fn advance_visible_preview(&mut self, cx: &mut Context<Self>) {
        let Some(rest) = self.chat.preview.text.strip_prefix(&self.visible_preview.text) else {
            self.update_visible_preview(cx);
            self.sync_rows(cx);
            cx.notify();
            return;
        };
        let remaining_chars = rest.chars().count();
        if remaining_chars == 0 { self.preview_last_tick = None; return; }
        let now = Instant::now();
        let elapsed = self.preview_last_tick.replace(now).map(|last| now.duration_since(last).as_secs_f64()).unwrap_or(0.033).max(0.033);
        let remaining_time = self.preview_deadline.unwrap_or(now).saturating_duration_since(now).as_secs_f64().max(0.05);
        let pace = 160.0_f64.max(remaining_chars as f64 / remaining_time);
        let count = ((pace * elapsed).ceil() as usize).clamp(1, remaining_chars);
        self.visible_preview.text.extend(rest.chars().take(count));
        self.sync_rows(cx);
        cx.notify();
        if count < remaining_chars { self.schedule_preview_tick(cx); }
        else { self.preview_last_tick = None; }
    }

    fn known_user_ids(&self) -> HashSet<String> {
        self.chat.events.iter().filter(|event| event.kind == "user_msg" && !event.queued()).map(|event| event.id.clone()).collect()
    }

    fn commands_key(&self) -> Option<String> {
        let (server, session) = (self.server.as_deref()?, self.selected.as_ref()?);
        Some(format!("{server}|{}|{}", session.provider, session.name))
    }

    fn command_list(&self) -> &[CommandInfo] {
        self.commands_key().and_then(|key| self.commands.get(&key)).and_then(|r| r.as_ref().ok()).map(Vec::as_slice).unwrap_or(&[])
    }

    // Codex muda a lista conforme o modo: sempre relê. Os demais usam a lista já buscada.
    fn ensure_commands(&mut self, force: bool) {
        let (Some(api), Some(session), Some(cache)) = (self.api.clone(), self.selected.clone(), self.commands_key()) else { return; };
        if !force && session.provider != "codex" && self.commands.get(&cache).is_some_and(|r| r.is_ok()) { return; }
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.commands(&session.name).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Commands(cache, result) }).await;
        });
    }

    fn can_send(&self) -> bool { !self.connection_dialog && self.chat_online && self.history_installed }

    // `confirmed` = a pessoa já aceitou o aviso de comando destrutivo para este mesmo texto.
    fn submit(&mut self, steer: bool, confirmed: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_send() { return; }
        let Some(key) = self.selected_key() else { return; };
        let text = self.composer.read(cx).value().to_string();
        let attached = self.attachments.get(&key).is_some_and(|list| !list.is_empty());
        if text.trim().is_empty() && !attached { return; }
        if self.delivery.pending(&key) || self.uploading.contains_key(&key) { return; }
        // Com anexo a legenda vai na frente do prompt: o comando do campo continua sendo o da mensagem.
        let provider = self.provider().0.to_owned();
        if let Some(command) = composer::typed_command(self.command_list(), &text).cloned() {
            if composer::needs_other_surface(&provider, &command) {
                self.action_feedback.insert(key, (tr("command_other_surface").replace("{cmd}", &format!("/{}", command.name)), true));
                cx.notify();
                return;
            }
            if command.destructive && !confirmed {
                self.confirm = Some(Confirm::Destructive(text));
                cx.notify();
                return;
            }
        }
        self.confirm = None;
        self.action_feedback.remove(&key);
        let known = self.known_user_ids();
        if attached { self.start_uploads(key, text, steer, known, cx); }
        else { self.deliver(key, text.clone(), text, steer, known, cx); }
        let _ = window;
    }

    fn deliver(&mut self, key: SessionKey, text: String, draft: String, steer: bool, known: HashSet<String>, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| self.server.as_deref() == Some(key.server.as_str())) else {
            self.action_feedback.insert(key, (tr("server_changed"), true));
            return;
        };
        if !self.delivery.begin(key.clone(), text.clone(), known) { return; }
        self.error = None;
        self.stop_feedback.remove(&key);
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = if steer { api.steer_text(&key.name, &text).await } else { api.send(&key.name, &text).await };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Sent(key, text, draft, result) }).await;
        });
        self.follow_engage(cx);
    }

    // Sobe um por vez; o que já subiu não sobe de novo numa nova tentativa, e falha para a fila sem repetir.
    fn start_uploads(&mut self, key: SessionKey, draft: String, steer: bool, known: HashSet<String>, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| self.server.as_deref() == Some(key.server.as_str())) else { return; };
        let Some(list) = self.attachments.get_mut(&key) else { return; };
        let mut jobs = Vec::new();
        for attachment in list.iter_mut() {
            if matches!(attachment.state, AttachState::Uploaded(_)) { continue; }
            attachment.state = AttachState::Waiting;
            jobs.push((attachment.id, attachment.name.clone(), attachment.bytes.clone()));
        }
        self.uploading.insert(key.clone(), list.iter().map(|a| a.id).collect());
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            for (id, name, bytes) in jobs {
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::UploadStep(key.clone(), id, None) }).await;
                let result = api.upload(&key.name, &name, composer::mime_for(&name), bytes.to_vec()).await;
                let failed = result.is_err();
                if tx.send(Envelope { connection, selection: None, payload: Payload::UploadStep(key.clone(), id, Some(result)) }).await.is_err() { return; }
                if failed { break; }
            }
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::UploadsDone(key, draft, steer, known) }).await;
        });
        cx.notify();
    }

    fn receive_upload(&mut self, key: SessionKey, id: u64, result: Option<Result<Uploaded, Failure>>) {
        let Some(attachment) = self.attachments.get_mut(&key).and_then(|list| list.iter_mut().find(|a| a.id == id)) else { return; };
        attachment.state = match result {
            None => AttachState::Uploading,
            Some(Ok(uploaded)) => AttachState::Uploaded(uploaded),
            Some(Err(error)) if error.uncertain => AttachState::Failed(tr("attach_uncertain")),
            Some(Err(error)) if error.status == Some(413) => AttachState::Failed(tr("attach_too_big")),
            Some(Err(error)) => AttachState::Failed(Self::failure(&error)),
        };
    }

    fn finish_uploads(&mut self, key: SessionKey, draft: String, steer: bool, known: HashSet<String>, cx: &mut Context<Self>) {
        let Some(batch) = self.uploading.remove(&key) else { return; };
        let Some(list) = self.attachments.get(&key) else { return; };
        let mut uploads = Vec::new();
        for attachment in list.iter().filter(|a| batch.contains(&a.id)) {
            match &attachment.state {
                AttachState::Uploaded(up) => uploads.push((attachment.image.is_some() || composer::image_format(&attachment.name).is_some(), up.clone())),
                _ => {
                    self.action_feedback.insert(key, (tr("attach_not_sent"), true));
                    return;
                }
            }
        }
        let message = composer::compose_prompt(&draft, &uploads, |speech| tr("attach_video_speech").replace("{texto}", speech));
        self.deliver(key, message, draft, steer, known, cx);
    }

    fn add_attachment(&mut self, key: &SessionKey, name: String, bytes: Vec<u8>) -> Result<(), String> {
        if composer::is_audio(&name) { return Err(tr("attach_audio").replace("{name}", &name)); }
        if bytes.len() as u64 > api::MAX_BYTES { return Err(tr("attach_too_big_named").replace("{name}", &name)); }
        let image = composer::image_format(&name).map(|format| Arc::new(Image::from_bytes(format, bytes.clone())));
        self.attach_seq += 1;
        let attachment = Attachment { id: self.attach_seq, name, bytes: Arc::new(bytes), image, state: AttachState::Waiting };
        self.attachments.entry(key.clone()).or_default().push(attachment);
        Ok(())
    }

    fn receive_files(&mut self, key: SessionKey, files: Vec<Result<Picked, String>>) {
        let mut problems = Vec::new();
        for file in files {
            match file.and_then(|picked| self.add_attachment(&key, picked.name, picked.bytes)) {
                Ok(()) => {}
                Err(problem) => problems.push(problem),
            }
        }
        if problems.is_empty() { self.action_feedback.remove(&key); }
        else { self.action_feedback.insert(key, (problems.join(" "), true)); }
    }

    // Leitura do disco fora da janela; tamanho conferido antes de ler.
    fn read_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return; };
        if paths.is_empty() || self.uploading.contains_key(&key) { return; }
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let files = tokio::task::spawn_blocking(move || paths.into_iter().map(|path| {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "arquivo".into());
                let meta = std::fs::metadata(&path).map_err(|_| tr("attach_read_failed").replace("{name}", &name))?;
                if !meta.is_file() { return Err(tr("attach_read_failed").replace("{name}", &name)); }
                if meta.len() > api::MAX_BYTES { return Err(tr("attach_too_big_named").replace("{name}", &name)); }
                std::fs::read(&path).map(|bytes| Picked { name: name.clone(), bytes }).map_err(|_| tr("attach_read_failed").replace("{name}", &name))
            }).collect()).await.unwrap_or_default();
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Files(key, files) }).await;
        });
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return; };
        let prompt = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            let chosen = prompt.await;
            let Some(this) = this.upgrade() else { return; };
            this.update(cx, |this, cx| match chosen {
                Ok(Ok(Some(paths))) if this.selected_key().as_ref() == Some(&key) => this.read_paths(paths, cx),
                Ok(Ok(_)) => {}
                _ => { this.action_feedback.insert(key, (tr("picker_failed"), true)); cx.notify(); }
            });
        }).detach();
    }

    // Colar: imagem vira anexo e arquivo copiado vira anexo; texto segue para o campo.
    fn paste(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.selected_key() else { return false; };
        let mut paths = Vec::new();
        let mut took = false;
        let mut problems = Vec::new();
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    took = true;
                    let ext = image.format.mime_type().rsplit('/').next().unwrap_or("png").replace("svg+xml", "svg").replace("jpeg", "jpg");
                    let name = format!("colado-{}.{ext}", self.attach_seq + 1);
                    if let Err(problem) = self.add_attachment(&key, name, image.bytes.clone()) { problems.push(problem); }
                }
                ClipboardEntry::ExternalPaths(list) => { took = true; paths.extend(list.paths().iter().cloned()); }
                _ => {}
            }
        }
        if !problems.is_empty() { self.action_feedback.insert(key, (problems.join(" "), true)); }
        if !paths.is_empty() { self.read_paths(paths, cx); }
        if took { cx.notify(); }
        took
    }

    fn remove_attachment(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return; };
        if self.uploading.contains_key(&key) { return; }
        if let Some(list) = self.attachments.get_mut(&key) {
            let gone = list.iter().position(|a| a.id == id).and_then(|n| list.remove(n).image);
            if list.is_empty() { self.attachments.remove(&key); }
            if let Some(image) = gone { release_image(image, window, cx); }
        }
        cx.notify();
    }

    fn open_recent(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        if self.recent.as_ref().is_some_and(|recent| recent.key == key) { self.recent = None; cx.notify(); return; }
        self.recent = Some(Recent { key: key.clone(), files: None });
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.uploads(&key.name).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Recent(key, result) }).await;
        });
        cx.notify();
    }

    // Baixa de volta um anexo do cofre e o põe no campo como qualquer outro, sem citar caminho por presunção.
    fn reattach(&mut self, filename: String, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        self.recent = None;
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.fetch(&key.name, &Source::Upload(filename.clone())).await
                .map(|bytes| Picked { name: filename.clone(), bytes })
                .map_err(|error| format!("{}: {}", filename, Self::fetch_failure(&error)));
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Files(key, vec![result]) }).await;
        });
        cx.notify();
    }

    fn ensure_media(&mut self, source: &Source) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        let slot = (key.clone(), source.clone());
        if self.media.contains(&slot) { return; }
        self.media.start(slot);
        let (connection, tx, source) = (self.connection, self.tx.clone(), source.clone());
        self.runtime.spawn(async move {
            let result = match api.fetch(&key.name, &source).await {
                Ok(bytes) => Ok(tokio::task::spawn_blocking(move || media::thumbnail(&bytes)).await.ok().flatten()),
                Err(error) => Err(error),
            };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Media(key, source, result) }).await;
        });
    }

    // Abrir grava uma cópia privada e entrega ao programa do sistema; salvar pergunta o destino. Nunca há token em URL.
    fn keep_file(&mut self, source: Source, name: String, open: bool, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        let safe = composer::safe_name(&name);
        // Único ponto por onde os dois botões passam: só abre o que, com o nome gravado, é tipo passivo.
        if open && !composer::openable(&safe) {
            self.action_feedback.insert(key, (tr("open_refused").replace("{name}", &safe), true));
            cx.notify();
            return;
        }
        let (connection, tx, runtime) = (self.connection, self.tx.clone(), self.runtime.clone());
        let write = move |target: Option<PathBuf>| {
            runtime.spawn(async move {
                let result = async {
                    let bytes = api.fetch(&key.name, &source).await.map_err(|error| format!("{safe}: {}", Self::fetch_failure(&error)))?;
                    let path = match target { Some(path) => path, None => private_copy(&safe).map_err(|_| tr("save_failed"))? };
                    // Até 100 MiB: a escrita não pode ocupar uma das duas threads do runtime.
                    tokio::task::spawn_blocking(move || std::fs::write(&path, bytes).map(|()| path)).await
                        .ok().and_then(Result::ok).ok_or_else(|| tr("save_failed"))
                }.await;
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Saved(key, open, result) }).await;
            });
        };
        if open { write(None); return; }
        let folder = std::env::var_os("XDG_DOWNLOAD_DIR").map(PathBuf::from).filter(|p| p.is_dir())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads")).filter(|p| p.is_dir()))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_else(std::env::temp_dir);
        let prompt = cx.prompt_for_new_path(&folder, Some(&composer::safe_name(&name)));
        let fail_key = self.selected_key();
        cx.spawn(async move |this, cx| {
            match prompt.await {
                Ok(Ok(Some(path))) => write(Some(path)),
                Ok(Ok(None)) => {}
                // O diálogo do sistema falhou: dizer, não parecer que Salvar não fez nada.
                _ => { let _ = this.update(cx, |this, cx| {
                    if let Some(key) = fail_key { this.action_feedback.insert(key, (tr("save_failed"), true)); }
                    cx.notify();
                }); }
            }
        }).detach();
    }

    fn can_interrupt(&self) -> bool {
        let (_, headless) = self.provider();
        self.chat_online && (self.chat.state.state == "working" || headless && self.chat.state.state == "awaiting_input")
    }

    fn request_stop(&mut self, cx: &mut Context<Self>) {
        if self.connection_dialog || !self.can_interrupt() { return; }
        self.confirm = Some(Confirm::Stop);
        cx.notify();
    }

    // Mensagem aceita que a conversa ainda não mostrou volta ao campo, e só então o backend limpa a entrada do terminal.
    fn interrupt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = None;
        if self.connection_dialog || !self.can_interrupt() { cx.notify(); return; }
        let (Some(api), Some(session)) = (self.api.clone(), self.selected.as_ref()) else { return; };
        let Some(key) = self.selected_key() else { return; };
        if self.stopping.contains(&key) { return; }
        let returned = self.delivery.take_unconfirmed(&key);
        if let Some(text) = &returned {
            let current = self.composer.read(cx).value().to_string();
            let merged = if current.trim().is_empty() { text.clone() } else { format!("{text}\n{current}") };
            self.composer.update(cx, |input, cx| { input.set_value(merged, window, cx); input.focus(window, cx); });
        }
        self.stopping.insert(key.clone());
        self.error = None;
        self.stop_feedback.remove(&key);
        let name = session.name.clone();
        let clear = returned.is_some();
        let (connection, selection) = (self.connection, self.selection);
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = api.interrupt(&name, clear).await;
            let _ = tx.send(Envelope { connection, selection: Some(selection), payload: Payload::Interrupted(key, result) }).await;
        });
        if clear { self.action_feedback.insert(self.selected_key().expect("selected"), (tr("stop_returned"), false)); }
        cx.notify();
    }

    // Preencher o campo com um comando; texto que não é comando pede confirmação antes de ser trocado.
    fn fill_command(&mut self, name: &str, protect: bool, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.composer.read(cx).value().to_string();
        if protect && !current.trim().is_empty() && composer::slash_query(&current).is_none() {
            self.confirm = Some(Confirm::Replace(name.to_owned()));
            cx.notify();
            return;
        }
        self.confirm = None;
        self.composer.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.insert(format!("/{name} "), window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    // Mesmo roteamento do web: argumento, destrutivo ou Codex preenchem; o resto envia na hora.
    fn pick_command(&mut self, command: CommandInfo, from_panel: bool, window: &mut Window, cx: &mut Context<Self>) {
        let provider = self.provider().0.to_owned();
        self.command_panel = false;
        if composer::needs_other_surface(&provider, &command) {
            if let Some(key) = self.selected_key() {
                self.action_feedback.insert(key, (tr("command_other_surface").replace("{cmd}", &format!("/{}", command.name)), true));
            }
            cx.notify();
            return;
        }
        if provider == "codex" || command.argument_hint.as_deref().is_some_and(|h| !h.is_empty()) || command.destructive {
            self.fill_command(&command.name, from_panel, window, cx);
            return;
        }
        let Some(key) = self.selected_key() else { return; };
        if !self.can_send() || self.delivery.pending(&key) || self.uploading.contains_key(&key) { return; }
        let draft = if from_panel { String::new() } else { self.composer.read(cx).value().to_string() };
        let known = self.known_user_ids();
        self.deliver(key, format!("/{}", command.name), draft, false, known, cx);
    }

    fn visible_suggestions(&self, cx: &App) -> Vec<CommandInfo> {
        let text = self.composer.read(cx).value().to_string();
        if self.suggest_dismissed.as_deref() == Some(text.as_str()) { return Vec::new(); }
        let Some(query) = composer::slash_query(&text) else { return Vec::new(); };
        composer::suggestions(self.command_list(), query).into_iter().cloned().collect()
    }

    fn tool_answered(&self, id: &str) -> bool {
        self.selected_key().is_some_and(|key| self.answered_tools.contains(&(key, id.to_owned())))
    }

    fn provider(&self) -> (&str, bool) {
        self.selected.as_ref().map(|s| (s.provider.as_str(), s.headless)).unwrap_or(("", false))
    }

    // Linha ao vivo do stream da sessão; enquanto ela não chega, a da lista (cache do backend).
    fn status(&self) -> Option<crate::status::StatusFields> {
        let session = self.selected.as_ref()?;
        let raw = self.chat.state.status_line.as_deref().or(session.status_line.as_deref());
        crate::status::parse(raw, Some(session))
    }

    fn queued_count(&self) -> usize {
        let (provider, headless) = self.provider();
        self.chat.waiting(provider, headless)
    }

    fn steer_offered(&self) -> bool {
        let (provider, headless) = self.provider();
        self.chat.state.state == "working" && self.queued_count() > 0 && (headless || matches!(provider, "codex" | "kimi"))
    }

    // Implementar pelo menu da TUI: só Codex com terminal, e só o plano da última resposta.
    fn codex_plan(&self) -> Option<(String, String)> {
        if self.provider() != ("codex", false) { return None; }
        for event in self.chat.events.iter().rev() {
            if event.kind == "user_msg" && !event.queued() { return None; }
            if event.kind == "assistant_msg" {
                if let Some(plan) = event.text.as_deref().and_then(interaction::proposed_plan) { return Some((event.id.clone(), plan)); }
            }
        }
        None
    }

    fn current_snapshot(&self, action: &Action) -> Option<String> {
        match action {
            Action::Answer => self.chat.ask.as_ref().map(|ask| ask.fingerprint.clone()),
            Action::Select(_) | Action::Submit => Some(select_snapshot(&self.chat.state)),
            Action::Implement => self.codex_plan().map(|(_, plan)| plan),
            Action::Discard(id) => self.chat.events.iter().any(|e| e.id == format!("queued-{id}") && e.desistiu == Some(true)).then(|| id.clone()),
            Action::Cancel | Action::Steer => Some(String::new()),
        }
    }

    fn answer_body(&self, cx: &Context<Self>) -> Option<Value> {
        let ask = self.chat.ask.as_ref()?;
        if self.ask_form.fingerprint != ask.fingerprint { return None; }
        let picks: Vec<Pick> = self.ask_form.picks.iter().enumerate().map(|(i, pick)| {
            if self.ask_form.typing.get(i) == Some(&true) {
                Pick::Text(self.ask_form.inputs.get(i).map(|input| input.read(cx).value().to_string()).unwrap_or_default())
            } else { pick.clone() }
        }).collect();
        interaction::answer_body(ask, &picks)
    }

    // O instantâneo é o pedido que a pessoa viu ao clicar; se o atual difere, nada sai.
    fn act(&mut self, action: Action, snapshot: String, cx: &mut Context<Self>) {
        if self.connection_dialog || !self.chat_online || !self.history_installed { return; }
        let (Some(api), Some(session), Some(key)) = (self.api.clone(), self.selected.clone(), self.selected_key()) else { return; };
        if self.current_snapshot(&action).as_deref() != Some(snapshot.as_str()) {
            self.action_feedback.insert(key, (tr("request_changed"), true));
            cx.notify();
            return;
        }
        let body = match &action {
            Action::Answer => match self.answer_body(cx) { Some(body) => Some(body), None => return },
            Action::Select(option) => Some(json!({"option": option})),
            _ => None,
        };
        let tool = (action == Action::Answer).then(|| self.chat.ask.as_ref().and_then(|ask| ask.tool_use_id.clone())).flatten();
        if !self.flight.begin(key.clone(), action.clone(), snapshot) { return; }
        if let Some(tool) = tool { self.answering.insert(key.clone(), tool); }
        self.action_feedback.remove(&key);
        let (connection, selection, tx) = (self.connection, self.selection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = if action == Action::Cancel { api.interrupt(&session.name, false).await.map(|_| Value::Null) }
                else { api.act(&session.name, &action.path(), body, matches!(action, Action::Discard(_)), action.seconds()).await };
            let _ = tx.send(Envelope { connection, selection: Some(selection), payload: Payload::Acted(key, action, result) }).await;
        });
        cx.notify();
    }

    fn receive_acted(&mut self, key: SessionKey, action: Action, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.flight.finish(&key, &action) else { return; };
        let answering = if action == Action::Answer { self.answering.remove(&key) } else { None };
        let current = self.selected_key().as_ref() == Some(&key);
        let note = match result {
            Err(error) => {
                if current && matches!(error.status, Some(401 | 403)) { self.open_connection(window, cx); }
                // 5xx pode ter agido lá: mostra o motivo do servidor e a incerteza juntos.
                let text = match (error.uncertain, error.status) {
                    (true, Some(_)) => format!("{} {}", tr(&error.detail), tr("action_uncertain")),
                    (true, None) => tr("action_uncertain"),
                    _ => Self::failure(&error),
                };
                (text, true)
            }
            Ok(value) => match &action {
                Action::Answer => {
                    // O `tool_result` chega depois; sem a marca a pergunta do transcript reabriria nesse meio-tempo.
                    if let Some(tool) = answering { self.answered_tools.insert((key.clone(), tool)); }
                    if current && self.chat.ask.as_ref().is_some_and(|ask| ask.fingerprint == snapshot) {
                        self.chat.ask = None;
                        self.ask_form = AskForm::default();
                    }
                    (tr(if value.get("fallback").and_then(Value::as_bool) == Some(true) { "ask_fallback" } else { "ask_sent" }), false)
                }
                Action::Select(_) | Action::Submit => (tr("option_sent"), false),
                Action::Cancel => (tr("stop_requested"), false),
                Action::Implement => (tr("plan_implement_sent"), false),
                Action::Discard(id) => {
                    if current { self.chat.retire(&format!("queued-{id}")); }
                    (tr("queue_discarded"), false)
                }
                Action::Steer => {
                    let steered: Steered = serde_json::from_value(value).unwrap_or_default();
                    if current { self.chat.steered(steered.promoted, &steered.queued_ids); }
                    if steered.promoted { (tr("queue_promoted"), false) }
                    else if steered.confirmed > 0 { (tr("queue_confirmed").replace("{n}", &steered.confirmed.to_string()), false) }
                    else { (tr("queue_not_promoted"), true) }
                }
            },
        };
        self.action_feedback.insert(key, note);
    }

    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        self.follow_content_changed(cx);
        self.items = conversation::build(&self.chat.events);
        let events = &self.chat.events;
        let mut ids: Vec<_> = self.items.iter().map(|item| item.id(events)).collect();
        let mut signatures: Vec<_> = self.items.iter().map(|item| signature(item, events)).collect();
        if !self.chat.live_thinking.is_empty() { ids.push(LIVE_THINKING.into()); signatures.push(String::new()); }
        if let Some(tool) = &self.chat.live_tool { ids.push(LIVE_TOOL.into()); signatures.push(format!("{}{}", tool.name, tool.input)); }
        if !self.visible_preview.text.is_empty() { ids.push(PREVIEW.into()); signatures.push(String::new()); }
        let prefix = self.row_ids.iter().zip(&ids).take_while(|(a,b)| a == b).count();
        let suffix = self.row_ids[prefix..].iter().rev().zip(ids[prefix..].iter().rev()).take_while(|(a,b)| a == b).count();
        if prefix + suffix < self.row_ids.len() || prefix + suffix < ids.len() {
            self.splice_rows(prefix..self.row_ids.len()-suffix, ids.len()-prefix-suffix);
        }
        // Mesma linha com outro conteúdo (resultado que chegou, grupo que cresceu): altura muda.
        let previous: HashMap<&String, &String> = self.row_ids.iter().zip(&self.row_signatures).collect();
        for (index, (id, signature)) in ids.iter().zip(&signatures).enumerate() {
            if previous.get(id).is_some_and(|old| *old != signature) { self.list_state.remeasure_items(index..index + 1); }
        }
        self.row_ids = ids;
        self.row_signatures = signatures;
        self.last_message = events.iter().rposition(|e| e.kind == "assistant_msg" || e.kind == "user_msg" && !e.queued());
        for (index, id) in self.row_ids.iter().enumerate() {
            let body = if id == PREVIEW { preview_source(&self.visible_preview) }
                else { match self.items.get(index) { Some(Item::Event(i)) => render_source(&events[*i]), _ => continue } };
            if let Some(cached) = self.rich.get_mut(id) {
                if cached.source != body {
                    let added = if id == PREVIEW { body.strip_prefix(&cached.source).map(str::to_owned) } else { None };
                    cached.source = body.clone();
                    if let Some(added) = added {
                        cached.view.update(cx, |view, cx| view.push_str(&added, cx));
                    } else {
                        cached.view.update(cx, |view, cx| view.set_text(&body, cx));
                    }
                    self.list_state.remeasure_items(index..index+1);
                }
            }
        }
        let rows: HashSet<&String> = self.row_ids.iter().collect();
        // Visões fora da lista (plano, diff do painel) usam linha "__…__" e saem só pelo limite do cache.
        self.rich.retain(|_, rich| rows.contains(&rich.row) || rich.row.starts_with("__"));
        let provider = self.provider().0.to_owned();
        if matches!(provider.as_str(), "pi" | "omp" | "kimi") {
            let derived = interaction::ask_from_events(&self.chat.events, &provider)
                .filter(|ask| !ask.tool_use_id.as_deref().is_some_and(|id| self.tool_answered(id)));
            if self.chat.update_ask(derived) { self.ask_form = AskForm::default(); }
        }
    }

    fn text_view(&mut self, key: &str, row: &str, source: String, cx: &mut Context<Self>) -> Entity<TextViewState> {
        self.render_tick += 1;
        if let Some(rich) = self.rich.get_mut(key) {
            rich.touched = self.render_tick;
            if rich.source != source {
                rich.view.update(cx, |view, cx| view.set_text(&source, cx));
                rich.source = source;
            }
            return rich.view.clone();
        }
        if self.rich.len() >= 240 {
            if let Some(old) = self.rich.iter().min_by_key(|(_, v)| v.touched).map(|(id, _)| id.clone()) { self.rich.remove(&old); }
        }
        let view = cx.new(|cx| TextViewState::markdown(&source, cx));
        let owner = row.to_owned();
        let observer = cx.observe(&view, move |this, _, cx| {
            if let Some(i) = this.row_ids.iter().position(|id| id == &owner) { this.follow_content_changed(cx); this.list_state.remeasure_items(i..i+1); cx.notify(); }
        });
        self.rich.insert(key.to_owned(), RichText { source, view: view.clone(), _observer: observer, touched: self.render_tick, row: row.to_owned() });
        view
    }

    fn render_row(&mut self, index: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(id) = self.row_ids.get(index).cloned() else { return div().into_any_element(); };
        let inner = match (id.as_str(), self.items.get(index).cloned()) {
            (PREVIEW, _) => self.render_message(index, &id, cx),
            (LIVE_THINKING, _) => self.render_live_thinking(cx),
            (LIVE_TOOL, _) => self.render_live_tool(),
            (_, Some(Item::Event(_))) => self.render_message(index, &id, cx),
            (_, Some(Item::Tool(tool))) => self.render_tool(tool, &id, cx),
            (_, Some(Item::Orphan(result))) => self.render_orphan(result, &id, cx),
            (_, Some(Item::Group { tools, .. })) => self.render_group(&id, &tools, cx),
            (_, Some(Item::Thinking { parts, .. })) => self.render_thinking(&id, &parts, cx),
            (_, None) => div().into_any_element(),
        };
        let message = id == PREVIEW || matches!(self.items.get(index), Some(Item::Event(_)));
        div().id(SharedString::from(id)).w_full().px(px(36.)).when(message, |el| el.py(px(8.))).when(!message, |el| el.py(px(2.)))
            .flex().justify_center()
            .child(div().w_full().max_w(px(column_width())).child(inner))
            .into_any_element()
    }

    fn disclosure(&self, key: &str, open: bool) -> Button {
        Button::new(SharedString::from(format!("toggle-{key}"))).ghost().small().w_full().toggled(open)
            .icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
    }

    fn tool_status(&self, tool: Tool) -> (String, Hsla) {
        let events = &self.chat.events;
        match tool.result.map(|i| &events[i]) {
            Some(result) if result.is_error == Some(true) => {
                let line = result.result.as_deref().and_then(|r| r.lines().map(str::trim).find(|l| !l.is_empty()));
                (line.map(|l| conversation::one_line(l, 72)).unwrap_or_else(|| tr("tool_failed")), theme::warning())
            }
            Some(result) => {
                let text = result.result.as_deref().unwrap_or("").trim();
                let lines = text.lines().count();
                let label = if text.is_empty() { tr("tool_done") } else if lines == 1 { tr("tool_line") } else { tr("tool_lines").replace("{n}", &lines.to_string()) };
                (label, theme::muted())
            }
            None if self.running(tool.call) => (tr("tool_running"), theme::accent()),
            None => (tr("tool_no_result"), theme::muted()),
        }
    }

    // Sem resultado só é "em execução" enquanto a sessão trabalha e nenhuma mensagem veio depois.
    fn running(&self, call: usize) -> bool {
        self.chat.state.state == "working" && self.last_message.is_none_or(|last| call > last)
    }

    fn render_tool(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let call = &self.chat.events[tool.call];
        let key = call.id.clone();
        let name = call.tool_name.clone().unwrap_or_else(|| tr("tool"));
        let summary = conversation::summarize_input(call.tool_name.as_deref(), call.tool_input.as_ref());
        let input = conversation::pretty_input(call.tool_input.as_ref());
        let (status, status_color) = self.tool_status(tool);
        let error = tool.result.is_some_and(|i| self.chat.events[i].is_error == Some(true));
        let open = self.expanded.contains(&key);
        let toggle_key = key.clone();
        let header = self.disclosure(&key, open)
            .accessibility_label(format!("{name}: {summary}. {status}"))
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::text() }).child(name))
            .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(summary))
            .child(div().flex_shrink_0().max_w(px(320.)).truncate().text_color(status_color).child(status))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let mut body = div().flex().flex_col().gap_2().pl_6().pt_1().pb_2();
        if open {
            if !input.is_empty() { body = body.child(self.detail(row, &format!("{key}:input"), tr("tool_input"), tr("copy_input"), input, false, cx)); }
            match tool.result {
                Some(i) => {
                    let result = self.chat.events[i].result.clone().unwrap_or_default();
                    body = body.child(self.detail(row, &format!("{key}:result"), tr("tool_output"), tr("copy_result"), result, error, cx));
                }
                None => body = body.child(div().text_sm().text_color(theme::muted()).child(self.tool_status(tool).0)),
            }
        }
        div().flex().flex_col().child(header).when(open, |el| el.child(body)).into_any_element()
    }

    fn detail(&mut self, row: &str, key: &str, label: String, copy_label: String, full: String, error: bool, cx: &mut Context<Self>) -> AnyElement {
        let total = full.chars().count();
        let (shown, clipped) = conversation::clip(&full, DETAIL_MAX);
        let view = self.text_view(key, row, conversation::fenced(shown), cx);
        let note = clipped.then(|| tr("clipped").replace("{shown}", &DETAIL_MAX.to_string()).replace("{total}", &total.to_string()));
        div().flex().flex_col().gap_1()
            .child(div().flex().items_center().justify_between()
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::muted() }).child(label))
                .child(Button::new(SharedString::from(format!("copy-{key}"))).ghost().xsmall().icon(IconName::Copy).label(copy_label)
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(full.clone())))))
            .child(TextView::new(&view).selectable(true).scrollable(false))
            .when_some(note, |el, note| el.child(div().text_xs().text_color(theme::muted()).child(note)))
            .into_any_element()
    }

    fn render_orphan(&mut self, index: usize, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let event = &self.chat.events[index];
        let key = event.id.clone();
        let error = event.is_error == Some(true);
        let full = event.result.clone().unwrap_or_default();
        let first = full.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| conversation::one_line(l, 96)).unwrap_or_default();
        let open = self.expanded.contains(&key);
        let toggle_key = key.clone();
        let header = self.disclosure(&key, open)
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::text() }).child(tr("tool_orphan")))
            .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(first))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let detail = open.then(|| self.detail(row, &format!("{key}:result"), tr("tool_output"), tr("copy_result"), full, error, cx));
        div().flex().flex_col().child(header)
            .when_some(detail, |el, detail| el.child(div().pl_6().pt_1().pb_2().child(detail)))
            .into_any_element()
    }

    fn render_group(&mut self, row: &str, tools: &[Tool], cx: &mut Context<Self>) -> AnyElement {
        let events = &self.chat.events;
        let names: Vec<String> = tools.iter().map(|t| events[t.call].tool_name.clone().unwrap_or_else(|| tr("tool"))).collect();
        let mut distinct = names.clone();
        distinct.dedup();
        let label = if distinct.len() == 1 { format!("{} · {}", names[0], tools.len()) } else { tr("tools_count").replace("{n}", &tools.len().to_string()) };
        let summary = if distinct.len() == 1 {
            tools.last().map(|t| conversation::summarize_input(events[t.call].tool_name.as_deref(), events[t.call].tool_input.as_ref())).unwrap_or_default()
        } else { conversation::one_line(&distinct.join(", "), 96) };
        let errors = tools.iter().filter(|t| t.result.is_some_and(|i| events[i].is_error == Some(true))).count();
        let running = tools.iter().any(|t| t.result.is_none() && self.running(t.call));
        let (status, color) = if errors > 0 { (tr("tools_errors").replace("{n}", &errors.to_string()), theme::warning()) }
            else if running { (tr("tool_running"), theme::accent()) }
            else { (String::new(), theme::muted()) };
        let open = self.expanded.contains(row);
        let toggle_key = row.to_owned();
        let header = self.disclosure(row, open)
            .accessibility_label(format!("{label}: {summary}. {status}"))
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(label))
            .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(summary))
            .child(div().flex_shrink_0().text_color(color).child(status))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let children: Vec<AnyElement> = if open { tools.iter().map(|&tool| self.render_tool(tool, row, cx)).collect() } else { Vec::new() };
        div().flex().flex_col().child(header)
            .when(open, |el| el.child(div().flex().flex_col().pl_5().children(children)))
            .into_any_element()
    }

    fn render_thinking(&mut self, row: &str, parts: &[usize], cx: &mut Context<Self>) -> AnyElement {
        let events = &self.chat.events;
        let thoughts: Vec<&ChatEvent> = parts.iter().map(|&i| &events[i]).filter(|e| e.kind == "thinking").collect();
        let summary = conversation::thought_summary(thoughts.first().and_then(|e| e.text.as_deref()).unwrap_or(""));
        let full = thoughts.iter().filter_map(|e| e.text.as_deref()).collect::<Vec<_>>().join("\n\n");
        let searches = parts.len() - thoughts.len();
        let open = self.expanded.contains(row);
        let toggle_key = row.to_owned();
        let header = self.disclosure(row, open)
            .accessibility_label(format!("{}: {summary}", tr("thinking")))
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr("thinking")))
            .child(div().flex_1().min_w_0().truncate().italic().text_color(theme::muted()).child(summary))
            .when(searches > 0, |el| el.child(div().flex_shrink_0().text_color(theme::muted())
                .child(if searches == 1 { tr("thinking_search") } else { tr("thinking_searches").replace("{n}", &searches.to_string()) })))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let mut body: Vec<AnyElement> = Vec::new();
        if open {
            let paired = if searches > 0 { conversation::pair_results(&self.chat.events).0 } else { HashMap::new() };
            for &i in parts {
                let event = &self.chat.events[i];
                if event.kind == "thinking" {
                    let key = format!("{}:thought", event.id);
                    let view = self.text_view(&key, row, safe_markdown(event.text.as_deref().unwrap_or("")), cx);
                    body.push(TextView::new(&view).selectable(true).scrollable(false).text_color(theme::muted()).into_any_element());
                } else {
                    body.push(self.render_tool(Tool { call: i, result: paired.get(&i).copied() }, row, cx));
                }
            }
            body.push(div().flex().child(Button::new(SharedString::from(format!("copy-{row}"))).ghost().xsmall().icon(IconName::Copy).label(tr("copy"))
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(full.clone())))).into_any_element());
        }
        div().flex().flex_col().child(header)
            .when(open, |el| el.child(div().flex().flex_col().gap_2().pl_6().pt_1().pb_2().children(body)))
            .into_any_element()
    }

    fn render_live_thinking(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let text = &self.chat.live_thinking;
        let start = text.char_indices().rev().nth(LIVE_THINKING_TAIL).map(|(i, _)| i).unwrap_or(0);
        let source = safe_markdown(&text[start..]);
        let view = self.text_view(LIVE_THINKING, LIVE_THINKING, source, cx);
        div().flex().flex_col().gap_1().px_3().py_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::accent()).child(tr("thinking_live")))
            .child(TextView::new(&view).selectable(true).scrollable(false).text_sm().text_color(theme::muted()))
            .into_any_element()
    }

    fn render_live_tool(&self) -> AnyElement {
        let Some(tool) = &self.chat.live_tool else { return div().into_any_element(); };
        let summary = conversation::summarize_input(Some(&tool.name), Some(&tool.input));
        div().flex().items_center().gap_2().px_3().py_1().text_sm()
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).child(tool.name.clone()))
            .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(summary))
            .child(div().flex_shrink_0().text_color(theme::accent()).child(tr("tool_running")))
            .into_any_element()
    }

    fn sync_ask_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fingerprint = self.chat.ask.as_ref().map(|ask| ask.fingerprint.clone()).unwrap_or_default();
        if fingerprint == self.ask_form.fingerprint { return; }
        let mut form = AskForm { fingerprint, ..Default::default() };
        if let Some(ask) = &self.chat.ask {
            for item in &ask.payload.questions {
                form.picks.push(Pick::Empty);
                form.typing.push(ask.codex() && item.options.is_empty());
                let secret = item.is_secret;
                let input = cx.new(|cx| InputState::new(window, cx).masked(secret).placeholder(tr("ask_placeholder")));
                // O botão Enviar depende do texto: cada edição redesenha o cartão.
                form._changes.push(cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()));
                form.inputs.push(input);
            }
        }
        self.ask_form = form;
    }

    fn set_pick(&mut self, fingerprint: &str, index: usize, pick: Option<Pick>, typing: bool, cx: &mut Context<Self>) {
        if self.ask_form.fingerprint != fingerprint || index >= self.ask_form.picks.len() { return; }
        if let Some(pick) = pick { self.ask_form.picks[index] = pick; }
        self.ask_form.typing[index] = typing;
        cx.notify();
    }

    fn render_ask(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.sync_ask_form(window, cx);
        let ask = self.chat.ask.clone()?;
        let fingerprint = ask.fingerprint.clone();
        let total = ask.payload.questions.len();
        let mut body = div().flex().flex_col().gap_4();
        for (qi, item) in ask.payload.questions.iter().enumerate() {
            let pick = self.ask_form.picks.get(qi).cloned().unwrap_or(Pick::Empty);
            let typing = self.ask_form.typing.get(qi) == Some(&true);
            let chosen = |i: usize| !typing && matches!(&pick, Pick::Options(list) if list.contains(&i));
            let mut options = div().flex().flex_col().gap_2();
            for (oi, option) in item.options.iter().enumerate() {
                let (fp, current, entry) = (fingerprint.clone(), pick.clone(), item.clone());
                let on_pick = cx.listener(move |this, _: &bool, _, cx| {
                    let next = interaction::toggle(&entry, &current, oi);
                    this.set_pick(&fp, qi, Some(next), false, cx);
                });
                let id = SharedString::from(format!("ask-{qi}-{oi}"));
                let control = if item.multi_select { Checkbox::new(id).label(option.label.clone()).checked(chosen(oi)).disabled(busy).on_click(on_pick).into_any_element() }
                    else { Radio::new(id).label(option.label.clone()).checked(chosen(oi)).disabled(busy).on_click(on_pick).into_any_element() };
                options = options.child(div().flex().flex_col().gap_1().child(control)
                    .when(!option.description.is_empty(), |el| el.child(div().pl_6().text_xs().text_color(theme::muted()).child(option.description.clone())))
                    .when_some(option.preview.clone().filter(|p| !p.is_empty()), |el, preview| el.child(div().ml_6().p_2().rounded_md().bg(theme::raised())
                        .font_family(crate::theme::MONO).text_xs().whitespace_nowrap().overflow_x_hidden().child(preview))));
            }
            let mut escapes = div().flex().flex_wrap().gap_2();
            if ask.allows_text(item) && !item.options.is_empty() {
                let fp = fingerprint.clone();
                escapes = escapes.child(Button::new(SharedString::from(format!("ask-type-{qi}"))).small().ghost().toggled(typing).disabled(busy)
                    .label(tr("ask_type")).on_click(cx.listener(move |this, _, window, cx| {
                        let next = !this.ask_form.typing.get(qi).copied().unwrap_or(false);
                        this.set_pick(&fp, qi, None, next, cx);
                        if next { if let Some(input) = this.ask_form.inputs.get(qi).cloned() { input.update(cx, |input, cx| input.focus(window, cx)); } }
                    })));
            }
            if ask.allows_chat() {
                let fp = fingerprint.clone();
                escapes = escapes.child(Button::new(SharedString::from(format!("ask-chat-{qi}"))).small().ghost().toggled(!typing && pick == Pick::Chat).disabled(busy)
                    .label(tr("ask_chat")).on_click(cx.listener(move |this, _, _, cx| this.set_pick(&fp, qi, Some(Pick::Chat), false, cx))));
            }
            let input = typing.then(|| self.ask_form.inputs.get(qi).cloned()).flatten();
            body = body.child(div().flex().flex_col().gap_2()
                .when(!item.header.is_empty(), |el| el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted())
                    .child(if total > 1 { format!("{} · {}/{}", item.header, qi + 1, total) } else { item.header.clone() })))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(item.question.clone()))
                .when(item.multi_select, |el| el.child(div().text_xs().text_color(theme::muted()).child(tr("ask_multi"))))
                .child(options)
                .when_some(input, |el, input| el.child(Input::new(&input).disabled(busy)))
                .when(item.is_secret, |el| el.child(div().text_xs().text_color(theme::muted()).child(tr("ask_secret"))))
                .child(escapes));
        }
        let ready = self.answer_body(cx).is_some();
        let fp = fingerprint.clone();
        let sending = busy && self.selected_key().and_then(|key| self.flight.running(&key).cloned()) == Some(Action::Answer);
        if self.ask_scroll.0 != fingerprint { self.ask_scroll = (fingerprint.clone(), ScrollHandle::new()); }
        Some(self.interaction_card(tr("ask_title"), scrolled("ask-scroll", &self.ask_scroll.1, 360., body),
            div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().text_xs().text_color(theme::muted()).child(tr(if ready { "ask_ready" } else { "ask_incomplete" })))
                .child(Button::new("ask-send").primary().label(tr(if sending { "sending" } else { "ask_send" })).disabled(busy || !ready)
                    .on_click(cx.listener(move |this, _, _, cx| this.act(Action::Answer, fp.clone(), cx))))
                .into_any_element()))
    }

    fn render_options(&mut self, busy: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = &self.chat.state;
        if self.chat.ask.is_some() || state.state != "awaiting_input" { return None; }
        let (question, options) = (state.question.clone()?, state.options.clone().filter(|o| !o.is_empty())?);
        let snapshot = select_snapshot(state);
        let plan = state.claude_plan_pending.clone().filter(|p| !p.plan.trim().is_empty());
        let multi = options.iter().any(|o| interaction::checkbox(o).is_some());
        let marked = options.iter().filter(|o| interaction::checkbox(o).is_some_and(|(on, _)| on)).count();
        let mut body = div().flex().flex_col().gap_2();
        if let Some(plan) = plan {
            let source = safe_markdown(&plan.plan);
            let view = match &self.plan_view {
                Some((cached, view)) if *cached == source => view.clone(),
                _ => {
                    let view = cx.new(|cx| TextViewState::markdown(&source, cx));
                    self.plan_view = Some((source, view.clone()));
                    view
                }
            };
            let source = self.plan_view.as_ref().map(|(source, _)| source.clone()).unwrap_or_default();
            if self.plan_scroll.0 != source { self.plan_scroll = (source, ScrollHandle::new()); }
            body = body.child(div().rounded_md().bg(theme::raised())
                .child(scrolled("plan-scroll", &self.plan_scroll.1, 320., div().p_3().child(TextView::new(&view).selectable(true).scrollable(false)))))
                .when_some(plan.path, |el, path| el.child(div().text_xs().text_color(theme::muted()).child(path)));
        }
        body = body.child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(question));
        for (i, option) in options.iter().enumerate() {
            let label = match interaction::checkbox(option) { Some((_, rest)) if multi => rest.to_owned(), _ => option.clone() };
            let on = multi && interaction::checkbox(option).is_some_and(|(on, _)| on);
            let snap = snapshot.clone();
            let id = SharedString::from(format!("option-{i}"));
            let label = format!("{}. {label}", i + 1);
            // Marcar no terminal é um /select por toque; o envio das marcadas é outro botão.
            body = body.child(if multi {
                Checkbox::new(id).label(label).checked(on).disabled(busy)
                    .on_click(cx.listener(move |this, _: &bool, _, cx| this.act(Action::Select(i + 1), snap.clone(), cx))).into_any_element()
            } else {
                Button::new(id).w_full().disabled(busy).label(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.act(Action::Select(i + 1), snap.clone(), cx))).into_any_element()
            });
        }
        let snap = snapshot.clone();
        let footer = div().flex().items_center().gap_2().child(div().flex_1())
            .child(Button::new("option-cancel").ghost().label(tr("cancel")).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.act(Action::Cancel, String::new(), cx))))
            .when(multi, |el| el.child(Button::new("option-submit").primary().disabled(busy || marked == 0)
                .label(tr("options_submit").replace("{n}", &marked.to_string()))
                .on_click(cx.listener(move |this, _, _, cx| this.act(Action::Submit, snap.clone(), cx)))));
        let title = if self.chat.state.claude_plan_pending.is_some() { tr("plan_pending_title") } else { tr("options_title") };
        Some(self.interaction_card(title, body.into_any_element(), footer.into_any_element()))
    }

    fn render_plan_bar(&mut self, busy: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, plan) = self.codex_plan()?;
        if self.plans_dismissed.contains(&id) || self.chat.ask.is_some() { return None; }
        let ready = self.chat.state.state == "idle" && self.queued_count() == 0;
        let dismiss = id.clone();
        Some(div().py_2().flex().items_center().gap_2().border_t_1().border_color(theme::border())
            .child(div().flex_1().min_w_0().text_sm().child(tr(if ready { "codex_plan_title" } else { "codex_plan_wait" })))
            .child(Button::new("plan-dismiss").small().ghost().label(tr("codex_plan_dismiss"))
                .on_click(cx.listener(move |this, _, _, cx| { this.plans_dismissed.insert(dismiss.clone()); cx.notify(); })))
            .child(Button::new("plan-implement").small().primary().label(tr("codex_plan_implement")).disabled(busy || !ready)
                .on_click(cx.listener(move |this, _, _, cx| this.act(Action::Implement, plan.clone(), cx))))
            .into_any_element())
    }

    fn interaction_card(&self, title: String, body: AnyElement, footer: AnyElement) -> AnyElement {
        // Pedido que espera você: moldura âmbar suave, como `.ask` do mock.
        div().px(px(36.)).py_2().flex().justify_center()
            .child(div().w_full().max_w(px(column_width())).p(px(14.)).rounded(px(14.)).border_1().border_color(theme::warning().opacity(0.35))
                .bg(theme::warning().opacity(0.06)).flex().flex_col().gap(px(10.))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(body).child(footer))
            .into_any_element()
    }

    fn render_refs(&mut self, row: &str, refs: Vec<(Source, String, bool)>, cx: &mut Context<Self>) -> AnyElement {
        let key = self.selected_key();
        let mut list = div().flex().flex_col().gap_2();
        for (n, (source, name, image)) in refs.into_iter().enumerate() {
            let preview = if image {
                self.ensure_media(&source);
                let state = key.as_ref().and_then(|key| self.media.get(&(key.clone(), source.clone())));
                Some(match state {
                    Some(MediaState::Image(picture)) => div().max_w(px(320.)).max_h(px(240.)).rounded_md().overflow_hidden()
                        .child(img(picture.clone()).max_w(px(320.)).max_h(px(240.)).object_fit(ObjectFit::Contain)).into_any_element(),
                    Some(MediaState::Failed(reason)) => div().text_xs().text_color(theme::warning())
                        .child(tr("media_failed").replace("{name}", &name).replace("{reason}", reason)).into_any_element(),
                    _ => div().w(px(160.)).h(px(96.)).rounded_md().bg(theme::raised()).flex().items_center().justify_center()
                        .text_xs().text_color(theme::muted()).child(tr("media_loading")).into_any_element(),
                })
            } else { None };
            let (open_source, open_name) = (source.clone(), name.clone());
            let (save_source, save_name) = (source, name.clone());
            let openable = composer::openable(&composer::safe_name(&name));
            list = list.child(div().flex().flex_col().gap_1()
                .when_some(preview, |el, preview| el.child(preview))
                .child(div().flex().items_center().gap_2().text_sm()
                    .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(name))
                    .when(openable, |el| el.child(Button::new(SharedString::from(format!("open-{row}-{n}"))).ghost().xsmall().label(tr("open"))
                        .on_click(cx.listener(move |this, _, _, cx| this.keep_file(open_source.clone(), open_name.clone(), true, cx)))))
                    .child(Button::new(SharedString::from(format!("save-{row}-{n}"))).ghost().xsmall().label(tr("save"))
                        .on_click(cx.listener(move |this, _, _, cx| this.keep_file(save_source.clone(), save_name.clone(), false, cx))))));
        }
        list.into_any_element()
    }

    fn render_attachments(&self, key: &SessionKey, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list = self.attachments.get(key).filter(|list| !list.is_empty())?;
        let busy = self.uploading.contains_key(key);
        let tiles = list.iter().map(|attachment| {
            let id = attachment.id;
            let (status, color) = match &attachment.state {
                AttachState::Waiting => (if busy { tr("attach_waiting") } else { String::new() }, theme::muted()),
                AttachState::Uploading => (tr("attach_uploading"), theme::accent()),
                AttachState::Uploaded(_) => (tr("attach_uploaded"), theme::muted()),
                AttachState::Failed(reason) => (reason.clone(), theme::warning()),
            };
            // Miniatura de 56 como no web; o nome e o estado ficam na dica e na linha de baixo.
            let waiting = matches!(attachment.state, AttachState::Waiting) && busy;
            let failed = matches!(attachment.state, AttachState::Failed(_));
            let tile = div().relative().flex_shrink_0().size(px(56.)).rounded(px(12.)).bg(theme::inset()).border_1()
                .border_color(match &attachment.state { AttachState::Uploading => theme::accent(), AttachState::Failed(_) => theme::danger(), _ => theme::border() })
                .when(waiting, |el| el.opacity(0.45))
                .child(match attachment.image.clone() {
                    Some(picture) => div().size_full().rounded(px(11.)).overflow_hidden().child(img(picture).size_full().object_fit(ObjectFit::Cover)).into_any_element(),
                    None => div().size_full().flex().flex_col().items_center().justify_center().gap_1().px_1()
                        .child(chrome::small_icon(IconName::File, 18., theme::muted()))
                        .child(div().w_full().text_center().truncate().font_family(crate::theme::MONO).text_size(px(9.)).text_color(theme::faint()).child(attachment.name.clone()))
                        .into_any_element(),
                })
                .child(div().absolute().top(px(-7.)).right(px(-7.)).child(Button::new(SharedString::from(format!("remove-attachment-{id}")))
                    .custom(ButtonCustomVariant::new(cx).color(theme::inset()).foreground(theme::muted()).hover(theme::raised()).active(theme::raised())).bg(theme::inset())
                    .icon(chrome::small_icon(IconName::Close, 12., theme::muted())).size(px(20.)).rounded_full().border_1().border_color(theme::border_strong())
                    .accessibility_label(tr("attach_remove").replace("{name}", &attachment.name)).tooltip(tr("attach_remove").replace("{name}", &attachment.name)).disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.remove_attachment(id, window, cx)))));
            (tile, (!status.is_empty() && (failed || matches!(attachment.state, AttachState::Uploading))).then(|| (attachment.name.clone(), status, color)))
        });
        let (tiles, notes): (Vec<_>, Vec<_>) = tiles.unzip();
        Some(div().flex().flex_col().gap_1()
            .child(div().flex().flex_wrap().gap(px(14.)).pt(px(7.)).px(px(2.)).pb(px(2.)).children(tiles))
            .children(notes.into_iter().flatten().map(|(name, status, color)| div().text_xs().text_color(color).truncate().child(format!("{name}: {status}"))))
            .into_any_element())
    }

    fn render_recent(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let recent = self.recent.as_ref().filter(|recent| Some(&recent.key) == self.selected_key().as_ref())?;
        let body = match &recent.files {
            None => div().text_sm().text_color(theme::muted()).child(tr("recent_loading")).into_any_element(),
            Some(Err(reason)) => div().text_sm().text_color(theme::warning()).child(reason.clone()).into_any_element(),
            Some(Ok(files)) if files.is_empty() => div().text_sm().text_color(theme::muted()).child(tr("recent_empty")).into_any_element(),
            Some(Ok(files)) => div().flex().flex_col().children(files.iter().enumerate().map(|(n, file)| {
                let name = file.filename.clone();
                Button::new(SharedString::from(format!("recent-{n}"))).ghost().small().w_full()
                    .child(div().flex_1().min_w_0().truncate().child(file.filename.clone()))
                    .child(div().flex_shrink_0().text_xs().text_color(theme::muted()).child(human_size(file.size)))
                    .on_click(cx.listener(move |this, _, _, cx| this.reattach(name.clone(), cx)))
            })).into_any_element(),
        };
        Some(div().p_3().rounded_md().bg(theme::raised()).flex().flex_col().gap_2()
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr("recent_title")))
            .child(div().id("recent-list").max_h(px(220.)).overflow_y_scroll().child(body))
            .into_any_element())
    }

    fn render_command_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.command_search.read(cx).value().to_lowercase();
        let cache = self.commands_key().and_then(|key| self.commands.get(&key));
        let body = match cache {
            None => div().text_sm().text_color(theme::muted()).child(tr("commands_loading")).into_any_element(),
            Some(Err(reason)) => div().flex().items_center().gap_2().text_sm().text_color(theme::warning())
                .child(div().flex_1().min_w_0().child(tr("commands_failed").replace("{reason}", reason)))
                .child(Button::new("commands-retry").ghost().xsmall().flex_shrink_0().label(tr("retry")).on_click(cx.listener(|this, _, _, cx| { this.ensure_commands(true); cx.notify(); })))
                .into_any_element(),
            Some(Ok(list)) => {
                let matches: Vec<&CommandInfo> = list.iter().filter(|c| query.is_empty() || c.name.to_lowercase().contains(&query)
                    || c.description.as_deref().is_some_and(|d| d.to_lowercase().contains(&query))).collect();
                if matches.is_empty() { div().text_sm().text_color(theme::muted()).child(tr("commands_empty")).into_any_element() }
                else {
                    let mut groups = div().flex().flex_col().gap_2();
                    for source in ["builtin", "skill", "plugin"] {
                        let items: Vec<&&CommandInfo> = matches.iter().filter(|c| c.source == source || source == "plugin" && !matches!(c.source.as_str(), "builtin" | "skill")).collect();
                        if items.is_empty() { continue; }
                        let group = tr(&format!("commands_{source}"));
                        groups = groups.child(div().pt_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(group.clone()));
                        // Linha do web: nome em mono e descrição embaixo; argumentos e selo da origem à direita.
                        for command in items {
                            let picked = (*command).clone();
                            let description = command.description.clone().unwrap_or_default();
                            groups = groups.child(Button::new(SharedString::from(format!("command-{}", command.name))).ghost().w_full().h_auto()
                                .px_3().py_2().rounded(px(8.))
                                .child(div().w_full().flex().items_center().gap_3()
                                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                                        .child(div().truncate().font_family(crate::theme::MONO).text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text())
                                            .child(format!("/{}", command.name)))
                                        .when(!description.is_empty(), |el| el.child(div().truncate().text_xs().text_color(theme::faint()).child(description))))
                                    .when_some(command.argument_hint.clone(), |el, hint| el.child(div().flex_shrink_0().max_w(px(220.)).truncate()
                                        .font_family(crate::theme::MONO).text_xs().text_color(theme::faint()).child(hint)))
                                    .when(command.destructive, |el| el.child(div().flex_shrink_0().text_xs().text_color(theme::warning()).child(tr("command_destructive"))))
                                    .child(div().flex_shrink_0().px(px(6.)).py(px(1.)).rounded(px(6.)).bg(theme::inset()).border_1().border_color(theme::border())
                                        .text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::faint()).child(group.to_uppercase())))
                                .on_click(cx.listener(move |this, _, window, cx| this.pick_command(picked.clone(), true, window, cx))));
                        }
                    }
                    groups.into_any_element()
                }
            }
        };
        div().p_4().rounded_md().bg(theme::raised()).flex().flex_col().gap_3()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(tr("commands")))
            .child(Input::new(&self.command_search))
            .child(div().id("command-list").max_h(px(360.)).overflow_y_scroll().child(body))
            .into_any_element()
    }

    fn render_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm.clone()?;
        let (text, action) = match &confirm {
            // Só o Claude com terminal é interrompido por Esc; Codex e sessão sem terminal param pelo backend.
            Confirm::Stop => {
                let (provider, headless) = self.provider();
                (tr(if provider == "codex" || headless { "stop_confirm_direct" } else { "stop_confirm" }), tr("stop"))
            }
            Confirm::Shortcut(label, _) => (tr("shortcut_confirm").replace("{label}", label), tr("shortcut_run")),
            Confirm::Reload => (tr("reload_confirm"), tr("reload")),
            Confirm::Destructive(text) => (tr("command_confirm").replace("{cmd}", text.split_whitespace().next().unwrap_or(text)), tr("send")),
            Confirm::Replace(name) => (tr("command_replace").replace("{cmd}", &format!("/{name}")), tr("command_replace_ok")),
            Confirm::Prefill(text) => (tr("prefill_replace").replace("{text}", &conversation::one_line(text, 60)), tr("command_replace_ok")),
        };
        Some(div().p_3().rounded_md().border_1().border_color(theme::warning()).flex().items_center().gap_2()
            .child(div().flex_1().min_w_0().text_sm().child(text))
            .child(Button::new("confirm-cancel").small().ghost().label(tr("cancel")).on_click(cx.listener(|this, _, window, cx| {
                this.confirm = None;
                this.composer.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            })))
            .child(Button::new("confirm-ok").small().primary().label(action).on_click(cx.listener(move |this, _, window, cx| match &confirm {
                Confirm::Stop => this.interrupt(window, cx),
                Confirm::Destructive(text) => {
                    // Só vale para o texto que a pessoa viu no aviso.
                    if this.composer.read(cx).value().as_ref() == text { this.submit(false, true, window, cx); }
                    else { this.confirm = None; cx.notify(); }
                }
                Confirm::Replace(name) => { this.confirm = None; this.fill_command(&name.clone(), false, window, cx); }
                Confirm::Prefill(text) => { this.confirm = None; this.prefill(&text.clone(), false, window, cx); }
                Confirm::Shortcut(_, shortcut) => { this.confirm = None; this.run_shortcut(shortcut.clone(), true, window, cx); }
                Confirm::Reload => { this.confirm = None; this.reload(cx); }
            })))
            .into_any_element())
    }

    fn render_suggestions(&self, suggestions: &[CommandInfo], cx: &mut Context<Self>) -> AnyElement {
        let active = self.suggest_pick.min(suggestions.len().saturating_sub(1));
        div().flex().flex_col().rounded_md().bg(theme::raised()).p_1()
            .children(suggestions.iter().enumerate().map(|(n, command)| {
                let picked = command.clone();
                Button::new(SharedString::from(format!("suggest-{}", command.name))).ghost().small().w_full().selected(n == active)
                    .child(div().flex_shrink_0().font_family(crate::theme::MONO).child(format!("/{}", command.name)))
                    .when_some(command.argument_hint.clone(), |el, hint| el.child(div().flex_shrink_0().text_xs().text_color(theme::muted()).child(hint)))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme::muted()).child(command.description.clone().unwrap_or_default()))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_command(picked.clone(), false, window, cx)))
            }))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_composer(&mut self, readable: bool, busy: bool, steer: bool, queued: usize, sending: bool, stopping: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = self.selected_key();
        let text = self.composer.read(cx).value().to_string();
        let uploading = key.as_ref().and_then(|key| self.uploading.get(key)).map(|batch| {
            let done = key.as_ref().and_then(|key| self.attachments.get(key)).map(|list| list.iter()
                .filter(|a| batch.contains(&a.id) && matches!(a.state, AttachState::Uploaded(_))).count()).unwrap_or(0);
            (done, batch.len())
        });
        let attached = key.as_ref().is_some_and(|key| self.attachments.get(key).is_some_and(|list| !list.is_empty()));
        let has_input = !text.trim().is_empty() || attached;
        let suggestions = if readable { self.visible_suggestions(cx) } else { Vec::new() };
        if self.suggest_pick >= suggestions.len() { self.suggest_pick = 0; }
        let tray = key.as_ref().and_then(|key| self.render_attachments(key, cx));
        let recent = self.render_recent(cx);
        let panel = (readable && self.command_panel).then(|| self.render_command_panel(cx));
        let confirm = self.render_confirm(cx);
        let (pills, mode) = self.render_ctl_pills(readable, cx);
        let ctl_panel = if readable { self.render_ctl_panel(cx) } else { None };
        let (provider, headless) = self.provider();
        let provider = provider.to_owned();
        let provider = provider.as_str();
        // A dica do terminal e o destinatário moram no placeholder, como no web.
        let placeholder = if readable && !self.terminal_suggestion.is_empty() {
            tr("terminal_suggestion").replace("{text}", &conversation::one_line(&self.terminal_suggestion, 120))
        } else { tr("composer").replace("{agent}", agent_name(provider)) };
        if self.composer_placeholder != placeholder {
            self.composer_placeholder = placeholder.clone();
            self.composer.update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
        }
        let steer_text = readable && has_input && (provider == "codex" || headless) && self.chat.state.state == "working";
        let blocked = sending || uploading.is_some() || !self.chat_online || !self.history_installed;
        let can_stop = self.can_interrupt();
        let focused = self.composer.read(cx).focus_handle(cx).is_focused(window);
        let paste_target = cx.entity().downgrade();
        let textarea = Textarea::new(&self.composer).appearance(false).disabled(!readable).on_paste(move |item, _, cx| {
            paste_target.update(cx, |this, cx| this.paste(item, cx)).unwrap_or(false)
        });
        let field = div().id("composer-field").text_base()
            // Teclas do campo que o compositor intercepta; sem uso aqui, seguem para o editor.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_suggestion(-1, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_suggestion(1, cx)))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| this.tab(window, cx)))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| this.escape(cx)))
            .child(textarea);

        // Painéis do compositor flutuam sobre a conversa, presos à borda de cima, como os popovers do web.
        let narrow = |el: Option<AnyElement>| el.map(|el| chrome::popover(el, true));
        let wide = |el: Option<AnyElement>| el.map(|el| chrome::popover(el, false));
        let floating: Vec<AnyElement> = [wide(confirm), narrow(ctl_panel), wide(panel), narrow(recent)].into_iter().flatten()
            .chain((!suggestions.is_empty()).then(|| chrome::popover(self.render_suggestions(&suggestions, cx), false)))
            .collect();
        let status = self.status();
        let repo = status.as_ref().and_then(|s| s.repo.clone()).filter(|_| readable);
        let ctx_pct = status.as_ref().and_then(|s| s.ctx_pct).filter(|_| readable);
        let queue_chip = (steer && queued > 0).then(|| Button::new("steer").ghost().xsmall().disabled(busy)
            .accessibility_label(tr("queue_send_now").replace("{n}", &queued.to_string()))
            .child(div().flex().items_center().gap_1().text_xs().text_color(theme::accent())
                .child("⏳").child(div().font_weight(FontWeight::SEMIBOLD).child(tr("queue_count").replace("{n}", &queued.to_string())))
                .child("·").child(div().underline().child(tr("queue_send_action"))))
            .on_click(cx.listener(|this, _, _, cx| this.act(Action::Steer, String::new(), cx))));
        // Fila acima do cartão, como a linha "Na fila" do mock.
        let queue_row = queue_chip.map(|chip| div().mb(px(8.)).px(px(12.)).py(px(4.)).flex().items_center().gap_2().rounded(px(10.))
            .border_1().border_color(theme::border_strong()).text_size(px(12.5)).text_color(theme::muted())
            .child(chrome::small_icon(IconName::List, 14., theme::faint())).child(chip));
        let session = self.selected.clone().filter(|_| readable);
        let footer = (repo.is_some() || ctx_pct.is_some() || session.is_some()).then(|| {
            let branch = status.as_ref().and_then(|s| s.branch.clone()).or_else(|| session.as_ref().and_then(|s| s.branch.clone())).unwrap_or_default();
            let dirty = status.as_ref().and_then(|s| s.dirty) == Some(true);
            let (added, removed) = session.as_ref().map(|s| (s.git_added.filter(|n| *n > 0), s.git_removed.filter(|n| *n > 0))).unwrap_or((None, None));
            let folder = repo.clone().or_else(|| session.as_ref().and_then(folder_name));
            let cost = status.as_ref().and_then(|s| s.cost_usd).map(side::money);
            let stats = self.stats.as_ref().filter(|_| readable).map(side::stats_line);
            let right = [ctx_pct.map(|p| tr("composer_ctx").replace("{n}", &p.round().to_string())), cost].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            div().pt(px(7.)).px(px(6.)).flex().items_center().gap(px(6.)).text_xs().text_color(theme::faint())
                .when_some(folder, |el, f| el.child(chrome::small_icon(IconName::Folder, 14., theme::faint())).child(div().max_w(px(200.)).truncate().child(f)))
                .when(!branch.is_empty(), |el| el.child(div().ml(px(4.)).flex().items_center().gap(px(4.)).min_w_0()
                    .child(chrome::small_icon(IconName::GitBranch, 14., theme::faint()))
                    .child(div().max_w(px(160.)).truncate().child(branch))
                    .when(dirty, |el| el.child(div().text_color(theme::warning()).child("*")))))
                .when_some(added, |el, a| el.child(div().text_color(theme::success()).child(format!("+{a}"))))
                .when_some(removed, |el, r| el.child(div().text_color(theme::removed()).child(format!("−{r}"))))
                .child(div().flex_1())
                // A linha de estatísticas do turno fica na dica: o rodapé mostra só contexto e custo, como no mock.
                .when(!right.is_empty(), |el| el.child(div().id("composer-ctx").flex_shrink_0().child(right)
                    .when_some(stats, |el, line| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(line.clone()).build(window, cx)))))
        });

        let send_label = tr(if sending || uploading.is_some() { "sending" } else { "send" });
        let action = if can_stop && !has_input {
            Button::new("stop").custom(ButtonCustomVariant::new(cx).color(theme::elevated()).foreground(theme::danger()).hover(theme::raised()).active(theme::raised()))
                .bg(theme::elevated()).child(div().size(px(10.)).rounded(px(2.)).bg(theme::danger())).size(px(30.)).rounded_full()
                .tooltip(tr("stop_hint")).accessibility_label(tr("stop")).disabled(stopping)
                .on_click(cx.listener(|this, _, _, cx| this.request_stop(cx)))
        } else {
            let enabled = !blocked && has_input;
            // Colado: botão claro com a seta na cor do fundo; caixa solta: destaque, como nos mocks.
            let (fill, ink) = match (enabled, theme::is_floating()) {
                (false, _) => (theme::raised(), theme::faint()),
                (true, true) => (theme::accent(), theme::on_accent()),
                (true, false) => (theme::text(), theme::background()),
            };
            Button::new("send").custom(ButtonCustomVariant::new(cx).color(fill).foreground(ink).hover(fill.opacity(0.88)).active(fill.opacity(0.8)))
                .bg(fill).icon(chrome::small_icon(IconName::ArrowUp, 16., ink))
                .size(px(30.)).rounded_full().tooltip(tr("send_hint")).accessibility_label(send_label).disabled(!enabled)
                .on_click(cx.listener(|this, _, window, cx| this.submit(false, false, window, cx)))
        };
        let commands = chrome::icon_button("commands", IconName::SquareSlash, tr("commands"), cx).disabled(!readable).selected(self.command_panel)
            .on_click(cx.listener(|this, _, window, cx| {
                this.command_panel = !this.command_panel;
                if this.command_panel {
                    this.close_controls();
                    this.recent = None;
                    this.ensure_commands(false);
                    this.command_search.update(cx, |input, cx| { input.set_value("", window, cx); input.focus(window, cx); });
                }
                cx.notify();
            }));
        let attach = chrome::icon_button("attach", IconName::Paperclip, tr("attach"), cx).disabled(!readable || uploading.is_some())
            .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)));
        let recent_btn = chrome::icon_button("attach-recent", IconName::RotateCcwClock, tr("attach_recent"), cx).disabled(!readable || uploading.is_some())
            .selected(self.recent.is_some()).on_click(cx.listener(|this, _, _, cx| this.open_recent(cx)));
        let control_row = div().flex().items_center().gap_1()
            .child(commands).child(attach).child(recent_btn)
            .child(div().flex_1())
            .when(steer_text, |el| el.child(chrome::pill_button("steer-text", cx).label(tr("steer_text")).disabled(blocked)
                .on_click(cx.listener(|this, _, window, cx| this.submit(true, false, window, cx)))))
            .children(pills)
            .children(mode)
            .child(div().ml(px(6.)).child(action));
        let card = div().relative().flex().flex_col().gap(px(10.)).pt(px(12.)).pr(px(12.)).pb(px(10.)).pl(px(16.)).rounded(px(18.)).border_1()
            .border_color(if focused { theme::accent_focus() } else { theme::border_strong() }).bg(theme::boxed()).shadow(theme::card_shadow())
            .when_some(tray, |el, tray| el.child(tray))
            .when_some(uploading, |el, (done, total)| el.child(div().text_xs().text_color(theme::accent())
                .child(tr("attach_progress").replace("{done}", &done.to_string()).replace("{total}", &total.to_string()))))
            .child(field)
            .child(control_row);
        div().id("composer").relative().flex_shrink_0().w_full().px(px(36.)).pb(px(10.)).flex().justify_center()
            .when(readable, |el| el.drag_over::<ExternalPaths>(|style, _, _, _| style.bg(theme::accent_dim()))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.read_paths(paths.paths().to_vec(), cx))))
            .child(div().relative().w_full().max_w(px(column_width())).flex().flex_col()
                .when(!floating.is_empty(), |el| el.child(div().absolute().left_0().right_0().bottom(relative(1.)).pb_2().flex().flex_col().gap_2()
                    .occlude().children(floating)))
                .children(queue_row)
                .child(card)
                .children(footer))
            .into_any_element()
    }

    fn move_suggestion(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.visible_suggestions(cx).len();
        if count == 0 { return; }
        self.suggest_pick = (self.suggest_pick as isize + step).rem_euclid(count as isize) as usize;
        cx.stop_propagation();
        cx.notify();
    }

    // Tab completa o comando destacado; com o campo vazio aceita a sugestão do terminal; senão segue a navegação.
    // Na captura, só `stop_propagation` impede o Tab de chegar também à troca de foco.
    fn tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let suggestions = self.visible_suggestions(cx);
        if let Some(command) = suggestions.get(self.suggest_pick.min(suggestions.len().saturating_sub(1))) {
            let name = command.name.clone();
            self.fill_command(&name, false, window, cx);
        } else if self.composer.read(cx).value().is_empty() && !self.terminal_suggestion.is_empty() {
            let text = self.terminal_suggestion.clone();
            self.composer.update(cx, |input, cx| { input.insert(text, window, cx); });
            cx.notify();
        } else { return; }
        cx.stop_propagation();
    }

    // Esc fecha o que está aberto sobre o campo; sem nada aberto e com a sessão trabalhando, pede para interromper.
    fn escape(&mut self, cx: &mut Context<Self>) {
        if self.confirm.is_some() { self.confirm = None; }
        else if !self.visible_suggestions(cx).is_empty() { self.suggest_dismissed = Some(self.composer.read(cx).value().to_string()); }
        else if self.command_panel || self.recent.is_some() || self.controls_open() { self.command_panel = false; self.recent = None; self.close_controls(); }
        else if self.can_interrupt() { self.confirm = Some(Confirm::Stop); }
        else { return; }
        cx.stop_propagation();
        cx.notify();
    }

    fn render_message(&mut self, index: usize, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let id = id.to_owned();
        let mut discard = None;
        let (body, copy_text, label, note, user, error) = if id == PREVIEW {
            let body = self.visible_preview.text.clone();
            (body.clone(), body, tr("assistant"), Some(tr("working")), false, false)
        } else {
            let Some(Item::Event(event_index)) = self.items.get(index) else { return div().into_any_element(); };
            let event = &self.chat.events[*event_index];
            let label = match event.kind.as_str() {
                "user_msg" => tr("you"), "assistant_msg" => tr("assistant"), "thinking" => tr("thinking"),
                "tool_use" | "tool_result" => event.tool_name.clone().unwrap_or_else(|| tr("tool")),
                "notice" => tr("notice"), _ => tr("unknown"),
            };
            let mut notes = Vec::new();
            if event.desistiu == Some(true) || event.id.starts_with("held:") || event.hook_error.is_some() {
                notes.push(event.hook_error.clone().unwrap_or_else(|| tr("delivery_failed")));
            }
            // Só entrada desistida sai da fila; o backend recusa descartar o que ainda está por entregar.
            if event.desistiu == Some(true) { discard = event.id.strip_prefix("queued-").map(str::to_owned); }
            else if event.queued() { notes.push(tr(if event.queued_delivered == Some(true) { "delivering" } else { "queued" })); }
            if event.is_error == Some(true) { notes.push(tr("tool_error")); }
            let full = display_body(event);
            if full.contains("![") || !matches!(event.kind.as_str(), "user_msg" | "assistant_msg" | "tool_use" | "tool_result" | "thinking" | "notice") {
                notes.push(tr("unsupported"));
            }
            (full, event.body(), label, (!notes.is_empty()).then(|| notes.join(" · ")), event.kind == "user_msg", event.is_error == Some(true))
        };
        let markdown = if id == PREVIEW { preview_source(&self.visible_preview) } else { safe_markdown(&body) };
        let view = self.text_view(&id, &id, markdown, cx);
        let refs = match self.items.get(index) {
            Some(Item::Event(i)) if id != PREVIEW => attachment_refs(&self.chat.events[*i]),
            _ => Vec::new(),
        };
        let files = (!refs.is_empty()).then(|| self.render_refs(&id, refs, cx));
        let busy = self.selected_key().is_some_and(|key| self.flight.busy(&key));
        let discard = discard.map(|entry| Button::new(format!("discard-{id}")).small().ghost().label(tr("queue_discard")).disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| this.act(Action::Discard(entry.clone()), entry.clone(), cx))));
        // Conversa sem cartões: usuário em bolha à direita, agente em texto corrido. Só o que não é nenhum dos
        // dois (erro, aviso, formato desconhecido) mantém o rótulo, porque ali o rótulo é informação.
        let plain = id == PREVIEW || (kind_of(&self.items, index, &self.chat.events) == Some("assistant_msg") && !error);
        let text = (!body.trim().is_empty() || files.is_none()).then(|| TextView::new(&view).selectable(true).scrollable(false).on_link_click(|url, _, _, cx| {
            if url.starts_with("https://") || url.starts_with("http://") { cx.open_url(url); }
        }));
        let content = conversation_text(div().flex().flex_col().gap_2())
            .when(!user && !plain, |el| el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::muted() }).child(label)))
            .children(text)
            .when_some(files, |el, files| el.child(files));
        let copy_label = tr("copy_message");
        div().id(SharedString::from(format!("message-{id}"))).w_full().flex().flex_col().gap_2()
            .map(|el| if user {
                el.items_end().child(div().max_w(relative(0.78)).px(px(14.)).py(px(10.)).rounded(px(18.)).bg(theme::user_bubble()).child(content))
            } else { el.child(content) })
            .when_some(note, |el, note| el.child(div().flex().items_center().gap_2().when(user, |el| el.justify_end())
                .child(div().min_w_0().text_sm().text_color(theme::warning()).child(note))
                .when_some(discard, |el, button| el.child(button))))
            // Copiar sai da vista e mora no menu de contexto (e no Ctrl+Shift+C para a última resposta).
            .context_menu(move |menu, _, _| {
                let text = copy_text.clone();
                menu.item(PopupMenuItem::new(copy_label.clone()).icon(IconName::Copy)
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))))
            })
            .into_any_element()
    }
}

// A barra fica no recuo à direita do conteúdo, sem cobrir controles; o modo Always mostra que há mais abaixo.
fn scrolled(id: &'static str, handle: &ScrollHandle, max: f32, content: impl IntoElement) -> AnyElement {
    div().relative()
        .child(div().id(id).max_h(px(max)).overflow_y_scroll().track_scroll(handle).pr_4().child(content))
        .child(div().absolute().inset_0().child(Scrollbar::vertical(handle).mode(ScrollbarMode::Always)))
        .into_any_element()
}

// Anexos da mensagem: marcadores do compositor e imagens do transcript (usuário) ou caminhos citados (assistente).
fn attachment_refs(event: &ChatEvent) -> Vec<(Source, String, bool)> {
    let body = event.body();
    let mut refs = Vec::new();
    match event.kind.as_str() {
        "user_msg" => {
            let pasted = event.image_count.unwrap_or(0) as usize;
            if let Some(marked) = composer::parse_marked(&body) {
                let images = marked.files.iter().filter(|(image, _)| *image).count();
                // Caminho escrito e foto no transcript: a mesma imagem sairia duas vezes.
                let mut skip = if marked.image_marks == images { pasted.min(images) } else { 0 };
                let mut kept: Vec<_> = marked.files.into_iter().rev().filter(|(image, _)| {
                    if *image && skip > 0 { skip -= 1; false } else { true }
                }).collect();
                kept.reverse();
                for (image, name) in kept {
                    let image = image && composer::image_format(&name).is_some();
                    refs.push((Source::Upload(name.clone()), name, image));
                }
            }
            for i in 0..pasted { refs.push((Source::Transcript(event.id.clone(), i), format!("imagem-{}.png", i + 1), true)); }
        }
        "assistant_msg" => for path in composer::cited_paths(&body) {
            let name = composer::basename(&path).to_owned();
            let image = composer::image_format(&name).is_some();
            refs.push((Source::Cited(path), name, image));
        },
        _ => {}
    }
    refs
}

// Anexo que saiu do campo: tira a imagem inteira do cache de assets e do atlas da GPU, que não a soltam sozinhos.
fn release_image(image: Arc<Image>, window: &mut Window, cx: &mut App) {
    if let Some(render) = image.clone().get_render_image(window, cx) { cx.drop_image(render, Some(window)); }
    image.remove_asset(cx);
}

impl Hangar {
    /// Barra lateral do mock: marca, escopo, seções "Aguardando você" e "Sessões", rodapé com o servidor e a engrenagem.
    fn render_sidebar(&self, selected_name: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let a = crate::appearance::get();
        let floating = a.panels == crate::appearance::Panels::Floating;
        let fit_content = floating && a.sidebar_height == crate::appearance::SidebarHeight::Content;
        let host = self.server_label(cx);
        let (waiting, rest): (Vec<&SessionInfo>, Vec<&SessionInfo>) = self.sessions.iter().partition(|s| s.state == "awaiting_input");
        let section = |label: String, count: Option<usize>| div().flex().items_center().justify_between().px(px(8.)).pt(px(12.)).pb(px(6.))
            .child(chrome::section_label(label))
            .when_some(count, |el, n| el.child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).child(n.to_string())));
        let rows = |list: Vec<&SessionInfo>, cx: &mut Context<Self>| list.into_iter().map(|session| {
            let selected = selected_name == Some(session.name.as_str());
            self.render_session_row(session.clone(), selected, &host, cx)
        }).collect::<Vec<_>>();
        let waiting_count = waiting.len();
        let list = div().id("session-list").min_h_0().overflow_y_scroll().px(px(8.)).flex().flex_col().gap(px(2.))
            .when(!fit_content, |el| el.flex_1())
            .when(self.sessions.is_empty() && self.list_error.is_none(), |el| el.child(div().p_2().text_xs().text_color(theme::faint())
                .child(tr(if self.list_online { "empty_sessions" } else { "connecting" }))))
            .when(waiting_count > 0, |el| el.child(section(tr("sidebar_awaiting"), Some(waiting_count))).children(rows(waiting, cx)))
            .when(!rest.is_empty(), |el| el.child(section(tr("sessions"), None)).children(rows(rest, cx)));
        div().w(px(284.)).flex_shrink_0().flex().flex_col().bg(theme::chrome())
            // A linha da janela estica os filhos; "Só o conteúdo" precisa soltar a barra do fundo.
            .map(|el| if fit_content { el.max_h_full().self_start() } else { el.h_full() })
            .map(|el| if floating { el.rounded(px(18.)).border_1().border_color(theme::border()).shadow(theme::panel_shadow()) }
                else { el.border_r_1().border_color(theme::border()) })
            .child(div().h(px(44.)).flex_shrink_0().px(px(14.)).flex().items_center().gap_2()
                .child(chrome::hangar_mark(16., theme::accent()))
                .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("brand")))
                .child(div().px_2().py(px(1.)).rounded_full().bg(theme::hover()).text_size(px(10.)).text_color(theme::faint()).child(tr("experimental"))))
            .child(div().flex_shrink_0().mx(px(8.)).mt(px(4.)).mb(px(8.)).h(px(32.)).px(px(8.)).flex().items_center().gap_2().font_weight(FontWeight::MEDIUM)
                .child(chrome::small_icon(IconName::Server, 16., theme::muted()))
                .child(div().flex_1().min_w_0().truncate().child(tr("sidebar_all_sessions")))
                .child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).child(self.sessions.len().to_string())))
            .child(list)
            .when_some(self.list_error.clone(), |el, text| el.child(div().px_4().py_1().flex().items_center().gap_2().text_xs().text_color(theme::warning())
                .child(div().flex_1().min_w_0().child(text))
                .child(Button::new("reconnect").xsmall().ghost().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))))))
            .child(div().h(px(48.)).flex_shrink_0().px(px(8.)).flex().items_center().gap_1().border_t_1().border_color(theme::border())
                .child(Button::new("connection").ghost().flex_1().min_w_0().h(px(32.)).px(px(6.))
                    .tooltip(tr("connection_tip")).accessibility_label(tr("connection"))
                    .child(div().w_full().min_w_0().flex().items_center().gap_2()
                        .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(if self.list_online { theme::success() } else { theme::warning() }))
                        .child(div().min_w_0().truncate().text_size(px(13.)).text_color(theme::muted()).child(host)))
                    .on_click(cx.listener(|this, _, window, cx| this.open_connection(window, cx))))
                .child(Button::new("open-settings").custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted())
                        .hover(theme::hover()).active(theme::hover()))
                    .icon(chrome::small_icon(IconName::Settings, 16., theme::muted())).size(px(28.)).rounded(px(6.))
                    .accessibility_label(tr("settings")).tooltip_with_action(tr("settings_open"), &OpenSettings, None)
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(settings::Page::Appearance, window, cx)))))
            .into_any_element()
    }

    /// Linha de 3 níveis do mock: pasta @ servidor e tempo; selo, nome e estado; pergunta pendente ou branch com o diff.
    fn render_session_row(&self, session: SessionInfo, selected: bool, host: &str, cx: &mut Context<Self>) -> AnyElement {
        let state = session.state.as_str();
        let limited = session.limited == Some(true);
        let untracked = session.tracked == Some(false);
        let chip_state = if limited { "limited" } else { state };
        let chip = chrome::state_chip(chip_state, tr(&format!("chip_{chip_state}")), false);
        let sub = match state {
            "awaiting_input" => session.question.clone().map(|q| (conversation::one_line(&q, 80), theme::warning(), false)),
            "working" => session.label.clone().filter(|l| !l.trim().is_empty())
                .map(|l| (conversation::one_line(l.split(" (").next().unwrap_or(&l), 80), theme::muted(), true)),
            _ => None,
        };
        let meta = place(&session, host);
        let when = session.last_activity.map(side::since);
        let branch = session.branch.clone().filter(|b| !b.is_empty());
        let (added, removed) = (session.git_added.filter(|n| *n > 0), session.git_removed.filter(|n| *n > 0));
        let name = session.name.clone();
        let lane = || div().w(px(18.)).flex_shrink_0();
        div().id(SharedString::from(session.name.clone())).flex_shrink_0().flex().flex_col().gap(px(1.)).px(px(8.)).py(px(7.)).rounded(px(10.))
            .when(selected, |el| el.bg(theme::selected_row()))
            .when(!selected, |el| el.hover(|el| el.bg(theme::hover())))
            .child(div().flex().items_center().gap(px(8.)).text_size(px(11.5)).text_color(theme::faint())
                .child(lane())
                .child(div().flex_1().min_w_0().truncate().child(meta))
                .when_some(when, |el, w| el.child(div().flex_shrink_0().child(w))))
            .child(div().flex().items_center().gap(px(8.)).when(untracked, |el| el.opacity(0.45))
                .child(lane().child(chrome::provider_glyph(&session.provider, 16.)))
                .child(div().flex_1().min_w_0().flex().items_center().gap_2()
                    .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(name))
                    .when(untracked, |el| el.child(badge(tr("untracked_badge"), theme::faint()))))
                .child(chip))
            .map(|el| match sub {
                Some((text, color, working)) => el.child(div().flex().gap(px(8.)).child(lane())
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(color).when(working, |el| el.italic()).child(text))),
                None if branch.is_some() || added.is_some() || removed.is_some() => el.child(div().flex().items_center().gap(px(8.))
                    .text_size(px(11.5)).text_color(theme::faint()).child(lane())
                    .child(div().flex_1().min_w_0().flex().items_center().gap(px(6.))
                        .when_some(branch, |el, b| el.child(chrome::small_icon(IconName::GitBranch, 12., theme::faint()))
                            .child(div().min_w_0().truncate().child(b)))
                        .when_some(added, |el, a| el.child(div().flex_shrink_0().text_color(theme::success()).child(format!("+{a}"))))
                        .when_some(removed, |el, r| el.child(div().flex_shrink_0().text_color(theme::removed()).child(format!("−{r}")))))),
                None => el,
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(session.clone(), window, cx);
                // Foco só no gesto sobre a lista; troca automática de transcript não tira o foco de ninguém.
                if !this.connection_dialog && session.readable() {
                    this.composer.update(cx, |input, cx| input.focus(window, cx));
                }
            }))
            .into_any_element()
    }
}

fn kind_of<'a>(items: &[Item], index: usize, events: &'a [ChatEvent]) -> Option<&'a str> {
    match items.get(index) { Some(Item::Event(i)) => events.get(*i).map(|e| e.kind.as_str()), _ => None }
}

fn folder_name(session: &SessionInfo) -> Option<String> {
    session.cwd.as_deref().and_then(|cwd| cwd.trim_end_matches('/').rsplit('/').next()).filter(|f| !f.is_empty()).map(str::to_owned)
}

/// "pasta @ servidor"; sem pasta legível, só o servidor.
fn place(session: &SessionInfo, host: &str) -> String {
    folder_name(session).map(|folder| format!("{folder} @ {host}")).unwrap_or_else(|| host.to_owned())
}

fn badge(text: String, color: Hsla) -> Div {
    div().flex_shrink_0().px(px(5.)).py(px(1.)).rounded(px(6.)).bg(theme::raised()).border_1().border_color(theme::border())
        .text_size(px(10.)).text_color(color).child(text)
}

fn agent_name(provider: &str) -> &'static str {
    match provider { "codex" => "Codex", "kimi" => "Kimi", "pi" => "Pi", "omp" => "OMP", _ => "Claude" }
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MiB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KiB", b >> 10),
        b => format!("{b} B"),
    }
}

// Cópia para abrir fica numa pasta só do usuário, com nome único; o programa do sistema recebe o caminho.
fn private_copy(name: &str) -> std::io::Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("hangar-native-files");
    std::fs::create_dir_all(&base)?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))?; }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    Ok(base.join(format!("{stamp}-{name}")))
}

// A conexão que funcionou volta na próxima abertura, como o login do app web; só o dono lê o arquivo.
fn saved_connection_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("hangar-native").join("connection.json"))
}

fn load_connection() -> Option<(String, String)> {
    let value: Value = serde_json::from_slice(&std::fs::read(saved_connection_path()?).ok()?).ok()?;
    let (address, token) = (value.get("address")?.as_str()?, value.get("token")?.as_str()?);
    (!address.is_empty() && !token.is_empty()).then(|| (address.to_owned(), token.to_owned()))
}

fn save_connection(address: &str, token: &str) -> std::io::Result<()> {
    let path = saved_connection_path().ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?;
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
    std::io::Write::write_all(&mut options.open(&tmp)?, json!({"address": address, "token": token}).to_string().as_bytes())?;
    std::fs::rename(&tmp, &path)
}

fn select_snapshot(state: &SessionState) -> String { json!([state.question, state.options]).to_string() }

fn display_body(event: &ChatEvent) -> String {
    match event.kind.as_str() {
        "notice" => tr(&event.body()),
        "assistant_msg" => interaction::plan_display(&event.body()),
        // Anexos viram cartões próprios; o texto mostra só a legenda.
        "user_msg" => { let body = event.body(); composer::parse_marked(&body).map(|m| m.caption).unwrap_or(body) }
        _ => event.body(),
    }
}

fn render_source(event: &ChatEvent) -> String { safe_markdown(&display_body(event)) }

// Identidade do conteúdo de uma linha que não é mensagem: muda quando chega resultado ou o grupo cresce.
fn signature(item: &Item, events: &[ChatEvent]) -> String {
    let tool = |t: &Tool| format!("{}>{}", events[t.call].id, t.result.map(|i| events[i].id.as_str()).unwrap_or(""));
    match item {
        Item::Event(_) => String::new(),
        Item::Orphan(i) => events[*i].result.as_deref().map(str::len).unwrap_or(0).to_string(),
        Item::Tool(t) => tool(t),
        Item::Group { tools, .. } => tools.iter().map(tool).collect::<Vec<_>>().join(","),
        Item::Thinking { parts, .. } => parts.iter().map(|&i| format!("{}:{}", events[i].id, events[i].text.as_deref().map(str::len).unwrap_or(0))).collect::<Vec<_>>().join(","),
    }
}

fn preview_source(preview: &Preview) -> String {
    if preview.md { return safe_markdown(&preview.text); }
    let line_count = preview.text.lines().count();
    let source = if preview.full || line_count <= 10 { preview.text.as_str() }
        else { &preview.text[preview.text.match_indices('\n').nth(line_count - 11).map(|(index, _)| index + 1).unwrap_or(0)..] };
    let mut output = String::with_capacity(source.len());
    for ch in source.chars() {
        match ch {
            '\n' => output.push_str("  \n"),
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '#' | '+' | '-' | '!' | '>' | '~' | '|' => {
                output.push('\\');
                output.push(ch);
            }
            _ => output.push(ch),
        }
    }
    output
}

fn safe_markdown(source: &str) -> String {
    if !source.contains('<') && !source.contains("![") { return source.to_owned(); }
    let mut output = String::with_capacity(source.len());
    let mut fence = None;
    let mut inline = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") { Some('`') }
            else if trimmed.starts_with("~~~") { Some('~') } else { None };
        if let Some(marker) = marker {
            if fence.is_none() { fence = Some(marker); }
            else if fence == Some(marker) { fence = None; }
            output.push_str(line);
            continue;
        }
        if fence.is_some() { output.push_str(line); continue; }
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '`' {
                let mut run = 1;
                while chars.peek() == Some(&'`') { chars.next(); run += 1; }
                inline = match inline { Some(open) if open == run => None, None => Some(run), other => other };
                for _ in 0..run { output.push('`'); }
            } else if inline.is_none() && ch == '<' && chars.peek().is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!')) {
                output.push_str("&lt;");
            } else if inline.is_none() && ch == '!' && chars.peek() == Some(&'[') {
                output.push_str("\\!");
            } else { output.push(ch); }
        }
    }
    output
}

async fn forward_stream(api: Api, name: Option<String>, connection: u64, selection: Option<u64>, target: async_channel::Sender<Envelope>) {
    let (tx, rx) = async_channel::bounded(128);
    let producer = async {
        api::sse::run(api, name, tx.clone()).await;
        tx.close();
    };
    let consumer = async {
        while let Ok(update) = rx.recv().await {
            if target.send(Envelope { connection, selection, payload: Payload::Stream(update) }).await.is_err() { break; }
        }
    };
    tokio::pin!(producer, consumer);
    tokio::select! {
        _ = &mut producer => { consumer.await; }
        _ = &mut consumer => {}
    }
}

impl Render for Hangar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.media.next_frame();
        let selected_name = self.selected.as_ref().map(|s| s.name.clone());
        let selected_key = self.selected_key();
        let sending = selected_key.as_ref().is_some_and(|key| self.delivery.pending(key));
        let stopping = selected_key.as_ref().is_some_and(|key| self.stopping.contains(key));
        let delivery_note = selected_key.as_ref().and_then(|key| {
            if self.delivery.pending(key) { return Some((tr("sending"), false)); }
            self.delivery.outcome(key).map(|outcome| match outcome {
                SendOutcome::Delivered => (tr("delivered"), false),
                SendOutcome::Queued => (tr("queued"), false),
                SendOutcome::Uncertain => (tr("delivery_uncertain"), true),
                SendOutcome::Rejected(reason) => (reason.clone(), true),
            })
        });
        let stop_note = selected_key.as_ref().and_then(|key| self.stop_feedback.get(key)).cloned();
        let floating = theme::is_floating();
        let sidebar = self.render_sidebar(selected_name.as_deref(), cx);

        // Sessão sem conversa não tem stream próprio: o estado é o da lista.
        let header_state = if self.chat_online && self.chat.state.state.is_empty() { "loading".to_owned() }
            else if self.chat_online { self.chat.state.state.clone() }
            else if let Some(s) = self.selected.as_ref().filter(|s| !s.readable()) { s.state.clone() }
            else if self.selected.is_some() { "reconnecting".to_owned() }
            else if self.list_online { "connected".to_owned() } else { "disconnected".to_owned() };
        let session_chip = matches!(header_state.as_str(), "working" | "idle" | "awaiting_input" | "dead");
        let limited_now = self.chat.state.limited.or(self.selected.as_ref().and_then(|s| s.limited)) == Some(true);
        let chip_state = if limited_now && session_chip { "limited".to_owned() } else { header_state.clone() };
        let place = self.selected.as_ref().map(|s| place(s, &self.server_label(cx)));
        let mut content = div().flex_1().min_w_0().h_full().flex().flex_col()
            .child(div().h(px(44.)).pl(px(20.)).pr(px(12.)).flex_shrink_0().flex().items_center().gap(px(10.)).when(floating, |el| el.mx(px(4.)))
                .when_some(self.selected.as_ref(), |el, s| el.child(chrome::provider_glyph(&s.provider, 18.)))
                .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).child(selected_name.clone().unwrap_or_else(|| tr("title"))))
                .when_some(place, |el, place| el.child(div().min_w_0().truncate().text_color(theme::faint()).child(place)))
                .child(div().flex_1())
                .child(if session_chip { chrome::state_chip(&chip_state, tr(&format!("chip_{chip_state}")), true) }
                    else { div().flex_shrink_0().text_xs().text_color(theme::status(&header_state)).child(tr(&header_state)).into_any_element() })
                .when(self.selected.is_some(), |el| el.child(chrome::icon_button("side-show", IconName::PanelRight,
                        tr(if self.side.open { "side_hide" } else { "side_show" }), cx)
                    .selected(self.side.open).on_click(cx.listener(|this, _, _, cx| this.toggle_side(cx))))));

        let prethread = self.render_prethread(cx);
        let prethread_open = prethread.is_some();
        if let Some(selected) = &self.selected {
            if let Some(card) = prethread {
                content = content.child(card);
            } else if !selected.readable() {
                content = content.child(div().flex_1().p_6().text_color(theme::muted()).child(tr(if selected.tracked == Some(false) { "untracked" } else { "starting" })));
            } else {
                content = content.child(in_column(div().py_2().flex().gap_2().items_center()
                    .when(self.has_older, |el| el.child(Button::new("older").small().ghost().label(tr("older")).disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| { this.history_limit = this.history_limit.saturating_add(400); this.etag = None; this.load_history(cx); }))))
                    .when(self.has_older, |el| el.child(div().text_xs().text_color(theme::muted()).child(format!("{} {}", tr("history_window"), self.history_limit))))
                    .when(self.loading, |el| el.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))))
                    .child(div().flex_1())
                    .child(Button::new("latest").small().ghost().label(tr("latest")).on_click(cx.listener(|this, _, _, cx| this.follow_engage(cx))))));
                if self.row_ids.is_empty() && !self.loading && self.error.is_none() {
                    content = content.child(div().flex_1().p_6().text_color(theme::muted()).child(tr("empty_chat")));
                } else {
                    let view = cx.entity().downgrade();
                    // Leitura Folha: uma folha da largura da coluna atrás das mensagens, com o fundo nas margens.
                    let sheet = (appearance::get().effective_reading() == appearance::Reading::Sheet).then(|| div().absolute().inset_0()
                        .px(px(20.)).pt(px(4.)).pb(px(8.)).flex().justify_center()
                        .child(div().w_full().h_full().max_w(px(column_width() + 32.)).rounded(px(14.)).border_1().border_color(theme::border())
                            .bg(theme::sheet()).shadow(theme::sheet_shadow())));
                    content = content.child(div().relative().flex_1().min_h_0().flex().flex_col()
                        .children(sheet)
                        .child(list(self.list_state.clone(), move |i, window, cx| {
                            view.update(cx, |this, cx| this.render_row(i, window, cx)).unwrap_or_else(|_| div().into_any_element())
                        }).flex_1().min_h_0())
                        .child(self.wheel_layer(cx)));
                    self.schedule_scroll(window, cx);
                }
            }
        } else { content = content.child(div().flex_1().flex().items_center().justify_center().text_color(theme::muted()).child(tr("choose_session"))); }

        let busy = selected_key.as_ref().is_some_and(|key| self.flight.busy(key));
        let action_note = selected_key.as_ref().and_then(|key| self.action_feedback.get(key)).cloned();
        let readable = self.selected.as_ref().is_some_and(|s| s.readable());
        let card = if readable { self.render_ask(busy, window, cx).or_else(|| self.render_options(busy, cx)) } else { None };
        let plan_bar = if readable && card.is_none() {
            self.render_plan_bar(busy, cx).or_else(|| self.render_headless_plan(cx)).or_else(|| self.render_plan_preview(cx))
        } else { None };
        let steer = readable && self.steer_offered();
        let queued = self.queued_count();
        // Sem cartão, o pedido depende do terminal (overlay, login ou seletor sem opções legíveis).
        // Pergunta do transcript já respondida espera só o `tool_result`: não é pedido sem resposta.
        let answered = interaction::ask_from_events(&self.chat.events, self.provider().0)
            .and_then(|ask| ask.tool_use_id).is_some_and(|id| self.tool_answered(&id));
        let pending = card.is_none() && !answered && !prethread_open && (self.chat.state.state == "awaiting_input" || self.chat.state.login == Some(true));
        // Faixas e avisos entre a conversa e o compositor ficam na mesma coluna das mensagens.
        content = content
            .when_some(card, |el, card| el.child(card))
            .when_some(plan_bar, |el, bar| el.child(in_column(bar)))
            .when(pending, |el| el.child(in_column(div().py_2().text_sm().text_color(theme::warning()).child(tr("pending_question")))))
            .when_some(self.chat.state.question.clone().filter(|_| pending), |el, question| el.child(in_column(div().text_sm().child(question))))
            .when_some(action_note, |el, (note, warning)| el.child(in_column(div().py_1().text_xs().text_color(if warning { theme::warning() } else { theme::muted() }).child(note))))
            .when_some(self.chat.state.problema_detalhe.clone().or_else(|| self.chat.state.problema.clone()), |el, problem| el.child(in_column(div().text_sm().text_color(theme::warning()).child(problem))))
            .when_some(self.error.clone(), |el, error| el.child(in_column(div().py_2().text_sm().text_color(theme::warning()).child(error)
                .child(Button::new("retry").small().ghost().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| {
                    if let Some(session) = this.selected.clone() { this.select(session, window, cx); }
                }))))))
            .when_some(delivery_note, |el, (note, warning)| el.child(in_column(div().py_1().text_xs().text_color(if warning { theme::warning() } else { theme::muted() }).child(note))))
            .when_some(stop_note, |el, (note, warning)| el.child(in_column(div().py_1().text_xs().text_color(if warning { theme::warning() } else { theme::muted() }).child(note))))
            .when(self.selected.is_some(), |el| el.child(self.render_composer(readable, busy, steer, queued, sending, stopping, window, cx)));

        let side = self.render_side(window, cx);
        let dialog = div().w(px(480.)).p_6().bg(theme::surface()).border_1().border_color(theme::border()).rounded_xl().flex().flex_col().gap_4()
            .child(div().text_xl().font_weight(FontWeight::BOLD).child(tr("connection")))
            .child(div().text_sm().text_color(theme::muted()).child(tr("connection_hint")))
            .child(div().text_sm().child(tr("server"))).child(Input::new(&self.address))
            .child(div().text_sm().child(tr("token"))).child(Input::new(&self.token))
            .when_some(self.error.clone(), |el, error| el.child(div().text_sm().text_color(theme::warning()).child(error)))
            .child(div().flex().justify_end().gap_2()
                .when(self.api.is_some(), |el| el.child(Button::new("cancel").label(tr("cancel")).on_click(cx.listener(|this, _, window, cx| {
                    this.connection_dialog = false;
                    // O campo do diálogo some da árvore; sem isto a janela fica sem foco e os atalhos não chegam.
                    this.root_focus.focus(window, cx);
                    cx.notify();
                }))))
                .child(Button::new("connect").primary().label(tr("connect")).on_click(cx.listener(|this, _, window, cx| this.connect(window, cx)))));

        let settings_page = self.settings;
        div().id("hangar-root").track_focus(&self.root_focus).relative().size_full().flex().bg(theme::window_fill()).text_color(theme::text()).text_base()
            .children(self.render_backdrop(window))
            .font_family(theme::SANS)
            .when(floating && settings_page.is_none(), |el| el.p(px(10.)).gap(px(10.)))
            .on_action(cx.listener(|this, _: &FocusComposer, window, cx| {
                if !this.connection_dialog && this.settings.is_none() && this.selected.as_ref().is_some_and(|s| s.readable()) {
                    this.composer.update(cx, |input, cx| input.focus(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                if !this.connection_dialog { this.open_settings(settings::Page::Appearance, window, cx); }
            }))
            .on_action(cx.listener(|this, _: &CopyLastReply, _, cx| {
                if let Some(text) = this.last_reply().filter(|_| this.settings.is_none()) { cx.write_to_clipboard(ClipboardItem::new_string(text)); }
            }))
            // Esc fora do campo fecha o painel aberto sobre o compositor (o clique no botão tira o foco do campo).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key != "escape" || this.connection_dialog { return; }
                if this.settings.is_some() {
                    this.close_settings(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if this.controls_open() || this.command_panel || this.recent.is_some() {
                    this.close_controls();
                    this.command_panel = false;
                    this.recent = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            // Clique em área sem foco próprio devolve o foco à raiz, para os atalhos continuarem chegando.
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                if window.focused(cx).is_none() { this.root_focus.focus(window, cx); }
            }))
            // Arrasto da borda do painel: segue o ponteiro na janela toda e termina ao soltar.
            .when(self.side_dragging(), |el| el.cursor_col_resize()
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    this.drag_side(f32::from(event.position.x), event.pressed_button == Some(MouseButton::Left), cx);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_drag(cx))))
            .map(|el| match settings_page {
                Some(page) => el.child(self.render_settings(page, cx)),
                None => el.child(sidebar).child(content).when_some(side, |el, side| el.child(side)),
            })
            .when(self.connection_dialog, |el| el.child(div().absolute().inset_0().bg(theme::scrim()).flex().items_center().justify_center()
                .child(dialog.focus_trap("connection-dialog", &self.connection_focus))))
            .children(Root::render_notification_layer(window, cx))
    }
}
