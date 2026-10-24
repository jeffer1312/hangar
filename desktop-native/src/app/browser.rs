//! Navegador do painel lateral: barra com voltar, avançar, recarregar e endereço, e a página embaixo.
//! O comportamento da barra (Enter navega, Esc desiste e devolve à página) vem do Zeron (MIT, crates/ui/src/browser/view.rs).
use std::{cell::Cell, rc::Rc};

use super::*;
use crate::appearance::SideTab;
use crate::browser::{Engine, Pointer, model};

#[cfg(not(target_os = "macos"))]
mod pick;

/// Por que o agente está parado: o botão Pausar ou a pessoa mexendo na página (Assumir, clique, tecla).
#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy, PartialEq)]
enum Pause { Asked, TookOver }

/// Faixa "agente no controle": some este tempo depois do último verbo terminar.
#[cfg(not(target_os = "macos"))]
const AGENT_LINGER: std::time::Duration = std::time::Duration::from_secs(2);

/// Quem pediu a navegação: só a pessoa leva o foco à página e troca o endereço que ela está digitando.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum By { User, Agent }
/// Tempo estacionada e parada até a WebView do Windows deixar de compor.
#[cfg(target_os = "windows")]
const SLEEP_AFTER: std::time::Duration = std::time::Duration::from_secs(10);
/// Tempo para a WebView acordada voltar a compor antes do verbo.
#[cfg(target_os = "windows")]
const WAKE_SETTLE: std::time::Duration = std::time::Duration::from_millis(200);

/// Pedido do painel ao `Hangar`: o print da página, ou o elemento escolhido com a foto dele, vai para o compositor da
/// sessão.
pub(super) enum PanelEvent {
    Shot { key: String, bytes: Vec<u8> },
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Element { key: String, element: crate::context_refs::Element, png: Option<Vec<u8>> },
}

impl EventEmitter<PanelEvent> for BrowserPanel {}

#[cfg(target_os = "linux")]
type Ctl =Rc<crate::browser::control::Controller<crate::browser::control::CdpPage>>;

/// Janela que a página abriu com `window.opener` (login OAuth): ocupa o painel, e o agente age nela, até fechar.
#[cfg(target_os = "linux")]
struct Popup {
    engine: Rc<Engine>,
    /// Controlador, CDP e atividade da página de origem, devolvidos quando o popup fecha.
    opener: (Option<Ctl>, Option<Rc<crate::browser::cdp::Cdp>>, Option<Task<()>>),
    _drain: Task<()>,
}

/// Erros guardados para a lista do selo; a contagem segue além disso.
const ERRORS_KEPT: usize = 100;

pub(super) struct BrowserPanel {
    /// `servidor::sessão` dona deste navegador: nome do sidecar que o hangar-preview lê.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    key: String,
    #[cfg(not(target_os = "macos"))]
    controller: Option<Rc<crate::browser::control::Controller<crate::browser::control::CdpPage>>>,
    /// CDP do motor, que o controlador usa.
    #[cfg(not(target_os = "macos"))]
    cdp: Option<Rc<crate::browser::cdp::Cdp>>,
    /// CDP do repasse da tela remota (`browser::relay`), que não passa pelo turno do controlador. No Windows é o do
    /// motor; no Linux é uma sessão própria no alvo, criada no primeiro espectador, com o screencast dele.
    #[cfg(not(target_os = "macos"))]
    relay: std::cell::RefCell<Option<Rc<crate::browser::cdp::Cdp>>>,
    /// Quem olha a tela remota; os ouvintes de evento do CDP guardam só referência fraca.
    #[cfg(not(target_os = "macos"))]
    viewer: Rc<std::cell::RefCell<Option<crate::browser::relay::Viewer>>>,
    /// Canal do espectador atual, para a thread do pipe entregar os quadros sem passar pela interface.
    #[cfg(target_os = "linux")]
    frames: std::sync::Arc<std::sync::Mutex<Option<async_channel::Sender<crate::browser::relay::ViewerEvent>>>>,
    /// Nasce na primeira navegação, que tem a janela.
    engine: Option<Result<Rc<Engine>, String>>,
    #[cfg(target_os = "linux")]
    popup: Option<Popup>,
    /// Endereço a abrir quando o motor terminar de nascer; `Some` enquanto ele nasce.
    starting: Option<(String, By)>,
    _start: Option<Task<()>>,
    events: async_channel::Sender<crate::browser::Event>,
    address: Entity<InputState>,
    /// Resposta do `prompt` da página, na caixa do diálogo.
    dialog_input: Entity<InputState>,
    page: model::PageState,
    /// Endereço recusado, mostrado embaixo da barra.
    invalid: Option<String>,
    /// Status da importação do login do Chrome, mostrado embaixo da barra.
    #[cfg(not(target_os = "macos"))]
    cookies_status: Option<String>,
    /// Login pedido com o motor ainda nascendo: a importação segue quando o CDP dele existir.
    #[cfg(not(target_os = "macos"))]
    import_pending: bool,
    /// Cookies sendo buscados (disco ou depuração) até `apply_cookies`: a linha de status não pode sumir no meio.
    #[cfg(not(target_os = "macos"))]
    importing: bool,
    /// Site escolhido no diálogo: só os cookies dele e dos domínios-pai entram. `None` = todos os sites.
    #[cfg(not(target_os = "macos"))]
    import_site: Option<String>,
    /// Site cuja página mostrou campo de senha: a linha que sugere trazer o login. Nunca importa sozinha.
    #[cfg(not(target_os = "macos"))]
    login_hint: Option<String>,
    /// Último site conferido atrás de campo de senha, para não repetir a cada evento da mesma página.
    #[cfg(not(target_os = "macos"))]
    hint_checked: Option<String>,
    /// Pedido do agente (`hangar-preview login`) esperando a pessoa: site (`None` = todos) e quem responde.
    #[cfg(not(target_os = "macos"))]
    login_request: Option<(Option<String>, futures::channel::oneshot::Sender<crate::browser::control::Reply>)>,
    /// Pedido do agente já aprovado: recebe só a contagem ou o motivo quando a importação termina.
    #[cfg(not(target_os = "macos"))]
    login_reply: Option<(Option<String>, futures::channel::oneshot::Sender<crate::browser::control::Reply>)>,
    /// Perfis com cookies em disco esperando a pessoa escolher de qual trazer.
    #[cfg(target_os = "linux")]
    cookie_sources: Vec<crate::browser::chrome_import::disk::Source>,
    /// URL cujo login já foi preenchido com as senhas salvas do Chrome, para não repetir a cada evento da mesma página.
    #[cfg(target_os = "linux")]
    last_fill: Option<String>,
    focus: FocusHandle,
    /// Origem da página no último desenho: o ponteiro chega ao motor relativo a ela.
    origin: Rc<Cell<Point<Pixels>>>,
    /// Tela emulada pelo `layout` do agente (largura, altura em px da página): desenhada encaixada, sem esticar.
    #[cfg(target_os = "linux")]
    emulated: Option<(f32, f32)>,
    /// px da página por px do painel na tela emulada; 1 sem emulação.
    page_scale: Rc<Cell<f32>>,
    /// Área visível da página na tela emulada (`visualViewport`: zoom e deslocamento). Página sem `viewport` de celular
    /// abre afastada, e o `Input` do CDP conta em px do layout. Relida a cada pergunta do cursor.
    #[cfg(target_os = "linux")]
    page_zoom: (f32, f32, f32),
    /// Decidido pelo `Hangar` a cada quadro (`browser_visible`).
    shown: bool,
    /// Último quadro da página, mostrado no lugar dela enquanto um diálogo ou aviso a cobre (Windows: a página nativa
    /// fica por cima de tudo e precisa sair da frente).
    #[cfg(target_os = "windows")]
    still: Option<Arc<Image>>,
    /// Quadros que saíram da tela: o `Hangar` os tira do cache e do atlas da GPU, que não os soltam sozinhos.
    #[cfg(target_os = "windows")]
    released: Vec<Arc<Image>>,
    /// Sobe a cada vez que o painel é coberto: captura de uma cobertura anterior que chega atrasada é descartada.
    #[cfg(target_os = "windows")]
    cover_gen: u32,
    /// Aviso do próprio painel (preenchimento do login que falhou), ao lado do `notice` que vem da página.
    local_notice: Option<String>,
    /// Cursor da página sob o mouse: o Chromium sem janela não avisa quando ele muda, então a página é perguntada.
    #[cfg(target_os = "linux")]
    cursor: CursorStyle,
    /// Pergunta do cursor em andamento e o último ponto pedido: uma pergunta por vez, sempre do ponto mais novo.
    #[cfg(target_os = "linux")]
    cursor_probe: (bool, Option<Point<Pixels>>),
    /// Erros da página desde a última navegação (JS não tratado, `console.error`, recurso que falhou), para o selo.
    page_errors: std::collections::VecDeque<String>,
    error_count: usize,
    errors_open: bool,
    /// O print da página está sendo tirado: o botão fica ocupado.
    shooting: bool,
    /// Modo "Escolher elemento".
    #[cfg(not(target_os = "macos"))]
    pick: pick::Picking,
    /// O endereço acabou de ir para a área de transferência: o ícone vira um visto por um instante.
    copied: bool,
    /// Aviso que a pessoa fechou no ×: o motor reenvia o estado inteiro, e o aviso voltaria a cada evento.
    dismissed_notice: u32,
    /// O agente navegou com a pessoa digitando no endereço: ao sair do campo, a barra volta ao endereço da página.
    address_stale: bool,
    /// O que o agente está fazendo, enquanto a faixa aparece.
    #[cfg(not(target_os = "macos"))]
    agent: Option<String>,
    /// Verbos do agente em andamento; a faixa só começa a sumir com zero.
    #[cfg(not(target_os = "macos"))]
    agent_running: u32,
    /// Muda a cada ação: o sumiço agendado por uma ação velha não apaga a faixa de uma nova.
    #[cfg(not(target_os = "macos"))]
    agent_seq: u64,
    #[cfg(not(target_os = "macos"))]
    paused: Option<Pause>,
    /// A sessão dona deste navegador está trabalhando: a faixa fica entre uma ação e outra, sem subir e descer.
    #[cfg(not(target_os = "macos"))]
    pub(super) session_working: bool,
    /// Fim do último verbo: aviso de entrada da página que chega logo depois é eco da entrada sintética do agente.
    #[cfg(target_os = "windows")]
    agent_finished_at: Option<std::time::Instant>,
    #[cfg(not(target_os = "macos"))]
    _activity: Option<Task<()>>,
    /// Conta os pedidos de dormir: só o último, sem uso desde então, esconde a WebView.
    #[cfg(target_os = "windows")]
    idle: Cell<u64>,
    /// Último estado do sidecar a gravar (chave, endereço, título).
    #[cfg(not(target_os = "macos"))]
    sidecar: std::sync::Arc<std::sync::Mutex<Option<(String, String, String)>>>,
    /// Escala da janela já passada ao controlador; 0 = nenhuma ainda.
    #[cfg(target_os = "linux")]
    scale: Rc<Cell<f32>>,
    _drain: Task<()>,
    _subscriptions: [Subscription; 5],
}

impl BrowserPanel {
    fn new(key: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (events, received) = async_channel::unbounded();
        let address = cx.new(|cx| InputState::new(window, cx).placeholder(tr("browser_address")));
        let dialog_input = cx.new(|cx| InputState::new(window, cx));
        let focus = cx.focus_handle();
        let drain = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = received.recv().await {
                if this.update_in(cx, |this, window, cx| this.receive(event, window, cx)).is_err() { break; }
            }
        });
        let address_focus = address.focus_handle(cx);
        let subscriptions = [
            cx.subscribe_in(&address, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event { this.submit(window, cx); }
            }),
            cx.on_focus(&address_focus, window, |this, _, _| if let Some(engine) = this.engine() { engine.release_focus() }),
            cx.on_blur(&address_focus, window, |this, window, cx| if std::mem::take(&mut this.address_stale) {
                let url = this.page.url.clone().unwrap_or_default();
                this.address.update(cx, |input, cx| input.set_value(url, window, cx));
            }),
            cx.on_focus(&focus, window, |this, _, _| if let Some(engine) = this.engine() { engine.focus(true) }),
            cx.on_blur(&focus, window, |this, _, _| if let Some(engine) = this.engine() { engine.focus(false) }),
        ];
        Self { key, #[cfg(not(target_os = "macos"))] controller: None, #[cfg(not(target_os = "macos"))] cdp: None,
            #[cfg(not(target_os = "macos"))] relay: Default::default(), #[cfg(not(target_os = "macos"))] viewer: Rc::default(), #[cfg(target_os = "linux")] frames: Default::default(), #[cfg(not(target_os = "macos"))] import_pending: false, #[cfg(not(target_os = "macos"))] importing: false, #[cfg(not(target_os = "macos"))] import_site: None, #[cfg(not(target_os = "macos"))] login_hint: None, #[cfg(not(target_os = "macos"))] hint_checked: None, #[cfg(not(target_os = "macos"))] login_request: None, #[cfg(not(target_os = "macos"))] login_reply: None, #[cfg(target_os = "linux")] cookie_sources: Vec::new(), engine: None, #[cfg(target_os = "linux")] popup: None, starting: None, _start: None, events, address, dialog_input, page: model::PageState::default(), invalid: None, #[cfg(not(target_os = "macos"))] cookies_status: None, #[cfg(target_os = "linux")] last_fill: None, focus,
            origin: Rc::default(), #[cfg(target_os = "linux")] emulated: None, page_scale: Rc::new(Cell::new(1.)), #[cfg(target_os = "windows")] still: None, #[cfg(target_os = "windows")] released: Vec::new(), #[cfg(target_os = "windows")] cover_gen: 0, #[cfg(target_os = "linux")] page_zoom: (1., 0., 0.), shown: false, local_notice: None, #[cfg(target_os = "linux")] cursor: CursorStyle::Arrow, #[cfg(target_os = "linux")] cursor_probe: (false, None), page_errors: Default::default(), error_count: 0, errors_open: false, shooting: false, #[cfg(not(target_os = "macos"))] pick: Default::default(), copied: false, dismissed_notice: 0, address_stale: false, #[cfg(not(target_os = "macos"))] agent: None, #[cfg(not(target_os = "macos"))] agent_running: 0,
            #[cfg(not(target_os = "macos"))] agent_seq: 0, #[cfg(not(target_os = "macos"))] paused: None, #[cfg(not(target_os = "macos"))] session_working: false, #[cfg(target_os = "windows")] agent_finished_at: None, #[cfg(not(target_os = "macos"))] _activity: None, #[cfg(target_os = "windows")] idle: Cell::new(0), #[cfg(not(target_os = "macos"))] sidecar: Default::default(), #[cfg(target_os = "linux")] scale: Rc::default(), _drain: drain, _subscriptions: subscriptions }
    }

    /// A página na tela: o popup aberto, senão a da sessão.
    fn engine(&self) -> Option<&Rc<Engine>> {
        #[cfg(target_os = "linux")]
        if let Some(popup) = &self.popup { return Some(&popup.engine); }
        self.engine.as_ref()?.as_ref().ok()
    }

    /// Evento da página da sessão. Com popup aberto ela fica parada atrás dele, e o estado dela é relido quando ele fecha.
    fn receive(&mut self, event: crate::browser::Event, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        match event {
            crate::browser::Event::Popup(target) => return self.adopt_popup(target, window, cx),
            crate::browser::Event::Closed => return,
            _ if self.popup.is_some() => return,
            _ => {}
        }
        self.apply(event, window, cx);
    }

    #[cfg(target_os = "linux")]
    fn receive_popup(&mut self, event: crate::browser::Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            crate::browser::Event::Closed => self.close_popup(window, cx),
            event => {
                self.apply(event, window, cx);
                // Popup que caiu ou se soltou sem o `targetDestroyed` não prende o painel nem o agente nele.
                if self.popup.as_ref().is_some_and(|p| !p.engine.alive()) { self.close_popup(window, cx); }
            }
        }
    }

    /// Motor próprio no popup; a página da sessão sai da tela e o controlador do agente passa a ele.
    #[cfg(target_os = "linux")]
    fn adopt_popup(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Ok(origin)) = &self.engine else { return };
        let (events, received) = async_channel::unbounded();
        let (adopting, origin) = (origin.adopt(target, events), Rc::downgrade(origin));
        cx.spawn_in(window, async move |this, cx| {
            let adopted = adopting.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let engine = match adopted {
                    Ok(engine) => Rc::new(engine),
                    Err(e) if e.starts_with(crate::browser::POPUP_GONE) => return,
                    Err(e) => {
                        eprintln!("[nav] popup da pagina nao abriu: {e}");
                        this.local_notice = Some(tr("browser_popup_failed").replace("{e}", &e));
                        return cx.notify();
                    }
                };
                // A página que abriu o popup foi trocada no meio: ele cai junto (o `Drop` fecha o alvo).
                if !matches!((&this.engine, origin.upgrade()), (Some(Ok(now)), Some(was)) if Rc::ptr_eq(now, &was)) { return; }
                let drain = cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = received.recv().await {
                        if this.update_in(cx, |this, window, cx| this.receive_popup(event, window, cx)).is_err() { break; }
                    }
                });
                if !engine.alive() { return; }
                this.stop_pick(cx);
                this.drop_viewer();
                let opener = match this.popup.take() {
                    // Popup novo no lugar do anterior, que fecha.
                    Some(old) => old.opener,
                    None => {
                        if let Some(origin) = this.engine() { origin.hide(); }
                        (this.controller.take(), this.cdp.take(), this._activity.take())
                    }
                };
                this.popup = Some(Popup { engine: engine.clone(), opener, _drain: drain });
                this.attach(&engine, cx);
                this.apply(crate::browser::Event::State(engine.page()), window, cx);
            });
        }).detach();
    }

    /// Popup fechado (por ele mesmo ou pela pessoa): a página de origem volta à tela com o controlador dela.
    #[cfg(target_os = "linux")]
    fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() { return; }
        self.stop_pick(cx);
        self.drop_viewer();
        let Some(Popup { opener, .. }) = self.popup.take() else { return };
        (self.controller, self.cdp, self._activity) = opener;
        self.scale.set(0.);
        if let Some(ctl) = self.controller.clone() {
            let hidden = !self.shown;
            cx.spawn(async move |_, _| ctl.set_hidden(hidden).await).detach();
        }
        if let Some(origin) = self.engine() { let page = origin.page(); self.apply(crate::browser::Event::State(page), window, cx); }
        cx.notify();
    }

    /// A tela remota do celular segue a sessão CDP de um alvo: na troca de página ela sai, e o próximo espectador entra
    /// na que está na tela.
    #[cfg(target_os = "linux")]
    fn drop_viewer(&self) {
        self.end_viewer();
        self.relay.borrow_mut().take();
    }

    fn apply(&mut self, event: crate::browser::Event, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        if let crate::browser::Event::UserInput = event { return self.user_input(cx); }
        #[cfg(target_os = "windows")]
        if let crate::browser::Event::Escape = event { return self.stop_pick(cx); }
        // Mesma textura com o conteúdo novo: a cópia guardada do painel já a pinta, basta a raiz redesenhar e o quadro
        // ser confirmado depois do desenho. Textura nova (primeiro quadro, outro tamanho) redesenha o painel.
        #[cfg(target_os = "linux")]
        if let crate::browser::Event::Frame = event {
            let Some(engine) = self.engine().cloned() else { return };
            let replaced = engine.texture_replaced();
            let weak = Rc::downgrade(&engine);
            window.on_next_frame(move |_, _| if let Some(engine) = weak.upgrade() { engine.frame_seen() });
            if replaced || !crate::motion::tick_root(cx) { cx.notify(); }
            return;
        }
        if let crate::browser::Event::State(page) = event {
            // Quem está digitando no endereço não perde o texto para a página que acabou de carregar.
            let url = page.url.clone().unwrap_or_default();
            if !self.address.focus_handle(cx).is_focused(window) && self.address.read(cx).value() != url {
                self.address.update(cx, |input, cx| input.set_value(url, window, cx));
            } else if self.address.focus_handle(cx).is_focused(window) && page.url != self.page.url {
                self.address_stale = true;
            }
            if page.url != self.page.url {
                self.local_notice = None;
                // O elemento escolhido é da página em que o modo foi ligado.
                #[cfg(not(target_os = "macos"))]
                self.stop_pick(cx);
            }
            #[cfg(target_os = "linux")]
            if let Some(key) = crate::browser::take_cookie_notice() { self.cookies_status = Some(tr(key)); }
            if page.dialog != self.page.dialog {
                #[cfg(not(target_os = "macos"))]
                if let Some(ctl) = &self.controller { ctl.set_dialog(page.dialog.as_ref().map(|d| (d.kind.clone(), d.message.clone(), d.default.clone()))); }
                if let Some(d) = &page.dialog && d.kind == "prompt" {
                    let default = d.default.clone();
                    self.dialog_input.update(cx, |input, cx| input.set_value(default, window, cx));
                }
            }
            #[cfg(not(target_os = "macos"))]
            let loaded = self.page.loading && !page.loading;
            #[cfg(target_os = "windows")]
            let moved = !page.loading && page.url != self.page.url;
            self.page = page;
            #[cfg(target_os = "linux")]
            if loaded { self.refresh_zoom(cx); }
            #[cfg(target_os = "windows")]
            if (loaded || moved) && let Some(engine) = self.engine() { let history = engine.history(); cx.spawn(async move |_, _| history.await).detach(); }
            #[cfg(not(target_os = "macos"))]
            self.save_sidecar(cx);
            #[cfg(target_os = "linux")]
            self.maybe_autofill(cx);
            #[cfg(not(target_os = "macos"))]
            self.maybe_suggest_login(cx);
        }
        cx.notify();
    }

    /// O arquivo que o hangar-preview lê, gravado fora da thread da tela. A trava serializa as escritas e cada uma grava
    /// o estado mais novo, então o arquivo termina sempre no último.
    #[cfg(not(target_os = "macos"))]
    fn save_sidecar(&self, cx: &mut Context<Self>) {
        let latest = (self.key.clone(), self.page.url.clone().unwrap_or_default(), self.page.title.clone());
        let slot = self.sidecar.clone();
        *slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(latest);
        cx.background_spawn(async move {
            let current = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((key, url, title)) = current.as_ref() { crate::browser::server::write_sidecar(key, url, title); }
        }).detach();
    }

    /// Ao terminar de carregar uma página https cuja origem tem senha salva no Chrome, preenche o login — só com a opção
    /// ligada nas Configurações. Uma vez por URL (o evento de estado repete em SPA). A leitura e a decifração rodam em
    /// thread de fundo; a senha em claro só vive aqui e no campo da página.
    #[cfg(target_os = "linux")]
    fn maybe_autofill(&mut self, cx: &mut Context<Self>) {
        if self.page.loading || !crate::appearance::get().chrome_autofill {
            return;
        }
        let Some(url) = self.page.url.clone() else { return };
        if self.last_fill.as_deref() == Some(url.as_str()) {
            return;
        }
        let Ok(parsed) = url::Url::parse(&url) else { return };
        if parsed.scheme() != "https" {
            return;
        }
        let origin = parsed.origin().ascii_serialization();
        let Some(cdp) = self.cdp.clone() else { return };
        self.last_fill = Some(url.clone());
        let page = url.clone();
        let fetch = cx.background_executor().spawn(async move { crate::browser::chrome_import::credentials_for(&page) });
        cx.spawn(async move |this, cx| {
            let Some((user, pass)) = fetch.await.into_iter().next() else { return };
            let js = crate::browser::chrome_import::inject_login_js(&origin, &user, &pass);
            let params = serde_json::json!({"expression": js, "returnByValue": true});
            // SPA desenha o formulário depois do carregamento: tenta algumas vezes, para na que achou o campo de senha.
            for wait in [0u64, 1500, 4000] {
                if wait > 0 {
                    cx.background_executor().timer(std::time::Duration::from_millis(wait)).await;
                }
                // Redirect ou navegação desde a leitura: a senha era da página anterior, não desta.
                if !this.read_with(cx, |this, _| this.page.url.as_deref() == Some(url.as_str())).unwrap_or(false) {
                    break;
                }
                match cdp.call("Runtime.evaluate", params.clone()).await {
                    Ok(v) => {
                        // Exceção no script (ou página morta) não é "sem campo de senha": loga e para de tentar. Só a
                        // classe e o texto: o resto do detalhe pode trazer a expressão, que tem a senha.
                        if let Some(detail) = v.get("exceptionDetails") {
                            let class = detail["exception"]["className"].as_str().unwrap_or("?");
                            let text = detail["text"].as_str().unwrap_or("");
                            eprintln!("[nav] autofill: exceção no script da página: {class}: {text}");
                            let _ = this.update(cx, |this, cx| { this.local_notice = Some(tr("browser_autofill_failed")); cx.notify(); });
                            break;
                        }
                        if v["result"]["value"].as_bool() == Some(true) {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("[nav] autofill: Runtime.evaluate falhou: {e}");
                        let _ = this.update(cx, |this, cx| { this.local_notice = Some(tr("browser_autofill_failed")); cx.notify(); });
                        break;
                    }
                }
            }
        }).detach();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.address.read(cx).value().to_string();
        match model::normalize_address(&text) {
            Err(key) => self.invalid = Some(tr(key)),
            Ok(url) => {
                // A linha do login fala do site em que foi pedida: some quando a pessoa vai a outro endereço, nunca num
                // redirect da página nem com importação, pedido do agente ou escolha de perfil ainda abertos.
                #[cfg(not(target_os = "macos"))]
                {
                    #[cfg(target_os = "linux")]
                    let picking = !self.cookie_sources.is_empty();
                    #[cfg(not(target_os = "linux"))]
                    let picking = false;
                    if !self.importing && !self.import_pending && self.login_request.is_none() && self.login_reply.is_none() && !picking {
                        self.cookies_status = None;
                    }
                }
                self.go(url, By::User, window, cx)
            }
        }
        cx.notify();
    }

    /// Navega, ou faz o motor nascer e navega quando ele ficar pronto.
    pub(super) fn go(&mut self, url: String, by: By, window: &mut Window, cx: &mut Context<Self>) {
        self.invalid = None;
        // Popup que caiu sem fechar não leva junto a página da sessão.
        #[cfg(target_os = "linux")]
        if self.popup.as_ref().is_some_and(|p| !p.engine.alive()) { self.close_popup(window, cx); }
        // Motor que falhou (ou o Chromium que caiu) volta a ser tentado no próximo `open`.
        #[cfg(not(target_os = "macos"))]
        if matches!(&self.engine, Some(Err(_))) || self.engine().is_some_and(|e| !e.alive()) { self.drop_engine(); }
        if self.engine.is_some() {
            self.navigate(url, by, window, cx);
        } else if self.starting.replace((url, by)).is_none() {
            // Enter repetido enquanto o motor nasce só troca o endereço pendente.
            self.start(window, cx);
        }
    }

    fn navigate(&mut self, url: String, by: By, window: &mut Window, cx: &mut Context<Self>) {
        let Some(engine) = self.engine() else { return };
        engine.load(&url);
        // O agente não apaga o endereço que a pessoa está digitando; o `State` da página o atualiza quando ela sair dali.
        if by == By::User || !self.address.focus_handle(cx).is_focused(window) {
            if by == By::User { self.address_stale = false; }
            self.address.update(cx, |input, cx| input.set_value(url, window, cx));
        } else {
            self.address_stale = true;
        }
        if by == By::User && self.shown { self.focus.focus(window, cx); }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let starter = Engine::prepare(window, cx);
        let events = self.events.clone();
        self._start = Some(cx.spawn_in(window, async move |this, cx| {
            // Corpo da tarefa, fora de `update`: a App não está emprestada, então o laço de mensagens que o WebView2
            // roda até nascer pode executar outras tarefas da GPUI sem pânico.
            // No Linux o motor nasce sem travar a interface; o WebView2 nasce numa chamada síncrona.
            #[cfg(target_os = "linux")]
            let engine = match starter { Ok(starter) => starter.start(events).await, Err(e) => Err(e) }.map(Rc::new);
            #[cfg(not(target_os = "linux"))]
            let engine = starter.and_then(|starter| starter.start(events)).map(Rc::new);
            if let Err(e) = &engine { eprintln!("[nav] motor do navegador falhou: {e}"); }
            // Painel fechado no meio: o motor cai junto com o resultado.
            let _ = this.update_in(cx, |this, window, cx| {
                this.engine = Some(engine);
                // O motor novo conta os avisos do zero.
                this.dismissed_notice = 0;
                #[cfg(not(target_os = "macos"))]
                if let Some(Ok(engine)) = &this.engine { let engine = engine.clone(); this.attach(&engine, cx); }
                // Motor que não nasceu guarda o endereço pedido: é o que "Tentar de novo" volta a abrir.
                match this.starting.take() {
                    Some((url, by)) if this.engine().is_some() => this.navigate(url, by, window, cx),
                    Some((url, _)) => this.page.url = Some(url),
                    None => {}
                }
                #[cfg(not(target_os = "macos"))]
                if std::mem::take(&mut this.import_pending) {
                    if this.cdp.is_some() { this.import_chrome_login(window, cx) } else {
                        let reason = match &this.engine { Some(Err(e)) => e.clone(), _ => "o navegador nao subiu".into() };
                        this.finish_login(Err(reason.clone()));
                        this.cookies_status = Some(tr("browser_cookies_error").replace("{e}", &reason));
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Liga o controlador do hangar-preview ao CDP do motor recém-nascido.
    #[cfg(not(target_os = "macos"))]
    fn attach(&mut self, engine: &Rc<Engine>, cx: &mut Context<Self>) {
        use crate::browser::control::{CdpPage, Controller, EVENTS};
        let cdp = engine.cdp();
        let ctl = Rc::new(Controller::new(CdpPage { cdp: cdp.clone(), executor: cx.background_executor().clone() }, !self.shown));
        for event in EVENTS {
            let weak = Rc::downgrade(&ctl);
            if let Err(e) = cdp.on(event, move |params| if let Some(ctl) = weak.upgrade() { ctl.on_event(event, &params) }) {
                eprintln!("[nav] evento {event} sem ouvinte: {e}");
            }
        }
        #[cfg(target_os = "windows")]
        { self.listen_relay(&cdp); *self.relay.borrow_mut() = Some(cdp.clone()); }
        self.listen_pick(&cdp, cx);
        let (activity, acted) = async_channel::unbounded();
        ctl.on_activity(move |a| { let _ = activity.try_send(a); });
        let (answer, answers) = async_channel::unbounded::<(bool, Option<String>, futures::channel::oneshot::Sender<Result<(), String>>)>();
        ctl.on_dialog_answer(move |accept, text, done| { let _ = answer.try_send((accept, text, done)); });
        cx.spawn(async move |this, cx| {
            while let Ok((accept, text, done)) = answers.recv().await {
                let Ok(call) = this.update(cx, |this, _| this.engine().map(|e| e.answer_dialog(accept, text.as_deref()))) else { break };
                let _ = done.send(match call { Some(call) => call.await, None => Err("o navegador fechou".into()) });
            }
        }).detach();
        let (error, errors) = async_channel::unbounded();
        ctl.on_page_error(move |e| { let _ = error.try_send(e); });
        (self.page_errors, self.error_count, self.errors_open) = (Default::default(), 0, false);
        cx.spawn(async move |this, cx| {
            while let Ok(e) = errors.recv().await {
                if this.update(cx, |this, cx| this.page_error(e, cx)).is_err() { break; }
            }
        }).detach();
        #[cfg(target_os = "linux")]
        {
            let (screen, screens) = async_channel::unbounded();
            ctl.on_layout(move |s| { let _ = screen.try_send(s); });
            self.emulated = None;
            cx.spawn(async move |this, cx| {
                while let Ok(s) = screens.recv().await {
                    if this.update(cx, |this, cx| { this.emulated = s.map(|(w, h)| (w as f32, h as f32)); this.refresh_zoom(cx); cx.notify(); }).is_err() { break; }
                }
            }).detach();
        }
        // Motor refeito começa sem pausa: a pausa era do motor que caiu.
        self.paused = None;
        self._activity = Some(cx.spawn(async move |this, cx| {
            while let Ok(a) = acted.recv().await {
                if this.update(cx, |this, cx| this.agent_acted(&a, cx)).is_err() { break; }
            }
        }));
        let setup = ctl.clone();
        cx.spawn(async move |_, _| setup.start().await).detach();
        self.controller = Some(ctl);
        self.cdp = Some(cdp);
        #[cfg(target_os = "linux")]
        self.scale.set(0.);
    }

    /// Entrega ao espectador atual os eventos que a tela remota acompanha.
    #[cfg(not(target_os = "macos"))]
    fn listen_relay(&self, cdp: &Rc<crate::browser::cdp::Cdp>) {
        for event in crate::browser::relay::WATCHED {
            let (viewer, page) = (Rc::downgrade(&self.viewer), Rc::downgrade(cdp));
            let listened = cdp.on(event, move |params| {
                let Some(viewer) = viewer.upgrade() else { return };
                let unsent = match viewer.borrow().as_ref() {
                    Some(viewer) => viewer.deliver(event, &params),
                    None => crate::browser::relay::frame_id(event, &params),
                };
                // Quadro sem entrega também é confirmado: sem o ack o Chromium para o screencast.
                if let (Some(id), Some(page)) = (unsent, page.upgrade()) {
                    drop(page.call("Page.screencastFrameAck", serde_json::json!({"sessionId": id})));
                }
            });
            if let Err(e) = listened { eprintln!("[nav] evento {event} sem ouvinte: {e}"); }
        }
    }

    /// Motor que falhou ou cujo Chromium caiu: some com tudo o que falava com ele, e o próximo `open` o refaz.
    #[cfg(not(target_os = "macos"))]
    fn drop_engine(&mut self) {
        #[cfg(target_os = "linux")]
        { self.popup = None; }
        self.end_viewer();
        self.relay.borrow_mut().take();
        self.pick.reset();
        self.controller = None;
        self.cdp = None;
        self.engine = None;
    }

    /// Espectador novo da tela remota; o anterior recebe o aviso de que outro aparelho assumiu.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn watch(&self, viewer: crate::browser::relay::Viewer, reply: futures::channel::oneshot::Sender<Result<(), String>>, cx: &mut Context<Self>) {
        if self.cdp.is_none() {
            let _ = reply.send(Err(format!("erro: o navegador da sessao {} ainda esta iniciando, tente de novo em instantes", self.key)));
            return;
        }
        // No Linux o espectador tem sessão própria no alvo, criada no primeiro; sem travar a interface.
        #[cfg(target_os = "linux")]
        if self.relay.borrow().is_none() {
            let Some(engine) = self.engine() else { let _ = reply.send(Err(format!("erro: a sessao {} nao tem navegador aberto", self.key))); return };
            let opening = engine.viewer_session();
            cx.spawn(async move |this, cx| {
                let opened = opening.await.map_err(|e| format!("erro: tela remota sem sessao no navegador: {e}"));
                let answer = this.update(cx, |panel, _| opened.map(|session| {
                    if panel.relay.borrow().is_none() {
                        panel.listen_relay(&session);
                        // Os quadros do espectador vão do pipe direto ao socket; o resto passa pela interface.
                        let (frames, ack) = (panel.frames.clone(), session.browser().acker(session.id()));
                        session.browser().sink(session.id(), Box::new(move |params| {
                            let events = frames.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
                            let unsent = match events {
                                Some(events) => crate::browser::relay::deliver(&events, "Page.screencastFrame", &params),
                                None => params["sessionId"].as_i64(),
                            };
                            if let Some(id) = unsent { ack(id.into()); }
                        }));
                        *panel.relay.borrow_mut() = Some(session);
                    }
                    panel.take_viewer(viewer);
                })).unwrap_or_else(|_| Err("erro: o navegador fechou".into()));
                let _ = reply.send(answer);
            }).detach();
            return;
        }
        let _ = cx;
        self.take_viewer(viewer);
        let _ = reply.send(Ok(()));
    }

    #[cfg(not(target_os = "macos"))]
    fn take_viewer(&self, viewer: crate::browser::relay::Viewer) {
        self.set_frames(Some(viewer.events.clone()));
        if let Some(old) = self.viewer.borrow_mut().replace(viewer) { old.detach(crate::browser::relay::REPLACED); }
    }

    #[cfg(not(target_os = "macos"))]
    fn set_frames(&self, events: Option<async_channel::Sender<crate::browser::relay::ViewerEvent>>) {
        #[cfg(target_os = "linux")]
        { *self.frames.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = events; }
        #[cfg(not(target_os = "linux"))]
        let _ = events;
    }

    /// O espectador `id` saiu: o screencast para, a menos que outro já o tenha substituído.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn unwatch(&self, id: u64) {
        let mut viewer = self.viewer.borrow_mut();
        if viewer.as_ref().is_none_or(|v| v.id != id) { return; }
        *viewer = None;
        self.set_frames(None);
        // No Linux a sessão do espectador cai inteira, e o screencast dela junto.
        #[cfg(target_os = "linux")]
        self.relay.borrow_mut().take();
        #[cfg(target_os = "windows")]
        if let Some(cdp) = &self.cdp { drop(cdp.call("Page.stopScreencast", serde_json::json!({}))); }
    }

    /// Navegador fechando: quem olha recebe o fim antes de o painel cair.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn end_viewer(&self) {
        self.set_frames(None);
        if let Some(viewer) = self.viewer.borrow_mut().take() { viewer.detach("target_closed"); }
    }

    /// CDP da tela remota: os comandos repassados do celular vão por ele.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn relay_cdp(&self) -> Option<Rc<crate::browser::cdp::Cdp>> { self.relay.borrow().clone() }

    /// Por que ainda não há controlador: `None` = motor nunca pedido, `Some(Ok)` = nascendo, `Some(Err)` = falhou.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn engine_status(&self) -> Option<Result<(), &str>> {
        match &self.engine {
            Some(Err(e)) => Some(Err(e)),
            Some(Ok(_)) => Some(Ok(())),
            None => self.starting.is_some().then_some(Ok(())),
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn controller(&self) -> Option<Rc<crate::browser::control::Controller<crate::browser::control::CdpPage>>> { self.controller.clone() }

    #[cfg(not(target_os = "macos"))]
    fn set_agent(&mut self, text: Option<String>) {
        if let Some(ctl) = &self.controller { ctl.set_strip_shown(text.is_some()); }
        self.agent = text;
    }

    /// A sessão parou de trabalhar: a faixa vai embora como depois do último verbo.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn set_session_working(&mut self, working: bool, cx: &mut Context<Self>) {
        self.session_working = working;
        if !working && self.agent_running == 0 && self.agent.is_some() { self.schedule_hide(cx); }
    }

    #[cfg(not(target_os = "macos"))]
    fn agent_acted(&mut self, a: &crate::browser::control::Activity, cx: &mut Context<Self>) {
        self.set_agent(Some(describe_activity(&self.key, a)));
        self.agent_seq += 1;
        // A atividade chega por canal: o verbo pode já ter terminado, e aí ninguém mais agendaria o sumiço.
        if self.agent_running == 0 { self.schedule_hide(cx); }
        cx.notify();
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn agent_started(&mut self) { self.agent_running += 1; }

    /// Verbo terminou: sem outro em andamento, a faixa some depois de `AGENT_LINGER`, se nenhuma ação nova chegar.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn agent_finished(&mut self, cx: &mut Context<Self>) {
        self.agent_running = self.agent_running.saturating_sub(1);
        #[cfg(target_os = "windows")]
        { self.agent_finished_at = Some(std::time::Instant::now()); }
        // O realce do alvo fica meio segundo depois da ação, se outra não vier antes: o controlador não espera por ele.
        if let Some(cdp) = self.cdp.clone() {
            let seq = self.agent_seq;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                if this.read_with(cx, |this, _| this.agent_seq == seq && this.agent_running == 0).unwrap_or(false) {
                    drop(cdp.call("Overlay.hideHighlight", serde_json::json!({})));
                }
            }).detach();
        }
        if self.agent_running > 0 || self.agent.is_none() { return; }
        self.schedule_hide(cx);
    }

    #[cfg(not(target_os = "macos"))]
    fn schedule_hide(&self, cx: &mut Context<Self>) {
        let seq = self.agent_seq;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AGENT_LINGER).await;
            let _ = this.update(cx, |this, cx| {
                if this.agent_seq != seq || this.agent_running > 0 { return; }
                // Sessão ainda trabalhando: a faixa fica, sem a ação que já acabou; some quando a sessão parar.
                let text = this.session_working.then(|| tr("browser_ctl_using").replace("{session}", this.key.rsplit("::").next().unwrap_or(&this.key)));
                if this.agent != text { this.set_agent(text); cx.notify(); }
            });
        }).detach();
    }

    #[cfg(not(target_os = "macos"))]
    fn pause(&mut self, why: Pause, cx: &mut Context<Self>) {
        self.paused = Some(why);
        if let Some(ctl) = &self.controller { ctl.set_paused(true); }
        cx.notify();
    }

    #[cfg(not(target_os = "macos"))]
    fn resume(&mut self, cx: &mut Context<Self>) {
        self.paused = None;
        if let Some(ctl) = &self.controller { ctl.set_paused(false); }
        cx.notify();
    }

    /// Clique ou tecla da pessoa na página com o agente agindo: ela assumiu, e o agente para na hora.
    #[cfg(not(target_os = "macos"))]
    fn user_input(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        if self.agent_running > 0 || self.agent_finished_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400)) { return; }
        if self.agent.is_some() && self.paused.is_none() { self.pause(Pause::TookOver, cx); }
    }

    /// Faixa entre a barra e a página: o agente agindo (Pausar, Assumir) ou parado pela pessoa (Retomar).
    #[cfg(not(target_os = "macos"))]
    fn render_agent(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let strip = |id: &'static str, color: Hsla| div().id(id).flex_shrink_0().flex().items_center().gap_2().px_3().py(px(5.))
            .bg(color.opacity(0.2)).border_b_1().border_color(color.opacity(0.45)).text_size(px(12.)).role(Role::Status)
            .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(color));
        let label = |text: String| div().flex_1().min_w_0().truncate().child(text);
        if let Some(why) = self.paused {
            let text = tr(if why == Pause::Asked { "browser_ctl_paused" } else { "browser_ctl_took_over" });
            return Some(strip("browser-agent-paused", theme::warning()).child(label(text))
                .child(Button::new("browser-agent-resume").outline().small().label(tr("browser_ctl_resume"))
                    .on_click(cx.listener(|this, _, _, cx| this.resume(cx))))
                .into_any_element());
        }
        let text = self.agent.clone()?;
        Some(strip("browser-agent", theme::accent()).child(label(text))
            .child(Button::new("browser-agent-pause").outline().small().label(tr("browser_ctl_pause"))
                .on_click(cx.listener(|this, _, _, cx| this.pause(Pause::Asked, cx))))
            .child(Button::new("browser-agent-take").outline().small().label(tr("browser_ctl_take"))
                .on_click(cx.listener(|this, _, window, cx| { this.pause(Pause::TookOver, cx); this.focus.focus(window, cx); })))
            .into_any_element())
    }

    /// Recarregar e "Tentar de novo": motor caído ou que não nasceu volta a nascer no mesmo endereço (`go` o refaz);
    /// com erro de carga, o endereço que falhou é carregado de novo, porque a página atual é a de erro.
    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // No Windows a página que caiu volta no mesmo WebView2, sem perder o histórico.
        #[cfg(target_os = "windows")]
        if let Some(engine) = self.engine() && !engine.alive() {
            engine.revive();
            if let Some(ctl) = &self.controller { ctl.reset_domains(); }
            return cx.notify();
        }
        // Fora do macOS o `go` já refaz o motor que falhou; aqui vale para todos.
        if matches!(&self.engine, Some(Err(_))) { self.engine = None; }
        let dead = !matches!(&self.engine, Some(Ok(e)) if e.alive());
        let url = self.page.url.clone().or_else(|| self.starting.as_ref().map(|(u, _)| u.clone()));
        match (dead || self.page.error.is_some(), url) {
            (true, Some(url)) => self.go(url, By::User, window, cx),
            _ => if let Some(engine) = self.engine() { engine.reload() },
        }
        cx.notify();
    }

    #[cfg(target_os = "linux")]
    fn copy_selection(&self, cx: &mut Context<Self>) {
        let Some(cdp) = self.cdp.clone() else { return };
        const SELECTION: &str = "(()=>{const a=document.activeElement;if(a&&a.type==='password')return '';if(a&&(a.tagName==='INPUT'||a.tagName==='TEXTAREA')&&a.selectionStart!=null)return a.value.slice(a.selectionStart,a.selectionEnd);return String(getSelection())})()";
        let call = cdp.call("Runtime.evaluate", serde_json::json!({"expression": SELECTION, "returnByValue": true}));
        cx.spawn(async move |_, cx| {
            match call.await {
                Ok(r) => if let Some(text) = r["result"]["value"].as_str().filter(|t| !t.is_empty()) {
                    let _ = cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned())));
                },
                Err(e) => eprintln!("[nav] copiar da pagina falhou: {e}"),
            }
        }).detach();
    }

    fn page_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        match error {
            Some(line) => {
                self.error_count += 1;
                self.page_errors.push_back(error_text(&line));
                if self.page_errors.len() > ERRORS_KEPT { self.page_errors.pop_front(); }
            }
            None if self.error_count == 0 => return,
            None => (self.page_errors, self.error_count, self.errors_open) = (Default::default(), 0, false),
        }
        cx.notify();
    }

    /// Print da página em PNG, que o `Hangar` anexa à mensagem da sessão.
    #[cfg(not(target_os = "macos"))]
    fn send_shot(&mut self, cx: &mut Context<Self>) {
        let Some(cdp) = self.cdp.clone().filter(|_| !self.shooting) else { return };
        self.shooting = true;
        cx.notify();
        let shot = cdp.call("Page.captureScreenshot", serde_json::json!({"format": "png"}));
        cx.spawn(async move |this, cx| {
            use base64::Engine as _;
            // Página com diálogo aberto ou travada não responde: sem prazo o botão ficava ocupado para sempre.
            let timer = cx.background_executor().timer(std::time::Duration::from_secs(15));
            let shot = match futures::future::select(std::pin::pin!(shot), std::pin::pin!(timer)).await {
                futures::future::Either::Left((r, _)) => r,
                futures::future::Either::Right(_) => Err(tr("browser_shot_timeout")),
            };
            let bytes = shot.and_then(|r| base64::engine::general_purpose::STANDARD
                .decode(r["data"].as_str().unwrap_or("")).map_err(|e| e.to_string())).and_then(|b| if b.is_empty() { Err("vazio".into()) } else { Ok(b) });
            let _ = this.update(cx, |this, cx| {
                this.shooting = false;
                match bytes {
                    Ok(bytes) => cx.emit(PanelEvent::Shot { key: this.key.clone(), bytes }),
                    Err(e) => this.local_notice = Some(tr("browser_shot_failed").replace("{e}", &e)),
                }
                cx.notify();
            });
        }).detach();
    }

    /// O diálogo da página no painel: a pessoa responde aqui, ou o agente pelo verbo `dialog`.
    fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // No Windows o diálogo é o nativo do WebView2, na frente da página: a pessoa responde nele.
        if cfg!(target_os = "windows") { return None; }
        let d = self.page.dialog.as_ref()?;
        let answer = |accept: bool| cx.listener(move |this: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
            let text = (accept && this.page.dialog.as_ref().is_some_and(|d| d.kind == "prompt")).then(|| this.dialog_input.read(cx).value().to_string());
            let Some(call) = this.engine().map(|e| e.answer_dialog(accept, text.as_deref())) else { return };
            cx.spawn(async move |this, cx| if let Err(e) = call.await {
                let _ = this.update(cx, |this, cx| { this.local_notice = Some(tr("browser_dialog_failed").replace("{e}", &e)); cx.notify(); });
            }).detach();
        });
        let title = tr(if d.kind == "alert" { "browser_dialog_says" } else { "browser_dialog_asks" });
        Some(div().id("browser-dialog").flex_shrink_0().flex().flex_col().gap_2().px_3().py(px(8.)).role(Role::AlertDialog)
            .bg(theme::warning().opacity(0.12)).border_b_1().border_color(theme::warning().opacity(0.4))
            .child(div().text_size(px(11.)).text_color(theme::muted()).child(title))
            .child(div().text_size(px(13.)).whitespace_normal().child(d.message.clone()))
            .when(d.kind == "prompt", |el| el.child(Input::new(&self.dialog_input).id("browser-dialog-input").small()))
            .child(div().flex().justify_end().gap_2()
                .when(d.kind != "alert", |el| el.child(Button::new("browser-dialog-cancel").outline().small().label(tr("browser_dialog_cancel")).on_click(answer(false))))
                .child(Button::new("browser-dialog-ok").primary().small().label(tr("browser_dialog_ok")).on_click(answer(true))))
            .into_any_element())
    }

    fn render_errors(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.errors_open || self.error_count == 0 { return None; }
        let lines = self.page_errors.iter().rev().cloned().collect::<Vec<_>>();
        Some(div().id("browser-errors").flex_shrink_0().flex().flex_col().border_b_1().border_color(theme::border())
            .child(div().flex().items_center().gap_2().px_3().py(px(4.))
                .child(div().flex_1().text_size(px(11.)).text_color(theme::muted()).child(tr("browser_errors_title")))
                .child(Button::new("browser-errors-clear").ghost().xsmall().label(tr("browser_errors_clear"))
                    .on_click(cx.listener(|this, _, _, cx| { (this.page_errors, this.error_count, this.errors_open) = (Default::default(), 0, false); cx.notify(); })))
                .child(chrome::icon_button("browser-errors-close", IconName::Close, tr("close"), cx)
                    .on_click(cx.listener(|this, _, _, cx| { this.errors_open = false; cx.notify(); }))))
            .child(div().id("browser-errors-list").max_h(px(160.)).overflow_y_scroll().px_3().pb(px(6.)).flex().flex_col().gap(px(3.))
                .children(lines.into_iter().map(|line| div().text_size(px(11.)).font_family("monospace").text_color(theme::danger())
                    .whitespace_normal().child(line))))
            .into_any_element())
    }

    fn copy_address(&mut self, url: Option<String>, cx: &mut Context<Self>) {
        let Some(url) = url else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        self.copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| { this.copied = false; cx.notify(); });
        }).detach();
    }

    fn restore_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.page.url.clone().unwrap_or_default();
        self.address.update(cx, |input, cx| input.set_value(url, window, cx));
        self.invalid = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Site da página aberta (http/https), sem o `www.`: o que o diálogo oferece.
    #[cfg(not(target_os = "macos"))]
    fn page_site(&self) -> Option<String> {
        let url = url::Url::parse(self.page.url.as_deref()?).ok()?;
        if !matches!(url.scheme(), "http" | "https") { return None; }
        let host = url.host_str()?;
        Some(host.strip_prefix("www.").unwrap_or(host).to_owned())
    }

    /// Diálogo "Usar meu login do Chrome em <site>": o perfil do navegador embutido é um só para todas as sessões e
    /// agentes, então a pessoa escolhe o alcance antes de qualquer cookie sair do Chrome.
    #[cfg(not(target_os = "macos"))]
    fn ask_chrome_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let site = self.page_site();
        let all = Rc::new(Cell::new(site.is_none()));
        let (title, help) = match &site {
            Some(site) => (tr("browser_login_title_site").replace("{site}", site), tr("browser_login_help_site").replace("{site}", site)),
            None => (tr("browser_login_title_all"), tr("browser_login_help_all")),
        };
        let check = site.is_some().then(|| (tr("browser_login_all_sites"), all.clone()));
        let panel = cx.entity().downgrade();
        chrome::confirm_alert_with(window, cx, title, help, check, tr("browser_login_ok"), ButtonVariant::Primary, move |window, cx| {
            let site = if all.get() { None } else { site.clone() };
            panel.update(cx, |this, cx| {
                this.import_site = site;
                this.login_hint = None;
                this.import_chrome_login(window, cx);
            }).is_ok()
        });
    }

    /// Página https com campo de senha visível: sugere trazer o login, uma vez por site. Só a linha; importar continua
    /// dependendo do diálogo.
    #[cfg(not(target_os = "macos"))]
    fn maybe_suggest_login(&mut self, cx: &mut Context<Self>) {
        if self.page.loading { return; }
        let Some(site) = self.page_site() else { return };
        if self.login_hint.as_ref().is_some_and(|hint| *hint != site) { self.login_hint = None; }
        if self.hint_checked.as_deref() == Some(site.as_str()) || !self.page.url.as_deref().is_some_and(|u| u.starts_with("https:")) {
            return;
        }
        let Some(cdp) = self.cdp.clone() else { return };
        self.hint_checked = Some(site.clone());
        let js = "(() => { const vis = (el) => el.offsetParent !== null && !el.disabled;             const walk = (root) => [...root.querySelectorAll('*')].some((el) => (el.tagName === 'INPUT' && el.type === 'password' && vis(el)) || (el.shadowRoot && walk(el.shadowRoot)));             return walk(document); })()";
        let params = serde_json::json!({"expression": js, "returnByValue": true});
        cx.spawn(async move |this, cx| {
            // SPA desenha o formulário depois do carregamento.
            for wait in [0u64, 1500, 4000] {
                if wait > 0 { cx.background_executor().timer(std::time::Duration::from_millis(wait)).await; }
                if !this.read_with(cx, |this, _| this.page_site().as_deref() == Some(site.as_str())).unwrap_or(false) { return; }
                if cdp.call("Runtime.evaluate", params.clone()).await.is_ok_and(|v| v["result"]["value"].as_bool() == Some(true)) {
                    let _ = this.update(cx, |this, cx| { this.login_hint = Some(site.clone()); cx.notify(); });
                    return;
                }
            }
        }).detach();
    }

    /// Pedido do agente: só a linha desenhada pelo app aprova. Clique do agente cai na página (pela depuração) e o
    /// celular só vê a página, então nenhum dos dois alcança esta linha.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn request_login(&mut self, site: Option<String>, reply: futures::channel::oneshot::Sender<crate::browser::control::Reply>,
        cx: &mut Context<Self>) {
        let site = site.or_else(|| self.page_site());
        if let Some((_, old)) = self.login_request.replace((site, reply)) {
            let _ = old.send(crate::browser::control::Reply::Text("erro: outro pedido de login substituiu este".into()));
        }
        cx.notify();
    }

    /// Resposta ao agente: a contagem ou o motivo, nunca valor de cookie.
    #[cfg(not(target_os = "macos"))]
    /// `Ok((trazidos, de fora))`.
    fn finish_login(&mut self, result: Result<(usize, usize), String>) {
        let Some((site, reply)) = self.login_reply.take() else { return };
        let site = site.unwrap_or_else(|| "todos os sites".into());
        let skipped = |n: usize| if n > 0 { format!(" ({n} ficaram de fora)") } else { String::new() };
        let text = match result {
            Ok((0, out)) => format!("erro: login de {site} nao veio: nenhum cookie do site no navegador{}", skipped(out)),
            Ok((n, out)) => format!("ok: {n} cookies para {site}{}", skipped(out)),
            Err(e) => format!("erro: login de {site} nao veio: {e}"),
        };
        let _ = reply.send(crate::browser::control::Reply::Text(text));
    }

    #[cfg(not(target_os = "macos"))]
    fn answer_login_request(&mut self, allow: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((site, reply)) = self.login_request.take() else { return };
        // O agente desistiu (passou o prazo do verbo): aprovar agora traria cookies para ninguém.
        if reply.is_canceled() {
            self.cookies_status = Some(tr("browser_login_agent_expired"));
            cx.notify();
            return;
        }
        if !allow {
            let _ = reply.send(crate::browser::control::Reply::Text("erro: login recusado pelo usuario".into()));
            cx.notify();
            return;
        }
        if let Some((_, old)) = self.login_reply.replace((site.clone(), reply)) {
            let _ = old.send(crate::browser::control::Reply::Text("erro: outro pedido de login foi aprovado no lugar".into()));
        }
        self.import_site = site;
        self.login_hint = None;
        self.import_chrome_login(window, cx);
    }

    /// Traz o login do navegador real do usuário para o navegador embutido, só no clique: no Linux pelo perfil em
    /// disco; senão pela depuração, onde cada conexão faz o Chrome perguntar "Permitir a depuração remota?".
    #[cfg(not(target_os = "macos"))]
    fn import_chrome_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::browser::chrome_import;
        let Some(cdp) = self.cdp.clone() else {
            // Sem página aberta o motor nem existe: o botão o faz nascer e a importação segue quando ele estiver pronto.
            self.import_pending = true;
            self.cookies_status = Some(tr("browser_cookies_starting"));
            // `go` refaz o motor que falhou ou caiu.
            if self.starting.is_none() { self.go("about:blank".into(), By::User, window, cx); }
            cx.notify();
            return;
        };
        // No Linux o perfil em disco vem primeiro: sem diálogo no navegador da pessoa, e com ele fechado.
        #[cfg(target_os = "linux")]
        {
            let mut sources = chrome_import::disk::sources();
            if sources.len() == 1 {
                self.read_disk_cookies(sources.remove(0), cx);
                return;
            }
            if !sources.is_empty() {
                self.cookie_sources = sources;
                self.cookies_status = Some(tr("browser_cookies_pick"));
                cx.notify();
                return;
            }
        }
        self.cookies_status = Some(tr("browser_cookies_fetching"));
        cx.notify();
        // Plano B pra Chromium/Brave antigos: porta fixa por variável de ambiente.
        let port = std::env::var("HANGAR_CHROME_CDP_PORT").ok().and_then(|v| v.trim().parse().ok());
        self.importing = true;
        let fetch = cx.background_executor().spawn(async move {
            chrome_import::fetch_cookies(port).map_err(|e| match e {
                chrome_import::ImportError::ChromeClosed => None,
                e => Some(tr(e.status_key()).replace("{e}", e.detail())),
            })
        });
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| this.apply_cookies(result, cdp, cx));
        }).detach();
    }

    /// Lê os cookies do perfil escolhido em thread de fundo (cópia do banco e `sqlite3`).
    #[cfg(target_os = "linux")]
    fn read_disk_cookies(&mut self, source: crate::browser::chrome_import::disk::Source, cx: &mut Context<Self>) {
        self.cookie_sources.clear();
        let Some(cdp) = self.cdp.clone() else {
            let reason = tr("browser_cookies_starting");
            self.finish_login(Err(reason.clone()));
            self.cookies_status = Some(reason);
            cx.notify();
            return;
        };
        self.cookies_status = Some(tr("browser_cookies_reading").replace("{source}", &source.label));
        cx.notify();
        self.importing = true;
        let fetch = cx.background_executor().spawn(async move {
            crate::browser::chrome_import::disk::read(&source)
                .map_err(|e| Some(tr(e.status_key()).replace("{e}", e.detail())))
        });
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| this.apply_cookies(result, cdp, cx));
        }).detach();
    }

    /// `Err(None)` é o Chrome sem depuração ligada; `Err(Some)`, a frase do erro.
    #[cfg(not(target_os = "macos"))]
    fn apply_cookies(&mut self, result: Result<Vec<serde_json::Value>, Option<String>>,
        cdp: Rc<crate::browser::cdp::Cdp>, cx: &mut Context<Self>) {
        self.importing = false;
        let cookies = match result {
            Ok(cookies) => cookies,
            Err(None) => {
                // Sem depuração ligada, o clique já abre a página do toggle no Chrome — um passo, não dois.
                self.activate_chrome(cx);
                self.finish_login(Err("a depuracao remota do Chrome esta desligada; a instrucao para liga-la esta no painel".into()));
                return;
            }
            Err(Some(text)) => {
                self.finish_login(Err(text.clone()));
                self.cookies_status = Some(text);
                cx.notify();
                return;
            }
        };
        let mut cookies = match &self.import_site {
            Some(site) => crate::browser::chrome_import::for_site(cookies, site),
            None => cookies,
        };
        // Cookie do disco que a chave do chaveiro não abriu: conta por site, para o login que faltou não virar "nenhum cookie".
        let before = cookies.len();
        cookies.retain(|c| c.get("__locked").is_none());
        let locked = before - cookies.len();
        if cookies.is_empty() && locked > 0 {
            let reason = tr("browser_cookies_keyring");
            self.finish_login(Err(reason.clone()));
            self.cookies_status = Some(reason);
            cx.notify();
            return;
        }
        // 0 cookies não é sucesso: a pessoa vê "nenhum cookie", não "0 trazidos" parecendo que deu certo.
        if cookies.is_empty() {
            self.finish_login(Ok((0, locked)));
            self.cookies_status = Some(match &self.import_site {
                Some(site) => tr("browser_login_none_site").replace("{site}", site),
                None => tr("browser_cookies_none"),
            });
            cx.notify();
            return;
        }
        let site = self.import_site.clone();
        cx.spawn(async move |this, cx| {
            // O Chromium recusa o lote inteiro por um cookie inválido: aí vai um por um e só os recusados ficam de fora.
            let total = cookies.len();
            let reply = match cdp.call("Storage.setCookies", serde_json::json!({"cookies": cookies})).await {
                Ok(_) => Ok(total),
                Err(e) => {
                    let mut written = 0;
                    for cookie in &cookies {
                        if cdp.call("Storage.setCookies", serde_json::json!({"cookies": [cookie]})).await.is_ok() { written += 1; }
                    }
                    if written > 0 { Ok(written) } else { Err(e) }
                }
            };
            let _ = this.update(cx, |this, cx| {
                match reply {
                    Ok(count) => {
                        this.finish_login(Ok((count, total - count + locked)));
                        let mut done = match &site {
                            Some(site) => tr("browser_login_ok_site").replace("{site}", site),
                            None => tr("browser_cookies_ok"),
                        }.replace("{n}", &count.to_string());
                        let skipped = total - count + locked;
                        if skipped > 0 { done = format!("{done} {}", tr("browser_cookies_partial").replace("{n}", &skipped.to_string())); }
                        this.cookies_status = Some(done.clone());
                        if let Some(engine) = this.engine() { engine.reload(); }
                        // Sucesso é aviso, não estado: some sozinho. Erro fica até a próxima tentativa.
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(std::time::Duration::from_secs(6)).await;
                            let _ = this.update(cx, |this, cx| {
                                if this.cookies_status.as_deref() == Some(done.as_str()) { this.cookies_status = None; cx.notify(); }
                            });
                        }).detach();
                    }
                    Err(e) => {
                        this.finish_login(Err(e.clone()));
                        this.cookies_status = Some(tr("browser_cookies_error").replace("{e}", &e));
                    }
                }
                cx.notify();
            });
        }).detach();
    }

    /// Abre a página da depuração remota no navegador do usuário e copia o endereço: o navegador em execução recusa
    /// `chrome://` vindo da linha de comando, então a instrução manda colar na barra.
    #[cfg(not(target_os = "macos"))]
    fn activate_chrome(&mut self, cx: &mut Context<Self>) {
        self.cookies_status = Some(match crate::browser::chrome_import::open_remote_debug_page() {
            Some(browser) => {
                cx.write_to_clipboard(ClipboardItem::new_string(browser.page.to_owned()));
                format!("{} {}", tr("browser_cookies_chrome_off"), tr("browser_cookies_activate")).replace("{browser}", browser.name)
            }
            None => tr("browser_cookies_no_chrome"),
        });
        cx.notify();
    }

    /// Clique na GPUI com a página nativa segurando o foco do sistema: sem isto o teclado (Esc, atalhos) seguiria
    /// indo para a página.
    pub(super) fn release_focus(&self) {
        if let Some(engine) = self.engine() { engine.release_focus() }
    }

    pub(super) fn focus_address(&self, window: &mut Window, cx: &mut App) {
        self.address.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Diz se mudou. Escondida, a página não se posiciona no desenho e a janela nativa some.
    /// O controlador põe um viewport fixo na página escondida, para ela seguir desenhando para o hangar-preview.
    fn set_shown(&mut self, shown: bool, covered: bool, cx: &mut Context<Self>) -> bool {
        if self.shown == shown { return false; }
        self.shown = shown;
        #[cfg(not(target_os = "macos"))]
        if !shown { self.stop_pick(cx); }
        #[cfg(target_os = "windows")]
        if shown && let Some(old) = self.still.take() { self.released.push(old); }
        #[cfg(target_os = "windows")]
        if !shown && covered && let Some(cdp) = self.cdp.clone() {
            // Coberta, a página só sai da frente com o último quadro pronto: sem ele o painel ficaria vazio atrás do
            // diálogo. O teto curto evita que um quadro que não vem segure a página por cima do diálogo.
            let shot = cdp.call("Page.captureScreenshot", serde_json::json!({"format": "jpeg", "quality": 85}));
            self.cover_gen += 1;
            let generation = self.cover_gen;
            cx.spawn(async move |this, cx| {
                let decode = |r: Result<serde_json::Value, String>| -> Option<Arc<Image>> {
                    use base64::Engine as _;
                    r.ok()?["data"].as_str().and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
                        .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Jpeg, bytes)))
                };
                let put = |this: &mut Self, image: Option<Arc<Image>>| {
                    let Some(image) = image else { return };
                    if this.cover_gen != generation { return this.released.push(image); }
                    if let Some(old) = this.still.replace(image) { this.released.push(old); }
                };
                let leave = |this: &mut Self, cx: &mut Context<Self>| {
                    if let Some(engine) = this.engine() { engine.hide(); }
                    // Só agora o viewport fixo da página escondida: antes, a captura podia sair já nesse tamanho.
                    if let Some(ctl) = this.controller.clone() { cx.spawn(async move |_, _| ctl.set_hidden(true).await).detach(); }
                    cx.notify();
                };
                let timer = cx.background_executor().timer(std::time::Duration::from_millis(300));
                match futures::future::select(std::pin::pin!(shot), std::pin::pin!(timer)).await {
                    futures::future::Either::Left((r, _)) => {
                        let image = decode(r);
                        let _ = this.update(cx, |this, cx| if !this.shown { put(this, image); leave(this, cx); });
                    }
                    // Quadro lento: a página sai da frente agora, e o quadro entra quando chegar.
                    futures::future::Either::Right((_, shot)) => {
                        if this.update(cx, |this, cx| { if !this.shown { leave(this, cx); } !this.shown }).unwrap_or(false) {
                            let image = decode(shot.await);
                            let _ = this.update(cx, |this, cx| if !this.shown { put(this, image); cx.notify(); });
                        }
                    }
                }
            }).detach();
        } else if !shown && let Some(engine) = self.engine() { engine.hide(); }
        #[cfg(not(target_os = "windows"))]
        let _ = covered;
        #[cfg(not(target_os = "windows"))]
        if !shown && let Some(engine) = self.engine() { engine.hide(); }
        #[cfg(target_os = "windows")]
        let capturing = !shown && covered && self.cdp.is_some();
        #[cfg(not(target_os = "windows"))]
        let capturing = false;
        #[cfg(not(target_os = "macos"))]
        if !capturing && let Some(ctl) = self.controller.clone() { cx.spawn(async move |_, _| ctl.set_hidden(!shown).await).detach(); }
        #[cfg(target_os = "windows")]
        if !shown { self.schedule_sleep(cx); }
        #[cfg(target_os = "macos")]
        let _ = cx;
        true
    }

    fn agent_paused(&self) -> bool {
        #[cfg(not(target_os = "macos"))]
        { self.paused.is_some() }
        #[cfg(target_os = "macos")]
        false
    }

    fn agent_acting(&self) -> bool {
        #[cfg(not(target_os = "macos"))]
        { self.agent.is_some() && self.paused.is_none() }
        #[cfg(target_os = "macos")]
        false
    }

    #[cfg(target_os = "linux")]
    fn probe_cursor(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) { let p = self.page_point(at); self.probe_again(p, cx); }

    /// `p` já em px da página.
    #[cfg(target_os = "linux")]
    fn probe_again(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        self.cursor_probe.1 = Some(p);
        if self.cursor_probe.0 { return; }
        let Some(cdp) = self.cdp.clone() else { return };
        self.cursor_probe.0 = true;
        let (x, y) = (f32::from(p.x), f32::from(p.y));
        // O cursor e a área visível na mesma pergunta: zoom de SPA ou de pinça chega aqui sem esperar uma carga.
        let js = format!("(()=>{{const k=(()=>{{const e=document.elementFromPoint({x},{y});if(!e)return 'default';const c=getComputedStyle(e).cursor;if(c!=='auto')return c;const t=e.tagName;if(e.isContentEditable||t==='TEXTAREA'||(t==='INPUT'&&!/^(button|submit|reset|checkbox|radio|file|image|range|color)$/i.test(e.type)))return 'text';return 'default'}})();return {{cursor:k,view:{VIEWPORT}}}}})()");
        let call = cdp.call("Runtime.evaluate", serde_json::json!({"expression": js, "returnByValue": true}));
        cx.spawn(async move |this, cx| {
            let value = call.await.ok().map(|r| r["result"]["value"].clone()).unwrap_or_default();
            let zoom = viewport_of(&value["view"]);
            let style = match value["cursor"].as_str() {
                Some("pointer") => CursorStyle::PointingHand,
                Some("text" | "vertical-text") => CursorStyle::IBeam,
                Some("crosshair") => CursorStyle::Crosshair,
                Some("grab") => CursorStyle::OpenHand,
                Some("grabbing" | "move") => CursorStyle::ClosedHand,
                Some("ew-resize" | "col-resize") => CursorStyle::ResizeLeftRight,
                Some("ns-resize" | "row-resize") => CursorStyle::ResizeUpDown,
                Some("not-allowed" | "no-drop") => CursorStyle::OperationNotAllowed,
                _ => CursorStyle::Arrow,
            };
            let _ = this.update(cx, |this, cx| {
                this.cursor_probe.0 = false;
                if let Some(z) = zoom { this.page_zoom = z; }
                if this.cursor != style { this.cursor = style; cx.notify(); }
                // O mouse andou durante a pergunta: pergunta de novo pelo ponto novo.
                if this.cursor_probe.1 != Some(p) && let Some(at) = this.cursor_probe.1 { this.probe_again(at, cx); }
            });
        }).detach();
    }

    /// Alguém vai usar a página (verbo do agente, espectador): acorda a WebView estacionada e recomeça a contagem.
    /// Devolve se ela estava dormindo.
    #[cfg(target_os = "windows")]
    pub(super) fn touch(&self, cx: &mut Context<Self>) -> bool {
        let woke = self.engine().is_some_and(|engine| engine.wake());
        self.schedule_sleep(cx);
        woke
    }

    /// Escondida, sem verbo do agente e sem espectador por `SLEEP_AFTER`, a WebView para de compor.
    #[cfg(target_os = "windows")]
    fn schedule_sleep(&self, cx: &mut Context<Self>) {
        if self.shown { return; }
        let turn = self.idle.get() + 1;
        self.idle.set(turn);
        let timer = cx.background_executor().timer(SLEEP_AFTER);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |panel, cx| {
                if panel.idle.get() != turn || panel.shown { return; }
                // Um `wait` longo segura a vez do controlador: a página está em uso, tenta de novo depois.
                if panel.controller.as_ref().is_some_and(|c| c.busy()) || panel.viewer.borrow().is_some() { return panel.schedule_sleep(cx); }
                if let Some(engine) = panel.engine() { engine.sleep(); }
            });
        }).detach();
    }

    /// Ponto do painel em px da página.
    fn page_point(&self, at: Point<Pixels>) -> Point<Pixels> {
        #[cfg(target_os = "linux")]
        let (zoom, ox, oy) = if self.emulated.is_some() && self.page_zoom.0 > 0. { self.page_zoom } else { (1., 0., 0.) };
        #[cfg(not(target_os = "linux"))]
        let (zoom, ox, oy) = (1., 0., 0.);
        let (p, s) = (at - self.origin.get(), self.page_scale.get() / zoom);
        point(px(ox + f32::from(p.x) * s), px(oy + f32::from(p.y) * s))
    }

    /// Relido ao trocar de tela emulada e a cada página carregada, um quadro depois: o tamanho emulado chega à página
    /// depois da resposta do CDP. Sem emulação o zoom não entra na conta.
    #[cfg(target_os = "linux")]
    fn refresh_zoom(&mut self, cx: &mut Context<Self>) {
        if self.emulated.is_none() { self.page_zoom = (1., 0., 0.); return; }
        let Some(cdp) = self.cdp.clone() else { return };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
            let call = cdp.call("Runtime.evaluate", serde_json::json!({"expression": format!("({VIEWPORT})"), "returnByValue": true}));
            let zoom = call.await.ok().and_then(|r| viewport_of(&r["result"]["value"]));
            let _ = this.update(cx, |this, _| if let Some(z) = zoom { this.page_zoom = z });
        }).detach();
    }

    fn pointer(&self, kind: Pointer, button: MouseButton, at: Point<Pixels>, clicks: usize, modifiers: &Modifiers) {
        // Escolhendo elemento, só o esquerdo vai ao modo de inspeção, que o segura antes da página; o direito e o do
        // meio abririam menu e aba nela.
        #[cfg(not(target_os = "macos"))]
        if self.pick.on && kind != Pointer::Move && button != MouseButton::Left { return; }
        if let Some(engine) = self.engine() { engine.mouse(kind, button, self.page_point(at), clicks, modifiers); }
    }

    fn render_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = div().id("browser-page").relative().flex_1().min_h_0().overflow_hidden()
            .track_focus(&self.focus).key_context("BrowserPage");
        let note = |text: String, color: Hsla| div().size_full().flex().items_center().justify_center().p_4()
            .text_size(px(12.)).text_color(color).text_center().whitespace_normal().child(text);
        let failure = |text: String, cx: &mut Context<Self>| div().size_full().flex().flex_col().items_center().justify_center().gap_3().p_4()
            .child(div().text_size(px(12.)).text_color(theme::danger()).text_center().whitespace_normal().child(text))
            .child(Button::new("browser-retry").outline().small().icon(IconName::RotateCw).label(tr("browser_retry"))
                .on_click(cx.listener(|this, _, window, cx| this.retry(window, cx))));
        let engine = match &self.engine {
            None if self.starting.is_some() => return page.role(Role::Status).child(note(tr("loading"), theme::muted())).into_any_element(),
            None => return page.child(note(tr("browser_empty"), theme::faint())).into_any_element(),
            Some(Err(error)) => return page.role(Role::Alert)
                .child(failure(tr("browser_failed").replace("{error}", error), cx)).into_any_element(),
            Some(Ok(engine)) => engine.clone(),
        };
        #[cfg(target_os = "linux")]
        let engine = self.popup.as_ref().map_or(engine, |p| p.engine.clone());
        // No Linux o erro da carga toma o lugar da página de erro do Chromium (em inglês e sem o botão). No Windows a
        // página é janela própria e ficaria por cima: lá o erro só vira página quando não há página nenhuma.
        if let Some(error) = &self.page.error && (cfg!(target_os = "linux") || self.page.url.is_none()) {
            return page.role(Role::Alert).child(failure(error.clone(), cx)).into_any_element();
        }
        let (origin, page_scale) = (self.origin.clone(), self.page_scale.clone());
        #[cfg(target_os = "linux")]
        let emulated = self.emulated;
        #[cfg(not(target_os = "linux"))]
        let emulated: Option<(f32, f32)> = None;
        #[cfg(target_os = "linux")]
        let (ctl, scale) = (self.controller.clone(), self.scale.clone());
        #[cfg(target_os = "linux")]
        let (hover, zoom) = (self.pick.hover.filter(|_| self.pick.on), if emulated.is_some() && self.page_zoom.0 > 0. { self.page_zoom } else { (1., 0., 0.) });
        let label = if self.page.title.trim().is_empty() { tr("browser") } else { self.page.title.clone() };
        #[cfg(target_os = "linux")]
        let page = page.cursor(self.cursor);
        #[cfg(target_os = "windows")]
        let page = page.when_some(self.still.clone().filter(|_| !self.shown), |el, still| el.child(img(still).absolute().inset_0().size_full().object_fit(ObjectFit::Contain)));
        page.bg(theme::background()).aria_label(label)
            .when(self.shown, |el| el.child(canvas(|_, _, _| {}, move |bounds, _, window, _cx| {
                // Tela emulada: encaixada no painel, centrada no topo, e o ponteiro convertido na mesma escala.
                let placed = match emulated {
                    Some((w, h)) if w > 0. && h > 0. => {
                        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                        let s = (bw / w).min(bh / h);
                        page_scale.set(1. / s);
                        Bounds { origin: bounds.origin + point(px((bw - w * s) / 2.), px(0.)), size: size(px(w * s), px(h * s)) }
                    }
                    _ => { page_scale.set(1.); bounds }
                };
                origin.set(placed.origin);
                engine.place(placed, window);
                // Realce do modo de escolha por cima do quadro: o do Chromium não sai no `chrome-headless-shell`.
                #[cfg(target_os = "linux")]
                if let Some(rect) = hover {
                    let area = pick::hover_bounds(rect, placed, page_scale.get(), zoom);
                    window.paint_quad(quad(area, px(0.), theme::accent().opacity(0.18), px(2.), theme::accent(), BorderStyle::Solid));
                }
                // A página desenha na escala desta janela (o Chromium nasce em 1, às vezes com ela escondida).
                #[cfg(target_os = "linux")]
                if scale.replace(window.scale_factor()) != window.scale_factor() && let Some(ctl) = ctl.clone() {
                    let wanted = window.scale_factor();
                    window.spawn(_cx, async move |_| ctl.set_scale(wanted).await).detach();
                }
            }).absolute().inset_0()))
            // Só no Linux a GPUI desenha por cima da página; no Windows ela é janela própria e cobriria o contorno.
            .when(cfg!(target_os = "linux") && self.agent_acting(), |el| el.child(div().absolute().inset_0().border_2().border_color(theme::accent())))
            // Os três botões: o direito abre o menu da página, o do meio abre link em outra aba (que vira esta).
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                if !matches!(event.button, MouseButton::Left | MouseButton::Right | MouseButton::Middle) { return; }
                #[cfg(not(target_os = "macos"))]
                this.user_input(cx);
                this.focus.focus(window, cx);
                #[cfg(target_os = "linux")]
                if event.button == MouseButton::Left { this.pick.shift = event.modifiers.shift; }
                this.pointer(Pointer::Down, event.button, event.position, event.click_count, &event.modifiers);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _, _| this.pointer(Pointer::Up, event.button, event.position, event.click_count, &event.modifiers)))
            .on_mouse_up(MouseButton::Right, cx.listener(|this, event: &MouseUpEvent, _, _| this.pointer(Pointer::Up, event.button, event.position, event.click_count, &event.modifiers)))
            .on_mouse_up(MouseButton::Middle, cx.listener(|this, event: &MouseUpEvent, _, _| this.pointer(Pointer::Up, event.button, event.position, event.click_count, &event.modifiers)))
            // Soltar fora da página ainda termina o arrasto começado nela.
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _, _| this.pointer(Pointer::Up, event.button, event.position, event.click_count, &event.modifiers)))
            .on_mouse_up_out(MouseButton::Right, cx.listener(|this, event: &MouseUpEvent, _, _| this.pointer(Pointer::Up, event.button, event.position, event.click_count, &event.modifiers)))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(|this, event: &MouseUpEvent, _, _| this.pointer(Pointer::Up, event.button, event.position, event.click_count, &event.modifiers)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _cx| {
                this.pointer(Pointer::Move, MouseButton::Left, event.position, 0, &event.modifiers);
                #[cfg(target_os = "linux")]
                this.probe_cursor(event.position, _cx);
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, _| {
                // A caixa medida ficou para trás; a próxima medida vem com o mouse.
                #[cfg(target_os = "linux")]
                { this.pick.hover = None; }
                let Some(engine) = this.engine() else { return };
                // A GPUI manda 3 linhas por passo da roda; 17,78 px por linha dá os 53 px do Chrome no Linux.
                let delta = event.delta.pixel_delta(px(17.78));
                // A GPUI manda o sinal oposto ao do motor.
                engine.scroll(this.page_point(event.position), point(-delta.x, -delta.y), &event.modifiers);
            }))
            // A tecla é da página: os atalhos do app (Esc, "/") não comem o que se digita nela.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                #[cfg(not(target_os = "macos"))]
                this.user_input(cx);
                if let Some(engine) = this.engine() {
                    let k = &event.keystroke;
                    // O Chromium sem janela tem área de transferência própria: colar lê a do sistema e digita o texto.
                    let paste = (k.modifiers.control && !k.modifiers.alt && k.key == "v") || (k.modifiers.shift && k.key == "insert");
                    // E copiar escreve na do sistema. A leitura da seleção sai antes da tecla, que é entregue depois
                    // dela, na ordem: o Ctrl+X ainda não cortou o texto.
                    #[cfg(target_os = "linux")]
                    if (k.modifiers.control && !k.modifiers.alt && !k.modifiers.shift && matches!(k.key.as_str(), "c" | "x"))
                        || (k.modifiers.control && k.key == "insert") {
                        this.copy_selection(cx);
                    }
                    match paste.then(|| cx.read_from_clipboard().and_then(|item| item.text())).flatten() {
                        Some(text) => engine.insert_text(&text),
                        None => engine.key(true, k),
                    }
                }
                cx.stop_propagation();
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                if let Some(engine) = this.engine() { engine.key(false, &event.keystroke); }
                cx.stop_propagation();
            }))
            .into_any_element()
    }
}

impl Render for BrowserPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.engine().is_some();
        let back = chrome::icon_button("browser-back", IconName::ArrowLeft, tr("browser_back"), cx)
            .disabled(!ready || !self.page.can_back)
            .on_click(cx.listener(|this, _, _, _| if let Some(engine) = this.engine() { engine.back() }));
        let forward = chrome::icon_button("browser-forward", IconName::ArrowRight, tr("browser_forward"), cx)
            .disabled(!ready || !self.page.can_forward)
            .on_click(cx.listener(|this, _, _, _| if let Some(engine) = this.engine() { engine.forward() }));
        // Carregando, o mesmo lugar para a carga, como no Chrome.
        let reload = if self.page.loading && ready {
            chrome::icon_button("browser-stop", IconName::Close, tr("browser_stop"), cx)
                .on_click(cx.listener(|this, _, _, _| if let Some(engine) = this.engine() { engine.stop() }))
        } else {
            chrome::icon_button("browser-reload", IconName::RotateCw, tr("browser_reload"), cx)
                .disabled(!ready || self.page.url.is_none())
                .on_click(cx.listener(|this, _, window, cx| this.retry(window, cx)))
        };
        let url = self.page.url.clone().filter(|u| u.starts_with("http://") || u.starts_with("https://"));
        let copy = chrome::icon_button("browser-copy", if self.copied { IconName::Check } else { IconName::Copy }, tr("browser_copy"), cx)
            .disabled(url.is_none())
            .on_click(cx.listener({ let url = url.clone(); move |this, _, _, cx| this.copy_address(url.clone(), cx) }));
        let open_out = chrome::icon_button("browser-open-out", IconName::ExternalLink, tr("browser_open_out"), cx)
            .disabled(url.is_none())
            .on_click(cx.listener(move |_, _, _, cx| if let Some(url) = &url { cx.open_url(url) }));
        let address = div().flex_1().min_w_0()
            .capture_action(cx.listener(|this, _: &Escape, window, cx| { cx.stop_propagation(); this.restore_address(window, cx); }))
            .child(Input::new(&self.address).id("browser-address").small().aria_label(tr("browser_address"))
                .when(self.page.loading, |el| el.suffix(chrome::Spinner::new("browser-loading", IconName::LoaderCircle, px(12.), theme::muted()))));
        #[allow(unused_mut)]
        let mut toolbar = div().id("browser-toolbar").flex_shrink_0().flex().items_center().gap_1().px_2().py(px(6.))
            .border_b_1().border_color(theme::border())
            .child(back).child(forward).child(reload).child(address)
            // O selo só aparece com erro: página limpa não ganha um zero na barra.
            .when(self.error_count > 0, |el| el.child(div().id("browser-errors-badge").flex_shrink_0().flex().items_center().gap_1()
                .px_2().py(px(2.)).rounded_full().cursor_pointer().bg(theme::danger().opacity(0.15)).border_1().border_color(theme::danger().opacity(0.4))
                .text_size(px(11.)).text_color(theme::danger()).role(Role::Button)
                .aria_label(tr("browser_errors_count").replace("{n}", &self.error_count.to_string()))
                .child(div().size(px(6.)).rounded_full().bg(theme::danger()))
                .child(if self.error_count == 1 { tr("browser_errors_one") } else { tr("browser_errors_count").replace("{n}", &self.error_count.to_string()) })
                .on_click(cx.listener(|this, _, _, cx| { this.errors_open = !this.errors_open; cx.notify(); }))))
            .child(copy).child(open_out);
        // Botão que traz o login do Chrome real do usuário (só onde o motor fala CDP).
        #[cfg(not(target_os = "macos"))]
        {
            toolbar = toolbar.child(chrome::icon_button("browser-shot", IconName::Image, tr("browser_send_shot"), cx)
                .disabled(!ready || self.page.url.is_none() || self.shooting)
                .on_click(cx.listener(|this, _, _, cx| this.send_shot(cx))));
            toolbar = toolbar.child(self.pick_button(ready, cx));
            toolbar = toolbar.child(Button::new("browser-cookies")
                .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted())
                    .hover(theme::hover()).active(theme::hover()))
                .icon(Icon::new(IconName::Key).size(px(14.))).label(tr("browser_cookies_bring"))
                .h(px(28.)).px(px(8.)).rounded(px(6.)).text_size(px(12.)).flex_shrink_0()
                .on_click(cx.listener(|this, _, window, cx| this.ask_chrome_login(window, cx))));
        }
        // Com a página na tela, a falha dela fica numa linha; sem página, `render_page` a mostra no lugar dela.
        // A queda vence o endereço recusado: com a página branca, ela é o que explica a tela.
        let dead = self.engine().is_some_and(|e| !e.alive());
        let page_error = self.page.error.clone().filter(|_| !cfg!(target_os = "linux") && ready && self.page.url.is_some());
        let notice = if dead { page_error.clone() } else { self.invalid.clone().or(page_error) };
        #[allow(unused_mut)]
        let mut root = div().size_full().flex().flex_col().key_context("BrowserPanel")
            // Antes da página: atalhos de navegador valem no painel inteiro e não chegam a ela.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let k = &event.keystroke;
                let (m, page) = (k.modifiers, this.focus.is_focused(window));
                let only = |alt: bool| m.alt == alt && !m.control && !m.platform && !m.shift;
                match k.key.as_str() {
                    #[cfg(not(target_os = "macos"))]
                    "escape" if this.pick.on && !m.modified() => this.stop_pick(cx),
                    "l" if m.secondary() && !m.alt && !m.shift => this.focus_address(window, cx),
                    "f5" if !m.modified() => this.retry(window, cx),
                    "r" if m.secondary() && !m.alt && !m.shift => this.retry(window, cx),
                    "left" if page && only(true) => { if let Some(engine) = this.engine() { engine.back() } }
                    "right" if page && only(true) => { if let Some(engine) = this.engine() { engine.forward() } }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(toolbar)
            .when_some(notice, |el, text| el.child(div().id("browser-notice").flex_shrink_0().flex().items_center().gap_2().px_3().py(px(6.)).role(Role::Alert)
                .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(theme::danger()).whitespace_normal().child(text))
                .when(dead, |el| el.child(Button::new("browser-notice-retry").outline().xsmall().icon(IconName::RotateCw).label(tr("browser_retry"))
                    .on_click(cx.listener(|this, _, window, cx| this.retry(window, cx)))))));
        #[cfg(not(target_os = "macos"))]
        {
            root = root.when_some(self.cookies_status.clone(), |el, text| el.child(div().id("browser-cookies-status")
                .flex_shrink_0().px_3().py(px(6.)).role(Role::Status)
                .text_size(px(11.)).text_color(theme::muted()).whitespace_normal().child(text)));
        }
        root = root.children(self.render_dialog(cx));
        root = root.children(self.render_errors(cx));
        let notice_line = |id: &'static str, text: String, cx: &mut Context<Self>, close: fn(&mut Self)| div().id(id).flex_shrink_0().flex()
            .items_center().gap_2().px_3().py(px(4.)).border_b_1().border_color(theme::border()).role(Role::Status)
            .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(theme::warning()).whitespace_normal().child(text))
            .child(chrome::icon_button(SharedString::from(format!("{id}-close")), IconName::Close, tr("close"), cx)
                .on_click(cx.listener(move |this, _, _, cx| { close(this); cx.notify(); })));
        #[cfg(target_os = "linux")]
        if self.popup.is_some() {
            root = root.child(div().id("browser-popup").flex_shrink_0().flex().items_center().gap_2().px_3().py(px(4.))
                .border_b_1().border_color(theme::border()).role(Role::Status)
                .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(theme::muted()).whitespace_normal().child(tr("browser_popup_open")))
                .child(Button::new("browser-popup-close").outline().xsmall().icon(IconName::Close).label(tr("browser_popup_close"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_popup(window, cx)))));
        }
        let page_notice = self.page.notice.clone().filter(|_| self.page.notice_seq != self.dismissed_notice);
        if let Some(text) = page_notice { root = root.child(notice_line("browser-page-notice", text, cx, |this| this.dismissed_notice = this.page.notice_seq)); }
        if let Some(text) = self.local_notice.clone() { root = root.child(notice_line("browser-local-notice", text, cx, |this| this.local_notice = None)); }
        #[cfg(not(target_os = "macos"))]
        { root = root.children(self.render_pick(cx)).children(self.render_agent(cx)); }
        #[cfg(not(target_os = "macos"))]
        if let Some((site, _)) = self.login_request.as_ref().filter(|(_, reply)| !reply.is_canceled()) {
            let text = match site {
                Some(site) => tr("browser_login_agent_site").replace("{site}", site),
                None => tr("browser_login_agent_all"),
            };
            root = root.child(div().id("browser-login-request").flex_shrink_0().flex().items_center().gap_2().px_3().py(px(6.))
                .border_b_1().border_color(theme::border()).role(Role::Alert)
                .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(theme::text()).whitespace_normal().child(text))
                .child(Button::new("browser-login-deny").small().label(tr("browser_login_agent_deny"))
                    .on_click(cx.listener(|this, _, window, cx| this.answer_login_request(false, window, cx))))
                .child(Button::new("browser-login-allow").small().primary().label(tr("browser_login_agent_allow"))
                    .on_click(cx.listener(|this, _, window, cx| this.answer_login_request(true, window, cx)))));
        }
        #[cfg(not(target_os = "macos"))]
        if let Some(site) = self.login_hint.clone() {
            root = root.child(div().id("browser-login-hint").flex_shrink_0().flex().items_center().gap_2().px_3().py(px(6.))
                .border_b_1().border_color(theme::border()).role(Role::Status)
                .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(theme::muted()).whitespace_normal()
                    .child(tr("browser_login_hint").replace("{site}", &site)))
                .child(Button::new("browser-login-hint-use").small().label(tr("browser_login_hint_use"))
                    .on_click(cx.listener(|this, _, window, cx| this.ask_chrome_login(window, cx))))
                .child(chrome::icon_button("browser-login-hint-close", IconName::Close, tr("close"), cx)
                    .on_click(cx.listener(|this, _, _, cx| { this.login_hint = None; cx.notify(); }))));
        }
        #[cfg(target_os = "linux")]
        if !self.cookie_sources.is_empty() {
            let picks = self.cookie_sources.iter().enumerate().map(|(i, source)| {
                let source = source.clone();
                Button::new(("browser-cookie-source", i)).small().label(source.label.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.read_disk_cookies(source.clone(), cx)))
            });
            root = root.child(div().id("browser-cookie-sources").flex_shrink_0().flex().flex_wrap().gap_1().px_3().pb(px(6.))
                .children(picks));
        }
        root.child(self.render_page(cx))
    }
}

/// Zoom e deslocamento da área visível da página, no formato que `viewport_of` lê.
#[cfg(target_os = "linux")]
const VIEWPORT: &str = "[visualViewport.scale,visualViewport.offsetLeft,visualViewport.offsetTop]";

#[cfg(target_os = "linux")]
fn viewport_of(v: &serde_json::Value) -> Option<(f32, f32, f32)> {
    let n = |i: usize| v.get(i).and_then(serde_json::Value::as_f64).map(|x| x as f32);
    Some((n(0).filter(|z| *z > 0.)?, n(1)?, n(2)?))
}

/// Linha do registro do agente (sem acento, com o tipo na frente) como a pessoa lê na lista do selo.
fn error_text(line: &str) -> String {
    if let Some(rest) = line.strip_prefix("erro nao tratado: ") { return tr("browser_errors_uncaught").replace("{e}", rest); }
    ["error: ", "assert: "].iter().find_map(|p| line.strip_prefix(p)).unwrap_or(line).to_owned()
}

/// "nav-tela está usando o navegador · clicando em “Salvar”". Nome longo de alvo é cortado: a faixa tem uma linha.
#[cfg(not(target_os = "macos"))]
fn describe_activity(key: &str, a: &crate::browser::control::Activity) -> String {
    let session = key.rsplit("::").next().unwrap_or(key);
    let target = a.target.as_deref().map(|t| {
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.chars().count() > 40 { format!("{}…", t.chars().take(39).collect::<String>()) } else { t }
    });
    let (named, plain) = match a.verb.as_str() {
        "click" => ("browser_ctl_click", "browser_ctl_click_plain"),
        "fill" => ("browser_ctl_fill", "browser_ctl_fill_plain"),
        "hover" => ("browser_ctl_hover", "browser_ctl_hover_plain"),
        "press" => ("browser_ctl_press", "browser_ctl_type"),
        "type" => ("", "browser_ctl_type"),
        "snapshot" | "text" | "url" | "console" | "network" => ("", "browser_ctl_read"),
        "wait" => ("", "browser_ctl_wait"),
        "shot" => ("", "browser_ctl_shot"),
        _ => ("", ""),
    };
    let using = tr("browser_ctl_using").replace("{session}", session);
    let doing = match target {
        Some(t) if !named.is_empty() => tr(named).replace("{target}", &t),
        _ if !plain.is_empty() => tr(plain),
        _ => return using,
    };
    format!("{using} · {doing}")
}

impl Hangar {
    /// Linha Navegador do menu do painel: abre o navegador na primeira vez e leva à aba dele.
    pub(super) fn open_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_is_remote() {
            if self.remote_browser_for(cx).is_some() { self.show_browser_tab(window, cx); }
            return;
        }
        let Some(key) = self.browser_key() else { return };
        #[cfg(target_os = "linux")]
        { self.side.warm = Engine::warm(window, cx).inspect_err(|e| eprintln!("[nav] Chromium nao subiu antes do endereco: {e}")).ok(); }
        let browser = self.browser_for(key, window, cx);
        self.show_browser_tab(window, cx);
        browser.update(cx, |panel, cx| panel.focus_address(window, cx));
    }

    /// "Abrir no painel" do site da conversa: o mesmo endereço no navegador desta sessão, à frente.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub(super) fn open_in_browser_panel(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        // Sem sessão com painel o link não se perde: vai ao navegador do sistema.
        let Some(key) = self.browser_key() else { return cx.open_url(&url) };
        let browser = self.browser_for(key, window, cx);
        browser.update(cx, |panel, cx| panel.go(url, By::User, window, cx));
        self.show_browser_tab(window, cx);
    }

    /// Painel aberto com a aba Navegador da sessão aberta à frente.
    fn show_browser_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.side_key() else { return };
        if !self.side.open { self.toggle_side(cx); }
        self.side.browser_open.insert(key);
        self.choose_side_tab(SideTab::Browser, window, cx);
    }

    /// O × da aba Navegador: a aba sai desta sessão e a página se esconde, mas o motor fica para o "+" reabrir a mesma
    /// página.
    pub(super) fn close_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let showing = self.side_tab() == SideTab::Browser;
        if let Some(key) = self.side_key() { self.side.browser_open.remove(&key); }
        #[cfg(target_os = "linux")]
        { self.side.warm = None; }
        for browser in self.side.browsers.values().cloned().collect::<Vec<_>>() { browser.update(cx, |panel, cx| { panel.set_shown(false, false, cx); }); }
        if showing { self.choose_side_tab(SideTab::Context, window, cx); } else { cx.notify(); }
    }

    /// No Windows e no macOS a página é uma janela filha, por cima de tudo o que a GPUI desenha: qualquer camada sobre o
    /// painel a esconde. Cobre o painel fora de vista (fechado, estreito, sem sessão, visor de arquivos expandido), outra
    /// aba à frente (menu, subagente), as páginas de Configurações e Custos, a caixa de configurações ao vivo, a conexão,
    /// a busca, os painéis presos ao compositor e à barra do topo, os diálogos e folhas do kit e as notificações. A aba
    /// fechada no × sai de `side_tab`, então também esconde. No
    /// Linux a página é desenhada pela própria GPUI, e as camadas passam por cima dela sem precisar escondê-la.
    /// `(visível, coberta)`: coberta = estaria à vista, não fosse uma camada por cima.
    fn browser_visible(&self, window: &mut Window, cx: &mut App) -> (bool, bool) {
        let covered = cfg!(not(target_os = "linux")) && (self.connection_dialog || self.search.open || self.popup_open()
            || window.has_active_dialog(cx) || window.has_active_sheet(cx) || !window.notifications(cx).is_empty());
        let in_view = self.side_width(window).is_some() && !self.files_expanded()
            && !self.side_menu_shown() && !self.subagent_tab_open() && self.side_tab() == SideTab::Browser
            && self.settings.is_none() && self.costs.view.is_none() && self.worktrees.view.is_none();
        (in_view && !covered, in_view && covered)
    }

    /// A sessão aberta é de outra máquina: o navegador dela mora no app de lá, e aqui só se vê a tela dele.
    pub(super) fn session_is_remote(&self) -> bool { self.session_api().is_some_and(|api| !api.is_loopback()) }

    /// Tela remota da sessão aberta, criada na primeira vez com a conexão e o token da máquina dela.
    fn remote_browser_for(&mut self, cx: &mut Context<Self>) -> Option<Entity<super::remote_browser::RemoteBrowser>> {
        let key = self.side_key()?;
        if let Some(remote) = self.side.remote_browsers.get(&key) { return Some(remote.clone()); }
        let (api, name, server) = (self.session_api()?, self.selected.as_ref()?.name.clone(), self.open_server());
        // O token vai na URL do socket: é o da máquina da sessão, não o do servidor ativo.
        let token = if self.is_active_key(&server) { self.active_token.clone() }
            else { self.server_entry(&server).map(|s| s.token.clone()).unwrap_or_default() };
        let runtime = self.runtime.handle().clone();
        let remote = cx.new(|cx| super::remote_browser::RemoteBrowser::new(name, api, token, runtime, cx));
        self.side.remote_browsers.insert(key, remote.clone());
        Some(remote)
    }

    /// A tela remota da sessão aberta, quando ela é de outra máquina e já foi aberta.
    pub(super) fn remote_browser_shown(&self) -> Option<Entity<super::remote_browser::RemoteBrowser>> {
        if !self.session_is_remote() { return None; }
        self.side.remote_browsers.get(&self.side_key()?).cloned()
    }

    /// A cada quadro da janela, antes das áreas guardadas desenharem.
    pub(super) fn sync_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (visible, covered) = self.browser_visible(window, cx);
        // A tela remota é desenhada pela GPUI: camada por cima não a esconde, só sair de vista a desconecta.
        let remote = self.remote_browser_shown().map(|r| r.entity_id());
        for (_, view) in self.side.remote_browsers.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>() {
            let shown = (visible || covered) && remote == Some(view.entity_id());
            view.update(cx, |view, cx| view.set_shown(shown, cx));
        }
        let current = self.browser_key();
        for (key, browser) in self.side.browsers.iter().map(|(k, b)| (k.clone(), b.clone())).collect::<Vec<_>>() {
            let mine = current.as_deref() == Some(key.as_str());
            #[cfg(not(target_os = "macos"))]
            {
                let name = key.rsplit("::").next().unwrap_or(&key);
                let working = self.sessions.iter().any(|s| s.name == name && s.state == "working");
                if browser.read(cx).session_working != working { browser.update(cx, |panel, cx| panel.set_session_working(working, cx)); }
            }
            let shown = visible && mine;
            #[cfg(target_os = "windows")]
            if !browser.read(cx).released.is_empty() {
                for image in browser.update(cx, |panel, _| std::mem::take(&mut panel.released)) { super::release_image(image, window, cx); }
            }
            if browser.update(cx, |panel, cx| panel.set_shown(shown, covered && mine, cx)) && shown {
                // O painel é guardado entre quadros: sem um redesenho dele, a página não volta a se posicionar.
                let id = browser.entity_id();
                window.on_next_frame(move |_, cx| cx.notify(id));
            }
        }
    }

    /// Dono do navegador na tela: a sessão aberta, com a máquina dela; uma chave só no macOS.
    pub(super) fn browser_key(&self) -> Option<String> {
        if !cfg!(target_os = "macos") { self.side_key() } else { Some("*".into()) }
    }

    /// `servidor::sessão` da sessão aberta: dono da aba lembrada e da aba Navegador do painel.
    pub(super) fn side_key(&self) -> Option<String> {
        Some(format!("{}::{}", super::servers::norm(&self.session_server()?), self.selected.as_ref()?.name))
    }

    fn browser_for(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) -> Entity<BrowserPanel> {
        if let Some(browser) = self.side.browsers.get(&key) { return browser.clone(); }
        let browser = cx.new(|cx| BrowserPanel::new(key.clone(), window, cx));
        cx.subscribe(&browser, |this, _, event: &PanelEvent, cx| match event {
            PanelEvent::Shot { key, bytes } => this.attach_page_shot(key, bytes.clone(), cx),
            #[cfg(not(target_os = "macos"))]
            PanelEvent::Element { key, element, png } => this.attach_page_element(key, element.clone(), png.clone(), cx),
            #[cfg(target_os = "macos")]
            PanelEvent::Element { .. } => {}
        }).detach();
        self.side.browsers.insert(key, browser.clone());
        browser
    }

    /// O print vai para a mensagem da sessão aberta, como uma imagem colada: o botão só existe com o painel dela à vista.
    /// Se a pessoa trocou de sessão enquanto o print saía, ele não vai para a conversa errada.
    fn attach_page_shot(&mut self, owner: &str, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let Some(key) = self.page_composer(owner, "print") else { return };
        self.attach_png(&key, "pagina", bytes);
        cx.notify();
    }

    /// O campo de mensagem que recebe o que veio do navegador de `owner` (`what`, para o diário): só o da sessão dele.
    fn page_composer(&self, owner: &str, what: &str) -> Option<SessionKey> {
        if self.browser_key().as_deref() != Some(owner) { eprintln!("[nav] {what} de {owner} descartado: outra sessao aberta"); return None; }
        self.composer_key()
    }

    /// Imagem da página anexada como uma colada (`<prefix>-<n>.png`); devolve o id do anexo, ou mostra por que não entrou.
    fn attach_png(&mut self, key: &SessionKey, prefix: &str, bytes: Vec<u8>) -> Option<u64> {
        let name = format!("{prefix}-{}.png", self.attach_seq + 1);
        match self.add_attachment(key, name, bytes) {
            Ok(()) => Some(self.attach_seq),
            Err(problem) => { self.action_feedback.insert(key.clone(), (problem, true)); None }
        }
    }

    /// `hangar-preview open` → backend → evento `nav` na lista. Com a sessão na tela a aba Navegador vem à frente; fora
    /// dela o navegador nasce escondido, o CLI já o dirige, e a aba aparece quando a sessão for aberta. Só do servidor
    /// desta máquina: o CLI que pediu roda nela.
    pub(super) fn receive_nav(&mut self, data: serde_json::Value, window: &mut Window, cx: &mut Context<Self>) {
        if cfg!(target_os = "macos") { return; }
        if !self.api.as_ref().is_some_and(|api| api.is_loopback()) { eprintln!("[nav] nav ignorado: servidor ativo nao e desta maquina"); return; }
        let (Some(name), Some(url)) = (data["name"].as_str(), data["url"].as_str()) else { eprintln!("[nav] nav ignorado: sem name/url: {data}"); return };
        // Mesma chave de `browser_key` com essa sessão aberta: a lista que traz o evento é a do servidor ativo.
        let Some(server) = self.server.as_deref() else { eprintln!("[nav] nav ignorado: sem servidor ativo"); return };
        let key = format!("{}::{name}", super::servers::norm(server));
        let browser = self.browser_for(key.clone(), window, cx);
        // Pausado pela pessoa, o agente não troca a página dela; o `open` do CLI pergunta o `estado` e devolve o erro.
        browser.update(cx, |panel, cx| if panel.agent_paused() { eprintln!("[nav] nav ignorado: navegador pausado pelo usuario") } else { panel.go(url.to_owned(), By::Agent, window, cx) });
        if self.browser_key().as_deref() == Some(key.as_str()) { self.show_browser_tab(window, cx); } else {
            self.side.browser_open.insert(key.clone());
            self.side.tabs.insert(key, SideTab::Browser);
        }
        if let Some(api) = self.api.clone() {
            let name = name.to_owned();
            // Sem a confirmação o backend manda o mesmo `nav` de novo na próxima conexão, que renavega e traz a aba de volta.
            self.runtime.spawn(async move {
                if let Err(e) = api.server_send(reqwest::Method::DELETE, &["sessions", &name, "nav"], None, 15).await {
                    eprintln!("[nav] confirmação do nav de {name} falhou, ele volta na próxima conexão: {e:?}");
                }
            });
        }
    }

    /// `/cdp/browser/<nome>` sem token: a sessão é do servidor desta máquina, e o navegador dela nasce (escondido, como
    /// no `hangar-preview open` fora da tela) se ainda não existir. Devolve a chave.
    #[cfg(target_os = "linux")]
    fn open_for_cdp(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, (u16, String)> {
        let local = self.api.as_ref().is_some_and(|api| api.is_loopback());
        let Some(server) = self.server.as_deref().filter(|_| local) else {
            return Err((503, "erro: no_local_server: o servidor ativo do app nao e o desta maquina".into()));
        };
        if !self.sessions.iter().any(|s| s.name == name) {
            return Err((404, format!("erro: session_not_found: a sessao {name} nao existe no servidor desta maquina")));
        }
        let key = format!("{}::{name}", super::servers::norm(server));
        let browser = self.browser_for(key.clone(), window, cx);
        browser.update(cx, |panel, cx| if panel.starting.is_none() && panel.engine().is_none_or(|e| !e.alive()) {
            panel.go("about:blank".into(), By::Agent, window, cx);
        });
        // A aba aparece na sessão; a da tela não troca de aba sozinha.
        self.side.browser_open.insert(key.clone());
        if self.browser_key().as_deref() != Some(key.as_str()) { self.side.tabs.insert(key.clone(), SideTab::Browser); }
        cx.notify();
        Ok(key)
    }

    /// Pedido do servidor local: vai ao navegador da sessão pedida, esteja ela na tela ou não.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn dispatch_preview(&mut self, request: crate::browser::server::Request, window: &mut Window, cx: &mut Context<Self>) {
        use crate::browser::{control::Reply, server::Request};
        fn answer(reply: futures::channel::oneshot::Sender<Reply>, text: String) { let _ = reply.send(Reply::Text(text)); }
        let missing = |key: &str| format!("erro: a sessao {key} nao tem navegador aberto");
        let (key, verb, args, tab, reply) = match request {
            Request::Command { key, verb, args, tab, reply } => (key, verb, args, tab, reply),
            Request::Cdp { key, method, params, reply } => {
                let Some(cdp) = self.side.browsers.get(&key).and_then(|b| b.read(cx).relay_cdp()) else { let _ = reply.send(Err(missing(&key))); return };
                let call = cdp.call(&method, params);
                return cx.spawn(async move |_, _| { let _ = reply.send(call.await); }).detach();
            }
            Request::Watch { key, viewer, reply } => {
                match self.side.browsers.get(&key).cloned() {
                    Some(b) => b.update(cx, |panel, cx| {
                        #[cfg(target_os = "windows")]
                        panel.touch(cx);
                        panel.watch(viewer, reply, cx)
                    }),
                    None => { let _ = reply.send(Err(missing(&key))); }
                }
                return;
            }
            Request::Unwatch { key, id } => {
                if let Some(b) = self.side.browsers.get(&key) { b.read(cx).unwatch(id); }
                return;
            }
            #[cfg(target_os = "linux")]
            Request::Pipe { key, reply } => {
                let found = self.side.browsers.get(&key).and_then(|b| b.read(cx).engine().filter(|e| e.alive()).map(|e| e.port()));
                let _ = reply.send(found.ok_or_else(|| missing(&key)));
                return;
            }
            #[cfg(target_os = "linux")]
            Request::Open { name, reply } => { let _ = reply.send(self.open_for_cdp(&name, window, cx)); return; }
        };
        if tab.is_some() { return answer(reply, "erro: o app nativo ainda nao tem abas: e um navegador por sessao".into()); }
        if verb == "login" {
            let Some(browser) = self.side.browsers.get(&key).cloned() else { return answer(reply, missing(&key)) };
            let site = match args.first().map(|a| crate::browser::chrome_import::site_of(a)) {
                Some(None) => return answer(reply, "erro: login [site]: site invalido (ex.: hangar-preview login github.com)".into()),
                Some(site) => site,
                None => None,
            };
            browser.update(cx, |panel, cx| panel.request_login(site, reply, cx));
            // A linha de aprovação fica no painel Navegador da sessão: é ele que aparece quando a pessoa abre a sessão.
            self.side.browser_open.insert(key.clone());
            self.side.tabs.insert(key, SideTab::Browser);
            cx.notify();
            return;
        }
        if verb == "close" {
            let closed = self.side.browsers.remove(&key).inspect(|b| b.read(cx).end_viewer()).is_some();
            if closed { crate::browser::server::remove_sidecar(&key); self.side.browser_open.remove(&key); cx.notify(); }
            return answer(reply, if closed { "ok: close".into() } else { format!("erro: a sessao {key} nao tem navegador aberto") });
        }
        let Some(browser) = self.side.browsers.get(&key).cloned() else { return answer(reply, missing(&key)) };
        #[cfg(target_os = "windows")]
        let woke = browser.update(cx, |panel, cx| panel.touch(cx));
        let panel = browser.read(cx);
        // Página que caiu não responde: sem isto cada verbo esperava o prazo inteiro do controlador.
        if panel.engine().is_some_and(|e| !e.alive()) {
            return answer(reply, format!("erro: a pagina da sessao {key} caiu; abra de novo com hangar-preview open <url>"));
        }
        let Some(ctl) = panel.controller() else {
            return answer(reply, match panel.engine_status() {
                Some(Err(e)) => format!("erro: o navegador da sessao {key} falhou ao iniciar: {e}"),
                Some(Ok(())) => format!("erro: o navegador da sessao {key} ainda esta iniciando, tente de novo em instantes"),
                None => format!("erro: a sessao {key} nao tem navegador aberto"),
            });
        };
        let browser = browser.downgrade();
        let _ = browser.update(cx, |panel, _| panel.agent_started());
        #[cfg(target_os = "windows")]
        let settle = woke.then(|| cx.background_executor().timer(WAKE_SETTLE));
        cx.spawn(async move |_, cx| {
            #[cfg(target_os = "windows")]
            if let Some(settle) = settle { settle.await; }
            let _ = reply.send(ctl.run(&verb, &args).await);
            let _ = browser.update(cx, |panel, cx| panel.agent_finished(cx));
        }).detach();
    }
}
