use std::{collections::{HashMap, HashSet}, path::PathBuf, sync::Arc, time::{Duration, Instant}};
use gpui_kit::{component::{button::*, checkbox::Checkbox, radio::Radio, tab::{Tab, TabBar}, scroll::{Scrollbar, ScrollbarMode}, menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
    input::{Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Textarea, TextareaState}, text::{TextView, TextViewState}, *}, *};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::assets::IconName;
use tokio::{runtime::Runtime, task::JoinHandle};
use crate::{api::{self, Api, Failure, Source, dto::*, sse::Update}, cards, chat::{Chat, LiveTool}, composer,
    conversation::{self, Item, Tool}, delivery::{DeliveryTracker, SendOutcome, SessionKey}, i18n::{tr, tr_shared}, theme,
    interaction::{self, Action, Ask, InFlight, Pick}, media::{self, MediaCache, MediaState}, appearance, motion};
use gpui_kit::component::notification::Notification;
use serde_json::{Value, json};

mod activity;
mod backdrop;
mod baton;
mod browser;
mod accounts;
pub(crate) mod chrome;
mod computer;
mod controls;
mod plan_review;
mod create;
mod device;
mod follow;
mod row_patch;
mod landing;
mod edits;
mod terminal_look;
mod git;
mod grouping;
mod group_sheet;
mod hangar_live;
mod harness;
mod viewer;
mod window_tray;
mod disk;
mod player;
mod machines;
mod migration;
mod orchestration;
mod orq_roles;
mod orq_panel;
mod orq_history;
mod home_usage;
mod recent;
mod orq_timeline;
mod panes;
mod page_card;
mod popup;
mod rail;
mod rows;
mod run;
mod share;
mod files;
mod settings;
mod mention;
mod server_config;
mod invite;
mod pair_accept;
mod external;
mod servers;
pub(crate) use servers::{ServerEntry, new_id as new_server_id};
mod shortcuts;
mod shortcut_transfer;
mod keyboard;
mod session_numbers;
mod session_picker;
mod side;
mod terminal;
mod sidebar;
mod subagent;
mod dictation;
mod sync;
mod connect;
mod guests;
mod shared_config;
mod tree;
mod find;
mod costs;
mod worktrees;
mod stats;
mod search;
mod topbar;
mod voice_ui;
pub(crate) mod setup;
/// Variável do ambiente do script com o código de uso único do askpass (`setup::askpass`).
pub(crate) const ASKPASS_CODE_ENV: &str = "HANGAR_ASKPASS_CODE";

actions!(hangar, [FocusComposer, OpenSettings, CopyLastReply, FocusSettingsSearch, FindProjectFile, FindProjectText, NextSession, PreviousSession, ToggleDictation, NewChat, OpenNewSession, CloseSession, RenameSession, OpenCosts, OpenSearch,
    ToggleSidebar, CyclePermission, OpenWorktrees]);

const LIVE_THINKING: &str = "__thinking__";
const LIVE_TOOL: &str = "__tool__";
const PREVIEW: &str = "__preview__";
const WORKING: &str = "__working__";
/// Entrada da linha "trabalhando"; a marca, desenhada fora da conversa, entra no mesmo tempo.
const WORKING_FADE: Duration = motion::WORKING.duration();
/// Quanto "Enviando…" espera o turno começar depois da entrega; passou disso, a sessão não vai trabalhar.
const SENT_BRIDGE: Duration = Duration::from_secs(5);
/// Prefixo da linha do cartão fixo de um agente rodando, seguido do id do tool_use.
const PINNED: &str = "pin:";
const COLUMN: f32 = 780.;
/// Abrir a sessão pede só a cauda que enche a tela: o backend lê o transcript de trás para a frente, e a janela
/// inteira (`HISTORY_PAGE`) custa muito mais em transcript grande. Ela vem logo depois, por baixo.
const FIRST_PAGE: usize = 60;
const HISTORY_PAGE: usize = 400;

/// Conexão, máquina e nome da sessão aberta (`session_owner`).
pub(crate) type SessionOwner = (u64, String, String);

/// Largura da tela sem sessão: a do mock vezes o ajuste de Aparência.
fn column_width() -> f32 { COLUMN * crate::appearance::get().column as f32 / 100. }

/// Tela sem sessão: faixas na largura de `column_width`.
fn landing_column(el: impl IntoElement) -> Div {
    div().w_full().flex_shrink_0().px(px(36.)).flex().justify_center().child(div().w_full().max_w(px(column_width())).child(el))
}

thread_local! {
    /// Largura da janela e painel direito aberto no quadro atual: os degraus da coluna do web são media queries da janela.
    static COLUMN_FRAME: std::cell::Cell<(f32, bool)> = const { std::cell::Cell::new((1280., false)) };
}

/// Caixa da coluna da conversa, como o `max-width` da `.messages-inner` e do `.composer-card` do web (MessageList.svelte,
/// Chat.svelte, Composer.svelte): teto por degrau da janela, maior com o painel direito aberto, e a escala da Aparência
/// sobre o menor entre o teto e o espaço, sem passar do espaço.
fn column_box(composer: bool) -> Div {
    let (window, side) = COLUMN_FRAME.get();
    let a = crate::appearance::get();
    let scale = a.column as f32 / 100.;
    let step = |wide: f32, mid: f32, base: f32| if window >= 1900. { wide } else if window >= 1600. { mid } else { base };
    let (ceiling, scale) = if side { (step(1440., 1320., 1200.) * scale, scale) }
        // Sem o painel, o compositor não segue a escala: é o `.composer-dock` do web.
        else if composer { (1400f32.min(window * 0.94), 1.) }
        else {
            let read = if a.palette == crate::appearance::Palette::Neutral { 920. } else { step(1200., 1080., 920.) };
            ((read * scale).min(window * step(0.76, 0.82, 0.94)), scale)
        };
    div().w(relative(scale.min(1.))).max_w(px(ceiling))
}

/// Recuo do texto dentro da coluna (`padding-inline` da `.messages-inner`); a margem de fora é o `padding` da lista.
fn column_padding() -> f32 { if COLUMN_FRAME.get().0 >= 1280. { 32. } else { 24. } }

/// Põe uma faixa na coluna da conversa: mesma margem e mesma largura das mensagens.
fn in_column(el: impl IntoElement) -> Div {
    div().w_full().flex_shrink_0().px(px(16.)).flex().justify_center().child(column_box(false).px(px(column_padding())).child(el))
}

/// Faixa de problema da sessão: a frase do web pelo código, como no web; código sem frase mostra o detalhe cru.
fn problem_banner(state: &SessionState) -> Option<String> {
    let code = state.problema.as_deref().filter(|code| !code.trim().is_empty());
    match code.and_then(|code| crate::i18n::tr_web(&format!("problema_{code}"), &HashMap::new())) {
        Some(text) => Some(match state.problema_detalhe.as_deref().and_then(|d| d.lines().next()).filter(|d| !d.trim().is_empty()) {
            Some(detail) => format!("{text} — {detail}"),
            None => text,
        }),
        None => state.problema_detalhe.clone().or_else(|| code.map(str::to_owned)),
    }
}

/// Texto da conversa com a fonte, o tamanho e a entrelinha escolhidos em Aparência. Em 100% são os do web no desktop:
/// resposta 17 px/1,7 (`.prose`, AssistantBubble.svelte) e bolha do usuário 16 px/1,55 (`.bubble-text`, UserBubble.svelte).
fn conversation_text(el: Div, user: bool) -> Div {
    let a = crate::appearance::get();
    let (size, line) = if user { (16., 1.55) } else { (17., 1.7) };
    el.font_family(a.font.family())
        .text_size(px(size * a.text_size as f32 / 100.)).line_height(relative(line * a.line_height as f32 / 100.))
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
    Files(SessionKey, Option<SessionOwner>, u64, Vec<Result<Picked, String>>),
    // `None` marca o início do envio daquele anexo.
    UploadStep(SessionKey, u64, Option<Result<Uploaded, Failure>>),
    UploadsDone(SessionKey, String, bool, HashSet<String>, Option<Vec<String>>),
    Commands(String, Result<Vec<CommandInfo>, Failure>),
    Recent(SessionKey, Result<Vec<UploadFile>, Failure>),
    // Miniatura já decodificada fora da thread da janela; `None` = bytes que não são imagem legível.
    Media(SessionKey, Source, Result<Option<Arc<RenderImage>>, Failure>),
    Saved(SessionKey, bool, Result<PathBuf, String>),
    ConnectionNotSaved(String),
    // Rede local aprendida para a máquina deste endereço salvo.
    Lan(String, api::route::Lan),
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
    Config(u64, Result<Value, Failure>),
    PushSettings(u64, Result<Value, Failure>),
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
    Connect(connect::ConnectReply),
    // Configuração compartilhada: fala com várias máquinas, cada uma pelo token dela.
    SharedConfig(shared_config::SharedConfigReply),
    // Máquinas: identificador, alcance e reinício do servidor conectado.
    Machines(machines::MachinesReply),
    Computer(computer::ComputerReply),
    // Diálogo Nova sessão: a resposta vai ao diálogo que a pediu, se ele ainda for o aberto.
    Create(EntityId, create::CreateReply),
    Transfer(EntityId, create::TransferReply),
    // Clique num botão de mod: o que o mod copiou ou mandou abrir vem na resposta.
    PluginPressed(Result<Value, Failure>),
    // Troca de aba de mod: só a falha interessa; a aba nova chega pelo `shown_id`.
    PluginShown(Result<Value, Failure>),
    PluginClosed(Result<Value, Failure>),
    // Digitação num campo de mod: o lugar, a `key` e a identidade do campo que mandou; a volta libera o próximo pedido
    // da fila dele, e só a falha aparece.
    PluginInput(String, crate::plugin_ui::Control, EntityId, Result<Value, Failure>),
    // Lista de outra máquina: a geração dos SSE de lista, a chave do servidor e o que chegou.
    Remote(u64, String, servers::RemoteUpdate),
    HeadlessPlan(SessionKey, controls::PlanOutcome),
    // Barra lateral: prévia, leitura do silenciar e as gravações do menu da sessão.
    Sidebar(sidebar::SidebarReply),
    Terminal(terminal::Reply),
    // O caminho do áudio que este pedido guardou nos anexos, também quando a transcrição falhou.
    Dictation(u64, Option<String>, Result<Value, Failure>),
    // Aba Atividade: a conta de subagentes no disco e a lista da aba.
    Activity(activity::ActivityReply),
    FileView(files::FileReply),
    Mentions(u64, Result<Vec<String>, Failure>),
    // Resumo do bastão (`GET …/bastao/dossie`), amarrado ao estado da tela que o pediu e ao número do pedido.
    Dossier(EntityId, u64, Result<String, Failure>),
    // Evento da chamada de voz com o número dela: o da chamada parada é descartado.
    Voice(u64, crate::voice::VoiceEvent),
    // Opção beta do servidor local, o Codex achado e a voz gravada neste computador.
    VoiceGate(Option<bool>, Option<crate::voice::rpc::Codex>, voice_ui::SavedVoice, Option<Vec<voice_ui::CodexAccount>>),
    // Catálogo de modelos do organizador para a conta escolhida, com o número do pedido.
    VoiceModels(u64, Result<Vec<voice_ui::OrganizerModel>, String>),
    // Histórico da sessão que recebeu pedido da voz e terminou fora da tela.
    VoiceHistory(u64, SessionKey, Result<api::History, Failure>),
    // Resultado de uma ferramenta de sessão da voz (criar, agrupar), com a chamada que espera a resposta.
    VoiceDone(u64, crate::voice::CallId, voice_ui::VoiceDone),
    // Decisão do Jev sobre uma fala, com as opções que foram perguntadas.
    VoiceJev(u64, voice_ui::JevAsked, Result<crate::voice::jev::Decision, String>),
}

#[derive(Clone, Debug, PartialEq)]
enum Reply {
    Catalog(controls::Ctl),
    // Valor aplicado e o que a fonte ao vivo mostrava no gesto.
    Applied(controls::Ctl, String, Option<String>),
    Cost(u64),
    /// Leitura do painel da orquestração, com o número do pedido.
    OrqPanel(u64),
    GitFiles,
    Diff(String),
    /// Rótulo do atalho e se o pedido foi No Hangar.
    Shell(String, bool),
    RunCode(String),
    Reload,
    PlanPreview(bool),
    PlanReview(u64),
    PreSelect(String),
    RunState,
    /// Leitura e gravação dos atalhos do projeto, com o número do pedido.
    ProjectShortcuts(u64),
    ProjectSaved(u64),
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

#[derive(Default)]
struct SystemNotifications {
    seq: u64,
    prefs: Option<PushPreferences>,
    finished: Option<(bool, u64)>,
    state: Option<String>,
    started: Option<Instant>,
}

struct PushPreferences { muted: Vec<String>, quiet: Option<(chrono::NaiveTime, chrono::NaiveTime)> }

impl PushPreferences {
    fn parse(value: Value) -> Option<Self> {
        let muted = value.get("muted")?.as_array()?.iter().map(|v| v.as_str().map(str::to_owned)).collect::<Option<_>>()?;
        let quiet = match value.get("quiet_hours")? {
            Value::Null => None,
            q => {
                let time = |key| {
                    let text = q.get(key)?.as_str()?;
                    chrono::NaiveTime::parse_from_str(text, "%H:%M:%S%.f")
                        .or_else(|_| chrono::NaiveTime::parse_from_str(text, "%H:%M")).ok()
                };
                Some((time("start")?, time("end")?))
            }
        };
        Some(Self { muted, quiet })
    }

    fn suppressed(&self, name: &str, now: chrono::NaiveTime) -> bool {
        self.muted.iter().any(|muted| muted == name) || self.quiet.is_some_and(|(start, end)| {
            if start <= end { start <= now && now < end } else { now >= start || now < end }
        })
    }
}

impl SystemNotifications {
    fn reset_stream(&mut self) { self.state = None; self.started = None; }

    fn advance(&mut self, state: &str, now: Instant) -> Option<&'static str> {
        let previous = self.state.replace(state.to_owned());
        if previous.as_deref() == Some(state) { return None; }
        // O primeiro retrato não prova quando o turno começou, nem uma transição ao vivo.
        previous?;
        match state {
            "working" => { self.started = Some(now); None }
            "awaiting_input" => Some("notify_awaiting"),
            "idle" => {
                let started = self.started.take();
                let (enabled, minimum) = self.finished?;
                (enabled && started.is_some_and(|start| now.duration_since(start).as_secs() >= minimum)).then_some("notify_finished")
            }
            _ => { self.started = None; None }
        }
    }
}

/// Conferência dos arquivos citados de uma sessão: mensagens já lidas, caminhos já perguntados e os que não abrem.
#[derive(Default)]
struct CiteCheck { owner: Option<SessionKey>, scanned: HashSet<String>, checked: HashSet<String>, dead: HashSet<String> }

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

/// Tipo das notificações que espelham aviso de mod; a chave de cada uma é o id do aviso.
struct PluginToast;

/// Tipo fixo do aviso de falha na digitação ou na troca de aba de um mod: um aviso novo substitui o anterior, e uma
/// sequência de teclas que falham não empilha um aviso por tecla.
struct PluginFailure;

// Formulário da pergunta atual; refeito quando a pergunta (identidade + conteúdo) muda.
#[derive(Default)]
struct AskForm { fingerprint: String, picks: Vec<Pick>, typing: Vec<bool>, inputs: Vec<Entity<InputState>>, _changes: Vec<Subscription>, tab: usize }

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
    /// Pasta real (canonicalize) de cada `cwd` de sessão nesta máquina, resolvida fora da thread da tela: pasta de rede
    /// travada não congela a janela. Ausente ou `None` = segue pelo backend.
    local_dirs: HashMap<String, Option<std::path::PathBuf>>,
    selected: Option<SessionInfo>,
    /// Máquina da sessão aberta quando não é a ativa: abrir uma sessão não troca o servidor das configurações.
    open_api: Option<Api>,
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
    _keyboard_subscription: Subscription,
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
    /// Terminais de atalho vivos da máquina ativa (evento `shortcut_terminals`); as outras máquinas guardam no `RemoteList`.
    live_terms: Vec<terminal::LiveTerm>,
    /// Cartão da pergunta aberto: (máquina, dono — vazio = No Hangar, terminal) e o diálogo que o mostra.
    question_open: Option<(String, String, String)>,
    question_card: Option<Entity<hangar_live::QuestionCard>>,
    /// Popover do chip "N no Hangar" aberto, e a falha da última ação dele.
    hangar_open: bool,
    /// Primeira ação da lista do chip: recebe o foco quando ela abre.
    hangar_focus: FocusHandle,
    hangar_error: Option<String>,
    /// Relógio do "rodando · N min": redesenha a cada 30 s enquanto há um No Hangar vivo.
    live_clock: Option<Task<()>>,
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
    plan_review: plan_review::ReviewState,
    list_state: ListState,
    /// Risca do marcador de mensagens sob o mouse: abre o cartão com a pergunta e o começo da resposta.
    rail_hover: Option<usize>,
    follow: follow::Follow,
    row_ids: Vec<String>,
    row_signatures: Vec<String>,
    conversation: conversation::incremental::Incremental,
    row_assets: Vec<row_patch::RowAssets>,
    expanded: HashSet<String>,
    // O `expanded` das conversas que saíram da tela: o que foi aberto ou fechado volta com a conversa.
    kept_expanded: HashMap<SessionKey, HashSet<String>>,
    // Sessão orq: ids dos eventos que abrem um dia novo, para o separador sair antes deles.
    orq_days: HashSet<String>,
    // Coluna que o gráfico de cada tabela mostra, pela chave "<linha>#t<n>".
    table_column: HashMap<String, usize>,
    // Tabelas que dão gráfico em cada resposta, com a fonte de onde saíram: refeitas só quando a fonte muda.
    tables: HashMap<String, (String, std::rc::Rc<[crate::tables::Table]>)>,
    last_message: Option<usize>,
    live_clear_epoch: [u64; 2],
    rich: HashMap<String, RichText>,
    // Texto das linhas já preparado, pela chave da linha ou da parte: esvaziado quando o chat muda, e o desenho
    // só prepara o que falta. Assim contar linhas, formatar JSON e limpar o markdown não roda a cada quadro.
    prepared: HashMap<String, Prepared>,
    /// Arquivos citados na conversa conferidos no servidor: os que não abrem ficam sem chip.
    cites: CiteCheck,
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
    mention: mention::Mention,
    command_panel: bool,
    /// Cartão aberto pelo anel de contexto do compositor.
    context_card: bool,
    command_search: Entity<InputState>,
    confirm: Option<Confirm>,
    confirm_no_ask: bool,
    terminal_suggestion: String,
    /// Faixa acima do prompt que os mods do Claude Code desenham, como veio do SSE `plugin_ui`.
    plugin_band: Value,
    /// Painéis que os mods abriram e o terminal desenhou, do mesmo SSE.
    plugin_panes: Vec<Value>,
    /// Painel na frente segundo o servidor (`shown_id`); `None` quando o servidor não manda.
    plugin_shown: Option<Option<String>>,
    /// Largura, em colunas, para a qual a faixa foi desenhada; `None` num servidor antigo.
    plugin_columns: Option<f64>,
    /// De onde vem a interface dos mods; `None` vale como terminal (sem digitação).
    plugin_source: Option<crate::plugin_ui::UiSource>,
    /// O que o plugin da sessão anuncia que atende (`btw`); vazio sem plugin ou num servidor antigo.
    plugin_caps: Vec<String>,
    /// Escolha local da aba: começa no último painel aberto e sobrevive aos redesenhos.
    plugin_local_tab: Option<String>,
    /// Rolagem da fileira de abas dos mods e a aba ativa para a qual ela já rolou.
    plugin_tabs_scroll: ScrollHandle,
    plugin_tabs_seen: Option<String>,
    plugin_tabs_waits: u8,
    /// Trechos dos mods (escopo de hover ou cartão absoluto) com o ponteiro em cima: lugar e caminho na árvore.
    plugin_hovered: HashSet<String>,
    /// Campos (`Input`) dos mods, por `plugin_ui::field_id`: nascem quando a árvore os traz e saem com ela.
    plugin_fields: HashMap<String, crate::plugin_ui::Field>,
    /// Quantos eventos `plugin_ui` chegaram: separa o desenho novo do mod do redesenho do app, para os campos.
    plugin_draws: u64,
    /// Últimos ids de aviso de mod (SSE `plugin_toast`) já mostrados; só os recentes voltam na reconexão.
    plugin_toasts_seen: std::collections::VecDeque<String>,
    /// Avisos de mod na tela, do mais antigo ao mais novo.
    plugin_toasts_shown: std::collections::VecDeque<SharedString>,
    recent: Option<Recent>,
    media: MediaCache<(SessionKey, Source)>,
    /// Páginas publicadas na conversa, pelo id da página.
    pages: page_card::Pages,
    full_images: viewer::FullImages,
    stats: Option<Stats>,
    side: side::Side,
    controls: controls::Controls,
    // Página de configurações aberta por cima da janela inteira; `None` é a janela da conversa.
    settings: Option<settings::Page>,
    settings_ui: settings::SettingsUi,
    // Abas: foco de cada aba pelo nome da sessão (setas andam entre elas) e a rolagem da faixa,
    // que traz a aba ativa para a vista quando a seleção muda.
    tab_focus: HashMap<String, FocusHandle>,
    tabs_scroll: ScrollHandle,
    appearance_note: Option<String>,
    // Por que o tema Desktop não está pintando com o papel de parede; `None` quando pinta ou não foi escolhido.
    desktop_note: Option<String>,
    // Pedidos numerados: resposta de pedido anterior ao último é descartada.
    palette_seq: u64,
    backdrop_seq: u64,
    backdrop_pending: bool,
    // Imagem do fundo na tela (arquivo escolhido ou papel de parede) e a assinatura dos bytes dela.
    backdrop: Option<(u64, Arc<RenderImage>)>,
    // Por que o fundo não desenha a imagem escolhida.
    backdrop_note: Option<String>,
    backdrop_busy: Option<backdrop::BackdropBusy>,
    grain: Arc<RenderImage>,
    device: device::Device,
    accounts: accounts::Accounts,
    orchestration: orchestration::Orchestration,
    orq_history: Option<orq_history::History>,
    orq_history_serial: u64,
    home_usage: home_usage::HomeUsage,
    // Conversas fechadas do modo Conversas; `reopen` é a aberta na área principal, que só vira sessão no Enviar.
    recents: recent::Recents,
    reopen: Option<recent::ArchiveEntry>,
    shortcuts: shortcuts::Shortcuts,
    keyboard: keyboard::Keyboard,
    session_picker: session_picker::SessionPicker,
    harness: harness::Harnesses,
    server_config: server_config::ServerConfig,
    sync: sync::Sync,
    connect: connect::Connect,
    shared: shared_config::SharedConfig,
    machines: machines::Machines,
    // Custos e Estatísticas de uso: página própria por cima da janela, fora das Configurações.
    costs: costs::Costs,
    worktrees: worktrees::Worktrees,
    usage_stats: stats::UsageStats,
    // Paleta "Buscar conversas" (Ctrl+K).
    search: search::Search,
    topbar: topbar::TopBar,
    window_tray: window_tray::WindowTray,
    system_notifications: SystemNotifications,
    computer: computer::Computer,
    new_session: Option<Entity<create::NewSession>>,
    sidebar: sidebar::Sidebar,
    terminal: Option<terminal::Panel>,
    terminal_serial: u64,
    // Busca do seletor de modelo quando a lista é longa.
    ctl_search: Entity<InputState>,
    act: activity::ActivityState,
    panes: panes::Panes,
    files: files::Files,
    tree: tree::Tree,
    find: find::Find,
    dossier: Option<Entity<baton::Dossier>>,
    /// Quando vimos o turno começar ao vivo; a sessão aberta já trabalhando conta do último envio.
    turn_seen: Option<Instant>,
    /// Envio entregue que o turno ainda não pegou: "Enviando…" segue até o estado virar trabalhando ou o prazo passar.
    sent_until: Option<(SessionKey, Instant)>,
    new_chat: Option<Entity<create::NewSession>>,
    /// A linha "Nova conversa" do topo da barra lateral, alcançável pelo Tab.
    new_chat_focus: FocusHandle,
    /// O menu aberto da tela sem sessão (máquina, pasta, modelo, conta, branch): a tela escreve; a camada da raiz lê.
    new_chat_folders: std::rc::Rc<std::cell::Cell<Option<create::Menu>>>,
    /// A chegada da primeira mensagem da tela sem sessão (`landing.rs`).
    landing: Option<landing::Landing>,
    /// A mensagem mandada da tela sem sessão, mostrada como enviada até o transcript trazer a real (`landing.rs`).
    opening: Option<landing::Opening>,
    /// O painel direito do último quadro (conversa e largura), para ver quando ele abre ou fecha.
    side_seen: Option<(Option<SessionKey>, Option<f32>)>,
    /// O painel entrando ou saindo: quando começou, se está abrindo e a largura dele.
    side_slide: Option<(Instant, bool, f32)>,
    /// Linhas que chegaram com a conversa aberta, e quando: entram com o `fade-in` do kit, uma vez.
    arrived: HashMap<String, Instant>,
    /// Partes dos grupos da Árvore já vistas, e as que chegaram com o grupo na tela (quando começam a entrar).
    tree_parts: HashSet<String>,
    part_arrived: HashMap<String, Instant>,
    /// Grupo da Árvore: aberto no último quadro e quando isso mudou, para dobrar em vez de pular.
    tree_folds: HashMap<String, (bool, Option<Instant>)>,
    /// Alguma parte da Árvore animando no desenho da linha atual: a linha pede o próximo quadro.
    tree_motion: bool,
    active_token: String,
    ready_sessions: Option<Vec<SessionInfo>>,
    /// Todas as máquinas conhecidas, a ativa inclusive, e a lista ao vivo de cada outra.
    servers: Vec<servers::ServerEntry>,
    remote: HashMap<String, servers::RemoteList>,
    remote_tasks: Vec<JoinHandle<()>>,
    remote_gen: u64,
    /// Sobe quando a lista de máquinas muda: a tela sem sessão refaz o seletor dela.
    servers_rev: u64,
    /// Servidores de convite cujo compartilhamento acabou (chave `servers::norm`).
    invite_ended: HashSet<String>,
    /// Sessão aberta antes da troca do servidor ativo para a máquina dela: reabre quando a lista nova chegar.
    pending_open: Option<String>,
    /// Sessão de outra máquina clicada antes da lista dela chegar (chave `servers::norm`, nome).
    pending_remote: Option<(String, String)>,
    /// Pares externos lidos de cada servidor próprio (chave dele, par).
    external_pairs: Vec<(String, api::ExternalPairDto)>,
    /// Sessões com par externo na última lista vista: a releitura dos pares só sai quando isto muda.
    external_seen: Option<Vec<(String, String, Option<PairExternal>)>>,
    /// Attach já feitos ou em voo (endereço, token da entrada, token do par).
    attached: HashSet<(String, String, String)>,
    external_seq: u64,
    dictation: dictation::Dictation,
    voice: voice_ui::VoiceUi,
    player: player::Player,
    connection_origin: Option<WeakFocusHandle>,
    /// Primeira abertura com o app Electron neste computador: a tela de conexão oferece trazer as configurações dele.
    electron_offer: bool,
    /// O assistente de instalação aberto: ocupa a janela inteira (`setup/`).
    setup: Option<Entity<setup::SetupWizard>>,
    /// O assistente fechado com o script ainda rodando: fica vivo (canal da senha, atualização suspensa), sem desenhar nem
    /// pegar teclado. Reabrir pelo menu o reaproveita.
    setup_hidden: Option<Entity<setup::SetupWizard>>,
    /// O cartão da entrada sem conexão salva: procurando ou o que achou neste computador.
    entry: Option<setup::Entry>,
}

impl Drop for Hangar {
    fn drop(&mut self) {
        for task in [&self.list_task, &self.session_task, &self.history_task].into_iter().flatten().chain(&self.remote_tasks) { task.abort(); }
    }
}

/// Bloqueante: o daemon de notificação pode demorar. Chamar fora da thread da janela.
fn show_system_notification(title: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification.appname("Hangar").icon("com.hangar.native").summary(title).body(body);
    // O servidor de notificação acha o ícone e o app pela entrada .desktop; Windows e macOS não têm a dica.
    #[cfg(all(unix, not(target_os = "macos")))]
    notification.hint(notify_rust::Hint::DesktopEntry("com.hangar.native".into()));
    if let Err(error) = notification.show() {
        eprintln!("notification: {error}");
    }
}

impl Hangar {
    pub fn new(runtime: Arc<Runtime>, appearance_error: Option<String>, crash: Option<String>, links: async_channel::Receiver<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        if let Some(error) = crash {
            cx.defer_in(window, move |_, window, cx| {
                let desc = tr("crash_desc").replace("{error}", &error);
                chrome::confirm_alert(window, cx, tr("crash_title"), desc, tr("crash_copy"), ButtonVariant::Primary,
                    move |_, cx| { cx.write_to_clipboard(ClipboardItem::new_string(error.clone())); true });
            });
        }
        Self::watch_system(window, cx);
        Self::watch_dictation(window, cx);
        let (tray_tx, tray_rx) = async_channel::unbounded::<crate::tray::TrayEvent>();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = tray_rx.recv().await {
                if this.update_in(cx, |this, window, cx| this.on_tray_event(event, window, cx)).is_err() { break; }
            }
        }).detach();
        cx.defer_in(window, |this, _, cx| this.sync_tray(cx));
        let tray_owner = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(this) = tray_owner.upgrade() else { return true };
            if !this.read(cx).closes_to_tray() { return true; }
            this.update(cx, |this, cx| this.hide_to_tray(window, cx));
            false
        });
        // Link `hangar://` desta ou de outra execução: abre o diálogo preenchido e traz a janela para a frente. Cada link entra
        // por um update novo, nunca de dentro de outro update do Hangar (reentrar dá pânico no GPUI).
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(link) = links.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    this.show_from_tray(window, cx);
                    if !link.is_empty() { this.open_invite_dialog(Some(link), window, cx); }
                });
                if alive.is_err() { break; }
            }
        }).detach();
        // Prazo do cache de prompt no compositor: mostra minutos, então 20 s bastam; só a faixa de baixo redesenha.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(20)).await;
            let alive = this.update(cx, |this, cx| {
                if this.selected.is_some() && crate::chat::last_cache(&this.chat.events).is_some() { this.redraw(panes::Area::Bottom, cx); }
            });
            if alive.is_err() { break; }
        }).detach();
        // O aviso de servidor desatualizado mora na barra desta view e vem do estado do atualizador.
        if let Some(updater) = cx.try_global::<crate::update::Handle>().map(|handle| handle.0.clone()) {
            cx.observe(&updater, |_, _, cx| cx.notify()).detach();
        }
        let saved = load_connection();
        // Entrada (spec "Entrada"): sem conexão salva, procura um Hangar neste computador antes do cartão; o assistente que
        // estava rodando reabre onde parou.
        let resume = setup::saved_run();
        let probe = saved.is_none() && resume.is_none() && !setup::demo() && setup::supported();
        match resume {
            Some(state) => cx.defer_in(window, move |this: &mut Self, window, cx| {
                setup::opening_at_launch();
                this.open_setup(setup::Origin::Resume(state), window, cx)
            }),
            None if setup::demo() => cx.defer_in(window, |this: &mut Self, window, cx| this.open_setup(setup::Origin::Menu, window, cx)),
            // Conserto do agente sem desfazer: o assistente abre e a recuperação dele devolve a pasta, com aviso.
            None if setup::interrupted_fix() => cx.defer_in(window, |this: &mut Self, window, cx| {
                setup::opening_at_launch();
                this.open_setup(setup::Origin::Menu, window, cx)
            }),
            None if probe => cx.defer_in(window, |this: &mut Self, window, cx| this.start_entry(window, cx)),
            None => {}
        }
        // Primeira abertura com a lista: as máquinas do app Electron entram sozinhas, como a conexão dele já entrava.
        let (mut known_servers, adopt) = match load_servers() { Some(list) => (list, false), None => (Vec::new(), true) };
        if adopt { cx.defer_in(window, |this: &mut Self, window, cx| this.adopt_electron_servers(window, cx)); }
        if let Some((address, token)) = &saved
            && !known_servers.iter().any(|s| servers::norm(&s.address) == servers::norm(address)) {
            known_servers.insert(0, servers::ServerEntry { id: servers::new_id(), label: servers::default_label(address),
                address: address.clone(), token: token.clone(), disabled: false, invite: false, lan: None, ephemeral: false });
        }
        let (saved_address, saved_token) = saved.clone().unwrap_or_else(|| ("http://127.0.0.1:8765".into(), String::new()));
        let address = cx.new(|cx| InputState::new(window, cx).default_value(saved_address).placeholder(tr("server")));
        let token = cx.new(|cx| InputState::new(window, cx).masked(true).default_value(saved_token).placeholder(tr("token")));
        if saved.is_some() { cx.defer_in(window, |this: &mut Self, window, cx| this.connect(window, cx)); }
        // A imagem escolhida abre sem depender de conexão; o papel de parede em Vidro vem ao conectar.
        cx.defer_in(window, |this: &mut Self, window, cx| this.refresh_backdrop(window, cx));
        let connection_focus = cx.focus_handle();
        // O cursor do campo só para de piscar ao perder o foco: foco num campo que nunca aparece o deixa piscando para sempre.
        // Com conexão salva o diálogo não abre; o `connect` que falhar põe o foco aqui.
        if saved.is_none() && !probe { address.update(cx, |input, cx| input.focus(window, cx)); }
        let composer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 10).submit_on_enter(true));
        let input_subscription = cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
            match event {
                InputEvent::PressEnter { secondary: false, shift: false } if !this.connection_dialog => {
                    if !this.accept_mention(window, cx) { this.submit(false, false, window, cx); }
                }
                // A lista de comandos acompanha o que se digita.
                // Texto e sugestões só aparecem na faixa de baixo.
                InputEvent::Change => { this.suggest_pick = 0; this.redraw(panes::Area::Bottom, cx); }
                _ => {}
            }
        });
        cx.observe(&composer, |this, _, cx| this.refresh_mention(cx)).detach();
        cx.bind_keys([KeyBinding::new("tab", NoAction, Some("Terminal")),
            KeyBinding::new("shift-tab", NoAction, Some("Terminal")),
            KeyBinding::new("ctrl-c", NoAction, Some("Terminal")),
            // Tab dentro da página navega os campos dela, não o foco do app.
            KeyBinding::new("tab", NoAction, Some("BrowserPage")),
            KeyBinding::new("shift-tab", NoAction, Some("BrowserPage")),
            // Num campo de texto o Ctrl+W apaga a palavra, como no readline; fechar sessão fica fora dele.
            KeyBinding::new("ctrl-w", gpui_kit::base::input::DeleteToPreviousWordStart, Some("Input"))]);
        let settings_ui = settings::SettingsUi::new(window, cx);
        let root_focus = cx.focus_handle();
        cx.on_focus_lost(window, |this: &mut Self, window, cx| this.machines_focus_lost(window, cx)).detach();
        let command_search = cx.new(|cx| InputState::new(window, cx).placeholder(tr("commands_search")));
        // A busca mora no painel de comandos, sobre o compositor.
        cx.subscribe(&command_search, |this, _, _: &InputEvent, cx| this.redraw(panes::Area::Bottom, cx)).detach();
        let (tx, rx) = async_channel::bounded::<Envelope>(256);
        let learned = tx.clone();
        // ponytail: fila cheia perde o aviso; a rede local é reaprendida na próxima decisão.
        api::route::on_learned(move |address, lan| {
            let _ = learned.try_send(Envelope { connection: 0, selection: None, payload: Payload::Lan(address, lan) });
        });
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
        #[cfg(not(target_os = "macos"))]
        {
            let (requests, received) = async_channel::unbounded::<crate::browser::server::Request>();
            match crate::browser::server::start(&runtime, requests) {
                Ok(_) => cx.spawn_in(window, async move |this, cx| {
                    while let Ok(request) = received.recv().await {
                        if this.update_in(cx, |this, _, cx| this.dispatch_preview(request, cx)).is_err() { break; }
                    }
                }).detach(),
                Err(e) => eprintln!("[nav] servidor do hangar-preview nao subiu: {e}"),
            }
        }
        // O fim que encolhe vira folga embaixo: o histórico à vista não sobe e desce a cada linha que entra e sai no fim.
        let list_state = ListState::new(0, ListAlignment::Bottom, px(300.)).hold_tail(px(160.));
        Self::watch_user_scroll(&list_state, cx);
        let sidebar = sidebar::Sidebar::new(window, cx);
        let panes = panes::Panes::new(cx);
        let owner = window.window_handle();
        let weak = cx.weak_entity();
        let keyboard_subscription = cx.intercept_keystrokes(move |stroke, window, cx| {
            if window.window_handle().window_id() != owner.window_id() { return; }
            let Some(event) = window.current_key_down_event().cloned() else { return; };
            if event.keystroke != stroke.keystroke { return; }
            let _ = weak.update(cx, |this, cx| {
                // Com o assistente aberto, os atalhos da conversa (Ctrl+número, Esc da sessão) não valem.
                if this.setup.is_some() { return; }
                let root_key = this.new_session.clone().is_some_and(|dialog| dialog.update(cx, |dialog, cx| dialog.root_key_down(&event, window, cx)));
                if root_key || (event.keystroke.key == "escape" && (this.close_plan_review(window, cx) || this.keyboard_escape(window, cx)))
                    || this.keyboard_key_down(&event, window, cx) || this.session_number_key(&event, window, cx) {
                    cx.stop_propagation();
                }
            });
        });
        Self {
            runtime, tx, api: None, server: None, connection: 0, selection: 0, revision: 0, sessions: Vec::new(), local_dirs: HashMap::new(), selected: None, open_api: None,
            chat: Chat::default(), list_task: None, session_task: None, history_task: None,
            address, token, unsaved_connection: None, connection_focus, root_focus, composer, composer_placeholder: String::new(), _input_subscription: input_subscription, _keyboard_subscription: keyboard_subscription,
            connection_dialog: true, list_online: false, chat_online: false, loading: false, history_started: false,
            history_installed: false, pending_chat: Vec::new(),
            history_limit: 400, has_older: false, etag: None, error: None, list_error: None,
            delivery: DeliveryTracker::default(), stopping: HashSet::new(), stop_feedback: HashMap::new(), drafts: HashMap::new(),
            flight: InFlight::default(), action_feedback: HashMap::new(), live_terms: Vec::new(), question_open: None, question_card: None,
            hangar_open: false, hangar_focus: cx.focus_handle(), hangar_error: None, live_clock: None, ask_form: AskForm::default(), plans_dismissed: HashSet::new(), answered_tools: HashSet::new(), answering: HashMap::new(), ask_scroll: Default::default(), plan_scroll: Default::default(), plan_view: None,
            plan_review: Default::default(),
            list_state, rail_hover: None, follow: Default::default(), row_ids: Vec::new(), row_signatures: Vec::new(), conversation: Default::default(), row_assets: Vec::new(), expanded: HashSet::new(), kept_expanded: HashMap::new(), orq_days: HashSet::new(),
            table_column: HashMap::new(), tables: HashMap::new(), last_message: None, live_clear_epoch: [0; 2], rich: HashMap::new(), prepared: HashMap::new(), cites: CiteCheck::default(), render_tick: 0,
            preview_drop_epoch: 0, preview_drop_scheduled: false,
            visible_preview: Preview::default(), preview_tick_epoch: 0, preview_tick_scheduled: false,
            preview_last_tick: None, preview_carry: 0., preview_deadline: None,
            attachments: HashMap::new(), attach_seq: 0, uploading: HashMap::new(), commands: HashMap::new(),
            suggest_pick: 0, suggest_dismissed: None, command_panel: false, context_card: false, command_search, confirm: None, confirm_no_ask: false,
            mention: Default::default(),
            terminal_suggestion: String::new(), plugin_band: Value::Null, plugin_panes: Vec::new(), plugin_shown: None, plugin_columns: None, plugin_source: None, plugin_caps: Vec::new(), plugin_local_tab: None, plugin_tabs_scroll: ScrollHandle::new(), plugin_tabs_seen: None, plugin_tabs_waits: 0, plugin_hovered: HashSet::new(), plugin_fields: HashMap::new(), plugin_draws: 0, plugin_toasts_seen: Default::default(), plugin_toasts_shown: Default::default(), recent: None, media: MediaCache::new(), pages: page_card::Pages::new(window.window_handle()), full_images: viewer::full_images(), stats: None,
            side: side::Side::default(), controls: controls::Controls::default(),
            settings: None, settings_ui, tab_focus: HashMap::new(), tabs_scroll: ScrollHandle::new(),
            appearance_note: appearance_error.map(|error| tr("settings_not_loaded").replace("{error}", &error)),
            desktop_note: None,
            palette_seq: 0, backdrop_seq: 0, backdrop_pending: false, backdrop: None, backdrop_note: None, backdrop_busy: None, grain: crate::media::grain(),
            device: device::Device::new(window, cx), accounts: accounts::Accounts::default(), orchestration: orchestration::Orchestration::default(), orq_history: None, orq_history_serial: 0, home_usage: Default::default(), recents: Default::default(), reopen: None, shortcuts: shortcuts::Shortcuts::default(),
            server_config: server_config::ServerConfig::default(), harness: harness::Harnesses::default(), sync: sync::Sync::default(), connect: connect::Connect::default(), shared: shared_config::SharedConfig::default(), machines: machines::Machines::default(),
            costs: Default::default(), worktrees: Default::default(), usage_stats: Default::default(), search: Default::default(), topbar: Default::default(), computer: computer::Computer::default(), new_session: None, sidebar,
            terminal: None, terminal_serial: 0,
            system_notifications: SystemNotifications::default(),
            window_tray: window_tray::WindowTray::new(tray_tx),
            act: activity::ActivityState::new(cx), files: files::Files::new(window, cx), keyboard: keyboard::Keyboard::new(window, cx), session_picker: Default::default(),
            tree: tree::Tree::new(window, cx), find: find::Find::new(window, cx), ctl_search: controls::search_field(window, cx), panes, dossier: None, turn_seen: None, sent_until: None,
            new_chat: None, new_chat_focus: cx.focus_handle().tab_stop(true),
            new_chat_folders: Default::default(), landing: None, opening: None, side_seen: None, side_slide: None, arrived: HashMap::new(), tree_parts: HashSet::new(), part_arrived: HashMap::new(), tree_folds: HashMap::new(), tree_motion: false, active_token: String::new(), ready_sessions: None,
            servers: known_servers, remote: HashMap::new(), remote_tasks: Vec::new(), remote_gen: 0, servers_rev: 0, invite_ended: HashSet::new(), pending_open: None, pending_remote: None,
            external_pairs: Vec::new(), external_seen: None, attached: HashSet::new(), external_seq: 0,
            dictation: Default::default(),
            voice: voice_ui::VoiceUi { a11y_dump: Self::watch_a11y_dump(window, cx), ..Default::default() },
            player: Default::default(),
            connection_origin: None,
            electron_offer: saved.is_none() && crate::electron::exists(),
            setup: None,
            setup_hidden: None,
            entry: probe.then_some(setup::Entry::Probing),
        }
    }

    /// Automático acompanha a preferência do sistema; o Desktop relê a paleta quando a janela volta ao foco.
    fn watch_system(window: &mut Window, cx: &mut Context<Self>) {
        theme::set_system_dark(matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark));
        theme::sync_kit(Some(window), cx);
        popup::install_kit_surface(cx);
        cx.observe_window_appearance(window, |this, window, cx| {
            theme::set_system_dark(matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark));
            theme::sync_kit(Some(window), cx);
            this.sync_sliders(window, cx);
            cx.notify();
        }).detach();
        chrome::set_window_active(window.is_window_active());
        chrome::set_software_gpu(window.gpu_specs().is_some_and(|gpu| gpu.is_software_emulated));
        cx.observe_window_activation(window, |this, window, cx| {
            chrome::set_window_active(window.is_window_active());
            if !window.is_window_active() {
                this.cancel_session_numbers(cx);
                let editing = this.keyboard.is_editing();
                this.keyboard.cancel_edit();
                if editing { cx.notify(); }
                return;
            }
            this.refresh_desktop_palette(cx);
            // O papel de parede muda fora da janela; a volta do foco é quando repintar importa.
            let a = appearance::get();
            if a.background == appearance::Background::Desktop && a.wallpaper == appearance::Wallpaper::Glass { this.refresh_backdrop(window, cx); }
        }).detach();
    }

    /// Pede a paleta do papel de parede quando o tema é Desktop. Sem conexão, diz isso e desenha como Automático.
    pub(super) fn refresh_desktop_palette(&mut self, cx: &mut Context<Self>) {
        if appearance::get().theme != appearance::ThemeMode::Desktop { self.desktop_note = None; return; }
        let Some(api) = self.desktop_api() else {
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
        if let Some(entry) = self.server_entry(&servers::norm(&address))
            && !entry.label.is_empty() && entry.label != servers::default_label(&address) { return entry.label.clone(); }
        let host = address.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
        if host.is_empty() { tr("connection") } else { host.to_owned() }
    }

    /// Máquina da sessão aberta: a de outra máquina pelo nome da entrada dela, ou o host.
    fn session_label(&self, cx: &App) -> String {
        let Some(key) = self.open_key() else { return self.server_label(cx) };
        match self.server_entry(&key) {
            Some(entry) if !entry.label.is_empty() => entry.label.clone(),
            _ => key.trim_start_matches("http://").trim_start_matches("https://").to_owned(),
        }
    }

    /// Texto da última resposta do agente na conversa aberta, para o atalho de copiar.
    fn last_reply(&self) -> Option<String> {
        self.chat.events.iter().rev().find(|e| e.kind == "assistant_msg").map(|e| e.body())
    }

    fn failure(error: &Failure) -> String {
        match error.status {
            // Rota fora da sessão do convite não é login perdido: o convidado segue autenticado.
            Some(403) if error.detail == "erro_fora_do_convite" => tr_shared("erro_fora_do_convite", &[]),
            Some(409) if error.detail == "erro_sessao_orq" => tr_shared("erro_sessao_orq", &[]),
            Some(401 | 403) => tr("auth_error"), Some(410) => tr_shared("convite_encerrado", &[]), Some(429) => tr("rate_limited"),
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

    /// Falha numa rota de mod. Recusa com código `erro_mod_*` que o app traduz (o mesmo critério do `failure_detail`) vale
    /// em qualquer status, inclusive o 503 do dono único (`erro_mod_guarda_indisponivel`): o `detail` já é a frase dela.
    /// Sem esse código, sem resposta ou 5xx é a frase `generic` do app, e a recusa (4xx) diz o motivo dela.
    fn plugin_failure(error: &Failure, generic: impl FnOnce() -> String) -> String {
        let known = error.code.as_deref()
            .is_some_and(|code| code.starts_with("erro_mod_") && crate::i18n::tr_web(code, &HashMap::new()).is_some());
        if known { error.detail.clone() }
        else if error.status.is_none_or(|status| status >= 500) { generic() }
        else { Self::failure(error) }
    }

    /// Clique num botão de mod: sem resposta ou 5xx não é entrega de mensagem, e a frase de reenviar enganaria.
    fn press_failure(error: &Failure) -> String {
        Self::plugin_failure(error, || tr_shared("plugin_clique_falhou", &[]))
    }

    /// Aviso de falha de uma ação de mod; um novo substitui o anterior (`PluginFailure`).
    fn notify_plugin_failure(text: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = text { window.push_notification(Notification::warning(text).id::<PluginFailure>(), cx); }
    }

    /// Painel de mod que não fechou: a frase genérica é a mesma do web.
    fn close_failure(error: &Failure) -> String {
        Self::plugin_failure(error, || tr_shared("plugin_fechar_falhou", &[]))
    }

    /// Digitação num campo de mod que não chegou: a frase genérica é a mesma do web.
    fn input_failure(error: &Failure) -> String {
        Self::plugin_failure(error, || tr_shared("plugin_input_falhou", &[]))
    }

    /// Troca de aba recusada. Servidor sem a rota (404 ou 405) é servidor antigo, não erro: a troca local já valeu. A frase
    /// genérica é a da troca de aba do web.
    fn show_failure(error: &Failure) -> Option<String> {
        if matches!(error.status, Some(404 | 405)) { return None; }
        Some(Self::plugin_failure(error, || tr_shared("plugin_aba_falhou", &[])))
    }

    fn selected_key(&self) -> Option<SessionKey> {
        SessionKey::new(&self.session_server()?, self.selected.as_ref()?)
    }

    /// Conexão da máquina da sessão aberta; sem sessão de outra máquina, a do servidor ativo.
    pub(super) fn session_api(&self) -> Option<Api> { self.open_api.clone().or_else(|| self.api.clone()) }

    pub(super) fn session_server(&self) -> Option<String> { self.open_api.as_ref().map(Api::identity).or_else(|| self.server.clone()) }

    /// Conexão de `server` se ela ainda está aberta; `None` quando a sessão trocou de máquina no meio.
    pub(super) fn api_for(&self, server: &str) -> Option<Api> {
        if self.server.as_deref() == Some(server) { return self.api.clone(); }
        self.open_api.clone().filter(|api| api.identity() == server)
    }

    /// Dono do que sobrevive a reabrir a mesma sessão (terminal, arquivos, ditado): `selection` muda até no clique na própria aba.
    /// A máquina entra porque `connection` não muda ao abrir sessão de outra, e o nome pode se repetir entre elas.
    pub(super) fn session_owner(&self) -> Option<SessionOwner> {
        Some((self.connection, self.session_server()?, self.selected.as_ref()?.name.clone()))
    }

    /// Lista da máquina `server`: a ativa ou a de outra máquina já lida.
    /// `server` pode vir como identidade da conexão ou já normalizado (chave das listas e das linhas).
    pub(super) fn sessions_of(&self, server: &str) -> &[SessionInfo] {
        if self.is_active_key(&servers::norm(server)) { return &self.sessions; }
        self.remote.get(&servers::norm(server)).map_or(&[], |list| &list.sessions)
    }

    fn open_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connection_dialog { self.connection_origin = window.focused(cx).map(|focus| focus.downgrade()); }
        self.connection_dialog = true;
        self.address.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (address, token) = (self.address.read(cx).value().to_string(), self.token.read(cx).value().to_string());
        let api = match Api::new(&address, &token) {
            Ok(api) => api,
            Err(error) => {
                // A sessão de outra máquina que pediu esta troca não abre mais tarde no servidor que ficou.
                self.pending_open = None;
                self.error = Some(Self::failure(&error));
                self.address.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
                return;
            }
        };
        // Como o `cp_active` do web: a máquina que abriu vira a da próxima abertura.
        self.unsaved_connection = Some((address, token.clone()));
        if let Some(key) = self.selected_key() { self.drafts.insert(key, self.composer.read(cx).value().to_string()); }
        let reopen = self.selected.clone().zip(self.session_server().map(|s| servers::norm(&s)));
        // A lista da máquina que sai fica na barra até o SSE dela chegar, sem piscar vazia.
        let previous = self.server.as_deref().map(servers::norm).filter(|key| *key != servers::norm(&api.identity()))
            .map(|key| (key, std::mem::take(&mut self.sessions), self.list_online, self.list_error.clone(), std::mem::take(&mut self.live_terms)));
        self.drop_connection(window, cx);
        self.active_token = token;
        self.api = Some(api.clone());
        self.server = Some(api.identity());
        if let Some((key, sessions, online, error, live_terms)) = previous {
            self.remote.insert(key, servers::RemoteList { loaded: true, online, sessions, error, api: None, live_terms });
        }
        self.start_remote_lists();
        self.sync_updater(cx);
        self.connection_dialog = false;
        // Conectado por qualquer caminho: reabrir o cartão mostra endereço + token, não a entrada da primeira abertura.
        self.entry = None;
        self.root_focus.focus(window, cx);
        let tx = self.tx.clone();
        let connection = self.connection;
        let ready_sessions = self.ready_sessions.take();
        self.list_task = Some(self.runtime.spawn(async move {
            api::route::ensure(&api).await;
            let result = match ready_sessions { Some(sessions) => Ok(sessions), None => api.sessions().await };
            let fatal = result.as_ref().err().is_some_and(|e| matches!(e.status, Some(401 | 403 | 410)));
            if tx.send(Envelope { connection, selection: None, payload: Payload::Sessions(result) }).await.is_err() || fatal { return; }
            forward_stream(api, None, connection, None, tx).await;
        }));
        self.reset_device(window, cx);
        // Convite só enxerga a própria sessão: custos, contas, busca, configuração e avisos do servidor responderiam 403.
        if !self.active_invite() {
            self.costs_reconnected(cx);
            self.worktrees_reconnected(cx);
            self.search_reconnected(cx);
            self.refresh_default_account(cx);
            self.schedule_account_refresh(cx);
            // O rascunho é deste servidor: na troca ele morre, no "Reconectar" ao mesmo ele fica.
            self.server_config.reconnected(format!("{}\n{}", self.server.as_deref().unwrap_or(""), self.token.read(cx).value()));
            // Página do servidor aberta na troca: relê do servidor novo.
            if let Some(page) = self.settings { self.settings_opened(page, cx); }
            self.load_notification_preferences();
        }
        self.refresh_desktop_palette(cx);
        let a = appearance::get();
        if a.background == appearance::Background::Desktop && a.wallpaper == appearance::Wallpaper::Glass { self.refresh_backdrop(window, cx); }
        // A sessão aberta continua aberta, na máquina dela; a do servidor que acabou de conectar abre quando a lista chegar.
        match reopen {
            Some((session, key)) if Some(key.as_str()) == self.server.as_deref().map(servers::norm).as_deref() => self.pending_open = Some(session.name),
            Some((session, key)) => { self.select_on(&key, session, window, cx); }
            None => {}
        }
        cx.notify();
    }

    /// A lista do servidor voltou: o que foi lido uma vez só e falhou na queda (atalhos, nova conversa) é lido de novo.
    fn server_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.active_invite() && self.side.shortcuts_failed() { self.load_notification_preferences(); }
        for view in [self.new_chat.clone(), self.new_session.clone()].into_iter().flatten() {
            view.update(cx, |view, cx| view.server_back(window, cx));
        }
    }

    /// Silenciar, horas quietas e atalhos globais da máquina da conversa aberta: são deles que o aviso e o painel falam.
    /// Convite não alcança essas rotas do servidor do dono: nada é lido, e nenhum aviso usa as preferências de outra máquina.
    fn load_notification_preferences(&mut self) {
        let server = self.open_server();
        let api = self.machine_api(&server).filter(|_| !self.server_entry(&server).is_some_and(|s| s.invite));
        let n = &mut self.system_notifications;
        n.seq += 1;
        n.prefs = None;
        n.finished = None;
        let Some(api) = api else { return };
        let (seq, connection, tx) = (n.seq, self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let (config, prefs) = tokio::join!(api.config(), api.server_read(&["push", "settings"], &[], 15));
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Config(seq, config) }).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::PushSettings(seq, prefs) }).await;
        });
    }

    /// O que é da conexão atual sai da tela e os pedidos em voo passam a ser descartados. Serve à troca de servidor e ao Sair.
    fn drop_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_terminal(false, window, cx);
        self.mention.close();
        self.active_token.clear();
        self.connection += 1;
        self.files_connection_dropped(cx);
        self.selection += 1;
        self.revision += 1;
        for slot in [&mut self.list_task, &mut self.session_task, &mut self.history_task] { if let Some(t) = slot.take() { t.abort(); } }
        self.leave_accounts();
        (self.selected, self.open_api, self.pending_remote) = (None, None, None);
        (self.recents, self.reopen) = (Default::default(), None);
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
        for image in self.full_images.borrow_mut().clear() { cx.drop_image(image, Some(window)); }
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
        // A lista viva da nova conexão chega pelo stream dela; o cartão da pergunta segue, com a conexão que ele guarda.
        (self.live_terms, self.hangar_open, self.hangar_error) = (Vec::new(), false, None);
        self.sync = sync::Sync::default();
        // Sem isto, um "Ligar" em voo da conexão anterior deixava a página travada e o código dela montado.
        self.connect = connect::Connect::default();
        self.shared = shared_config::SharedConfig::default();
        self.controls = controls::Controls::default();
        self.accounts = accounts::Accounts::default();
        self.orchestration = orchestration::Orchestration::default();
        self.shortcuts = shortcuts::Shortcuts::default();
        self.harness = harness::Harnesses::default();
        self.machines = machines::Machines::default();
        self.system_notifications = SystemNotifications::default();
        self.computer = computer::Computer::default();
    }

    /// Sessão da lista do servidor ativo.
    fn select(&mut self, session: SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        // Escolher outra conversa desiste da de outra máquina que esperava a lista chegar.
        self.pending_remote = None;
        self.open_session(None, session, window, cx);
    }

    /// A mesma sessão de novo (Tentar de novo), na máquina em que ela já estava.
    fn reselect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.selected.clone() { self.open_session(self.open_api.clone(), session, window, cx); }
    }

    /// Sessão de qualquer máquina da lista, sem trocar o servidor ativo. Sem a conexão da máquina, avisa o motivo e devolve
    /// `false`: quem chamou não segue como se tivesse aberto.
    pub(super) fn select_on(&mut self, key: &str, session: SessionInfo, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.is_active_key(key) { self.select(session, window, cx); return true; }
        let Some(api) = self.machine_api(key) else {
            let text = tr("remote_open_failed").replace("{name}", &session.name).replace("{erro}", &self.machine_error(key));
            window.push_notification(Notification::warning(text), cx);
            return false;
        };
        self.pending_remote = None;
        self.open_session(Some(api), session, window, cx);
        true
    }

    fn open_session(&mut self, open_api: Option<Api>, session: SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        api::open_trace_start(&session.name);
        // Outra conversa escolhida no meio da criação: a mensagem segue sendo enviada, mas a bolha é da tela que ficou.
        self.opening = None;
        // Vindo da tela sem sessão (nova conversa ou conversa fechada), o texto dela fica guardado com ela.
        self.stash_view_draft(cx);
        self.reopen = None;
        let same_server = self.open_api.as_ref().map(Api::identity) == open_api.as_ref().map(Api::identity);
        if !same_server || self.selected.as_ref().is_none_or(|selected| selected.name != session.name) {
            self.close_terminal(false, window, cx);
        }
        if let Some(old) = self.selected_key() { self.drafts.insert(old, self.composer.read(cx).value().to_string()); }
        self.keep_expanded();
        self.open_api = open_api;
        // Avisos, atalhos globais e contas passam a ser os da máquina desta conversa.
        if !same_server { self.load_notification_preferences(); }
        // Com abas (só a lista ativa), a aba da sessão aberta entra na vista da faixa.
        if self.open_api.is_none() && let Some(ix) = self.sessions.iter().position(|s| s.name == session.name) { self.tabs_scroll.scroll_to_item(ix); }
        self.selection += 1;
        self.revision += 1;
        if let Some(t) = self.session_task.take() { t.abort(); }
        if let Some(t) = self.history_task.take() { t.abort(); }
        self.chat = Chat::default();
        self.system_notifications.reset_stream();
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
        self.history_limit = FIRST_PAGE;
        self.etag = None;
        self.has_older = false;
        self.rich.clear();
        self.pages.clear();
        // A busca era da conversa anterior.
        self.find.reset();
        row_patch::reset_rows(&mut self.row_ids, &mut self.arrived, &mut self.tree_folds);
        self.list_state.reset(0);
        self.follow_reset();
        let key = self.session_server().and_then(|server| SessionKey::new(&server, &session));
        let draft = key.as_ref().and_then(|key| self.drafts.get(key).cloned()).unwrap_or_default();
        self.expanded = key.as_ref().and_then(|key| self.kept_expanded.get(key).cloned()).unwrap_or_default();
        self.composer.update(cx, |input, cx| input.set_value(draft, window, cx));
        self.confirm = None;
        self.terminal_suggestion.clear();
        self.plugin_band = Value::Null;
        self.plugin_panes.clear();
        self.plugin_shown = None;
        self.plugin_columns = None;
        self.plugin_source = None;
        self.plugin_caps.clear();
        self.plugin_local_tab = None;
        self.plugin_tabs_seen = None;
        self.plugin_hovered.clear();
        self.plugin_fields.clear();
        self.close_recent();
        self.command_panel = false;
        // Os menus são da tela sem sessão: sem isto, o Esc seguinte seria gasto num deles, já fora da tela.
        self.new_chat_folders.set(None);
        self.suggest_dismissed = None;
        self.side.on_select();
        self.dossier = None;
        self.controls.on_select();
        self.reset_subagent_count();
        self.selected = Some(session.clone());
        self.voice_session_opened(cx);
        if !same_server || session.engine.as_deref().is_some_and(|e| !e.is_empty()) || session.uses_engine_account() {
            self.load_session_accounts(cx);
        }
        self.refresh_shortcut_terms(&session.name);
        if session.readable() {
            if let Some(api) = self.session_api() {
                let tx = self.tx.clone();
                let connection = self.connection;
                let selection = self.selection;
                self.session_task = Some(self.runtime.spawn(forward_stream(api, Some(session.name), connection, Some(selection), tx)));
            }
        }
        self.conversation = Default::default();
        self.sync_activity(cx);
        self.load_run_state();
        self.load_project_shortcuts();
        cx.notify();
    }

    /// Nova conversa e Nova sessão não abrem sem servidor nem por cima de um diálogo.
    fn create_blocked(&self, window: &mut Window, cx: &mut App) -> bool {
        self.api.is_none() || window.has_active_dialog(cx) || self.connection_dialog
    }

    /// Volta à tela sem sessão, a da nova conversa. O rascunho da sessão fica guardado como na troca de sessão.
    pub(super) fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.create_blocked(window, cx) { return; }
        if self.settings.is_some() && !self.settings_live() { self.close_settings(window, cx); }
        self.pending_remote = None;
        // Cada tela tem o próprio texto: o da conversa fechada fica com ela, e a nova conversa volta com o dela.
        let home = self.selected.is_none() && self.reopen.is_none();
        self.stash_view_draft(cx);
        self.close_open_session(window, cx);
        self.reopen = None;
        if !home { self.load_view_draft(window, cx); }
        self.composer.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Fecha a sessão aberta, guardando o rascunho dela.
    pub(super) fn close_open_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_none() { return; }
        self.close_terminal(false, window, cx);
        if let Some(old) = self.selected_key() { self.drafts.insert(old, self.composer.read(cx).value().to_string()); }
        self.keep_expanded();
        self.selection += 1;
        if let Some(task) = self.session_task.take() { task.abort(); }
        if let Some(task) = self.history_task.take() { task.abort(); }
        let remote = self.open_api.is_some();
        (self.selected, self.open_api) = (None, None);
        if remote { self.load_notification_preferences(); }
        self.opening = None;
        self.chat = Chat::default();
        self.turn_seen = None;
        self.reset_details();
        self.cancel_preview_drop();
        self.clear_visible_preview();
        self.error = None;
        self.loading = false;
        self.chat_online = false;
        self.close_popups();
        self.composer.update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Mais 400 eventos para trás: pelo botão ou pela rolagem que chega ao topo.
    pub(crate) fn load_older(&mut self, cx: &mut Context<Self>) {
        // Zero é o histórico inteiro, já carregado pela busca.
        if self.history_limit == 0 { return; }
        self.history_limit = self.history_limit.saturating_add(400);
        self.etag = None;
        self.load_history(cx);
    }

    /// O transcript inteiro (`limit=0`): a busca na conversa vale também para o que não foi carregado.
    pub(super) fn load_all(&mut self, cx: &mut Context<Self>) {
        if self.history_limit == 0 { return; }
        // Pedido em voo com o limite velho seria descartado na chegada: cancela e pede de novo.
        if let Some(task) = self.history_task.take() { task.abort(); }
        (self.history_limit, self.etag) = (0, None);
        self.load_history(cx);
    }

    fn load_history(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(session)) = (self.session_api(), self.selected.as_ref()) else { return; };
        if self.history_task.as_ref().is_some_and(|task| !task.is_finished()) { return; }
        self.history_started = true;
        self.loading = true;
        api::open_trace(|| format!("history request limit={}", self.history_limit));
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
                self.voice_sent(&key, &text, &result);
                self.receive_sent(key, text, draft, result, window, cx);
                cx.notify();
                return;
            }
            // A chamada é deste computador: a troca de servidor não a derruba, só a geração dela decide.
            Payload::Voice(generation, event) => { self.receive_voice(generation, event, window, cx); return; }
            Payload::VoiceGate(enabled, codex, saved, accounts) => { self.receive_voice_gate(enabled, codex, saved, accounts, window, cx); return; }
            Payload::VoiceHistory(generation, key, result) => { self.voice_history(generation, key, result); return; }
            Payload::VoiceModels(seq, result) => { self.receive_organizer_models(seq, result, window, cx); return; }
            Payload::VoiceDone(generation, call, done) => { self.voice_done(generation, call, done, window, cx); return; }
            Payload::VoiceJev(generation, asked, result) => { self.voice_jev_result(generation, asked, result, window, cx); return; }
            Payload::Files(key, owner, generation, files) => { self.receive_files(key, owner, generation, files, cx); cx.notify(); return; }
            Payload::UploadStep(key, id, result) => { let key = self.delivery.current(key); self.receive_upload(key, id, result); cx.notify(); return; }
            Payload::UploadsDone(key, draft, steer, known, group) => {
                let key = self.delivery.current(key);
                self.finish_uploads(key, draft, steer, known, group, cx);
                cx.notify();
                return;
            }
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
                let lost = self.selected_key().as_ref() == Some(&key) && result.as_ref().err().is_some_and(|e| self.chat_auth_lost(e));
                if lost
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
            Payload::Lan(address, lan) => { self.remember_lan(&address, lan); return; }
            // Cada máquina tem a própria geração: a troca do ativo não derruba as listas das outras.
            Payload::Remote(generation, key, update) => {
                if generation == self.remote_gen {
                    // Os terminais vivos dela (evento novo ou stream caído) acertam o chip, os blocos e o painel.
                    let live = matches!(&update, servers::RemoteUpdate::Stream(Update::Offline(_)))
                        || matches!(&update, servers::RemoteUpdate::Stream(Update::Frame(frame)) if frame.event == "shortcut_terminals");
                    // Menu, renomear, fechar e grupo das linhas desta máquina acompanham a lista dela como os da ativa.
                    if self.receive_remote(generation, key.clone(), update, cx) { self.sidebar_sessions_changed(window, cx); }
                    self.voice_sessions();
                    self.remote_changed(&key, window, cx);
                    if live { self.live_changed(&key, window, cx); }
                }
                return;
            }
            payload => payload,
        };
        if envelope.connection != self.connection { return; }
        if envelope.selection.is_some_and(|selection| selection != self.selection) { return; }
        let is_chat = envelope.selection.is_some();
        // A conversa só é refeita quando o chat mudou, e a janela só redesenha quando algo visível mudou:
        // ping, lista e estatísticas chegam o tempo todo e não mexem nas linhas.
        let selection = self.selection;
        let (mut rows, mut visible, mut tail, mut keep_end) = (false, true, false, false);
        match payload {
            Payload::Terminal(reply) => self.receive_terminal(reply, window, cx),
            Payload::Mentions(seq, result) => { self.receive_mentions(seq, result, cx); }
            Payload::Sessions(Ok(sessions)) => {
                self.list_error = None;
                if let Some((address, token)) = self.unsaved_connection.take() {
                    let known = self.servers.iter().any(|s| servers::norm(&s.address) == servers::norm(&address));
                    let label = if known { String::new() } else { servers::default_label(&address) };
                    servers::upsert(&mut self.servers, servers::ServerEntry { id: servers::new_id(), label, address, token, disabled: false, invite: false, lan: None, ephemeral: false });
                    // Disco fora da thread da janela; só a falha volta.
                    self.persist_servers();
                    self.sync_updater(cx);
                }
                self.replace_sessions(sessions, window, cx);
                self.voice_sessions();
                // Primeira lista desta conexão: liga a troca de servidor e a abertura do app.
                self.refresh_voice_gate(cx);
                if let Some(name) = self.pending_open.take()
                    && let Some(session) = self.sessions.iter().find(|s| s.name == name).cloned() {
                    self.select(session, window, cx);
                }
            }
            Payload::Sessions(Err(error)) => {
                self.pending_open = None;
                if self.auth_lost(&error) {
                    self.open_connection(window, cx);
                    self.error = Some(Self::failure(&error));
                }
                self.list_error = Some(self.active_failure(&error));
            }
            Payload::Stream(Update::Online) => {
                if is_chat {
                    api::open_trace(|| "sse online".into());
                    self.system_notifications.reset_stream();
                    self.pending_chat.retain(|update| !matches!(update, ChatUpdate::State(_)));
                    self.chat_online = true;
                    self.error = None;
                    if !self.history_started { self.load_history(cx); }
                } else {
                    let back = !self.list_online;
                    self.list_online = true;
                    if back { self.server_back(window, cx); }
                }
            }
            Payload::Stream(Update::Offline(error)) => {
                if if is_chat { self.chat_auth_lost(&error) } else { self.auth_lost(&error) } {
                    self.open_connection(window, cx);
                    self.error = Some(Self::failure(&error));
                }
                if is_chat { self.chat_online = false; self.error = Some(self.chat_failure(&error)); }
                else {
                    self.list_online = false;
                    self.list_error = Some(self.active_failure(&error));
                    // Stream da lista caído: o chip e os blocos não mostram terminal velho dele.
                    if !std::mem::take(&mut self.live_terms).is_empty() {
                        let server = self.active_key();
                        self.live_changed(&server, window, cx);
                    }
                }
            }
            Payload::Stream(Update::Frame(frame)) => {
                let applied = if is_chat {
                    let (applied, changed) = self.accept_chat_frame(&frame.event, frame.data, window, cx);
                    (rows, visible, tail) = (changed == Changed::Rows, !matches!(changed, Changed::Nothing | Changed::Bottom), changed == Changed::Tail);
                    if changed == Changed::Bottom { self.redraw(panes::Area::Bottom, cx); }
                    applied
                } else if frame.event == "sessions" {
                    match serde_json::from_value(frame.data) {
                        Ok(sessions) => {
                            // Lista igual à que está na tela não redesenha a janela.
                            visible = self.list_error.is_some() || self.sessions != sessions;
                            self.list_error = None;
                            self.replace_sessions(sessions, window, cx);
                            self.voice_sessions();
                            true
                        }
                        Err(_) => { self.list_error = Some(tr("invalid_response")); false }
                    }
                } else if frame.event == "list_error" { self.list_error = Some(tr("list_stale")); true }
                else if frame.event == "nav" { self.receive_nav(frame.data, window, cx); true }
                else if frame.event == "shortcut_terminals" {
                    let list = terminal::parse_live_terms(&frame.data);
                    // Lista igual à de antes não redesenha; o tempo de "rodando" anda pelo relógio próprio.
                    if list != self.live_terms {
                        self.live_terms = list;
                        let server = self.active_key();
                        self.live_changed(&server, window, cx);
                    } else { visible = false; }
                    true
                }
                else { visible = false; true };
                let _ = frame.applied.send(applied);
            }
            Payload::History(revision, limit, result) if revision == self.revision && limit == self.history_limit => {
                rows = true;
                self.history_task = None;
                self.loading = false;
                api::open_trace(|| "history on ui thread".into());
                match result {
                    Ok(history) => {
                        self.etag = history.etag;
                        if let Some(events) = history.events {
                            self.has_older = limit > 0 && events.len() >= limit;
                            self.chat.merge_history(events);
                            api::open_trace(|| "history merged".into());
                            if self.chat.preview.text.is_empty() {
                                self.cancel_preview_drop();
                                self.clear_visible_preview();
                            }
                        }
                        let has_pages = self.chat.events.iter().any(|event| conversation::is_page_call(event.tool_name.as_deref()));
                        self.pages.warm(has_pages, window, cx);
                        let first = !self.history_installed;
                        // A janela inteira que veio por baixo da primeira página.
                        keep_end = !first && limit == HISTORY_PAGE;
                        self.history_installed = true;
                        for update in std::mem::take(&mut self.pending_chat) { self.apply_chat_update(update, window, cx); }
                        api::open_trace(|| "pending applied".into());
                        self.error = None;
                        self.ensure_commands(false);
                        self.discover_plan();
                        // A primeira conta de subagentes espera a conversa chegar, como o `aoAquecer` do web.
                        if first { self.restart_subagent_count(cx); }
                        // Primeira página cheia: a janela inteira vem por baixo, sem aviso de carregando e sem o
                        // "Carregar anteriores" de uma janela que já está crescendo.
                        if first && limit < HISTORY_PAGE && self.has_older {
                            self.history_limit = HISTORY_PAGE;
                            self.etag = None;
                            self.load_history(cx);
                            (self.loading, self.has_older) = (false, false);
                        }
                    }
                    Err(error) => {
                        // O histórico inteiro (busca) falhou: volta à janela normal, e a busca e o "Carregar anteriores"
                        // podem pedir de novo em vez de ficarem presos ao que já veio.
                        if limit == 0 { (self.history_limit, self.has_older) = (HISTORY_PAGE, true); }
                        if error.status == Some(404) {
                            if let Some(api) = self.session_api() {
                                let tx = self.tx.clone();
                                let connection = self.connection;
                                // Sessão de outra máquina: a lista relida é a dela, e a aberta a acompanha por `remote_changed`.
                                let remote = self.open_key().map(|key| (self.remote_gen, key));
                                self.runtime.spawn(async move {
                                    let result = api.sessions().await;
                                    let payload = match remote {
                                        Some((generation, key)) => Payload::Remote(generation, key, servers::RemoteUpdate::Sessions(result)),
                                        None => Payload::Sessions(result),
                                    };
                                    let _ = tx.send(Envelope { connection, selection: None, payload }).await;
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
                let started = std::time::Instant::now();
                media::trace(format_args!("thumb installed {source:?}"));
                for image in self.media.insert((key, source), state) { cx.drop_image(image, Some(window)); }
                let rows = self.row_ids.len();
                if rows > 0 { self.follow_content_changed(cx); self.list_state.remeasure_items(0..rows); }
                media::trace(format_args!("thumb install took {:.2} ms, remeasured {rows} rows", started.elapsed().as_secs_f64() * 1000.));
            }
            Payload::Config(seq, result) => {
                if seq != self.system_notifications.seq { return; }
                self.system_notifications.finished = result.as_ref().ok().and_then(|v| Some((
                    v.pointer("/campos/notify_finished/valor")?.as_bool()?,
                    v.pointer("/campos/finish_min_seconds/valor")?.as_u64()?,
                )));
                self.side.receive_config(result.map_err(|error| Self::failure(&error)));
            }
            Payload::PushSettings(seq, result) => {
                if seq != self.system_notifications.seq { return; }
                self.system_notifications.prefs = result.ok().and_then(PushPreferences::parse);
                if self.system_notifications.prefs.is_none() || self.system_notifications.finished.is_none() {
                    window.push_notification(Notification::warning(tr("notify_settings_failed")), cx);
                }
            }
            Payload::Device(reply) => { self.receive_device(reply, window, cx); return; }
            Payload::Accounts(reply) => { self.receive_accounts(reply, window, cx); return; }
            Payload::Orchestration(reply) => { self.receive_orchestration(reply, cx); return; }
            Payload::Shortcuts(reply) => { self.receive_shortcuts(reply, window, cx); return; }
            Payload::Harness(reply) => { self.receive_harness(reply, cx); return; }
            Payload::ServerConfig(reply) => {
                // A configuração gravada é da ativa: só vale para os avisos quando a conversa aberta é dela.
                if matches!(&reply, server_config::ServerConfigReply::QuietSaved(..) | server_config::ServerConfigReply::Saved(..))
                    && self.open_api.is_none() {
                    self.load_notification_preferences();
                }
                self.receive_server_config(reply, window, cx); return;
            }
            Payload::Sync(reply) => { self.receive_sync(reply, window, cx); return; }
            Payload::Connect(reply) => { self.receive_connect(reply, window, cx); return; }
            Payload::SharedConfig(reply) => { self.receive_shared_config(reply, cx); return; }
            Payload::Machines(reply) => { self.receive_machines(reply, window, cx); return; }
            Payload::Computer(reply) => { self.receive_computer(reply, window, cx); return; }
            Payload::Create(dialog, reply) => { self.receive_create(dialog, reply, window, cx); return; }
            Payload::Transfer(dialog, reply) => { self.receive_agent_transfer(dialog, reply, window, cx); return; }
            Payload::PluginPressed(result) => { self.receive_plugin_press(result, window, cx); return; }
            Payload::PluginClosed(result) => {
                Self::notify_plugin_failure(result.err().map(|error| Self::close_failure(&error)), window, cx);
                return;
            }
            Payload::PluginShown(result) => {
                Self::notify_plugin_failure(result.err().and_then(|error| Self::show_failure(&error)), window, cx);
                return;
            }
            Payload::PluginInput(site, control, field, result) => {
                Self::notify_plugin_failure(result.err().map(|error| Self::input_failure(&error)), window, cx);
                // O próximo da fila só sai pelo mesmo campo: um campo recriado com a mesma `key` tem fila própria.
                let next = self.plugin_fields.get_mut(&crate::plugin_ui::field_id(&site, &control))
                    .filter(|f| f.state.entity_id() == field).and_then(|f| f.outbox.done());
                if let Some(next) = next { self.send_plugin_input(&site, &control, next); }
                return;
            }
            Payload::Sidebar(reply) => {
                // Só o silenciar da máquina da conversa aberta muda as preferências que os avisos desta janela leem.
                if matches!(&reply, sidebar::SidebarReply::Wrote(t, sidebar::Write::Mute(_), _) if t.server == self.open_server()) { self.load_notification_preferences(); }
                self.receive_sidebar(reply, window, cx); return;
            }
            Payload::Activity(reply) => { self.receive_activity(reply, cx); return; }
            Payload::Dictation(seq, path, result) => { self.receive_dictation(seq, path, result, window, cx); return; }
            Payload::FileView(reply) => { self.receive_file_view(reply, window, cx); return; }
            Payload::Dossier(key, seq, result) => { self.receive_dossier(key, seq, result, cx); return; }
            Payload::DesktopPalette(seq, result) => { self.receive_desktop_palette(seq, result, window, cx); return; }
            Payload::Sent(..) | Payload::Interrupted(..) | Payload::Acted(..) | Payload::Files(..) | Payload::UploadStep(..)
                | Payload::UploadsDone(..) | Payload::Saved(..) | Payload::ConnectionNotSaved(..) | Payload::Reply(..) | Payload::HeadlessPlan(..)
                | Payload::AppearanceSaved(..) | Payload::Backdrop(..) | Payload::BackdropPicked(..) | Payload::BackdropRemoved(..)
                | Payload::Remote(..) | Payload::Lan(..) | Payload::Voice(..) | Payload::VoiceGate(..) | Payload::VoiceHistory(..) | Payload::VoiceDone(..) | Payload::VoiceModels(..) | Payload::VoiceJev(..) => unreachable!(),
        }
        // Lista que trocou ou tirou a sessão aberta refaz a conversa.
        if rows || self.selection != selection { self.sync_rows(cx); }
        if keep_end { self.follow_keep_end(); }
        else if tail {
            self.sync_tail_rows(cx);
            self.redraw(panes::Area::Conversation, cx);
            return;
        }
        if visible { cx.notify(); }
    }

    fn receive_sent(&mut self, key: SessionKey, text: String, draft: String, result: Result<Delivery, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.delivery.current(key);
        let outcome = match &result {
            Ok(delivery) if delivery.delivered => SendOutcome::Delivered,
            Ok(_) => SendOutcome::Queued,
            Err(error) if error.uncertain => SendOutcome::Uncertain,
            Err(error) => SendOutcome::Rejected(Self::failure(error)),
        };
        let typed = self.delivery.typed(&key, &text);
        if !self.delivery.complete(&key, &text, outcome) { return; }
        if result.is_ok() { self.bridge_sending(key.clone(), cx); }
        self.sync_working_row(cx);
        let current = self.selected_key().as_ref() == Some(&key);
        let confirmed = self.delivery.outcome(&key).is_none();
        if result.is_ok() || confirmed {
            // O que saiu do campo no Enter já não está nele: o que estiver lá agora é texto novo, mesmo que igual.
            if !typed {
                if self.drafts.get(&key).is_some_and(|saved| saved == &draft) { self.drafts.remove(&key); }
                if current && !draft.is_empty() && self.composer.read(cx).value().as_ref() == draft {
                    self.composer.update(cx, |input, cx| input.set_value("", window, cx));
                }
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
            // Quem esperava este envio sai agora, na ordem em que foi mandado.
            if let Some((next, steer, group)) = self.delivery.next_held(&key) {
                let known = if current { self.known_user_ids() } else { HashSet::new() };
                if self.post(key.clone(), next.clone(), next.clone(), steer, known, group, cx) {
                    self.delivery.mark_typed(&key);
                    self.sync_working_row(cx);
                } else {
                    let mut back = self.delivery.take_held(&key);
                    back.insert(0, next);
                    self.return_to_field(&key, back, current, window, cx);
                }
            }
        } else {
            let mut back = self.delivery.take_held(&key);
            if typed { back.insert(0, text); }
            if !back.is_empty() { self.return_to_field(&key, back, current, window, cx); }
            if !typed && !current && !draft.is_empty() { self.drafts.entry(key.clone()).or_insert(draft); }
            if current && result.as_ref().err().is_some_and(|e| self.chat_auth_lost(e)) {
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
        // Sessão que sumiu da lista acabou de fechar: a conversa dela passa a ser recente.
        let closed = self.sessions.iter().any(|old| !sessions.iter().any(|s| s.name == old.name));
        self.sessions = sessions;
        self.recents_sessions_changed(closed, cx);
        self.resolve_local_dirs(cx);
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
        if old.is_some() && self.open_api.is_none() {
            let list = self.sessions.clone();
            self.follow_open(&list, window, cx);
        }
        self.sidebar_sessions_changed(window, cx);
    }

    /// Transcript trocado na mesma sessão (`/clear`): envio em voo, espera, campo, anexos e "mandar pro grupo" seguem para a
    /// chave nova. Devolve a antiga, que a abertura ainda preenche com o campo e deve sair depois.
    fn follow_transcript(&mut self, old: &SessionInfo, new: &SessionInfo, cx: &mut Context<Self>) -> Option<SessionKey> {
        if new.jsonl == old.jsonl || new.lifecycle_id != old.lifecycle_id { return None; }
        let server = self.session_server()?;
        let (from, to) = (SessionKey::new(&server, old)?, SessionKey::new(&server, new)?);
        self.delivery.rekey(&from, &to);
        if let Some(list) = self.attachments.remove(&from) { self.attachments.insert(to.clone(), list); }
        if let Some(batch) = self.uploading.remove(&from) { self.uploading.insert(to.clone(), batch); }
        if let Some((on, _)) = self.sidebar.grouping.send_to_group.as_mut().filter(|(on, _)| *on == from) { *on = to.clone(); }
        self.drafts.remove(&from);
        self.drafts.insert(to, self.composer.read(cx).value().to_string());
        Some(from)
    }

    /// A sessão aberta acompanha a lista da máquina dela: dados novos, transcript trocado ou sumiço.
    pub(super) fn follow_open(&mut self, list: &[SessionInfo], window: &mut Window, cx: &mut Context<Self>) {
        // Antes de soltar a conexão aberta: o renomear em voo é da máquina dela.
        let target = self.selected_target();
        if self.new_session.as_ref().is_some_and(|d| d.read(cx).transfer_busy_for(target.as_ref())) { return; }
        // Trocando de conta, a sessão some e volta noutro transcript: a conversa fica na tela até a resposta e até a lista
        // trazê-la de volta; sumida por mais que o prazo depois da resposta, vale o "sessão encerrada" de sempre.
        // Resposta perdida (conexão trocada no meio) não segura a tela além do prazo do próprio pedido.
        if let Some(t) = target.as_ref() && let Some((sent, answered)) = self.sidebar.moving.get(t).copied() {
            let back = list.iter().any(|s| s.name == t.name);
            let settled = match answered { Some(at) => back || at.elapsed() > Duration::from_secs(15), None => sent.elapsed() > Duration::from_secs(130) };
            if !settled { return; }
            self.sidebar.moving.remove(t);
        }
        if let Some(old) = self.selected.clone() {
            match list.iter().find(|s| s.name == old.name).cloned() {
                Some(new) if new.jsonl != old.jsonl || new.tracked != old.tracked => {
                    let moved = self.follow_transcript(&old, &new, cx);
                    self.open_session(self.open_api.clone(), new, window, cx);
                    if let Some(from) = moved { self.drafts.remove(&from); }
                }
                Some(new) => {
                    let before = self.selected_key();
                    if let Some(key) = &before { self.controls.on_session_update(key, &new); }
                    self.selected = Some(new);
                    // Recentes listados para a chave de antes: com outra chave, a lista some da tela mas seguiria aberta.
                    if self.selected_key() != before { self.close_recent(); }
                }
                None => {
                    self.close_terminal(false, window, cx);
                    self.selection += 1;
                    if let Some(task) = self.session_task.take() { task.abort(); }
                    if let Some(task) = self.history_task.take() { task.abort(); }
                    let remote = self.open_api.is_some();
                    (self.selected, self.open_api) = (None, None);
                    if remote { self.load_notification_preferences(); }
                    self.chat = Chat::default();
                    self.turn_seen = None;
                    self.reset_details();
                    self.cancel_preview_drop();
                    self.clear_visible_preview();
                    self.loading = false;
                    self.chat_online = false;
                    if !target.is_some_and(|t| self.lost_while_renaming(&t)) { self.error = Some(tr("session_gone")); }
                }
            }
        }
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
            "plugin_ui" | "plugin_ui_delta" => {
                // A árvore chega por valor: `surfaces` move em vez de copiar centenas de KB por evento. A diferença
                // (`plugin_ui_delta`) junta-se à vista anterior, também movida.
                let old_ids = crate::plugin_ui::pane_ids(&self.plugin_panes);
                let data = if event == "plugin_ui_delta" {
                    match crate::plugin_ui::apply_delta(&mut self.plugin_band, &mut self.plugin_panes, data) {
                        Ok(data) => data,
                        Err(_) => { self.error = Some(tr("invalid_response")); return (false, Changed::Screen); }
                    }
                } else { data };
                let s = crate::plugin_ui::surfaces(data);
                self.plugin_local_tab = crate::plugin_ui::follow_local(&old_ids, &crate::plugin_ui::pane_ids(&s.panes),
                    self.plugin_local_tab.as_deref());
                self.plugin_band = s.above;
                self.plugin_panes = s.panes;
                self.plugin_shown = s.shown_id;
                self.plugin_columns = s.columns;
                self.plugin_source = s.source;
                self.plugin_caps = s.caps;
                self.plugin_draws += 1;
                self.keep_plugin_hovered();
                return (true, Changed::Screen);
            }
            "plugin_toast" => {
                self.show_plugin_toast(&data, window, cx);
                return (true, Changed::Nothing);
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
                // A bolha da fila já mostra o envio: a de saída seria a segunda cópia até a resposta do POST.
                if event.kind == "user_msg" {
                    if let (Some(key), Some(text)) = (self.selected_key(), event.text.as_deref()) {
                        self.delivery.confirm_real(&key, &event.id, text);
                    }
                }
                let assistant = event.kind == "assistant_msg";
                self.chat.apply(event);
                if assistant { self.voice_message(); }
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
                let notice = self.system_notifications.advance(&state.state, Instant::now());
                if self.history_installed && self.chat_online && self.system_notifications.finished.is_some() && !window.is_window_active() {
                    if let (Some(message), Some(session), Some(prefs)) = (notice, &self.selected, &self.system_notifications.prefs) {
                        if !prefs.suppressed(&session.name, chrono::Local::now().time()) {
                            let (title, body) = (format!("Hangar · {}", session.name), tr(message));
                            self.runtime.spawn_blocking(move || show_system_notification(&title, &body));
                        }
                    }
                }
                // Turno terminou: o plano do Claude com terminal pode ter mudado de arquivo.
                let finished = self.chat.state.state == "working" && state.state != "working";
                let resumed = self.chat.state.state == "awaiting_input" && state.state == "working";
                let turned = (self.chat.state.state == "working") != (state.state == "working");
                // A duração sai do começo antes de ele ser zerado: é ela que a linha final mostra.
                if finished { self.chat.turn_done = Some(turn_done_text(self.turn_start())); }
                else if state.state == "working" { self.chat.turn_done = None; }
                // Estado vazio é a conversa recém-aberta: o turno já corria, e quem conta é o último envio.
                if turned { self.turn_seen = (state.state == "working" && !self.chat.state.state.is_empty()).then(Instant::now); }
                if state.state == "working" { self.sent_until = None; }
                if finished { self.voice_turn_finished(&state.state, cx); }
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
                // Vazio é o bloco fechado: o registro chega pelo transcript; esperar deixava o primeiro pedaço preso na tela.
                if text.is_empty() {
                    self.chat.live_thinking.clear();
                    self.live_clear_epoch[Live::Thinking as usize] += 1;
                }
                else if self.chat.update_live_thinking(text) { self.live_clear_epoch[Live::Thinking as usize] += 1; }
            }
            ChatUpdate::LiveTool(tool) => match tool {
                Some(tool) => if self.chat.update_live_tool(tool) { self.live_clear_epoch[Live::Tool as usize] += 1; },
                None => self.defer_live_clear(Live::Tool, cx),
            },
            ChatUpdate::Reset => {
                self.system_notifications.reset_stream();
                // Transcript trocado: as perguntas respondidas desta sessão não valem mais.
                if let Some(key) = self.selected_key() {
                    self.answered_tools.retain(|(owner, _)| owner != &key);
                    self.answering.remove(&key);
                }
                self.revision += 1;
                self.terminal_suggestion.clear();
                self.plugin_band = Value::Null;
                self.plugin_panes.clear();
                self.plugin_shown = None;
                self.plugin_columns = None;
                self.plugin_source = None;
                self.plugin_caps.clear();
                self.plugin_local_tab = None;
                self.plugin_tabs_seen = None;
                self.plugin_hovered.clear();
                self.plugin_fields.clear();
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
                row_patch::reset_rows(&mut self.row_ids, &mut self.arrived, &mut self.tree_folds);
                self.pages.clear();
                self.list_state.reset(0);
                self.follow_reset();
                self.etag = None;
                self.load_history(cx);
            }
        }
    }

    /// Guarda o que está aberto na conversa que sai da tela; a chave leva o transcript, e um `/clear` não herda nada.
    fn keep_expanded(&mut self) {
        let Some(key) = self.selected_key() else { return; };
        if self.expanded.is_empty() { self.kept_expanded.remove(&key); }
        else { self.kept_expanded.insert(key, self.expanded.clone()); }
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
        toggle_detail(&mut self.expanded, &mut self.prepared, &key);
        self.prepare_tools();
        let row = self.row_ids.iter().position(|id| id == &key || id.strip_prefix(PINNED) == Some(key.as_str())).or_else(|| {
            let events = &self.chat.events;
            self.conversation.items.iter().position(|item| match item {
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
        // Bolha da fila que já existia antes do envio não confirma o envio novo, mesmo com o mesmo texto.
        self.chat.events.iter().filter(|event| event.kind == "user_msg").map(|event| event.id.clone()).collect()
    }

    fn commands_key(&self) -> Option<String> {
        let (server, session) = (self.session_server()?, self.selected.as_ref()?);
        Some(format!("{server}|{}|{}", session.provider, session.name))
    }

    fn command_list(&self) -> &[CommandInfo] {
        self.commands_key().and_then(|key| self.commands.get(&key)).and_then(|r| r.as_ref().ok()).map(Vec::as_slice).unwrap_or(&[])
    }

    // Codex muda a lista conforme o modo: sempre relê. Os demais usam a lista já buscada.
    fn ensure_commands(&mut self, force: bool) {
        let (Some(api), Some(session), Some(cache)) = (self.session_api(), self.selected.clone(), self.commands_key()) else { return; };
        if !force && session.provider != "codex" && self.commands.get(&cache).is_some_and(|r| r.is_ok()) { return; }
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.commands(&session.name).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Commands(cache, result) }).await;
        });
    }

    // O orquestrador recusa mensagem (409): nada que envie pela conversa dele sai daqui.
    fn can_send(&self) -> bool {
        !self.connection_dialog && self.chat_online && self.history_installed && !self.selected.as_ref().is_some_and(SessionInfo::orq)
    }

    /// O plugin desta sessão, com ou sem terminal, responde o `/btw` num painel (`caps` do `plugin_ui`).
    fn btw_ready(&self) -> bool {
        self.plugin_caps.iter().any(|cap| cap == "btw")
    }

    // `confirmed` = a pessoa já aceitou o aviso de comando destrutivo para este mesmo texto.
    fn submit(&mut self, steer: bool, confirmed: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.defer_dictation_send(steer, cx) { return; }
        if self.selected.is_none() && self.reopen.is_some() { self.send_reopen(window, cx); return; }
        if self.selected.is_none() {
            let text = self.composer.read(cx).value().to_string();
            let attached = self.attachments.get(&create::new_chat_key()).map(|list| list.iter()
                .map(|a| (a.name.clone(), a.bytes.clone(), a.image.is_some() || composer::image_format(&a.name).is_some())).collect::<Vec<_>>())
                .unwrap_or_default();
            if text.trim().is_empty() && attached.is_empty() { return; }
            if let Some(view) = self.new_chat.clone() {
                let selection = self.selection;
                let names = attached.iter().map(|(name, ..)| name.clone()).collect();
                view.update(cx, |view, cx| view.create(Some((selection, text.clone(), attached)), cx));
                if view.read(cx).creating { self.begin_opening(view, text, names, window, cx); }
                cx.notify();
            }
            return;
        }
        if !self.can_send() { return; }
        let Some(key) = self.selected_key() else { return; };
        let text = self.composer.read(cx).value().to_string();
        let attached = self.attachments.get(&key).is_some_and(|list| !list.is_empty());
        if text.trim().is_empty() && !attached { return; }
        let flying = self.delivery.pending(&key);
        // Anexo sobe antes do envio e não entra na espera: aguarda o envio em voo, como o que já está subindo.
        if self.uploading.contains_key(&key) || flying && attached { return; }
        // Com anexo a legenda vai na frente do prompt: o comando do campo continua sendo o da mensagem.
        let provider = self.provider().0.to_owned();
        if let Some(command) = composer::typed_command(self.command_list(), &text).cloned() {
            if let Some(warning) = composer::blocked_command(&provider, &command, self.btw_ready()) {
                self.action_feedback.insert(key, (tr(warning).replace("{cmd}", &format!("/{}", command.name)), true));
                cx.notify();
                return;
            }
            if command.destructive && !confirmed && !appearance::get().skip_chat_confirmations {
                self.confirm = Some(Confirm::Destructive(text));
                self.confirm_no_ask = false;
                cx.notify();
                return;
            }
        }
        self.confirm = None;
        self.action_feedback.remove(&key);
        // O grupo é o da hora do Enter: o texto pode sair depois, com outra conversa aberta ou o grupo mudado.
        let group = if steer { None } else { self.group_targets(&key, &text) };
        // Sem terminal, o `/btw` só sai traduzido e com o plugin anunciando que o atende; mesmo com a lista de
        // comandos ainda vazia (sem `blocked_command`), nunca vai cru ao Claude Code.
        let surface_btw = group.is_none() && self.plugin_source == Some(crate::plugin_ui::UiSource::Surface) && composer::side_question(&text);
        if surface_btw && !self.btw_ready() {
            self.action_feedback.insert(key, (tr("command_btw_unavailable").replace("{cmd}", "/btw"), true));
            cx.notify();
            return;
        }
        let text = if surface_btw { composer::surface_side_question(&text) } else { text };
        // Enter com um envio em voo não se perde: o texto sai do campo e vai na vez dele.
        if flying {
            self.delivery.hold(key.clone(), text, steer, group);
            self.clear_sent_field(&key, window, cx);
            return;
        }
        let known = self.known_user_ids();
        if attached { self.start_uploads(key, text, steer, known, group, cx); return; }
        self.deliver(key.clone(), text.clone(), text, steer, known, group, cx);
        // O campo esvazia no Enter, sem esperar o backend: o que for digitado depois é outra mensagem.
        if self.delivery.pending(&key) {
            self.delivery.mark_typed(&key);
            self.clear_sent_field(&key, window, cx);
        }
    }

    /// Texto que saiu do campo no Enter não fica nele nem no rascunho guardado da conversa.
    fn clear_sent_field(&mut self, key: &SessionKey, window: &mut Window, cx: &mut Context<Self>) {
        self.drafts.remove(key);
        self.composer.update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Envio que falhou devolve ao campo o que saiu dele, na ordem, antes do que foi digitado depois.
    fn return_to_field(&mut self, key: &SessionKey, mut texts: Vec<String>, current: bool, window: &mut Window, cx: &mut Context<Self>) {
        let rest = if current { self.composer.read(cx).value().to_string() } else { self.drafts.get(key).cloned().unwrap_or_default() };
        if !rest.trim().is_empty() { texts.push(rest); }
        let merged = texts.join("\n");
        if current { self.composer.update(cx, |input, cx| input.set_value(merged, window, cx)); }
        else { self.drafts.insert(key.clone(), merged); }
    }

    /// `group`: com o "mandar pro grupo" ligado no Enter, os nomes que recebem (ela e os membros).
    fn deliver(&mut self, key: SessionKey, text: String, draft: String, steer: bool, known: HashSet<String>, group: Option<Vec<String>>, cx: &mut Context<Self>) {
        if !self.post(key.clone(), text, draft, steer, known, group, cx) { return; }
        self.sync_working_row(cx);
        self.error = None;
        self.stop_feedback.remove(&key);
        self.follow_engage(cx);
        // O envio muda o aviso, o botão e o erro da faixa de baixo, que é guardada entre quadros.
        cx.notify();
    }

    /// O envio em si, sem mexer na conversa aberta: serve também ao texto que esperava a vez noutra sessão.
    fn post(&mut self, key: SessionKey, text: String, draft: String, steer: bool, known: HashSet<String>, group: Option<Vec<String>>, cx: &mut Context<Self>) -> bool {
        // Sessão fora da tela (envio nomeado pela voz, fila de outra sessão) usa a conexão guardada da máquina dela.
        let Some(api) = self.api_for(&key.server).or_else(|| self.machine_api(&servers::norm(&key.server))) else {
            self.action_feedback.insert(key, (tr("server_changed"), true));
            cx.notify();
            return false;
        };
        if !self.delivery.begin(key.clone(), text.clone(), known) { cx.notify(); return false; }
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = match group {
                Some(names) => api.broadcast(&names, &text).await.and_then(|results| group_sheet::group_delivery(&key.name, results)),
                None if steer => api.steer_text(&key.name, &text).await,
                None => api.send(&key.name, &text).await,
            };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Sent(key, text, draft, result) }).await;
        });
        true
    }

    // Sobe um por vez; o que já subiu não sobe de novo numa nova tentativa, e falha para a fila sem repetir.
    fn start_uploads(&mut self, key: SessionKey, draft: String, steer: bool, known: HashSet<String>, group: Option<Vec<String>>, cx: &mut Context<Self>) {
        let Some(api) = self.api_for(&key.server) else { return; };
        let Some(list) = self.attachments.get_mut(&key) else { return; };
        let mut jobs = Vec::new();
        for attachment in list.iter_mut() {
            if matches!(attachment.state, AttachState::Uploaded(_)) { continue; }
            attachment.state = AttachState::Waiting;
            jobs.push((attachment.id, attachment.name.clone(), attachment.bytes.clone()));
        }
        self.uploading.insert(key.clone(), list.iter().map(|a| a.id).collect());
        let (connection, tx, uploads) = (self.connection, self.tx.clone(), self.uploads_for(&key));
        self.runtime.spawn(async move {
            let retention = match uploads { disk::Uploads::Local { .. } => disk::retention(&api).await, disk::Uploads::Remote => None };
            for (id, name, bytes) in jobs {
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::UploadStep(key.clone(), id, None) }).await;
                let result = uploads.upload(&api, &key.name, &name, bytes.to_vec(), retention).await;
                let failed = result.is_err();
                if tx.send(Envelope { connection, selection: None, payload: Payload::UploadStep(key.clone(), id, Some(result)) }).await.is_err() { return; }
                if failed { break; }
            }
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::UploadsDone(key, draft, steer, known, group) }).await;
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

    fn finish_uploads(&mut self, key: SessionKey, draft: String, steer: bool, known: HashSet<String>, group: Option<Vec<String>>, cx: &mut Context<Self>) {
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
        self.deliver(key, message, draft, steer, known, group, cx);
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

    fn receive_files(&mut self, key: SessionKey, owner: Option<SessionOwner>, generation: u64, files: Vec<Result<Picked, String>>, cx: &mut Context<Self>) {
        let current_generation = self.check_dictation_owner(cx);
        let attachment_key = self.delivery.current(key.clone());
        let mut problems = Vec::new();
        let mut audio_problems = Vec::new();
        for file in files {
            match file {
                Ok(picked) if composer::is_audio(&picked.name) => {
                    let result = if generation != current_generation || owner.is_none() || owner != self.dictation_owner(cx)
                        || self.composer_key().as_ref() != Some(&key) {
                        Err(tr("attach_audio_session_changed"))
                    } else { self.transcribe_file(&key, picked.name, picked.bytes, cx) };
                    if let Err(problem) = result { audio_problems.push(problem); }
                }
                file => {
                    if let Err(problem) = file.and_then(|picked| self.add_attachment(&attachment_key, picked.name, picked.bytes)) {
                        problems.push(problem);
                    }
                }
            }
        }
        if key == attachment_key { problems.extend(audio_problems); }
        else if !audio_problems.is_empty() { self.action_feedback.insert(key, (audio_problems.join(" "), true)); }
        if problems.is_empty() { self.action_feedback.remove(&attachment_key); }
        else { self.action_feedback.insert(attachment_key, (problems.join(" "), true)); }
    }

    // Leitura do disco fora da janela; tamanho conferido antes de ler.
    fn read_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(key) = self.composer_key() else { return; };
        if paths.is_empty() || self.uploading.contains_key(&key) { return; }
        let generation = self.check_dictation_owner(cx);
        let owner = self.dictation_owner(cx);
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let files = tokio::task::spawn_blocking(move || paths.into_iter().map(|path| {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "arquivo".into());
                let meta = std::fs::metadata(&path).map_err(|_| tr("attach_read_failed").replace("{name}", &name))?;
                if !meta.is_file() { return Err(tr("attach_read_failed").replace("{name}", &name)); }
                if meta.len() > api::MAX_BYTES { return Err(tr("attach_too_big_named").replace("{name}", &name)); }
                std::fs::read(&path).map(|bytes| Picked { name: name.clone(), bytes }).map_err(|_| tr("attach_read_failed").replace("{name}", &name))
            }).collect()).await.unwrap_or_default();
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Files(key, owner, generation, files) }).await;
        });
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.composer_key() else { return; };
        let prompt = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            let chosen = prompt.await;
            let Some(this) = this.upgrade() else { return; };
            this.update(cx, |this, cx| match chosen {
                Ok(Ok(Some(paths))) if this.composer_key().as_ref() == Some(&key) => this.read_paths(paths, cx),
                Ok(Ok(_)) => {}
                _ => { this.action_feedback.insert(key, (tr("picker_failed"), true)); cx.notify(); }
            });
        }).detach();
    }

    // Colar: imagem vira anexo e arquivo copiado vira anexo; texto segue para o campo.
    fn paste(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.composer_key() else { return false; };
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
        let Some(key) = self.composer_key() else { return; };
        if self.uploading.contains_key(&key) { return; }
        if let Some(list) = self.attachments.get_mut(&key) {
            let gone = list.iter().position(|a| a.id == id).and_then(|n| list.remove(n).image);
            if list.is_empty() { self.attachments.remove(&key); }
            if let Some(image) = gone { release_image(image, window, cx); }
        }
        cx.notify();
    }

    fn open_recent(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        if self.recent.as_ref().is_some_and(|recent| recent.key == key) { self.close_recent(); cx.notify(); return; }
        self.command_panel = false;
        self.close_controls();
        self.recent = Some(Recent { key: key.clone(), files: None });
        let (connection, tx, uploads) = (self.connection, self.tx.clone(), self.uploads_for(&key));
        self.runtime.spawn(async move {
            let result = uploads.list(&api, &key.name).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Recent(key, result) }).await;
        });
        cx.notify();
    }

    // Baixa de volta um anexo do cofre e o põe no campo como qualquer outro, sem citar caminho por presunção.
    fn reattach(&mut self, filename: String, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        let generation = self.check_dictation_owner(cx);
        let owner = self.dictation_owner(cx);
        self.close_recent();
        let (connection, tx, uploads) = (self.connection, self.tx.clone(), self.uploads_for(&key));
        self.runtime.spawn(async move {
            let result = uploads.fetch(&api, &key.name, &Source::Upload(filename.clone())).await
                .map(|bytes| Picked { name: filename.clone(), bytes })
                .map_err(|error| format!("{}: {}", filename, Self::fetch_failure(&error)));
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Files(key, owner, generation, vec![result]) }).await;
        });
        cx.notify();
    }

    /// Áudio da lista de recentes volta ao ditado lido do arquivo que o servidor já tem.
    fn dictate_recent(&mut self, filename: String, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return; };
        self.close_recent();
        match self.dictate_upload(filename, cx) {
            Ok(()) => { self.action_feedback.remove(&key); }
            Err(problem) => { self.action_feedback.insert(key, (problem, true)); }
        }
        cx.notify();
    }

    /// Toca um áudio da lista de recentes, lido de onde estão os anexos da sessão (disco desta máquina ou backend).
    fn play_recent(&mut self, filename: String, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        let (uploads, source) = (self.uploads_for(&key), Source::Upload(filename.clone()));
        self.toggle_audio(format!("recent:{filename}"), &filename, async move {
            uploads.fetch(&api, &key.name, &source).await.map_err(|error| Self::saved_audio_failure(&error))
        }, cx);
    }

    fn ensure_media(&mut self, source: &Source) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        let slot = (key.clone(), source.clone());
        if self.media.contains(&slot) { return; }
        self.media.start(slot);
        media::trace(format_args!("thumb start {source:?}"));
        let (connection, tx, source, uploads) = (self.connection, self.tx.clone(), source.clone(), self.uploads_for(&key));
        self.runtime.spawn(async move {
            let started = std::time::Instant::now();
            let result = match uploads.fetch(&api, &key.name, &source).await {
                Ok(bytes) => {
                    media::trace(format_args!("thumb fetched {source:?} {} B in {:.1} ms", bytes.len(), started.elapsed().as_secs_f64() * 1000.));
                    Ok(tokio::task::spawn_blocking(move || media::thumbnail(&bytes)).await.ok().flatten())
                }
                Err(error) => Err(error),
            };
            media::trace(format_args!("thumb ready {source:?} in {:.1} ms", started.elapsed().as_secs_f64() * 1000.));
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Media(key, source, result) }).await;
        });
    }

    // Abrir grava uma cópia privada e entrega ao programa do sistema; salvar pergunta o destino. Nunca há token em URL.
    fn keep_file(&mut self, source: Source, name: String, open: bool, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        let safe = composer::safe_name(&name);
        // Único ponto por onde os dois botões passam: só abre o que, com o nome gravado, é tipo passivo.
        if open && !composer::openable(&safe) {
            self.action_feedback.insert(key, (tr("open_refused").replace("{name}", &safe), true));
            cx.notify();
            return;
        }
        let (connection, tx, runtime, uploads) = (self.connection, self.tx.clone(), self.runtime.clone(), self.uploads_for(&key));
        let write = move |target: Option<PathBuf>| {
            runtime.spawn(async move {
                let result = async {
                    let bytes = uploads.fetch(&api, &key.name, &source).await.map_err(|error| format!("{safe}: {}", Self::fetch_failure(&error)))?;
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
        let (provider, headless) = self.provider();
        // O orquestrador não tem turno para parar: o Esc e o botão de parar não se oferecem.
        // Pergunta aberta tem turno esperando: interromper a cancela, como o Esc na TUI.
        provider != "orq" && self.chat_online
            && (self.chat.state.state == "working" || headless && self.chat.state.state == "awaiting_input" || self.chat.ask.is_some())
    }

    fn request_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection_dialog || !self.can_interrupt() { return; }
        if appearance::get().skip_chat_confirmations { self.interrupt(window, cx); return; }
        self.confirm = Some(Confirm::Stop);
        self.confirm_no_ask = false;
        cx.notify();
    }

    // Mensagem aceita que a conversa ainda não mostrou volta ao campo, e só então o backend limpa a entrada do terminal.
    fn interrupt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = None;
        if self.connection_dialog || !self.can_interrupt() { cx.notify(); return; }
        let (Some(api), Some(session)) = (self.session_api(), self.selected.as_ref()) else { return; };
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
        if protect && !current.trim().is_empty() && !composer::only_command(&current) {
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
        // Da lista em linha, só o `/nome` que é a mensagem toda roteia; no meio do texto a escolha só completa o nome.
        if !from_panel {
            let whole = self.composer_cursor(cx).and_then(|(text, cursor)| composer::slash_token(&text, cursor).map(|t| t.whole));
            if whole == Some(false) { self.complete_slash(&command.name, window, cx); return; }
        }
        if let Some(warning) = composer::blocked_command(&provider, &command, self.btw_ready()) {
            if let Some(key) = self.selected_key() {
                self.action_feedback.insert(key, (tr(warning).replace("{cmd}", &format!("/{}", command.name)), true));
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
        self.deliver(key, format!("/{}", command.name), draft, false, known, None, cx);
    }

    fn visible_suggestions(&self, cx: &App) -> Vec<CommandInfo> {
        let Some((text, cursor)) = self.composer_cursor(cx) else { return Vec::new(); };
        if self.suggest_dismissed.as_deref() == Some(text.as_str()) { return Vec::new(); }
        let Some(token) = composer::slash_token(&text, cursor) else { return Vec::new(); };
        composer::suggestions(self.command_list(), token.query).into_iter().cloned().collect()
    }

    /// Texto do campo e posição do cursor; com trecho selecionado não há cursor.
    fn composer_cursor(&self, cx: &App) -> Option<(String, usize)> {
        let input = self.composer.read(cx);
        let selected = input.selected_range();
        selected.is_empty().then(|| (input.value().to_string(), selected.end))
    }

    /// Troca o `/nome` sob o cursor pelo comando escolhido: o resto da mensagem fica, e nada é enviado.
    fn complete_slash(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, cursor)) = self.composer_cursor(cx) else { return; };
        let Some(token) = composer::slash_token(&text, cursor) else { return; };
        let (range, insert) = composer::slash_replacement(&text, &token, name);
        self.replace_composer(range, insert, window, cx);
        cx.notify();
    }

    /// Troca um trecho do campo e devolve o foco a ele.
    pub(super) fn replace_composer(&mut self, range: std::ops::Range<usize>, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |input, cx| {
            input.set_selected_range(range, cx);
            input.replace(text, window, cx);
            input.focus(window, cx);
        });
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
        self.chat.state.state == "working" && self.queued_count() > 0 && (headless || matches!(provider, "codex" | "kimi" | "claude"))
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
            Action::Answer | Action::Skip => self.chat.ask.as_ref().map(|ask| ask.fingerprint.clone()),
            Action::Select(_) | Action::Submit => Some(select_snapshot(&self.chat.state)),
            Action::Implement => self.codex_plan().map(|(_, plan)| plan),
            Action::Discard(id) => self.chat.events.iter().any(|e| e.id == format!("queued-{id}") && e.desistiu == Some(true)).then(|| id.clone()),
            Action::Cancel | Action::Steer => Some(String::new()),
        }
    }

    fn answer_body(&self, cx: &Context<Self>) -> Option<Value> {
        let ask = self.chat.ask.as_ref()?;
        if self.ask_form.fingerprint != ask.fingerprint { return None; }
        interaction::answer_body(ask, &self.ask_picks(cx))
    }

    /// Escolhas do formulário com o texto digitado no lugar de quem está digitando.
    fn ask_picks(&self, cx: &Context<Self>) -> Vec<Pick> {
        self.ask_form.picks.iter().enumerate().map(|(i, pick)| {
            if self.ask_form.typing.get(i) == Some(&true) {
                Pick::Text(self.ask_form.inputs.get(i).map(|input| input.read(cx).value().to_string()).unwrap_or_default())
            } else { pick.clone() }
        }).collect()
    }

    // O instantâneo é o pedido que a pessoa viu ao clicar; se o atual difere, nada sai.
    fn act(&mut self, action: Action, snapshot: String, cx: &mut Context<Self>) {
        if self.connection_dialog || !self.chat_online || !self.history_installed { return; }
        let (Some(api), Some(session), Some(key)) = (self.session_api(), self.selected.clone(), self.selected_key()) else { return; };
        if self.current_snapshot(&action).as_deref() != Some(snapshot.as_str()) {
            self.action_feedback.insert(key, (tr("request_changed"), true));
            cx.notify();
            return;
        }
        let body = match &action {
            Action::Answer => match self.answer_body(cx) { Some(body) => Some(body), None => return },
            Action::Skip => match self.chat.ask.as_ref().and_then(|ask| ask.payload.request_id.clone()) {
                Some(id) => Some(json!({"request_id": id})), None => return },
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
                if current && self.chat_auth_lost(&error) { self.open_connection(window, cx); }
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
                Action::Skip => {
                    if current && self.chat.ask.as_ref().is_some_and(|ask| ask.fingerprint == snapshot) {
                        self.chat.ask = None;
                        self.ask_form = AskForm::default();
                    }
                    (tr("ask_skipped"), false)
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
        let stable = self.chat.take_unsynced();
        let delta = self.conversation.update(&self.chat.events, stable, conversation::View {
            thinking: a.thinking_tools, tasks: a.task_list,
            merge_thinking: a.tool_look == appearance::ToolLook::Tree,
            every_run_groups: a.tool_look == appearance::ToolLook::Terminal,
        });
        if delta.activity_changed { self.sync_activity(cx); }
        self.orq_days = if self.selected.as_ref().is_some_and(SessionInfo::orq) { orq_timeline::day_starts(self.chat.events.iter().filter(|event| event.orq.is_some())) } else { HashSet::new() };
        api::open_trace(|| format!("sync_rows built {} items, {} rows unchanged", self.conversation.items.len(), delta.from));
        self.sync_row_ids(Some(delta), cx);
        api::open_trace(|| format!("sync_rows prepared {} rows", self.row_ids.len()));
        self.check_cites(delta.stable_events, cx);
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
        let full = self.row_ids.len() < self.conversation.items.len();
        if full { self.sync_rows(cx) } else { self.sync_row_ids(None, cx) }
    }

    /// Reconstrói somente o sufixo invalidado; as linhas anteriores conservam vetores e caches.
    fn sync_row_ids(&mut self, rebuilt: Option<conversation::incremental::Delta>, cx: &mut Context<Self>) {
        let items = self.conversation.items.len();
        let from = rebuilt.map_or(items, |delta| delta.from).min(self.row_ids.len());
        let stable = rebuilt.map_or(self.chat.events.len(), |delta| delta.stable_events);
        let opening = self.opening_row_shown();
        let events = &self.chat.events;
        let (mut ids, mut signatures): (Vec<String>, Vec<String>) = self.conversation.items[from..].iter().map(|item| {
            let id = item.id(events);
            let day = matches!(item, Item::Event(_)) && self.orq_days.contains(&id);
            let signature = if day { format!("day{}", signature(item, events)) } else { signature(item, events) };
            (id, signature)
        }).unzip();
        if !self.chat.live_thinking.is_empty() { ids.push(LIVE_THINKING.into()); signatures.push(String::new()); }
        if let Some(tool) = &self.chat.live_tool { ids.push(LIVE_TOOL.into()); signatures.push(format!("{}{}", tool.name, tool.input)); }
        if !self.visible_preview.text.is_empty() { ids.push(PREVIEW.into()); signatures.push(String::new()); }
        if opening { ids.push(landing::OPENING.into()); signatures.push(String::new()); }
        if self.working_row_shown() { ids.push(WORKING.into()); signatures.push(String::new()); }
        for agent in self.conversation.running_agents() { ids.push(format!("{PINNED}{}", events[agent.call].id)); signatures.push(String::new()); }
        let patch = row_patch::RowPatch::between(from, &self.row_ids[from..], &self.row_signatures[from..],
            &ids, &signatures, if rebuilt.is_some() { items - from } else { 0 });
        let prefix = patch.remove.start;
        let added = patch.insert.len();
        let spliced = !patch.remove.is_empty() || added > 0;
        let entering = prefix > 0 && patch.remove.is_empty() && (1..=4).contains(&added);
        let mut old_parts = HashSet::new();
        if rebuilt.is_some() {
            for assets in self.row_assets.drain(from..) {
                for key in assets.prepared { self.prepared.remove(&key); }
                for id in assets.parts { self.tree_parts.remove(&id); old_parts.insert(id); }
            }
            for item in &self.conversation.items[from..] {
                if let Item::Event(i) = item {
                    self.prepared.insert(events[*i].id.clone(), prepare_message(&events[*i], &self.cites.dead));
                }
                self.row_assets.push(row_patch::RowAssets::of(item, events, &self.conversation.paired));
            }
            if rebuilt.is_some_and(|delta| delta.full) { self.last_message = None; }
            if let Some(i) = events[stable..].iter().rposition(|e| e.kind == "assistant_msg" || e.kind == "user_msg" && !e.queued()) {
                self.last_message = Some(stable + i);
            }
        }
        let rewritten: Vec<(usize, String)> = ids.iter().enumerate().filter_map(|(i, id)| {
            let cached = self.rich.get(id)?;
            if id == PREVIEW {
                let body = preview_source(&self.visible_preview);
                return (cached.source != body).then_some((from + i, body));
            }
            let Prepared::Message { markdown, .. } = self.prepared.get(id)? else { return None };
            (cached.source != *markdown).then(|| (from + i, markdown.clone()))
        }).collect();
        if rebuilt.is_some() {
            self.prepare_tools_from(from);
            self.sync_tables(appearance::get().table_chart, from);
        }
        // Só linha que muda de altura puxa a mola: um quadro sem mudança não pode desgrudar a lista do fim.
        if spliced || !patch.resized.is_empty() || !rewritten.is_empty() { self.follow_content_changed(cx); }
        if spliced { self.splice_rows(patch.remove.clone(), &ids[patch.insert.clone()]); }
        // Só o bloco novo entra animado: um acréscimo pequeno no meio ou no fim de uma conversa já aberta. Troca de linha (a
        // prévia virando a resposta gravada, o envio virando a mensagem real), histórico carregando ou páginas antigas
        // chegando em cima aparecem direto.
        if entering {
            let now = Instant::now();
            for id in &ids[patch.insert.clone()] { if id != WORKING { self.arrived.insert(id.clone(), now); } }
        }
        // Parte nova num grupo da Árvore entra animada pela mesma regra: conversa já na tela e poucas de uma vez, uma
        // depois da outra.
        if rebuilt.is_some() {
            let parts: Vec<&String> = self.row_assets[from..].iter().flat_map(|assets| assets.parts.iter()).collect();
            let fresh: Vec<&String> = parts.iter().copied().filter(|id| !old_parts.contains(*id) && !self.tree_parts.contains(*id)).collect();
            if prefix > 0 && (1..=4).contains(&fresh.len()) {
                let now = Instant::now();
                for (n, id) in fresh.into_iter().enumerate() { self.part_arrived.insert(id.clone(), now + motion::TOOL_STAGGER * n as u32); }
            }
            self.tree_parts.extend(parts.into_iter().cloned());
            for id in old_parts { if !self.tree_parts.contains(&id) { self.part_arrived.remove(&id); } }
        }
        for index in patch.resized { self.list_state.remeasure_items(index..index + 1); }
        for (index, body) in rewritten {
            let id = &ids[index - from];
            let Some(cached) = self.rich.get_mut(id) else { continue };
            let added = if id == PREVIEW { body.strip_prefix(&cached.source).map(str::to_owned) } else { None };
            cached.source = body.clone();
            if let Some(added) = added {
                cached.view.update(cx, |view, cx| view.push_str(&added, cx));
            } else {
                cached.view.update(cx, |view, cx| view.set_text(&body, cx));
            }
            self.list_state.remeasure_items(index..index+1);
        }
        let kept: HashSet<_> = ids.iter().collect();
        let removed: HashSet<_> = self.row_ids[from..].iter().filter(|id| !kept.contains(id)).cloned().collect();
        // Visões fora da lista (plano, diff do painel) usam linha "__…__" e saem só pelo limite do cache.
        self.rich.retain(|_, rich| !removed.contains(&rich.row) || rich.row.starts_with("__"));
        self.pages.retain_rows(|row| !removed.contains(row));
        for id in removed {
            self.arrived.remove(&id);
            self.tree_folds.remove(&id);
            self.tables.remove(&id);
        }
        self.row_ids.truncate(from);
        self.row_ids.extend(ids);
        self.row_signatures.truncate(from);
        self.row_signatures.extend(signatures);
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
    /// Confere no servidor, em lote, os arquivos citados nas mensagens ainda não conferidos; os que não abrem perdem o
    /// chip, e as mensagens são preparadas de novo. Falha na conferência deixa os chips como estão.
    fn check_cites(&mut self, mut stable: usize, cx: &mut Context<Self>) {
        let owner = self.selected_key();
        if self.cites.owner != owner { self.cites = CiteCheck { owner: owner.clone(), ..Default::default() }; stable = 0; }
        let (Some(owner), Some(api)) = (owner, self.session_api()) else { return };
        let mut fresh = Vec::new();
        let mut revived = false;
        for event in &self.chat.events[stable..] {
            if !(event.kind == "assistant_msg" || peer_of(event).is_some()) || !self.cites.scanned.insert(event.id.clone()) { continue; }
            for path in composer::citation_paths(&composer::citation_markdown(&display_body(event))) {
                // Mensagem nova citando um que não abria ("vou criar x.rs" e depois "criei x.rs"): confere de novo.
                if self.cites.dead.remove(&path) { revived = true; fresh.push(path); }
                else if self.cites.checked.insert(path.clone()) { fresh.push(path); }
            }
        }
        if revived { self.refresh_cites(cx); }
        if fresh.is_empty() { return; }
        let job = self.runtime.spawn(async move {
            // Cada lote vale sozinho: a falha de um não joga fora o que os outros já responderam.
            let (mut dead, mut failed, mut error) = (Vec::new(), Vec::new(), None);
            // ponytail: lotes de 50; o backend procura fora da pasta no máximo 30 por pedido, relendo o transcript.
            for batch in fresh.chunks(50) {
                let answer = api.act(&owner.name, &["files", "resolver"], Some(json!({"caminhos": batch})), false, 30).await
                    .and_then(|value| value.get("faltam").and_then(Value::as_array).cloned().ok_or_else(|| Failure::local("invalid_response")));
                match answer {
                    Ok(missing) => dead.extend(missing.iter().filter_map(Value::as_str).map(str::to_owned)),
                    Err(e) => { failed.extend_from_slice(batch); error = Some(e); }
                }
            }
            (owner, dead, failed, error)
        });
        cx.spawn(async move |this, cx| {
            let Ok((owner, dead, failed, error)) = job.await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.cites.owner.as_ref() != Some(&owner) { return; }
                // Falhou: sai do conferido e entra no próximo `sync_rows`. O chip fica clicável até lá.
                if let Some(error) = error {
                    eprintln!("conferir {} arquivos citados falhou: {}", failed.len(), Self::fetch_failure(&error));
                    for path in &failed { this.cites.checked.remove(path); }
                }
                if !dead.is_empty() {
                    this.cites.dead.extend(dead);
                    // A conversa pode ter sido zerada enquanto a conferência rodava: os itens só valem refeitos agora,
                    // e o texto já preparado tem os chips antigos.
                    this.chat.invalidate();
                    this.sync_rows(cx);
                    cx.notify();
                }
            });
        }).detach();
    }

    /// Mensagens preparadas de novo depois que o conjunto de chips mortos mudou.
    fn refresh_cites(&mut self, cx: &mut Context<Self>) {
        self.sync_row_ids(Some(conversation::incremental::Delta {
            from: 0, previous_len: self.conversation.items.len(), stable_events: 0, full: true, activity_changed: false,
        }), cx);
        cx.notify();
    }

    fn sync_tables(&mut self, enabled: bool, from: usize) {
        if !enabled { self.tables.clear(); return; }
        let decimal = tr("decimal").chars().next().unwrap_or(',');
        let events = &self.chat.events;
        for item in &self.conversation.items[from..] {
            let Item::Event(i) = item else { self.tables.remove(&item.id(events)); continue };
            let event = &events[*i];
            if event.kind != "assistant_msg" || event.is_error == Some(true) { self.tables.remove(&event.id); continue; }
            let source = safe_markdown(&composer::citation_markdown_with(&display_body(event), &|p| self.cites.dead.contains(p)));
            if self.tables.get(&event.id).is_some_and(|(seen, _)| seen == &source) { continue; }
            let tables = crate::tables::read(&source, decimal).into();
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
        api::open_trace(|| format!("markdown start {row} {} bytes", source.len()));
        let observer = cx.observe(&view, move |this, _, cx| {
            api::open_trace(|| format!("markdown parsed {owner}"));
            if let Some(i) = this.row_ids.iter().position(|id| id == &owner) { this.follow_content_changed(cx); this.list_state.remeasure_items(i..i+1); }
        });
        self.rich.insert(key.to_owned(), RichText { source, view: view.clone(), _observer: observer, touched: self.render_tick, row: row.to_owned() });
        view
    }

    fn render_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(id) = self.row_ids.get(index).cloned() else { return div().into_any_element(); };
        self.tree_motion = false;
        let inner = match (id.as_str(), self.conversation.items.get(index).cloned()) {
            (PREVIEW, _) => self.render_message(index, &id, cx),
            (LIVE_THINKING, _) => self.render_live_thinking(cx),
            (LIVE_TOOL, _) => self.render_live_tool(),
            (WORKING, _) => self.render_working(cx),
            (landing::OPENING, _) => self.render_opening_bubble(cx),
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
        if self.tree_motion { motion::request_frame(window, cx); }
        let message = id == PREVIEW || id == landing::OPENING || matches!(self.conversation.items.get(index), Some(Item::Event(_)));
        let row = row_frame(inner, message);
        // Pede quadro só para a área da conversa, e só enquanto a linha entra; rolar até ela depois não a anima de novo.
        let entering = self.arrived.get(&id).map(|at| motion::FADE_IN.raw(*at)).filter(|raw| *raw < 1. && !cx.reduce_motion());
        let row = match entering {
            Some(raw) => { motion::request_frame(window, cx); motion::fade_in(row, motion::FADE_IN.ease(raw)) }
            None => { self.arrived.remove(&id); row }
        };
        row.id(SharedString::from(id)).into_any_element()
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
        self.conversation.pinned.contains(&call) || self.chat.state.state == "working" && self.last_message.is_none_or(|last| call > last)
    }

    fn pinned_call(&self, event_id: &str) -> Option<usize> {
        self.conversation.running_agents().map(|agent| agent.call).find(|&call| self.chat.events[call].id == event_id)
    }

    /// A linha de trabalhando fica sob a última linha durante todo o turno, com pensamento, ferramenta ou texto chegando,
    /// e já no envio.
    fn working_row_shown(&self) -> bool { self.chat.state.state == "working" || self.sending_shown() || self.chat.turn_done.is_some() }

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
                self.splice_rows(at..at + 1, &[]);
                self.row_ids.remove(at);
                self.row_signatures.remove(at);
            }
            None => {
                let at = self.row_ids.iter().position(|id| id.starts_with(PINNED)).unwrap_or(self.row_ids.len());
                self.splice_rows(at..at, &[WORKING.into()]);
                self.row_ids.insert(at, WORKING.into());
                self.row_signatures.insert(at, String::new());
            }
        }
    }

    /// Marca, verbo e segundos numa linha só, de altura fixa: o texto que muda não remede a lista. Marca e segundos
    /// animam fora da conversa guardada (`working_mark_float`); aqui ficam só os lugares deles.
    fn render_working(&self, cx: &mut Context<Self>) -> AnyElement {
        let sending = self.sending_shown();
        // Mesma caixa nos dois estados: trocar de um para o outro não muda a altura da linha.
        let line = || div().relative().h(px(38.)).flex().items_center().gap(px(8.));
        // Turno acabado: a mesma linha, parada, com quanto durou e quando terminou, como o "Worked for" do Claude Code.
        if let Some(text) = self.chat.turn_done.clone().filter(|_| !sending) {
            return line()
                .child(div().w(px(14.)).flex_none().flex().justify_center().text_size(px(13.)).text_color(theme::faint()).child("✻"))
                .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme::faint()).child(text))
                .into_any_element();
        }
        let verb = if sending { tr("sending") } else { working_verb(self.chat.state.label.as_deref()) };
        let since = if sending { None } else { self.turn_start() };
        let tokens = if sending { None } else { working_tokens(self.chat.state.label.as_deref()).map(SharedString::from) };
        // Sem recuo: a marca começa na borda da coluna, alinhada com o texto das mensagens.
        let row = line()
            .child(self.working_mark_slot(panes::Area::Conversation, "working-line", 14., theme::accent()))
            .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(verb))
            .map(|el| match since {
                Some(since) => el.child(self.elapsed_slot(panes::Area::Conversation, "working-elapsed", since, tokens)),
                // Sem começo conhecido não há segundos, mas os tokens do terminal valem sozinhos.
                None => el.when_some(tokens, |el, tokens| el.child(div().min_w_0().truncate().pt(px(1.)).text_size(px(11.)).text_color(theme::faint()).child(tokens))),
            });
        if cx.reduce_motion() { return row.into_any_element(); }
        row.with_animation("working-line-in", Animation::new(WORKING_FADE).with_easing(motion::ease_out),
            |el, t| el.opacity(t).top(px(6. * (1. - t)))).into_any_element()
    }

    /// "Ir para o fim" flutuando no pé da conversa, centrada na coluna, só enquanto ela está solta e longe do fim.
    fn render_jump_pill(&self, cx: &mut Context<Self>) -> AnyElement {
        // O hover usa o mesmo material da pílula; o `hover()` do tema é translúcido na caixa solta.
        let glass = appearance::get().surface_material == appearance::SurfaceMaterial::Glass;
        let fill = theme::popup_fill(theme::elevated());
        let hover = fill.blend(theme::text().alpha(0.06));
        let pill = Button::new("jump-latest")
            .custom(ButtonCustomVariant::new(cx).color(fill).foreground(theme::text()).hover(hover).active(hover))
            // `elevated`, um degrau acima da conversa: com `raised` a pílula sumia no fundo.
            .bg(fill).h(px(30.)).pl(px(11.)).pr(px(13.)).rounded(px(15.)).border_1().border_color(theme::border_strong())
            .shadow(theme::popover_shadow()).text_size(px(13.))
            .icon(Icon::new(IconName::ArrowDown).size(px(13.)).text_color(theme::faint())).label(tr("latest"))
            .on_click(cx.listener(|this, _, _, cx| this.follow_engage(cx)));
        let pill = if glass { chrome::Glass::new(pill, px(15.)).into_any_element() } else { pill.into_any_element() };
        let wrap = div().absolute().left_0().right_0().bottom(px(16.)).flex().justify_center().child(pill);
        if cx.reduce_motion() { return wrap.into_any_element(); }
        wrap.with_animation("jump-latest-in", Animation::new(WORKING_FADE).with_easing(motion::ease_out),
            |el, t| el.opacity(t).bottom(px(10. + 6. * t))).into_any_element()
    }

    fn render_tool(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> AnyElement {
        if let Some(card) = self.render_agent_card(tool, cx) { return card; }
        if appearance::get().tool_look == appearance::ToolLook::Chips { return self.render_single_chip(tool, row, cx); }
        if appearance::get().tool_look == appearance::ToolLook::Terminal { return self.render_terminal_tool(tool, row, cx); }
        let call = &self.chat.events[tool.call];
        let key = call.id.clone();
        let name = call.tool_name.clone().unwrap_or_else(|| tr("tool"));
        let summary = conversation::summarize_input(call.tool_name.as_deref(), call.tool_input.as_ref());
        let (status, status_color) = self.tool_status(tool);
        let error = tool.result.is_some_and(|i| self.chat.events[i].is_error == Some(true));
        let open = self.expanded.contains(&key);
        // Edição pronta mostra "+5 −2" no lugar do "pronto"; rodando ou com erro, o estado vale mais.
        let edit_totals = (!error && tool.result.is_some()).then(|| edits::totals(call)).flatten();
        let toggle_key = key.clone();
        let header = self.disclosure(&key, open)
            .accessibility_label(format!("{name}: {summary}. {status}"))
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::text() }).child(name))
            .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(summary))
            .map(|el| match edit_totals {
                Some(totals) => el.child(totals),
                None => el.child(div().flex_shrink_0().max_w(px(320.)).truncate().text_color(status_color).child(status)),
            })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
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
        // Edição de arquivo mostra o diff no lugar da entrada crua; o resultado só aparece se falhou.
        let result = tool.result.map(|i| &self.chat.events[i]);
        let diff = edits::card(call, result, cx);
        let has_diff = diff.is_some();
        let mut body = div().flex().flex_col().gap_2().pt_1().pb_2();
        // O arquivo que a ferramenta leu ou mandou: a pele Terminal mostra o do SendUserFile fora do corpo, sempre à vista.
        let shown_outside = appearance::get().tool_look == appearance::ToolLook::Terminal && sends_files(call);
        let refs = if shown_outside { Vec::new() } else { tool_file_refs(call) };
        if !refs.is_empty() { body = body.child(self.render_refs(&format!("{row}-read"), refs, cx)); }
        if let Some(diff) = diff { body = body.child(diff); }
        else if matches!(input, Prepared::Detail { total, .. } if total > 0) {
            body = body.child(self.detail(row, &input_key, input, tr("tool_input"), tr("copy_input"), false, cx));
        }
        match tool.result {
            Some(_) if has_diff && !error => body,
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
    fn prepare_tools(&mut self) { self.prepare_tools_from(0); }

    fn prepare_tools_from(&mut self, from: usize) {
        let (events, prepared, expanded) = (&self.chat.events, &mut self.prepared, &self.expanded);
        let mut tools = Vec::new();
        let mut orphans = Vec::new();
        for item in &self.conversation.items[from..] {
            match item {
                Item::Tool(tool) => tools.push(*tool),
                // O raciocínio que a Árvore põe no grupo não tem entrada nem resultado a preparar.
                Item::Group { tools: group, .. } => tools.extend(group.iter().copied().filter(|t| events[t.call].kind != "thinking")),
                Item::Thinking { parts, .. } => tools.extend(parts.iter().filter(|&&i| events[i].kind != "thinking")
                    .map(|&i| Tool { call: i, result: self.conversation.paired.get(&i).copied() })),
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
        if appearance::get().tool_look == appearance::ToolLook::Terminal { return self.render_terminal_group(row, tools, cx); }
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
            let paired: HashMap<usize, usize> = if has_calls { parts.iter().filter_map(|&i| self.conversation.paired.get(&i).map(|&r| (i, r))).collect() } else { HashMap::new() };
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
        let summary = conversation::summarize_input(Some(&tool.name), tool.input.as_object());
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
        // Várias perguntas: uma por aba, com marca nas respondidas; uma só dispensa a faixa.
        let tab = self.ask_form.tab.min(total.saturating_sub(1));
        let tabs = (total > 1).then(|| {
            let picks = self.ask_picks(cx);
            let tabs = ask.payload.questions.iter().enumerate().map(|(qi, item)| {
                let done = picks.get(qi).is_some_and(|pick| interaction::answer(&ask, item, pick).is_some());
                let label = if item.header.is_empty() { tr("ask_tab").replace("{n}", &(qi + 1).to_string()) } else { item.header.clone() };
                Tab::new().label(label).when(done, |tab| tab.prefix(chrome::small_icon(IconName::Check, 12., theme::success())))
            }).collect::<Vec<_>>();
            let fp = fingerprint.clone();
            TabBar::new("ask-tabs").underline().small().selected_index(tab).children(tabs)
                .on_click(cx.listener(move |this, index: &usize, _, cx| {
                    if this.ask_form.fingerprint == fp { this.ask_form.tab = *index; cx.notify(); }
                }))
        });
        let mut body = div().flex().flex_col().gap_4();
        for (qi, item) in ask.payload.questions.iter().enumerate().filter(|(qi, _)| *qi == tab) {
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
                options = options.child(if item.multi_select {
                    ask_option(Checkbox::new(id).accessibility_label(option.label.clone()).checked(chosen(oi)).disabled(busy).on_click(on_pick), option, chosen(oi), busy).into_any_element()
                } else {
                    ask_option(Radio::new(id).accessibility_label(option.label.clone()).checked(chosen(oi)).disabled(busy).on_click(on_pick), option, chosen(oi), busy).into_any_element()
                });
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
                .when(total == 1 && !item.header.is_empty(), |el| el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted())
                    .child(item.header.clone())))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(item.question.clone()))
                .when(item.multi_select, |el| el.child(div().text_xs().text_color(theme::muted()).child(tr("ask_multi"))))
                .child(options)
                .when_some(input, |el, input| el.child(Input::new(&input).disabled(busy)))
                .when(item.is_secret, |el| el.child(div().text_xs().text_color(theme::muted()).child(tr("ask_secret"))))
                .child(escapes));
        }
        let ready = self.answer_body(cx).is_some();
        // Cancelar sempre existe: pergunta assíncrona do Codex só se dispensa (Skip); as demais interrompem o turno.
        let cancel = {
            let (action, fp) = if ask.payload.is_async { (Action::Skip, fingerprint.clone()) } else { (Action::Cancel, String::new()) };
            Button::new("ask-cancel").ghost().label(tr("cancel")).disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.act(action.clone(), fp.clone(), cx)))
        };
        let fp = fingerprint.clone();
        let sending = busy && self.selected_key().and_then(|key| self.flight.running(&key).cloned()) == Some(Action::Answer);
        let scroll_key = format!("{fingerprint}#{tab}");
        if self.ask_scroll.0 != scroll_key { self.ask_scroll = (scroll_key, ScrollHandle::new()); }
        let body = div().flex().flex_col().gap_3().when_some(tabs, |el, tabs| el.child(tabs))
            .child(scrolled("ask-scroll", &self.ask_scroll.1, 360., body)).into_any_element();
        Some(self.interaction_card(tr("ask_title"), body,
            div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().text_xs().text_color(theme::muted()).child(tr(if ready { "ask_ready" } else { "ask_incomplete" })))
                .child(cancel)
                .child(if tab + 1 < total {
                    // Troca de aba só pelo botão ou pela faixa: pular sozinho no clique desorienta.
                    Button::new("ask-next").primary().label(tr("ask_next")).disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| if this.ask_form.fingerprint == fp { this.ask_form.tab = tab + 1; cx.notify(); }))
                } else {
                    Button::new("ask-send").primary().label(tr(if sending { "sending" } else { "ask_send" })).disabled(busy || !ready)
                        .on_click(cx.listener(move |this, _, _, cx| this.act(Action::Answer, fp.clone(), cx)))
                })
                .into_any_element()))
    }

    fn render_options(&mut self, busy: bool, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.sync_plan_review(window, cx);
        if self.plan_review_available() { return self.render_plan_review_card(false, busy, window, cx); }
        let state = &self.chat.state;
        if state.claude_plan_pending.is_some() { return None; }
        if self.chat.ask.is_some() || state.state != "awaiting_input" { return None; }
        let (question, options) = (state.question.clone()?, state.options.clone().filter(|o| !o.is_empty())?);
        // Menu do AskUserQuestion no pane: quem responde é o card nativo (depois de enviar, este seletor piscava por cima).
        if self.chat.ask_pane() { return None; }
        let snapshot = select_snapshot(state);
        let plan = plan_pending(state).filter(|p| !p.plan.trim().is_empty());
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
        // URL do texto abre fora do app; o texto da pergunta segue puro.
        let links = interaction::question_links(&question);
        body = body.child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(question));
        for (i, url) in links.into_iter().enumerate() {
            body = body.child(Button::new(SharedString::from(format!("question-link-{i}"))).small().ghost().label(url.clone())
                .on_click(move |_, _, cx| cx.open_url(&url)));
        }
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
        // Pedido que espera você: mesmo material do compositor (legível sobre papel de parede e vidro), com a
        // moldura na cor escolhida em Aparência (destaque ou âmbar).
        in_column(div().p(px(14.)).rounded(px(14.)).border_1().border_color(theme::ask_highlight().opacity(0.55))
                .bg(theme::boxed()).shadow(theme::card_shadow()).flex().flex_col().gap(px(10.))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(body).child(footer)).py_2()
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
            let player = crate::audio::is_audio(&name).then(|| {
                let (audio_key, play_source, play_name) = (format!("ref:{row}:{n}"), source.clone(), name.clone());
                self.audio_controls(&audio_key.clone(), move |this, cx| {
                    let (Some(api), Some(key)) = (this.session_api(), this.selected_key()) else { return };
                    let (uploads, source) = (this.uploads_for(&key), play_source.clone());
                    this.toggle_audio(audio_key.clone(), &play_name, async move {
                        uploads.fetch(&api, &key.name, &source).await.map_err(|error| Self::fetch_failure(&error))
                    }, cx);
                }, cx)
            });
            let (save_source, save_name) = (source, name.clone());
            let openable = composer::openable(&composer::safe_name(&name));
            list = list.child(div().flex().flex_col().gap_1()
                .when_some(preview, |el, preview| el.child(preview))
                .when_some(player, |el, player| el.child(player))
                .child(div().flex().items_center().gap_2().text_sm()
                    .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(name))
                    .when(openable, |el| el.child(Button::new(SharedString::from(format!("open-{row}-{n}"))).ghost().xsmall().label(tr("open"))
                        .on_click(cx.listener(move |this, _, _, cx| this.keep_file(open_source.clone(), open_name.clone(), true, cx)))))
                    .child(Button::new(SharedString::from(format!("save-{row}-{n}"))).ghost().xsmall().label(tr("save"))
                        .on_click(cx.listener(move |this, _, _, cx| this.keep_file(save_source.clone(), save_name.clone(), false, cx))))));
        }
        list.into_any_element()
    }

    /// Miniaturas da bolha do usuário (`.thumb-row` do `UserBubble` web): 96 px, 80 quando há várias. Abrir e salvar
    /// ficam no visor, que o clique abre.
    fn render_thumbs(&mut self, row: &str, images: Vec<Source>, cx: &mut Context<Self>) -> Option<AnyElement> {
        if images.is_empty() { return None; }
        let key = self.selected_key();
        let side = px(if images.len() > 1 { 80. } else { 96. });
        let label = activity::web("anexos_ver_original");
        let mut tiles = Vec::new();
        for (n, source) in images.iter().enumerate() {
            self.ensure_media(source);
            let state = key.as_ref().and_then(|key| self.media.get(&(key.clone(), source.clone())));
            let inner = match state {
                // O id guarda o quadro corrente: sem ele a GPUI não anima o GIF.
                Some(MediaState::Image(picture)) => img(picture.clone()).size_full().object_fit(ObjectFit::Cover)
                    .id(SharedString::from(format!("thumb-image-{row}-{n}"))).into_any_element(),
                Some(MediaState::Failed(reason)) => div().size_full().p_1().overflow_hidden().text_size(px(10.)).text_color(theme::warning())
                    .child(reason.clone()).into_any_element(),
                _ => div().into_any_element(),
            };
            let (open_key, sources) = (key.clone(), images.clone());
            tiles.push(div().id(SharedString::from(format!("thumb-{row}-{n}"))).focusable().tab_stop(true).cursor_pointer()
                .role(Role::Button).aria_label(label.clone())
                .size(side).flex_shrink_0().rounded_md().overflow_hidden().bg(theme::raised()).border_1().border_color(theme::border())
                .focus_visible(|el| el.border_color(theme::accent_focus()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(key) = open_key.clone() { this.open_image(key, sources.clone(), n, window, cx); }
                }))
                .child(inner));
        }
        Some(div().flex().flex_wrap().gap_1().children(tiles).into_any_element())
    }

    /// Chip "de: X" do recado (`.peer-head` do `UserBubble` web): leva ao chat do remetente quando ele está na lista;
    /// recado do painel não tem sessão para abrir.
    fn peer_head(&self, row: &str, peer: cards::PeerMessage, cx: &mut Context<Self>) -> AnyElement {
        let name = HashMap::from([("n".to_owned(), peer.from.clone())]);
        let key = match peer.scope { cards::PeerScope::Peer => "board_peer_de", cards::PeerScope::Group => "board_peer_grupo", cards::PeerScope::Panel => "board_peer_painel" };
        let label = crate::i18n::tr_web(key, &name).unwrap_or_else(|| peer.from.clone());
        // O remetente é da máquina da conversa aberta, não da ativa.
        let server = self.open_server();
        let target = self.sessions_of(&server).iter().any(|s| s.name == peer.from).then(|| sidebar::Target::new(&server, &peer.from))
            .filter(|_| peer.scope != cards::PeerScope::Panel);
        let chip = div().id(SharedString::from(format!("peer-{row}"))).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(peer_tint(peer.scope));
        let chip = match target {
            Some(target) => {
                let open = crate::i18n::tr_web("user_abrir_chat_de", &name).unwrap_or_default();
                chip.child(format!("{label} ›")).cursor_pointer().focusable().tab_stop(true).role(Role::Button).aria_label(open.clone())
                    .hover(|el| el.underline()).focus_visible(|el| el.underline())
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(open.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| { this.select_target(&target, window, cx); }))
            }
            None => chip.child(label),
        };
        div().flex().flex_wrap().items_center().gap(px(6.)).child(chip)
            .when_some(peer.canal, |el, canal| el.child(div().px(px(7.)).py(px(1.)).rounded_full().bg(theme::raised())
                .text_size(px(9.5)).font_weight(FontWeight::BOLD).text_color(theme::muted()).child(canal.to_uppercase())))
            .into_any_element()
    }

    fn render_attachments(&self, key: &SessionKey, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list = self.attachments.get(key).filter(|list| !list.is_empty())?;
        let busy = self.uploading.contains_key(key);
        // O visor recebe só as imagens, na ordem da faixa: as setas dele passam de uma para a outra.
        let images: Vec<(u64, Source)> = list.iter().filter(|a| a.image.is_some())
            .map(|a| (a.id, Source::Memory(a.name.clone(), crate::api::Shared(a.bytes.clone())))).collect();
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
                    Some(picture) => {
                        let (key, sources) = (key.clone(), images.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>());
                        let index = images.iter().position(|(n, _)| *n == id).unwrap_or(0);
                        div().id(SharedString::from(format!("attachment-view-{id}"))).size_full().rounded(px(11.)).overflow_hidden().cursor_pointer()
                            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tr("attach_view")).build(window, cx))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_image(key.clone(), sources.clone(), index, window, cx)))
                            .child(img(picture).size_full().object_fit(ObjectFit::Cover)).into_any_element()
                    }
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
                if composer::is_audio(&name) {
                    // Áudio não volta ao campo como anexo: toca aqui ou volta ao ditado, lido do que o servidor já tem.
                    // O player fica fora do `popup::row`: a linha é um botão e o play também a dispararia.
                    let (play, dictate) = (name.clone(), name.clone());
                    return div().id(SharedString::from(format!("recent-{n}"))).px(px(8.)).py(px(4.)).flex().flex_col().gap_1()
                        .child(div().flex().items_center().gap_2()
                            .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                            .child(div().flex_shrink_0().text_xs().text_color(theme::muted()).child(human_size(file.size)))
                            .child(Button::new(SharedString::from(format!("recent-dictate-{n}"))).ghost().xsmall()
                                .label(tr("dictation_again")).accessibility_label(format!("{}: {name}", tr("dictation_again")))
                                .on_click(cx.listener(move |this, _, _, cx| this.dictate_recent(dictate.clone(), cx)))))
                        .child(self.audio_controls(&format!("recent:{name}"), move |this, cx| this.play_recent(play.clone(), cx), cx))
                        .into_any_element();
                }
                popup::row(SharedString::from(format!("recent-{n}")), false)
                    .child(div().flex_1().min_w_0().truncate().child(file.filename.clone()))
                    .child(div().flex_shrink_0().text_xs().text_color(theme::muted()).child(human_size(file.size)))
                    .on_click(cx.listener(move |this, _, _, cx| this.reattach(name.clone(), cx)))
                    .into_any_element()
            })).into_any_element(),
        };
        Some(div().p(px(popup::INSET)).rounded_md().bg(theme::popup_content_fill()).flex().flex_col().gap(px(2.))
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
        div().p(px(popup::INSET)).rounded_md().bg(theme::popup_content_fill()).flex().flex_col().gap(px(2.))
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
        let can_skip = matches!(&confirm, Confirm::Stop | Confirm::Destructive(_));
        Some(div().p_3().rounded_md().border_1().border_color(theme::warning()).flex().flex_col().gap_2()
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().text_sm().child(text))
                .child(Button::new("confirm-cancel").small().ghost().label(tr("cancel")).on_click(cx.listener(|this, _, window, cx| {
                    this.confirm = None;
                    this.confirm_no_ask = false;
                    this.composer.update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                })))
                .child(Button::new("confirm-ok").small().primary().label(action).on_click(cx.listener(move |this, _, window, cx| match &confirm {
                Confirm::Stop => { if this.can_interrupt() { this.remember_skip_chat_confirmations(cx); this.interrupt(window, cx); } },
                Confirm::Destructive(text) => {
                    // Só vale para o texto que a pessoa viu no aviso.
                    if this.composer.read(cx).value().as_ref() == text { this.remember_skip_chat_confirmations(cx); this.submit(false, true, window, cx); }
                    else { this.confirm = None; cx.notify(); }
                }
                Confirm::Replace(name) => { this.confirm = None; this.fill_command(&name.clone(), false, window, cx); }
                Confirm::Prefill(text) => { this.confirm = None; this.prefill(&text.clone(), false, window, cx); }
                Confirm::Shortcut(_, shortcut) => { this.confirm = None; this.run_shortcut(shortcut.clone(), true, window, cx); }
                Confirm::Reload => { this.confirm = None; this.reload(cx); }
                }))))
            .when(can_skip, |el| el.child(Checkbox::new("confirm-no-ask").small().label(tr("confirm_no_ask_actions"))
                .checked(self.confirm_no_ask).on_click(cx.listener(|this, checked: &bool, _, cx| { this.confirm_no_ask = *checked; cx.notify(); }))))
            .into_any_element())
    }

    fn remember_skip_chat_confirmations(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_no_ask { return; }
        let mut next = appearance::get();
        next.skip_chat_confirmations = true;
        self.apply_appearance(next, true, cx);
        self.confirm_no_ask = false;
    }

    fn render_suggestions(&self, suggestions: &[CommandInfo], cx: &mut Context<Self>) -> AnyElement {
        let active = self.suggest_pick.min(suggestions.len().saturating_sub(1));
        div().flex().flex_col().rounded_md().bg(theme::popup_content_fill()).p_1()
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
        // Conversa fechada aberta: o campo escreve para ela, e o Enviar a retoma (`send_reopen`).
        let reopen = self.selected.is_none() && self.reopen.is_some();
        let resuming = reopen && self.reopen_sending();
        let new_chat = !reopen && self.selected.is_none() && self.new_chat.is_some();
        let creating = new_chat && self.new_chat.as_ref().is_some_and(|view| view.read(cx).creating);
        let can_create = new_chat && self.new_chat.as_ref().is_some_and(|view| view.read(cx).can_create(cx));
        let key = self.composer_key();
        // A tela sem sessão anexa e dita antes de a sessão existir; os anexos sobem depois que ela nasce.
        let attachable = readable || (new_chat && key.is_some());
        let text = self.composer.read(cx).value().to_string();
        let uploading = key.as_ref().and_then(|key| self.uploading.get(key)).map(|batch| {
            let done = key.as_ref().and_then(|key| self.attachments.get(key)).map(|list| list.iter()
                .filter(|a| batch.contains(&a.id) && matches!(a.state, AttachState::Uploaded(_))).count()).unwrap_or(0);
            (done, batch.len())
        });
        let attached = key.as_ref().is_some_and(|key| self.attachments.get(key).is_some_and(|list| !list.is_empty()));
        let dictated = self.dictation_here(cx) && (self.dictation.recording() || self.dictation.processing());
        let has_input = !text.trim().is_empty() || attached || dictated;
        let suggestions = if readable { self.visible_suggestions(cx) } else { Vec::new() };
        if self.suggest_pick >= suggestions.len() { self.suggest_pick = 0; }
        let tray = key.as_ref().and_then(|key| self.render_attachments(key, cx));
        let confirm = self.render_confirm(cx);
        let (pills, mode) = self.render_ctl_pills(readable, cx);
        let pills = if new_chat { self.new_chat_pills(cx) } else { pills };
        let (provider, headless) = self.provider();
        let provider = match self.reopen.as_ref().filter(|_| reopen) {
            Some(entry) if !entry.provider.is_empty() => entry.provider.clone(),
            Some(_) => "claude".to_owned(),
            None => if new_chat { self.new_chat_provider(cx) } else { provider }.to_owned(),
        };
        let provider = provider.as_str();
        // A dica do terminal e o destinatário moram no placeholder, como no web.
        let placeholder = if readable && !self.terminal_suggestion.is_empty() {
            tr("terminal_suggestion").replace("{text}", &conversation::one_line(&self.terminal_suggestion, 120))
        } else { tr("composer").replace("{agent}", agent_name(provider)) };
        if self.composer_placeholder != placeholder {
            self.composer_placeholder = placeholder.clone();
            self.composer.update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
        }
        let steer_text = readable && has_input && (provider == "codex" || headless) && self.chat.state.state == "working"
            && self.selected_key().is_none_or(|key| self.group_targets(&key, "").is_none());
        // Só o anexo espera o envio em voo; texto sai do campo e vai na vez dele.
        let blocked = self.dictation.send_pending() || if reopen { resuming || self.reopen_blocked(cx) } else if new_chat { !can_create } else { sending && attached || uploading.is_some() || !self.chat_online || !self.history_installed };
        let can_stop = self.can_interrupt();
        let focused = self.composer.read(cx).focus_handle(cx).is_focused(window);
        let paste_target = cx.entity().downgrade();
        let textarea = Textarea::new(&self.composer).accessibility_id("composer-input").appearance(false).disabled((!readable && !new_chat && !reopen) || creating || resuming).on_paste(move |item, _, cx| {
            paste_target.update(cx, |this, cx| this.paste(item, cx)).unwrap_or(false)
        });
        let field = div().id("composer-field").text_base()
            // Teclas do campo que o compositor intercepta; sem uso aqui, seguem para o editor.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_suggestion(-1, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_suggestion(1, cx)))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| this.tab(window, cx)))
                .capture_action(cx.listener(|this, _: &Escape, window, cx| this.escape(window, cx)))
            .child(textarea);

        // Aviso e sugestões seguem o que se digita: ficam presos à borda de cima, sem cortina. Os painéis abertos por
        // botão moram na camada da raiz (`popup.rs`), presos ao botão.
        let floating: Vec<AnyElement> = confirm.map(|el| chrome::popover(el, false)).into_iter()
            .chain((!suggestions.is_empty()).then(|| chrome::popover(self.render_suggestions(&suggestions, cx), false)))
            .chain(self.render_mentions(cx).map(|el| chrome::popover(el, false)))
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
        let cache = crate::chat::last_cache(&self.chat.events).filter(|_| readable);
        let footer = (repo.is_some() || ctx_pct.is_some() || session.is_some() || cache.is_some()).then(|| {
            let branch = status.as_ref().and_then(|s| s.branch.clone()).or_else(|| session.as_ref().and_then(|s| s.branch.clone())).unwrap_or_default();
            let dirty = status.as_ref().and_then(|s| s.dirty) == Some(true);
            let (added, removed) = session.as_ref().map(|s| (s.git_added.filter(|n| *n > 0), s.git_removed.filter(|n| *n > 0))).unwrap_or((None, None));
            let folder = repo.clone().or_else(|| session.as_ref().and_then(folder_name));
            let cost = status.as_ref().and_then(|s| s.cost_usd).map(|usd| self.money(usd));
            // Anéis de contexto e de uso da conta; sem dado dizem isso, nunca 0%.
            let percent = |pct: Option<f64>| pct.map(|p| format!("{}%", p.round())).unwrap_or_else(|| tr("no_data"));
            // O anel mostra a janela de 5 h; conta que só publica a semanal (Codex, alguns planos) mostra a semanal.
            let account = if self.has_proxy_session() {
                self.focused_proxy_pct()
            } else {
                // Sem cota na linha, vale a leitura da mesma conta pela API.
                status.as_ref().filter(|_| readable).and_then(|s| s.five_hour_pct.or(s.weekly_pct).or(s.monthly_pct))
                    .or_else(|| self.focused_account().filter(|_| readable).and_then(|(_, _, window)| window.map(|(_, pct)| pct)))
            };
            // Anel, nome curto e número com a mesma geometria nos dois; o detalhe de cada um abre num cartão, não numa dica.
            let ring = |id: &'static str, name: String, pct: Option<f64>, open: bool| Button::new(id)
                .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::faint()).hover(theme::hover()).active(theme::hover()))
                .when(open, |el| el.bg(theme::hover())).flex_shrink_0().h(px(22.)).px(px(6.)).rounded(px(6.))
                .accessibility_label(format!("{name}: {}", percent(pct)))
                .child(div().flex().items_center().gap(px(5.)).text_xs()
                    .child(chrome::ring(pct))
                    .child(div().max_w(px(160.)).truncate().text_color(theme::faint()).child(name))
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(chrome::ring_text(pct)).child(percent(pct))));
            let has_git = !branch.is_empty();
            let place = div().min_w_0().flex().items_center().gap(px(6.)).text_xs().text_color(theme::faint())
                .when_some(folder, |el, f| el.child(chrome::small_icon(IconName::Folder, 14., theme::faint())).child(div().max_w(px(200.)).truncate().child(f)))
                .when(has_git, |el| el.child(div().ml(px(4.)).flex().items_center().gap(px(4.)).min_w_0()
                    .child(chrome::small_icon(IconName::GitBranch, 14., theme::faint()))
                    .child(div().max_w(px(160.)).truncate().child(branch))
                    .when(dirty, |el| el.child(div().text_color(theme::warning()).child("*")))))
                .when_some(added, |el, a| el.child(div().text_color(theme::success()).child(format!("+{a}"))))
                .when_some(removed, |el, r| el.child(div().text_color(theme::removed()).child(format!("−{r}"))));
            // Com repositório, a faixa abre o git da sessão (o `repo-chip` do web); o recuo negativo mantém o texto no lugar.
            let place = if has_git {
                Button::new("composer-git").custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::faint())
                    .hover(theme::hover()).active(theme::hover())).h(px(22.)).ml(px(-4.)).px(px(4.)).rounded(px(6.))
                    .tooltip(tr("git_open")).accessibility_label(tr("git_open")).child(place)
                    .on_click(cx.listener(|this, _, window, cx| this.open_git_panel(window, cx))).into_any_element()
            } else { place.into_any_element() };
            // O recuo negativo põe o texto do último item na mesma borda da faixa da pasta, do outro lado.
            let usage = div().flex_shrink_0().mr(px(-6.)).flex().items_center().gap(px(2.))
                .when_some(cache, |el, cache| el.child(cache_chip(cache)))
                .child(popup::anchor(div(), "composer-ctx").child(ring("composer-ctx", tr("ring_context"), ctx_pct, self.context_card)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_context_card(cx)))))
                .child(popup::anchor(div(), "composer-account").child(ring("composer-account", self.focused_account().map(|(_, name, _)| name).unwrap_or_else(|| tr("ring_account")), account, self.accounts.card)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_usage_card(cx)))))
                .when_some(cost, |el, cost| el.child(div().text_color(theme::faint()).opacity(0.6).child("·"))
                    .child(div().h(px(22.)).px(px(6.)).flex().items_center().text_color(theme::muted()).child(cost)));
            div().pt(px(7.)).px(px(6.)).flex().items_center().gap(px(6.)).text_xs().text_color(theme::faint())
                .children(session.as_ref().map(|s| self.render_group_chips(s, cx)))
                .child(place)
                .child(div().flex_1())
                .child(usage)
        });

        let send_label = tr(if creating { "create_creating" } else if sending && attached || uploading.is_some() || resuming { "sending" } else { "send" });
        let action = if can_stop && !has_input {
            Button::new("stop").custom(ButtonCustomVariant::new(cx).color(theme::elevated()).foreground(theme::danger()).hover(theme::raised()).active(theme::raised()))
                .bg(theme::elevated()).child(div().size(px(10.)).rounded(px(2.)).bg(theme::danger())).size(px(30.)).rounded_full()
                .tooltip(tr("stop_hint")).accessibility_label(tr("stop")).disabled(stopping)
                .on_click(cx.listener(|this, _, window, cx| this.request_stop(window, cx)))
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
                    this.close_recent();
                    this.ensure_commands(false);
                    this.command_search.update(cx, |input, cx| { input.set_value("", window, cx); input.focus(window, cx); });
                }
                cx.notify();
            }));
        let attach = chrome::icon_button("attach", IconName::Paperclip, tr("attach"), cx).disabled(!attachable || uploading.is_some())
            .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)));
        let recent_btn = popup::anchor(div(), "attach-recent").child(chrome::icon_button("attach-recent", IconName::RotateCcwClock, tr("attach_recent"), cx)
            .disabled(!readable || uploading.is_some()).selected(self.recent.is_some()).on_click(cx.listener(|this, _, _, cx| this.open_recent(cx))));
        let (microphone, dictation_style, dictation_strip) = self.render_dictation(attachable, cx);
        let control_row = div().flex().items_center().gap_1()
            .child(commands).child(attach).child(recent_btn).child(microphone).children(dictation_style)
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
            .children(dictation_strip)
            .child(control_row);
        // Na conversa, a caixa e o recuo de 12 px do `.composer` do web; a tela sem sessão fica na largura dela.
        let landing = self.new_chat_screen();
        let frame = if landing { div().w_full().max_w(px(column_width())) } else { column_box(true) };
        div().id("composer").relative().flex_shrink_0().w_full().px(px(if landing { 36. } else { 12. })).pb(px(10.)).flex().justify_center()
            .when(attachable, |el| el.drag_over::<ExternalPaths>(|style, _, _, _| style.bg(theme::accent_dim()))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.read_paths(paths.paths().to_vec(), cx))))
            .child(popup::anchor(frame.relative().flex().flex_col(), "composer")
                .when(!floating.is_empty(), |el| el.child(div().absolute().left_0().right_0().bottom(relative(1.)).pb_2().flex().flex_col().gap_2()
                    .occlude().children(floating)))
                .children(queue_row)
                .child(chrome::glass_panel(card, px(18.)))
                .children(footer))
            .into_any_element()
    }

    fn move_suggestion(&mut self, step: isize, cx: &mut Context<Self>) {
        if self.move_mention(step, cx) { cx.stop_propagation(); return; }
        let count = self.visible_suggestions(cx).len();
        if count == 0 { return; }
        self.suggest_pick = (self.suggest_pick as isize + step).rem_euclid(count as isize) as usize;
        cx.stop_propagation();
        cx.notify();
    }

    // Tab completa o comando destacado; com o campo vazio aceita a sugestão do terminal; senão segue a navegação.
    // Na captura, só `stop_propagation` impede o Tab de chegar também à troca de foco.
    fn tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.accept_mention(window, cx) { cx.stop_propagation(); return; }
        let suggestions = self.visible_suggestions(cx);
        if let Some(command) = suggestions.get(self.suggest_pick.min(suggestions.len().saturating_sub(1))) {
            let name = command.name.clone();
            self.complete_slash(&name, window, cx);
        } else if self.composer.read(cx).value().is_empty() && !self.terminal_suggestion.is_empty() {
            let text = self.terminal_suggestion.clone();
            self.composer.update(cx, |input, cx| { input.insert(text, window, cx); });
            cx.notify();
        } else { return; }
        cx.stop_propagation();
    }

    // Esc fecha o que está aberto sobre o campo; sem nada aberto e com a sessão trabalhando, pede para interromper.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_plan_review(window, cx) { return; }
        if self.confirm.is_some() { self.confirm = None; self.confirm_no_ask = false; }
        else if self.cancel_machine_rename() {}
        else if self.mention_is_open(cx) { self.mention.close(); }
        else if !self.visible_suggestions(cx).is_empty() { self.suggest_dismissed = Some(self.composer.read(cx).value().to_string()); }
        else if self.close_popups() {}
        else if self.side_menu_escape(cx) {}
        else if self.can_interrupt() {
            if appearance::get().skip_chat_confirmations { self.interrupt(window, cx); }
            else { self.confirm = Some(Confirm::Stop); self.confirm_no_ask = false; }
        }
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
        // Sessão orq: linha do tempo do orquestrador já interpretada pelo backend, desenhada à parte.
        if let Some(&Item::Event(event_index)) = self.conversation.items.get(index).filter(|_| id != PREVIEW) {
            if self.chat.events.get(event_index).is_some_and(|event| event.orq.is_some()) { return self.render_orq_event(&id, event_index, cx); }
        }
        // Texto preparado quando o chat mudou; a prévia usa a fonte que o passo do streaming já montou.
        let (markdown, blank) = if id == PREVIEW {
            let markdown = self.rich.get(&id).map(|rich| rich.source.clone()).unwrap_or_else(|| preview_source(&self.visible_preview));
            (markdown, self.visible_preview.text.trim().is_empty())
        } else {
            let Some(Item::Event(event_index)) = self.conversation.items.get(index) else { return div().into_any_element(); };
            let local;
            let message = match self.prepared.get(&id) {
                Some(message) => message,
                None => { local = prepare_message(&self.chat.events[*event_index], &self.cites.dead); &local }
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
            let Some(Item::Event(event_index)) = self.conversation.items.get(index) else { return div().into_any_element(); };
            let event = &self.chat.events[*event_index];
            let label = match event.kind.as_str() {
                "user_msg" => tr("you"), "assistant_msg" => tr("assistant"), "thinking" => tr("thinking"),
                "tool_use" | "tool_result" => event.tool_name.clone().unwrap_or_else(|| tr("tool")),
                "notice" => event.loaded_skill()
                    .and_then(|skill| crate::i18n::tr_web("notice_skill_loaded", &HashMap::from([("name".to_owned(), skill.name)])))
                    .unwrap_or_else(|| tr("notice")),
                _ => tr("unknown"),
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
        let plain = id == PREVIEW || (kind_of(&self.conversation.items, index, &self.chat.events) == Some("assistant_msg") && !error);
        // Resposta gravada com tabela numérica: o texto vai em trechos, e cada tabela ganha o botão Gráfico.
        // Só o retrato do `sync_tables`, e só com a mesma fonte que está sendo desenhada; a prévia nunca tem gráfico.
        let charted = (plain && id != PREVIEW && appearance::get().table_chart).then(|| self.tables.get(&id)).flatten()
            .filter(|(source, tables)| *source == markdown && !tables.is_empty()).map(|(_, tables)| tables.clone());
        let refs = match self.conversation.items.get(index) {
            Some(Item::Event(i)) if id != PREVIEW => self.chat.events.get(*i).map(attachment_refs).unwrap_or_default(),
            _ => Vec::new(),
        };
        // Na bolha do usuário as imagens saem em miniatura, lado a lado e acima do texto, como no web.
        let peer = match self.conversation.items.get(index) { Some(Item::Event(i)) if user => self.chat.events.get(*i).and_then(peer_of), _ => None };
        let peer_scope = peer.as_ref().map(|peer| peer.scope);
        let peer_head = peer.map(|peer| self.peer_head(&id, peer, cx));
        let (images, refs): (Vec<_>, Vec<_>) = refs.into_iter().partition(|(_, _, image)| user && *image);
        let thumbs = self.render_thumbs(&id, images.into_iter().map(|(source, _, _)| source).collect(), cx);
        let files = (!refs.is_empty()).then(|| self.render_refs(&id, refs, cx));
        let more_key = format!("{id}#more");
        // Skill injetada: só o rótulo, e o SKILL.md inteiro no "mostrar mais" (como a linha recolhida do web).
        let skill = match self.conversation.items.get(index) {
            Some(Item::Event(i)) if id != PREVIEW => self.chat.events.get(*i).is_some_and(|e| e.skill.is_some()),
            _ => false,
        };
        let long = skill || (user && long_message(&markdown));
        let open = self.expanded.contains(&more_key);
        let text: Vec<AnyElement> = match charted {
            None if skill && !open => Vec::new(),
            Some(tables) => self.render_charted(&id, &markdown, &tables, cx),
            None if !blank || (files.is_none() && thumbs.is_none()) => {
                let view = self.text_view(&id, &id, markdown, cx);
                let runner = (plain && id != PREVIEW).then(|| cx.weak_entity());
                let text = chat_text_runnable(&view, cx, runner.clone(), &id).motion(stream_motion(id == PREVIEW)).on_link_click(open_web_link)
                    .markdown_extensions(citation_extensions(&id, cx.weak_entity()));
                vec![collapse(text, long, open).into_any_element()]
            }
            None => Vec::new(),
        };
        let more = long.then(|| more_button(format!("more-{id}"), open)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(more_key.clone(), cx))));
        // Hora e copiar sob a mensagem, só ao passar o mouse; a faixa é reservada para a lista não remedir.
        let actions = (id != PREVIEW && (user || plain)).then(|| {
            let ts = match self.conversation.items.get(index) { Some(Item::Event(i)) => self.chat.events.get(*i).and_then(|e| e.ts), _ => None };
            let (view, copy_id) = (cx.weak_entity(), id.clone());
            div().h(px(24.)).flex().items_center().gap_1().opacity(0.).group_hover(ROW_GROUP, |s| s.opacity(1.))
                .when_some(stamp(ts), |el, at| el.child(div().text_xs().text_color(theme::faint()).child(at)))
                .child(CopyButton {
                    id: SharedString::from(format!("copy-{id}")).into(),
                    value: std::rc::Rc::new(move |cx| view.upgrade().and_then(|view| view.read(cx).copy_text(&copy_id)).unwrap_or_default()),
                })
        });
        let content = conversation_text(div().flex().flex_col().gap_2(), user)
            .when(!user && !plain, |el| el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::muted() }).child(label)))
            .when_some(peer_head, |el, head| el.child(head))
            .when_some(thumbs, |el, thumbs| el.child(thumbs))
            .children(text)
            .when_some(more, |el, more| el.child(more))
            .when_some(files, |el, files| el.child(files));
        let row = div().id(SharedString::from(format!("message-{id}"))).group(ROW_GROUP).w_full().flex().flex_col().gap_2()
            .map(|el| if user { el.items_end().child(match peer_scope { Some(scope) => peer_bubble(content, scope), None => user_bubble(content) }) } else { el.child(content) })
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
                .small().outline().tooltip(title.clone()).accessibility_label(title)
                .child(crate::fileicons::citation_icon(composer::basename(&path)))
                .child(div().min_w_0().whitespace_nowrap().text_ellipsis().child(label))
                .on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| this.open_file(path.clone(), line, window, cx));
                })
        }
    }
    MarkdownExtensions::default().plugin(Citations { row: row.to_owned(), owner })
}

fn shell_code_language(lang: Option<&str>) -> bool {
    lang.is_some_and(|lang| matches!(lang.to_ascii_lowercase().as_str(), "bash" | "sh" | "zsh" | "fish" | "shell" | "powershell" | "ps1" | "pwsh"))
}

/// Markdown da conversa. É o `TextView` do gpui-base porque o do componente não repassa os campos que só a
/// conversa liga (marcador em coluna, faixa de linguagem).
fn chat_text(view: &Entity<TextViewState>, cx: &App) -> gpui_kit::base::TextView {
    gpui_kit::base::TextView::new(view).selectable(true).scrollable(false).style(theme::conversation_markdown(cx))
        .code_block_actions(copy_code)
}

fn chat_text_runnable(view: &Entity<TextViewState>, cx: &App, runner: Option<WeakEntity<Hangar>>, row: &str) -> gpui_kit::base::TextView {
    let text = chat_text(view, cx);
    let Some(owner) = runner else { return text };
    let row = row.to_owned();
    text.code_block_actions(move |block, window, cx| {
        let code = block.code().to_string();
        let language = block.lang().map(|lang| lang.to_string());
        let can_run = shell_code_language(language.as_deref()) && !code.trim().is_empty() && code.len() <= 4096;
        let owner = owner.clone();
        div().flex().items_center().gap(px(4.))
            .when(can_run, |el| el.child(Button::new(format!("run-block-{row}-{}", block.span.as_ref().map_or(0, |span| span.start)))
                .ghost().xsmall().label(tr("code_run")).accessibility_label(tr("code_run_aria"))
                .on_click(move |_, _, cx| { let _ = owner.update(cx, |this, cx| this.run_code_command(code.clone(), language.clone(), cx)); })))
            .child(copy_code(block, window, cx))
    })
}

/// Grupo de hover da linha da mensagem: a faixa de hora e copiar acende com ele.
const ROW_GROUP: &str = "message-row";

/// Recuo e coluna de uma linha da conversa; mensagem tem mais ar em cima e embaixo.
fn row_frame(inner: AnyElement, message: bool) -> Div {
    div().w_full().px(px(16.)).when(message, |el| el.py(px(8.))).when(!message, |el| el.py(px(2.)))
        .flex().justify_center()
        .child(column_box(false).px(px(column_padding())).child(inner))
}

/// A conversa ainda sem histórico: turnos fantasmas no formato das linhas (pergunta em bolha à direita, resposta em
/// linhas à esquerda), colados no fim como a lista. Cada peça só aparece depois da espera do `Skeleton`, então sessão
/// rápida não pisca.
fn render_history_skeleton() -> Div {
    const TURNS: [(f32, [f32; 3]); 3] = [(180., [0.94, 0.88, 0.52]), (260., [0.9, 0.97, 0.7]), (140., [0.86, 0.62, 0.0])];
    let mut row = 0;
    let turns = TURNS.iter().enumerate().map(|(turn, (bubble, lines))| {
        let user = chrome::Skeleton::new(("history-skeleton-user", turn)).row(row).w(px(*bubble)).h(px(38.)).rounded(px(18.));
        row += 1;
        let answer = lines.iter().enumerate().filter(|(_, width)| **width > 0.).map(|(line, width)| {
            row += 1;
            chrome::Skeleton::new(("history-skeleton-line", turn * 3 + line)).row(row).w(relative(*width)).h(px(12.)).rounded(px(4.))
        }).collect::<Vec<_>>();
        div().flex().flex_col().gap(px(14.)).child(div().flex().justify_end().child(user))
            .child(div().flex().flex_col().gap(px(9.)).children(answer))
    }).collect::<Vec<_>>();
    div().flex_1().min_h_0().flex().flex_col().justify_end().pb(px(20.)).overflow_hidden()
        .child(in_column(div().flex().flex_col().gap(px(28.)).children(turns)))
}

/// Bolha do usuário, na conversa e no subagente.
fn user_bubble(content: impl IntoElement) -> Div {
    div().max_w(relative(0.78)).px(px(14.)).py(px(10.)).rounded(px(18.)).bg(theme::user_bubble()).child(content)
}

/// Recado de outra sessão com a cor de quem manda: accent (1:1), âmbar (grupo), neutro (app), como no web.
fn peer_bubble(content: impl IntoElement, scope: cards::PeerScope) -> Div {
    let tint = peer_tint(scope);
    let fill = if scope == cards::PeerScope::Peer { theme::accent_dim() } else { tint.opacity(0.12) };
    user_bubble(content).bg(fill).border_1().border_color(tint)
}

fn peer_tint(scope: cards::PeerScope) -> Hsla {
    match scope { cards::PeerScope::Peer => theme::accent(), cards::PeerScope::Group => theme::warning(), cards::PeerScope::Panel => theme::muted() }
}

/// Recado de outra sessão numa mensagem do usuário, lido da legenda (os anexos ficam de fora).
fn peer_of(event: &ChatEvent) -> Option<cards::PeerMessage> {
    let body = event.body();
    cards::peer_message(&composer::parse_marked(&body).map(|m| m.caption).unwrap_or(body))
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
/// A linha inteira da opção é o controle: rótulo, descrição e prévia recebem o clique e o foco do teclado dele.
fn ask_option<E: Styled + InteractiveElement + ParentElement + IntoElement>(control: E, option: &AskOption, selected: bool, busy: bool) -> E {
    // Linha sem caixa em volta: só a escolhida ganha fundo e borda, o que a separa das outras de relance.
    control.w_full().px_3().py_2().rounded_lg().border_1()
        .border_color(if selected { theme::accent().opacity(0.6) } else { transparent_black() })
        .when(selected, |el| el.bg(theme::accent_dim()))
        .when(!busy, |el| el.cursor_pointer())
        .when(!busy && !selected, |el| el.hover(|style| style.bg(theme::hover())))
        .child(div().font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(option.label.clone()))
        .when(!option.description.is_empty(), |el| el.child(div().text_sm().text_color(theme::text().opacity(0.78)).child(option.description.clone())))
        .when_some(option.preview.clone().filter(|p| !p.is_empty()), |el, preview| el.child(div().mt_1().p_2().rounded_md().bg(theme::raised())
            .font_family(crate::theme::MONO).text_xs().whitespace_nowrap().overflow_x_hidden().child(preview)))
}

/// Quadros seguidos que a fileira de abas dos mods espera pela geometria antes de desistir.
const TAB_SCROLL_WAITS: u8 = 3;

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
            refs.extend(composer::cited_paths(&body).into_iter().map(cited_ref));
            for url in composer::image_urls(&body) { refs.push((Source::Remote(url.clone()), composer::url_name(&url).to_owned(), true)); }
        }
        _ => {}
    }
    refs
}

/// SendUserFile é como o agente põe um arquivo diante da pessoa: o caminho vai em `files`.
pub(super) fn sends_files(call: &ChatEvent) -> bool {
    call.tool_name.as_deref().is_some_and(|name| name.eq_ignore_ascii_case("senduserfile"))
}

/// Arquivos citados na entrada da ferramenta: o que o Read leu e o que o SendUserFile mandou. O transcript não traz os
/// bytes; o caminho citado vem pelo `/file` (regra do web).
pub(super) fn tool_file_refs(call: &ChatEvent) -> Vec<(Source, String, bool)> {
    let input = call.tool_input.as_ref();
    let paths: Vec<&str> = match call.tool_name.as_deref() {
        Some(name) if name.eq_ignore_ascii_case("read") => vec![crate::editdiff::input_path(input)],
        _ if sends_files(call) => input.and_then(|i| i.get("files")).and_then(|f| f.as_array())
            .map(|files| files.iter().filter_map(|f| f.as_str()).collect()).unwrap_or_default(),
        _ => Vec::new(),
    };
    paths.into_iter().flat_map(composer::cited_paths).map(cited_ref).collect()
}

/// Caminho citado como anexo: o nome que aparece embaixo e se ele abre como imagem.
fn cited_ref(path: String) -> (Source, String, bool) {
    let name = composer::basename(&path).to_owned();
    let image = composer::image_format(&name).is_some();
    (Source::Cited(path), name, image)
}

// Anexo que saiu do campo: tira a imagem inteira do cache de assets e do atlas da GPU, que não a soltam sozinhos.
fn release_image(image: Arc<Image>, window: &mut Window, cx: &mut App) {
    if let Some(render) = image.clone().get_render_image(window, cx) { cx.drop_image(render, Some(window)); }
    image.remove_asset(cx);
}

impl Hangar {
    /// Barra lateral na caixa dela: a lista cheia, o trilho, ou os dois se trocando enquanto a largura anda.
    fn render_sidebar(&self, selected_name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let full = appearance::get().full_sidebar_width();
        match self.rail_progress() {
            None if self.rail() => self.nav_frame(sidebar::RAIL_WIDTH, false, self.render_nav_rail(selected_name, cx), None),
            None => self.nav_frame(full, true, self.render_sidebar_full(selected_name, window, cx), Some(self.nav_resize_handle(full, cx))),
            Some(p) => {
                // A lista cheia sai nos primeiros 60% e o trilho entra nos últimos 60%, os dois presos à esquerda e cortados
                // pela caixa que anda: a lista parece deslizar para baixo da borda, e o trilho, sair dela.
                let (out, into) = (1. - (p / 0.6).min(1.), ((p - 0.4) / 0.6).clamp(0., 1.));
                let layer = |width: f32, opacity: f32, content: AnyElement| div().absolute().top_0().left_0().h_full().w(px(width))
                    .flex().flex_col().opacity(opacity).child(content);
                let both = div().size_full().relative().overflow_hidden()
                    .child(layer(full, out, self.render_sidebar_full(selected_name, window, cx)))
                    .child(layer(sidebar::RAIL_WIDTH, into, self.render_nav_rail(selected_name, cx)));
                self.nav_frame(self.nav_width(), p < 0.5, both.into_any_element(), None)
            }
        }
    }

    /// A caixa da barra: fundo, borda, cantos e sombra do painel solto, na largura dada. `full` é a lista cheia, que no
    /// modo Conversas tem fundo e borda próprios.
    fn nav_frame(&self, width: f32, full: bool, content: AnyElement, handle: Option<AnyElement>) -> AnyElement {
        let a = appearance::get();
        let conversations = full && a.navigation == appearance::Navigation::Conversations;
        let (surface, _, _, border) = theme::conversation_sidebar();
        let floating = a.panels == appearance::Panels::Floating;
        // Durante a troca as duas formas são camadas soltas, que não dão altura: a caixa ocupa a coluna inteira.
        let fit_content = floating && a.sidebar_height == appearance::SidebarHeight::Content && self.rail_progress().is_none();
        let panel = chrome::glass_panel(div().w(px(width)).flex_shrink_0().flex().flex_col().bg(if conversations { surface } else { theme::chrome() })
            // A linha da janela estica os filhos; "Só o conteúdo" solta a barra do fundo.
            .map(|el| if fit_content { el.max_h_full() } else { el.h_full() })
            .map(|el| if floating { el.rounded(px(theme::PANEL_RADIUS)).border_1().border_color(theme::border()).shadow(theme::panel_shadow()) }
                else { el.border_r_1().border_color(theme::border()) })
            .when(conversations, |el| el.border_color(border))
            .relative()
            .child(content)
            .children(handle),
            px(if floating { theme::PANEL_RADIUS } else { 0. }));
        // A view guardada não é flex: quem centra a barra "só o conteúdo" na altura é esta coluna, como o `align-self: center` do web.
        if fit_content { div().size_full().flex().flex_col().justify_center().child(panel).into_any_element() } else { panel }
    }

    /// A lista cheia: marca, nova conversa, escopo, seções "Aguardando você" e "Sessões", rodapé com a conexão.
    fn render_sidebar_full(&self, selected_name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let a = crate::appearance::get();
        let conversations = a.navigation == appearance::Navigation::Conversations;
        let floating = a.panels == crate::appearance::Panels::Floating;
        let fit_content = floating && a.sidebar_height == crate::appearance::SidebarHeight::Content;
        let host = self.server_label(cx);
        let layout = self.sidebar_layout(cx);
        let section = |label: String, count: Option<usize>| div().flex().items_center().justify_between().px(px(8.)).pt(px(12.)).pb(px(6.))
            .child(chrome::section_label(label))
            .when_some(count, |el, n| el.child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).child(n.to_string())));
        let mut children: Vec<AnyElement> = Vec::new();
        // Membros de um grupo vêm juntos, sob o cabeçalho do bloco, como o `clusterByPair` do web.
        let rows = |list: &[&SessionInfo], remote: Option<&str>, children: &mut Vec<AnyElement>, window: &mut Window, cx: &mut Context<Self>| for row in grouping::cluster(list) {
            let session = match row {
                grouping::ListRow::Header { gid, label, members } => {
                    children.push(self.render_pair_header(&gid, &label, &members, remote, window, cx));
                    continue;
                }
                grouping::ListRow::External { gid, .. } if self.sidebar.is_collapsed(&grouping::pair_key(&gid, remote)) => continue,
                grouping::ListRow::External { gid, owner, session, alias } => {
                    children.push(self.render_external_pair_row(&gid, &owner, &session, &alias, remote, window, cx));
                    continue;
                }
                grouping::ListRow::Session(session) if self.pair_collapsed(session, remote) => continue,
                grouping::ListRow::Session(session) => session,
            };
            let selected = selected_name == Some(session.name.as_str()) && self.open_key().as_deref() == remote;
            let remote = remote.map(str::to_owned);
            children.push(if conversations { self.render_conversation_row(session.clone(), selected, remote, window, cx) }
                else { self.render_session_row(session.clone(), selected, remote, window, cx) });
        };
        let place = |layout: &sidebar::Layout, remote: Option<&str>, children: &mut Vec<AnyElement>, window: &mut Window, cx: &mut Context<Self>| {
            if !layout.waiting.is_empty() {
                children.push(section(tr("sidebar_awaiting"), Some(layout.waiting.len())).into_any_element());
                rows(&layout.waiting, remote, children, window, cx);
            }
            for group in &layout.groups {
                if layout.by_project {
                    let awaiting = group.sessions.iter().filter(|s| s.state == "awaiting_input").count();
                    children.push(self.render_group_header(group, awaiting, window, cx));
                    if self.sidebar.is_collapsed(&group.key) { continue; }
                } else if remote.is_none() && !self.multi_server() {
                    children.push(section(group.label.clone(), None).into_any_element());
                }
                rows(&group.sessions, remote, children, window, cx);
            }
        };
        let mut total = layout.total;
        let mut remote_rows = 0;
        if self.multi_server() {
            // Como o web com mais de uma máquina: um bloco por servidor, na ordem da lista.
            let active = self.active_key();
            for entry in self.servers.iter().filter(|s| !s.disabled) {
                let key = servers::norm(&entry.address);
                if key == active {
                    let error = self.invite_ended.contains(&key).then(|| tr_shared("convite_encerrado", &[])).or(self.list_error.clone());
                    children.push(self.render_server_header(&entry.id, &key, &entry.label, layout.total, entry.invite, error, cx));
                    if !self.sidebar.is_collapsed(&format!("server:{key}")) { place(&layout, None, &mut children, window, cx); }
                } else if let Some(list) = self.remote.get(&key) {
                    let remote = self.remote_layout(&key, &list.sessions, cx);
                    total += remote.total;
                    remote_rows += remote.total;
                    children.push(self.render_server_header(&entry.id, &key, &entry.label, remote.total, entry.invite, list.error.clone(), cx));
                    if !self.sidebar.is_collapsed(&format!("server:{key}")) { place(&remote, Some(&key), &mut children, window, cx); }
                }
            }
        } else {
            place(&layout, None, &mut children, window, cx);
        }
        children.extend(self.render_recents(window, cx));
        let active = self.active_key();
        let empty = remote_rows == 0 && self.sessions.iter().all(|s| self.sidebar.is_hidden(&active, &s.name));
        let list = div().id("session-list").min_h_0().overflow_y_scroll().px(px(8.)).flex().flex_col().gap(px(2.))
            // A borda do painel já ocupa parte do recuo externo de oito pixels.
            .when(conversations, |el| el.pl(px(if floating { 7. } else { 8. })).pr(px(7.)))
            .when(!fit_content, |el| el.flex_1())
            .when(empty && self.list_error.is_none(), |el| el.child(div().p_2().text_xs().text_color(theme::faint())
                .child(tr(if self.list_online { "empty_sessions" } else { "connecting" }))))
            .when(layout.filter_empty(), |el| el.child(div().p_2().text_xs().text_color(theme::faint()).child(tr("sidebar_filter_empty"))))
            .children(children)
            .map(|el| self.drop_background(el, cx));
        let filter = layout.show_filter().then(|| div().flex_shrink_0().px(px(8.)).pb(px(4.))
            .child(Input::new(&self.sidebar.filter).small().cleanable(true).prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))
                .aria_label(tr("sidebar_filter")).accessibility_id("sidebar-filter")));
        div().w_full().min_h_0().flex().flex_col().when(!fit_content, |el| el.h_full())
            .child(div().h(px(44.)).flex_shrink_0().px(px(14.)).flex().items_center().gap_2()
                .child(chrome::hangar_mark(20., theme::accent()))
                .child(div().flex_1().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("brand")))
                .children(self.render_hangar_chip(hangar_live::Chip::Label, cx)))
            // A tela sem sessão, como o "New session" do topo da barra do Zeron; o "Nova sessão" do rodapé segue abrindo o diálogo.
            // Mesma coluna, recuo e altura da linha "Todas as sessões" logo abaixo; o destaque é o translúcido das linhas da
            // lista, e o atalho aparece apagado só com o ponteiro em cima.
            .child({
                let (on, enabled) = (self.new_chat_screen() && self.reopen.is_none(), self.api.is_some());
                div().id("sidebar-new-chat").group("sidebar-new-chat").flex_shrink_0().mx(px(8.)).mt(px(4.)).h(px(32.)).px(px(8.))
                    .flex().items_center().gap_2().rounded(px(8.)).font_weight(FontWeight::MEDIUM)
                    .track_focus(&self.new_chat_focus)
                    .when(self.new_chat_focus.is_focused(window), |el| el.focus_ring_style(window, cx))
                    .role(Role::Button).aria_selected(on).aria_label(tr("new_chat_title"))
                    .when(on, |el| el.bg(theme::selected_row()))
                    .when(!enabled, |el| el.opacity(0.5))
                    .when(enabled && !on, |el| el.cursor_pointer().hover(|el| el.bg(theme::hover())))
                    .child(chrome::small_icon(IconName::SquarePen, 16., theme::muted()))
                    .child(div().flex_1().min_w_0().truncate().child(tr("new_chat_title")))
                    .children(gpui_kit::component::kbd::Kbd::global_binding_for_action(&NewChat, window).map(|key| div().flex_shrink_0()
                        .font_family(theme::MONO).text_size(px(11.)).font_weight(FontWeight::NORMAL).text_color(theme::faint())
                        .opacity(0.).group_hover("sidebar-new-chat", |s| s.opacity(1.)).child(key.appearance(false))))
                    .when(enabled, |el| el.on_click(cx.listener(|this, _, window, cx| this.go_home(window, cx)))
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if !matches!(event.keystroke.key.as_str(), "enter" | "space") { return; }
                            this.go_home(window, cx);
                            cx.stop_propagation();
                        })))
            })
            .child(div().flex_shrink_0().mx(px(8.)).mt(px(4.)).mb(px(8.)).h(px(32.)).px(px(8.)).flex().items_center().gap_2().font_weight(FontWeight::MEDIUM)
                .child(chrome::small_icon(IconName::Server, 16., theme::muted()))
                .child(div().flex_1().min_w_0().truncate().child(tr("sidebar_all_sessions")))
                .child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).child(total.to_string()))
                .child(self.render_group_picker(cx)))
            .children(filter)
            .child(list)
            .when_some(self.list_error.clone(), |el, text| el.child(div().px_4().py_1().flex().items_center().gap_2().text_xs().text_color(theme::warning())
                .child(div().flex_1().min_w_0().child(text))
                .child(Button::new("reconnect").xsmall().ghost().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))))))
            // O CTA do rodapé da barra do web, com o recolher ao lado.
            .child(div().flex_shrink_0().px(px(8.)).pt(px(8.)).pb(px(8.)).flex().items_center().gap_2()
                .child(self.new_session_button(false, cx))
                .child(self.fold_button("sidebar-fold", cx)))
            // A engrenagem mora na barra do app, acima de tudo; o rodapé fica com a conexão.
            .child(div().h(px(48.)).flex_shrink_0().px(px(8.)).flex().items_center().gap_1().border_t_1().border_color(theme::border())
                .child(Button::new("connection").ghost().flex_1().min_w_0().h(px(32.)).px(px(6.))
                    .tooltip(tr("connection_tip")).accessibility_label(tr("connection"))
                    .child(div().w_full().min_w_0().flex().items_center().gap_2()
                        .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(if self.list_online { theme::success() } else { theme::warning() }))
                        .child(div().min_w_0().truncate().text_size(px(13.)).text_color(theme::muted()).child(host)))
                    .on_click(cx.listener(|this, _, window, cx| this.open_connection(window, cx)))))
            .into_any_element()
    }

    /// Abas no topo (como o web) ou embaixo: todas as sessões numa faixa, e o servidor e a conexão que moravam
    /// no rodapé da barra lateral. ←/→ andam o foco entre as abas; Enter ou Espaço abrem a sessão.
    fn render_tabs(&mut self, selected_name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let floating = theme::is_floating();
        let host = self.server_label(cx);
        let weak = cx.entity().downgrade();
        let active = self.active_key();
        let tabs = self.sessions.iter().filter(|s| !self.sidebar.is_hidden(&active, &s.name)).filter_map(|session| {
            let focus = self.tab_focus.get(&session.name)?.clone();
            // As abas são da lista ativa: sessão de mesmo nome aberta em outra máquina não marca nenhuma.
            let on = self.open_api.is_none() && selected_name == Some(session.name.as_str());
            let state = if session.limited == Some(true) { "limited" } else { session.state.as_str() };
            // Nome, estado e perguntas por extenso: o ponto só diz o estado pela cor.
            let mut label = format!("{} · {}", session.name, tr(&format!("chip_{state}")));
            if session.pending_questions > 0 { label.push_str(&format!(" · ? {}", session.pending_questions)); }
            // Trabalhando é a marca animada da lista (parada com movimento reduzido); os outros estados são um ponto na cor dele.
            let mark = if session.state == "working" {
                self.working_mark_slot(panes::Area::Nav, format!("tab-mark-{}", session.name), 14., theme::accent())
            } else {
                div().size(px(8.)).mx(px(2.)).flex_shrink_0().rounded_full().bg(if state == "limited" { theme::limited() } else { theme::status(state) }).into_any_element()
            };
            let pick = session.clone();
            let open = session.clone();
            let menu_target = sidebar::Target::new(&active, &session.name);
            let label = self.session_number_label(&menu_target, label);
            let number = self.session_number_badge(&menu_target);
            Some(div().id(SharedString::from(format!("tab-{}", session.name))).track_focus(&focus).flex_shrink_0().max_w(px(200.)).h(px(32.)).px(px(8.))
                .flex().items_center().gap(px(6.)).rounded(px(6.)).border_1().cursor_pointer()
                .map(|el| if on { el.bg(theme::accent_dim()).border_color(theme::accent()).text_color(theme::text()).font_weight(FontWeight::SEMIBOLD) }
                    else { el.border_color(transparent_black()).text_color(theme::muted()).hover(|el| el.bg(theme::hover())) })
                .when(focus.is_focused(window), |el| el.focus_ring_style(window, cx))
                .role(Role::Tab).aria_selected(on).aria_label(label.clone())
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx))
                .child(mark)
                .children(number)
                .child(chrome::provider_glyph(&session.provider, 14.))
                .child(div().min_w_0().truncate().text_size(px(13.)).child(session.name.clone()))
                .when(session.pending_questions > 0, |el| el.child(div().flex_shrink_0().text_xs().text_color(theme::warning())
                    .child(format!("? {}", session.pending_questions))))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select(pick.clone(), window, cx);
                    this.focus_composer_for(&pick, window, cx);
                }))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.select(open.clone(), window, cx);
                        cx.stop_propagation();
                    }
                }))
                // Clique direito na aba abre o mesmo menu da linha da barra, como o SessionTabs do web.
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.start_menu(menu_target.clone(), cx)))
                .context_menu(sidebar::session_menu(weak.clone(), active.clone(), session.clone())))
        }).collect::<Vec<_>>();
        // A folga lateral deixa o anel de foco da primeira e da última aba fora do recorte da rolagem. A faixa mede o que
        // as abas medem e só encolhe (rolando) quando falta espaço, para os botões de criar ficarem logo depois da última.
        let strip = div().id("tabs-strip").flex_shrink_1().min_w_0().h_full().px(px(3.)).flex().items_center().gap(px(2.)).overflow_x_scroll().track_scroll(&self.tabs_scroll)
            .role(Role::TabList).aria_label(tr("sessions"))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let step = match event.keystroke.key.as_str() { "left" => -1, "right" => 1, _ => return };
                let active = this.active_key();
                let names: Vec<&String> = this.sessions.iter().filter(|s| !this.sidebar.is_hidden(&active, &s.name)).map(|s| &s.name).collect();
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
        chrome::glass_panel(div().h(px(44.)).w_full().flex_shrink_0().px(px(8.)).flex().items_center().gap(px(6.))
            .bg(theme::chrome()).border_color(theme::border())
            .map(|el| if floating { el.rounded(px(theme::PANEL_RADIUS)).border_1().shadow(theme::panel_shadow()) }
                else if appearance::get().navigation == appearance::Navigation::BottomTabs { el.border_t_1() } else { el.border_b_1() })
            .child(div().px(px(6.)).child(chrome::hangar_mark(16., theme::accent())))
            .children(self.render_hangar_chip(hangar_live::Chip::Label, cx))
            .child(div().flex_1().min_w_0().h_full().flex().items_center().gap(px(6.))
                .child(strip)
                .child(chrome::icon_button("tabs-new-chat", IconName::SquarePen, tr("new_chat_title"), cx).flex_shrink_0().selected(self.new_chat_screen() && self.reopen.is_none())
                    .disabled(self.api.is_none()).on_click(cx.listener(|this, _, window, cx| this.go_home(window, cx))))
                .child(self.new_session_button(true, cx)))
            .when_some(self.list_error.clone(), |el, text| el.child(div().flex_shrink_0().max_w(px(260.)).flex().items_center().gap_1()
                .child(div().min_w_0().truncate().text_xs().text_color(theme::warning()).child(text))
                .child(Button::new("reconnect").xsmall().ghost().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))))))
            .child(Button::new("connection").ghost().flex_shrink_0().max_w(px(220.)).h(px(32.)).px(px(8.))
                .tooltip(tr("connection_tip")).accessibility_label(tr("connection"))
                .child(div().min_w_0().flex().items_center().gap_2()
                    .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(if self.list_online { theme::success() } else { theme::warning() }))
                    .child(div().min_w_0().truncate().text_size(px(13.)).text_color(theme::muted()).child(host)))
                .on_click(cx.listener(|this, _, window, cx| this.open_connection(window, cx)))),
            px(if floating { theme::PANEL_RADIUS } else { 0. }))
    }

    /// Linha do Zeron: estado, glifo, nome e hora numa linha só; no Normal, a branch fora de main/master embaixo.
    /// `remote` é a chave da máquina de uma linha que não é do servidor ativo: menu, foco e gestos são os mesmos, cada um
    /// na máquina da linha.
    fn render_conversation_row(&self, session: SessionInfo, selected: bool, remote: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let compact = appearance::get().sidebar_compact;
        let (_, selection, hover, _) = theme::conversation_sidebar();
        let name = session.name.clone();
        let target = sidebar::Target::new(&remote.clone().unwrap_or_else(|| self.active_key()), &name);
        let focus = self.row_focus(&target);
        let focused = focus.is_some_and(|f| f.contains_focused(window, cx));
        let hovered = self.sidebar.hover.as_ref() == Some(&target);
        let menu_open = self.sidebar.button_menu.as_ref() == Some(&target);
        let show_menu = hovered || focused || menu_open;
        let outcome = selected.then(|| self.selected_key()).flatten().and_then(|key| self.delivery.outcome(&key));
        let queued = selected && self.history_installed && self.queued_count() > 0;
        let state = conversation_row_state(&session, outcome, queued);
        let color = theme::conversation_status(state);
        let status_label = tr(&format!("sidebar_state_{state}"));
        let label = self.session_number_label(&target,
            match folder_name(&session) { Some(folder) => format!("{name} · {folder} · {status_label}"), None => format!("{name} · {status_label}") });
        let branch = shown_branch(&session).filter(|_| !compact);
        // Como a branch, o chip e o aviso de worktree apagada ficam fora do modo compacto.
        let worktree = worktree_label(&session).filter(|_| !compact && !session.worktree_gone);
        let worktree_gone = !compact && session.worktree_gone;
        let second_line = branch.is_some() || worktree.is_some() || worktree_gone;
        let worktree_target = session.worktree_path.clone().or_else(|| session.cwd.clone()).unwrap_or_default();
        let merged = self.worktrees.status(&worktree_target).is_some_and(|s| s.merged);
        let time = div().flex_shrink_0().text_size(px(11.)).line_height(px(14.)).text_color(theme::muted())
            .children(session.last_activity.map(side::since));
        // Id da marca com a máquina: a de mesmo nome em outra máquina não divide a animação.
        let row_key = if remote.is_some() { target.id() } else { name.clone() };
        let glyph = if state == "working" {
            self.nav_mark(format!("conversation-mark-{row_key}"), 13., color, None)
        } else { div().size(px(6.)).rounded_full().bg(color).into_any_element() };
        let status = div().size(px(13.)).flex_shrink_0().flex().items_center().justify_center().child(glyph);
        let weak = cx.weak_entity();
        let menu = || {
            let (weak, target) = (weak.clone(), target.clone());
            Button::new(SharedString::from(format!("conversation-menu-{row_key}"))).ghost().xsmall()
                .icon(chrome::small_icon(IconName::Ellipsis, 13., theme::muted())).h(px(18.)).px(px(4.)).rounded(px(5.))
                // No hover fica fora do Tab; com foco na linha o botão está visível e acessível pelo teclado.
                .tab_stop(focused)
                .accessibility_label(tr("sidebar_options").replace("{n}", &name)).tooltip(tr("sidebar_options_tip"))
                .dropdown_menu_with_anchor(Anchor::TopRight, sidebar::session_menu(weak.clone(), target.server.clone(), session.clone()))
                .on_open_change(move |open, _, cx| { let _ = weak.update(cx, |this, cx| {
                    let hover = this.sidebar.hover.take();
                    this.button_menu(target.clone(), *open, cx);
                    this.sidebar.hover = hover;
                }); })
        };
        let title = div().w_full().min_w_0().h(px(17.)).flex().items_center().gap(px(4.))
            .child(status)
            .children(self.session_number_badge(&target))
            .when(!session.orq(), |el| el.child(badge(agent_name(&session.provider).to_owned(), theme::muted())))
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).line_height(px(17.)).child(name.clone()))
            .when(session.pending_questions > 0, |el| el.child(div().flex_shrink_0().text_xs().text_color(theme::warning())
                .child(format!("? {}", session.pending_questions))))
            .when(session.tracked == Some(false), |el| el.child(badge(tr("untracked_badge"), theme::muted())))
            .when(session.orq(), |el| el.child(badge(tr_shared("orq_row_badge", &[]), theme::muted())))
            .when_some(session.owner.clone(), |el, owner| el.child(badge(format!("👤 {owner}"), theme::muted())))
            .child(div().w(px(21.)).h(px(17.)).flex_shrink_0().flex().items_center()
                .when(show_menu, |el| el.child(menu()))).child(time);
        let (open, menu_target, click_target) = (target.clone(), target.clone(), target.clone());
        let hover_target = target.clone();
        let row_id = format!("conversation-row-{row_key}");
        div().id(SharedString::from(row_id)).relative().flex_shrink_0()
            .h(px(if second_line { 45. } else { 29. }))
            .px(px(8.)).py(px(6.)).flex().flex_col().gap(px(2.)).rounded(px(8.)).text_color(theme::text())
            .when_some(focus, |el, focus| el.track_focus(focus))
            .when(focus.is_some_and(|f| f.is_focused(window)), |el| el.focus_ring_style(window, cx))
            .when(selected, |el| el.bg(selection))
            .when(!selected, |el| el.hover(|el| el.bg(hover)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered { this.sidebar.hover = Some(hover_target.clone()); }
                else if this.sidebar.hover.as_ref() == Some(&hover_target) { this.sidebar.hover = None; }
                this.redraw(panes::Area::Nav, cx);
            }))
            .role(Role::Button).aria_selected(selected).aria_label(label.clone())
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx))
            .child(title)
            // Recuo do estado mais o vão: a branch começa embaixo do glifo.
            .when(second_line, |el| el.child(div().w_full().min_w_0().h(px(14.)).pl(px(17.)).flex().items_center().gap(px(4.))
                .text_size(px(11.)).line_height(px(14.)).text_color(theme::muted())
                .when_some(branch, |el, branch| el
                    .child(chrome::small_icon(IconName::GitBranch, 12., theme::muted()))
                    .child(div().flex_1().min_w_0().truncate().child(branch)))
                .when_some(worktree, |el, label| {
                    el.child(div().id(SharedString::from(format!("wt-chip-{row_key}"))).cursor_pointer()
                        .flex().items_center().gap(px(3.)).text_color(theme::accent())
                        .child(chrome::small_icon(IconName::GitBranch, 11., theme::accent()))
                        .child(if merged { format!("{label} ✓") } else { label })
                        .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.open_worktree(worktree_target.clone(), window, cx); })))
                })
                .when(worktree_gone, |el| el.child(div().text_color(theme::muted()).child(tr_shared("worktree_apagada", &[]))))))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !matches!(event.keystroke.key.as_str(), "enter" | "space") || !this.row_focus(&open).is_some_and(|f| f.is_focused(window)) { return; }
                this.select_target(&open, window, cx);
                cx.stop_propagation();
            }))
            .map(|el| self.group_row(el, &session, remote.as_deref(), 8., cx))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| {
                let hover = this.sidebar.hover.take();
                this.start_menu(menu_target.clone(), cx);
                this.sidebar.hover = hover;
            }))
            // O ⋯ do Zeron segue o ponteiro: abrir não o apaga.
            .on_click(cx.listener(move |this, _, window, cx| {
                let hover = this.sidebar.hover.take();
                this.open_target(&click_target, window, cx);
                this.sidebar.hover = hover;
            }))
            .context_menu(sidebar::session_menu(cx.entity().downgrade(), target.server, session)).into_any_element()
    }

    /// Linha do web (Sidebar.svelte): a marca tingida pelo estado no lugar do avatar; nome com a conta e a hora da última
    /// resposta; embaixo, a resposta com ◆, a pergunta ou o que está fazendo; por fim a pasta (lista por servidor ou
    /// worktree), a branch fora de main/master, o ↑/↓ do upstream e o diff.
    /// Mais, como o web: "? N" das perguntas, o ⋯ e o clique direito com o menu da sessão, pressionar 500 ms para renomear
    /// na própria linha e a prévia da última resposta ao parar o mouse. A linha entra no Tab (Enter abre) e o ⋯ vem depois dela.
    fn render_session_row(&self, session: SessionInfo, selected: bool, remote: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // O orquestrador não roda agente: selo de provider nele seria mentira.
        // Sessão de motor roda outro modelo pelo Claude Code: o selo diz o motor, como o chip ⚙ do web.
        let engine = session.engine.clone().filter(|e| !e.is_empty() && session.provider == "claude");
        let on_engine = engine.is_some();
        let provider_label = (!session.orq()).then(|| engine.map_or_else(|| agent_name(&session.provider).to_owned(), |e| format!("⚙ {e}")));
        let target = sidebar::Target::new(&remote.clone().unwrap_or_else(|| self.active_key()), &session.name);
        // Ids com a máquina: a de mesmo nome em outra máquina não divide marca, menu nem selos.
        let row_key = if remote.is_some() { target.id() } else { session.name.clone() };
        let state = session.state.as_str();
        let limited = session.limited == Some(true);
        let untracked = session.tracked == Some(false);
        let mark_color = if limited { theme::limited() } else { theme::status(state) };
        // Trabalhando, a marca é pintada fora da lista guardada: a batida não redesenha a lista.
        let working = state == "working" && !limited;
        let mark = if working {
            self.nav_mark(format!("row-mark-{row_key}"), 18., mark_color, None)
        } else { chrome::hangar_mark(18., mark_color).into_any_element() };
        let avatar = div().relative().size(px(18.)).flex_shrink_0().flex().items_center().justify_center().child(mark);
        let state_label = tr(&format!("chip_{}", if limited { "limited" } else { state }));
        let reply = session.last_reply.as_deref().filter(|r| state == "idle" && !r.trim().is_empty());
        let asking = state == "awaiting_input" || session.pending_questions > 0;
        let fresh = match reply {
            Some(r) => Some((conversation::one_line(r, 120), sidebar::Sub::Reply)),
            None if asking => session.question.clone().map(|q| (conversation::one_line(&q, 80), sidebar::Sub::Question)),
            None if state == "working" => session.label.clone().filter(|l| !l.trim().is_empty())
                .map(|l| (conversation::one_line(l.split(" (").next().unwrap_or(&l), 80), sidebar::Sub::Working)),
            None => None,
        };
        let sub = self.sidebar.keep_sub(&target, session.jsonl.as_deref(), fresh);
        // Âmbar só com a pergunta aberta agora: a guardada de antes, já respondida, fica na cor de sempre.
        let sub_color = if asking && matches!(sub, Some((_, sidebar::Sub::Question))) { theme::warning() } else { theme::muted() };
        let when = session.last_reply_at.filter(|_| state == "idle").map(side::since);
        let account = account_chip(session.conta.as_deref());
        // Como o web: a pasta só com a lista por servidor (por projeto o cabeçalho já a diz), e sempre na worktree.
        let folder = folder_name(&session).filter(|_| session.worktree == Some(true) || (self.multi_server() && !Self::by_project()));
        let branch = shown_branch(&session);
        let (added, removed) = (session.git_added.filter(|n| *n > 0), session.git_removed.filter(|n| *n > 0));
        let (ahead, behind) = (session.git_ahead.filter(|n| *n > 0), session.git_behind.filter(|n| *n > 0));
        let sync_title = tr("git_sync_title").replace("{ahead}", &ahead.unwrap_or(0).to_string()).replace("{behind}", &behind.unwrap_or(0).to_string());
        let name = session.name.clone();
        let questions = session.pending_questions;
        let editing = self.sidebar.editing.as_ref().filter(|e| e.target == target).map(|e| e.input.clone());
        let focus = self.row_focus(&target).cloned();
        let focused = focus.as_ref().is_some_and(|f| f.contains_focused(window, cx));
        let hovered = self.sidebar.hover.as_ref() == Some(&target);
        let weak = cx.entity().downgrade();
        let menu_open = self.sidebar.button_menu.as_ref() == Some(&target);
        let show_menu = selected || hovered || focused || menu_open;
        // O ⋯ mora no lugar do tempo: com ele à vista, o tempo sai (senão sobra um pedaço do "12m" atrás dele).
        let when = when.filter(|_| !show_menu);
        let menu_button = show_menu.then(|| {
            let (weak, target) = (weak.clone(), target.clone());
            div().absolute().top(px(5.)).right(px(6.)).rounded(px(6.)).bg(if selected { theme::selected_row() } else { theme::hover() })
                .child(Button::new(SharedString::from(format!("row-menu-{row_key}"))).ghost().xsmall().icon(IconName::Ellipsis)
                    .accessibility_label(tr("sidebar_options").replace("{n}", &name)).tooltip(tr("sidebar_options_tip"))
                    .dropdown_menu_with_anchor(Anchor::TopRight, sidebar::session_menu(weak.clone(), target.server.clone(), session.clone()))
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
                .when(session.headless, |el| el.child(div().id(SharedString::from(format!("row-headless-{row_key}"))).flex_shrink_0().flex().opacity(0.72)
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(tr("create_mode_headless")).build(window, cx))
                    .child(chrome::no_terminal_mark(12., theme::muted()))))
                .when(session.orq(), |el| el.child(badge(tr_shared("orq_row_badge", &[]), theme::muted())))
                .when(session.shared, |el| el.child(div().id(SharedString::from(format!("row-shared-{row_key}"))).flex_shrink_0().flex()
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(tr_shared("sessao_compartilhada", &[])).build(window, cx))
                    .child(chrome::small_icon(IconName::Link, 12., theme::accent()))))
                .when_some(session.owner.clone(), |el, owner| el.child(badge(format!("👤 {owner}"), theme::muted())))
                .when(questions > 0, |el| el.child(div().flex_shrink_0().text_xs().text_color(theme::warning()).child(format!("? {questions}"))))
                .when(untracked, |el| el.child(badge(tr("untracked_badge"), theme::faint()))).into_any_element(),
        };
        let (hover_target, press_target, menu_target, key_open, click_target) = (target.clone(), target.clone(), target.clone(), target.clone(), target.clone());
        let row_id = row_key.clone();
        let mut spoken = vec![name.clone()];
        if let Some(label) = &provider_label { spoken.push(label.clone()); }
        spoken.push(state_label);
        if session.headless { spoken.push(tr("create_mode_headless")); }
        div().id(SharedString::from(row_id)).relative().flex_shrink_0().flex().flex_col().gap(px(1.)).px(px(8.)).py(px(7.)).rounded(px(10.))
            .when_some(focus.as_ref(), |el, focus| el.track_focus(focus))
            .when(focus.as_ref().is_some_and(|f| f.is_focused(window)), |el| el.focus_ring_style(window, cx))
            .when(selected, |el| el.bg(theme::selected_row()))
            .when(!selected, |el| el.hover(|el| el.bg(theme::hover())))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.row_hover(hover_target.clone(), *hovered, cx)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| this.row_pointer(f32::from(event.position.y))))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| this.row_press(press_target.clone(), window, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.row_release()))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.start_menu(menu_target.clone(), cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                // Só a própria linha: Enter no ⋯ ou no campo do nome é deles.
                if !matches!(event.keystroke.key.as_str(), "enter" | "space") || !this.row_focus(&key_open).is_some_and(|f| f.is_focused(window)) { return; }
                this.select_target(&key_open, window, cx);
                cx.stop_propagation();
            }))
            .role(Role::Button).aria_selected(selected)
            .aria_label(self.session_number_label(&target, spoken.join(" · ")))
            // O ⋯ fica por cima do fim da linha do nome: ela cede o espaço dele.
            .child(div().flex().items_center().gap(px(8.)).when(show_menu, |el| el.pr(px(22.)))
                .child(avatar)
                .children(self.session_number_badge(&target))
                .child(div().flex_1().min_w_0().flex().items_center().gap(px(6.)).when(untracked, |el| el.opacity(0.45)).child(name_el))
                .when_some(account, |el, (label, color)| el.child(div().flex_shrink_1().min_w_0().max_w(px(96.)).h(px(16.)).px(px(6.))
                    .flex().items_center().gap(px(4.)).rounded_full().bg(theme::hover())
                    .text_size(px(10.)).font_weight(FontWeight::MEDIUM).text_color(theme::muted())
                    .child(div().size(px(5.)).flex_shrink_0().rounded_full().bg(color))
                    .child(div().min_w_0().truncate().child(label))))
                .when_some(when, |el, w| el.child(div().flex_shrink_0().text_size(px(10.)).text_color(theme::faint()).child(w))))
            .when_some(sub, |el, (text, kind)| el.child(div().flex().items_center().gap(px(8.)).child(lane())
                .child(div().flex_1().min_w_0().flex().items_center().gap(px(4.)).text_xs().text_color(sub_color)
                    .when(kind == sidebar::Sub::Reply, |el| el.child(div().flex_shrink_0().text_size(px(8.)).text_color(theme::faint()).child("◆")))
                    .child(div().min_w_0().truncate().when(kind == sidebar::Sub::Working && working, |el| el.italic()).child(text)))))
            .when(provider_label.is_some() || folder.is_some() || branch.is_some() || added.is_some() || removed.is_some() || ahead.is_some() || behind.is_some(), |el| el.child(div().flex().items_center().gap(px(8.))
                .text_size(px(11.5)).text_color(theme::faint()).child(lane())
                .child(div().flex_1().min_w_0().flex().items_center().gap(px(6.))
                    .when_some(provider_label, |el, label| el.child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD)
                        .text_color(if on_engine { theme::accent() } else { theme::muted() }).child(label)))
                    .when_some(folder, |el, f| el.child(chrome::small_icon(IconName::Folder, 12., theme::faint()))
                        .child(div().min_w_0().truncate().child(f)))
                    .when_some(branch, |el, b| el.child(chrome::small_icon(IconName::GitBranch, 12., theme::faint()))
                        .child(div().min_w_0().truncate().child(b)))
                    // Zero não desenha: a ausência da seta é "em dia".
                    .when(ahead.is_some() || behind.is_some(), |el| el.child(div().id(SharedString::from(format!("row-sync-{row_key}")))
                        .flex_shrink_0().flex().gap(px(4.)).font_weight(FontWeight::SEMIBOLD)
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(sync_title.clone()).build(window, cx))
                        .when_some(ahead, |el, n| el.child(div().text_color(theme::accent()).child(format!("↑{n}"))))
                        .when_some(behind, |el, n| el.child(div().text_color(theme::warning()).child(format!("↓{n}"))))))
                    .when_some(added, |el, a| el.child(div().flex_shrink_0().text_color(theme::success()).child(format!("+{a}"))))
                    .when_some(removed, |el, r| el.child(div().flex_shrink_0().text_color(theme::removed()).child(format!("−{r}")))))))
            .children(menu_button)
            .map(|el| self.group_row(el, &session, remote.as_deref(), 10., cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.hide_preview();
                if this.take_long_press() { return; }
                // Foco só no gesto sobre a lista; troca automática de transcript não tira o foco de ninguém.
                this.open_target(&click_target, window, cx);
            }))
            .context_menu(sidebar::session_menu(weak, target.server, session))
            .into_any_element()
    }
}

fn conversation_row_state(session: &SessionInfo, outcome: Option<&SendOutcome>, queued: bool) -> &'static str {
    if matches!(outcome, Some(SendOutcome::Rejected(_))) { "failed" }
    else if session.problema.as_ref().is_some_and(|problem| !problem.trim().is_empty()) { "problem" }
    else if session.limited == Some(true) { "limited" }
    else if matches!(outcome, Some(SendOutcome::Uncertain)) { "uncertain" }
    else if queued { "queued" }
    else { match session.state.as_str() {
        "working" => "working", "awaiting_input" => "input", "idle" => "idle", "dead" => "dead", _ => "unknown",
    } }
}

fn kind_of<'a>(items: &[Item], index: usize, events: &'a [ChatEvent]) -> Option<&'a str> {
    match items.get(index) { Some(Item::Event(i)) => events.get(*i).map(|e| e.kind.as_str()), _ => None }
}

/// A pasta onde o agente trabalha: a worktree para onde ele foi, ou a de abertura.
fn folder_name(session: &SessionInfo) -> Option<String> {
    session.git_dir().map(composer::basename).filter(|f| !f.is_empty() && *f != "/").map(str::to_owned)
}

/// A branch que a linha mostra: main e master são o normal e ficam de fora, como no web.
fn shown_branch(session: &SessionInfo) -> Option<String> {
    session.branch.clone().filter(|b| !b.is_empty() && b != "main" && b != "master")
}

/// Nome curto do chip: a pasta da worktree onde o agente está.
fn worktree_label(session: &SessionInfo) -> Option<String> {
    let path = session.worktree_path.clone()
        .or_else(|| session.worktree.unwrap_or(false).then(|| session.cwd.clone()).flatten())?;
    std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Selo da conta do `chipDaConta` do web (lib/conta.ts): o nome é o sufixo da pasta, a pasta sem sufixo é a padrão,
/// e a cor sai do nome da pasta, com as mesmas seis tintas. Motor (`chave:`) não tem selo.
fn account_chip(conta: Option<&str>) -> Option<(String, Hsla)> {
    const TINTS: [u32; 6] = [0x80bbc3, 0xd3a781, 0x97c69d, 0xcd99b9, 0x9ba9d9, 0xccbf87];
    let conta = conta?;
    let harness = ["claude", "codex"].into_iter().find(|h| conta.starts_with(&format!("{h}:")))?;
    let base = conta[harness.len() + 1..].trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("");
    let own = if base == format!(".{harness}") { "" } else { base.strip_prefix(&format!(".{harness}-")).unwrap_or(base) };
    let label = own.strip_prefix(&format!("{harness}-")).filter(|rest| !rest.is_empty()).unwrap_or(own);
    let label = if label.is_empty() { crate::i18n::tr_web("conta_padrao", &HashMap::new()).unwrap_or_default() } else { label.to_owned() };
    let hash = base.chars().fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c.encode_utf16(&mut [0; 2])[0] as u32));
    Some((label, rgb(TINTS[hash as usize % TINTS.len()]).into()))
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
    Some(appearance::dir()?.join("connection.json"))
}

/// Onde o diálogo de salvar abre: a pasta de downloads, ou a casa do usuário.
fn downloads_folder() -> PathBuf {
    // `home_dir` e não `HOME`: o Windows não define `HOME`, e o diálogo abria no %TEMP%.
    std::env::var_os("XDG_DOWNLOAD_DIR").map(PathBuf::from).filter(|p| p.is_dir())
        .or_else(|| std::env::home_dir().map(|home| home.join("Downloads")).filter(|p| p.is_dir()))
        .or_else(std::env::home_dir).unwrap_or_else(std::env::temp_dir)
}

fn load_connection() -> Option<(String, String)> {
    let value: Value = serde_json::from_slice(&std::fs::read(saved_connection_path()?).ok()?).ok()?;
    let (address, token) = (value.get("address")?.as_str()?, value.get("token")?.as_str()?);
    (!address.is_empty() && !token.is_empty()).then(|| (address.to_owned(), token.to_owned()))
}

/// As máquinas conhecidas, gravadas junto da conexão ativa (o `cp_servers` do web).
/// `None` é o arquivo de antes da lista (ou nenhum): ainda não houve escolha de máquinas.
fn load_servers() -> Option<Vec<servers::ServerEntry>> {
    let value: Value = serde_json::from_slice(&std::fs::read(saved_connection_path()?).ok()?).ok()?;
    serde_json::from_value(value.get("servers")?.clone()).ok()
}

fn save_connection(address: &str, token: &str, servers: &[servers::ServerEntry]) -> std::io::Result<()> {
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
    std::io::Write::write_all(&mut options.open(&tmp)?, json!({"address": address, "token": token, "servers": servers}).to_string().as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// Fração da altura da janela que um painel de mod acima da faixa pode ocupar.
const PLUGIN_PANE_MAX_SHARE: f32 = 0.45;

fn select_snapshot(state: &SessionState) -> String { json!([state.question, state.options]).to_string() }

fn display_body(event: &ChatEvent) -> String {
    match event.kind.as_str() {
        // Skill injetada: o corpo é o SKILL.md, que a linha recolhida só mostra ao abrir.
        "notice" => event.loaded_skill().map(|skill| skill.body).unwrap_or_else(|| tr(&event.body())),
        "assistant_msg" => interaction::plan_display(&event.body()),
        // Anexos viram cartões próprios; o texto mostra só a legenda.
        "user_msg" => {
            let body = event.body();
            if let Some(cards::Card::Codex(card)) = message_card(event) { return card.report; }
            let caption = composer::parse_marked(&body).map(|m| m.caption).unwrap_or(body);
            // Recado de outra sessão: remetente e etiqueta vão para o chip da bolha.
            cards::peer_message(&caption).map(|peer| peer.body).unwrap_or(caption)
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
/// Prazo do cache de prompt (o `cache-chip` do web): ponto verde e minutos em mono; âmbar no fim do prazo, ponto apagado
/// ao expirar. Não é botão: não há o que fazer com ele além de saber.
fn cache_chip(cache: crate::chat::LastCache) -> impl IntoElement {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.);
    let (_, ending, label) = crate::chat::cache_left(cache, now);
    let web = |key: &str, params: &[(&str, String)]| crate::i18n::tr_web(key, &params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
        .unwrap_or_else(|| key.to_owned());
    let tip = match &label {
        Some(label) => web("composer_cache_vale", &[("label", label.clone()),
            ("janela", web(if cache.ttl >= 3600 { "composer_cache_1_hora" } else { "composer_cache_5_min" }, &[]))]),
        None => web("composer_cache_expirou", &[]),
    };
    let (ink, dot) = match (&label, ending) {
        (None, _) => (theme::muted(), theme::muted().opacity(0.5)),
        (Some(_), true) => (theme::warning(), theme::warning()),
        (Some(_), false) => (theme::muted(), theme::success()),
    };
    div().id("composer-cache").flex_shrink_0().h(px(22.)).px(px(6.)).flex().items_center().gap(px(4.))
        .font_family(theme::MONO).text_xs().text_color(ink)
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        .child(div().size(px(6.)).rounded_full().bg(dot))
        .child(label.unwrap_or_else(|| web("composer_expirou", &[])))
}

/// "Trabalhou por 7s · 14:59"; conversa aberta no meio do turno não sabe quando ele começou, e fica só a hora.
fn turn_done_text(start: Option<Instant>) -> String {
    let now = chrono::Local::now();
    let clock = clock(Some(now.timestamp() as f64)).unwrap_or_default();
    match start {
        Some(start) => tr("turn_done").replace("{time}", &chrome::format_elapsed(start.elapsed())).replace("{clock}", &clock),
        None => tr("turn_done_clock").replace("{clock}", &clock),
    }
}

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

fn toggle_detail(expanded: &mut HashSet<String>, prepared: &mut HashMap<String, Prepared>, key: &str) {
    if expanded.remove(key) {
        prepared.remove(&format!("{key}:input"));
        prepared.remove(&format!("{key}:result"));
    } else { expanded.insert(key.to_owned()); }
}

fn prepare_detail(full: String) -> Prepared {
    let total = full.chars().count();
    let (shown, clipped) = conversation::clip(&full, DETAIL_MAX);
    let fenced = conversation::fenced(shown);
    Prepared::Detail { fenced, total, clipped, full: full.into() }
}

fn prepare_message(event: &ChatEvent, dead: &HashSet<String>) -> Prepared {
    let card = message_card(event);
    let body = display_body(event);
    // Recado de outra sessão também é escrito por agente: cita arquivo do mesmo jeito que a resposta.
    let cites = event.kind == "assistant_msg" || peer_of(event).is_some();
    let source = if cites { composer::citation_markdown_with(&body, &|p| dead.contains(p)) } else { body.clone() };
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

/// A contagem de tokens dos parênteses do spinner ("… (37s · ↓ 1.4k tokens · thought for 5s)" → "↓ 1.4k tokens").
fn working_tokens(label: Option<&str>) -> Option<String> {
    let inside = label?.split_once(" (")?.1.trim_end_matches(')');
    inside.split(" · ").map(str::trim).find(|part| part.starts_with(['↑', '↓']) && part.ends_with("tokens")).map(str::to_owned)
}

fn preview_source(preview: &Preview) -> String {
    if preview.md { return safe_markdown(&composer::citation_markdown(&crate::mend::close_hanging(&interaction::plan_display(&preview.text)))); }
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
    /// Barra lateral ou faixa de abas (em cima ou embaixo), conforme Aparência.
    fn render_nav(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let selected_name = self.selected.as_ref().map(|s| s.name.clone());
        if appearance::get().navigation.tabs() { self.render_tabs(selected_name.as_deref(), window, cx) }
            else { self.render_sidebar(selected_name.as_deref(), window, cx) }
    }

    /// Conversa sem mensagens: a marca do harness, o título e o convite, no meio da área.
    fn render_empty_chat(&self) -> Div {
        let provider = self.provider().0;
        let (color, glyph) = theme::provider(provider);
        // Superfície dos menus por baixo da marca: sobre o papel de parede a cor do harness sozinha some.
        let mark = div().size(px(48.)).rounded(px(14.)).bg(theme::popup_fill(theme::raised())).border_1().border_color(theme::glass_border())
            .shadow(theme::popover_shadow()).flex().items_center().justify_center().text_color(color)
            .map(|el| match chrome::provider_logo(provider, 26., color) {
                Some(logo) => el.child(logo),
                None => el.text_size(px(20.)).font_weight(FontWeight::BOLD).child(glyph),
            });
        // Sem compositor, o convite a escrever vira o caminho até quem recebe.
        let hint = if self.selected.as_ref().is_some_and(SessionInfo::orq) { tr_shared("erro_sessao_orq", &[]) }
            else { tr("empty_chat_hint").replace("{agent}", agent_name(provider)) };
        div().flex_1().px_6().pb_6().flex().flex_col().items_center().justify_center().gap_3()
            .child(mark)
            .child(div().flex().flex_col().items_center().gap_1()
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(tr("empty_chat")))
                .child(div().max_w(px(360.)).text_sm().text_center().text_color(theme::muted())
                    .child(hint)))
    }

    /// Rodapé da linha `orq`, como o do web: sem campo de digitar, o selo e o botão que abre o árbitro da orquestração.
    fn render_orq_footer(&self, orq: &str, cx: &mut Context<Self>) -> Div {
        // A linha `orq` aberta é da máquina da sessão aberta, e o árbitro também.
        let target = sidebar::Target::new(&self.open_server(), orq);
        div().w_full().py_3().flex().flex_col().items_center().gap_2()
            .child(div().text_sm().text_color(theme::muted()).child(tr_shared("orq_row_badge", &[])))
            .child(Button::new("orq-talk-to-arbiter").small().label(tr_shared("orq_talk_to_arbiter", &[]))
                .disabled(self.arbiter_of(&target).is_none())
                .on_click(cx.listener(move |this, _, window, cx| this.open_arbiter(&target, window, cx))))
    }

    /// Lido da lista atual da máquina dela, não da linha guardada ao abrir: a sucessão troca o árbitro sem trocar a linha `orq`.
    fn arbiter_of(&self, orq: &sidebar::Target) -> Option<&SessionInfo> {
        let list = self.sessions_of(&orq.server);
        list.iter().find(|s| s.name == orq.name).and_then(|s| s.arbiter(list))
    }

    /// Abre o árbitro com o campo focado, como o clique na linha dele.
    fn open_arbiter(&mut self, orq: &sidebar::Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(arbiter) = self.arbiter_of(orq) else { return };
        let target = sidebar::Target::new(&orq.server, &arbiter.name);
        self.open_target(&target, window, cx);
    }

    /// Foco no campo depois de escolher uma conversa, só se ela tem compositor desenhado.
    fn focus_composer_for(&mut self, session: &SessionInfo, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connection_dialog && session.takes_messages() { self.composer.update(cx, |input, cx| input.focus(window, cx)); }
    }

    /// Entre o cabeçalho e a faixa de baixo: o cartão de antes da conversa, a lista ou o aviso de vazio.
    fn render_conversation_area(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // A miniatura só conta como vista quando a conversa é desenhada: guardada entre quadros, ela segue na tela.
        self.media.next_frame();
        api::open_trace(|| format!("conversation render rows={} loading={}", self.row_ids.len(), self.loading));
        if api::open_trace_on() { window.on_next_frame(|_, _| api::open_trace(|| "next frame".into())); }
        let mut content = div().size_full().flex().flex_col();
        let prethread = self.render_prethread(cx);
        if let Some(selected) = &self.selected {
            if let Some(card) = prethread {
                content = content.child(card);
            } else if !selected.readable() {
                content = content.child(div().flex_1().p_6().text_color(theme::muted()).child(tr(if selected.tracked == Some(false) { "untracked" } else { "starting" })));
            } else {
                if self.row_ids.is_empty() && self.loading {
                    content = content.child(render_history_skeleton());
                } else if self.has_older || self.loading {
                    content = content.child(in_column(div().py_2().flex().gap_2().items_center()
                        .when(self.has_older, |el| el.child(Button::new("older").small().outline().label(tr("older")).disabled(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.load_older(cx)))))
                        .when(self.has_older, |el| el.child(div().text_xs().text_color(theme::muted()).child(format!("{} {}", tr("history_window"), self.history_limit))))
                        .when(self.loading, |el| el.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))))));
                }
                if self.row_ids.is_empty() && !self.loading && self.error.is_none() {
                    content = content.child(self.render_empty_chat());
                } else if !self.row_ids.is_empty() || !self.loading {
                    let view = cx.entity().downgrade();
                    // Leitura Folha: uma folha da largura da coluna atrás das mensagens, com o fundo nas margens.
                    let sheet = (appearance::get().effective_reading() == appearance::Reading::Sheet).then(|| div().absolute().inset_0()
                        .px(px(16.)).pt(px(4.)).pb(px(8.)).flex().justify_center()
                        .child(column_box(false).h_full().rounded(px(14.)).border_1().border_color(theme::border())
                            .bg(theme::sheet()).shadow(theme::sheet_shadow())));
                    content = content.child(div().relative().flex_1().min_h_0().flex().flex_col()
                        .children(sheet)
                        .child(page_card::Paint::edge(&self.pages.paint, true))
                        .child(list(self.list_state.clone(), move |i, window, cx| {
                            view.update(cx, |this, cx| this.render_row(i, window, cx)).unwrap_or_else(|_| div().into_any_element())
                        }).flex_1().min_h_0())
                        .child(page_card::Paint::edge(&self.pages.paint, false))
                        .child(self.wheel_layer(cx))
                        .children(self.render_rail(cx))
                        .when(self.follow_detached(), |el| el.child(self.render_jump_pill(cx)))
                        .children(self.render_find(cx)));
                    self.schedule_scroll(window, cx);
                }
            }
        } else { content = content.child(self.render_new_chat(window, cx)); }
        content.into_any_element()
    }

    /// Quem atende o clique num botão de mod; sessão só leitura deixa os botões como rótulo.
    fn plugin_press(&self, cx: &mut Context<Self>) -> Option<crate::plugin_ui::Press> {
        if self.selected.as_ref().is_some_and(|s| s.read_only()) { return None; }
        let view = cx.entity().downgrade();
        Some(std::rc::Rc::new(move |site: &str, button: &crate::plugin_ui::Control, _: &mut Window, cx: &mut App| {
            let (site, button) = (site.to_owned(), button.clone());
            let _ = view.update(cx, |this, cx| this.press_plugin(site, button, cx));
        }))
    }

    /// Quem atende o `✕` de um painel de mod; sessão só leitura fica sem ele.
    fn plugin_close(&self, cx: &mut Context<Self>) -> Option<crate::plugin_ui::Close> {
        if self.selected.as_ref().is_some_and(|s| s.read_only()) { return None; }
        let view = cx.entity().downgrade();
        Some(std::rc::Rc::new(move |site: &str, _: &mut Window, cx: &mut App| {
            let site = site.to_owned();
            let _ = view.update(cx, |this, cx| this.close_plugin(site, cx));
        }))
    }

    /// Chama uma rota `plugin/<ação>` da sessão aberta; a resposta volta como o `Payload` que `wrap` monta.
    fn spawn_plugin(&self, action: &'static str, body: Value, wrap: impl FnOnce(Result<Value, Failure>) -> Payload + Send + 'static) {
        let (Some(api), Some(session)) = (self.session_api(), self.selected.clone()) else { return };
        let (connection, selection, tx) = (self.connection, self.selection, self.tx.clone());
        self.runtime.spawn(async move {
            let mut result = api.act(&session.name, &["plugin", action], Some(body.clone()), false, 10).await;
            if let Err(error) = &result
                && let Some((retry, older)) = crate::plugin_ui::older_server_retry(action, &body, error.status) {
                result = api.act(&session.name, &["plugin", retry], Some(older), false, 10).await;
            }
            let _ = tx.send(Envelope { connection, selection: Some(selection), payload: wrap(result) }).await;
        });
    }

    fn press_plugin(&mut self, site: String, button: crate::plugin_ui::Control, cx: &mut Context<Self>) {
        self.spawn_plugin("press", json!({"site": site, "plugin": button.plugin, "key": button.key}), Payload::PluginPressed);
        cx.notify();
    }

    fn close_plugin(&mut self, site: String, cx: &mut Context<Self>) {
        // O `✕` tira o painel da tela: o hover dele sai junto, sem esperar o evento que confirma o fechamento.
        self.keep_plugin_hovered_without(Some(&site));
        self.spawn_plugin("close", json!({"site": site}), Payload::PluginClosed);
        cx.notify();
    }

    /// Hover dos mods depois de um evento novo ou de uma troca de aba: fica só o trecho ainda desenhado na faixa ou no
    /// painel da frente.
    fn keep_plugin_hovered(&mut self) { self.keep_plugin_hovered_without(None); }

    /// Como `keep_plugin_hovered`, tirando o painel `gone` (fechado pelo `✕`) dos lugares à vista.
    fn keep_plugin_hovered_without(&mut self, gone: Option<&str>) {
        if self.plugin_hovered.is_empty() { return; }
        let ids = crate::plugin_ui::pane_ids(&self.plugin_panes);
        let active = crate::plugin_ui::active_pane(&ids, &self.plugin_shown, self.plugin_local_tab.as_deref()).filter(|id| Some(id.as_str()) != gone);
        let pane = active.as_deref()
            .and_then(|id| self.plugin_panes.iter().find(|p| p["id"].as_str() == Some(id)).map(|p| (id, &p["tree"])));
        let places: Vec<(&str, &Value)> = std::iter::once((crate::plugin_ui::BAND_SITE, &self.plugin_band)).chain(pane).collect();
        crate::plugin_ui::keep_hovered(&mut self.plugin_hovered, &places);
    }

    /// Hover dos mods: o app guarda os trechos com o ponteiro e redesenha só a área de baixo quando muda.
    fn plugin_hover(&self, cx: &mut Context<Self>) -> crate::plugin_ui::Hover {
        let view = cx.entity().downgrade();
        std::rc::Rc::new(move |id: &str, on: bool, _: &mut Window, cx: &mut App| {
            let id = id.to_owned();
            let _ = view.update(cx, |this, cx| {
                let changed = if on { this.plugin_hovered.insert(id) } else { this.plugin_hovered.remove(&id) };
                if changed { this.redraw(panes::Area::Bottom, cx); }
            });
        })
    }

    /// Os campos dos mods acompanham a árvore. O valor desenhado entra conforme o `FieldSync`: com a pessoa no campo ele
    /// fica pendente (um redesenho atrasado não apaga o que se digita) e entra quando o campo perde o foco; logo depois
    /// do envio, entra mesmo com foco.
    fn sync_plugin_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted: Vec<(String, String, crate::plugin_ui::FieldSpec)> =
            std::iter::once((crate::plugin_ui::BAND_SITE.to_owned(), &self.plugin_band))
                .chain(self.plugin_panes.iter().map(|p| (p["id"].as_str().unwrap_or("").to_owned(), &p["tree"])))
                .flat_map(|(site, tree)| crate::plugin_ui::fields(tree).into_iter()
                    .map(move |f| (crate::plugin_ui::field_id(&site, &f.control), site.clone(), f)))
                .collect();
        self.plugin_fields.retain(|id, _| wanted.iter().any(|(w, _, _)| w == id));
        for (id, site, spec) in wanted {
            if let Some(field) = self.plugin_fields.get_mut(&id) {
                // Este desenho só é novo para o campo uma vez por evento `plugin_ui`; os outros são redesenhos do app.
                let fresh = (field.seen != self.plugin_draws).then_some(spec.value.as_str());
                field.seen = self.plugin_draws;
                let (shown, focused) = {
                    let input = field.state.read(cx);
                    (input.value().to_string(), input.focus_handle(cx).is_focused(window))
                };
                // O texto de ajuda segue o mod; só é reposto quando mudou, porque o `set_placeholder` redesenha o campo.
                if field.placeholder != spec.placeholder {
                    field.placeholder = spec.placeholder.clone();
                    field.state.update(cx, |input, cx| input.set_placeholder(spec.placeholder, window, cx));
                }
                if let Some(value) = field.sync.draw(fresh, &shown, focused) {
                    field.state.update(cx, |input, cx| input.set_value(value, window, cx));
                }
                continue;
            }
            let state = cx.new(|cx| InputState::new(window, cx).placeholder(spec.placeholder.clone()).default_value(spec.value.clone()));
            let control = spec.control.clone();
            let changes = cx.subscribe_in(&state, window, move |this, input, event: &InputEvent, _, cx| {
                // A faixa de baixo é uma área guardada: sem redesenho, o valor pendente só entraria no próximo evento.
                if matches!(event, InputEvent::Blur) { this.redraw(panes::Area::Bottom, cx); return; }
                let Some(kind) = crate::plugin_ui::input_kind(event) else { return };
                let value = input.read(cx).value().to_string();
                this.input_plugin(&site, &control, kind, value);
            });
            let field = crate::plugin_ui::Field { state, sync: crate::plugin_ui::FieldSync::new(&spec.value), outbox: Default::default(),
                placeholder: spec.placeholder, seen: self.plugin_draws, _changes: changes };
            self.plugin_fields.insert(id, field);
        }
    }

    /// Digitação num `Input` de mod, só na sessão sem terminal e fora do só leitura. Todo `change` vai: o `set_value`
    /// que repõe o valor desenhado não emite `Change`, então o que chega aqui é a pessoa digitando. Sai pela fila do
    /// campo (`Outbox`): um pedido em voo por vez, para as teclas chegarem ao mod na ordem. Sem `notify`: nada do app
    /// muda, e cada tecla redesenharia a janela inteira; o campo se redesenha sozinho e o mod responde por evento.
    fn input_plugin(&mut self, site: &str, control: &crate::plugin_ui::Control, kind: &'static str, value: String) {
        let read_only = self.selected.as_ref().is_some_and(|s| s.read_only());
        if !crate::plugin_ui::accepts_typing(self.plugin_source, read_only) { return; }
        let Some(field) = self.plugin_fields.get_mut(&crate::plugin_ui::field_id(site, control)) else { return };
        if kind == "submit" { field.sync.submitted() } else { field.sync.typed(&value) }
        if let Some(request) = field.outbox.push(kind, value) { self.send_plugin_input(site, control, request); }
    }

    /// Manda à rota o pedido que a fila do campo liberou. Se a sessão deixou de aceitar digitação no meio, a fila acaba.
    fn send_plugin_input(&mut self, site: &str, control: &crate::plugin_ui::Control, (kind, value): crate::plugin_ui::InputRequest) {
        let read_only = self.selected.as_ref().is_some_and(|s| s.read_only());
        let Some(field) = self.plugin_fields.get_mut(&crate::plugin_ui::field_id(site, control)) else { return };
        let Some(body) = crate::plugin_ui::input_request(self.plugin_source, read_only, site, control, kind, &value) else {
            field.outbox = Default::default();
            return;
        };
        let (site, control, id) = (site.to_owned(), control.clone(), field.state.entity_id());
        self.spawn_plugin("input", body, move |result| Payload::PluginInput(site, control, id, result));
    }

    /// O rótulo de envio do `Input`: manda o que está no campo.
    fn submit_plugin_field(&mut self, site: &str, control: &crate::plugin_ui::Control, cx: &mut Context<Self>) {
        let Some(value) = self.plugin_fields.get(&crate::plugin_ui::field_id(site, control)).map(|f| f.state.read(cx).value().to_string()) else { return };
        self.input_plugin(site, control, "submit", value);
    }

    /// O que a faixa e os painéis dos mods precisam do app. A digitação só existe na sessão sem terminal e fora do só
    /// leitura; nas outras o campo aparece desabilitado.
    fn plugin_view(&self, cx: &mut Context<Self>) -> crate::plugin_ui::View<'_> {
        let entity = cx.entity().downgrade();
        let shown = entity.clone();
        let show: crate::plugin_ui::Show = std::rc::Rc::new(move |site: &str, _: &mut Window, cx: &mut App| {
            let site = site.to_owned();
            let _ = shown.update(cx, |this, cx| this.show_plugin(site, cx));
        });
        let typing = crate::plugin_ui::accepts_typing(self.plugin_source, self.selected.as_ref().is_some_and(|s| s.read_only()));
        let submit = typing.then(|| -> crate::plugin_ui::Submit {
            std::rc::Rc::new(move |site: &str, control: &crate::plugin_ui::Control, _: &mut Window, cx: &mut App| {
                let (site, control) = (site.to_owned(), control.clone());
                let _ = entity.update(cx, |this, cx| this.submit_plugin_field(&site, &control, cx));
            })
        });
        crate::plugin_ui::View { press: self.plugin_press(cx), close: (!self.plugin_panes.is_empty()).then(|| self.plugin_close(cx)).flatten(), show, tabs_scroll: &self.plugin_tabs_scroll, columns: self.plugin_columns,
            hover: Some(self.plugin_hover(cx)), hovered: &self.plugin_hovered, fields: &self.plugin_fields, submit }
    }

    /// Rola a fileira de abas até a ativa quando ela mudou. No primeiro quadro o handle ainda não tem geometria (ela só
    /// é gravada no prepaint): a aba fica pendente e o quadro seguinte tenta de novo, no máximo `TAB_SCROLL_WAITS` vezes.
    fn follow_plugin_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = crate::plugin_ui::pane_ids(&self.plugin_panes);
        let active = crate::plugin_ui::active_pane(&ids, &self.plugin_shown, self.plugin_local_tab.as_deref());
        let laid_out = self.plugin_tabs_scroll.bounds().size.width > px(0.);
        match crate::plugin_ui::tab_scroll_target(self.plugin_tabs_seen.as_deref(), &ids, active.as_deref(), laid_out) {
            crate::plugin_ui::TabScroll::To(ix) => {
                self.plugin_tabs_scroll.scroll_to_item(ix);
                self.plugin_tabs_seen = active;
                self.plugin_tabs_waits = 0;
            }
            crate::plugin_ui::TabScroll::Wait if self.plugin_tabs_waits < TAB_SCROLL_WAITS => {
                self.plugin_tabs_waits += 1;
                cx.on_next_frame(window, |_, _, cx| cx.notify());
            }
            _ => self.plugin_tabs_waits = 0,
        }
    }

    /// Troca de aba: seguindo o `shown_id`, a aba só muda quando o novo chega; sem ele (servidor antigo), a troca é
    /// local. O servidor é avisado nos dois casos, menos em sessão só leitura.
    fn show_plugin(&mut self, site: String, cx: &mut Context<Self>) {
        if !crate::plugin_ui::follows_server(&crate::plugin_ui::pane_ids(&self.plugin_panes), &self.plugin_shown) {
            self.plugin_local_tab = Some(site.clone());
            self.keep_plugin_hovered();
            cx.notify();
            self.redraw(panes::Area::Bottom, cx);
        }
        if self.selected.as_ref().is_some_and(|s| s.read_only()) { return; }
        self.spawn_plugin("show", json!({"site": site}), Payload::PluginShown);
    }

    // O que o mod copiou ou mandou abrir acontece aqui, na máquina de quem clicou, e não na do terminal.
    fn receive_plugin_press(&mut self, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            Ok(reply) => {
                if let Some(text) = reply.get("copied").and_then(Value::as_str) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
                    window.push_notification(Notification::info(tr_shared("plugin_copiado", &[])), cx);
                }
                if let Some(url) = crate::plugin_ui::safe_href(&reply["opened"]) { cx.open_url(&url); }
            }
            Err(error) => window.push_notification(Notification::warning(Self::press_failure(&error)), cx),
        }
    }

    /// Aviso (`$.ui.toast`) de um mod: o terminal o desenha por alguns segundos e ele não entra na conversa.
    fn show_plugin_toast(&mut self, data: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(toast) = crate::plugin_ui::toast(data) else { return };
        // A reconexão do SSE repõe os avisos ainda vivos: o id diz quais já passaram por aqui.
        if self.plugin_toasts_seen.contains(&toast.id) { return; }
        if self.plugin_toasts_seen.len() >= 200 { self.plugin_toasts_seen.pop_front(); }
        self.plugin_toasts_seen.push_back(toast.id.clone());
        let key = SharedString::from(toast.id);
        // No máximo 4 na tela: um mod insistente não cobre a conversa.
        while self.plugin_toasts_shown.len() >= 4 {
            if let Some(old) = self.plugin_toasts_shown.pop_front() { window.remove_notification1::<PluginToast>(old, cx); }
        }
        self.plugin_toasts_shown.push_back(key.clone());
        // Sem o autohide: ele é fixo em 5 s, e o prazo do aviso é o que o mod pediu.
        let note = Notification::info(toast.text).id1::<PluginToast>(key.clone()).autohide(false);
        window.push_notification(if toast.plugin.is_empty() { note } else { note.title(toast.plugin) }, cx);
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(toast.timeout).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.plugin_toasts_shown.retain(|k| *k != key);
                window.remove_notification1::<PluginToast>(key, cx);
            });
        }).detach();
    }

    /// Cartões, faixas e avisos entre a conversa e o compositor, e o compositor.
    fn render_bottom_area(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.new_chat_screen() && self.reopen.is_some() { return self.render_reopen(window, cx); }
        if self.new_chat_screen() { return self.render_new_chat(window, cx); }
        self.sync_plugin_fields(window, cx);
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
        // O que saiu do campo e o backend ainda não respondeu fica à vista, apagado, até virar mensagem da conversa.
        let outgoing: Vec<SharedString> = selected_key.as_ref()
            .map(|key| self.delivery.outgoing(key).into_iter().map(|text| SharedString::from(text.to_owned())).collect()).unwrap_or_default();
        let prethread_open = self.prethread_key().is_some();
        let mut content = div().w_full().flex().flex_col();
        let busy = selected_key.as_ref().is_some_and(|key| self.flight.busy(key));
        let action_note = selected_key.as_ref().and_then(|key| self.action_feedback.get(key)).cloned();
        let readable = self.selected.as_ref().is_some_and(|s| s.readable());
        if readable { self.follow_plugin_tab(window, cx); }
        let orq = self.selected.as_ref().filter(|s| s.orq()).map(|s| s.name.clone());
        // A sessão da outra pessoa não recebe resposta nem plano daqui: o servidor dela recusa.
        let read_only = self.selected.as_ref().is_some_and(|s| s.read_only());
        let card = if readable && !read_only { self.render_ask(busy, window, cx).or_else(|| self.render_options(busy, window, cx)) } else { None };
        let plan_bar = if readable && !read_only && card.is_none() {
            self.render_plan_bar(busy, cx).or_else(|| self.render_headless_plan(cx)).or_else(|| self.render_plan_preview(cx))
        } else { None };
        let steer = readable && self.steer_offered();
        let queued = self.queued_count();
        // Sem cartão, o pedido depende do terminal (overlay, login ou seletor sem opções legíveis).
        // Pergunta do transcript já respondida espera só o `tool_result`: não é pedido sem resposta.
        let answered = interaction::ask_from_events(&self.chat.events, self.provider().0)
            .and_then(|ask| ask.tool_use_id).is_some_and(|id| self.tool_answered(&id));
        // Menu de uma pergunta nativa aberta ou recém-respondida: o card é quem responde, sem aviso de terminal.
        let ask_pane = self.chat.ask_pane();
        let pending = card.is_none() && !answered && !ask_pane && !prethread_open && (self.chat.state.state == "awaiting_input" || self.chat.state.login);
        // Faixas e avisos entre a conversa e o compositor ficam na mesma coluna das mensagens.
        content = content
            .children(outgoing.into_iter().map(|text| in_column(div().w_full().flex().flex_col().items_end().gap_1().py_1()
                .child(user_bubble(conversation_text(div().whitespace_normal().line_clamp(6).text_ellipsis(), true).child(text)).opacity(0.6))
                .child(div().text_xs().text_color(theme::muted()).child(tr("sending"))))))
            .when_some(card, |el, card| el.child(card))
            .when_some(plan_bar, |el, bar| el.child(in_column(bar)))
            .when(pending, |el| el.child(in_column(div().py_2().text_sm().text_color(theme::warning()).child(tr("pending_question")))))
            .when_some(self.chat.state.question.clone().filter(|_| pending), |el, question| el.child(in_column(div().text_sm().child(question))))
            .when_some(action_note, |el, (note, warning)| el.child(in_column(div().py_1().text_xs().text_color(if warning { theme::warning() } else { theme::muted() }).child(note))))
            .when_some(problem_banner(&self.chat.state), |el, problem| el.child(in_column(div().text_sm().text_color(theme::warning()).child(problem))))
            .when_some(self.error.clone(), |el, error| el.child(in_column(div().py_2().text_sm().text_color(theme::warning()).child(error)
                .child(Button::new("retry").small().ghost().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| {
                    this.reselect(window, cx);
                }))))))
            .when_some(delivery_note, |el, (note, warning)| el.child(in_column(div().py_1().text_xs().text_color(if warning { theme::warning() } else { theme::muted() }).child(note))))
            .when_some(stop_note, |el, (note, warning)| el.child(in_column(div().py_1().text_xs().text_color(if warning { theme::warning() } else { theme::muted() }).child(note))))
            .children(readable.then(|| {
                let view = self.plugin_view(cx);
                // Painéis de mod ficam acima da faixa, como o terminal os abre, em qualquer largura: com mais de um, em abas,
                // só o da frente desenhado. A altura tem teto para o painel comprido rolar por dentro.
                let tallest = f32::from(window.viewport_size().height) * PLUGIN_PANE_MAX_SHARE;
                let ids = crate::plugin_ui::pane_ids(&self.plugin_panes);
                let active = crate::plugin_ui::active_pane(&ids, &self.plugin_shown, self.plugin_local_tab.as_deref());
                crate::plugin_ui::panes(&self.plugin_panes, active.as_deref(), &view, tallest).map(in_column).into_iter()
                    .chain(crate::plugin_ui::band(&self.plugin_band, &view).map(in_column)).collect::<Vec<_>>()
            }).into_iter().flatten())
            .map(|el| match orq {
                Some(orq) => el.child(in_column(self.render_orq_footer(&orq, cx))),
                None if read_only => el.child(in_column(div().py_2().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("par_so_leitura")))),
                None => el.when(self.selected.is_some() || self.api.is_some(),
                    |el| el.child(self.render_composer(readable, busy, steer, queued, sending, stopping, window, cx))),
            });
        self.measured_bottom(content.into_any_element())
    }

    /// O painel direito abrindo ou fechando na mesma conversa: a largura dele e quanto está à vista (0 a 1), pedindo
    /// quadros até acabar. Trocar de conversa, a chegada da primeira mensagem e o movimento reduzido não deslizam.
    fn side_slide_frame(&mut self, window: &mut Window, cx: &App) -> Option<(f32, f32)> {
        let now = (self.selected_key(), self.side_width(window));
        if let Some((key, width)) = self.side_seen.replace(now.clone())
            && key == now.0 && width.is_some() != now.1.is_some() && !cx.reduce_motion() && !self.landing_active() {
            self.side_slide = now.1.or(width).map(|w| (Instant::now(), now.1.is_some(), w));
        }
        let (start, opening, width) = self.side_slide?;
        if start.elapsed() >= motion::RESIZE.total() { self.side_slide = None; return None; }
        motion::request_frame(window, cx);
        let t = motion::RESIZE.ease(motion::RESIZE.raw(start));
        Some((width, if opening { t } else { 1. - t }))
    }

    /// Saindo, o painel ainda desenha o que mostrava enquanto desliza para fora.
    pub(super) fn side_closing(&self) -> bool { self.side_slide.is_some_and(|(_, opening, _)| !opening) }
}

impl Render for Hangar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_a11y_retain(window, cx);
        // O assistente ocupa a janela: nada da conversa por baixo recebe tecla nem clique.
        if let Some(setup) = self.setup.clone() {
            return div().id("hangar-root").size_full().bg(theme::window_fill()).text_color(theme::text()).text_base()
                .font_family(theme::SANS).child(setup).into_any_element();
        }
        self.sync_plan_review(window, cx);
        self.rail_frame(window);
        let selected_name = self.selected.as_ref().map(|s| s.name.clone());
        let floating = theme::is_floating();
        let chat_background = appearance::get().background_scope == appearance::BackgroundScope::Chat;
        let desktop_window = appearance::get().background == appearance::Background::Desktop
            && appearance::get().wallpaper == appearance::Wallpaper::Window;
        // Página de Configurações ocupa a janela; a caixa ao vivo deixa a janela da conversa por baixo.
        let page = self.settings.filter(|_| !self.settings_ui.live);
        let costs_page = self.costs.view.is_some();
        let worktrees_page = self.worktrees.view.is_some();
        let navigation = appearance::get().navigation;
        let (tabs, bottom_tabs) = (navigation.tabs(), navigation == appearance::Navigation::BottomTabs);
        let cutout = chat_background && page.is_none() && !costs_page && !worktrees_page && (desktop_window || floating);
        let chat_bounds = std::rc::Rc::new(std::cell::Cell::new(Bounds::<Pixels>::default()));
        // Colados com barra lateral, ela sobe até o topo e a barra do app começa na borda dela, como no Zeron. O fundo
        // do chat passa a ser da coluna que junta a barra e o chat, para a barra ter a cor dele.
        let beside_sidebar = !floating && !tabs && page.is_none() && !costs_page && !worktrees_page;
        let chat_fill = |el: Div, this: &Self, window: &Window| el
            .when(chat_background, |el| el.bg(theme::window_fill()))
            .when(cutout, |el| {
                let bounds = chat_bounds.clone();
                el.child(canvas(move |area, _, _| bounds.set(area), |_, _, _, _| {}).absolute().inset_0())
            })
            .when(chat_background, |el| el.children(this.render_backdrop(window)));

        // Sessão sem conversa não tem stream próprio: o estado é o da lista.
        let header_state = if self.chat_online && self.chat.state.state.is_empty() { "loading".to_owned() }
            else if self.chat_online { self.chat.state.state.clone() }
            else if let Some(s) = self.selected.as_ref().filter(|s| !s.readable()) { s.display_state().to_owned() }
            else if self.selected.is_some() { "reconnecting".to_owned() }
            else if self.list_online { "connected".to_owned() } else { "disconnected".to_owned() };
        let session_chip = matches!(header_state.as_str(), "working" | "idle" | "awaiting_input" | "dead");
        let limited_now = if self.chat.state.state.is_empty() { self.selected.as_ref().and_then(|s| s.limited) == Some(true) } else { self.chat.state.limited };
        let chip_state = if limited_now && session_chip { "limited".to_owned() } else { header_state.clone() };
        let place = self.selected.as_ref().map(|s| place(s, &self.session_label(cx)));
        let landing::Frame { drop, shown, rise } = self.landing_frame(window, cx);
        let opening = self.opening.clone().filter(|_| self.selected.is_none());
        let content = div().relative().flex_1().min_w_0().h_full().flex().flex_col()
            .when(!beside_sidebar, |el| chat_fill(el, self, window))
            .child(div().h(px(44.)).pl(px(20.)).pr(px(12.)).flex_shrink_0().flex().items_center().gap(px(10.)).when(floating, |el| el.mx(px(4.)))
                .relative().opacity(shown).top(px(rise))
                .when_some(self.selected.as_ref(), |el, s| el.child(chrome::provider_glyph(&s.provider, 18.)))
                .when_some(opening.as_ref(), |el, o| el.child(chrome::provider_glyph(&o.provider, 18.)))
                .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD)
                    .child(selected_name.clone().or_else(|| opening.as_ref().map(|o| o.name.clone())).unwrap_or_else(|| tr("title"))))
                .when_some(place, |el, place| el.child(div().min_w_0().truncate().text_color(theme::faint()).child(place)))
                .child(div().flex_1())
                // Sem sessão, o estado da ligação só aparece quando ela não está normal: "Conectado" não diz nada.
                .when(session_chip || header_state != "connected", |el| el.child(if session_chip { chrome::state_chip(&chip_state, tr(&format!("chip_{chip_state}")), true) }
                    else { div().flex_shrink_0().text_xs().text_color(theme::status(&header_state)).child(tr(&header_state)).into_any_element() }))
                .when_some(self.render_activity_button(window, cx), |el, button| el.child(button))
                .when(self.selected.is_some(), |el| el.child(chrome::icon_button("side-show", IconName::PanelRight,
                        tr(if self.side.open { "side_hide" } else { "side_show" }), cx)
                    .selected(self.side.open).on_click(cx.listener(|this, _, _, cx| this.toggle_side(cx)))))
                .when(self.selected.as_ref().is_some_and(|s| terminal::terminal_offered(s, self.has_shortcut_terms())), |el| el.child(chrome::icon_button("terminal-show", IconName::SquareTerminal,
                        tr("term_toggle"), cx).selected(self.terminal.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx))))))
            // Cada área é uma view própria, guardada entre quadros quando pode (`panes.rs`).
            // Sem sessão, a faixa de baixo ocupa a área toda: o compositor fica no meio da tela.
            .when(!self.new_chat_screen(), |el| el.child(self.pane_element(panes::Area::Conversation,
                StyleRefinement::default().w_full().flex_1().min_h_0().opacity(shown).top(px(rise)))))
            // Entre a conversa e a faixa de baixo: o que a faixa abre por cima (comandos, sugestões) cobre a marca. Sem a
            // conversa na tela, os lugares dela são do último desenho, de outra sessão.
            .when(page.is_none() && !self.new_chat_screen(), |el| el.child(self.working_mark_float(panes::Area::Conversation, WORKING_FADE, cx.reduce_motion())))
            .child(self.pane_element(panes::Area::Bottom, if self.new_chat_screen() { StyleRefinement::default().w_full().flex_1().min_h_0() }
                else { StyleRefinement::default().w_full().flex_shrink_0().h(px(self.panes.bottom_height.get())).top(px(-drop)) }))
            .children(self.render_terminal(window, cx))
            .children(self.render_file_view(cx));
        // Visor de arquivos expandido: sem a lista de sessões e sem o painel direito.
        let files_expanded = self.files_expanded();
        let nav = if page.is_some() || costs_page || worktrees_page || files_expanded { None }
            else if tabs { Some(self.pane_element(panes::Area::Nav, StyleRefinement::default().w_full().h(px(44.)).flex_shrink_0()
                .bg(if chat_background { theme::background().alpha(1.) } else { transparent_black() }))) }
            else { Some(self.pane_element(panes::Area::Nav, StyleRefinement::default().w(px(self.nav_width())).h_full().flex_shrink_0()
                .bg(if chat_background { theme::background().alpha(1.) } else { transparent_black() }))) };
        self.sync_side_cost(window);
        self.sync_browser(window, cx);
        // A marca da aba Atividade anima fora das duas views guardadas (painel e aba), depois delas na árvore.
        let slide = self.side_slide_frame(window, cx);
        let beside = beside_sidebar.then(|| self.side_width(window).filter(|_| !files_expanded)
            .or(slide.map(|(width, _)| width)).or_else(|| self.opening_side_width(window)).unwrap_or(0.));
        // A barra é guardada entre quadros; `beside` muda sem aviso (painel deslizando), e aí ela sai deste desenho e
        // redesenha a cópia para o próximo. No Windows sempre sai do desenho: a área de arrastar e os botões da janela
        // não sobrevivem à cópia guardada.
        let beside_moved = self.panes.top_beside.replace(beside) != beside;
        if beside_moved { self.redraw(panes::Area::Top, cx); }
        let topbar = self.pane_element_live(panes::Area::Top, StyleRefinement::default().w_full().h(px(topbar::height())).flex_shrink_0(),
            beside_moved || cfg!(target_os = "windows"));
        let (topbar, topbar_beside) = if beside_sidebar { (None, Some(chat_fill(div(), self, window).relative().flex_1().min_w_0().h_full().flex().flex_col().child(topbar))) }
            else { (Some(topbar), None) };
        let side = self.side_width(window).or(slide.map(|(width, _)| width)).map(|width| div().h_full().flex_shrink_0().relative().opacity(shown).top(px(rise))
            .when(chat_background, |el| el.bg(theme::background().alpha(1.)))
            .child(self.pane_element(panes::Area::Side, StyleRefinement::default().w(px(width)).h_full().flex_shrink_0()))
            // Só com a aba à vista: fora dela a view não redesenha e não limpa os próprios lugares.
            .when(self.activity_tab(), |el| el.child(self.activity_mark_float(cx)))
            .child(self.subagent_mark_float(cx)).into_any_element())
            // Entrando ou saindo, o painel desliza da borda com a largura final: a conversa muda de largura uma vez só.
            .map(|side| match slide {
                Some((width, shown)) => div().w(px(width)).h_full().flex_shrink_0().overflow_hidden()
                    .child(div().relative().left(px(width * (1. - shown))).h_full().child(side)).into_any_element(),
                None => side,
            })
            .or_else(|| self.opening_side_width(window).map(|width| div().h_full().flex_shrink_0().relative().opacity(shown).top(px(rise))
                .when(chat_background, |el| el.bg(theme::background().alpha(1.))).child(self.render_opening_side(width)).into_any_element()));
        let side = side.filter(|_| !files_expanded);
        // Antes das áreas guardadas desenharem: elas leem a coluna daqui.
        COLUMN_FRAME.set((f32::from(window.viewport_size().width), side.is_some()));
        let dialog_top = window.viewport_size().height / 10.;
        let dialog_width = (window.viewport_size().width - px(32.)).min(px(480.));
        let entry = self.render_entry(cx);
        let entry_shown = entry.is_some();
        // Só montado com a conexão aberta: a raiz redesenha a cada batida das animações.
        let dialog = self.connection_dialog.then(|| div().id("connection-card").w(dialog_width).max_h(window.viewport_size().height - dialog_top - px(16.))
            .p(px(20.)).bg(theme::popup_fill(theme::raised())).border_1().border_color(theme::glass_border()).rounded(px(16.))
            .shadow_xl().overflow_y_scroll().occlude().flex().flex_col().gap_4()
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.api.is_some() {
                    this.connection_dialog = false;
                    this.connection_origin.take().and_then(|origin| origin.upgrade()).unwrap_or_else(|| this.root_focus.clone()).focus(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .map(|el| match entry {
                Some(card) => el.child(card),
                None => el.child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(tr("connection")))
                    .child(div().text_sm().text_color(theme::muted()).child(tr("connection_hint")))
                    .child(div().text_sm().child(tr("server"))).child(Input::new(&self.address).aria_label(tr("server")))
                    .child(div().text_sm().child(tr("token"))).child(Input::new(&self.token).aria_label(tr("token"))),
            })
            .when(self.electron_offer, |el| el.child(div().flex().flex_col().gap_2().pt_3().border_t_1().border_color(theme::glass_border())
                .child(div().text_sm().text_color(theme::muted()).child(tr("electron_import_offer_hint")))
                .child(Button::new("electron-import").outline().label(tr("electron_import_offer"))
                    .on_click(cx.listener(|this, _, window, cx| this.import_electron(false, window, cx))))))
            .when_some(self.error.clone(), |el, error| el.child(div().text_sm().text_color(theme::warning()).child(error)))
            .when(!entry_shown, |el| el.child(div().flex().justify_end().gap_2()
                .when(self.api.is_some(), |el| el.child(Button::new("cancel").label(tr("cancel")).on_click(cx.listener(|this, _, window, cx| {
                    this.connection_dialog = false;
                    this.connection_origin.take().and_then(|origin| origin.upgrade()).unwrap_or_else(|| this.root_focus.clone()).focus(window, cx);
                    cx.notify();
                }))))
                .child(Button::new("connect").primary().label(tr("connect")).on_click(cx.listener(|this, _, window, cx| {
                    this.connect(window, cx);
                    if !this.connection_dialog {
                        window.close_all_dialogs(cx);
                        this.root_focus.focus(window, cx);
                    }
                }))))));
        self.finish_landing(window);

        let ticker = motion::ticker(window, cx);
        let dialog_in = self.connection_dialog.then(|| motion::enter("connection-dialog-in", motion::DIALOG_IN, window, cx)).unwrap_or(1.);
        let live = self.settings_live().then(|| self.render_live(window, cx));
        div().id("hangar-root").track_focus(&self.root_focus).relative().size_full().flex()
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                for browser in this.side.browsers.values() { browser.read(cx).release_focus() }
            }))
            // Sessão solta fora da lista: nada acontece, só termina o arrasto.
            .on_drop(cx.listener(|this, _: &grouping::SessionDrag, _, cx| this.end_session_drag(cx)))
            .bg(if !chat_background { theme::window_fill() }
                else if cutout { transparent_black().into() }
                else { theme::background().alpha(1.).into() })
            .text_color(theme::text()).text_base()
            // O recorte medido deixa o desktop aparecer só no chat, inclusive entre as caixas soltas.
            .when(cutout, |el| el.child(canvas(|_, _, _| {}, move |bounds, _, window, _| {
                let chat = chat_bounds.get();
                for (start, end) in [
                    (bounds.origin, point(bounds.right(), chat.top())),
                    (point(bounds.left(), chat.bottom()), bounds.bottom_right()),
                    (point(bounds.left(), chat.top()), point(chat.left(), chat.bottom())),
                    (point(chat.right(), chat.top()), point(bounds.right(), chat.bottom())),
                ] {
                    window.paint_quad(fill(Bounds::from_corners(start, end), theme::background().alpha(1.)));
                }
            }).absolute().inset_0()))
            .when(!chat_background, |el| el.children(self.render_backdrop(window)))
            .font_family(theme::SANS)
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, window, cx| {
                this.keyboard_modifiers_changed(event, window, cx);
                this.session_number_modifiers(event.modifiers, window, cx);
            }))
            .capture_key_up(cx.listener(|this, _: &KeyUpEvent, window, cx| {
                this.session_number_modifiers(window.modifiers(), window, cx);
            }))
            .on_action(cx.listener(|this, action: &keyboard::RunShortcut, window, cx| this.run_keyboard_shortcut(action, window, cx)))
            .on_action(cx.listener(|this, _: &FocusComposer, window, cx| {
                let page_open = this.settings.is_some() && !this.settings_live() || this.costs.view.is_some() || this.worktrees.view.is_some();
                if !this.connection_dialog && !page_open && (this.selected.as_ref().is_some_and(SessionInfo::takes_messages)
                    || this.selected.is_none() && this.api.is_some()) {
                    this.composer.update(cx, |input, cx| input.focus(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                if !this.connection_dialog { this.open_settings(settings::Page::Appearance, window, cx); }
            }))
            .on_action(cx.listener(|this, _: &OpenCosts, window, cx| this.toggle_costs(window, cx)))
            .on_action(cx.listener(|this, _: &OpenWorktrees, window, cx| this.toggle_worktrees(window, cx)))
            .on_action(cx.listener(|this, _: &OpenSearch, window, cx| this.toggle_search(window, cx)))
            .on_action(cx.listener(|this, _: &FocusSettingsSearch, window, cx| this.focus_search(window, cx)))
            .on_action(cx.listener(|this, _: &FindProjectFile, window, cx| this.find_project_files(false, window, cx)))
            .on_action(cx.listener(|this, _: &FindProjectText, window, cx| this.find_project_files(true, window, cx)))
            .on_action(cx.listener(|this, _: &NextSession, window, cx| this.step_session(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousSession, window, cx| this.step_session(-1, window, cx)))
            .on_action(cx.listener(|this, _: &NewChat, window, cx| this.go_home(window, cx)))
            .on_action(cx.listener(|this, _: &OpenNewSession, window, cx| {
                if !this.create_blocked(window, cx) { this.open_new_session(None, window, cx); }
            }))
            .on_action(cx.listener(|this, _: &CloseSession, window, cx| this.close_selected(window, cx)))
            .on_action(cx.listener(|this, _: &RenameSession, window, cx| this.rename_selected(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| if !this.connection_dialog { this.toggle_rail(cx) }))
            .on_action(cx.listener(|this, _: &ToggleDictation, window, cx| this.toggle_dictation(window, cx)))
            .on_action(cx.listener(|this, _: &CopyLastReply, _, cx| {
                let page_open = this.settings.is_some() && !this.settings_live();
                if let Some(text) = this.last_reply().filter(|_| !page_open) { cx.write_to_clipboard(ClipboardItem::new_string(text)); }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| if this.costs_page_key(event, window) { cx.stop_propagation() }))
            // Fora de campo de texto a tecla chega aqui; dentro dele o atalho do campo a troca pela ação, pega na captura.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let m = &event.keystroke.modifiers;
                // "/" fora de campo de texto leva ao campo de mensagem, sem digitar a barra, como o web.
                if event.keystroke.key_char.as_deref() == Some("/") && !m.control && !m.alt && !m.platform && !window.text_input_focused()
                    && !this.connection_dialog && !window.has_active_dialog(cx) {
                    window.dispatch_action(Box::new(FocusComposer), cx);
                    cx.stop_propagation();
                    return;
                }
                if m.control || m.alt || m.platform || m.shift || !this.chat_keys_apply(window, cx) { return; }
                if this.chat_page_key(&event.keystroke.key, window, cx) { cx.stop_propagation(); }
            }))
            .capture_action(cx.listener(|this, _: &gpui_kit::base::input::MovePageUp, window, cx| {
                if this.chat_keys_apply(window, cx) && this.chat_page_key("pageup", window, cx) { cx.stop_propagation(); }
            }))
            .capture_action(cx.listener(|this, _: &gpui_kit::base::input::MovePageDown, window, cx| {
                if this.chat_keys_apply(window, cx) && this.chat_page_key("pagedown", window, cx) { cx.stop_propagation(); }
            }))
            // Shift+Tab no campo de mensagem é a tecla do terminal do Claude e do Codex; nos outros campos segue recuando.
            .capture_action(cx.listener(|this, _: &gpui_kit::base::input::OutdentInline, window, cx| {
                if this.connection_dialog || !this.composer.read(cx).focus_handle(cx).is_focused(window) { return; }
                match this.provider().0 {
                    "claude" => this.cycle_permission(cx),
                    "codex" => this.toggle_codex_mode(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &CyclePermission, window, cx| {
                if !this.connection_dialog && !window.has_active_dialog(cx) { this.cycle_permission(cx); }
            }))
            // Esc fora do campo fecha o painel aberto sobre o compositor (o clique no botão tira o foco do campo).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Com a confirmação aberta, o Esc é dela: fecha só o diálogo.
                if event.keystroke.key != "escape" || this.connection_dialog || this.search_focused(window, cx) || window.has_active_dialog(cx) { return; }
                if this.search.open { this.close_search(window, cx); cx.stop_propagation(); return; }
                if this.shortcuts_escape(window, cx) { cx.stop_propagation(); return; }
                if this.costs_escape(window, cx) { cx.stop_propagation(); return; }
                if this.worktrees.view.is_some() { this.close_worktrees(window, cx); cx.stop_propagation(); return; }
                // O painel preso a um botão é a camada de cima: fecha antes de arquivos e terminal, e o foco volta ao campo.
                // Com a página de configurações aberta nenhum painel está na tela; a flag das pastas fica para quando ela fechar.
                if (this.settings.is_none() || this.settings_live()) && this.close_popups() {
                    this.composer.update(cx, |input, cx| input.focus(window, cx));
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if this.side_menu_escape(cx) { cx.stop_propagation(); return; }
                if this.files_escape(window, cx) { cx.stop_propagation(); return; }
                if this.terminal.is_some() && (this.settings.is_none() || this.settings_live()) {
                    this.close_terminal(true, window, cx);
                    cx.stop_propagation();
                    return;
                }
                if this.settings.is_some() {
                    this.close_settings(window, cx);
                    cx.stop_propagation();
                    return;
                }
                // Foco na resposta digitada, na raiz (clique numa opção ou no vazio devolve o foco a ela) ou em lugar nenhum:
                // o Esc cancela a pergunta como no campo. Outros campos (renomear, endereço) têm o Esc deles.
                let in_ask = window.focused(cx).is_none_or(|focus| focus == this.root_focus
                    || this.ask_form.inputs.iter().any(|input| input.focus_handle(cx) == focus));
                if this.chat.ask.is_some() && in_ask {
                    if this.confirm.take().is_some() { this.confirm_no_ask = false; cx.notify(); } else { this.request_stop(window, cx); }
                    cx.stop_propagation();
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
            .when(self.nav_resizing(), |el| el.cursor_col_resize()
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    this.drag_nav(f32::from(event.position.x), event.pressed_button == Some(MouseButton::Left), cx);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _, cx| this.drag_nav(f32::from(event.position.x), false, cx))))
            .when(self.terminal_dragging(), |el| el.cursor_row_resize()
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                    this.drag_terminal(f32::from(event.position.y), event.pressed_button == Some(MouseButton::Left), window, cx);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, window, cx| {
                    this.drag_terminal(0., false, window, cx);
                })))
            // Arrasto da caixa ao vivo pelo cabeçalho: mesmo esquema, gravando a posição ao soltar.
            .when(self.live_dragging(), |el| el.cursor_move()
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                    this.drag_live(event.position, event.pressed_button == Some(MouseButton::Left), window, cx);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, window, cx| this.drag_live(event.position, false, window, cx))))
            // A barra do app fica acima de tudo, inclusive das páginas, de ponta a ponta; soltos, a margem é só dos painéis.
            .flex_col()
            .children(topbar)
            .child(div().w_full().flex_1().min_h_0().flex().when(floating && page.is_none() && !costs_page && !worktrees_page, |el| el.p(px(10.)).gap(px(10.)))
                .map(|el| match (page, nav) {
                _ if worktrees_page => el.child(self.render_worktrees(window, cx)),
                _ if costs_page => el.child(self.render_costs(window, cx)),
                (Some(page), _) => el.child(self.render_settings(page, window, cx)),
                // Abas: a faixa em cima (ou embaixo), a conversa e o painel do outro lado, sem barra lateral.
                (None, Some(bar)) if tabs => {
                    let body = div().flex_1().min_h_0().flex().when(floating, |el| el.gap(px(10.))).child(content).when_some(side, |el, side| el.child(side));
                    el.flex_col().map(|el| if bottom_tabs { el.child(body).child(bar) } else { el.child(bar).child(body) })
                        .child(self.working_mark_float(panes::Area::Nav, WORKING_FADE, cx.reduce_motion()))
                }
                (None, sidebar) => el.children(sidebar)
                    .when(page.is_none(), |el| el.child(self.working_mark_float(panes::Area::Nav, WORKING_FADE, cx.reduce_motion())))
                    .map(|el| match topbar_beside {
                        Some(column) => el.child(column
                            .child(div().w_full().flex_1().min_h_0().flex().child(content).when_some(side, |el, side| el.child(side)))),
                        None => el.child(content).when_some(side, |el, side| el.child(side)),
                    }),
            }))
            .children(live)
            .children(self.render_preview(window))
            .children(self.render_landing_ghost(cx))
            .children(self.render_popup(window, cx))
            .children(self.render_search(window, cx))
            .children(self.render_plan_review_overlay(window, cx))
            .child(ticker)
            // Uma autenticação recusada pode abrir a conexão sobre um formulário já aberto: adiada, fica acima dos diálogos do kit.
            .when_some(dialog, |el, dialog| el.child(deferred(div().absolute().inset_0().bg(cx.theme().overlay).occlude().opacity(dialog_in)
                .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                    if this.api.is_some() {
                        this.connection_dialog = false;
                        this.connection_origin.take().and_then(|origin| origin.upgrade()).unwrap_or_else(|| this.root_focus.clone()).focus(window, cx);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }))
                .flex().items_start().justify_center().pt(dialog_top)
                .child(div().relative().top(px(2. * (1. - dialog_in))).child(if appearance::get().surface_material == appearance::SurfaceMaterial::Glass {
                    chrome::Glass::new(dialog.focus_trap("connection-dialog", &self.connection_focus), px(16.)).into_any_element()
                } else { dialog.focus_trap("connection-dialog", &self.connection_focus).into_any_element() })))
                .with_priority(gpui_kit::base::POPUP_PRIORITY + 1)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{message_card, preview_step, safe_markdown, stream_motion, working_tokens, working_verb};
    use crate::{api::dto::ChatEvent, cards::Card, i18n::tr};
    use std::{collections::HashSet, time::Duration};

    #[test]
    fn closing_historical_tool_releases_full_details_without_another_event() {
        use super::{Prepared, prepare_detail, toggle_detail};
        use std::collections::HashMap;
        let mut expanded = HashSet::from(["old".to_owned(), "other".to_owned()]);
        let mut prepared = HashMap::from([
            ("old:input".into(), prepare_detail("Entrada".repeat(10000))),
            ("old:result".into(), prepare_detail("Saída".repeat(100000))),
            ("result:lines".into(), Prepared::Lines(100)),
            ("other:result".into(), prepare_detail("Outro detalhe".into())),
        ]);
        toggle_detail(&mut expanded, &mut prepared, "old");
        assert!(!expanded.contains("old"));
        assert!(!prepared.contains_key("old:input"));
        assert!(!prepared.contains_key("old:result"));
        assert!(prepared.contains_key("result:lines"));
        assert!(prepared.contains_key("other:result"));
        toggle_detail(&mut expanded, &mut prepared, "old");
        assert!(expanded.contains("old"));
    }

    #[test]
    fn plugin_show_404_is_an_older_server_not_an_error() {
        use super::{Failure, Hangar};
        let failure = |status| Failure { status, detail: "x".into(), retry_after: None, uncertain: false, code: None };
        assert_eq!(Hangar::show_failure(&failure(Some(404))), None);
        assert_eq!(Hangar::show_failure(&failure(Some(405))), None);
        assert_eq!(Hangar::show_failure(&failure(Some(409))), Some(Hangar::failure(&failure(Some(409)))));
        // Sem resposta ou 5xx, a frase da troca de aba (a mesma do web), não a do clique.
        assert_eq!(Hangar::show_failure(&failure(None)), Some(crate::i18n::tr_shared("plugin_aba_falhou", &[])));
        assert_eq!(Hangar::show_failure(&failure(Some(503))), Some(crate::i18n::tr_shared("plugin_aba_falhou", &[])));
        assert_ne!(crate::i18n::tr_shared("plugin_aba_falhou", &[]), "plugin_aba_falhou");
    }

    #[test]
    fn mod_field_failure_without_an_answer_or_with_5xx_is_the_app_phrase() {
        use super::{Failure, Hangar};
        use crate::i18n::tr_shared;
        let failure = |status| Failure { status, detail: "x".into(), retry_after: None, uncertain: true, code: None };
        assert_eq!(Hangar::input_failure(&failure(None)), tr_shared("plugin_input_falhou", &[]));
        assert_eq!(Hangar::input_failure(&failure(Some(502))), tr_shared("plugin_input_falhou", &[]));
        // A recusa traz o motivo, como no clique.
        assert_eq!(Hangar::input_failure(&failure(Some(409))), Hangar::failure(&failure(Some(409))));
        assert_ne!(tr_shared("plugin_input_falhou", &[]), "plugin_input_falhou");
    }

    #[test]
    fn mod_refusal_with_a_known_code_uses_its_sentence_at_any_status() {
        use super::{Failure, Hangar};
        use crate::i18n::{tr_shared, tr_web};
        use std::collections::HashMap;
        // 503 do dono único com `erro_mod_guarda_indisponivel`: o `detail` já é a frase dele, nos três pedidos de mod.
        let sentence = tr_web("erro_mod_guarda_indisponivel", &HashMap::new()).unwrap();
        let guard = Failure { status: Some(503), detail: sentence.clone(), retry_after: None, uncertain: true,
            code: Some("erro_mod_guarda_indisponivel".into()) };
        assert_eq!(Hangar::show_failure(&guard), Some(sentence.clone()));
        assert_eq!(Hangar::input_failure(&guard), sentence);
        assert_eq!(Hangar::press_failure(&guard), sentence);
        // 500 sem código: a frase genérica de cada um.
        let bare = Failure { status: Some(500), detail: "HTTP 500".into(), retry_after: None, uncertain: true, code: None };
        assert_eq!(Hangar::show_failure(&bare), Some(tr_shared("plugin_aba_falhou", &[])));
        assert_eq!(Hangar::input_failure(&bare), tr_shared("plugin_input_falhou", &[]));
        assert_eq!(Hangar::press_failure(&bare), tr_shared("plugin_clique_falhou", &[]));
        assert_eq!(Hangar::close_failure(&bare), tr_shared("plugin_fechar_falhou", &[]));
        assert_ne!(tr_shared("plugin_fechar_falhou", &[]), "plugin_fechar_falhou");
        // Código que o app não conhece não vale como frase: 5xx com ele segue a genérica.
        let unknown = Failure { code: Some("internal_info".into()), ..bare };
        assert_eq!(Hangar::show_failure(&unknown), Some(tr_shared("plugin_aba_falhou", &[])));
    }

    #[test]
    fn mod_request_during_an_agent_switch_shows_the_translated_refusal() {
        use super::{Failure, Hangar};
        use std::collections::HashMap;
        // O 409 `session_transfer_busy` do Python, repassado pelo Rust: o `failure_detail` já o traduziu, e os três
        // pedidos de mod mostram a frase, não o texto cru do servidor.
        let sentence = crate::i18n::tr_web("session_transfer_busy", &HashMap::new()).unwrap();
        let busy = Failure { status: Some(409), detail: sentence.clone(), retry_after: None, uncertain: false,
            code: Some("session_transfer_busy".into()) };
        assert_eq!(Hangar::show_failure(&busy), Some(sentence.clone()));
        assert_eq!(Hangar::input_failure(&busy), sentence);
        assert_eq!(Hangar::press_failure(&busy), sentence);
    }

    #[test]
    fn mod_click_failure_is_not_a_message_delivery() {
        use super::{Failure, Hangar};
        let failure = |status| Failure { status, detail: "x".into(), retry_after: None, uncertain: true, code: None };
        // A frase do clique é a mesma do web (`plugin_clique_falhou`), não uma `native_*`.
        let generic = crate::i18n::tr_shared("plugin_clique_falhou", &[]);
        assert_ne!(generic, "plugin_clique_falhou");
        assert_eq!(Hangar::press_failure(&failure(Some(500))), generic);
        assert_eq!(Hangar::press_failure(&failure(None)), generic);
        assert_eq!(Hangar::press_failure(&failure(Some(409))), Hangar::failure(&failure(Some(409))));
    }

    #[test]
    fn worktree_label_prefers_real_location() {
        use super::SessionInfo;
        let s = SessionInfo { name: "a".into(), cwd: Some("/r/hangar".into()), worktree: Some(true),
                              worktree_path: Some("/r/hangar-x".into()), ..Default::default() };
        assert_eq!(super::worktree_label(&s).as_deref(), Some("hangar-x"));
        assert_eq!(super::worktree_label(&SessionInfo { name: "a".into(), ..Default::default() }), None);
    }

    #[test]
    fn conversation_corner_preserves_delivery_truth_and_session_warnings() {
        use super::{conversation_row_state, SendOutcome, SessionInfo};
        let mut session = SessionInfo { state: "working".into(), ..Default::default() };
        assert_eq!(conversation_row_state(&session, None, false), "working");
        assert_eq!(conversation_row_state(&session, None, true), "queued");
        assert_eq!(conversation_row_state(&session, Some(&SendOutcome::Queued), false), "working");
        assert_eq!(conversation_row_state(&session, Some(&SendOutcome::Uncertain), true), "uncertain");
        assert_eq!(conversation_row_state(&session, Some(&SendOutcome::Rejected("no".into())), true), "failed");
        session.limited = Some(true);
        assert_eq!(conversation_row_state(&session, None, true), "limited");
        session.problema = Some("broken config".into());
        assert_eq!(conversation_row_state(&session, None, true), "problem");
        session.problema = Some(" ".into());
        session.limited = None;
        for (state, expected) in [("idle", "idle"), ("awaiting_input", "input"), ("dead", "dead"), ("other", "unknown")] {
            session.state = state.into();
            assert_eq!(conversation_row_state(&session, None, false), expected);
        }
    }

    #[test]
    fn problem_banner_translates_known_codes_and_keeps_raw_detail_otherwise() {
        use super::{problem_banner, SessionState};
        crate::i18n::set_language(crate::appearance::Language::Pt);
        let mut state = SessionState { problema: Some("terminal_input_composer_busy".into()), problema_detalhe: Some("composer_busy".into()), ..Default::default() };
        assert_eq!(problem_banner(&state).unwrap(), "A mensagem está esperando: o campo de digitação do terminal tem texto — composer_busy");
        state.problema = Some("codigo_sem_frase".into());
        assert_eq!(problem_banner(&state).unwrap(), "composer_busy");
        state.problema_detalhe = None;
        assert_eq!(problem_banner(&state).unwrap(), "codigo_sem_frase");
        state.problema = None;
        assert!(problem_banner(&state).is_none());
    }

    #[test]
    fn orq_texts_come_from_the_web_keys() {
        use crate::{api::Failure, i18n::tr_shared};
        let refused = Failure { status: Some(409), detail: "erro_sessao_orq".into(), retry_after: None, uncertain: false, code: None };
        assert_eq!(super::Hangar::failure(&refused), "O orquestrador não recebe mensagens; fale com o árbitro.");
        assert_eq!(tr_shared("orq_row_badge", &[]), "Orquestrador · sem LLM");
        assert_eq!(tr_shared("orq_talk_to_arbiter", &[]), "Falar com o árbitro");
    }

    #[test]
    fn system_notifications_require_live_transitions_and_confirmed_preferences() {
        use super::{PushPreferences, SystemNotifications};
        use chrono::NaiveTime;
        use serde_json::json;
        use std::time::Instant;
        let time = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        let prefs = PushPreferences::parse(json!({"muted": ["muted"], "quiet_hours": {"start": "22:00", "end": "07:00"}})).unwrap();
        assert!(prefs.suppressed("muted", time(12, 0)));
        for now in [time(22, 0), time(23, 59), time(0, 0), time(6, 59)] { assert!(prefs.suppressed("open", now)); }
        assert!(!prefs.suppressed("open", time(7, 0)));
        let equal = PushPreferences::parse(json!({"muted": [], "quiet_hours": {"start": "12:00", "end": "12:00"}})).unwrap();
        assert!(!equal.suppressed("open", time(12, 0)));
        assert!(PushPreferences::parse(json!({"muted": [], "quiet_hours": {"start": "bad", "end": "07:00"}})).is_none());
        assert!(PushPreferences::parse(json!({})).is_none());
        let now = Instant::now();
        let mut n = SystemNotifications { finished: Some((true, 45)), ..Default::default() };
        assert_eq!(n.advance("working", now), None);
        assert_eq!(n.advance("idle", now + Duration::from_secs(90)), None);
        assert_eq!(n.advance("working", now), None);
        assert_eq!(n.advance("idle", now + Duration::from_secs(44)), None);
        assert_eq!(n.advance("working", now), None);
        assert_eq!(n.advance("idle", now + Duration::from_secs(45)), Some("notify_finished"));
        assert_eq!(n.advance("idle", now + Duration::from_secs(46)), None);
        assert_eq!(n.advance("awaiting_input", now), Some("notify_awaiting"));
        assert_eq!(n.advance("awaiting_input", now), None);
        assert_eq!(n.advance("working", now), None);
        assert_eq!(n.advance("awaiting_input", now), Some("notify_awaiting"));
        assert_eq!(n.advance("idle", now + Duration::from_secs(45)), Some("notify_finished"));
        assert_eq!(n.advance("working", now), None);
        assert_eq!(n.advance("awaiting_input", now), Some("notify_awaiting"));
        assert_eq!(n.advance("working", now + Duration::from_secs(100)), None);
        assert_eq!(n.advance("idle", now + Duration::from_secs(110)), None);
        n.finished = Some((false, 0));
        assert_eq!(n.advance("working", now), None);
        assert_eq!(n.advance("idle", now), None);
        n.reset_stream();
        assert_eq!(n.advance("awaiting_input", now), None);
    }

    #[test]
    fn working_line_takes_the_terminal_verb_and_counts_its_own_seconds() {
        assert_eq!(working_verb(Some("Sketching… (6s · esc to interrupt)")), "Sketching…");
        assert_eq!(working_verb(Some("Writing tests…")), "Writing tests…");
        // Rótulo sem verbo (outro harness) ou ausente: a palavra nossa.
        assert_eq!(working_verb(Some("Running")), tr("working_line"));
        assert_eq!(working_verb(None), tr("working_line"));
        assert_eq!(working_tokens(Some("Gitifying… (37s · ↓ 1.4k tokens · thought for 5s)")).as_deref(), Some("↓ 1.4k tokens"));
        assert_eq!(working_tokens(Some("Gitifying… (2m 3s · ↑ 812 tokens)")).as_deref(), Some("↑ 812 tokens"));
        assert_eq!((working_tokens(Some("Sketching… (6s · esc to interrupt)")), working_tokens(Some("Writing tests…")), working_tokens(None)), (None, None, None));
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
        let super::Prepared::Message { markdown, .. } = super::prepare_message(&event("user_msg"), &HashSet::new()) else { panic!() };
        assert_eq!(markdown, body);
        assert!(!super::long_message(&markdown));
        let super::Prepared::Message { markdown, .. } = super::prepare_message(&event("assistant_msg"), &HashSet::new()) else { panic!() };
        assert_eq!(markdown.matches("hangar-file:").count(), 2);
        assert_eq!(super::stamp(None), None);
        let now = chrono::Local::now().timestamp() as f64;
        assert_eq!(super::stamp(Some(now)), super::clock(Some(now)));
        assert!(super::stamp(Some(now - 3. * 86_400.)).is_some_and(|s| s.len() > 5));
    }

    #[test]
    fn sent_and_read_files_become_cited_refs() {
        use crate::api::Source;
        let call = |name: &str, input: serde_json::Value| ChatEvent { kind: "tool_use".into(), tool_name: Some(name.into()),
            tool_input: input.as_object().cloned(), ..Default::default() };
        let sent = call("SendUserFile", serde_json::json!({"files": ["/tmp/a/print.png", "/tmp/a/video.mp4"], "caption": "x"}));
        assert!(super::sends_files(&sent));
        assert_eq!(super::tool_file_refs(&sent), vec![
            (Source::Cited("/tmp/a/print.png".into()), "print.png".into(), true),
            (Source::Cited("/tmp/a/video.mp4".into()), "video.mp4".into(), false)]);
        let read = call("Read", serde_json::json!({"file_path": "/tmp/a/print.png"}));
        assert!(!super::sends_files(&read));
        assert_eq!(super::tool_file_refs(&read), vec![(Source::Cited("/tmp/a/print.png".into()), "print.png".into(), true)]);
        assert!(super::tool_file_refs(&call("Bash", serde_json::json!({"command": "ls /tmp/a/print.png"}))).is_empty());
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
    fn memory_footer_is_filtered_only_from_assistant_display_and_markdown_preview() {
        let text = "Resposta\n<oai-mem-citation>MEMORY.md:1";
        for kind in ["user_msg", "assistant_msg"] {
            let event = ChatEvent { kind: kind.into(), text: Some(text.into()), ..Default::default() };
            assert_eq!(super::display_body(&event), if kind == "assistant_msg" { "Resposta\n" } else { text });
        }
        let preview = super::Preview { text: text.into(), md: true, ..Default::default() };
        assert_eq!(super::preview_source(&preview), "Resposta\n");
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
