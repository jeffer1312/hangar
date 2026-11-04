//! Superfície flutuante presa ao gatilho: a posição vem do `Positioner` do gpui-base (vira de lado quando falta
//! espaço), uma cortina por baixo engole o clique fora e fecha, e entrada e saída são o `menu-in`/`menu-out` do kit de
//! movimento. Mais as peças das linhas dos menus, iguais em todo popover.
use super::*;
use std::cell::RefCell;
use gpui_kit::base::{Align, Placement, Positioner};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::{Element, ElementId, GlobalElementId, InspectorElementId, LayoutId};
use super::panes::Area;

thread_local! {
    // ponytail: uma janela só; com várias, o id do gatilho precisaria levar a janela.
    static ANCHORS: RefCell<Anchors> = RefCell::default();
}

/// Quem desenhou a âncora: uma área guardada (`panes`) ou, com `None`, a raiz (diálogos, camadas e o que vai em
/// `deferred`, pintado fora do desenho da área).
type Scope = Option<Area>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Mark { bounds: Bounds<Pixels>, scope: Scope, generation: u64 }

/// Onde cada âncora foi desenhada e em que desenho de quem a desenhou. A área guardada reusada não regrava as suas: a
/// posição vale enquanto a área não redesenhar e seguir na tela; redesenhada sem a peça, a peça saiu.
#[derive(Default)]
struct Anchors {
    marks: HashMap<SharedString, Mark>,
    /// Sobe a cada desenho da área (`Pane::render`) e, para a raiz, a cada quadro, no fim do desenho dela.
    generations: HashMap<Scope, u64>,
    /// O quadro da raiz em que cada área esteve na tela pela última vez, redesenhada ou reusada.
    shown: HashMap<Area, u64>,
    /// Áreas que voltaram à tela neste quadro depois de ficar fora.
    returned: HashSet<Area>,
    drawing: Scope,
    /// Lidas desde o começo do quadro: só quem tem leitor pede outro quadro ao mudar, senão rolar a lista de sessões
    /// redesenharia a janela inteira.
    read: HashSet<SharedString>,
}

impl Anchors {
    fn generation(&self, scope: Scope) -> u64 { self.generations.get(&scope).copied().unwrap_or(0) }

    fn advance(&mut self, scope: Scope) { *self.generations.entry(scope).or_default() += 1; }

    fn show(&mut self, area: Area) {
        let frame = self.generation(None);
        if self.shown.insert(area, frame).is_some_and(|last| last + 1 < frame) { self.returned.insert(area); }
        else { self.returned.remove(&area); }
    }

    /// Grava a posição desenhada; diz se um leitor dela precisa de outro quadro (mudou de lugar ou voltou à tela).
    /// Voltar é quem a desenhou ter desenhado sem ela, ou a área ter saído da tela: área reusada do cache por muitos
    /// quadros não conta, senão cada redesenho dela com leitor aberto redesenharia a janela inteira.
    fn record(&mut self, id: SharedString, bounds: Bounds<Pixels>, scope: Scope, generation: u64) -> bool {
        let back = scope.is_some_and(|area| self.returned.contains(&area));
        let changed = self.marks.insert(id.clone(), Mark { bounds, scope, generation })
            .is_none_or(|prev| prev.bounds != bounds || prev.scope != scope || prev.generation + 1 < generation || back);
        changed && self.read.contains(&id)
    }

    /// Tira as marcas que quem as desenhou já redesenhou sem elas; as de área reusada do cache continuam valendo.
    fn prune(&mut self) {
        let generations = &self.generations;
        self.marks.retain(|_, mark| mark.generation >= generations.get(&mark.scope).copied().unwrap_or(0));
    }

    fn valid(&self, id: &str) -> Option<Bounds<Pixels>> {
        let mark = self.marks.get(id)?;
        let on_screen = mark.scope.is_none_or(|area| self.shown.get(&area) == Some(&self.generation(None)));
        (on_screen && mark.generation == self.generation(mark.scope)).then_some(mark.bounds)
    }
}

/// Guarda onde `el` foi desenhado, para o painel (ou o balão do tour) preso a `id` abrir ali. Vale do quadro seguinte
/// em diante, enquanto quem o desenhou não redesenhar sem ele.
pub(crate) fn anchor<E: ParentElement>(el: E, id: impl Into<SharedString>) -> E {
    let id = id.into();
    el.child(canvas(move |bounds, window, _| {
        // A raiz lê a posição antes deste prepaint: mudou, desenha mais um quadro para o painel acompanhar.
        let moved = ANCHORS.with(|a| {
            let mut a = a.borrow_mut();
            let scope = a.drawing;
            let generation = a.generation(scope);
            a.record(id, bounds, scope, generation)
        });
        if moved { window.on_next_frame(|window, _| window.refresh()); }
    }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
}

/// A posição do quadro anterior; `None` se a peça não está na tela.
pub(crate) fn anchor_bounds(id: &str) -> Option<Bounds<Pixels>> {
    ANCHORS.with(|a| {
        let mut a = a.borrow_mut();
        if !a.read.contains(id) { a.read.insert(SharedString::from(id.to_owned())); }
        a.valid(id)
    })
}

/// A mesma posição, sem virar leitor: para varrer candidatas (as linhas da lista) sem que mexer em cada uma redesenhe
/// a janela. A escolhida passa depois por `anchor_bounds`.
pub(crate) fn anchor_peek(id: &str) -> Option<Bounds<Pixels>> { ANCHORS.with(|a| a.borrow().valid(id)) }

/// Um quadro da raiz: as leituras do quadro anterior saem na entrada; a geração da raiz sobe na saída, depois de a
/// raiz ler as posições do quadro anterior e antes de as âncoras dela gravarem as deste.
pub(super) struct RootFrame(());

/// De quantos em quantos quadros da raiz as marcas velhas saem.
const PRUNE_EVERY: u64 = 600;

pub(super) fn root_frame() -> RootFrame {
    ANCHORS.with(|a| a.borrow_mut().read.clear());
    RootFrame(())
}

impl Drop for RootFrame {
    fn drop(&mut self) {
        ANCHORS.with(|a| {
            let mut a = a.borrow_mut();
            a.advance(None);
            // Linhas de sessão fechada e peças que sumiram não ficam para sempre no mapa.
            if a.generation(None) % PRUNE_EVERY == 0 { a.prune(); }
        });
    }
}

/// A área acabou de redesenhar: o que ela gravou antes deixa de valer até ela gravar de novo.
pub(super) fn area_redrawn(area: Area) { ANCHORS.with(|a| a.borrow_mut().advance(Some(area))); }

/// Marca a área como na tela neste quadro; fica fora da view guardada, para valer também quando ela é reusada.
pub(super) fn area_on_screen(area: Area) -> impl IntoElement {
    canvas(move |_, _, _| ANCHORS.with(|a| a.borrow_mut().show(area)), |_, _, _, _| {}).absolute().size_0()
}

/// O desenho da área: as âncoras gravadas dentro dele (views filhas e itens de lista incluídos) levam a área.
pub(super) fn area_scope(area: Area, child: AnyElement) -> AnyElement { AreaScope { area, child }.into_any_element() }

struct AreaScope { area: Area, child: AnyElement }

impl IntoElement for AreaScope {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for AreaScope {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App)
        -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (),
        window: &mut Window, cx: &mut App) {
        let outer = ANCHORS.with(|a| std::mem::replace(&mut a.borrow_mut().drawing, Some(self.area)));
        self.child.prepaint(window, cx);
        ANCHORS.with(|a| a.borrow_mut().drawing = outer);
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (),
        window: &mut Window, cx: &mut App) {
        self.child.paint(window, cx);
    }
}

/// Ciclo do painel: o estado do app diz se está aberto; aqui fica a última cópia desenhada, para a saída animar o
/// mesmo conteúdo depois que o app já fechou. `from` é quanto dele aparecia (0 a 1) quando a direção virou em `since`:
/// a entrada anda no tempo do `menu-in` e a saída, mais curta, no do `menu-out`.
struct Presence<T> { shown: Option<T>, since: Instant, from: f32, leaving: bool, enter: motion::Spec }

impl<T> Default for Presence<T> {
    fn default() -> Self { Self { shown: None, since: Instant::now(), from: 0., leaving: false, enter: motion::MENU_IN } }
}

impl<T: Clone> Presence<T> {
    /// Quanto aparece agora, sem curva.
    fn visible(&self) -> f32 {
        let (spec, sign) = if self.leaving { (motion::MENU_OUT, -1.) } else { (self.enter, 1.) };
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
enum Floating { Controls(super::controls::Open), Commands, Recent(Recent), NewChat(super::create::Menu), Usage, Context, Hangar, Voice, Machines, Artifacts, WorktreeCleanup }

impl Hangar {
    fn floating(&self) -> Option<Floating> {
        if self.worktrees.cleanup_open { return Some(Floating::WorktreeCleanup); }
        // Sem o compositor na tela, o gatilho não foi desenhado e o painel não tem onde se prender.
        let page = self.settings.is_some() && !self.settings_ui.live;
        // O cartão aberto pela pílula da barra do topo existe em qualquer tela, com ou sem sessão.
        if self.accounts.card && self.accounts.card_top { return Some(Floating::Usage); }
        // A lista do chip "N no Hangar" também vale em qualquer tela: o chip mora na barra de sessões.
        if self.hangar_open { return Some(Floating::Hangar); }
        if self.machines_tray.open && appearance::get().navigation.tabs() && !page && self.costs.view.is_none()
            && self.worktrees.view.is_none() && !self.files_expanded() { return Some(Floating::Machines); }
        // O painel da voz também: a pílula mora na barra do topo. Sem a pílula, não há onde prendê-lo.
        if self.voice.open && (self.voice.enabled || self.voice.call.is_some()) { return Some(Floating::Voice); }
        if let Some(menu) = self.new_chat_folders.get().filter(|_| !page && self.selected.is_none() && self.api.is_some()) {
            return Some(Floating::NewChat(menu));
        }
        if page || !self.selected.as_ref().is_some_and(|s| s.readable()) { return None; }
        if self.artifacts.open && !self.files_visible() { return Some(Floating::Artifacts); }
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
            || self.hangar_open || self.voice.open || self.machines_tray.open || self.artifacts.open || self.worktrees.cleanup_open;
        self.worktrees.cleanup_open = false;
        self.close_controls();
        self.hangar_open = false;
        self.machines_tray.open = false;
        self.artifacts.open = false;
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
        // Só quem abre escolhe o ritmo: trocar no fechamento faria a saída partir de outro ponto.
        let enter = live.as_ref().map(|f| if matches!(f, Floating::Usage) { motion::CARD_OPEN } else { motion::MENU_IN });
        let (shown, visible, leaving) = presence.update(cx, |presence, _| {
            if let Some(enter) = enter { presence.enter = enter; }
            presence.frame(live, still)
        })?;
        let card = matches!(shown, Floating::Usage);
        // As barras do cartão sobem depois que ele já apareceu: os quadros seguem até elas chegarem.
        let filling = card && !still && self.accounts.card_since.is_some_and(|at| at.elapsed() < motion::BAR_FILL.total());
        if visible < 1. || filling { motion::request_frame(window, cx); }
        let mut placement = Placement::Bottom;
        let (anchor, align, narrow, content) = match shown {
            Floating::WorktreeCleanup => ("topbar-worktree-cleanup".to_owned(), Align::End, true, self.render_worktree_cleanup_panel(cx)),
            Floating::Controls(open) => (open.anchor(), Align::End, true, self.render_ctl_panel_for(open, window, cx)),
            Floating::Commands => ("composer".to_owned(), Align::Start, false, Some(self.render_command_panel(cx))),
            Floating::Usage => ((if self.accounts.card_top { "topbar-account" } else { "composer-account" }).to_owned(), Align::End, true,
                Some(self.render_usage_card(window, cx))),
            Floating::Context => ("composer-ctx".to_owned(), Align::End, true, Some(self.render_context_card())),
            Floating::Hangar => ("hangar-chip".to_owned(), Align::Start, true, Some(self.render_hangar_popover(window, cx))),
            Floating::Artifacts => {
                placement = Placement::Top;
                ("artifacts-toggle".to_owned(), Align::Start, true, Some(self.render_artifacts_panel(window, cx)))
            }
            Floating::Machines => {
                if appearance::get().navigation == appearance::Navigation::BottomTabs { placement = Placement::Top; }
                ("machines-tray-toggle".to_owned(), Align::Start, true, Some(self.render_machines_tray(window, cx)))
            }
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
        let surface = chrome::popover(content?, narrow && !card && anchor != "machines-tray-toggle");
        // O de contas é mais largo que os outros: o email inteiro e o reset semanal cabem na linha.
        let surface = if card { div().w(px(480.)).child(surface).into_any_element() }
            else if narrow { surface } else { div().w(trigger.size.width).child(surface).into_any_element() };
        let dismiss = cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.close_popups();
            cx.stop_propagation();
            cx.notify();
        });
        let close = cx.listener(|this, _: &(), _, cx| {
            this.close_popups();
            cx.notify();
        });
        Some(layer(trigger, placement, align, surface, visible, leaving, card, dismiss, close))
    }
}

/// Cortina que fecha no clique fora sem deixar o clique chegar ao que está atrás, e a superfície presa ao gatilho,
/// abaixo dele ou virada para cima quando não cabe. Saindo, nada ali responde ao ponteiro.
#[allow(clippy::too_many_arguments)]
fn layer(trigger: Bounds<Pixels>, placement: Placement, align: Align, surface_content: AnyElement, visible: f32, leaving: bool, card: bool,
    dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static, close: impl Fn(&(), &mut Window, &mut App) + 'static) -> AnyElement {
    let surface = if leaving { motion::menu_out(div(), motion::MENU_OUT.ease(1. - visible)) }
        else if card { motion::card_open(div(), motion::CARD_OPEN.ease(visible)) }
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
pub(crate) fn dialog(surface: Dialog) -> Dialog {
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
    use super::{ANCHORS, Anchors, Area, Presence, anchor_bounds, anchor_peek, centered_top, px};
    use crate::motion;
    use gpui_kit::{Bounds, Pixels, point, size};

    fn at(x: f32) -> Bounds<Pixels> { Bounds::new(point(px(x), px(0.)), size(px(10.), px(10.))) }

    /// Um quadro da raiz com a área na tela: a geração da raiz sobe e a marca da área grava este quadro.
    fn frame(anchors: &mut Anchors, area: Option<Area>) {
        anchors.advance(None);
        if let Some(area) = area { anchors.show(area); }
    }

    #[test]
    fn anchor_holds_while_its_area_is_reused_and_drops_when_it_redraws_without_it() {
        let mut anchors = Anchors::default();
        let nav = Some(Area::Nav);
        frame(&mut anchors, nav);
        anchors.advance(nav);
        let drawn = anchors.generation(nav);
        anchors.record("row".into(), at(1.), nav, drawn);
        assert_eq!(anchors.valid("row"), Some(at(1.)));
        // Reusada do cache: não redesenha nem regrava.
        frame(&mut anchors, nav);
        assert_eq!(anchors.valid("row"), Some(at(1.)));
        // Redesenhada sem a peça.
        anchors.advance(nav);
        assert_eq!(anchors.valid("row"), None);
        // Desenhada de novo.
        let drawn = anchors.generation(nav);
        anchors.record("row".into(), at(2.), nav, drawn);
        assert_eq!(anchors.valid("row"), Some(at(2.)));
    }

    #[test]
    fn anchor_of_an_area_off_screen_is_not_valid() {
        let mut anchors = Anchors::default();
        let side = Some(Area::Side);
        frame(&mut anchors, side);
        anchors.record("panel".into(), at(1.), side, anchors.generation(side));
        assert_eq!(anchors.valid("panel"), Some(at(1.)));
        frame(&mut anchors, None);
        assert_eq!(anchors.valid("panel"), None);
        frame(&mut anchors, side);
        assert_eq!(anchors.valid("panel"), Some(at(1.)));
    }

    #[test]
    fn root_anchor_lasts_one_frame_unless_drawn_again() {
        let mut anchors = Anchors::default();
        anchors.record("dialog".into(), at(1.), None, anchors.generation(None));
        assert_eq!(anchors.valid("dialog"), Some(at(1.)));
        frame(&mut anchors, None);
        assert_eq!(anchors.valid("dialog"), None);
        anchors.record("dialog".into(), at(1.), None, anchors.generation(None));
        assert_eq!(anchors.valid("dialog"), Some(at(1.)));
    }

    #[test]
    fn only_a_read_anchor_asks_for_another_frame() {
        let mut anchors = Anchors::default();
        assert!(!anchors.record("x".into(), at(1.), None, 0), "sem leitor, mudar não redesenha");
        anchors.read.insert("x".into());
        assert!(anchors.record("x".into(), at(2.), None, 0), "mudou de lugar");
        assert!(!anchors.record("x".into(), at(2.), None, 0), "mesmo lugar");
        frame(&mut anchors, None);
        assert!(!anchors.record("x".into(), at(2.), None, anchors.generation(None)), "desenhada no quadro seguinte");
        frame(&mut anchors, None);
        frame(&mut anchors, None);
        assert!(anchors.record("x".into(), at(2.), None, anchors.generation(None)), "voltou depois de sumir");
    }

    #[test]
    fn area_reused_for_many_frames_does_not_ask_for_another_frame() {
        let mut anchors = Anchors::default();
        let top = Some(Area::Top);
        anchors.read.insert("search".into());
        frame(&mut anchors, top);
        anchors.advance(top);
        anchors.record("search".into(), at(1.), top, anchors.generation(top));
        for _ in 0..5 { frame(&mut anchors, top); }
        anchors.advance(top);
        assert!(!anchors.record("search".into(), at(1.), top, anchors.generation(top)), "reusada e redesenhada no mesmo lugar");
        // Redesenhada sem a peça e depois com ela.
        anchors.advance(top);
        anchors.advance(top);
        assert!(anchors.record("search".into(), at(1.), top, anchors.generation(top)), "a peça voltou");
        // A área saiu da tela e voltou.
        frame(&mut anchors, None);
        frame(&mut anchors, top);
        anchors.advance(top);
        assert!(anchors.record("search".into(), at(1.), top, anchors.generation(top)), "a área voltou");
    }

    #[test]
    fn prune_drops_only_marks_whose_drawer_redrew_without_them() {
        let mut anchors = Anchors::default();
        let (nav, side) = (Some(Area::Nav), Some(Area::Side));
        frame(&mut anchors, nav);
        anchors.advance(nav);
        anchors.advance(side);
        anchors.record("closed-row".into(), at(1.), nav, anchors.generation(nav));
        anchors.record("panel".into(), at(1.), side, anchors.generation(side));
        anchors.record("dialog".into(), at(1.), None, anchors.generation(None));
        // A lista redesenhou sem a linha; o painel segue do cache; a raiz passou de quadro.
        anchors.advance(nav);
        frame(&mut anchors, side);
        anchors.prune();
        assert!(!anchors.marks.contains_key("closed-row"));
        assert!(!anchors.marks.contains_key("dialog"));
        assert_eq!(anchors.valid("panel"), Some(at(1.)), "área em cache continua valendo");
    }

    #[test]
    fn peek_reads_without_becoming_a_reader() {
        ANCHORS.with(|a| a.borrow_mut().record("row".into(), at(1.), None, 0));
        assert_eq!(anchor_peek("row"), Some(at(1.)));
        assert!(ANCHORS.with(|a| !a.borrow().read.contains("row")), "a varredura não pede outro quadro");
        assert_eq!(anchor_bounds("row"), Some(at(1.)));
        assert!(ANCHORS.with(|a| a.borrow().read.contains("row")));
    }

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
