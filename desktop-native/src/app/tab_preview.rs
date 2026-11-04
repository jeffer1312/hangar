//! Prévia da sessão sobre a aba: com o mouse parado na aba, o chat dela abre por cima, desenhado pelo mesmo código do chat
//! principal (conversa, faixa de baixo e compositor), para ler, rolar e responder sem trocar de sessão. A conversa vem do
//! `/history` em consulta enquanto a prévia está aberta, nunca de um SSE por aba.
//!
//! O código do chat lê os campos da sessão aberta direto do `Hangar`. A prévia guarda os dela num pacote
//! (`ConversationSlot`) e o troca com os do `Hangar` só durante o desenho dela e durante um clique ou tecla dentro dela;
//! a troca de volta sai no fim do evento (`defer`), no começo do próximo desenho e no começo de cada resposta.
use super::*;
use super::controls::Ctl;
use super::sidebar::Target;

const SHOW_DELAY: Duration = Duration::from_millis(800);
const HIDE_DELAY: Duration = Duration::from_millis(500);
const POLL_EVERY: Duration = Duration::from_secs(2);
const EVENTS: usize = 80;
const WIDTH: f32 = 1200.;
const HEIGHT: f32 = 640.;
/// A lista de campos é uma só: o pacote e a troca saem dela, e campo novo entra nos dois.
macro_rules! conversation_slot {
    ($($field:ident: $ty:ty),+ $(,)?) => {
        pub(super) struct ConversationSlot { $($field: $ty),+ }
        impl Hangar {
            pub(super) fn swap_conversation(&mut self, slot: &mut ConversationSlot) { $(std::mem::swap(&mut self.$field, &mut slot.$field);)+ }
        }
    };
}

conversation_slot! {
    selected: Option<SessionInfo>,
    open_api: Option<Api>,
    chat: Chat,
    chat_online: bool,
    loading: bool,
    history_started: bool,
    history_installed: bool,
    history_limit: usize,
    has_older: bool,
    error: Option<String>,
    list_state: ListState,
    rail_hover: Option<usize>,
    follow: follow::Follow,
    row_ids: Vec<String>,
    row_signatures: Vec<String>,
    conversation: conversation::incremental::Incremental,
    row_assets: Vec<row_patch::RowAssets>,
    expanded: HashSet<String>,
    orq_days: HashSet<String>,
    table_column: HashMap<String, usize>,
    tables: HashMap<String, (String, std::rc::Rc<[crate::tables::Table]>)>,
    last_message: Option<usize>,
    live_clear_epoch: [u64; 2],
    rich: HashMap<String, RichText>,
    prepared: HashMap<String, Prepared>,
    cites: CiteCheck,
    visible_preview: Preview,
    pages: page_card::Pages,
    turn_seen: Option<Instant>,
    arrived: HashMap<String, Instant>,
    tree_parts: HashSet<String>,
    part_arrived: HashMap<String, Instant>,
    tree_folds: HashMap<String, (bool, Option<Instant>)>,
    tree_motion: bool,
    stats: Option<Stats>,
    composer: Entity<TextareaState>,
    composer_placeholder: String,
    ask_form: AskForm,
    ask_scroll: (String, ScrollHandle),
    plan_scroll: (String, ScrollHandle),
    plan_view: Option<(String, Entity<TextViewState>)>,
    plan_review: plan_review::ReviewState,
    suggest_pick: usize,
    slash_open: bool,
    suggest_dismissed: Option<String>,
    mention: mention::Mention,
    command_panel: bool,
    context_card: bool,
    confirm: Option<Confirm>,
    confirm_no_ask: bool,
    terminal_suggestion: String,
    recent: Option<Recent>,
    plugin_band: Value,
    plugin_panes: Vec<Value>,
    plugin_shown: Option<Option<String>>,
    plugin_columns: Option<f64>,
    plugin_source: Option<crate::plugin_ui::UiSource>,
    plugin_caps: Vec<String>,
    plugin_local_tab: Option<String>,
    plugin_question_seen: Option<(Option<SessionKey>, String)>,
    plugin_tabs_seen: Option<String>,
    plugin_tabs_waits: u8,
    plugin_hovered: HashSet<String>,
    plugin_fields: HashMap<String, crate::plugin_ui::Field>,
}

impl ConversationSlot {
    pub(super) fn new(session: SessionInfo, api: Option<Api>, composer: Entity<TextareaState>, window: &Window) -> Self {
        Self {
            selected: Some(session), open_api: api, chat: Chat::default(), chat_online: true, loading: true, history_started: true,
            history_installed: false, history_limit: EVENTS, has_older: false, error: None,
            list_state: ListState::new(0, ListAlignment::Bottom, px(300.)).hold_tail(px(160.)), rail_hover: None, follow: Default::default(),
            row_ids: Vec::new(), row_signatures: Vec::new(), conversation: Default::default(), row_assets: Vec::new(),
            expanded: HashSet::new(), orq_days: HashSet::new(), table_column: HashMap::new(), tables: HashMap::new(), last_message: None,
            live_clear_epoch: [0; 2], rich: HashMap::new(), prepared: HashMap::new(), cites: CiteCheck::default(),
            visible_preview: Preview::default(), pages: page_card::Pages::new(window.window_handle()), turn_seen: None,
            arrived: HashMap::new(), tree_parts: HashSet::new(), part_arrived: HashMap::new(), tree_folds: HashMap::new(), tree_motion: false,
            stats: None, composer, composer_placeholder: String::new(), ask_form: AskForm::default(), ask_scroll: Default::default(),
            plan_scroll: Default::default(), plan_view: None, plan_review: Default::default(), suggest_pick: 0, slash_open: false,
            suggest_dismissed: None, mention: Default::default(), command_panel: false, context_card: false, confirm: None,
            confirm_no_ask: false, terminal_suggestion: String::new(), recent: None, plugin_band: Value::Null, plugin_panes: Vec::new(),
            plugin_shown: None, plugin_columns: None, plugin_source: None, plugin_caps: Vec::new(), plugin_local_tab: None,
            plugin_question_seen: None, plugin_tabs_seen: None, plugin_tabs_waits: 0, plugin_hovered: HashSet::new(),
            plugin_fields: HashMap::new(),
        }
    }

    /// A chave de um clique é desta conversa: id de linha ou de evento, com ou sem o sufixo da parte.
    pub(super) fn owns(&self, key: &str) -> bool {
        let base = key.split([':', '#']).next().unwrap_or(key);
        let base = base.strip_prefix(PINNED).unwrap_or(base);
        self.row_ids.iter().any(|id| id == base || id == key) || self.chat.events.iter().any(|event| event.id == base)
    }
}

pub(super) enum TabPreviewReply {
    History(u64, Result<api::History, Failure>),
    Wallpaper(Result<Option<Arc<RenderImage>>, Failure>),
}

/// Papel de parede no modo Janela: o app não o desenha (o desktop aparece por trás), mas a prévia cobre a conversa e
/// precisa da foto para ficar igual ao resto da janela.
fn wallpaper_window() -> bool {
    let a = appearance::get();
    a.background == appearance::Background::Desktop && a.wallpaper == appearance::Wallpaper::Window
}

pub(super) struct TabPreview {
    target: Target,
    seq: u64,
    /// `None` enquanto o pacote está dentro do `Hangar` (desenho ou evento da prévia).
    slot: Option<ConversationSlot>,
    etag: Option<String>,
    loaded: bool,
    busy: bool,
    error: Option<String>,
    hovered: bool,
    /// A pessoa clicou ou digitou dentro da prévia.
    engaged: bool,
    /// Eventos pedidos ao `/history`; sobe a cada página anterior.
    limit: usize,
    /// Chegar ao topo pede a página anterior uma vez; só rola para longe do topo arma de novo.
    older_armed: bool,
    /// A linha do topo antes da página anterior chegar: ela continua na tela depois.
    older_anchor: Option<String>,
    /// O modo de permissão vem do SSE na principal; aqui, da leitura do ciclo, uma vez.
    mode_read: bool,
    _composer: Subscription,
    _mention: Subscription,
    _poll: Task<()>,
}

impl Hangar {
    /// Roda `f` com a conversa da prévia no lugar da principal e destroca antes de devolver.
    pub(super) fn with_tab_preview_slot<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Context<Self>) -> R) -> Option<R> {
        let mut slot = self.tab_preview.as_mut()?.slot.take()?;
        self.swap_conversation(&mut slot);
        let inside = std::mem::replace(&mut self.in_tab_preview, true);
        let result = f(self, cx);
        self.in_tab_preview = inside;
        self.swap_conversation(&mut slot);
        if let Some(preview) = self.tab_preview.as_mut() { preview.slot = Some(slot); }
        Some(result)
    }

    /// Como `with_tab_preview_slot`, mas também dentro de um evento da prévia, quando a conversa dela já está no lugar.
    fn in_tab_preview_slot<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Context<Self>) -> R) -> Option<R> {
        if self.tab_preview_parked.is_some() { return Some(f(self, cx)); }
        self.with_tab_preview_slot(cx, f)
    }

    /// Clique ou tecla dentro da prévia: a conversa dela entra no lugar até o fim deste evento, para todo handler do chat
    /// (enviar, colar, anexar, modelo, permissão, histórico) agir na sessão dela.
    pub(super) fn enter_tab_preview_event(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_preview_parked.is_some() { return; }
        let Some(mut slot) = self.tab_preview.as_mut().and_then(|p| p.slot.take()) else { return; };
        self.swap_conversation(&mut slot);
        self.tab_preview_parked = Some(slot);
        self.in_tab_preview = true;
        cx.defer_in(window, |this, _, _| this.leave_tab_preview_event());
    }

    /// Devolve a conversa principal ao lugar; sem evento da prévia em curso, não faz nada.
    pub(super) fn leave_tab_preview_event(&mut self) {
        let Some(mut parked) = self.tab_preview_parked.take() else { return; };
        self.swap_conversation(&mut parked);
        self.in_tab_preview = false;
        // A prévia pode ter fechado no meio do evento: o pacote dela vai junto.
        if let Some(preview) = self.tab_preview.as_mut() { preview.slot = Some(parked); }
    }

    /// Clique de linha que veio da prévia: abrir e fechar mexe na conversa dela, não na principal.
    pub(super) fn tab_preview_owns(&self, key: &str) -> bool {
        self.tab_preview.as_ref().and_then(|p| p.slot.as_ref()).is_some_and(|slot| slot.owns(key))
    }

    pub(super) fn toggle_in_tab_preview(&mut self, key: String, cx: &mut Context<Self>) -> bool {
        if self.in_tab_preview || !self.tab_preview_owns(&key) { return false; }
        self.with_tab_preview_slot(cx, |this, cx| this.toggle_here(key, cx));
        true
    }

    /// Dono que o ditado teria dentro da prévia aberta (a sessão dela).
    pub(super) fn tab_preview_dictation_owner(&mut self, cx: &mut Context<Self>) -> Option<SessionOwner> {
        self.tab_preview.as_ref()?;
        self.in_tab_preview_slot(cx, |this, cx| this.dictation_owner(cx)).flatten()
    }

    pub(super) fn tab_preview_copy_text(&self, id: &str) -> Option<String> {
        let slot = self.tab_preview.as_ref()?.slot.as_ref()?;
        slot.chat.events.iter().find(|event| event.id == id).map(ChatEvent::body)
    }

    /// Painel do compositor aberto pela prévia: o da raiz é desenhado com a conversa dela.
    pub(super) fn tab_preview_owns_popup(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(slot) = self.tab_preview.as_ref().and_then(|p| p.slot.as_ref()) else { return false; };
        if slot.command_panel || slot.context_card || slot.recent.is_some() { return true; }
        let Some(open) = self.ctl_snapshot() else { return false; };
        let key = self.with_tab_preview_slot(cx, |this, _| this.selected_key()).flatten();
        key.as_ref() == Some(open.session())
    }

    /// O painel aberto pela prévia é desenhado na raiz, fora da caixa dela: a camada cobre a janela para que todo clique
    /// enquanto ele está aberto (nele ou no fundo que o fecha) troque para a conversa da prévia antes de agir.
    pub(super) fn render_tab_preview_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.with_tab_preview_slot(cx, |this, cx| this.render_popup(window, cx)).flatten()?;
        Some(div().absolute().inset_0()
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| this.enter_tab_preview_event(window, cx)))
            .capture_any_mouse_up(cx.listener(|this, _, window, cx| this.enter_tab_preview_event(window, cx)))
            .child(popup).into_any_element())
    }

    /// Foco no campo só segura a prévia depois que a pessoa clicou ou digitou nela: foco que chegou sozinho não conta.
    fn tab_preview_typing(&self, window: &Window, cx: &App) -> bool {
        let Some(preview) = self.tab_preview.as_ref() else { return false; };
        preview.slot.as_ref().is_some_and(|slot| {
            let input = slot.composer.read(cx);
            (preview.engaged && input.focus_handle(cx).is_focused(window)) || !input.value().trim().is_empty()
        })
    }

    fn tab_preview_held(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        self.tab_preview_typing(window, cx) || self.tab_preview_owns_popup(cx) || self.dictation_in_tab_preview(cx)
    }

    /// Roda do mouse ou clique na faixa: a prévia sai e só volta depois de mexer e parar sobre uma aba.
    pub(super) fn cancel_tab_hover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_hover_epoch += 1;
        self.tab_hover = None;
        if !self.tab_preview.as_ref().is_some_and(|p| !p.hovered) { return; }
        // Segurada (texto, painel, ditado), ela fica; o prazo de fechar volta a contar para ela sair quando soltar.
        if self.tab_preview_held(window, cx) { self.schedule_tab_preview_close(self.tab_hover_epoch, window, cx); }
        else { self.close_tab_preview(cx); }
    }

    /// A aba sob o mouse na faixa mudou (`None`: saiu das abas). Pela posição, não por entrar e sair de cada aba: o que é
    /// pintado por cima da aba (o avatar animado) não a faz perder o mouse, e voltar à mesma aba depois de fechar reabre.
    pub(super) fn tab_pointer(&mut self, target: Option<Target>, window: &mut Window, cx: &mut Context<Self>) {
        if !appearance::get().tab_preview { return; }
        self.tab_hover_moved = Instant::now();
        if self.tab_hover == target { return; }
        self.tab_hover_epoch += 1;
        let epoch = self.tab_hover_epoch;
        self.tab_hover = target.clone();
        // Fora das abas, ou na sessão aberta (que já está na tela): a prévia aberta fecha pelo prazo de sempre.
        let Some(target) = target.filter(|t| self.selected_target().as_ref() != Some(t)) else {
            self.schedule_tab_preview_close(epoch, window, cx);
            return;
        };
        let open = self.tab_preview.as_ref().map(|preview| preview.target == target);
        match open {
            Some(true) => {}
            // Com uma aberta, passar para outra aba troca na hora.
            // Segurada, ela não troca: passar por outra aba no caminho levaria o rascunho ou o ditado dela.
            Some(false) if self.tab_preview_held(window, cx) => {}
            Some(false) => self.open_tab_preview(target, window, cx),
            // Abre só com o mouse parado sobre a aba: passar por ela, mesmo devagar, não abre.
            None => {
                cx.spawn_in(window, async move |this, cx| {
                    let mut wait = SHOW_DELAY;
                    loop {
                        cx.background_executor().timer(wait).await;
                        let Ok(Some(rest)) = this.update_in(cx, |this, window, cx| {
                            if this.tab_hover_epoch != epoch || this.tab_hover.as_ref() != Some(&target)
                                || this.selected_target().as_ref() == Some(&target) { return None; }
                            let still = this.tab_hover_moved.elapsed();
                            if still < SHOW_DELAY { return Some(SHOW_DELAY - still); }
                            this.open_tab_preview(target.clone(), window, cx);
                            None
                        }) else { return };
                        wait = rest;
                    }
                }).detach();
            }
        }
    }

    fn schedule_tab_preview_close(&mut self, epoch: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_preview.is_none() { return; }
        // Segurada (texto, painel ou ditado da prévia), confere de novo: fecha assim que soltar, com o mouse ainda fora.
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(HIDE_DELAY).await;
            let again = this.update_in(cx, |this, window, cx| {
                if this.tab_hover_epoch != epoch { return false; }
                let Some(preview) = &this.tab_preview else { return false; };
                if preview.hovered || this.tab_hover.as_ref() == Some(&preview.target) { return false; }
                if this.tab_preview_held(window, cx) { return true; }
                this.close_tab_preview(cx);
                false
            }).unwrap_or(false);
            if !again { break; }
        }).detach();
    }

    pub(super) fn close_tab_preview(&mut self, cx: &mut Context<Self>) {
        // Fechar no meio de um evento da prévia: a principal volta antes, e o pacote da prévia sai com ela.
        self.leave_tab_preview_event();
        if self.tab_preview.take().is_some() { cx.notify(); }
    }

    fn open_tab_preview(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.target_session(&target).cloned() else { return; };
        self.leave_tab_preview_event();
        // Como a principal: `open_api` só existe para sessão de outra máquina.
        let api = (!self.is_active_key(&target.server)).then(|| self.machine_api(&target.server)).flatten();
        let composer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 10).submit_on_enter(true));
        let composer_sub = cx.subscribe_in(&composer, window, |this, _, event: &InputEvent, window, cx| {
            this.in_tab_preview_slot(cx, |this, cx| this.composer_event(event, window, cx));
        });
        let mention = cx.observe(&composer, |this, _, cx| { this.in_tab_preview_slot(cx, |this, cx| this.refresh_mention(cx)); });
        let slot = ConversationSlot::new(session, api, composer, window);
        self.tab_preview_seq += 1;
        let poll = cx.spawn(async move |this, cx| loop {
            if this.update(cx, |this, cx| this.fetch_tab_preview(cx)).is_err() { break; }
            cx.background_executor().timer(POLL_EVERY).await;
        });
        self.tab_preview = Some(TabPreview {
            target, seq: self.tab_preview_seq, slot: Some(slot), etag: None, loaded: false, busy: false, error: None, hovered: false, engaged: false,
            limit: EVENTS, older_armed: false, older_anchor: None, mode_read: false,
            _composer: composer_sub, _mention: mention, _poll: poll,
        });
        self.load_tab_preview_wallpaper();
        cx.notify();
    }

    /// O ciclo de permissão da sessão da prévia, pela leitura sem sonda da pílula: a resposta entra no `controls.known`
    /// pela chave da sessão, e a pílula da prévia mostra o modo.
    fn read_tab_preview_mode(&self, api: Api, key: SessionKey) {
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["permission-modes"], &[], 45).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Catalog(Ctl::Mode), result) }).await;
        });
    }

    /// A foto do papel de parede, uma vez, pela mesma rota do modo Vidro.
    fn load_tab_preview_wallpaper(&mut self) {
        if !wallpaper_window() || self.tab_preview_wallpaper.is_some() || self.tab_preview_wallpaper_loading { return; }
        let Some(api) = self.desktop_api() else { return; };
        self.tab_preview_wallpaper_loading = true;
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = match api.desktop_wallpaper().await {
                Ok(bytes) => tokio::task::spawn_blocking(move || crate::media::backdrop(&bytes)).await.ok().flatten()
                    .map(Some).ok_or_else(|| Failure::local("backdrop_invalid")),
                Err(error) => Err(error),
            };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::TabPreview(TabPreviewReply::Wallpaper(result)) }).await;
        });
    }

    /// Uma batida da consulta; com um pedido em voo, a batida não pede outro.
    fn fetch_tab_preview(&mut self, cx: &mut Context<Self>) {
        let Some((target, seq, etag, limit)) = self.tab_preview.as_ref().filter(|p| !p.busy)
            .map(|p| (p.target.clone(), p.seq, p.etag.clone(), p.limit)) else { return; };
        let Some(api) = self.machine_api(&target.server) else {
            let error = self.machine_error(&target.server);
            if let Some(preview) = self.tab_preview.as_mut() { (preview.error, preview.loaded) = (Some(error), true); }
            cx.notify();
            return;
        };
        if let Some(preview) = self.tab_preview.as_mut() { preview.busy = true; }
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.history(&target.name, limit, etag.as_deref()).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::TabPreview(TabPreviewReply::History(seq, result)) }).await;
        });
    }

    /// As linhas da conversa da prévia pelo mesmo caminho da principal, sem o que é da sessão aberta (Atividade,
    /// conferência de citações).
    pub(super) fn sync_tab_preview_rows(&mut self, cx: &mut Context<Self>) {
        let a = appearance::get();
        let stable = self.chat.take_unsynced();
        let delta = self.conversation.update(&self.chat.events, stable, conversation::View {
            thinking: a.thinking_tools, tasks: a.task_list,
            merge_thinking: a.tool_look == appearance::ToolLook::Tree,
            every_run_groups: a.tool_look == appearance::ToolLook::Terminal,
        });
        self.sync_row_ids(Some(delta), cx);
    }

    pub(super) fn receive_tab_preview(&mut self, reply: TabPreviewReply, cx: &mut Context<Self>) {
        let (seq, result) = match reply {
            TabPreviewReply::History(seq, result) => (seq, result),
            TabPreviewReply::Wallpaper(result) => {
                self.tab_preview_wallpaper_loading = false;
                // Sem a foto, a prévia fica no fundo cheio do tema; o motivo vai ao log, a tela não muda por ele.
                match result {
                    Ok(image) => self.tab_preview_wallpaper = image,
                    Err(error) => eprintln!("prévia da aba sem papel de parede: {}", Self::fetch_failure(&error)),
                }
                cx.notify();
                return;
            }
        };
        let Some(target) = self.tab_preview.as_ref().map(|p| p.target.clone()) else { return; };
        let session = self.target_session(&target).cloned();
        let Some(preview) = self.tab_preview.as_mut() else { return; };
        if seq != preview.seq { return; }
        preview.busy = false;
        match result {
            Ok(history) => {
                (preview.etag, preview.error, preview.loaded) = (history.etag, None, true);
                let (limit, anchor) = (preview.limit, preview.older_anchor.take());
                let read_mode = !std::mem::replace(&mut preview.mode_read, true);
                // A lista é a fonte do estado ao vivo da sessão (trabalhando, aguardando), sem SSE próprio.
                let mode = self.with_tab_preview_slot(cx, |this, cx| {
                    if let Some(session) = session {
                        this.chat.state.state = session.state.clone();
                        this.selected = Some(session);
                    }
                    if let Some(events) = history.events {
                        this.has_older = events.len() >= limit;
                        this.chat.merge_history(events);
                    }
                    (this.loading, this.history_installed) = (false, true);
                    this.sync_tab_preview_rows(cx);
                    // Página anterior chegou: a linha que estava no topo continua na tela.
                    if let Some(index) = anchor.and_then(|id| this.row_ids.iter().position(|row| *row == id)) {
                        this.list_state.scroll_to(ListOffset { item_ix: index, offset_in_item: px(0.) });
                    }
                    // Só o Claude tem o ciclo lido sem mexer no terminal; o modo do Codex vem só do SSE.
                    read_mode.then(|| this.session_api().zip(this.selected_key()).filter(|_| this.provider().0 == "claude")).flatten()
                }).flatten();
                if let Some((api, key)) = mode { self.read_tab_preview_mode(api, key); }
            }
            Err(error) => (preview.error, preview.loaded) = (Some(Self::fetch_failure(&error)), true),
        }
        cx.notify();
    }

    /// A Aparência mudou o desenho das linhas: refaz as da prévia e remede todas, como a principal.
    pub(super) fn restyle_tab_preview(&mut self, cx: &mut Context<Self>) {
        self.with_tab_preview_slot(cx, |this, cx| {
            this.chat.invalidate();
            this.sync_tab_preview_rows(cx);
            this.list_state.remeasure();
        });
    }

    /// A conversa da prévia como a da área principal: a folha da Leitura e a mesma lista, cada linha desenhada pelo
    /// `render_row` com o pacote da prévia no lugar.
    fn render_tab_preview_conversation(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list_state = self.tab_preview.as_ref()?.slot.as_ref()?.list_state.clone();
        // No topo, a página anterior, como a principal; uma vez por chegada ao topo.
        let top = list_state.logical_scroll_top().item_ix;
        if let Some(preview) = self.tab_preview.as_mut() {
            let has_older = preview.slot.as_ref().is_some_and(|slot| slot.has_older);
            if top > 2 { preview.older_armed = true; }
            else if top == 0 && has_older && preview.older_armed && !preview.busy {
                preview.older_armed = false;
                preview.older_anchor = preview.slot.as_ref().and_then(|slot| slot.row_ids.first().cloned());
                (preview.limit, preview.etag) = (preview.limit + EVENTS, None);
                let view = cx.entity().downgrade();
                cx.defer(move |cx| { let _ = view.update(cx, |this, cx| this.fetch_tab_preview(cx)); });
            }
        }
        let view = cx.entity().downgrade();
        let sheet = (appearance::get().effective_reading() == appearance::Reading::Sheet).then(|| div().absolute().inset_0()
            .px(px(16.)).pt(px(4.)).pb(px(8.)).flex().justify_center()
            .child(column_box(false).h_full().rounded(px(14.)).border_1().border_color(theme::border())
                .bg(theme::sheet()).shadow(theme::sheet_shadow())));
        Some(div().relative().size_full().flex().flex_col()
            .children(sheet)
            .child(list(list_state, move |i, window, cx| {
                view.update(cx, |this, cx| this.with_tab_preview_slot(cx, |this, cx| this.render_row(i, window, cx)))
                    .ok().flatten().unwrap_or_else(|| div().into_any_element())
            }).flex_1().min_h_0())
            .into_any_element())
    }

    pub(super) fn render_tab_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !appearance::get().tab_preview { return None; }
        // Diálogo aberto (o visor de imagem que a prévia abriu, por exemplo): a prévia, por cima de tudo, o esconderia.
        if window.has_active_dialog(cx) { return None; }
        let preview = self.tab_preview.as_ref()?;
        let (target, loaded, error) = (preview.target.clone(), preview.loaded, preview.error.clone());
        let (left, top, bottom) = self.tab_anchor(&target)?;
        let below = appearance::get().navigation != appearance::Navigation::BottomTabs;

        let faint = |text: String| div().size_full().flex().items_center().justify_center().text_sm().text_color(theme::faint()).child(text).into_any_element();
        let body = match (loaded, error) {
            (false, _) => faint(tr("loading")),
            (true, Some(error)) => div().size_full().p_4().text_sm().text_color(theme::warning()).child(error).into_any_element(),
            (true, None) => self.render_tab_preview_conversation(cx).unwrap_or_else(|| faint(tr("loading"))),
        };
        // A mesma faixa de baixo do chat (barra do plano, perguntas, fila, compositor), com a conversa da prévia no lugar.
        let bottom_area = self.with_tab_preview_slot(cx, |this, cx| this.render_bottom_content(window, cx));

        let viewport = window.viewport_size();
        let width = px(WIDTH).min(viewport.width - px(32.));
        let height = px(HEIGHT).min(viewport.height - px(140.));
        // Posição calculada aqui, não pelo `snap` do `anchored`: o papel de parede de dentro precisa dela para alinhar.
        let x = left.min(viewport.width - width - px(8.)).max(px(8.));
        let y = if below { bottom + px(8.) } else { top - px(8.) - height }.max(px(8.));
        // A foto do fundo (arquivo, vidro ou o papel de parede do modo Janela) do tamanho da janela e deslocada pela
        // posição da prévia: dentro dela aparece o mesmo pedaço que aparece atrás do app, sob o mesmo tom da janela.
        // Sem foto, fundo cheio: a conversa de trás não atravessa.
        let image = self.backdrop.as_ref().map(|(_, image)| image.clone())
            .or_else(|| wallpaper_window().then(|| self.tab_preview_wallpaper.clone()).flatten());
        let backdrop = match image {
            // As mesmas camadas da janela sobre o desktop: o tom dela, o véu e o grão do fundo por cima da foto.
            Some(image) => div().absolute().inset_0().overflow_hidden()
                .child(img(image).absolute().left(-x).top(-y).w(viewport.width).h(viewport.height).object_fit(ObjectFit::Cover))
                .child(div().absolute().inset_0().bg(theme::window_fill()))
                .child(div().absolute().inset_0().bg(theme::veil()))
                .child(self.grain_layer(size(width, height))),
            None => div().absolute().inset_0().bg(theme::background().alpha(1.)),
        };
        let content = div().id("tab-preview").relative().w(width).h(height).flex().flex_col().overflow_hidden()
            .rounded(px(14.)).border_1().border_color(theme::glass_border()).shadow(theme::popover_shadow())
            .occlude()
            .child(backdrop)
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                if let Some(preview) = this.tab_preview.as_mut() { preview.engaged = true; }
                this.enter_tab_preview_event(window, cx);
            }))
            .capture_any_mouse_up(cx.listener(|this, _, window, cx| this.enter_tab_preview_event(window, cx)))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && !this.tab_preview_owns_popup(cx) {
                    this.close_tab_preview(cx);
                    cx.stop_propagation();
                    return;
                }
                if let Some(preview) = this.tab_preview.as_mut() { preview.engaged = true; }
                this.enter_tab_preview_event(window, cx);
            }))
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if let Some(preview) = this.tab_preview.as_mut() { preview.hovered = *hovered; }
                this.tab_hover_epoch += 1;
                if !*hovered { let epoch = this.tab_hover_epoch; this.schedule_tab_preview_close(epoch, window, cx); }
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| if !this.tab_preview_owns_popup(cx) { this.close_tab_preview(cx); }))
            .child(div().relative().flex_1().min_h_0().pt(px(8.)).child(body))
            .children(bottom_area.map(|area| div().relative().flex_shrink_0().child(area)));
        Some(deferred(anchored().position_mode(AnchoredPositionMode::Window).position(point(x, y)).child(content)).into_any_element())
    }
}
