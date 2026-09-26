use std::{collections::{HashMap, HashSet}, path::PathBuf, sync::Arc, time::{Duration, Instant}};
use gpui_kit::{component::{button::*, checkbox::Checkbox, radio::Radio, scroll::{Scrollbar, ScrollbarMode}, menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
    input::{Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Textarea, TextareaState}, text::{TextView, TextViewState}, *}, *};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::assets::IconName;
use tokio::{runtime::Runtime, task::JoinHandle};
use crate::{api::{self, Api, Failure, Source, dto::*, sse::Update}, cards, chat::{Chat, LiveTool}, composer,
    conversation::{self, Item, Tool}, delivery::{DeliveryTracker, SendOutcome, SessionKey}, i18n::tr, theme,
    interaction::{self, Action, Ask, InFlight, Pick}, media::{self, MediaCache, MediaState}, appearance};
use gpui_kit::component::notification::Notification;
use serde_json::{Value, json};

mod activity;
mod backdrop;
mod baton;
mod accounts;
mod chrome;
mod computer;
mod controls;
mod create;
mod device;
mod follow;
mod harness;
mod viewer;
mod machines;
mod orchestration;
mod panes;
mod popup;
mod rows;
mod files;
mod settings;
mod server_config;
mod shortcuts;
mod side;
mod sidebar;
mod subagent;
mod sync;

actions!(hangar, [FocusComposer, OpenSettings, CopyLastReply, FocusSettingsSearch, NextSession, PreviousSession]);

const LIVE_THINKING: &str = "__thinking__";
const LIVE_TOOL: &str = "__tool__";
const PREVIEW: &str = "__preview__";
const WORKING: &str = "__working__";
/// Entrada da linha "trabalhando"; a marca, desenhada fora da conversa, entra no mesmo tempo.
const WORKING_FADE: Duration = Duration::from_millis(200);
/// Quanto "Enviando…" espera o turno começar depois da entrega; passou disso, a sessão não vai trabalhar.
const SENT_BRIDGE: Duration = Duration::from_secs(5);
/// Prefixo da linha do cartão fixo de um agente rodando, seguido do id do tool_use.
const PINNED: &str = "pin:";
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
    // Cotação, diário e atualização da conexão atual (páginas Geral, Diário de uso e Sobre).
    Device(device::DeviceReply),
    // Contas e modelos da conexão atual.
    Accounts(accounts::AccountsReply),
    Orchestration(orchestration::OrchestrationReply),
    // Página Atalhos da conexão atual.
    Shortcuts(shortcuts::ShortcutsReply),
    // Harnesses: saúde dos CLIs e consertos do servidor conectado.
    Harness(harness::HarnessReply),
    // Notificações e Anexos: rascunho do servidor e horas silenciosas da conexão atual.
    ServerConfig(server_config::ServerConfigReply),
    Sync(sync::SyncReply),
    // Máquinas: identificador, alcance e reinício do servidor conectado.
    Machines(machines::MachinesReply),
    Computer(computer::ComputerReply),
    // Diálogo Nova sessão: a resposta vai ao diálogo que a pediu, se ele ainda for o aberto.
    Create(EntityId, create::CreateReply),
    HeadlessPlan(SessionKey, controls::PlanOutcome),
    // Barra lateral: prévia, leitura do silenciar e as gravações do menu da sessão.
    Sidebar(sidebar::SidebarReply),
    // Aba Atividade: a conta de subagentes no disco e a lista da aba.
    Activity(activity::ActivityReply),
    FileView(files::FileReply),
    // Resumo do bastão (`GET …/bastao/dossie`), amarrado ao estado da tela que o pediu e ao número do pedido.
    Dossier(EntityId, u64, Result<String, Failure>),
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

#[derive(Clone)]
struct Recent { key: SessionKey, files: Option<Result<Vec<UploadFile>, String>> }

struct Envelope { connection: u64, selection: Option<u64>, payload: Payload }
struct RichText { source: String, view: Entity<TextViewState>, _observer: Subscription, touched: u64, row: String }
enum ChatUpdate { Message(ChatEvent), Preview(Preview), State(SessionState), Question(Option<Ask>), Thinking(String), LiveTool(Option<LiveTool>), Reset }
/// Texto preparado de uma linha: a mensagem pronta para o `TextView`, as linhas do resultado de uma chamada ou o
/// detalhe aberto (entrada/saída) de uma chamada.
#[derive(Clone)]
enum Prepared {
    /// `card`: o cartão já lido, para o desenho não reler o texto a cada quadro.
    Message { markdown: String, blank: bool, card: Option<cards::Card> },
    Lines(usize),
    Detail { fenced: String, total: usize, clipped: bool, full: SharedString },
}
/// O que um quadro do SSE mudou: nada (ping), só a tela (estatísticas, aviso), as linhas da conversa, ou só as linhas
/// do fim dela (prévia, pensamento e ferramenta ao vivo), que ninguém fora da conversa lê.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Changed { Nothing, Screen, Rows, Tail, Bottom }

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
    // Coluna que o gráfico de cada tabela mostra, pela chave "<linha>#t<n>".
    table_column: HashMap<String, usize>,
    // Tabelas que dão gráfico em cada resposta, com a fonte de onde saíram: refeitas só quando a fonte muda.
    tables: HashMap<String, (String, std::rc::Rc<[crate::tables::Table]>)>,
    // Chamada → resultado do transcript, refeito junto com as linhas; o desenho só consulta.
    paired: HashMap<usize, usize>,
    // Agentes e shells de fundo, dobrados junto com as linhas; `pinned` são as chamadas dos agentes rodando.
    activity: conversation::Activity,
    pinned: HashSet<usize>,
    last_message: Option<usize>,
    live_clear_epoch: [u64; 2],
    rich: HashMap<String, RichText>,
    // Texto das linhas já preparado, pela chave da linha ou da parte: esvaziado quando o chat muda, e o desenho
    // só prepara o que falta. Assim contar linhas, formatar JSON e limpar o markdown não roda a cada quadro.
    prepared: HashMap<String, Prepared>,
    render_tick: u64,
    preview_drop_epoch: u64,
    preview_drop_scheduled: bool,
    visible_preview: Preview,
    preview_tick_epoch: u64,
    preview_tick_scheduled: bool,
    preview_last_tick: Option<Instant>,
    preview_carry: f64,
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
    // Abas no topo: foco de cada aba pelo nome da sessão (setas andam entre elas) e a rolagem da faixa,
    // que traz a aba ativa para a vista quando a seleção muda.
    tab_focus: HashMap<String, FocusHandle>,
    tabs_scroll: ScrollHandle,
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
    device: device::Device,
    accounts: accounts::Accounts,
    orchestration: orchestration::Orchestration,
    shortcuts: shortcuts::Shortcuts,
    harness: harness::Harnesses,
    server_config: server_config::ServerConfig,
    sync: sync::Sync,
    machines: machines::Machines,
    computer: computer::Computer,
    new_session: Option<Entity<create::NewSession>>,
    sidebar: sidebar::Sidebar,
    // Busca do seletor de modelo quando a lista é longa.
    ctl_search: Entity<InputState>,
    act: activity::ActivityState,
    panes: panes::Panes,
    files: files::Files,
    dossier: Option<Entity<baton::Dossier>>,
    /// Quando vimos o turno começar ao vivo; a sessão aberta já trabalhando conta do último envio.
    turn_seen: Option<Instant>,
    /// Envio entregue que o turno ainda não pegou: "Enviando…" segue até o estado virar trabalhando ou o prazo passar.
    sent_until: Option<(SessionKey, Instant)>,
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
        // O cursor do campo só para de piscar ao perder o foco: foco num campo que nunca aparece o deixa piscando para sempre.
        // Com conexão salva o diálogo não abre; o `connect` que falhar põe o foco aqui.
        if saved.is_none() { address.update(cx, |input, cx| input.focus(window, cx)); }
        let composer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 10).submit_on_enter(true));
        let input_subscription = cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
            match event {
                InputEvent::PressEnter { secondary: false, shift: false } if !this.connection_dialog => this.submit(false, false, window, cx),
                // A lista de comandos acompanha o que se digita.
                // Texto e sugestões só aparecem na faixa de baixo.
                InputEvent::Change => { this.suggest_pick = 0; this.redraw(panes::Area::Bottom, cx); }
                _ => {}
            }
        });
        // Ctrl+L leva ao campo de mensagem; a raiz da janela trata a ação e segura o foco quando nada mais o tem.
        cx.bind_keys([KeyBinding::new("ctrl-l", FocusComposer, None), KeyBinding::new("ctrl-,", OpenSettings, None),
            KeyBinding::new("ctrl-shift-c", CopyLastReply, None), KeyBinding::new("ctrl-f", FocusSettingsSearch, None),
            KeyBinding::new("secondary-down", NextSession, None), KeyBinding::new("secondary-up", PreviousSession, None)]);
        let settings_ui = settings::SettingsUi::new(window, cx);
        let root_focus = cx.focus_handle();
        cx.on_focus_lost(window, |this: &mut Self, window, cx| this.machines_focus_lost(window, cx)).detach();
        let command_search = cx.new(|cx| InputState::new(window, cx).placeholder(tr("commands_search")));
        // A busca mora no painel de comandos, sobre o compositor.
        cx.subscribe(&command_search, |this, _, _: &InputEvent, cx| this.redraw(panes::Area::Bottom, cx)).detach();
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
        let sidebar = sidebar::Sidebar::new(window, cx);
        let panes = panes::Panes::new(cx);
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
            table_column: HashMap::new(), tables: HashMap::new(), paired: HashMap::new(), activity: Default::default(), pinned: HashSet::new(), last_message: None, live_clear_epoch: [0; 2], rich: HashMap::new(), prepared: HashMap::new(), render_tick: 0,
            preview_drop_epoch: 0, preview_drop_scheduled: false,
            visible_preview: Preview::default(), preview_tick_epoch: 0, preview_tick_scheduled: false,
            preview_last_tick: None, preview_carry: 0., preview_deadline: None,
            attachments: HashMap::new(), attach_seq: 0, uploading: HashMap::new(), commands: HashMap::new(),
            suggest_pick: 0, suggest_dismissed: None, command_panel: false, command_search, confirm: None,
            terminal_suggestion: String::new(), recent: None, media: MediaCache::new(), stats: None,
            side: side::Side::default(), controls: controls::Controls::default(),
            settings: None, settings_ui, tab_focus: HashMap::new(), tabs_scroll: ScrollHandle::new(),
            appearance_note: appearance_error.map(|error| tr("settings_not_loaded").replace("{error}", &error)),
            desktop_note: None,
            palette_seq: 0, backdrop_seq: 0, backdrop: None, backdrop_note: None, backdrop_busy: None, grain: crate::media::grain(),
            device: device::Device::default(), accounts: accounts::Accounts::default(), orchestration: orchestration::Orchestration::default(), shortcuts: shortcuts::Shortcuts::default(),
            server_config: server_config::ServerConfig::default(), harness: harness::Harnesses::default(), sync: sync::Sync::default(), machines: machines::Machines::default(), computer: computer::Computer::default(), new_session: None, sidebar,
            act: activity::ActivityState::new(cx), files: files::Files::new(window, cx), ctl_search: controls::search_field(window, cx), panes, dossier: None, turn_seen: None, sent_until: None,
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
        if error.detail.starts_with("erro_arq_") {
            if let Some(message) = crate::i18n::tr_web(&error.detail, &HashMap::new()) { return message; }
        }
        match error.status { Some(401) => tr("auth_error"), Some(_) => tr(&error.detail), None => Self::setting_failure(error) }
    }

    /// Gravação de configuração que ficou sem resposta (tempo esgotado, conexão recusada): a frase do web para queda de rede. O texto de
    /// entrega incerta é do envio de mensagens.
    fn setting_failure(error: &Failure) -> String {
        if error.status.is_none() && error.uncertain { tr("connection_failed") } else { Self::failure(error) }
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
            Err(error) => {
                self.error = Some(Self::failure(&error));
                self.address.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
                return;
            }
        };
        self.unsaved_connection = Some((address, token));
        if let Some(key) = self.selected_key() { self.drafts.insert(key, self.composer.read(cx).value().to_string()); }
        self.drop_connection(window, cx);
        self.api = Some(api.clone());
        self.server = Some(api.identity());
        self.connection_dialog = false;
        self.root_focus.focus(window, cx);
        let tx = self.tx.clone();
        let connection = self.connection;
        self.list_task = Some(self.runtime.spawn(async move {
            let result = api.sessions().await;
            let fatal = result.as_ref().err().is_some_and(|e| matches!(e.status, Some(401 | 403)));
            if tx.send(Envelope { connection, selection: None, payload: Payload::Sessions(result) }).await.is_err() || fatal { return; }
            forward_stream(api, None, connection, None, tx).await;
        }));
        self.reset_device(cx);
        // O rascunho é deste servidor: na troca ele morre, no "Reconectar" ao mesmo ele fica.
        self.server_config.reconnected(format!("{}\n{}", self.server.as_deref().unwrap_or(""), self.token.read(cx).value()));
        // Página do servidor aberta na troca: relê do servidor novo.
        if let Some(page) = self.settings { self.settings_opened(page, cx); }
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

    /// O que é da conexão atual sai da tela e os pedidos em voo passam a ser descartados. Serve à troca de servidor e ao Sair.
    fn drop_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.connection += 1;
        self.selection += 1;
        self.revision += 1;
        for slot in [&mut self.list_task, &mut self.session_task, &mut self.history_task] { if let Some(t) = slot.take() { t.abort(); } }
        self.leave_accounts();
        self.selected = None;
        self.sessions.clear();
        self.chat = Chat::default();
        self.turn_seen = None;
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
        self.loading = false;
        self.history_started = false;
        self.history_installed = false;
        self.pending_chat.clear();
        self.composer.update(cx, |input, cx| input.set_value("", window, cx));
        self.sync_rows(cx);
        self.side.reset_server();
        self.sidebar.reset_server();
        self.sync = sync::Sync::default();
        self.controls = controls::Controls::default();
        self.accounts = accounts::Accounts::default();
        self.orchestration = orchestration::Orchestration::default();
        self.shortcuts = shortcuts::Shortcuts::default();
        self.harness = harness::Harnesses::default();
        self.machines = machines::Machines::default();
        self.computer = computer::Computer::default();
    }

    fn select(&mut self, session: SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(old) = self.selected_key() { self.drafts.insert(old, self.composer.read(cx).value().to_string()); }
        // Com as abas no topo, a aba da sessão aberta entra na vista da faixa.
        if let Some(ix) = self.sessions.iter().position(|s| s.name == session.name) { self.tabs_scroll.scroll_to_item(ix); }
        self.selection += 1;
        self.revision += 1;
        if let Some(t) = self.session_task.take() { t.abort(); }
        if let Some(t) = self.history_task.take() { t.abort(); }
        self.chat = Chat::default();
        self.turn_seen = None;
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
        self.dossier = None;
        self.controls.on_select();
        self.reset_subagent_count();
        self.selected = Some(session.clone());
        if session.readable() {
            if let Some(api) = self.api.clone() {
                let tx = self.tx.clone();
                let connection = self.connection;
                let selection = self.selection;
                self.session_task = Some(self.runtime.spawn(forward_stream(api, Some(session.name), connection, Some(selection), tx)));
            }
        }
        (self.activity, self.pinned) = (Default::default(), HashSet::new());
        self.sync_activity(cx);
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
        // A conversa só é refeita quando o chat mudou, e a janela só redesenha quando algo visível mudou:
        // ping, lista e estatísticas chegam o tempo todo e não mexem nas linhas.
        let selection = self.selection;
        let (mut rows, mut visible, mut tail) = (false, true, false);
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
                let applied = if is_chat {
                    let (applied, changed) = self.accept_chat_frame(&frame.event, frame.data, window, cx);
                    (rows, visible, tail) = (changed == Changed::Rows, !matches!(changed, Changed::Nothing | Changed::Bottom), changed == Changed::Tail);
                    if changed == Changed::Bottom { self.redraw(panes::Area::Bottom, cx); }
                    applied
                } else if frame.event == "sessions" {
                    match serde_json::from_value(frame.data) {
                        Ok(sessions) => { self.list_error = None; self.replace_sessions(sessions, window, cx); true }
                        Err(_) => { self.list_error = Some(tr("invalid_response")); false }
                    }
                } else if frame.event == "list_error" { self.list_error = Some(tr("list_stale")); true }
                else { visible = false; true };
                let _ = frame.applied.send(applied);
            }
            Payload::History(revision, limit, result) if revision == self.revision && limit == self.history_limit => {
                rows = true;
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
                        let first = !self.history_installed;
                        self.history_installed = true;
                        for update in std::mem::take(&mut self.pending_chat) { self.apply_chat_update(update, window, cx); }
                        self.error = None;
                        self.ensure_commands(false);
                        self.discover_plan();
                        // A primeira conta de subagentes espera a conversa chegar, como o `aoAquecer` do web.
                        if first { self.restart_subagent_count(cx); }
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
                        for update in std::mem::take(&mut self.pending_chat) { self.apply_chat_update(update, window, cx); }
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
            Payload::Device(reply) => { self.receive_device(reply, cx); return; }
            Payload::Accounts(reply) => { self.receive_accounts(reply, window, cx); return; }
            Payload::Orchestration(reply) => { self.receive_orchestration(reply, cx); return; }
            Payload::Shortcuts(reply) => { self.receive_shortcuts(reply, cx); return; }
            Payload::Harness(reply) => { self.receive_harness(reply, cx); return; }
            Payload::ServerConfig(reply) => { self.receive_server_config(reply, window, cx); return; }
            Payload::Sync(reply) => { self.receive_sync(reply, window, cx); return; }
            Payload::Machines(reply) => { self.receive_machines(reply, window, cx); return; }
            Payload::Computer(reply) => { self.receive_computer(reply, window, cx); return; }
            Payload::Create(dialog, reply) => { self.receive_create(dialog, reply, window, cx); return; }
            Payload::Sidebar(reply) => { self.receive_sidebar(reply, window, cx); return; }
            Payload::Activity(reply) => { self.receive_activity(reply, cx); return; }
            Payload::FileView(reply) => { self.receive_file_view(reply, window, cx); return; }
            Payload::Dossier(key, seq, result) => { self.receive_dossier(key, seq, result, cx); return; }
            Payload::DesktopPalette(seq, result) => { self.receive_desktop_palette(seq, result, window, cx); return; }
            Payload::Sent(..) | Payload::Interrupted(..) | Payload::Acted(..) | Payload::Files(..) | Payload::UploadStep(..)
                | Payload::UploadsDone(..) | Payload::Saved(..) | Payload::ConnectionNotSaved(..) | Payload::Reply(..) | Payload::HeadlessPlan(..)
                | Payload::AppearanceSaved(..) | Payload::Backdrop(..) | Payload::BackdropPicked(..) | Payload::BackdropRemoved(..) => unreachable!(),
        }
        // Lista que trocou ou tirou a sessão aberta refaz a conversa.
        if rows || self.selection != selection { self.sync_rows(cx); }
        else if tail {
            self.sync_tail_rows(cx);
            self.redraw(panes::Area::Conversation, cx);
            return;
        }
        if visible { cx.notify(); }
    }

    fn receive_sent(&mut self, key: SessionKey, text: String, draft: String, result: Result<Delivery, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = match &result {
            Ok(delivery) if delivery.delivered => SendOutcome::Delivered,
            Ok(_) => SendOutcome::Queued,
            Err(error) if error.uncertain => SendOutcome::Uncertain,
            Err(error) => SendOutcome::Rejected(Self::failure(error)),
        };
        if !self.delivery.complete(&key, &text, outcome) { return; }
        if result.is_ok() { self.bridge_sending(key.clone(), cx); }
        self.sync_working_row(cx);
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
        // Aba com o foco, pela posição na lista antiga: se a sessão dela sumir, o foco não pode ficar numa alça morta.
        let focused_tab = self.sessions.iter().position(|s| self.tab_focus.get(&s.name).is_some_and(|f| f.is_focused(window)))
            .map(|ix| (ix, self.sessions[ix].name.clone()));
        self.sessions = sessions;
        // Cada aba guarda o próprio foco pela vida da sessão; aba de sessão que sumiu leva o dela junto.
        self.tab_focus.retain(|name, _| self.sessions.iter().any(|s| &s.name == name));
        for session in &self.sessions {
            if !self.tab_focus.contains_key(&session.name) { self.tab_focus.insert(session.name.clone(), cx.focus_handle().tab_stop(true)); }
        }
        // O foco passa para a aba que ficou naquela posição; sem abas, volta à raiz para os atalhos seguirem valendo.
        if let Some((ix, _)) = focused_tab.filter(|(_, name)| !self.tab_focus.contains_key(name)) {
            match self.sessions.len().checked_sub(1).map(|last| ix.min(last)) {
                Some(next) => {
                    if let Some(focus) = self.tab_focus.get(&self.sessions[next].name) { focus.focus(window, cx); }
                    self.tabs_scroll.scroll_to_item(next);
                }
                None => self.root_focus.focus(window, cx),
            }
        }
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
                    self.turn_seen = None;
                    self.reset_details();
                    self.cancel_preview_drop();
                    self.clear_visible_preview();
                    self.loading = false;
                    self.chat_online = false;
                    if !self.lost_while_renaming(&old.name) { self.error = Some(tr("session_gone")); }
                }
            }
        }
        self.sidebar_sessions_changed(window, cx);
    }

    /// Aplica um quadro do SSE da conversa. Devolve se ele valeu e o que mudou na tela.
    fn accept_chat_frame(&mut self, event: &str, data: serde_json::Value, window: &mut Window, cx: &mut Context<Self>) -> (bool, Changed) {
        let update = match event {
            "message" | "queue_confirmed" => serde_json::from_value(data).map(ChatUpdate::Message),
            "preview" => serde_json::from_value(data).map(ChatUpdate::Preview),
            "state" => serde_json::from_value(data).map(ChatUpdate::State),
            "ask_question" if data.is_null() => Ok(ChatUpdate::Question(None)),
            "ask_question" => serde_json::from_value(data.clone()).map(|payload| ChatUpdate::Question(Some(Ask::new(payload, &data)))),
            "reset" => Ok(ChatUpdate::Reset),
            "suggest" => {
                self.terminal_suggestion = data.get("text").and_then(Value::as_str).unwrap_or("").to_owned();
                return (true, Changed::Screen);
            }
            "stats" => {
                return match serde_json::from_value::<Option<Stats>>(data) {
                    // Só o rodapé do compositor lê as estatísticas.
                    Ok(stats) => { self.stats = stats; (true, Changed::Bottom) }
                    Err(_) => { self.error = Some(tr("invalid_response")); (false, Changed::Screen) }
                };
            }
            "pensamento" | "ferramenta" => {
                let Some(text) = data.get("text").and_then(|v| v.as_str()) else {
                    self.error = Some(tr("invalid_response"));
                    return (false, Changed::Screen);
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
            _ => return (true, Changed::Nothing),
        };
        let update = match update {
            Ok(update) => update,
            Err(_) => { self.error = Some(tr("invalid_response")); return (false, Changed::Screen); }
        };
        if !self.history_installed && !matches!(update, ChatUpdate::Reset) {
            self.pending_chat.push(update);
            return (true, Changed::Nothing);
        }
        // Estado não mexe nos eventos: redesenha (chip, "em execução") sem refazer a conversa.
        let changed = match &update {
            // O mesmo estado repetido não redesenha: a conversa guardada sairia do cache à toa.
            ChatUpdate::State(state) if *state == self.chat.state && self.chat.ask.is_none() => Changed::Nothing,
            ChatUpdate::State(_) => Changed::Screen,
            ChatUpdate::Preview(_) | ChatUpdate::Thinking(_) | ChatUpdate::LiveTool(_) => Changed::Tail,
            _ => Changed::Rows,
        };
        self.apply_chat_update(update, window, cx);
        (true, changed)
    }

    fn apply_chat_update(&mut self, update: ChatUpdate, window: &mut Window, cx: &mut Context<Self>) {
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
                    self.update_visible_preview(window, cx);
                }
                if self.chat.state.state != "working" { self.defer_preview_drop(cx); }
            }
            ChatUpdate::State(state) => {
                // Turno terminou: o plano do Claude com terminal pode ter mudado de arquivo.
                let finished = self.chat.state.state == "working" && state.state != "working";
                let resumed = self.chat.state.state == "awaiting_input" && state.state == "working";
                let turned = (self.chat.state.state == "working") != (state.state == "working");
                // Estado vazio é a conversa recém-aberta: o turno já corria, e quem conta é o último envio.
                if turned { self.turn_seen = (state.state == "working" && !self.chat.state.state.is_empty()).then(Instant::now); }
                if state.state == "working" { self.sent_until = None; }
                self.chat.update_state(state);
                self.sync_working_row(cx);
                if turned { self.restart_subagent_count(cx); }
                self.sync_activity(cx);
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
                self.turn_seen = None;
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
        self.table_column.clear();
        self.tables.clear();
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
        self.prepare_tools();
        let row = self.row_ids.iter().position(|id| id == &key || id.strip_prefix(PINNED) == Some(key.as_str())).or_else(|| {
            let events = &self.chat.events;
            self.items.iter().position(|item| match item {
                Item::Group { tools, .. } => tools.iter().any(|t| events[t.call].id == key),
                Item::Thinking { parts, .. } => parts.iter().any(|&i| events[i].id == key),
                _ => false,
            })
        // Chave de uma parte da linha ("<linha>#…"): passo da lista de tarefas, tabela da resposta.
        }).or_else(|| self.row_ids.iter().position(|id| key.strip_prefix(id.as_str()).is_some_and(|rest| rest.starts_with('#'))));
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
        self.preview_carry = 0.;
        self.preview_deadline = None;
        self.visible_preview = Preview::default();
    }

    fn update_visible_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.schedule_preview_tick(window, cx);
    }

    /// Um passo por quadro da tela: o texto anda no ritmo da mola da rolagem, não num relógio próprio.
    fn schedule_preview_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview_tick_scheduled || self.visible_preview.text == self.chat.preview.text { return; }
        self.preview_tick_scheduled = true;
        let (connection, selection, epoch) = (self.connection, self.selection, self.preview_tick_epoch);
        cx.on_next_frame(window, move |this, window, cx| {
            if this.connection != connection || this.selection != selection || this.preview_tick_epoch != epoch { return; }
            this.preview_tick_scheduled = false;
            this.advance_visible_preview(window, cx);
        });
    }

    fn advance_visible_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rest) = self.chat.preview.text.strip_prefix(&self.visible_preview.text) else {
            self.update_visible_preview(window, cx);
            self.sync_rows(cx);
            cx.notify();
            return;
        };
        let remaining_chars = rest.chars().count();
        if remaining_chars == 0 { (self.preview_last_tick, self.preview_carry) = (None, 0.); return; }
        let now = Instant::now();
        let elapsed = self.preview_last_tick.replace(now).map(|last| now.duration_since(last).as_secs_f64()).unwrap_or(1. / 60.);
        let remaining_time = self.preview_deadline.unwrap_or(now).saturating_duration_since(now).as_secs_f64().max(0.05);
        let pace = 160.0_f64.max(remaining_chars as f64 / remaining_time);
        let count;
        (count, self.preview_carry) = preview_step(self.preview_carry, pace, elapsed, remaining_chars);
        // Quadro sem caractere novo não muda a tela: só espera o próximo.
        if count == 0 { self.schedule_preview_tick(window, cx); return; }
        self.visible_preview.text.extend(rest.chars().take(count));
        // O texto que anda só existe nas linhas da conversa: as outras áreas ficam como estão.
        if !self.sync_preview_row(cx) { self.sync_tail_rows(cx); }
        self.redraw(panes::Area::Conversation, cx);
        if count < remaining_chars { self.schedule_preview_tick(window, cx); }
        else { (self.preview_last_tick, self.preview_carry) = (None, 0.); }
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
            cx.notify();
            return;
        };
        if !self.delivery.begin(key.clone(), text.clone(), known) { cx.notify(); return; }
        self.sync_working_row(cx);
        self.error = None;
        self.stop_feedback.remove(&key);
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = if steer { api.steer_text(&key.name, &text).await } else { api.send(&key.name, &text).await };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Sent(key, text, draft, result) }).await;
        });
        self.follow_engage(cx);
        // O envio muda o aviso, o botão e o erro da faixa de baixo, que é guardada entre quadros.
        cx.notify();
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
        self.command_panel = false;
        self.close_controls();
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
        let prompt = cx.prompt_for_new_path(&downloads_folder(), Some(&composer::safe_name(&name)));
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
        let a = appearance::get();
        self.activity = conversation::fold_activity(&self.chat.events);
        self.pinned = self.activity.running_agents().map(|agent| agent.call).collect();
        self.sync_activity(cx);
        self.items = conversation::build(&self.chat.events, conversation::View { thinking: a.thinking_tools, tasks: a.task_list,
            merge_thinking: a.tool_look == appearance::ToolLook::Tree }, &self.pinned);
        self.paired = conversation::pair_results(&self.chat.events).0;
        self.sync_tables(a.table_chart);
        self.sync_row_ids(true, cx);
        let provider = self.provider().0.to_owned();
        if matches!(provider.as_str(), "pi" | "omp" | "kimi") {
            let derived = interaction::ask_from_events(&self.chat.events, &provider)
                .filter(|ask| !ask.tool_use_id.as_deref().is_some_and(|id| self.tool_answered(id)));
            if self.chat.update_ask(derived) { self.ask_form = AskForm::default(); }
        }
    }

    /// Só as linhas do fim (pensamento, ferramenta e prévia ao vivo, trabalhando, agentes fixos) mudaram: os eventos são
    /// os mesmos, e os itens, o texto preparado e as assinaturas da última reconstrução continuam valendo.
    fn sync_tail_rows(&mut self, cx: &mut Context<Self>) {
        // Lista zerada (troca de sessão, reset) ainda sem os itens: só a reconstrução inteira sabe as linhas.
        let full = self.row_ids.len() < self.items.len();
        if full { self.sync_rows(cx) } else { self.sync_row_ids(false, cx) }
    }

    /// Ids e assinaturas das linhas, splice na lista e texto das linhas que mudaram. `full` = os itens foram refeitos;
    /// sem ele, só as linhas depois dos itens são comparadas.
    fn sync_row_ids(&mut self, full: bool, cx: &mut Context<Self>) {
        let events = &self.chat.events;
        let items = self.items.len();
        let (mut ids, mut signatures): (Vec<String>, Vec<String>) = if full {
            (self.items.iter().map(|item| item.id(events)).collect(), self.items.iter().map(|item| signature(item, events)).collect())
        } else { (self.row_ids[..items].to_vec(), self.row_signatures[..items].to_vec()) };
        if !self.chat.live_thinking.is_empty() { ids.push(LIVE_THINKING.into()); signatures.push(String::new()); }
        if let Some(tool) = &self.chat.live_tool { ids.push(LIVE_TOOL.into()); signatures.push(format!("{}{}", tool.name, tool.input)); }
        if !self.visible_preview.text.is_empty() { ids.push(PREVIEW.into()); signatures.push(String::new()); }
        if self.working_row_shown() { ids.push(WORKING.into()); signatures.push(String::new()); }
        for agent in self.activity.running_agents() { ids.push(format!("{PINNED}{}", events[agent.call].id)); signatures.push(String::new()); }
        let prefix = self.row_ids.iter().zip(&ids).take_while(|(a,b)| a == b).count();
        let suffix = self.row_ids[prefix..].iter().rev().zip(ids[prefix..].iter().rev()).take_while(|(a,b)| a == b).count();
        let spliced = prefix + suffix < self.row_ids.len() || prefix + suffix < ids.len();
        // Mesma linha com outro conteúdo (resultado que chegou, grupo que cresceu): altura muda.
        let previous: HashMap<&String, &String> = self.row_ids.iter().zip(&self.row_signatures).collect();
        let resized: Vec<usize> = ids.iter().zip(&signatures).enumerate()
            .filter(|(_, (id, signature))| previous.get(id).is_some_and(|old| old != signature)).map(|(index, _)| index).collect();
        let mut prepared = HashMap::new();
        let rewritten: Vec<(usize, String)> = ids.iter().enumerate().filter_map(|(index, id)| {
            if !full && index < items { return None; }
            let body = if id == PREVIEW { preview_source(&self.visible_preview) } else {
                let Some(Item::Event(i)) = self.items.get(index) else { return None };
                let message = prepare_message(&events[*i]);
                let Prepared::Message { markdown, .. } = &message else { unreachable!() };
                let body = markdown.clone();
                prepared.insert(id.clone(), message);
                body
            };
            self.rich.get(id).filter(|cached| cached.source != body).map(|_| (index, body))
        }).collect();
        if full {
            self.prepared = prepared;
            self.last_message = events.iter().rposition(|e| e.kind == "assistant_msg" || e.kind == "user_msg" && !e.queued());
            self.prepare_tools();
        }
        // Só linha que muda de altura puxa a mola: um quadro sem mudança não pode desgrudar a lista do fim.
        if spliced || !resized.is_empty() || !rewritten.is_empty() { self.follow_content_changed(cx); }
        if spliced { self.splice_rows(prefix..self.row_ids.len()-suffix, ids.len()-prefix-suffix); }
        for index in resized { self.list_state.remeasure_items(index..index + 1); }
        for (index, body) in rewritten {
            let Some(cached) = self.rich.get_mut(&ids[index]) else { continue };
            let added = if ids[index] == PREVIEW { body.strip_prefix(&cached.source).map(str::to_owned) } else { None };
            cached.source = body.clone();
            if let Some(added) = added {
                cached.view.update(cx, |view, cx| view.push_str(&added, cx));
            } else {
                cached.view.update(cx, |view, cx| view.set_text(&body, cx));
            }
            self.list_state.remeasure_items(index..index+1);
        }
        self.row_ids = ids;
        self.row_signatures = signatures;
        let rows: HashSet<&String> = self.row_ids.iter().collect();
        // Visões fora da lista (plano, diff do painel) usam linha "__…__" e saem só pelo limite do cache.
        self.rich.retain(|_, rich| rows.contains(&rich.row) || rich.row.starts_with("__"));
    }

    /// Passo do streaming com a linha da prévia já na lista: só ela muda, e o resto da conversa não é refeito
    /// a cada quadro. Devolve falso quando a linha ainda não existe e é preciso o `sync_rows` inteiro.
    fn sync_preview_row(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.row_ids.iter().rposition(|id| id == PREVIEW) else { return false };
        let body = preview_source(&self.visible_preview);
        let Some(cached) = self.rich.get(PREVIEW) else { return true };
        if cached.source == body { return true; }
        let added = body.strip_prefix(&cached.source).map(str::to_owned);
        self.follow_content_changed(cx);
        let Some(cached) = self.rich.get_mut(PREVIEW) else { return true };
        cached.source = body.clone();
        match added {
            Some(added) => cached.view.update(cx, |view, cx| view.push_str(&added, cx)),
            None => cached.view.update(cx, |view, cx| view.set_text(&body, cx)),
        }
        self.list_state.remeasure_items(index..index + 1);
        true
    }

    /// Tabelas das respostas gravadas que dão gráfico. Lidas aqui, quando as linhas mudam, e só para a
    /// resposta cuja fonte mudou; o desenho só consulta. Sem a opção, nada é lido nem guardado.
    fn sync_tables(&mut self, enabled: bool) {
        let mut old = std::mem::take(&mut self.tables);
        if !enabled { return; }
        let decimal = tr("decimal").chars().next().unwrap_or(',');
        let events = &self.chat.events;
        for item in &self.items {
            let Item::Event(i) = item else { continue };
            let event = &events[*i];
            if event.kind != "assistant_msg" || event.is_error == Some(true) { continue; }
            let source = safe_markdown(&composer::citation_markdown(&display_body(event)));
            let tables = match old.remove(&event.id) {
                Some((seen, tables)) if seen == source => tables,
                _ => crate::tables::read(&source, decimal).into(),
            };
            self.tables.insert(event.id.clone(), (source, tables));
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
        // O parse que termina já redesenha quem mostra o texto (o estado é lido no desenho); aqui só a
        // altura da linha é refeita, sem um segundo quadro para a janela inteira.
        let observer = cx.observe(&view, move |this, _, cx| {
            if let Some(i) = this.row_ids.iter().position(|id| id == &owner) { this.follow_content_changed(cx); this.list_state.remeasure_items(i..i+1); }
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
            (WORKING, _) => self.render_working(cx),
            (pin, _) if pin.starts_with(PINNED) => match self.pinned_call(&pin[PINNED.len()..]) {
                // Sem resultado: o do lançamento em segundo plano não é o fim.
                Some(call) => self.render_tool(Tool { call, result: None }, &id, cx),
                None => div().into_any_element(),
            },
            (_, Some(Item::Event(_))) => self.render_message(index, &id, cx),
            (_, Some(Item::Tool(tool))) => self.render_tool(tool, &id, cx),
            (_, Some(Item::Orphan(result))) => self.render_orphan(result, &id, cx),
            (_, Some(Item::Group { tools, .. })) => self.render_group(&id, &tools, cx),
            (_, Some(Item::Thinking { parts, .. })) => self.render_thinking(&id, &parts, cx),
            (_, Some(Item::Tasks { tasks, .. })) => self.render_tasks(&id, &tasks, cx),
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
            // O subagente acabou num erro de API: o resultado do Agent no pai não vem marcado como erro.
            _ if self.agent_failed(tool.call) => (tr("tool_failed"), theme::warning()),
            Some(result) => {
                let label = match self.result_lines(result) {
                    0 => tr("tool_done"), 1 => tr("tool_line"), lines => tr("tool_lines").replace("{n}", &lines.to_string()),
                };
                (label, theme::muted())
            }
            None if self.running(tool.call) => (tr("tool_running"), theme::accent()),
            None => (tr("tool_no_result"), theme::muted()),
        }
    }

    // Sem resultado só é "em execução" enquanto a sessão trabalha e nenhuma mensagem veio depois. Agente rodando é
    // "em execução" até o fim real, mesmo com a sessão ociosa.
    fn running(&self, call: usize) -> bool {
        self.pinned.contains(&call) || self.chat.state.state == "working" && self.last_message.is_none_or(|last| call > last)
    }

    fn pinned_call(&self, event_id: &str) -> Option<usize> {
        self.activity.running_agents().map(|agent| agent.call).find(|&call| self.chat.events[call].id == event_id)
    }

    /// A linha de trabalhando fica sob a última linha durante todo o turno, com pensamento, ferramenta ou texto chegando,
    /// e já no envio.
    fn working_row_shown(&self) -> bool { self.chat.state.state == "working" || self.sending_shown() }

    /// Envio pendente, ou entregue há pouco e ainda sem o turno: sem esta ponte a linha sairia e voltaria no meio.
    fn sending_shown(&self) -> bool {
        let Some(key) = self.selected_key() else { return false };
        self.chat.state.state != "working" && (self.delivery.pending(&key)
            || self.sent_until.as_ref().is_some_and(|(sent, until)| *sent == key && Instant::now() < *until))
    }

    /// O relógio só confere a linha no fim do prazo; um envio mais novo que troque o prazo não é desfeito por ele.
    fn bridge_sending(&mut self, key: SessionKey, cx: &mut Context<Self>) {
        self.sent_until = Some((key, Instant::now() + SENT_BRIDGE));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SENT_BRIDGE).await;
            this.update(cx, |this, cx| { this.sync_working_row(cx); cx.notify(); }).ok();
        }).detach();
    }

    /// Começo do turno: o que vier por último entre o último envio gravado e a virada vista ao vivo. Sem nenhum dos
    /// dois (conversa aberta no meio de um turno sem envio), não há o que contar.
    fn turn_start(&self) -> Option<Instant> {
        let sent = self.chat.events.iter().rev().find(|event| event.kind == "user_msg" && !event.queued()).and_then(|event| event.ts)
            .and_then(|ts| {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
                Instant::now().checked_sub(Duration::from_secs_f64((now - ts).max(0.)))
            });
        match (sent, self.turn_seen) { (Some(sent), Some(seen)) => Some(sent.max(seen)), (sent, seen) => sent.or(seen) }
    }

    /// Estado que chega pelo SSE não refaz a conversa: a linha de trabalhando entra ou sai por um splice, antes dos cartões fixos.
    fn sync_working_row(&mut self, cx: &mut Context<Self>) {
        let at = self.row_ids.iter().position(|id| id == WORKING);
        if at.is_some() == self.working_row_shown() { return; }
        self.follow_content_changed(cx);
        match at {
            Some(at) => {
                self.row_ids.remove(at);
                self.row_signatures.remove(at);
                self.splice_rows(at..at + 1, 0);
            }
            None => {
                let at = self.row_ids.iter().position(|id| id.starts_with(PINNED)).unwrap_or(self.row_ids.len());
                self.row_ids.insert(at, WORKING.into());
                self.row_signatures.insert(at, String::new());
                self.splice_rows(at..at, 1);
            }
        }
    }

    /// Marca, verbo e segundos numa linha só, de altura fixa: o texto que muda não remede a lista. Marca e segundos
    /// animam fora da conversa guardada (`working_mark_float`); aqui ficam só os lugares deles.
    fn render_working(&self, cx: &mut Context<Self>) -> AnyElement {
        let sending = self.sending_shown();
        let verb = if sending { tr("sending") } else { working_verb(self.chat.state.label.as_deref()) };
        let since = if sending { None } else { self.turn_start() };
        // Sem recuo: a marca começa na borda da coluna, alinhada com o texto das mensagens.
        let row = div().relative().h(px(38.)).flex().items_center().gap(px(8.))
            .child(self.working_mark_slot(panes::Area::Conversation, "working-line", 14., theme::accent()))
            .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(verb))
            .when_some(since, |el, since| el.child(self.elapsed_slot(panes::Area::Conversation, "working-elapsed", since)));
        if cx.reduce_motion() { return row.into_any_element(); }
        row.with_animation("working-line-in", Animation::new(WORKING_FADE).with_easing(chrome::ease_out),
            |el, t| el.opacity(t).top(px(6. * (1. - t)))).into_any_element()
    }

    /// "Ir para o fim" flutuando no pé da conversa, centrada na coluna, só enquanto ela está solta e longe do fim.
    fn render_jump_pill(&self, cx: &mut Context<Self>) -> AnyElement {
        // Hover opaco também, um toque da cor do texto sobre o fundo: o `hover()` do tema é translúcido na caixa solta.
        let hover = theme::elevated().blend(theme::text().alpha(0.06));
        let pill = Button::new("jump-latest")
            .custom(ButtonCustomVariant::new(cx).color(theme::elevated()).foreground(theme::text()).hover(hover).active(hover))
            // O variante pinta a cor misturada com transparente; opaco vem daqui, senão o texto da conversa aparece através.
            // `elevated`, um degrau acima da conversa: com `raised` a pílula sumia no fundo.
            .bg(theme::elevated()).h(px(30.)).pl(px(11.)).pr(px(13.)).rounded(px(15.)).border_1().border_color(theme::border_strong())
            .shadow(theme::popover_shadow()).text_size(px(13.))
            .icon(Icon::new(IconName::ArrowDown).size(px(13.)).text_color(theme::faint())).label(tr("latest"))
            .on_click(cx.listener(|this, _, _, cx| this.follow_engage(cx)));
        let wrap = div().absolute().left_0().right_0().bottom(px(16.)).flex().justify_center().child(pill);
        if cx.reduce_motion() { return wrap.into_any_element(); }
        wrap.with_animation("jump-latest-in", Animation::new(WORKING_FADE).with_easing(chrome::ease_out),
            |el, t| el.opacity(t).bottom(px(10. + 6. * t))).into_any_element()
    }

    fn render_tool(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> AnyElement {
        if appearance::get().tool_look == appearance::ToolLook::Chips { return self.render_single_chip(tool, row, cx); }
        let call = &self.chat.events[tool.call];
        let key = call.id.clone();
        let name = call.tool_name.clone().unwrap_or_else(|| tr("tool"));
        let summary = conversation::summarize_input(call.tool_name.as_deref(), call.tool_input.as_ref());
        let (status, status_color) = self.tool_status(tool);
        let error = tool.result.is_some_and(|i| self.chat.events[i].is_error == Some(true)) || self.agent_failed(tool.call);
        let open = self.expanded.contains(&key);
        let toggle_key = key.clone();
        // O cartão Agent abre a conversa dele na aba Atividade em vez de expandir: ↗ no lugar da seta de abrir, e a
        // marca animada enquanto o subagente roda.
        let agent = activity::agent_request(call);
        let is_agent = agent.is_some();
        let running = is_agent && tool.result.is_none() && !error && self.running(tool.call);
        let label = if agent.is_some() { format!("{}: {summary}. {status}", activity::web("tool_abrir_agente")) } else { format!("{name}: {summary}. {status}") };
        let button = if agent.is_some() {
            Button::new(SharedString::from(format!("toggle-{key}"))).ghost().small().w_full().h(px(34.)).px(px(10.)).rounded(px(0.))
                .child(chrome::small_icon(IconName::Bot, 14., if error { theme::warning() } else { theme::muted() }))
        } else { self.disclosure(&key, open) };
        let header = button
            .accessibility_label(label)
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::text() }).child(name))
            .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(summary))
            .when(running, |el| el.child(self.working_mark_slot(panes::Area::Conversation, format!("agent-{key}"), 12., theme::accent())))
            .child(div().flex_shrink_0().max_w(px(320.)).truncate().text_color(status_color).child(status))
            .when(agent.is_some(), |el| el.child(chrome::small_icon(IconName::ExternalLink, 14., theme::faint())))
            .on_click(cx.listener(move |this, _, _, cx| match &agent {
                Some(request) => this.open_agent(request.clone(), cx),
                None => this.toggle(toggle_key.clone(), cx),
            }));
        // O cartão Agent é um objeto próprio, em caixa como no modo Chips; as outras chamadas seguem linha.
        if is_agent { return rows::chip_box().child(header).into_any_element(); }
        let body = open.then(|| self.tool_body(tool, row, cx).pl_6());
        div().flex().flex_col().child(header).children(body).into_any_element()
    }

    /// Entrada e resultado da chamada aberta; o mesmo conteúdo no Clássico e nos Chips.
    fn tool_body(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> Div {
        let call = &self.chat.events[tool.call];
        let key = call.id.clone();
        let error = tool.result.is_some_and(|i| self.chat.events[i].is_error == Some(true));
        let input_key = format!("{key}:input");
        let input = self.prepared_detail(&input_key, || conversation::pretty_input(call.tool_input.as_ref()));
        let mut body = div().flex().flex_col().gap_2().pt_1().pb_2();
        // Imagem que o Read leu: o transcript não traz os bytes, o caminho citado vem pelo `/file` (regra do web).
        if call.tool_name.as_deref().is_some_and(|name| name.eq_ignore_ascii_case("read")) {
            let path = call.tool_input.as_ref().and_then(|input| input.get("file_path").or_else(|| input.get("path"))).and_then(|path| path.as_str()).unwrap_or("");
            let refs: Vec<_> = composer::cited_paths(path).into_iter().map(|path| {
                let name = composer::basename(&path).to_owned();
                let image = composer::image_format(&name).is_some();
                (Source::Cited(path), name, image)
            }).collect();
            if !refs.is_empty() { body = body.child(self.render_refs(&format!("{row}-read"), refs, cx)); }
        }
        if matches!(input, Prepared::Detail { total, .. } if total > 0) {
            body = body.child(self.detail(row, &input_key, input, tr("tool_input"), tr("copy_input"), false, cx));
        }
        match tool.result {
            Some(i) => {
                let result_key = format!("{key}:result");
                let result = self.prepared_detail(&result_key, || self.chat.events[i].result.clone().unwrap_or_default());
                body.child(self.detail(row, &result_key, result, tr("tool_output"), tr("copy_result"), error, cx))
            }
            None => body.child(div().text_sm().text_color(theme::muted()).child(self.tool_status(tool).0)),
        }
    }

    /// Linhas do resultado aparado, contadas no `prepare_tools`; faltando, conta aqui sem guardar.
    fn result_lines(&self, result: &ChatEvent) -> usize {
        match self.prepared.get(&format!("{}:lines", result.id)) {
            Some(Prepared::Lines(lines)) => *lines,
            _ => count_lines(result),
        }
    }

    /// Detalhe aberto preparado no `prepare_tools`; faltando, monta aqui sem guardar.
    fn prepared_detail(&self, key: &str, full: impl FnOnce() -> String) -> Prepared {
        match self.prepared.get(key) {
            Some(detail @ Prepared::Detail { .. }) => detail.clone(),
            _ => prepare_detail(full()),
        }
    }

    /// Contar, cortar e cercar uma saída grande pesa: faz uma vez por mudança do chat ou abertura de detalhe,
    /// nunca no desenho. Só insere o que falta.
    fn prepare_tools(&mut self) {
        let (events, prepared, expanded) = (&self.chat.events, &mut self.prepared, &self.expanded);
        let mut tools = Vec::new();
        let mut orphans = Vec::new();
        for item in &self.items {
            match item {
                Item::Tool(tool) => tools.push(*tool),
                // O raciocínio que a Árvore põe no grupo não tem entrada nem resultado a preparar.
                Item::Group { tools: group, .. } => tools.extend(group.iter().copied().filter(|t| events[t.call].kind != "thinking")),
                Item::Thinking { parts, .. } => tools.extend(parts.iter().filter(|&&i| events[i].kind != "thinking")
                    .map(|&i| Tool { call: i, result: self.paired.get(&i).copied() })),
                Item::Orphan(i) => orphans.push(*i),
                Item::Event(_) | Item::Tasks { .. } => {}
            }
        }
        for tool in tools {
            let call = &events[tool.call];
            if let Some(result) = tool.result.map(|i| &events[i]) {
                prepared.entry(format!("{}:lines", result.id)).or_insert_with(|| Prepared::Lines(count_lines(result)));
            }
            if !expanded.contains(&call.id) { continue; }
            prepared.entry(format!("{}:input", call.id)).or_insert_with(|| prepare_detail(conversation::pretty_input(call.tool_input.as_ref())));
            if let Some(result) = tool.result.map(|i| &events[i]) {
                prepared.entry(format!("{}:result", call.id)).or_insert_with(|| prepare_detail(result.result.clone().unwrap_or_default()));
            }
        }
        for i in orphans {
            let event = &events[i];
            if expanded.contains(&event.id) {
                prepared.entry(format!("{}:result", event.id)).or_insert_with(|| prepare_detail(event.result.clone().unwrap_or_default()));
            }
        }
    }

    fn detail(&mut self, row: &str, key: &str, detail: Prepared, label: String, copy_label: String, error: bool, cx: &mut Context<Self>) -> AnyElement {
        let Prepared::Detail { fenced, total, clipped, full } = detail else { return div().into_any_element() };
        let view = self.text_view(key, row, fenced, cx);
        let note = clipped.then(|| tr("clipped").replace("{shown}", &DETAIL_MAX.to_string()).replace("{total}", &total.to_string()));
        div().flex().flex_col().gap_1()
            .child(div().flex().items_center().justify_between()
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::muted() }).child(label))
                .child(Button::new(SharedString::from(format!("copy-{key}"))).ghost().xsmall().icon(IconName::Copy).label(copy_label)
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(full.to_string())))))
            .child(TextView::new(&view).selectable(true).scrollable(false))
            .when_some(note, |el, note| el.child(div().text_xs().text_color(theme::muted()).child(note)))
            .into_any_element()
    }

    fn render_orphan(&mut self, index: usize, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let event = &self.chat.events[index];
        let key = event.id.clone();
        let error = event.is_error == Some(true);
        let first = event.result.as_deref().unwrap_or("").lines().map(str::trim).find(|l| !l.is_empty()).map(|l| conversation::one_line(l, 96)).unwrap_or_default();
        let open = self.expanded.contains(&key);
        let toggle_key = key.clone();
        let header = self.disclosure(&key, open)
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::text() }).child(tr("tool_orphan")))
            .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(first))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let result_key = format!("{key}:result");
        let detail = open.then(|| {
            let detail = self.prepared_detail(&result_key, || self.chat.events[index].result.clone().unwrap_or_default());
            self.detail(row, &result_key, detail, tr("tool_output"), tr("copy_result"), error, cx)
        });
        div().flex().flex_col().child(header)
            .when_some(detail, |el, detail| el.child(div().pl_6().pt_1().pb_2().child(detail)))
            .into_any_element()
    }

    fn render_group(&mut self, row: &str, tools: &[Tool], cx: &mut Context<Self>) -> AnyElement {
        if appearance::get().tool_look == appearance::ToolLook::Chips { return self.render_chip_group(row, tools, cx); }
        if appearance::get().tool_look == appearance::ToolLook::Tree { return self.render_tree_group(row, tools, cx); }
        let events = &self.chat.events;
        let names: Vec<String> = tools.iter().map(|t| events[t.call].tool_name.clone().unwrap_or_else(|| tr("tool"))).collect();
        let mut distinct = names.clone();
        distinct.dedup();
        let label = if distinct.len() == 1 { format!("{} · {}", names[0], tools.len()) } else { tr("tools_count").replace("{n}", &tools.len().to_string()) };
        let summary = if distinct.len() == 1 {
            tools.last().map(|t| conversation::summarize_input(events[t.call].tool_name.as_deref(), events[t.call].tool_input.as_ref())).unwrap_or_default()
        } else { conversation::one_line(&distinct.join(", "), 96) };
        // O Agent cujo subagente falhou conta como erro, como no cartão dele.
        let errors = tools.iter().filter(|t| t.result.is_some_and(|i| events[i].is_error == Some(true)) || self.agent_failed(t.call)).count();
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
        // O carregador de ferramentas entra no bloco mas não conta nem aparece, como no web.
        let calls: Vec<&ChatEvent> = parts.iter().map(|&i| &events[i]).filter(|e| e.kind != "thinking" && e.tool_name.as_deref() != Some("ToolSearch")).collect();
        let searches_only = calls.iter().all(|e| conversation::is_search(e.tool_name.as_deref()));
        let count = match (calls.len(), searches_only) {
            (0, _) => None,
            (1, true) => Some(tr("thinking_search")),
            (n, true) => Some(tr("thinking_searches").replace("{n}", &n.to_string())),
            (1, false) => Some(tr("thinking_call")),
            (n, false) => Some(tr("thinking_calls").replace("{n}", &n.to_string())),
        };
        let has_calls = parts.len() > thoughts.len();
        let open = self.expanded.contains(row);
        let toggle_key = row.to_owned();
        let header = self.disclosure(row, open)
            .accessibility_label(format!("{}: {summary}", tr("thinking")))
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr("thinking")))
            .child(div().flex_1().min_w_0().truncate().italic().text_color(theme::muted()).child(summary))
            .when_some(count, |el, count| el.child(div().flex_shrink_0().text_color(theme::muted()).child(count)))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let mut body: Vec<AnyElement> = Vec::new();
        if open {
            // Só os pares das chamadas deste bloco, tirados do mapa refeito no `sync_rows`.
            let paired: HashMap<usize, usize> = if has_calls { parts.iter().filter_map(|&i| self.paired.get(&i).map(|&r| (i, r))).collect() } else { HashMap::new() };
            for &i in parts {
                let event = &self.chat.events[i];
                if event.tool_name.as_deref() == Some("ToolSearch") { continue; }
                if event.kind == "thinking" {
                    let key = format!("{}:thought", event.id);
                    let view = self.text_view(&key, row, safe_markdown(event.text.as_deref().unwrap_or("")), cx);
                    body.push(chat_text(&view, cx).text_color(theme::muted()).into_any_element());
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
            .child(chat_text(&view, cx).text_sm().text_color(theme::muted()))
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
                .child(scrolled("plan-scroll", &self.plan_scroll.1, 320., div().p_3().child(TextView::new(&view).selectable(true).scrollable(false).code_block_actions(copy_code)))))
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
        let images: Vec<Source> = refs.iter().filter(|(_, _, image)| *image).map(|(source, _, _)| source.clone()).collect();
        let mut list = div().flex().flex_col().gap_2();
        let mut shown = 0;
        for (n, (source, name, image)) in refs.into_iter().enumerate() {
            let preview = if image {
                self.ensure_media(&source);
                let state = key.as_ref().and_then(|key| self.media.get(&(key.clone(), source.clone())));
                let thumb = match state {
                    Some(MediaState::Image(picture)) => div().max_w(px(320.)).max_h(px(240.)).rounded_md().overflow_hidden()
                        // O id guarda o quadro corrente: sem ele a GPUI não anima o GIF. Fora da tela não é desenhado e para.
                        .child(img(picture.clone()).max_w(px(320.)).max_h(px(240.)).object_fit(ObjectFit::Contain)
                            .id(SharedString::from(format!("thumb-image-{row}-{n}")))).into_any_element(),
                    Some(MediaState::Failed(reason)) => div().text_xs().text_color(theme::warning())
                        .child(tr("media_failed").replace("{name}", &name).replace("{reason}", reason)).into_any_element(),
                    _ => div().w(px(160.)).h(px(96.)).rounded_md().bg(theme::raised()).flex().items_center().justify_center()
                        .text_xs().text_color(theme::muted()).child(tr("media_loading")).into_any_element(),
                };
                // Focável para o Esc do visor devolver o foco aqui; Enter abre como o clique.
                let (open_key, sources, index) = (key.clone(), images.clone(), shown);
                shown += 1;
                Some(div().id(SharedString::from(format!("thumb-{row}-{n}"))).focusable().tab_stop(true).cursor_pointer()
                    .self_start().rounded_md().border_1().border_color(transparent_black())
                    .focus_visible(|el| el.border_color(theme::accent_focus()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(key) = open_key.clone() { this.open_image(key, sources.clone(), index, window, cx); }
                    }))
                    .child(thumb))
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
            None => popup::skeleton("recent-loading", 3).into_any_element(),
            Some(Err(reason)) => div().px(px(8.)).text_sm().text_color(theme::warning()).child(reason.clone()).into_any_element(),
            Some(Ok(files)) if files.is_empty() => div().px(px(8.)).text_sm().text_color(theme::muted()).child(tr("recent_empty")).into_any_element(),
            Some(Ok(files)) => div().flex().flex_col().children(files.iter().enumerate().map(|(n, file)| {
                let name = file.filename.clone();
                popup::row(SharedString::from(format!("recent-{n}")), false)
                    .child(div().flex_1().min_w_0().truncate().child(file.filename.clone()))
                    .child(div().flex_shrink_0().text_xs().text_color(theme::muted()).child(human_size(file.size)))
                    .on_click(cx.listener(move |this, _, _, cx| this.reattach(name.clone(), cx)))
            })).into_any_element(),
        };
        Some(div().p(px(popup::INSET)).rounded_md().bg(theme::raised()).flex().flex_col().gap(px(2.))
            .child(popup::title(tr("recent_title"), Some("esc")))
            .child(div().id("recent-list").max_h(px(220.)).overflow_y_scroll().child(body))
            .into_any_element())
    }

    fn render_command_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.command_search.read(cx).value().to_lowercase();
        let cache = self.commands_key().and_then(|key| self.commands.get(&key));
        let body = match cache {
            None => popup::skeleton("commands-loading", 4).into_any_element(),
            Some(Err(reason)) => div().px(px(8.)).flex().items_center().gap_2().text_sm().text_color(theme::warning())
                .child(div().flex_1().min_w_0().child(tr("commands_failed").replace("{reason}", reason)))
                .child(Button::new("commands-retry").ghost().xsmall().flex_shrink_0().label(tr("retry")).on_click(cx.listener(|this, _, _, cx| { this.ensure_commands(true); cx.notify(); })))
                .into_any_element(),
            Some(Ok(list)) => {
                let matches: Vec<&CommandInfo> = list.iter().filter(|c| query.is_empty() || c.name.to_lowercase().contains(&query)
                    || c.description.as_deref().is_some_and(|d| d.to_lowercase().contains(&query))).collect();
                if matches.is_empty() { div().px(px(8.)).text_sm().text_color(theme::muted()).child(tr("commands_empty")).into_any_element() }
                else {
                    let mut groups = div().flex().flex_col().gap(px(2.));
                    for source in ["builtin", "skill", "plugin"] {
                        let items: Vec<&&CommandInfo> = matches.iter().filter(|c| c.source == source || source == "plugin" && !matches!(c.source.as_str(), "builtin" | "skill")).collect();
                        if items.is_empty() { continue; }
                        let group = tr(&format!("commands_{source}"));
                        groups = groups.child(popup::title(group.clone(), None));
                        // Linha do web: nome em mono e descrição embaixo; argumentos e selo da origem à direita.
                        for command in items {
                            let picked = (*command).clone();
                            let description = command.description.clone().unwrap_or_default();
                            groups = groups.child(popup::row(SharedString::from(format!("command-{}", command.name)), false)
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
        div().p(px(popup::INSET)).rounded_md().bg(theme::raised()).flex().flex_col().gap(px(2.))
            .child(popup::title(tr("commands"), Some("esc")))
            .child(div().px(px(4.)).pb(px(4.)).child(Input::new(&self.command_search)))
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
        let confirm = self.render_confirm(cx);
        let (pills, mode) = self.render_ctl_pills(readable, cx);
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

        // Aviso e sugestões seguem o que se digita: ficam presos à borda de cima, sem cortina. Os painéis abertos por
        // botão moram na camada da raiz (`popup.rs`), presos ao botão.
        let floating: Vec<AnyElement> = confirm.map(|el| chrome::popover(el, false)).into_iter()
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
            let cost = status.as_ref().and_then(|s| s.cost_usd).map(|usd| self.money(usd));
            let stats = self.stats.as_ref().filter(|_| readable).map(side::stats_line);
            // Anéis de contexto e de uso da conta (janela de 5h); sem dado dizem isso, nunca 0%.
            let percent = |pct: Option<f64>| pct.map(|p| format!("{}%", p.round())).unwrap_or_else(|| tr("no_data"));
            let limits = status.as_ref().filter(|_| readable).map(|s| [(tr("limit_5h"), s.five_hour_pct), (tr("limit_7d"), s.weekly_pct)]
                .into_iter().map(|(label, pct)| format!("{label} {}", percent(pct))).collect::<Vec<_>>().join(" · "));
            let account = status.as_ref().and_then(|s| s.five_hour_pct).filter(|_| readable);
            let ring = |id: &'static str, pct: Option<f64>, tip: String| div().id(id).flex_shrink_0().flex().items_center().gap(px(5.))
                .child(chrome::ring(pct)).child(percent(pct))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx));
            let ctx_tip = [Some(format!("{} {}", tr("ring_context"), percent(ctx_pct))), stats].into_iter().flatten().collect::<Vec<_>>().join("\n");
            let account_tip = format!("{}: {}", tr("ring_account"), limits.unwrap_or_else(|| tr("no_data")));
            div().pt(px(7.)).px(px(6.)).flex().items_center().gap(px(6.)).text_xs().text_color(theme::faint())
                .when_some(folder, |el, f| el.child(chrome::small_icon(IconName::Folder, 14., theme::faint())).child(div().max_w(px(200.)).truncate().child(f)))
                .when(!branch.is_empty(), |el| el.child(div().ml(px(4.)).flex().items_center().gap(px(4.)).min_w_0()
                    .child(chrome::small_icon(IconName::GitBranch, 14., theme::faint()))
                    .child(div().max_w(px(160.)).truncate().child(branch))
                    .when(dirty, |el| el.child(div().text_color(theme::warning()).child("*")))))
                .when_some(added, |el, a| el.child(div().text_color(theme::success()).child(format!("+{a}"))))
                .when_some(removed, |el, r| el.child(div().text_color(theme::removed()).child(format!("−{r}"))))
                .child(div().flex_1())
                // A linha de estatísticas do turno fica na dica do anel de contexto.
                .child(ring("composer-ctx", ctx_pct, ctx_tip))
                .child(div().ml(px(6.)).child(ring("composer-account", account, account_tip)))
                .when_some(cost, |el, cost| el.child(div().flex_shrink_0().child("·")).child(div().flex_shrink_0().child(cost)))
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
        let recent_btn = popup::anchor(div(), "attach-recent").child(chrome::icon_button("attach-recent", IconName::RotateCcwClock, tr("attach_recent"), cx)
            .disabled(!readable || uploading.is_some()).selected(self.recent.is_some()).on_click(cx.listener(|this, _, _, cx| this.open_recent(cx))));
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
            .child(popup::anchor(div().relative().w_full().max_w(px(column_width())).flex().flex_col(), "composer")
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
        else if self.close_popups() {}
        else if self.can_interrupt() { self.confirm = Some(Confirm::Stop); }
        else { return; }
        cx.stop_propagation();
        cx.notify();
    }

    /// Texto que "Copiar" leva de uma linha de mensagem: o corpo gravado, ou a prévia como está na tela.
    fn copy_text(&self, id: &str) -> Option<String> {
        if id == PREVIEW { return Some(self.visible_preview.text.clone()); }
        self.chat.events.iter().find(|event| event.id == id).map(ChatEvent::body)
    }

    /// Notificação de subagente do Codex: o desfecho no cabeçalho, o relatório em markdown e a notificação crua num
    /// bloco fechado, porque o cartão pode ler errado e o texto original não.
    fn render_codex_card(&mut self, id: &str, event: usize, card: cards::CodexSubagent, markdown: String, cx: &mut Context<Self>) -> AnyElement {
        let failed = card.failed();
        let title = match card.status.as_str() {
            "completed" => activity::web("subagente_card_concluido"),
            _ if failed => activity::web("subagente_card_falhou"),
            status => crate::i18n::tr_web("subagente_card_status", &HashMap::from([("s".to_owned(), status.to_owned())]))
                .unwrap_or_else(|| status.to_owned()),
        };
        // Só os 8 primeiros: o agent_path é um uuid inteiro e come a linha do cabeçalho.
        let short: String = card.agent_path.chars().take(8).collect();
        let raw_key = format!("{id}#raw");
        let open = self.expanded.contains(&raw_key);
        let event = &self.chat.events[event];
        // O cru só aberto, e com o mesmo teto dos detalhes das chamadas.
        let raw = open.then(|| {
            let body = event.body();
            let (shown, clipped) = conversation::clip(body.trim(), DETAIL_MAX);
            let note = clipped.then(|| tr("clipped").replace("{shown}", &DETAIL_MAX.to_string())
                .replace("{total}", &body.trim().chars().count().to_string()));
            (shown.to_owned(), note)
        });
        let time = clock(event.ts);
        let view = self.text_view(id, id, markdown, cx);
        let tint = if failed { theme::danger() } else { theme::muted() };
        let header = div().flex().items_center().gap_2().px_3().py_2().bg(theme::inset())
            .child(Icon::new(IconName::Bot).size_4().flex_shrink_0().text_color(tint))
            .child(div().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(if failed { theme::danger() } else { theme::text() }).child(title))
            .when(!short.is_empty(), |el| el.child(div().flex_shrink_0().px(px(6.)).py(px(1.)).rounded_full().bg(theme::raised())
                .font_family(theme::MONO).text_size(px(10.5)).text_color(theme::muted()).child(short)))
            .when_some(time, |el, time| el.child(div().ml_auto().flex_shrink_0().text_size(px(10.5)).text_color(theme::muted()).child(time)));
        let toggle_key = raw_key.clone();
        let original = self.disclosure(&raw_key, open)
            .child(div().flex_1().text_xs().text_color(theme::muted()).child(activity::web("subagente_card_original")))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        // Relatório de outra ferramenta num bloco estreito: letra pequena e títulos no tamanho do texto, como no web.
        let report_style = gpui_kit::component::text::TextViewStyle::default().heading_font_size(|level, _| px(if level <= 1 { 13. } else { 12. }));
        let body = div().flex().flex_col().gap_2().px_3().pt_2().pb_3()
            .child(TextView::new(&view).selectable(true).scrollable(false).text_xs().text_color(theme::muted()).style(report_style)
                .code_block_actions(copy_code).on_link_click(open_web_link)
                .markdown_extensions(citation_extensions(&id, cx.weak_entity())))
            .child(div().flex().flex_col().pt_1().border_t_1().border_color(theme::border()).child(original)
                .when_some(raw, |el, (raw, note)| el.child(div().px_2().pt_1().font_family(theme::MONO).text_xs().text_color(theme::muted()).child(raw))
                    .when_some(note, |el, note| el.child(div().px_2().pt_1().text_xs().text_color(theme::muted()).child(note)))));
        let row = div().id(SharedString::from(format!("message-{id}"))).w_full().flex()
            .child(div().max_w(relative(0.8)).min_w(px(280.)).flex().flex_col().overflow_hidden().rounded_md().bg(theme::raised())
                .border_1().border_color(if failed { theme::danger().opacity(0.45) } else { theme::border() })
                .child(header).child(body));
        with_copy_menu(row, id.to_owned(), cx.weak_entity())
    }

    fn render_message(&mut self, index: usize, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let id = id.to_owned();
        let mut discard = None;
        let mut baton = None;
        // Texto preparado quando o chat mudou; a prévia usa a fonte que o passo do streaming já montou.
        let (markdown, blank) = if id == PREVIEW {
            let markdown = self.rich.get(&id).map(|rich| rich.source.clone()).unwrap_or_else(|| preview_source(&self.visible_preview));
            (markdown, self.visible_preview.text.trim().is_empty())
        } else {
            let Some(Item::Event(event_index)) = self.items.get(index) else { return div().into_any_element(); };
            let local;
            let message = match self.prepared.get(&id) {
                Some(message) => message,
                None => { local = prepare_message(&self.chat.events[*event_index]); &local }
            };
            let Prepared::Message { markdown, blank, card, .. } = message else { return div().into_any_element(); };
            match card.clone() {
                Some(cards::Card::Codex(card)) => return self.render_codex_card(&id, *event_index, card, markdown.clone(), cx),
                Some(cards::Card::Baton(card)) => baton = Some((*event_index, card)),
                None => {}
            }
            (markdown.clone(), *blank)
        };
        let (label, note, user, error) = if id == PREVIEW {
            // "Trabalhando" é da linha de baixo, que segue sob o texto chegando.
            (tr("assistant"), None, false, false)
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
            (label, (!notes.is_empty()).then(|| notes.join(" · ")), event.kind == "user_msg", event.is_error == Some(true))
        };
        let busy = self.selected_key().is_some_and(|key| self.flight.busy(&key));
        let discard = discard.map(|entry| Button::new(format!("discard-{id}")).small().ghost().label(tr("queue_discard")).disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| this.act(Action::Discard(entry.clone()), entry.clone(), cx))));
        // O bastão chega pela fila: o cartão fica no lugar da bolha, e a nota de entrega (com o Descartar) embaixo dele.
        if let Some((event, card)) = baton {
            let card = self.render_baton_card(&id, event, card, cx);
            let row = div().id(SharedString::from(format!("message-{id}"))).w_full().flex().flex_col().gap_2().child(card)
                .when_some(note, |el, note| el.child(div().flex().items_center().gap_2()
                    .child(div().min_w_0().text_sm().text_color(theme::warning()).child(note))
                    .when_some(discard, |el, button| el.child(button))));
            return with_copy_menu(row, id, cx.weak_entity());
        }
        // Conversa sem cartões: usuário em bolha à direita, agente em texto corrido. Só o que não é nenhum dos
        // dois (erro, aviso, formato desconhecido) mantém o rótulo, porque ali o rótulo é informação.
        let plain = id == PREVIEW || (kind_of(&self.items, index, &self.chat.events) == Some("assistant_msg") && !error);
        // Resposta gravada com tabela numérica: o texto vai em trechos, e cada tabela ganha o botão Gráfico.
        // Só o retrato do `sync_tables`, e só com a mesma fonte que está sendo desenhada; a prévia nunca tem gráfico.
        let charted = (plain && id != PREVIEW && appearance::get().table_chart).then(|| self.tables.get(&id)).flatten()
            .filter(|(source, tables)| *source == markdown && !tables.is_empty()).map(|(_, tables)| tables.clone());
        let refs = match self.items.get(index) {
            Some(Item::Event(i)) if id != PREVIEW => attachment_refs(&self.chat.events[*i]),
            _ => Vec::new(),
        };
        let files = (!refs.is_empty()).then(|| self.render_refs(&id, refs, cx));
        let more_key = format!("{id}#more");
        let long = user && long_message(&markdown);
        let open = self.expanded.contains(&more_key);
        let text: Vec<AnyElement> = match charted {
            Some(tables) => self.render_charted(&id, &markdown, &tables, cx),
            None if !blank || files.is_none() => {
                let view = self.text_view(&id, &id, markdown, cx);
                let text = chat_text(&view, cx).motion(stream_motion(id == PREVIEW)).on_link_click(open_web_link)
                    .markdown_extensions(citation_extensions(&id, cx.weak_entity()));
                vec![collapse(text, long, open).into_any_element()]
            }
            None => Vec::new(),
        };
        let more = long.then(|| more_button(format!("more-{id}"), open)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(more_key.clone(), cx))));
        // Hora e copiar sob a mensagem, só ao passar o mouse; a faixa é reservada para a lista não remedir.
        let actions = (id != PREVIEW && (user || plain)).then(|| {
            let ts = match self.items.get(index) { Some(Item::Event(i)) => self.chat.events[*i].ts, _ => None };
            let (view, copy_id) = (cx.weak_entity(), id.clone());
            div().h(px(24.)).flex().items_center().gap_1().opacity(0.).group_hover(ROW_GROUP, |s| s.opacity(1.))
                .when_some(stamp(ts), |el, at| el.child(div().text_xs().text_color(theme::faint()).child(at)))
                .child(CopyButton {
                    id: SharedString::from(format!("copy-{id}")).into(),
                    value: std::rc::Rc::new(move |cx| view.upgrade().and_then(|view| view.read(cx).copy_text(&copy_id)).unwrap_or_default()),
                })
        });
        let content = conversation_text(div().flex().flex_col().gap_2())
            .when(!user && !plain, |el| el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::muted() }).child(label)))
            .children(text)
            .when_some(more, |el, more| el.child(more))
            .when_some(files, |el, files| el.child(files));
        let row = div().id(SharedString::from(format!("message-{id}"))).group(ROW_GROUP).w_full().flex().flex_col().gap_2()
            .map(|el| if user { el.items_end().child(user_bubble(content)) } else { el.child(content) })
            .when_some(note, |el, note| el.child(div().flex().items_center().gap_2().when(user, |el| el.justify_end())
                .child(div().min_w_0().text_sm().text_color(theme::warning()).child(note))
                .when_some(discard, |el, button| el.child(button))))
            .when_some(actions, |el, actions| el.child(actions));
        with_copy_menu(row, id, cx.weak_entity())
    }
}

// Copiar sai da vista e mora no menu de contexto (e no Ctrl+Shift+C para a última resposta). O texto é lido no
// clique, não copiado a cada quadro.
fn with_copy_menu(row: Stateful<Div>, id: String, view: WeakEntity<Hangar>) -> AnyElement {
    let copy_label = tr("copy_message");
    row.context_menu(move |menu, _, _| {
        let (view, id) = (view.clone(), id.clone());
        sidebar::menu_style(menu).item(PopupMenuItem::new(copy_label.clone()).icon(IconName::Copy)
            .on_click(move |_, _, cx| {
                let text = view.upgrade().and_then(|view| view.read(cx).copy_text(&id));
                if let Some(text) = text { cx.write_to_clipboard(ClipboardItem::new_string(text)); }
            }))
    }).into_any_element()
}

fn open_web_link(url: &SharedString, _: &ClickEvent, _: &mut Window, cx: &mut App) {
    if url.starts_with("https://") || url.starts_with("http://") { cx.open_url(url); }
}

fn citation_extensions(row: &str, owner: WeakEntity<Hangar>) -> gpui_kit::base::text::MarkdownExtensions {
    use gpui_kit::base::text::{markdown_ast, MarkdownExtensions, MarkdownNode, MarkdownParseContext, MarkdownPlugin};
    struct Citations { row: String, owner: WeakEntity<Hangar> }
    impl MarkdownPlugin for Citations {
        fn name(&self) -> &str { "file-citation" }
        fn parse(&self, node: &markdown_ast::Node, context: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
            let markdown_ast::Node::Link(link) = node else { return None; };
            let url = url::Url::parse(&link.url).ok().filter(|url| url.scheme() == "hangar-file")?;
            let path = url.query_pairs().find(|(key, _)| key == "path")?.1.into_owned();
            let line = url.query_pairs().find(|(key, _)| key == "line").and_then(|(_, value)| value.parse::<u32>().ok()).filter(|n| *n > 0);
            let text = format!("{path}{}", line.map(|n| format!(":{n}")).unwrap_or_default());
            Some(MarkdownNode::new(self.name(), (path, line)).text(text).markdown(context.node_source(node).unwrap_or_default().to_owned()))
        }
        fn render(&self, node: &MarkdownNode, _: &mut Window, _: &mut App) -> impl IntoElement {
            let (path, line) = node.data::<(String, Option<u32>)>().unwrap().clone();
            let owner = self.owner.clone();
            let label = format!("{}{}", composer::basename(&path), line.map(|n| format!(":{n}")).unwrap_or_default());
            let title = tr("citation_open").replace("{path}", node.as_text());
            Button::new(format!("citation-{}-{}", self.row, node.source_range().map_or(0, |range| range.start)))
                .small().outline().label(label).tooltip(title.clone()).accessibility_label(title)
                .on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| this.open_file(path.clone(), line, window, cx));
                })
        }
    }
    MarkdownExtensions::default().plugin(Citations { row: row.to_owned(), owner })
}

/// Markdown da conversa. É o `TextView` do gpui-base porque o do componente não repassa os campos que só a
/// conversa liga (marcador em coluna, faixa de linguagem).
fn chat_text(view: &Entity<TextViewState>, cx: &App) -> gpui_kit::base::TextView {
    gpui_kit::base::TextView::new(view).selectable(true).scrollable(false).style(theme::conversation_markdown(cx))
        .code_block_actions(copy_code)
}

/// Grupo de hover da linha da mensagem: a faixa de hora e copiar acende com ele.
const ROW_GROUP: &str = "message-row";

/// Bolha do usuário, na conversa e no subagente.
fn user_bubble(content: impl IntoElement) -> Div {
    div().max_w(relative(0.78)).px(px(14.)).py(px(10.)).rounded(px(18.)).bg(theme::user_bubble()).child(content)
}

/// Mensagem longa do usuário (mais de 5 linhas ou 400 caracteres) recolhe, para um log colado não virar bloco sem fim.
fn long_message(source: &str) -> bool { source.lines().count() > 5 || source.chars().count() > 400 }

/// Texto recolhido em 5 linhas enquanto a mensagem longa está fechada.
fn collapse(text: gpui_kit::base::TextView, long: bool, open: bool) -> gpui_kit::base::TextView {
    if long && !open { text.max_lines(5) } else { text }
}

fn more_button(id: impl Into<ElementId>, open: bool) -> Button {
    Button::new(id).ghost().xsmall().label(tr(if open { "show_less" } else { "show_more" }))
        .icon(if open { IconName::ChevronUp } else { IconName::ChevronDown })
}

/// Copiar da faixa de hover: o comportamento do `Clipboard` do kit (✓ por 2 s), mas fora da ordem do Tab, porque a faixa só aparece com o mouse (pelo
/// teclado, copiar segue no menu de contexto e no Ctrl+Shift+C). O texto é lido no clique.
#[derive(IntoElement)]
struct CopyButton { id: ElementId, value: std::rc::Rc<dyn Fn(&App) -> String> }

impl RenderOnce for CopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let copied = window.use_keyed_state(self.id.clone(), cx, |_, _| false);
        let done = *copied.read(cx);
        let value = self.value;
        Button::new(self.id).ghost().xsmall().tab_stop(false).tooltip(tr("copy_message"))
            .icon(if done { IconName::Check } else { IconName::Copy })
            .when(!done, |el| el.on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(value(cx)));
                copied.update(cx, |copied, cx| { *copied = true; cx.notify(); });
                let copied = copied.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    _ = copied.update(cx, |copied, cx| { *copied = false; cx.notify(); });
                }).detach();
            }))
    }
}

/// Hora da mensagem: só "HH:MM" se for de hoje, com o dia antes se não for.
fn stamp(ts: Option<f64>) -> Option<String> {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    let at = Local.timestamp_opt(ts.filter(|ts| ts.is_finite() && *ts > 0.)? as i64, 0).single()?;
    let time = format!("{:02}:{:02}", at.hour(), at.minute());
    if at.date_naive() == Local::now().date_naive() { return Some(time); }
    let day = tr("message_date").replace("{d}", &format!("{:02}", at.day())).replace("{m}", &format!("{:02}", at.month()));
    Some(format!("{day} {time}"))
}

fn copy_code(block: &gpui_kit::base::text::CodeBlock, _: &mut Window, _: &mut App) -> gpui_kit::component::clipboard::Clipboard {
    gpui_kit::component::clipboard::Clipboard::new("copy").xsmall().value(block.code()).tooltip(activity::web("comum_copiar_codigo"))
}

/// Resposta chegando: cada pedaço esmaece por palavra em 250 ms, e a resposta assentada não anima.
fn stream_motion(live: bool) -> gpui_kit::base::TextViewMotion {
    let motion = gpui_kit::base::TextViewMotion::default();
    if !live { return motion; }
    motion.with_stream_fade(Duration::from_millis(250)).with_stream_fade_stagger(Duration::from_millis(20))
        .with_stream_fade_easing(gpui_kit::base::Easing::EaseOut)
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
        "assistant_msg" => {
            for path in composer::cited_paths(&body) {
                let name = composer::basename(&path).to_owned();
                let image = composer::image_format(&name).is_some();
                refs.push((Source::Cited(path), name, image));
            }
            for url in composer::image_urls(&body) { refs.push((Source::Remote(url.clone()), composer::url_name(&url).to_owned(), true)); }
        }
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
    fn render_sidebar(&self, selected_name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let a = crate::appearance::get();
        let floating = a.panels == crate::appearance::Panels::Floating;
        let fit_content = floating && a.sidebar_height == crate::appearance::SidebarHeight::Content;
        let host = self.server_label(cx);
        let layout = self.sidebar_layout(cx);
        let section = |label: String, count: Option<usize>| div().flex().items_center().justify_between().px(px(8.)).pt(px(12.)).pb(px(6.))
            .child(chrome::section_label(label))
            .when_some(count, |el, n| el.child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).child(n.to_string())));
        let mut children: Vec<AnyElement> = Vec::new();
        let rows = |list: &[&SessionInfo], children: &mut Vec<AnyElement>, window: &mut Window, cx: &mut Context<Self>| for session in list {
            let selected = selected_name == Some(session.name.as_str());
            children.push(self.render_session_row((*session).clone(), selected, &host, window, cx));
        };
        if !layout.waiting.is_empty() {
            children.push(section(tr("sidebar_awaiting"), Some(layout.waiting.len())).into_any_element());
            rows(&layout.waiting, &mut children, window, cx);
        }
        for group in &layout.groups {
            if layout.by_project {
                let awaiting = group.sessions.iter().filter(|s| s.state == "awaiting_input").count();
                children.push(self.render_group_header(group, awaiting, window, cx));
                if self.sidebar.is_collapsed(&group.key) { continue; }
            } else {
                children.push(section(group.label.clone(), None).into_any_element());
            }
            rows(&group.sessions, &mut children, window, cx);
        }
        let empty = self.sessions.iter().all(|s| self.sidebar.hidden().contains(&s.name));
        let list = div().id("session-list").min_h_0().overflow_y_scroll().px(px(8.)).flex().flex_col().gap(px(2.))
            .when(!fit_content, |el| el.flex_1())
            .when(empty && self.list_error.is_none(), |el| el.child(div().p_2().text_xs().text_color(theme::faint())
                .child(tr(if self.list_online { "empty_sessions" } else { "connecting" }))))
            .when(layout.filter_empty(), |el| el.child(div().p_2().text_xs().text_color(theme::faint()).child(tr("sidebar_filter_empty"))))
            .children(children);
        let filter = layout.show_filter().then(|| div().flex_shrink_0().px(px(8.)).pb(px(4.))
            .child(Input::new(&self.sidebar.filter).small().cleanable(true).prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))
                .aria_label(tr("sidebar_filter"))));
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
                .child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).child(layout.total.to_string()))
                .child(self.render_group_picker(cx)))
            .children(filter)
            .child(list)
            .when_some(self.list_error.clone(), |el, text| el.child(div().px_4().py_1().flex().items_center().gap_2().text_xs().text_color(theme::warning())
                .child(div().flex_1().min_w_0().child(text))
                .child(Button::new("reconnect").xsmall().ghost().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))))))
            // O CTA do rodapé da barra do web.
            .child(div().flex_shrink_0().px(px(8.)).pt(px(8.)).pb(px(8.)).child(self.new_session_button(false, cx)))
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

    /// Abas no topo (como o web): todas as sessões numa faixa, e o servidor, a conexão e a engrenagem que moravam
    /// no rodapé da barra lateral. ←/→ andam o foco entre as abas; Enter ou Espaço abrem a sessão.
    fn render_tabs(&mut self, selected_name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let floating = theme::is_floating();
        let host = self.server_label(cx);
        let weak = cx.entity().downgrade();
        let tabs = self.sessions.iter().filter(|s| !self.sidebar.hidden().contains(&s.name)).filter_map(|session| {
            let focus = self.tab_focus.get(&session.name)?.clone();
            let on = selected_name == Some(session.name.as_str());
            let state = if session.limited == Some(true) { "limited" } else { session.state.as_str() };
            // Nome, estado e perguntas por extenso: o ponto só diz o estado pela cor.
            let mut label = format!("{} · {}", session.name, tr(&format!("chip_{state}")));
            if session.pending_questions > 0 { label.push_str(&format!(" · ? {}", session.pending_questions)); }
            // Trabalhando é a marca animada da lista (parada com movimento reduzido); os outros estados são um ponto na cor dele.
            let mark = if session.state == "working" {
                chrome::WorkingMark::new(SharedString::from(format!("tab-mark-{}", session.name)), 14., theme::accent()).into_any_element()
            } else {
                div().size(px(8.)).mx(px(2.)).flex_shrink_0().rounded_full().bg(if state == "limited" { theme::limited() } else { theme::status(state) }).into_any_element()
            };
            let pick = session.clone();
            let open = session.clone();
            let menu_name = session.name.clone();
            Some(div().id(SharedString::from(format!("tab-{}", session.name))).track_focus(&focus).flex_shrink_0().max_w(px(200.)).h(px(32.)).px(px(8.))
                .flex().items_center().gap(px(6.)).rounded(px(6.)).border_1().cursor_pointer()
                .map(|el| if on { el.bg(theme::accent_dim()).border_color(theme::accent()).text_color(theme::text()).font_weight(FontWeight::SEMIBOLD) }
                    else { el.border_color(transparent_black()).text_color(theme::muted()).hover(|el| el.bg(theme::hover())) })
                .when(focus.is_focused(window), |el| el.focus_ring_style(window, cx))
                .role(Role::Tab).aria_selected(on).aria_label(label.clone())
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx))
                .child(mark)
                .child(chrome::provider_glyph(&session.provider, 14.))
                .child(div().min_w_0().truncate().text_size(px(13.)).child(session.name.clone()))
                .when(session.pending_questions > 0, |el| el.child(div().flex_shrink_0().text_xs().text_color(theme::warning())
                    .child(format!("? {}", session.pending_questions))))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select(pick.clone(), window, cx);
                    if !this.connection_dialog && pick.readable() { this.composer.update(cx, |input, cx| input.focus(window, cx)); }
                }))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.select(open.clone(), window, cx);
                        cx.stop_propagation();
                    }
                }))
                // Clique direito na aba abre o mesmo menu da linha da barra, como o SessionTabs do web.
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.start_menu(menu_name.clone(), cx)))
                .context_menu(sidebar::session_menu(weak.clone(), session.clone())))
        }).collect::<Vec<_>>();
        // A folga lateral deixa o anel de foco da primeira e da última aba fora do recorte da rolagem.
        let strip = div().id("tabs-strip").flex_1().min_w_0().h_full().px(px(3.)).flex().items_center().gap(px(2.)).overflow_x_scroll().track_scroll(&self.tabs_scroll)
            .role(Role::TabList).aria_label(tr("sessions"))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let step = match event.keystroke.key.as_str() { "left" => -1, "right" => 1, _ => return };
                let names: Vec<&String> = this.sessions.iter().filter(|s| !this.sidebar.hidden().contains(&s.name)).map(|s| &s.name).collect();
                let Some(current) = names.iter().position(|name| this.tab_focus.get(*name).is_some_and(|f| f.is_focused(window))) else { return };
                let next = (current as isize + step).rem_euclid(names.len() as isize) as usize;
                if let Some(focus) = this.tab_focus.get(names[next]) { focus.focus(window, cx); }
                this.tabs_scroll.scroll_to_item(next);
                cx.stop_propagation();
                cx.notify();
            }))
            .children(tabs)
            .when(self.sessions.is_empty() && self.list_error.is_none(), |el| el.child(div().px_2().text_xs().text_color(theme::faint())
                .child(tr(if self.list_online { "empty_sessions" } else { "connecting" }))));
        div().h(px(44.)).w_full().flex_shrink_0().px(px(8.)).flex().items_center().gap(px(6.))
            .map(|el| if floating { el.rounded(px(18.)).border_1().border_color(theme::border()).bg(theme::chrome()).shadow(theme::panel_shadow()) }
                else { el.bg(theme::chrome()).border_b_1().border_color(theme::border()) })
            .child(div().px(px(6.)).child(chrome::hangar_mark(16., theme::accent())))
            .child(strip)
            .child(self.new_session_button(true, cx))
            .when_some(self.list_error.clone(), |el, text| el.child(div().flex_shrink_0().max_w(px(260.)).flex().items_center().gap_1()
                .child(div().min_w_0().truncate().text_xs().text_color(theme::warning()).child(text))
                .child(Button::new("reconnect").xsmall().ghost().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))))))
            .child(Button::new("connection").ghost().flex_shrink_0().max_w(px(220.)).h(px(32.)).px(px(8.))
                .tooltip(tr("connection_tip")).accessibility_label(tr("connection"))
                .child(div().min_w_0().flex().items_center().gap_2()
                    .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(if self.list_online { theme::success() } else { theme::warning() }))
                    .child(div().min_w_0().truncate().text_size(px(13.)).text_color(theme::muted()).child(host)))
                .on_click(cx.listener(|this, _, window, cx| this.open_connection(window, cx))))
            .child(Button::new("open-settings").custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted())
                    .hover(theme::hover()).active(theme::hover()))
                .icon(chrome::small_icon(IconName::Settings, 16., theme::muted())).size(px(28.)).rounded(px(6.))
                .accessibility_label(tr("settings")).tooltip_with_action(tr("settings_open"), &OpenSettings, None)
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(settings::Page::Appearance, window, cx))))
            .into_any_element()
    }

    /// Linha de 3 níveis do mock: pasta @ servidor e tempo; selo, nome e estado; pergunta pendente ou branch com o diff.
    /// Mais, como o web: "? N" das perguntas, o ⋯ e o clique direito com o menu da sessão, pressionar 500 ms para renomear
    /// na própria linha e a prévia da última resposta ao parar o mouse. A linha entra no Tab (Enter abre) e o ⋯ vem depois dela.
    fn render_session_row(&self, session: SessionInfo, selected: bool, host: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = session.state.as_str();
        let limited = session.limited == Some(true);
        let untracked = session.tracked == Some(false);
        let chip_state = if limited { "limited" } else { state };
        let chip = if chip_state == "working" {
            chrome::working_chip(SharedString::from(format!("row-mark-{}", session.name)), tr("chip_working"))
        } else { chrome::state_chip(chip_state, tr(&format!("chip_{chip_state}")), false) };
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
        let questions = session.pending_questions;
        let editing = self.sidebar.editing.as_ref().filter(|e| e.old == name).map(|e| e.input.clone());
        let focus = self.tab_focus.get(&name).cloned();
        let focused = focus.as_ref().is_some_and(|f| f.contains_focused(window, cx));
        let hovered = self.sidebar.hover.as_deref() == Some(name.as_str());
        let weak = cx.entity().downgrade();
        let menu_open = self.sidebar.button_menu.as_deref() == Some(name.as_str());
        let show_menu = selected || hovered || focused || menu_open;
        // O ⋯ mora no lugar do tempo: com ele à vista, o tempo sai (senão sobra um pedaço do "12m" atrás dele).
        let when = when.filter(|_| !show_menu);
        let menu_button = show_menu.then(|| {
            let (weak, target) = (weak.clone(), session.name.clone());
            div().absolute().top(px(5.)).right(px(6.)).rounded(px(6.)).bg(if selected { theme::selected_row() } else { theme::hover() })
                .child(Button::new(SharedString::from(format!("row-menu-{name}"))).ghost().xsmall().icon(IconName::Ellipsis)
                    .accessibility_label(tr("sidebar_options").replace("{n}", &name)).tooltip(tr("sidebar_options_tip"))
                    .dropdown_menu_with_anchor(Anchor::TopRight, sidebar::session_menu(weak.clone(), session.clone()))
                    .on_open_change(move |open, _, cx| { let _ = weak.update(cx, |this, cx| this.button_menu(target.clone(), *open, cx)); }))
        });
        let lane = || div().w(px(18.)).flex_shrink_0();
        let name_el = match editing {
            // O campo não deixa o clique nele virar clique na linha.
            Some(input) => div().flex_1().min_w_0().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_action(cx.listener(|this, _: &Escape, window, cx| this.cancel_session_rename(window, cx)))
                .child(Input::new(&input).xsmall().aria_label(tr("sidebar_new_name"))).into_any_element(),
            None => div().flex_1().min_w_0().flex().items_center().gap_2()
                .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(name.clone()))
                .when(questions > 0, |el| el.child(div().flex_shrink_0().text_xs().text_color(theme::warning()).child(format!("? {questions}"))))
                .when(untracked, |el| el.child(badge(tr("untracked_badge"), theme::faint()))).into_any_element(),
        };
        let (hover_name, press_name, menu_name, key_open, menu_session) = (name.clone(), name.clone(), name.clone(), session.clone(), session.clone());
        div().id(SharedString::from(session.name.clone())).relative().flex_shrink_0().flex().flex_col().gap(px(1.)).px(px(8.)).py(px(7.)).rounded(px(10.))
            .when_some(focus.as_ref(), |el, focus| el.track_focus(focus))
            .when(focus.as_ref().is_some_and(|f| f.is_focused(window)), |el| el.focus_ring_style(window, cx))
            .when(selected, |el| el.bg(theme::selected_row()))
            .when(!selected, |el| el.hover(|el| el.bg(theme::hover())))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.row_hover(hover_name.clone(), *hovered, cx)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| this.row_pointer(f32::from(event.position.y))))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| this.row_press(press_name.clone(), window, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.row_release()))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.start_menu(menu_name.clone(), cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                // Só a própria linha: Enter no ⋯ ou no campo do nome é deles.
                if !matches!(event.keystroke.key.as_str(), "enter" | "space") || !this.tab_focus.get(&key_open.name).is_some_and(|f| f.is_focused(window)) { return; }
                this.select(key_open.clone(), window, cx);
                cx.stop_propagation();
            }))
            .child(div().flex().items_center().gap(px(8.)).text_size(px(11.5)).text_color(theme::faint())
                .child(lane())
                .child(div().flex_1().min_w_0().truncate().child(meta))
                .when_some(when, |el, w| el.child(div().flex_shrink_0().child(w))))
            .child(div().flex().items_center().gap(px(8.)).when(untracked, |el| el.opacity(0.45))
                .child(lane().child(chrome::provider_glyph(&session.provider, 16.)))
                .child(name_el)
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
            .children(menu_button)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.hide_preview();
                if this.take_long_press() { return; }
                this.select(session.clone(), window, cx);
                // Foco só no gesto sobre a lista; troca automática de transcript não tira o foco de ninguém.
                if !this.connection_dialog && session.readable() {
                    this.composer.update(cx, |input, cx| input.focus(window, cx));
                }
            }))
            .context_menu(sidebar::session_menu(weak, menu_session))
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

/// Onde o diálogo de salvar abre: a pasta de downloads, ou a casa do usuário.
fn downloads_folder() -> PathBuf {
    std::env::var_os("XDG_DOWNLOAD_DIR").map(PathBuf::from).filter(|p| p.is_dir())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads")).filter(|p| p.is_dir()))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_else(std::env::temp_dir)
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
        "user_msg" => {
            let body = event.body();
            if let Some(cards::Card::Codex(card)) = message_card(event) { return card.report; }
            composer::parse_marked(&body).map(|m| m.caption).unwrap_or(body)
        }
        _ => event.body(),
    }
}

/// Mensagem de usuário sem imagem que vira cartão, como no web: o bastão também na fila (é por ela que ele chega); o
/// subagente do Codex só gravado.
fn message_card(event: &ChatEvent) -> Option<cards::Card> {
    if event.kind != "user_msg" || event.image_count.unwrap_or(0) > 0 { return None; }
    let text = event.text.as_deref()?;
    if let Some(baton) = cards::baton(text) { return Some(cards::Card::Baton(baton)); }
    if event.queued() || event.id.starts_with("held:") { return None; }
    cards::codex_subagent(text).map(cards::Card::Codex)
}

/// Hora local "HH:MM" de um instante do transcript.
fn clock(ts: Option<f64>) -> Option<String> {
    use chrono::{Local, TimeZone, Timelike};
    let at = Local.timestamp_opt(ts.filter(|ts| ts.is_finite() && *ts > 0.)? as i64, 0).single()?;
    Some(format!("{:02}:{:02}", at.hour(), at.minute()))
}

fn render_source(event: &ChatEvent) -> String { safe_markdown(&display_body(event)) }

/// Caracteres da prévia neste quadro. O resto fracionário passa ao próximo: em `t` segundos entram `floor(pace·t)`,
/// seja qual for a taxa da tela.
fn preview_step(carry: f64, pace: f64, elapsed: f64, remaining: usize) -> (usize, f64) {
    let carry = carry + pace * elapsed;
    let count = (carry.floor() as usize).min(remaining);
    (count, carry - count as f64)
}

fn count_lines(result: &ChatEvent) -> usize {
    result.result.as_deref().unwrap_or("").trim().lines().count()
}

fn prepare_detail(full: String) -> Prepared {
    let total = full.chars().count();
    let (shown, clipped) = conversation::clip(&full, DETAIL_MAX);
    let fenced = conversation::fenced(shown);
    Prepared::Detail { fenced, total, clipped, full: full.into() }
}

fn prepare_message(event: &ChatEvent) -> Prepared {
    let card = message_card(event);
    let body = display_body(event);
    let source = if event.kind == "assistant_msg" { composer::citation_markdown(&body) } else { body.clone() };
    Prepared::Message { markdown: safe_markdown(&source), blank: body.trim().is_empty(), card }
}

// Identidade do conteúdo de uma linha que não é mensagem: muda quando chega resultado ou o grupo cresce.
fn signature(item: &Item, events: &[ChatEvent]) -> String {
    // O raciocínio dentro do grupo da Árvore conta pelo tamanho do texto, como no bloco de pensamento.
    let tool = |t: &Tool| format!("{}>{}:{}", events[t.call].id, t.result.map(|i| events[i].id.as_str()).unwrap_or(""),
        events[t.call].text.as_deref().filter(|_| events[t.call].kind == "thinking").map_or(0, str::len));
    match item {
        Item::Event(_) => String::new(),
        Item::Orphan(i) => events[*i].result.as_deref().map(str::len).unwrap_or(0).to_string(),
        Item::Tool(t) => tool(t),
        Item::Group { tools, .. } => tools.iter().map(tool).collect::<Vec<_>>().join(","),
        Item::Thinking { parts, .. } => parts.iter().map(|&i| format!("{}:{}", events[i].id, events[i].text.as_deref().map(str::len).unwrap_or(0))).collect::<Vec<_>>().join(","),
        Item::Tasks { tasks, .. } => format!("{tasks:?}"),
    }
}

/// O verbo do spinner do terminal ("Sketching… (6s · esc to interrupt)" → "Sketching…"); rótulo sem ele, "Trabalhando…".
/// Os segundos são contados por nós, então os do terminal saem junto com o resto dos parênteses.
fn working_verb(label: Option<&str>) -> String {
    label.and_then(|label| label.split(" (").next()).map(str::trim).filter(|verb| verb.ends_with('…'))
        .map(str::to_owned).unwrap_or_else(|| tr("working_line"))
}

fn preview_source(preview: &Preview) -> String {
    if preview.md { return safe_markdown(&composer::citation_markdown(&crate::mend::close_hanging(&preview.text))); }
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
        let mut chars = line.char_indices().peekable();
        while let Some((at, ch)) = chars.next() {
            if ch == '`' {
                let mut run = 1;
                while chars.peek().is_some_and(|&(_, next)| next == '`') { chars.next(); run += 1; }
                inline = match inline { Some(open) if open == run => None, None => Some(run), other => other };
                for _ in 0..run { output.push('`'); }
            } else if inline.is_none() && ch == '<' && chars.peek().is_some_and(|&(_, next)| next.is_ascii_alphabetic() || matches!(next, '/' | '!')) {
                output.push_str("&lt;");
            } else if inline.is_none() && ch == '!' && chars.peek().is_some_and(|&(_, next)| next == '[') {
                match image_markdown(&line[at + 1..]) {
                    Some((text, used)) => {
                        output.push_str(&text);
                        while chars.peek().is_some_and(|&(next, _)| next <= at + used) { chars.next(); }
                    }
                    None => output.push_str("\\!"),
                }
            } else { output.push(ch); }
        }
    }
    output
}

/// `[alt](alvo)` depois do `!`: a miniatura sai como anexo da bolha, e no texto fica o link (remoto) ou só o rótulo
/// (local, como no web). Devolve o texto e quantos bytes consumiu; `None` = não é imagem em markdown.
fn image_markdown(rest: &str) -> Option<(String, usize)> {
    let close = rest.find("](")?;
    let alt = &rest[1..close];
    let len = rest[close + 2..].find(')')?;
    let target = &rest[close + 2..close + 2 + len];
    if alt.contains(['[', ']', '\n']) || target.is_empty() || target.contains(|c: char| c.is_whitespace() || c == '<' || c == '>') { return None; }
    let alt = alt.trim().replace('<', "&lt;");
    let remote = target.starts_with("http://") || target.starts_with("https://");
    let text = match (remote, alt.is_empty()) {
        (true, true) => format!("[{target}]({target})"),
        (true, false) => format!("[{alt}]({target})"),
        (false, true) => composer::basename(target).replace('<', "&lt;"),
        (false, false) => alt,
    };
    Some((text, close + 3 + len))
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

impl Hangar {
    /// Barra lateral ou abas no topo, conforme Aparência.
    fn render_nav(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let selected_name = self.selected.as_ref().map(|s| s.name.clone());
        if appearance::get().navigation == appearance::Navigation::Tabs { self.render_tabs(selected_name.as_deref(), window, cx) }
            else { self.render_sidebar(selected_name.as_deref(), window, cx) }
    }

    /// Entre o cabeçalho e a faixa de baixo: o cartão de antes da conversa, a lista ou o aviso de vazio.
    fn render_conversation_area(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // A miniatura só conta como vista quando a conversa é desenhada: guardada entre quadros, ela segue na tela.
        self.media.next_frame();
        let mut content = div().size_full().flex().flex_col();
        let prethread = self.render_prethread(cx);
        if let Some(selected) = &self.selected {
            if let Some(card) = prethread {
                content = content.child(card);
            } else if !selected.readable() {
                content = content.child(div().flex_1().p_6().text_color(theme::muted()).child(tr(if selected.tracked == Some(false) { "untracked" } else { "starting" })));
            } else {
                if self.has_older || self.loading {
                    content = content.child(in_column(div().py_2().flex().gap_2().items_center()
                        .when(self.has_older, |el| el.child(Button::new("older").small().outline().label(tr("older")).disabled(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| { this.history_limit = this.history_limit.saturating_add(400); this.etag = None; this.load_history(cx); }))))
                        .when(self.has_older, |el| el.child(div().text_xs().text_color(theme::muted()).child(format!("{} {}", tr("history_window"), self.history_limit))))
                        .when(self.loading, |el| el.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))))));
                }
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
                        .child(self.wheel_layer(cx))
                        .when(self.follow_detached(), |el| el.child(self.render_jump_pill(cx))));
                    self.schedule_scroll(window, cx);
                }
            }
        } else { content = content.child(div().flex_1().flex().items_center().justify_center().text_color(theme::muted()).child(tr("choose_session"))); }
        content.into_any_element()
    }

    /// Cartões, faixas e avisos entre a conversa e o compositor, e o compositor.
    fn render_bottom_area(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let selected_key = self.selected_key();
        let sending = selected_key.as_ref().is_some_and(|key| self.delivery.pending(key));
        let stopping = selected_key.as_ref().is_some_and(|key| self.stopping.contains(key));
        let delivery_note = selected_key.as_ref().and_then(|key| {
            // "Enviando…" é da linha de trabalhando, sob a conversa.
            if self.delivery.pending(key) { return None; }
            self.delivery.outcome(key).map(|outcome| match outcome {
                SendOutcome::Delivered => (tr("delivered"), false),
                SendOutcome::Queued => (tr("queued"), false),
                SendOutcome::Uncertain => (tr("delivery_uncertain"), true),
                SendOutcome::Rejected(reason) => (reason.clone(), true),
            })
        });
        let stop_note = selected_key.as_ref().and_then(|key| self.stop_feedback.get(key)).cloned();
        let prethread_open = self.prethread_key().is_some();
        let mut content = div().w_full().flex().flex_col();
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
        self.measured_bottom(content.into_any_element())
    }
}

impl Render for Hangar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_name = self.selected.as_ref().map(|s| s.name.clone());
        let floating = theme::is_floating();
        // Página de Configurações ocupa a janela; a caixa ao vivo deixa a janela da conversa por baixo.
        let page = self.settings.filter(|_| !self.settings_ui.live);
        let tabs = appearance::get().navigation == appearance::Navigation::Tabs;

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
        let content = div().relative().flex_1().min_w_0().h_full().flex().flex_col()
            .child(div().h(px(44.)).pl(px(20.)).pr(px(12.)).flex_shrink_0().flex().items_center().gap(px(10.)).when(floating, |el| el.mx(px(4.)))
                .when_some(self.selected.as_ref(), |el, s| el.child(chrome::provider_glyph(&s.provider, 18.)))
                .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).child(selected_name.clone().unwrap_or_else(|| tr("title"))))
                .when_some(place, |el, place| el.child(div().min_w_0().truncate().text_color(theme::faint()).child(place)))
                .child(div().flex_1())
                .child(if session_chip { chrome::state_chip(&chip_state, tr(&format!("chip_{chip_state}")), true) }
                    else { div().flex_shrink_0().text_xs().text_color(theme::status(&header_state)).child(tr(&header_state)).into_any_element() })
                .when_some(self.render_activity_button(window, cx), |el, button| el.child(button))
                .when(self.selected.is_some(), |el| el.child(chrome::icon_button("side-show", IconName::PanelRight,
                        tr(if self.side.open { "side_hide" } else { "side_show" }), cx)
                    .selected(self.side.open).on_click(cx.listener(|this, _, _, cx| this.toggle_side(cx))))))
            // Cada área é uma view própria, guardada entre quadros quando pode (`panes.rs`).
            .child(self.pane_element(panes::Area::Conversation, StyleRefinement::default().w_full().flex_1().min_h_0(), cx))
            // Entre a conversa e a faixa de baixo: o que a faixa abre por cima (comandos, sugestões) cobre a marca.
            .when(page.is_none(), |el| el.child(self.working_mark_float(panes::Area::Conversation, WORKING_FADE, cx.reduce_motion())))
            .child(self.pane_element(panes::Area::Bottom, StyleRefinement::default().w_full().flex_shrink_0().h(px(self.panes.bottom_height.get())), cx))
            .children(self.render_file_view(cx));
        let nav = if page.is_some() { None }
            else if tabs { Some(self.pane_element(panes::Area::Nav, StyleRefinement::default().w_full().h(px(44.)).flex_shrink_0(), cx)) }
            else { Some(self.pane_element(panes::Area::Nav, StyleRefinement::default().w(px(284.)).h_full().flex_shrink_0(), cx)) };
        self.sync_side_cost(window);
        // A marca da aba Atividade anima fora das duas views guardadas (painel e aba), depois delas na árvore.
        let side = self.side_width(window).map(|width| div().h_full().flex_shrink_0()
            .child(self.pane_element(panes::Area::Side, StyleRefinement::default().w(px(width)).h_full().flex_shrink_0(), cx))
            // Só com a aba à vista: fora dela a view não redesenha e não limpa os próprios lugares.
            .when(self.activity_tab(), |el| el.child(self.activity_mark_float(cx))).into_any_element());
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

        let live = self.settings_live().then(|| self.render_live(window, cx));
        div().id("hangar-root").track_focus(&self.root_focus).relative().size_full().flex().bg(theme::window_fill()).text_color(theme::text()).text_base()
            .children(self.render_backdrop(window))
            .font_family(theme::SANS)
            .when(floating && page.is_none(), |el| el.p(px(10.)).gap(px(10.)))
            .on_action(cx.listener(|this, _: &FocusComposer, window, cx| {
                let page_open = this.settings.is_some() && !this.settings_live();
                if !this.connection_dialog && !page_open && this.selected.as_ref().is_some_and(|s| s.readable()) {
                    this.composer.update(cx, |input, cx| input.focus(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                if !this.connection_dialog { this.open_settings(settings::Page::Appearance, window, cx); }
            }))
            .on_action(cx.listener(|this, _: &FocusSettingsSearch, window, cx| this.focus_search(window, cx)))
            .on_action(cx.listener(|this, _: &NextSession, window, cx| this.step_session(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousSession, window, cx| this.step_session(-1, window, cx)))
            .on_action(cx.listener(|this, _: &CopyLastReply, _, cx| {
                let page_open = this.settings.is_some() && !this.settings_live();
                if let Some(text) = this.last_reply().filter(|_| !page_open) { cx.write_to_clipboard(ClipboardItem::new_string(text)); }
            }))
            // Esc fora do campo fecha o painel aberto sobre o compositor (o clique no botão tira o foco do campo).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Com a confirmação aberta, o Esc é dela: fecha só o diálogo.
                if event.keystroke.key != "escape" || this.connection_dialog || this.search_focused(window, cx) || window.has_active_dialog(cx) { return; }
                if this.shortcuts_escape(window, cx) { cx.stop_propagation(); return; }
                if this.files_escape(window, cx) { cx.stop_propagation(); return; }
                if this.settings.is_some() {
                    this.close_settings(window, cx);
                    cx.stop_propagation();
                    return;
                }
                // O painel preso a um botão fecha e o foco volta ao campo, o próximo alvo de quem digitava.
                if this.close_popups() {
                    this.composer.update(cx, |input, cx| input.focus(window, cx));
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
            // Arrasto da caixa ao vivo pelo cabeçalho: mesmo esquema, gravando a posição ao soltar.
            .when(self.live_dragging(), |el| el.cursor_move()
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                    this.drag_live(event.position, event.pressed_button == Some(MouseButton::Left), window, cx);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, window, cx| this.drag_live(event.position, false, window, cx))))
            .map(|el| match (page, nav) {
                (Some(page), _) => el.child(self.render_settings(page, cx)),
                // Abas no topo: a faixa em cima, a conversa e o painel embaixo, sem barra lateral.
                (None, Some(bar)) if tabs => el.flex_col().child(bar)
                    .child(div().flex_1().min_h_0().flex().when(floating, |el| el.gap(px(10.))).child(content).when_some(side, |el, side| el.child(side))),
                (None, sidebar) => el.children(sidebar).child(content).when_some(side, |el, side| el.child(side)),
            })
            .children(live)
            .children(self.render_preview(window))
            .children(self.render_popup(window, cx))
            .when(self.connection_dialog, |el| el.child(div().absolute().inset_0().bg(theme::scrim()).flex().items_center().justify_center()
                .child(dialog.focus_trap("connection-dialog", &self.connection_focus))))
            // Diálogos e avisos numa view própria: a animação deles redesenha só ela, não as áreas guardadas.
            .child(self.panes.overlay.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{message_card, preview_step, safe_markdown, stream_motion, working_verb};
    use crate::{api::dto::ChatEvent, cards::Card, i18n::tr};
    use std::time::Duration;

    #[test]
    fn working_line_takes_the_terminal_verb_and_counts_its_own_seconds() {
        assert_eq!(working_verb(Some("Sketching… (6s · esc to interrupt)")), "Sketching…");
        assert_eq!(working_verb(Some("Writing tests…")), "Writing tests…");
        // Rótulo sem verbo (outro harness) ou ausente: a palavra nossa.
        assert_eq!(working_verb(Some("Running")), tr("working_line"));
        assert_eq!(working_verb(None), tr("working_line"));
        let at = |s| super::chrome::format_elapsed(Duration::from_secs(s));
        assert_eq!((at(6), at(65), at(3725)), ("6s".into(), "1m 5s".into(), "1h 2m".into()));
    }

    #[test]
    fn long_user_message_collapses_past_five_lines_or_400_chars_and_time_skips_unknown() {
        assert!(!super::long_message("1\n2\n3\n4\n5"));
        assert!(super::long_message("1\n2\n3\n4\n5\n6"));
        assert!(!super::long_message(&"a".repeat(400)));
        assert!(super::long_message(&"á".repeat(401)));
        let body = format!("{} /home/x/app.rs:12 e `app.rs:12`.", "a".repeat(345));
        let event = |kind: &str| ChatEvent { kind: kind.into(), text: Some(body.clone()), ..Default::default() };
        let super::Prepared::Message { markdown, .. } = super::prepare_message(&event("user_msg")) else { panic!() };
        assert_eq!(markdown, body);
        assert!(!super::long_message(&markdown));
        let super::Prepared::Message { markdown, .. } = super::prepare_message(&event("assistant_msg")) else { panic!() };
        assert_eq!(markdown.matches("hangar-file:").count(), 2);
        assert_eq!(super::stamp(None), None);
        let now = chrono::Local::now().timestamp() as f64;
        assert_eq!(super::stamp(Some(now)), super::clock(Some(now)));
        assert!(super::stamp(Some(now - 3. * 86_400.)).is_some_and(|s| s.len() > 5));
    }

    #[test]
    fn markdown_image_becomes_label_or_remote_link() {
        assert_eq!(safe_markdown("veja ![gráfico](/tmp/g.png) e ![](out/b.gif)\n"), "veja gráfico e b.gif\n");
        assert_eq!(safe_markdown("![logo](https://h.io/l.png). ![](http://h.io/a.png)"), "[logo](https://h.io/l.png). [http://h.io/a.png](http://h.io/a.png)");
        // Sem forma de imagem, alvo com `<` ou dentro de código: o `!` continua escapado ou intocado.
        assert_eq!(safe_markdown("![só texto] e ![x](https://h/<b>)"), "\\![só texto] e \\![x](https://h/&lt;b>)");
        assert_eq!(safe_markdown("`![a](/t/a.png)`"), "`![a](/t/a.png)`");
    }

    #[test]
    fn only_the_live_reply_fades_word_by_word() {
        let live = stream_motion(true);
        assert_eq!((live.stream_fade(), live.stream_fade_stagger()), (Duration::from_millis(250), Duration::from_millis(20)));
        // A resposta assentada volta ao padrão sem esmaecer, desligando o que o quadro anterior ligou.
        let settled = stream_motion(false);
        assert_eq!((settled.stream_fade(), settled.stream_fade_stagger()), (Duration::ZERO, Duration::ZERO));
    }

    #[test]
    fn baton_card_also_in_the_queue_codex_card_only_recorded() {
        let baton = "[hangar: passagem de bastão] Você continua o trabalho da sessão `origem-x` — é a mesma.\n\
            Comece lendo o resumo do trabalho em `/srv/r.md`.\nLeia o plano antes.\nA sessão `origem-x` continua VIVA.\n\
            A continuação NÃO move esses vínculos.";
        let codex = "<subagent_notification>\n{\"status\": {\"completed\": \"ok\"}}\n</subagent_notification>";
        let event = |id: &str, text: &str, images: u32| ChatEvent { kind: "user_msg".into(), id: id.into(), text: Some(text.into()),
            image_count: Some(images), ..ChatEvent::default() };
        for id in ["b1", "queued-b1", "held:b1"] { assert!(matches!(message_card(&event(id, baton, 0)), Some(Card::Baton(_))), "{id}"); }
        assert!(matches!(message_card(&event("k1", codex, 0)), Some(Card::Codex(_))));
        for id in ["queued-k1", "held:k1"] { assert_eq!(message_card(&event(id, codex, 0)), None, "{id}"); }
        // Com imagem, a imagem vence, como no web.
        assert_eq!(message_card(&event("b2", baton, 1)), None);
        assert_eq!(message_card(&ChatEvent { kind: "assistant_msg".into(), ..event("b3", baton, 0) }), None);
    }

    #[test]
    fn preview_pace_does_not_depend_on_the_refresh_rate() {
        for hz in [60., 144.] {
            let (mut shown, mut carry) = (0, 0.);
            for _ in 0..hz as usize {
                let count;
                (count, carry) = preview_step(carry, 160., 1. / hz, usize::MAX);
                shown += count;
            }
            // A soma de 1/hz em ponto flutuante pode ficar um fio abaixo de 160.
            assert!((159..=160).contains(&shown), "{hz} Hz: {shown} caracteres em 1 s");
        }
    }
}
