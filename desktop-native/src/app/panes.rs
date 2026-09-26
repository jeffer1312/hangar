// Áreas da janela em views próprias: o que se mexe numa área (streaming, rolagem, digitação, animação do diálogo) não
// redesenha as outras. O estado continua no `Hangar`; cada view só guarda o desenho dela entre quadros.
use std::{cell::{Cell, RefCell}, rc::Rc, sync::OnceLock, time::{Duration, Instant}};
use gpui_kit::{component::Root, *};
use super::Hangar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Area { Nav, Conversation, Bottom, Side, Overlay }

pub(super) struct Pane { hangar: WeakEntity<Hangar>, area: Area, _observe: Option<Subscription> }

pub(super) struct Panes {
    pub nav: Entity<Pane>,
    pub conversation: Entity<Pane>,
    pub bottom: Entity<Pane>,
    pub side: Entity<Pane>,
    pub overlay: Entity<Pane>,
    /// Altura medida da faixa de baixo: a view guardada precisa de altura definida, e o compositor cresce com o texto.
    pub bottom_height: Rc<Cell<f32>>,
    /// Lugares que as áreas guardadas deixaram para o que anima dentro delas (`MarkPlace`).
    pub marks: MarkPlaces,
}

/// O que se pinta num lugar: a marca animada ou os segundos da linha "trabalhando".
#[derive(Clone, Copy)]
pub(super) enum Floating { Mark(Hsla), Elapsed(Instant) }

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
            // chegaria à área. O diálogo não é guardado e roda em todo desenho da janela.
            _observe: (area != Area::Overlay).then(|| cx.observe(&hangar, |_, _, cx| cx.notify())),
        });
        Self {
            nav: pane(Area::Nav), conversation: pane(Area::Conversation), bottom: pane(Area::Bottom), side: pane(Area::Side),
            overlay: pane(Area::Overlay), bottom_height: Rc::new(Cell::new(120.)), marks: Rc::default(),
        }
    }
}

/// Só para medir: HANGAR_NATIVE_PANE_FRAMES=1 escreve no stderr cada desenho de área. Sem a variável, nada sai.
fn count_frames() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HANGAR_NATIVE_PANE_FRAMES").is_some())
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let area = self.area;
        if count_frames() { eprintln!("pane {area:?}"); }
        rendered(cx.entity_id(), window, cx);
        let Some(hangar) = self.hangar.upgrade() else { return div().into_any_element() };
        hangar.update(cx, |this, cx| {
            this.panes.marks.borrow_mut().retain(|place| place.area != area);
            this.render_area(area, window, cx)
        })
    }
}

impl Hangar {
    fn render_area(&mut self, area: Area, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match area {
            Area::Nav => self.render_nav(window, cx),
            Area::Conversation => self.render_conversation_area(window, cx),
            Area::Bottom => self.render_bottom_area(window, cx),
            Area::Side => self.render_side(window, cx).unwrap_or_else(|| div().into_any_element()),
            Area::Overlay => div().absolute().inset_0()
                .children(Root::render_dialog_layer(window, cx))
                .children(Root::render_notification_layer(window, cx))
                .into_any_element(),
        }
    }

    /// A área guardada entre quadros; o diálogo não, porque lê o estado do `Root`, que não o avisa.
    pub(super) fn pane_element(&self, area: Area, style: StyleRefinement, cx: &App) -> AnyElement {
        let pane = match area {
            Area::Nav => &self.panes.nav, Area::Conversation => &self.panes.conversation, Area::Bottom => &self.panes.bottom,
            Area::Side => &self.panes.side,
            Area::Overlay => {
                let mut frame = div();
                frame.style().refine(&style);
                return frame.child(self.panes.overlay.clone()).into_any_element();
            }
        };
        // O painel mostra a aba Atividade, que tem views próprias com texto selecionável dentro dele.
        let nested = if area == Area::Side { self.activity_views(cx) } else { Vec::new() };
        cached_selectable(pane.clone().into(), nested, style)
    }

    /// Redesenha uma área só, sem acordar as outras: para o que muda só nela (texto chegando, rolagem, digitação).
    pub(super) fn redraw(&self, area: Area, cx: &mut Context<Self>) {
        let pane = match area {
            Area::Nav => &self.panes.nav, Area::Conversation => &self.panes.conversation, Area::Bottom => &self.panes.bottom,
            Area::Side => &self.panes.side, Area::Overlay => &self.panes.overlay,
        };
        pane.update(cx, |_, cx| cx.notify());
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

/// Guarda uma view entre quadros sem perder o texto selecionável dela. A seleção do kit apaga, no fim do quadro, o texto
/// que não se pintou, e a view reusada não pinta: o `canvas` depois dela avisa o kit que ela e as views de dentro dela
/// (`nested`) continuam na tela. Toda view guardada assim chama `rendered` no próprio `render`.
pub(super) fn cached_selectable(view: AnyView, nested: Vec<EntityId>, style: StyleRefinement) -> AnyElement {
    let mut ids = nested;
    ids.push(view.entity_id());
    let mut frame = div().relative();
    frame.style().refine(&style);
    frame.child(view.cached(StyleRefinement::default().size_full()))
        .child(canvas(|_, _, _| {}, move |_, _, window, cx| {
            for id in ids { base::TextSelection::retain_cached_view(id, window, cx); }
        }).absolute().size_0())
        .into_any_element()
}

/// A view guardada desenhou de novo: o texto dela que não se pintou sai da seleção.
pub(super) fn rendered(view: EntityId, window: &Window, cx: &mut App) {
    base::TextSelection::view_rendered(view, window, cx);
}

/// Largura do lugar dos segundos: cabe "59m 59s" sem a linha mudar de medida a cada tique.
const ELAPSED_WIDTH: f32 = 52.;

impl Hangar {
    /// O lugar vazio da marca animada `key` na área `area`: a marca é pintada fora da view guardada, no lugar que esta
    /// caixa gravou, pelo `working_mark_float` da mesma área.
    pub(super) fn working_mark_slot(&self, area: Area, key: impl Into<SharedString>, size: f32, color: Hsla) -> AnyElement {
        mark_slot(&self.panes.marks, area, key, size, color)
    }

    /// O lugar dos segundos contados desde `since`, pintados fora da view guardada: o tique de 1 s não redesenha a área.
    pub(super) fn elapsed_slot(&self, area: Area, key: impl Into<SharedString>, since: Instant) -> AnyElement {
        div().w(px(ELAPSED_WIDTH)).h_full().flex_shrink_0().child(mark_place(self.panes.marks.clone(), area, key.into(), Floating::Elapsed(since)))
            .into_any_element()
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
            let t = if self.reduce_motion { 1. }
                else { super::chrome::ease_out((place.born.elapsed().as_secs_f32() / self.fade.as_secs_f32()).min(1.)) };
            let inner = match place.draw {
                Floating::Mark(color) => super::chrome::WorkingMark::new(place.key.clone(), f32::from(place.at.size.width), color)
                    .into_any_element(),
                Floating::Elapsed(since) => super::chrome::Elapsed::new(place.key.clone(), since).into_any_element(),
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
