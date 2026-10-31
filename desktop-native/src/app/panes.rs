// Áreas da janela em views próprias: o que se mexe numa área (streaming, rolagem, digitação, animação do diálogo) não
// redesenha as outras. O estado continua no `Hangar`; cada view só guarda o desenho dela entre quadros.
use std::{cell::{Cell, RefCell}, rc::Rc, sync::OnceLock, time::{Duration, Instant}};
use gpui_kit::{prelude::FluentBuilder, *};
use super::Hangar;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Area { Nav, Conversation, Bottom, Side, Top }

pub(super) struct Pane { hangar: WeakEntity<Hangar>, area: Area, _observe: Option<Subscription> }

pub(super) struct Panes {
    pub nav: Entity<Pane>,
    pub conversation: Entity<Pane>,
    pub bottom: Entity<Pane>,
    pub side: Entity<Pane>,
    pub top: Entity<Pane>,
    /// O `beside` da barra do app no último desenho: vem de medidas da raiz que mudam sem aviso (painel deslizando).
    pub top_beside: Cell<Option<f32>>,
    /// Altura medida da faixa de baixo: a view guardada precisa de altura definida, e o compositor cresce com o texto.
    pub bottom_height: Rc<Cell<f32>>,
    /// Lugares que as áreas guardadas deixaram para o que anima dentro delas (`MarkPlace`).
    pub marks: MarkPlaces,
}

/// O que se pinta num lugar: a marca animada, o avatar trabalhando ou os segundos da linha "trabalhando".
#[derive(Clone)]
pub(super) enum Floating { Mark(Hsla), Avatar(super::chrome::AvatarLook), Elapsed(Instant, Option<SharedString>), Shimmer(SharedString), ThinkingFocus(SharedString) }

/// Um lugar vazio deixado por uma área guardada: posição, recorte dela e quando o lugar nasceu. A área que não repinta
/// deixa os lugares do último desenho, que continuam certos; a que redesenha apaga os seus e grava os que aparecerem.
#[derive(Clone)]
pub(super) struct MarkPlace { area: Area, key: SharedString, at: Bounds<Pixels>, clip: Bounds<Pixels>, born: Instant, draw: Floating }

pub(super) type MarkPlaces = Rc<RefCell<Vec<MarkPlace>>>;

impl Panes {
    pub fn new(cx: &mut Context<Hangar>) -> Self {
        let hangar = cx.entity();
        let mut pane = |area: Area| cx.new(|cx| Pane {
            hangar: hangar.downgrade(), area,
            // A view guardada não enxerga o `notify` do `Hangar`, que é ancestral dela: sem isto, o que mudou nele não
            // chegaria à área.
            _observe: Some(cx.observe(&hangar, |_, _, cx| cx.notify())),
        });
        Self {
            nav: pane(Area::Nav), conversation: pane(Area::Conversation), bottom: pane(Area::Bottom), side: pane(Area::Side),
            top: pane(Area::Top), top_beside: Cell::new(None),
            bottom_height: Rc::new(Cell::new(120.)), marks: Rc::default(),
        }
    }
}

/// Só para medir: HANGAR_NATIVE_PANE_FRAMES=1 escreve no stderr cada desenho de área, com o tempo de montar a árvore dela
/// (µs) e o instante do desenho (ms desde a primeira linha). Sem a variável, nada sai.
fn count_frames() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HANGAR_NATIVE_PANE_FRAMES").is_some())
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let area = self.area;
        let started = count_frames().then(Instant::now);
        let Some(hangar) = self.hangar.upgrade() else { return div().into_any_element() };
        let element = hangar.update(cx, |this, cx| {
            this.panes.marks.borrow_mut().retain(|place| place.area != area);
            this.render_area(area, window, cx)
        });
        if let Some(started) = started {
            static EPOCH: OnceLock<Instant> = OnceLock::new();
            let epoch = *EPOCH.get_or_init(|| started);
            eprintln!("pane {area:?} {} {}", started.elapsed().as_micros(), started.duration_since(epoch).as_millis());
        }
        // Depois do desenho: quem lê as âncoras durante ele ainda vê as do desenho anterior.
        super::popup::area_redrawn(area);
        super::popup::area_scope(area, element)
    }
}

impl Hangar {
    fn render_area(&mut self, area: Area, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match area {
            Area::Nav => self.render_nav(window, cx),
            Area::Conversation => self.render_conversation_area(window, cx),
            Area::Bottom => self.render_bottom_area(window, cx),
            Area::Side => {
                // Fechando, o painel ainda desliza para fora com o conteúdo de antes.
                let open = self.side.open;
                self.side.open |= self.side_closing();
                let side = self.render_side(window, cx);
                self.side.open = open;
                side.unwrap_or_else(|| div().into_any_element())
            }
            Area::Top => self.render_topbar(self.panes.top_beside.get(), window, cx),
        }
    }

    fn pane(&self, area: Area) -> &Entity<Pane> {
        match area {
            Area::Nav => &self.panes.nav, Area::Conversation => &self.panes.conversation, Area::Bottom => &self.panes.bottom,
            Area::Side => &self.panes.side, Area::Top => &self.panes.top,
        }
    }

    /// A área guardada entre quadros.
    pub(super) fn pane_element(&self, area: Area, style: StyleRefinement) -> AnyElement {
        // Na chegada da primeira mensagem a conversa, o painel e a faixa de baixo (que, antes de a sessão nascer, desenha a
        // conversa por vir) desenham a cada quadro: a cópia guardada não acompanha a opacidade de quem a envolve.
        let live = self.landing_active() && matches!(area, Area::Conversation | Area::Side | Area::Bottom);
        self.pane_element_live(area, style, live)
    }

    /// A área desenhada neste quadro sem a cópia guardada quando `live`.
    pub(super) fn pane_element_live(&self, area: Area, style: StyleRefinement, live: bool) -> AnyElement {
        let pane = self.pane(area);
        let shown = super::popup::area_on_screen(area);
        if live {
            let mut frame = div();
            frame.style().refine(&style);
            return frame.child(shown).child(AnyView::from(pane.clone())).into_any_element();
        }
        let mut frame = div().relative();
        frame.style().refine(&style);
        frame.child(shown).child(AnyView::from(pane.clone()).cached(StyleRefinement::default().size_full())).into_any_element()
    }

    /// Redesenha uma área só, sem acordar as outras: para o que muda só nela (texto chegando, rolagem, digitação).
    pub(super) fn redraw(&self, area: Area, cx: &mut Context<Self>) {
        // Notificar pelo id, sem `update`: chamado de dentro do desenho da própria área, o `update` dela entra em pânico.
        App::notify(cx, self.pane(area).entity_id());
    }

    /// A faixa de baixo ancorada no pé da caixa: crescendo, ela sobe por cima da conversa no mesmo quadro, e a medida
    /// devolve a altura nova à janela no quadro seguinte.
    pub(super) fn measured_bottom(&self, content: AnyElement) -> AnyElement {
        let height = self.panes.bottom_height.clone();
        div().size_full().relative()
            .child(div().absolute().left_0().right_0().bottom_0().flex().flex_col()
                .child(content)
                .child(canvas(move |bounds, window, _| {
                    let measured = f32::from(bounds.size.height);
                    if (height.get() - measured).abs() > 0.5 { height.set(measured); window.request_animation_frame(); }
                }, |_, _, _, _| {}).absolute().inset_0()))
            .into_any_element()
    }
}

/// Guarda uma view entre quadros; o kit mantém selecionável o texto de uma view reusada sem repintar.
pub(super) fn cached_selectable(view: AnyView, style: StyleRefinement) -> AnyElement {
    let mut frame = div().relative();
    frame.style().refine(&style);
    frame.child(view.cached(StyleRefinement::default().size_full())).into_any_element()
}

/// Largura do lugar dos segundos: cabe "59m 59s" sem a linha mudar de medida a cada tique.
const ELAPSED_WIDTH: f32 = 52.;

impl Hangar {
    /// O lugar vazio da marca animada `key` na área `area`: a marca é pintada fora da view guardada, no lugar que esta
    /// caixa gravou, pelo `working_mark_float` da mesma área.
    pub(super) fn working_mark_slot(&self, area: Area, key: impl Into<SharedString>, size: f32, color: Hsla) -> AnyElement {
        mark_slot(&self.panes.marks, area, key, size, color)
    }

    /// O avatar da sessão: trabalhando, ele é pintado fora da view guardada (a animação não redesenha a área); nos
    /// outros estados é parado, dentro dela.
    pub(super) fn avatar_slot(&self, area: Area, key: impl Into<SharedString>, look: super::chrome::AvatarLook, size: f32) -> AnyElement {
        if !look.working() { return super::chrome::session_avatar(&look, size, None).into_any_element(); }
        div().size(px(size)).flex_shrink_0()
            .child(mark_place(self.panes.marks.clone(), area, key.into(), Floating::Avatar(look))).into_any_element()
    }

    /// O lugar dos segundos contados desde `since`, pintados fora da view guardada: o tique de 1 s não redesenha a área.
    /// `suffix` vai depois dos segundos no mesmo texto ("29s · ↓ 1.4k tokens"); com ele o lugar ocupa o resto da linha.
    pub(super) fn elapsed_slot(&self, area: Area, key: impl Into<SharedString>, since: Instant, suffix: Option<SharedString>) -> AnyElement {
        div().h_full().map(|el| if suffix.is_some() { el.flex_1().min_w_0() } else { el.w(px(ELAPSED_WIDTH)).flex_shrink_0() })
            .child(mark_place(self.panes.marks.clone(), area, key.into(), Floating::Elapsed(since, suffix)))
            .into_any_element()
    }

    /// O título `text` com o brilho passando, pintado fora da view guardada; na área fica o mesmo texto invisível, que
    /// dá a medida e o corte.
    pub(super) fn shimmer_slot(&self, area: Area, key: impl Into<SharedString>, text: String) -> AnyElement {
        let text = SharedString::from(text);
        div().relative().min_w_0().truncate().text_color(transparent_black()).child(text.clone())
            .child(div().absolute().inset_0().child(mark_place(self.panes.marks.clone(), area, key.into(), Floating::Shimmer(text))))
            .into_any_element()
    }

    /// O lugar do brilho do "Pensando": a linha apagada fica na área, guardada e acessível; o brilho é pintado por
    /// cima, fora da view guardada, para a batida não redesenhar a conversa.
    pub(super) fn thinking_focus_slot(&self, area: Area, key: impl Into<SharedString>, label: SharedString) -> AnyElement {
        mark_place(self.panes.marks.clone(), area, key.into(), Floating::ThinkingFocus(label)).into_any_element()
    }

    /// O que anima nos lugares da área, desenhado fora da view guardada: a batida suja só isso e a raiz, que redesenha
    /// em todo quadro, e a área segue reusada do cache. Fica depois da área na árvore, para ler os lugares já gravados.
    pub(super) fn working_mark_float(&self, area: Area, fade: Duration, reduce_motion: bool) -> AnyElement {
        float_marks(self.panes.marks.clone(), area, fade, reduce_motion)
    }
}

/// O `working_mark_float` de uma lista de lugares própria: a view guardada dentro de uma área (a aba Atividade) guarda
/// e limpa os seus, e o painel que redesenha sem ela não os apaga.
pub(super) fn float_marks(places: MarkPlaces, area: Area, fade: Duration, reduce_motion: bool) -> AnyElement {
    FloatingMark { places, area, fade, reduce_motion }.into_any_element()
}

/// O mesmo lugar de `working_mark_slot`, na lista de lugares de quem chama.
pub(super) fn mark_slot(places: &MarkPlaces, area: Area, key: impl Into<SharedString>, size: f32, color: Hsla) -> AnyElement {
    div().size(px(size)).flex_shrink_0().child(mark_place(places.clone(), area, key.into(), Floating::Mark(color))).into_any_element()
}

/// O mesmo lugar de `elapsed_slot`, na lista de lugares de quem chama.
pub(super) fn elapsed_place(places: &MarkPlaces, area: Area, key: impl Into<SharedString>, since: Instant) -> AnyElement {
    div().w(px(ELAPSED_WIDTH)).h_full().flex_shrink_0().child(mark_place(places.clone(), area, key.into(), Floating::Elapsed(since, None)))
        .into_any_element()
}

fn mark_place(places: MarkPlaces, area: Area, key: SharedString, draw: Floating) -> impl IntoElement {
    canvas(move |bounds, window, _| {
        // O nascimento do lugar, para o que se pinta nele entrar junto com o fade da linha.
        let born = window.with_global_id(ElementId::Name(format!("{key}-born").into()), |id, window| {
            window.with_element_state(id, |born: Option<Instant>, _| {
                let born = born.unwrap_or_else(Instant::now);
                (born, born)
            })
        });
        let mut places = places.borrow_mut();
        places.retain(|place| place.area != area || place.key != key);
        places.push(MarkPlace { area, key, at: bounds, clip: window.content_mask().bounds, born, draw });
    }, |_, _, _, _| {}).size_full()
}

/// Desenha, em cada lugar da área, o que ele pede, no recorte gravado neste quadro pela área, ou no último desenho dela
/// se foi reusada. Posição absoluta, fora do fluxo da janela.
struct FloatingMark { places: MarkPlaces, area: Area, fade: Duration, reduce_motion: bool }

impl IntoElement for FloatingMark {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for FloatingMark {
    type RequestLayoutState = ();
    type PrepaintState = Vec<(AnyElement, Bounds<Pixels>)>;

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App)
        -> (LayoutId, ()) {
        let mut style = Style::default();
        style.position = Position::Absolute;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (),
        window: &mut Window, cx: &mut App) -> Self::PrepaintState {
        // Cópia dos lugares: desenhar o filho não pode achar a lista emprestada.
        let places: Vec<MarkPlace> = self.places.borrow().iter().filter(|place| place.area == self.area).cloned().collect();
        places.into_iter().map(|place| {
            // Texto em cache nasce visível: sem foco, o relógio não redesenha depois do primeiro quadro. O avatar já estava
            // parado na linha: entrar com o fade apagaria o logo.
            let t = if self.reduce_motion || matches!(place.draw, Floating::Avatar(_) | Floating::Elapsed(..) | Floating::Shimmer(_) | Floating::ThinkingFocus(_)) { 1. }
                else { crate::motion::ease_out((place.born.elapsed().as_secs_f32() / self.fade.as_secs_f32()).min(1.)) };
            let width = f32::from(place.at.size.width);
            let inner = match place.draw {
                Floating::Mark(color) => super::chrome::WorkingMark::new(place.key.clone(), width, color).into_any_element(),
                Floating::Avatar(look) => super::chrome::LiveAvatar::new(place.key.clone(), look, width).into_any_element(),
                Floating::Elapsed(since, suffix) => super::chrome::Elapsed::new(place.key.clone(), since).suffix(suffix).into_any_element(),
                Floating::Shimmer(text) => super::chrome::Shimmer::new(place.key.clone(), text).into_any_element(),
                Floating::ThinkingFocus(label) => super::chrome::ThinkingFocus::new(place.key.clone(), label).into_any_element(),
            };
            let mut child = div().size_full().opacity(t).child(inner).into_any_element();
            window.with_content_mask(Some(ContentMask { bounds: place.clip }), |window| {
                child.layout_as_root(place.at.size.map(AvailableSpace::Definite), window, cx);
                child.prepaint_at(place.at.origin, window, cx);
            });
            (child, place.clip)
        }).collect()
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (),
        state: &mut Self::PrepaintState, window: &mut Window, cx: &mut App) {
        for (child, clip) in state {
            window.with_content_mask(Some(ContentMask { bounds: *clip }), |window| child.paint(window, cx));
        }
    }
}
