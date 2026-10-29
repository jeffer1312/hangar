//! Navegador de uma sessão de outra máquina: a tela chega pelo `nav-remoto` do servidor dela, o mesmo canal da tela
//! remota do celular, e clique, roda e tecla voltam por ele. O navegador continua sendo o do app daquela máquina.
use std::{cell::Cell, rc::Rc, time::{Duration, Instant}};

use base64::Engine as _;
use serde_json::{Value, json};

use super::*;
use crate::{api::Api, ws::{self, TextEvent, TextSocket}};

/// Movimento do mouse vai no máximo a cada este tempo: o socket tem fila curta e o outro lado é rede.
const MOVE_EVERY: Duration = Duration::from_millis(40);
/// Largura máxima do quadro pedido, a mesma que o servidor aceita.
const MAX_WIDTH: f32 = 1400.;
const QUALITY: u32 = 80;
/// Tamanho do painel repassado à página no máximo a cada este tempo, enquanto ele é arrastado.
const RESIZE_EVERY: Duration = Duration::from_millis(500);

enum Status { Idle, Connecting, Live, Down(String) }

pub(super) struct RemoteBrowser {
    name: String,
    api: Api,
    token: String,
    runtime: tokio::runtime::Handle,
    socket: Option<TextSocket>,
    status: Status,
    url: Option<String>,
    frame: Option<Arc<RenderImage>>,
    /// Quadro tirado por print: a tela da outra máquina não está compondo a página.
    slow: bool,
    shown: bool,
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    scale: Rc<Cell<f32>>,
    last_move: Option<Instant>,
    /// Tamanho do painel já imposto à página da outra máquina, e quando.
    sent_size: Option<((u32, u32), Instant)>,
    /// Reenvio do tamanho agendado para depois do intervalo, enquanto o painel é arrastado.
    resize_pending: bool,
    /// Erro ou recusa que a outra máquina mandou sem fechar a conexão.
    notice: Option<String>,
    /// Sobe a cada conexão: evento de uma conexão anterior que chega atrasado é descartado.
    seq: u64,
    _read: Task<()>,
}

impl RemoteBrowser {
    pub(super) fn new(name: String, api: Api, token: String, runtime: tokio::runtime::Handle, cx: &mut Context<Self>) -> Self {
        Self { name, api, token, runtime, socket: None, status: Status::Idle, url: None, frame: None, slow: false, shown: false,
            focus: cx.focus_handle(), bounds: Rc::default(), scale: Rc::new(Cell::new(1.)), last_move: None, sent_size: None, resize_pending: false, notice: None, seq: 0, _read: Task::ready(()) }
    }

    /// Só conectado enquanto está na tela: o canal aceita um espectador por vez, e fechado ele volta a quem precisar.
    /// Chamado a cada quadro da janela, que é quando o painel muda de tamanho.
    pub(super) fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if shown == self.shown {
            if shown { self.sync_size(cx); }
            return;
        }
        self.shown = shown;
        if shown { self.connect(cx) } else { self.disconnect(cx) }
        cx.notify();
    }

    /// Fecha a conexão e solta o último quadro: reaberta, a aba não mostra uma tela velha como se fosse a atual.
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.seq += 1;
        if let Some(mut socket) = self.socket.take() { socket.close(); }
        self._read = Task::ready(());
        self.status = Status::Idle;
        self.url = None;
        if let Some(old) = self.frame.take() { cx.drop_image(old, None); }
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        self.disconnect(cx);
        let seq = self.seq;
        let width = f32::from(self.bounds.get().size.width) * self.scale.get();
        let width = if width > 0. { width.min(MAX_WIDTH) } else { MAX_WIDTH };
        // Tela de computador: qualidade acima da do celular, que paga a banda no túnel.
        let path = format!("/api/sessions/{}/nav-remoto?w={}&q={QUALITY}", self.name, width.round() as u32);
        let socket = TextSocket::open(&self.runtime, &self.api, &path, self.token.clone());
        let events = socket.events();
        self.socket = Some(socket);
        self.status = Status::Connecting;
        (self.sent_size, self.resize_pending, self.notice) = (None, false, None);
        self._read = cx.spawn(async move |this, cx| {
            let mut said: Option<String> = None;
            while let Ok(event) = events.recv().await {
                let text = match event {
                    Ok(TextEvent::Connected) => {
                        if this.update(cx, |this, cx| if this.seq == seq { this.status = Status::Live; this.sync_size(cx); cx.notify(); }).is_err() { return; }
                        continue;
                    }
                    Ok(TextEvent::Text(text)) => text,
                    Ok(TextEvent::Closed) => break,
                    Err(error) => { said.get_or_insert_with(|| failure_text(error)); break; }
                };
                let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };
                match msg["t"].as_str() {
                    Some("q") => {
                        let (data, slow) = (msg["d"].as_str().unwrap_or("").to_owned(), msg["lento"].as_bool().unwrap_or(false));
                        let image = cx.background_spawn(async move {
                            let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
                            crate::media::decode(&bytes, 4096, 4096, None)
                        }).await;
                        let Some(image) = image else { eprintln!("[nav] tela remota: quadro ilegível descartado"); continue };
                        let kept = this.update(cx, |this, cx| {
                            if this.seq != seq { return cx.drop_image(image, None); }
                            if let Some(old) = this.frame.replace(image) { cx.drop_image(old, None); }
                            this.slow = slow;
                            cx.notify();
                        });
                        if kept.is_err() { return; }
                    }
                    Some("url") => {
                        let url = msg["url"].as_str().map(str::to_owned);
                        if this.update(cx, |this, cx| if this.seq == seq { this.url = url; cx.notify(); }).is_err() { return; }
                    }
                    Some("layout") => {
                        let refused = msg["resposta"].as_str().filter(|r| !r.starts_with("layout:")).map(str::to_owned);
                        if this.update(cx, |this, cx| if this.seq == seq { this.notice = refused; cx.notify(); }).is_err() { return; }
                    }
                    // O servidor manda erro sem fechar quando o tamanho não pôde ser aplicado: aparece já, na linha
                    // de aviso, e vira o motivo se a conexão cair logo em seguida.
                    Some("erro") => {
                        said = msg["m"].as_str().map(str::to_owned);
                        let text = said.clone();
                        if this.update(cx, |this, cx| if this.seq == seq { this.notice = text; cx.notify(); }).is_err() { return; }
                    }
                    _ => {}
                }
            }
            let reason = said.unwrap_or_else(|| tr_shared("nav_remoto_caiu", &[]));
            let _ = this.update(cx, |this, cx| if this.seq == seq {
                this.socket = None;
                this.notice = None;
                this.status = Status::Down(reason);
                cx.notify();
            });
        });
    }

    /// A página da outra máquina toma o tamanho deste painel; ela devolve o de antes quando a conexão fecha.
    fn sync_size(&mut self, cx: &mut Context<Self>) {
        let size = self.bounds.get().size;
        let (w, h) = (f32::from(size.width).round() as u32, f32::from(size.height).round() as u32);
        if w < 50 || h < 50 || !matches!(self.status, Status::Live) { return; }
        if let Some(((sw, sh), at)) = self.sent_size {
            if sw.abs_diff(w) < 16 && sh.abs_diff(h) < 16 { return; }
            // Arrastando: o último tamanho sai quando o intervalo passar, mesmo sem outro quadro da janela.
            if at.elapsed() < RESIZE_EVERY {
                if !std::mem::replace(&mut self.resize_pending, true) {
                    let wait = RESIZE_EVERY - at.elapsed();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(wait).await;
                        let _ = this.update(cx, |this, cx| { this.resize_pending = false; this.sync_size(cx); });
                    }).detach();
                }
                return;
            }
        }
        if self.send(json!({"t": "layout", "w": w, "h": h})) { self.sent_size = Some(((w, h), Instant::now())); }
    }

    /// `false` = não saiu (sem conexão ou fila cheia).
    fn send(&self, msg: Value) -> bool {
        match (&self.socket, &self.status) {
            (Some(socket), Status::Live) => socket.send(&msg.to_string()).is_ok(),
            _ => false,
        }
    }

    /// Ponto da tela em fração (0..1) do quadro desenhado; fora dele, `None`, ou a borda mais perto com `clamp`.
    fn at(&self, p: Point<Pixels>, clamp: bool) -> Option<(f32, f32)> {
        // A proporção desenhada é a da imagem: no modo de print o tamanho anunciado não é o da página.
        let size = self.frame.as_ref()?.size(0);
        fraction(self.bounds.get(), (size.width.0 as f32, size.height.0 as f32), p, clamp)
    }

    fn mouse(&mut self, action: &str, button: MouseButton, p: Point<Pixels>, modifiers: &Modifiers) {
        let b = match button { MouseButton::Left => "left", MouseButton::Right => "right", MouseButton::Middle => "middle", _ => return };
        // Soltar fora da imagem ainda solta o botão na página: senão ele fica apertado lá.
        let Some((x, y)) = self.at(p, action == "release") else { return };
        self.send(json!({"t": "m", "acao": action, "b": b, "x": x, "y": y, "mod": modifier_bits(modifiers)}));
    }

    fn render_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let note = |text: String, color: Hsla| div().size_full().flex().items_center().justify_center().p_4()
            .text_size(px(12.)).text_color(color).text_center().whitespace_normal().child(text);
        let (bounds, scale) = (self.bounds.clone(), self.scale.clone());
        div().id("remote-browser-page").relative().flex_1().min_h_0().overflow_hidden().bg(theme::background())
            .track_focus(&self.focus).key_context("BrowserPage").role(Role::Application).aria_label(tr_shared("nav_remoto_tela", &[]))
            .child(canvas(|_, _, _| {}, move |b, _, window, _| { bounds.set(b); scale.set(window.scale_factor()); }).absolute().inset_0())
            .map(|el| match (&self.frame, &self.status) {
                (Some(image), _) => el.child(img(image.clone()).absolute().inset_0().size_full().object_fit(ObjectFit::Contain)),
                (None, Status::Down(_)) => el.child(note(tr_shared("nav_remoto_caiu", &[]), theme::muted())),
                (None, _) => el.child(note(tr_shared("nav_remoto_ligando", &[]), theme::muted())),
            })
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.focus.focus(window, cx);
                this.mouse("press", event.button, event.position, &event.modifiers);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, _| this.mouse("release", e.button, e.position, &e.modifiers)))
            .on_mouse_up(MouseButton::Right, cx.listener(|this, e: &MouseUpEvent, _, _| this.mouse("release", e.button, e.position, &e.modifiers)))
            .on_mouse_up(MouseButton::Middle, cx.listener(|this, e: &MouseUpEvent, _, _| this.mouse("release", e.button, e.position, &e.modifiers)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, _| this.mouse("release", e.button, e.position, &e.modifiers)))
            .on_mouse_up_out(MouseButton::Right, cx.listener(|this, e: &MouseUpEvent, _, _| this.mouse("release", e.button, e.position, &e.modifiers)))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(|this, e: &MouseUpEvent, _, _| this.mouse("release", e.button, e.position, &e.modifiers)))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, _| {
                if this.last_move.is_some_and(|t| t.elapsed() < MOVE_EVERY) { return; }
                this.last_move = Some(Instant::now());
                this.mouse("move", MouseButton::Left, e.position, &e.modifiers);
            }))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, _| {
                let Some((x, y)) = this.at(e.position, false) else { return };
                // Mesma conta do navegador local: 17,78 px por linha, e o sinal oposto ao da página.
                let d = e.delta.pixel_delta(px(17.78));
                this.send(json!({"t": "w", "x": x, "y": y, "dx": -f32::from(d.x), "dy": -f32::from(d.y)}));
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                let paste = (k.modifiers.control && !k.modifiers.alt && k.key == "v") || (k.modifiers.shift && k.key == "insert");
                match paste.then(|| cx.read_from_clipboard().and_then(|item| item.text())).flatten() {
                    Some(text) => { this.send(json!({"t": "txt", "s": text})); }
                    None => if let Some(msg) = key_message(k) { this.send(msg); },
                }
                cx.stop_propagation();
            }))
            .into_any_element()
    }
}

impl Render for RemoteBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let down = match &self.status { Status::Down(reason) => Some(reason.clone()), _ => None };
        let url = self.url.clone().filter(|u| !u.is_empty()).unwrap_or_else(|| tr_shared("nav_remoto_sem_url", &[]));
        let toolbar = div().id("remote-browser-toolbar").flex_shrink_0().flex().items_center().gap_2().px_3().py(px(8.))
            .border_b_1().border_color(theme::border())
            .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).font_family("monospace").text_color(theme::muted()).child(url))
            .when(matches!(self.status, Status::Connecting), |el| el.child(chrome::Spinner::new("remote-browser-loading", IconName::LoaderCircle, px(12.), theme::muted())));
        div().size_full().flex().flex_col().key_context("BrowserPanel")
            .child(toolbar)
            .when_some(down, |el, reason| el.child(div().id("remote-browser-down").flex_shrink_0().flex().items_center().gap_2().px_3().py(px(6.))
                .role(Role::Alert)
                .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(theme::danger()).whitespace_normal().child(reason))
                .child(Button::new("remote-browser-retry").outline().xsmall().icon(IconName::RotateCw).label(tr_shared("nav_remoto_religar", &[]))
                    .on_click(cx.listener(|this, _, _, cx| { this.connect(cx); cx.notify(); })))))
            .when_some(self.notice.clone(), |el, text| el.child(div().id("remote-browser-notice").flex_shrink_0().px_3().py(px(4.)).role(Role::Status)
                .text_size(px(11.)).text_color(theme::warning()).whitespace_normal().child(text)))
            .when(self.slow && matches!(self.status, Status::Live), |el| el.child(div().flex_shrink_0().px_3().py(px(4.))
                .text_size(px(11.)).text_color(theme::muted()).child(tr_shared("nav_remoto_lento", &[]))))
            .child(self.render_page(cx))
    }
}

fn failure_text(error: ws::Error) -> String {
    match error {
        // O servidor recusa com 403 tanto a sessão sem navegador aberto quanto o token que não vale.
        ws::Error::Http(401 | 403 | 404) => tr("browser_remote_no_browser"),
        _ => tr_shared("nav_remoto_caiu", &[]),
    }
}

fn modifier_bits(m: &Modifiers) -> u8 {
    [(m.alt, 1), (m.control, 2), (m.platform, 4), (m.shift, 8)].iter().filter(|(on, _)| *on).fold(0, |a, (_, b)| a | b)
}

/// Ponto da tela em fração do quadro desenhado com `ObjectFit::Contain` (centrado, sem esticar).
fn fraction(bounds: Bounds<Pixels>, (fw, fh): (f32, f32), p: Point<Pixels>, clamp: bool) -> Option<(f32, f32)> {
    let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if fw <= 0. || fh <= 0. || bw <= 0. || bh <= 0. { return None; }
    let s = (bw / fw).min(bh / fh);
    let (dw, dh) = (fw * s, fh * s);
    let x = (f32::from(p.x - bounds.origin.x) - (bw - dw) / 2.) / dw;
    let y = (f32::from(p.y - bounds.origin.y) - (bh - dh) / 2.) / dh;
    if clamp { return Some((x.clamp(0., 1.), y.clamp(0., 1.))); }
    ((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)).then_some((x, y))
}

/// Tecla no formato do `nav-remoto`: nomeada vai como tecla, texto digitado como texto, Ctrl/Alt + letra como atalho.
fn key_message(k: &Keystroke) -> Option<Value> {
    let m = &k.modifiers;
    let bits = modifier_bits(m);
    let named = match k.key.as_str() {
        "enter" => "Enter", "backspace" => "Backspace", "delete" => "Delete", "escape" => "Escape", "tab" => "Tab",
        "left" => "ArrowLeft", "up" => "ArrowUp", "right" => "ArrowRight", "down" => "ArrowDown", "home" => "Home",
        "end" => "End", "pageup" => "PageUp", "pagedown" => "PageDown", _ => "",
    };
    if !named.is_empty() { return Some(json!({"t": "k", "key": named, "mod": bits})); }
    if !m.control && !m.alt && !m.platform
        && let Some(text) = k.key_char.as_deref().filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
        return Some(json!({"t": "txt", "s": text}));
    }
    if !(m.control || m.alt) { return None; }
    let mut chars = k.key.chars();
    let c = chars.next()?;
    if chars.next().is_some() { return None; }
    let upper = c.to_ascii_uppercase();
    let code = if upper.is_ascii_alphanumeric() { upper as u32 } else { 0 };
    Some(json!({"t": "k", "key": c.to_string(), "mod": bits, "code": code}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, TestAppContext, prelude::*};
    use gpui_kit::{Bounds, point, px, size};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    enum PeerAction { Text(Value), Close }

    struct Peer {
        width: Option<String>,
        quality: Option<String>,
        actions: async_channel::Sender<PeerAction>,
        input: async_channel::Receiver<Value>,
        closed: async_channel::Receiver<()>,
    }

    struct RemoteServer {
        runtime: tokio::runtime::Runtime,
        address: std::net::SocketAddr,
        peers: async_channel::Receiver<Peer>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        task: Option<tokio::task::JoinHandle<()>>,
    }

    impl RemoteServer {
        fn new() -> Self {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let listener = { let _entered = runtime.enter(); tokio::net::TcpListener::from_std(listener).unwrap() };
            let (connected, peers) = async_channel::unbounded();
            let (stop, mut stopped) = tokio::sync::oneshot::channel();
            let task = runtime.spawn(async move {
                let mut serving = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        _ = &mut stopped => break,
                        accepted = listener.accept() => {
                            let (stream, _) = accepted.unwrap();
                            let connected = connected.clone();
                            serving.spawn(async move { remote_peer(stream, connected).await });
                        }
                        Some(result) = serving.join_next() => result.unwrap(),
                    }
                }
                serving.abort_all();
                while let Some(result) = serving.join_next().await {
                    if let Err(error) = result { assert!(error.is_cancelled()); }
                }
            });
            Self { runtime, address, peers, stop: Some(stop), task: Some(task) }
        }

        fn browser(&self, cx: &mut TestAppContext) -> Entity<RemoteBrowser> {
            let _entered = self.runtime.enter();
            let api = Api::new(&format!("http://{}", self.address), "proof-owner").unwrap();
            cx.update(|cx| cx.new(|cx| RemoteBrowser::new("native-proof".into(), api, "proof-owner".into(), self.runtime.handle().clone(), cx)))
        }
    }

    impl Drop for RemoteServer {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() { let _ = stop.send(()); }
            if let Some(task) = self.task.take() {
                self.runtime.block_on(async { tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap(); });
            }
        }
    }

    async fn remote_peer(mut stream: tokio::net::TcpStream, connected: async_channel::Sender<Peer>) {
        let head = tokio::time::timeout(Duration::from_secs(2), async {
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                assert!(bytes.len() < 16 * 1024);
                bytes.push(stream.read_u8().await.unwrap());
            }
            String::from_utf8(bytes).unwrap()
        }).await.unwrap();
        let path = head.lines().next().unwrap().split(' ').nth(1).unwrap();
        let url = url::Url::parse(&format!("http://proof.test{path}")).unwrap();
        assert_eq!(url.path(), "/api/sessions/native-proof/nav-remoto");
        let parameter = |key: &str| url.query_pairs().find_map(|(k, v)| (k == key).then(|| v.into_owned()));
        assert!(parameter("token").as_deref() == Some("proof-owner"));
        let key = head.lines().filter_map(|line| line.split_once(':')).find_map(|(name, value)| name.eq_ignore_ascii_case("sec-websocket-key").then(|| value.trim())).unwrap();
        stream.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n", ws::accept_key(key)).as_bytes()).await.unwrap();
        let (actions, messages) = async_channel::unbounded();
        let (sent, input) = async_channel::unbounded();
        let (ended, closed) = async_channel::bounded(1);
        connected.send(Peer { width: parameter("w"), quality: parameter("q"), actions, input, closed }).await.ok().unwrap();
        let (mut reader, mut writer) = stream.into_split();
        let mut message = ws::Message::default();
        loop {
            let next = ws::read_client_frame(&mut reader);
            tokio::pin!(next);
            let frame = loop {
                tokio::select! {
                    frame = &mut next => break frame,
                    action = messages.recv() => match action {
                        Ok(PeerAction::Text(value)) => ws::write_server_frame(&mut writer, 1, value.to_string().as_bytes()).await.unwrap(),
                        Ok(PeerAction::Close) => {
                            ws::write_server_frame(&mut writer, 8, &1000u16.to_be_bytes()).await.unwrap();
                            return;
                        }
                        Err(_) => return,
                    }
                }
            };
            let Ok(frame) = frame else { let _ = ended.send(()).await; return };
            if frame.opcode == 8 {
                let _ = ws::write_server_frame(&mut writer, 8, &frame.data).await;
                let _ = ended.send(()).await;
                return;
            }
            if let Some(bytes) = message.push(frame).unwrap() {
                sent.send(serde_json::from_slice(&bytes).unwrap()).await.ok().unwrap();
            }
        }
    }

    fn received<T>(cx: &mut TestAppContext, channel: &async_channel::Receiver<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            cx.run_until_parked();
            if let Ok(value) = channel.try_recv() { return value; }
            assert!(Instant::now() < deadline, "o socket de ensaio precisa progredir");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn until(cx: &mut TestAppContext, browser: &Entity<RemoteBrowser>, predicate: impl Fn(&RemoteBrowser) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            cx.run_until_parked();
            if browser.read_with(cx, |browser, _| predicate(browser)) { return; }
            assert!(Instant::now() < deadline, "o consumidor precisa aplicar o evento");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn chromium() -> Value {
        serde_json::from_str(include_str!("../../../backend/tests/fixtures/contract/remote-browser/chromium.json")).unwrap()
    }

    fn publish(peer: &Peer, value: Value) { peer.actions.try_send(PeerAction::Text(value)).ok().unwrap(); }

    #[gpui::test]
    fn visibility_opens_the_real_socket_and_hide_releases_the_frame_and_connection(cx: &mut TestAppContext) {
        let server = RemoteServer::new();
        let browser = server.browser(cx);
        cx.run_until_parked();
        assert!(server.peers.is_empty());
        browser.update(cx, |browser, cx| {
            browser.bounds.set(Bounds { origin: point(px(0.), px(0.)), size: size(px(2000.), px(600.)) });
            browser.scale.set(2.);
            browser.set_shown(true, cx);
        });
        let first = received(cx, &server.peers);
        assert_eq!((first.width.as_deref(), first.quality.as_deref()), (Some("1400"), Some("80")));
        assert_eq!(received(cx, &first.input), json!({"t": "layout", "w": 2000, "h": 600}));
        publish(&first, json!({"t": "q", "d": chromium()["frame"]["data"], "lento": true}));
        publish(&first, json!({"t": "url", "url": "http://example.test/first"}));
        until(cx, &browser, |b| b.frame.is_some() && b.url.as_deref() == Some("http://example.test/first"));
        let old = browser.read_with(cx, |b, _| Arc::downgrade(b.frame.as_ref().unwrap()));
        let seq = browser.read_with(cx, |b, _| b.seq);
        browser.update(cx, |browser, cx| browser.set_shown(false, cx));
        received(cx, &first.closed);
        assert!(browser.read_with(cx, |b, _| b.socket.is_none() && b.frame.is_none() && b.url.is_none() && matches!(b.status, Status::Idle) && b.seq > seq));
        assert!(old.upgrade().is_none(), "o quadro retirado não fica retido pelo consumidor");
        browser.update(cx, |browser, cx| browser.set_shown(true, cx));
        let second = received(cx, &server.peers);
        assert_eq!(received(cx, &second.input), json!({"t": "layout", "w": 2000, "h": 600}));
        publish(&second, json!({"t": "url", "url": "http://example.test/second"}));
        until(cx, &browser, |b| b.url.as_deref() == Some("http://example.test/second"));
        browser.update(cx, |browser, cx| browser.set_shown(false, cx));
        received(cx, &second.closed);
    }

    #[gpui::test]
    fn incoming_frames_url_layout_and_error_are_consumed_from_the_socket(cx: &mut TestAppContext) {
        let server = RemoteServer::new();
        let browser = server.browser(cx);
        browser.update(cx, |browser, cx| browser.set_shown(true, cx));
        let peer = received(cx, &server.peers);
        assert_eq!((peer.width.as_deref(), peer.quality.as_deref()), (Some("1400"), Some("80")));
        until(cx, &browser, |b| matches!(b.status, Status::Live));
        for data in ["not-base64%", "bm90LWFuLWltYWdl"] { publish(&peer, json!({"t": "q", "d": data})); }
        publish(&peer, json!({"t": "url", "url": "http://example.test/invalid-drained"}));
        until(cx, &browser, |b| b.url.as_deref() == Some("http://example.test/invalid-drained"));
        assert!(browser.read_with(cx, |b, _| b.frame.is_none()));
        publish(&peer, json!({"t": "q", "d": chromium()["frame"]["data"], "lento": true}));
        until(cx, &browser, |b| b.frame.is_some());
        let old = browser.read_with(cx, |b, _| {
            assert_eq!((b.frame.as_ref().unwrap().size(0).width.0, b.frame.as_ref().unwrap().size(0).height.0), (390, 293));
            assert!(b.slow);
            Arc::downgrade(b.frame.as_ref().unwrap())
        });
        publish(&peer, json!({"t": "q", "d": chromium()["screenshot"]["data"]}));
        until(cx, &browser, |b| b.frame.as_ref().is_some_and(|image| image.size(0).width.0 == 800));
        assert!(old.upgrade().is_none());
        assert!(!browser.read_with(cx, |b, _| b.slow));
        publish(&peer, json!({"t": "layout", "resposta": "erro: tamanho recusado"}));
        until(cx, &browser, |b| b.notice.as_deref() == Some("erro: tamanho recusado"));
        publish(&peer, json!({"t": "layout", "resposta": "layout: 800x600"}));
        until(cx, &browser, |b| b.notice.is_none());
        publish(&peer, json!({"t": "erro", "m": "erro: a página de ensaio encerrou"}));
        until(cx, &browser, |b| b.notice.as_deref() == Some("erro: a página de ensaio encerrou"));
        peer.actions.try_send(PeerAction::Close).ok().unwrap();
        until(cx, &browser, |b| matches!(&b.status, Status::Down(reason) if reason == "erro: a página de ensaio encerrou"));
        assert!(browser.read_with(cx, |b, _| b.socket.is_none() && b.notice.is_none()));
        browser.update(cx, |browser, cx| browser.set_shown(false, cx));
    }

    #[gpui::test]
    fn contained_mouse_release_and_key_messages_reach_the_websocket_peer(cx: &mut TestAppContext) {
        let server = RemoteServer::new();
        let browser = server.browser(cx);
        browser.update(cx, |browser, cx| browser.set_shown(true, cx));
        let peer = received(cx, &server.peers);
        publish(&peer, json!({"t": "q", "d": chromium()["screenshot"]["data"]}));
        until(cx, &browser, |b| b.frame.is_some() && matches!(b.status, Status::Live));
        browser.update(cx, |browser, _| {
            browser.bounds.set(Bounds { origin: point(px(10.), px(20.)), size: size(px(1000.), px(600.)) });
            browser.mouse("press", MouseButton::Left, point(px(510.), px(320.)), &Modifiers::default());
            browser.mouse("release", MouseButton::Left, point(px(-100.), px(900.)), &Modifiers { control: true, shift: true, ..Default::default() });
            browser.mouse("press", MouseButton::Left, point(px(50.), px(320.)), &Modifiers::default());
        });
        assert_eq!(received(cx, &peer.input), json!({"t": "m", "acao": "press", "b": "left", "x": 0.5, "y": 0.5, "mod": 0}));
        assert_eq!(received(cx, &peer.input), json!({"t": "m", "acao": "release", "b": "left", "x": 0.0, "y": 1.0, "mod": 10}));
        for (stroke, expected) in [
            (stroke("c", Some("ação 🎯"), false), json!({"t": "txt", "s": "ação 🎯"})),
            (stroke("enter", Some("\n"), false), json!({"t": "k", "key": "Enter", "mod": 0})),
            (stroke("a", Some("a"), true), json!({"t": "k", "key": "a", "mod": 2, "code": 65})),
        ] {
            browser.update(cx, |browser, _| { assert!(browser.send(key_message(&stroke).unwrap())); });
            assert_eq!(received(cx, &peer.input), expected);
        }
        assert!(key_message(&stroke("'", None, false)).is_none());
        assert!(peer.input.is_empty(), "a faixa vazia não envia outro press");
        browser.update(cx, |browser, cx| browser.set_shown(false, cx));
        received(cx, &peer.closed);
    }

    #[gpui::test]
    fn resize_sends_only_significant_sizes_and_eventually_the_last_pending_value(cx: &mut TestAppContext) {
        let server = RemoteServer::new();
        let browser = server.browser(cx);
        browser.update(cx, |browser, cx| {
            browser.bounds.set(Bounds { origin: point(px(0.), px(0.)), size: size(px(400.), px(300.)) });
            browser.set_shown(true, cx);
        });
        let peer = received(cx, &server.peers);
        assert_eq!((peer.width.as_deref(), peer.quality.as_deref()), (Some("400"), Some("80")));
        assert_eq!(received(cx, &peer.input), json!({"t": "layout", "w": 400, "h": 300}));
        for (w, h) in [(49., 300.), (400., 49.), (415., 315.)] {
            browser.update(cx, |browser, cx| {
                browser.bounds.set(Bounds { origin: point(px(0.), px(0.)), size: size(px(w), px(h)) });
                browser.set_shown(true, cx);
            });
        }
        cx.run_until_parked();
        assert!(!browser.read_with(cx, |b, _| b.resize_pending));
        for (w, h) in [(416., 300.), (640., 480.), (800., 600.)] {
            browser.update(cx, |browser, cx| {
                browser.bounds.set(Bounds { origin: point(px(0.), px(0.)), size: size(px(w), px(h)) });
                browser.set_shown(true, cx);
            });
        }
        cx.run_until_parked();
        assert!(browser.read_with(cx, |b, _| b.resize_pending));
        assert!(peer.input.is_empty(), "o resize aguarda o intervalo");
        std::thread::sleep(Duration::from_millis(510));
        cx.executor().advance_clock(Duration::from_millis(500));
        assert_eq!(received(cx, &peer.input), json!({"t": "layout", "w": 800, "h": 600}));
        assert!(peer.input.is_empty());
        browser.update(cx, |browser, cx| browser.set_shown(false, cx));
        received(cx, &peer.closed);
    }

    #[test]
    fn fraction_follows_the_centered_frame() {
        // Painel 1000×400 com página 1280×800: o quadro ocupa 640×400, centrado com 180 px de cada lado.
        let card = Bounds { origin: point(px(10.), px(20.)), size: size(px(1000.), px(400.)) };
        assert_eq!(fraction(card, (1280., 800.), point(px(190.), px(20.)), false), Some((0., 0.)));
        assert_eq!(fraction(card, (1280., 800.), point(px(510.), px(220.)), false), Some((0.5, 0.5)));
        assert_eq!(fraction(card, (1280., 800.), point(px(100.), px(220.)), false), None, "faixa vazia ao lado não é página");
        assert_eq!(fraction(card, (1280., 800.), point(px(100.), px(220.)), true), Some((0., 0.5)), "soltar fora cai na borda");
        assert_eq!(fraction(card, (0., 800.), point(px(510.), px(220.)), true), None);
    }

    fn stroke(key: &str, key_char: Option<&str>, control: bool) -> Keystroke {
        Keystroke { modifiers: Modifiers { control, ..Default::default() }, key: key.into(), key_char: key_char.map(Into::into) }
    }

    #[test]
    fn keys_become_nav_remoto_messages() {
        assert_eq!(key_message(&stroke("enter", Some("\n"), false)), Some(json!({"t": "k", "key": "Enter", "mod": 0})));
        assert_eq!(key_message(&stroke("c", Some("ç"), false)), Some(json!({"t": "txt", "s": "ç"})));
        assert_eq!(key_message(&stroke("a", Some("a"), true)), Some(json!({"t": "k", "key": "a", "mod": 2, "code": 65})));
        assert_eq!(key_message(&stroke("'", None, false)), None, "tecla morta compondo não vai");
    }
}
