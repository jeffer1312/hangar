//! Superfície flutuante presa ao gatilho: a posição vem do `Positioner` do gpui-base (vira de lado quando falta
//! espaço), uma cortina por baixo engole o clique fora e fecha, e entrada e saída duram `FADE`. Mais as peças das
//! linhas dos menus, iguais em todo popover.
use super::*;
use std::cell::RefCell;
use gpui_kit::base::{Align, Placement, Positioner};

const FADE: Duration = Duration::from_millis(140);

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

fn anchor_bounds(id: &str) -> Option<Bounds<Pixels>> { ANCHORS.with(|a| a.borrow().get(id).copied()) }

/// Ciclo do painel: o estado do app diz se está aberto; aqui fica a última cópia desenhada, para a saída animar o
/// mesmo conteúdo depois que o app já fechou.
struct Presence<T> { shown: Option<T>, since: Instant, leaving: bool }

impl<T> Default for Presence<T> {
    fn default() -> Self { Self { shown: None, since: Instant::now(), leaving: false } }
}

impl<T: Clone> Presence<T> {
    /// O que desenhar neste quadro, quanto dele aparece (0 a 1) e se está saindo.
    fn frame(&mut self, live: Option<T>, still: bool) -> Option<(T, f32, bool)> {
        let progress = |since: Instant| if still { 1. } else { (since.elapsed().as_secs_f32() / FADE.as_secs_f32()).min(1.) };
        match live {
            Some(value) => {
                if self.shown.is_none() || self.leaving {
                    // Reaberto no meio da saída, volta da opacidade em que estava, sem piscar.
                    let from = if self.shown.is_some() { 1. - progress(self.since) } else { 0. };
                    self.since = Instant::now() - FADE.mul_f32(from);
                    self.leaving = false;
                }
                self.shown = Some(value.clone());
                Some((value, progress(self.since), false))
            }
            None => {
                if !self.leaving && self.shown.is_some() {
                    let from = 1. - progress(self.since);
                    self.since = Instant::now() - FADE.mul_f32(from);
                    self.leaving = true;
                }
                let left = 1. - progress(self.since);
                if still || left <= 0. { self.shown = None; self.leaving = false; return None; }
                Some((self.shown.clone()?, left, true))
            }
        }
    }
}

/// Painel do compositor e a cópia do que ele mostra.
#[derive(Clone)]
enum Floating { Controls(super::controls::Open), Commands, Recent(Recent) }

impl Hangar {
    fn floating(&self) -> Option<Floating> {
        // Sem o compositor na tela, o gatilho não foi desenhado e o painel não tem onde se prender.
        let page = self.settings.is_some() && !self.settings_ui.live;
        if page || !self.selected.as_ref().is_some_and(|s| s.readable()) { return None; }
        if let Some(open) = self.ctl_snapshot() { return Some(Floating::Controls(open)); }
        if self.command_panel { return Some(Floating::Commands); }
        self.recent.clone().map(Floating::Recent)
    }

    /// Fecha o painel aberto sobre o compositor; diz se havia um.
    pub(super) fn close_popups(&mut self) -> bool {
        let open = self.controls_open() || self.command_panel || self.recent.is_some();
        self.close_controls();
        self.command_panel = false;
        self.recent = None;
        open
    }

    /// Camada da raiz com o painel do compositor preso ao gatilho dele.
    pub(super) fn render_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let live = self.floating();
        let still = cx.reduce_motion();
        let presence = window.use_keyed_state("composer-popup", cx, |_, _| Presence::<Floating>::default());
        let (shown, visible, leaving) = presence.update(cx, |presence, _| presence.frame(live, still))?;
        if visible < 1. { window.request_animation_frame(); }
        let (anchor, align, narrow, content) = match shown {
            Floating::Controls(open) => (open.anchor(), Align::End, true, self.render_ctl_panel_for(open, window, cx)),
            Floating::Commands => ("composer".to_owned(), Align::Start, false, Some(self.render_command_panel(cx))),
            Floating::Recent(recent) => {
                let live = self.recent.replace(recent);
                let content = self.render_recent(cx);
                self.recent = live;
                ("attach-recent".to_owned(), Align::Start, true, content)
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
        Some(layer(trigger, align, surface, visible, leaving, dismiss))
    }
}

/// Cortina que fecha no clique fora sem deixar o clique chegar ao que está atrás, e a superfície presa ao gatilho,
/// abaixo dele ou virada para cima quando não cabe. Saindo, nada ali responde ao ponteiro.
fn layer(trigger: Bounds<Pixels>, align: Align, surface: AnyElement, visible: f32, leaving: bool,
    dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> AnyElement {
    let surface = div().relative().opacity(chrome::ease_out(visible)).child(surface)
        .when(leaving, |el| el.child(div().absolute().inset_0().occlude()));
    let placed = Positioner::side(trigger).placement(Placement::Bottom).align(align).offset(px(8.)).margin(px(8.));
    div().absolute().inset_0()
        .when(!leaving, |el| el.child(div().id("popup-scrim").absolute().inset_0().occlude().on_any_mouse_down(dismiss)))
        .child(if leaving { placed } else { placed.occlude() }.child(surface))
        .into_any_element()
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
        .when(selected, |el| el.bg(theme::accent_dim()))
}

/// Linhas vazias no lugar da lista enquanto ela carrega.
pub(super) fn skeleton(id: &str, rows: usize) -> Div {
    div().flex().flex_col().gap(px(6.)).px(px(8.)).py(px(4.))
        .children((0..rows).map(|n| chrome::Skeleton::new(SharedString::from(format!("{id}-{n}"))).h(px(22.)).rounded(px(7.))))
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{FADE, Presence};

    #[test]
    fn closing_keeps_the_last_copy_until_the_fade_ends() {
        let mut presence = Presence::default();
        assert_eq!(presence.frame(Some(1), false).map(|(v, _, leaving)| (v, leaving)), Some((1, false)));
        assert_eq!(presence.frame(None, false).map(|(v, _, leaving)| (v, leaving)), Some((1, true)));
        presence.since -= FADE;
        assert_eq!(presence.frame(None, false), None);
    }

    #[test]
    fn reduced_motion_opens_whole_and_closes_at_once() {
        let mut presence = Presence::default();
        assert_eq!(presence.frame(Some(1), true), Some((1, 1., false)));
        assert_eq!(presence.frame(None, true), None);
    }
}
