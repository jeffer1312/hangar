//! Vídeo tocado dentro do app (`hangar_video`, sobre a libmpv): o quadro e os controles. Quem abre escolhe onde o
//! player mora (hoje o diálogo do visor) e passa o endereço já resolvido: caminho local ou a rota do servidor, que a
//! libmpv só alcança pela ponte local (`proxy`), a única que leva o token.
mod proxy;
mod surface;

pub use proxy::Upstream;

use std::{cell::{Cell, RefCell}, rc::Rc, time::{Duration, Instant}};
use gpui_kit::{component::{button::*, *}, *};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::assets::IconName;
use hangar_video::{Options, PixelFormat, Player, Snapshot, VideoError};
use crate::{i18n::tr, theme};

/// A ordem de bytes que a pintura de cada sistema lê (ver `surface`).
const FORMAT: PixelFormat = if cfg!(any(target_os = "linux", target_os = "windows")) { PixelFormat::Rgba } else { PixelFormat::Bgra };

/// Passo das setas e do volume pelo teclado.
const SEEK_STEP: f64 = 5.;
const VOLUME_STEP: f64 = 5.;

/// Quadros chegando sem a tela desenhar (janela escondida ou minimizada) por este tempo: o vídeo pausa em vez de
/// gastar a máquina tocando para ninguém.
const HIDDEN_PAUSE: Duration = Duration::from_secs(2);

/// Altura da faixa de controles embaixo do quadro.
pub const CONTROLS_H: f32 = 44.;

enum Phase { Opening, Ready, Failed(VideoError) }

/// O que o player abre.
pub enum VideoSource {
    /// Arquivo desta máquina, pelo caminho.
    Local(String),
    /// Rota de outra máquina: a libmpv pede à ponte local, sem credencial.
    Remote(Upstream),
}

/// O que o pincel do quadro usa: ele roda na pintura, fora do `render`.
struct Core {
    player: Option<Player>,
    surface: surface::Surface,
    target: (u32, u32),
    /// Desde quando há quadro pronto que a tela não desenhou.
    unpainted_since: Option<Instant>,
}

pub struct VideoPlayer {
    core: Rc<RefCell<Core>>,
    phase: Phase,
    state: Snapshot,
    focus: FocusHandle,
    /// Barra de posição medida no último quadro: o clique vira fração dela.
    bar: Rc<Cell<Option<Bounds<Pixels>>>>,
    scrubbing: bool,
    /// A tela cheia foi este player que ligou: fechar o vídeo a desliga.
    fullscreen_by_us: bool,
    /// Fechado pelo `release`: um `Player::open` que termine depois sai sem ser guardado.
    released: bool,
    /// A ponte do vídeo remoto; sai junto com o player.
    proxy: Option<proxy::Proxy>,
    _open: Task<()>,
    _watch: Task<()>,
}

impl VideoPlayer {
    /// `runtime`: onde a ponte do vídeo remoto atende.
    pub fn new(source: VideoSource, runtime: &tokio::runtime::Handle, cx: &mut Context<Self>) -> Self {
        // Um aviso guardado basta: a tela relê o estado inteiro quando acorda.
        let (tx, rx) = async_channel::bounded::<()>(1);
        let mut proxy = None;
        let address = match source {
            VideoSource::Local(path) => Ok(path),
            VideoSource::Remote(upstream) => proxy::start(runtime, upstream)
                .map(|started| { let address = started.address().to_owned(); proxy = Some(started); address })
                .map_err(|error| VideoError::Init(format!("local proxy: {error}"))),
        };
        let (phase, _open) = match address {
            Err(error) => {
                crate::log_line(&format!("video: {error}"));
                (Phase::Failed(error), Task::ready(()))
            }
            Ok(address) => {
                let options = Options { format: FORMAT };
                let open = cx.background_spawn(async move { Player::open(&address, &options, move |_| { let _ = tx.try_send(()); }) });
                (Phase::Opening, cx.spawn(async move |this, cx| {
                    let result = open.await;
                    let _ = this.update(cx, |this, cx| {
                        // Fechado enquanto abria: o player sai aqui, sem tocar para um diálogo que já não existe.
                        if this.released { return; }
                        match result {
                            Ok(player) => { this.core.borrow_mut().player = Some(player); this.phase = Phase::Ready; }
                            Err(error) => {
                                crate::log_line(&format!("video: {error}"));
                                this.phase = Phase::Failed(error);
                            }
                        }
                        cx.notify();
                    });
                }))
            }
        };
        let _watch = cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.changed(cx)).is_err() { break; }
            }
        });
        // Rede de segurança para um fechamento que não passe pelo `release`: a libmpv para junto com a entidade.
        cx.on_release(|this: &mut Self, cx| {
            let mut core = this.core.borrow_mut();
            core.player = None;
            core.surface.release(None, cx);
        }).detach();
        VideoPlayer {
            core: Rc::new(RefCell::new(Core { player: None, surface: surface::Surface::new(), target: (0, 0), unpainted_since: None })),
            phase, state: Snapshot::default(), focus: cx.focus_handle(), bar: Rc::new(Cell::new(None)),
            scrubbing: false, fullscreen_by_us: false, released: false, proxy, _open, _watch,
        }
    }

    /// O teclado do player (espaço, setas, m, f) precisa do foco nele ao abrir.
    pub fn focus(&self) -> FocusHandle { self.focus.clone() }

    /// Largura sobre altura do vídeo, quando ele já disse o tamanho.
    pub fn aspect(&self) -> Option<f32> { self.state.video_size.map(|(w, h)| w as f32 / h.max(1) as f32) }

    /// Fecha a libmpv e a ponte, solta a textura e desfaz a tela cheia que este player ligou.
    pub fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.released = true;
        self.proxy = None;
        {
            let mut core = self.core.borrow_mut();
            core.player = None;
            core.surface.release(Some(window), cx);
        }
        if std::mem::take(&mut self.fullscreen_by_us) && window.is_fullscreen() { window.toggle_fullscreen(); }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let mut core = self.core.borrow_mut();
        let Some((state, seq)) = core.player.as_ref().map(|player| (player.snapshot(), player.frame_seq())) else { return };
        if seq > core.surface.seq() {
            let since = *core.unpainted_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= HIDDEN_PAUSE && !state.paused && let Some(player) = core.player.as_ref() { player.set_paused(true); }
        } else {
            core.unpainted_since = None;
        }
        drop(core);
        // A libmpv mantém o erro no estado; o log recebe cada erro novo uma vez só.
        if let Some(error) = &state.error && self.state.error.as_ref() != Some(error) {
            crate::log_line(&format!("video: {error}"));
        }
        self.state = state;
        cx.notify();
    }

    fn with_player(&self, f: impl FnOnce(&Player)) {
        if let Some(player) = self.core.borrow().player.as_ref() { f(player); }
    }

    fn ready(&self) -> bool { matches!(self.phase, Phase::Ready) && self.state.loaded && self.state.error.is_none() }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if !self.ready() { return; }
        self.with_player(Player::toggle);
        cx.notify();
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = !self.state.muted;
        self.with_player(|player| player.set_muted(muted));
        cx.notify();
    }

    fn change_volume(&mut self, delta: f64) {
        let volume = self.state.volume + delta;
        self.with_player(|player| { player.set_volume(volume); if delta > 0. { player.set_muted(false); } });
    }

    fn toggle_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Só desfaz no fechamento a tela cheia que este player ligou; a que já estava fica.
        self.fullscreen_by_us = !window.is_fullscreen();
        window.toggle_fullscreen();
        cx.notify();
    }

    fn seek_at(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(bar), Some(duration)) = (self.bar.get(), self.state.duration) else { return };
        let fraction = (f32::from(at.x - bar.origin.x) / f32::from(bar.size.width).max(1.)).clamp(0., 1.) as f64;
        let seconds = duration * fraction;
        self.with_player(|player| player.seek_to(seconds));
        // A posição anda já, sem esperar a libmpv confirmar.
        self.state.position = Some(seconds);
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || !self.ready() { return; }
        match event.keystroke.key.as_str() {
            "space" | "k" => self.toggle(cx),
            "left" => self.with_player(|player| player.seek_by(-SEEK_STEP)),
            "right" => self.with_player(|player| player.seek_by(SEEK_STEP)),
            "up" => self.change_volume(VOLUME_STEP),
            "down" => self.change_volume(-VOLUME_STEP),
            "m" => self.toggle_mute(cx),
            "f" => self.toggle_fullscreen(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// O texto no lugar do quadro quando não há o que tocar.
    fn problem(&self) -> Option<String> {
        match &self.phase {
            Phase::Failed(VideoError::Unavailable(_)) =>
                Some(tr(if cfg!(target_os = "linux") { "video_need_mpv_linux" } else { "video_need_mpv" })),
            Phase::Failed(error) => Some(tr("video_failed").replace("{reason}", &error.to_string())),
            _ => self.state.error.as_ref().map(|error| tr("video_failed").replace("{reason}", error)),
        }
    }

    fn render_stage(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(problem) = self.problem() {
            return div().id("video-problem").size_full().flex().items_center().justify_center().px_6().role(Role::Alert)
                .text_sm().text_color(theme::warning()).whitespace_normal().text_center().child(problem).into_any_element();
        }
        let core = self.core.clone();
        let frame = canvas(|_, _, _| (), move |bounds, (), window, _| {
            let mut core = core.borrow_mut();
            let core = &mut *core;
            let Some(player) = core.player.as_ref() else { return };
            let scale = window.scale_factor();
            let target = ((f32::from(bounds.size.width) * scale) as u32, (f32::from(bounds.size.height) * scale) as u32);
            if core.target != target { core.target = target; player.set_target(target.0, target.1); }
            core.surface.paint(player, bounds, window);
            if player.frame_seq() <= core.surface.seq() { core.unpainted_since = None; }
        }).absolute().size_full();
        let waiting = (!self.state.loaded).then(|| tr("video_loading")).or_else(|| self.state.buffering.then(|| tr("video_buffering")));
        div().id("video-stage").relative().size_full().cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
            .child(frame)
            .when_some(waiting, |el, text| el.child(div().id("video-waiting").absolute().inset_0().flex().items_center().justify_center().role(Role::Status)
                .child(div().px_3().py_1().rounded_md().bg(theme::scrim()).text_sm().text_color(theme::muted()).child(text))))
            .into_any_element()
    }

    fn render_controls(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.state;
        let ready = self.ready();
        let playing = ready && !state.paused && !state.ended;
        let play_label = tr(if playing { "video_pause" } else { "video_play" });
        let mute_label = tr(if state.muted { "video_unmute" } else { "video_mute" });
        let full = window.is_fullscreen();
        let full_label = tr(if full { "video_exit_fullscreen" } else { "video_fullscreen" });
        let position = state.position.unwrap_or(0.);
        let fraction = state.duration.map_or(0., |d| (position / d).clamp(0., 1.)) as f32;
        let cached = state.duration.zip(state.cached_until).map_or(fraction, |(d, c)| (c.max(position) / d).clamp(0., 1.) as f32);
        let bar_bounds = self.bar.clone();
        let line = |width: f32, color: Hsla| div().absolute().left_0().top(px(6.)).h(px(4.)).w(relative(width)).rounded_full().bg(color);
        let bar = div().id("video-seek").flex_1().min_w(px(80.)).h(px(16.)).relative().when(ready, |el| el.cursor_pointer())
            .role(Role::Slider).aria_label(tr("video_seek")).aria_numeric_value(position)
            .aria_min_numeric_value(0.).aria_max_numeric_value(state.duration.unwrap_or(0.))
            .child(canvas(move |bounds, _, _| bar_bounds.set(Some(bounds)), |_, _, _, _| {}).absolute().size_full())
            .child(line(1., theme::border_strong()))
            .child(line(cached, theme::faint()))
            .child(line(fraction, theme::accent()))
            .child(div().absolute().top(px(2.)).left(relative(fraction)).ml(px(-6.)).size(px(12.)).rounded_full().bg(theme::text()))
            .when(ready, |el| el
                .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| { this.scrubbing = true; this.seek_at(event.position, cx); }))
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    if !this.scrubbing { return; }
                    if event.pressed_button != Some(MouseButton::Left) { this.scrubbing = false; return; }
                    this.seek_at(event.position, cx);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.scrubbing = false)));
        let time = |seconds: Option<f64>| div().flex_shrink_0().font_family(theme::MONO).text_xs().text_color(theme::muted())
            .child(hangar_video::clock(if ready { seconds } else { None }));
        div().h(px(CONTROLS_H)).flex_shrink_0().flex().items_center().gap_2().px_3().rounded_lg()
            .border_1().border_color(theme::border()).bg(theme::inset())
            .child(Button::new("video-play").ghost().small().icon(if playing { IconName::Pause } else { IconName::Play })
                .tooltip(play_label.clone()).accessibility_label(play_label).disabled(!ready)
                .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))))
            .child(time(Some(position)))
            .child(bar)
            .child(time(state.duration))
            .child(Button::new("video-mute").ghost().small().icon(if state.muted { IconName::VolumeX } else { IconName::Volume2 })
                .tooltip(mute_label.clone()).accessibility_label(mute_label).disabled(!ready)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))))
            .child(Button::new("video-fullscreen").ghost().small().icon(if full { IconName::Minimize } else { IconName::Maximize })
                .tooltip(full_label.clone()).accessibility_label(full_label)
                .on_click(cx.listener(|this, _, window, cx| this.toggle_fullscreen(window, cx))))
            .into_any_element()
    }
}

impl Render for VideoPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stage = self.render_stage(cx);
        let controls = self.render_controls(window, cx);
        div().id("video-player").track_focus(&self.focus).key_context("VideoPlayer").on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)))
            .size_full().flex().flex_col().gap_2()
            // Fundo preto: o quadro encaixado deixa sobra nos lados, como em qualquer player.
            .child(div().flex_1().min_h_0().rounded_md().overflow_hidden().bg(gpui_kit::black()).child(stage))
            .child(controls)
    }
}
