//! Superfície flutuante presa ao gatilho: a posição vem do `Positioner` do gpui-base (vira de lado quando falta
//! espaço), uma cortina por baixo engole o clique fora e fecha, e entrada e saída são o `menu-in`/`menu-out` do kit de
//! movimento. Mais as peças das linhas dos menus, iguais em todo popover.
use super::*;
use std::cell::RefCell;
use gpui_kit::base::{Align, Placement, Positioner};
use gpui_kit::component::dialog::Dialog;

thread_local! {
    // ponytail: uma janela só; com várias, o id do gatilho precisaria levar a janela.
    static ANCHORS: RefCell<HashMap<SharedString, Bounds<Pixels>>> = RefCell::default();
}

/// Guarda onde `el` foi desenhado, para o painel preso a `id` abrir ali. Vale do quadro seguinte em diante.
pub(super) fn anchor<E: ParentElement>(el: E, id: impl Into<SharedString>) -> E {
    let id = id.into();
    el.child(canvas(move |bounds, window, _| {
        // A raiz lê a posição antes deste prepaint: mudou, desenha mais um quadro para o painel acompanhar.
        let moved = ANCHORS.with(|a| a.borrow_mut().insert(id, bounds) != Some(bounds));
        if moved { window.on_next_frame(|window, _| window.refresh()); }
    }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
}

pub(super) fn anchor_bounds(id: &str) -> Option<Bounds<Pixels>> { ANCHORS.with(|a| a.borrow().get(id).copied()) }

/// Ciclo do painel: o estado do app diz se está aberto; aqui fica a última cópia desenhada, para a saída animar o
/// mesmo conteúdo depois que o app já fechou. `from` é quanto dele aparecia (0 a 1) quando a direção virou em `since`:
/// a entrada anda no tempo do `menu-in` e a saída, mais curta, no do `menu-out`.
struct Presence<T> { shown: Option<T>, since: Instant, from: f32, leaving: bool }

impl<T> Default for Presence<T> {
    fn default() -> Self { Self { shown: None, since: Instant::now(), from: 0., leaving: false } }
}

impl<T: Clone> Presence<T> {
    /// Quanto aparece agora, sem curva.
    fn visible(&self) -> f32 {
        let (spec, sign) = if self.leaving { (motion::MENU_OUT, -1.) } else { (motion::MENU_IN, 1.) };
        (self.from + sign * self.since.elapsed().as_secs_f32() / spec.total().as_secs_f32()).clamp(0., 1.)
    }

    /// Vira a direção partindo de onde está: reaberto no meio da saída, não pisca.
    fn turn(&mut self, leaving: bool) {
        self.from = if self.shown.is_some() { self.visible() } else { 0. };
        (self.since, self.leaving) = (Instant::now(), leaving);
    }

    /// O que desenhar neste quadro, quanto dele aparece (0 a 1) e se está saindo.
    fn frame(&mut self, live: Option<T>, still: bool) -> Option<(T, f32, bool)> {
        match live {
            Some(value) => {
                if self.shown.is_none() || self.leaving { self.turn(false); }
                self.shown = Some(value.clone());
                Some((value, if still { 1. } else { self.visible() }, false))
            }
            None => {
                if !self.leaving && self.shown.is_some() { self.turn(true); }
                let left = self.visible();
                if still || left <= 0. { self.shown = None; self.leaving = false; return None; }
                Some((self.shown.clone()?, left, true))
            }
        }
    }
}

/// Painel do compositor e a cópia do que ele mostra.
#[derive(Clone)]
enum Floating { Controls(super::controls::Open), Commands, Recent(Recent), NewChat(super::create::Menu), Usage, Context, Hangar, Voice }

impl Hangar {
    fn floating(&self) -> Option<Floating> {
        // Sem o compositor na tela, o gatilho não foi desenhado e o painel não tem onde se prender.
        let page = self.settings.is_some() && !self.settings_ui.live;
        // O cartão aberto pela pílula da barra do topo existe em qualquer tela, com ou sem sessão.
        if self.accounts.card && self.accounts.card_top { return Some(Floating::Usage); }
        // A lista do chip "N no Hangar" também vale em qualquer tela: o chip mora na barra de sessões.
        if self.hangar_open { return Some(Floating::Hangar); }
        // O painel da voz também: a pílula mora na barra do topo. Sem a pílula, não há onde prendê-lo.
        if self.voice.open && (self.voice.enabled || self.voice.call.is_some()) { return Some(Floating::Voice); }
        if let Some(menu) = self.new_chat_folders.get().filter(|_| !page && self.selected.is_none() && self.api.is_some()) {
            return Some(Floating::NewChat(menu));
        }
        if page || !self.selected.as_ref().is_some_and(|s| s.readable()) { return None; }
        if let Some(open) = self.ctl_snapshot() { return Some(Floating::Controls(open)); }
        if self.command_panel { return Some(Floating::Commands); }
        if self.accounts.card { return Some(Floating::Usage); }
        if self.context_card { return Some(Floating::Context); }
        self.recent.clone().map(Floating::Recent)
    }

    pub(super) fn popup_open(&self) -> bool { self.floating().is_some() }

    /// Fecha o painel aberto sobre o compositor; diz se havia um.
    pub(super) fn close_popups(&mut self) -> bool {
        let folders = self.new_chat_folders.replace(None).is_some();
        let open = folders || self.controls_open() || self.command_panel || self.recent.is_some() || self.accounts.card || self.context_card
            || self.hangar_open || self.voice.open;
        self.close_controls();
        self.hangar_open = false;
        self.voice.open = false;
        self.command_panel = false;
        self.accounts.card = false;
        self.context_card = false;
        self.close_recent();
        open
    }

    /// Camada da raiz com o painel do compositor preso ao gatilho dele.
    pub(super) fn render_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let live = self.floating();
        let still = cx.reduce_motion();
        let presence = window.use_keyed_state("composer-popup", cx, |_, _| Presence::<Floating>::default());
        let (shown, visible, leaving) = presence.update(cx, |presence, _| presence.frame(live, still))?;
        if visible < 1. { motion::request_frame(window, cx); }
        let mut placement = Placement::Bottom;
        let (anchor, align, narrow, content) = match shown {
            Floating::Controls(open) => (open.anchor(), Align::End, true, self.render_ctl_panel_for(open, window, cx)),
            Floating::Commands => ("composer".to_owned(), Align::Start, false, Some(self.render_command_panel(cx))),
            Floating::Usage => ((if self.accounts.card_top { "topbar-account" } else { "composer-account" }).to_owned(), Align::End, true,
                Some(self.render_usage_card(window, cx))),
            Floating::Context => ("composer-ctx".to_owned(), Align::End, true, Some(self.render_context_card())),
            Floating::Hangar => ("hangar-chip".to_owned(), Align::Start, true, Some(self.render_hangar_popover(window, cx))),
            Floating::Voice => ("topbar-voice".to_owned(), Align::End, true, Some(self.render_voice_panel(window, cx))),
            Floating::Recent(recent) => {
                let live = self.recent.replace(recent);
                let content = self.render_recent(cx);
                self.recent = live;
                ("attach-recent".to_owned(), Align::Start, true, content)
            }
            Floating::NewChat(menu) => {
                let viewport = window.viewport_size().height;
                let (above, below) = anchor_bounds(menu.anchor()).map(|t| (t.top(), viewport - t.bottom())).unwrap_or_default();
                // Acima só com espaço para a lista de pastas; janela baixa abre para baixo, por cima do compositor.
                // Os de baixo do compositor sobem quando falta altura embaixo.
                let up = if menu.above() { above >= px(360.) || above >= below } else { below < px(360.) && above > below };
                let room = if up { above } else { below } - px(16.);
                if up { placement = Placement::Top; }
                (menu.anchor().to_owned(), if menu.above() { Align::End } else { Align::Start }, true,
                    self.new_chat.clone().map(|view| view.update(cx, |view, cx| view.render_menu(menu, room, cx))))
            }
        };
        let trigger = anchor_bounds(&anchor)?;
        let surface = chrome::popover(content?, narrow);
        let surface = if narrow { surface } else { div().w(trigger.size.width).child(surface).into_any_element() };
        let dismiss = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.close_popups();
            cx.stop_propagation();
            cx.notify();
        });
        let close = cx.listener(|this, _: &(), _, cx| {
            this.close_popups();
            cx.notify();
        });
        Some(layer(trigger, placement, align, surface, visible, leaving, dismiss, close))
    }
}

/// Cortina que fecha no clique fora sem deixar o clique chegar ao que está atrás, e a superfície presa ao gatilho,
/// abaixo dele ou virada para cima quando não cabe. Saindo, nada ali responde ao ponteiro.
fn layer(trigger: Bounds<Pixels>, placement: Placement, align: Align, surface_content: AnyElement, visible: f32, leaving: bool,
    dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static, close: impl Fn(&(), &mut Window, &mut App) + 'static) -> AnyElement {
    let surface = if leaving { motion::menu_out(div(), motion::MENU_OUT.ease(1. - visible)) }
        else { motion::menu_in(div(), motion::MENU_IN.ease(visible)) };
    let surface = surface.child(surface_content).when(leaving, |el| el.child(div().absolute().inset_0().occlude()));
    let placed = Positioner::side(trigger).placement(placement).align(align).offset(px(8.)).margin(px(8.));
    div().absolute().inset_0()
        // Para a acessibilidade a cortina é o "Fechar" do painel: sem tecla, é o único jeito de um agente o fechar.
        .when(!leaving, |el| el.child(div().id("popup-scrim").absolute().inset_0().occlude().on_any_mouse_down(dismiss)
            .role(Role::Button).aria_label(tr("close"))
            .on_a11y_action(AccessibleAction::Click, move |_, window, cx| close(&(), window, cx))))
        .child(if leaving { placed } else { placed.occlude() }.child(surface))
        .into_any_element()
}

/// Superfície de todo menu e lista suspensa do kit, igual à do `chrome::popover` (raio 12, borda de vidro, sombra
/// `--elev-2`), com o desfoque atrás no Vidro. Lê a aparência a cada desenho: instala uma vez só.
/// O desfoque vai na camada que o kit abre para a superfície: numa camada própria, de ordem maior, ele rodaria depois
/// do conteúdo do menu e apagaria separadores e ícones.
pub(super) fn install_kit_surface(cx: &mut App) {
    cx.set_global(gpui_kit::component::popover::PopupSurface::new(|bounds, window, _| {
        let radius = px(12.);
        if appearance::get().surface_material == appearance::SurfaceMaterial::Glass {
            window.paint_backdrop_blur(bounds, Corners::all(radius), chrome::GLASS_BLUR);
        }
        window.paint_drop_shadows(bounds, Corners::all(radius), &theme::popover_shadow());
        window.paint_quad(quad(bounds, radius, theme::popup_fill(theme::raised()), px(1.), theme::glass_border(), BorderStyle::Solid));
    }));
}

/// O mesmo cartão para confirmações e formulários; o kit continua cuidando do foco, da altura e da rolagem.
pub(super) fn dialog(surface: Dialog) -> Dialog {
    let surface = surface.p(px(20.)).rounded(px(16.)).bg(theme::popup_fill(theme::raised()))
        .border_color(theme::glass_border());
    if appearance::get().surface_material == appearance::SurfaceMaterial::Glass {
        surface.bg(transparent_black()).border_color(transparent_black()).background_painter(|bounds, window, _| {
            chrome::paint_glass(bounds, px(16.), window);
            window.paint_quad(quad(bounds, px(16.), theme::popup_fill(theme::raised()), px(1.), theme::glass_border(), BorderStyle::Solid));
        })
    } else { surface }
}

/// Título de seção do popover, em caixa alta miúda; a tecla, quando há, fica à direita.
pub(super) fn title(text: String, key: Option<&'static str>) -> Div {
    div().px(px(8.)).pt(px(6.)).pb(px(4.)).flex().items_center().gap_2()
        .child(div().flex_1().min_w_0().truncate().text_size(px(10.)).font_weight(FontWeight::MEDIUM).text_color(theme::faint())
            .child(text.to_uppercase()))
        .when_some(key, |el, key| el.child(key_hint(key)))
}

/// Recuo do conteúdo dentro do cartão (raio 12, borda 1): a linha de raio 7 fica concêntrica a ele.
pub(super) const INSET: f32 = 4.;

/// Filete de borda a borda entre seções; o recuo negativo desfaz o do cartão.
pub(super) fn separator() -> Div { div().h(px(1.)).mx(px(-INSET)).my(px(2.)).bg(theme::border()) }

/// Tecla de atalho dentro de uma linha do popover.
pub(super) fn key_hint(keys: &'static str) -> Div {
    div().flex_none().px(px(5.)).py(px(1.)).rounded(px(5.)).bg(theme::inset())
        .font_family(theme::MONO).text_size(px(10.)).text_color(theme::faint()).child(keys)
}

/// Linha clicável do popover: raio concêntrico ao do cartão, realce igual em todos os menus.
pub(super) fn row(id: impl Into<ElementId>, selected: bool) -> Button {
    Button::new(id).ghost().w_full().h_auto().px(px(8.)).py(px(6.)).rounded(px(7.)).text_size(px(13.))
        .when(selected, |el| el.bg(theme::accent_dim())).aria_selected(selected)
}

/// Topo de um diálogo curto no meio da janela, pela altura típica dele. O kit o põe a um décimo do topo; em janela baixa
/// esse décimo continua sendo o piso.
pub(super) fn centered_top(viewport: Pixels, height: Pixels) -> Pixels {
    ((viewport - height) / 2.).max(viewport / 10.)
}

/// Linhas vazias no lugar da lista enquanto ela carrega.
pub(super) fn skeleton(id: &str, rows: usize) -> Div {
    div().flex().flex_col().gap(px(6.)).px(px(8.)).py(px(4.))
        .children((0..rows).map(|n| chrome::Skeleton::new(SharedString::from(format!("{id}-{n}"))).row(n).h(px(22.)).rounded(px(7.))))
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{Presence, centered_top, px};
    use crate::motion;

    #[test]
    fn short_dialog_sits_in_the_middle_with_a_tenth_as_the_floor() {
        assert_eq!(centered_top(px(800.), px(150.)), px(325.));
        assert_eq!(centered_top(px(200.), px(170.)), px(20.));
    }

    #[test]
    fn closing_keeps_the_last_copy_until_the_fade_ends() {
        let mut presence = Presence::default();
        assert_eq!(presence.frame(Some(1), false).map(|(v, _, leaving)| (v, leaving)), Some((1, false)));
        presence.since -= motion::MENU_IN.total();
        assert_eq!(presence.frame(None, false).map(|(v, _, leaving)| (v, leaving)), Some((1, true)));
        presence.since -= motion::MENU_OUT.total();
        assert_eq!(presence.frame(None, false), None);
    }

    #[test]
    fn reduced_motion_opens_whole_and_closes_at_once() {
        let mut presence = Presence::default();
        assert_eq!(presence.frame(Some(1), true), Some((1, 1., false)));
        assert_eq!(presence.frame(None, true), None);
    }
}
