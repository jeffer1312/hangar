// Áreas da janela em views próprias: o que se mexe numa área (streaming, rolagem, digitação, animação do diálogo) não
// redesenha as outras. O estado continua no `Hangar`; cada view só guarda o desenho dela entre quadros.
use std::{cell::Cell, rc::Rc, sync::OnceLock, time::Duration};
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
    /// Onde a linha "trabalhando" deixou o lugar da marca no último desenho da conversa (`MarkSlot`).
    pub working_mark: MarkSlot,
}

/// Lugar da marca: posição, recorte da lista e quando a linha nasceu. A conversa guardada não repinta, então o valor
/// do último desenho dela continua certo; a conversa que redesenha zera e a linha, se aparecer, grava de novo.
pub(super) type MarkSlot = Rc<Cell<Option<(Bounds<Pixels>, Bounds<Pixels>, std::time::Instant)>>>;

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
            overlay: pane(Area::Overlay), bottom_height: Rc::new(Cell::new(120.)), working_mark: Rc::default(),
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
            if area == Area::Conversation { this.panes.working_mark.set(None); }
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

impl Hangar {
    /// O lugar vazio da marca na linha "trabalhando": grava onde ficou para a marca de fora da conversa.
    pub(super) fn working_mark_slot(&self, size: f32) -> AnyElement {
        let slot = self.panes.working_mark.clone();
        div().size(px(size)).flex_shrink_0().child(canvas(move |bounds, window, _| {
            // O nascimento da linha, para a marca entrar junto com o fade dela.
            let born = window.with_global_id("working-mark-born".into(), |id, window| {
                window.with_element_state(id, |born: Option<std::time::Instant>, _| {
                    let born = born.unwrap_or_else(std::time::Instant::now);
                    (born, born)
                })
            });
            slot.set(Some((bounds, window.content_mask().bounds, born)));
        }, |_, _, _, _| {}).size_full()).into_any_element()
    }

    /// A marca animada da linha "trabalhando", desenhada fora da conversa guardada: a batida dela suja só ela e a
    /// raiz, que redesenha em todo quadro, e a conversa segue reusada do cache.
    pub(super) fn working_mark_float(&self, fade: Duration, reduce_motion: bool) -> AnyElement {
        // O nascimento é o do último desenho da conversa; o lugar é lido só no prepaint, depois dela.
        let t = match self.panes.working_mark.get() {
            _ if reduce_motion => 1.,
            Some((_, _, born)) => super::chrome::ease_out((born.elapsed().as_secs_f32() / fade.as_secs_f32()).min(1.)),
            None => 0.,
        };
        FloatingMark { slot: self.panes.working_mark.clone(), child: Some(div().size_full().opacity(t)
            .child(super::chrome::WorkingMark::new("working-line", 22., super::theme::accent())).into_any_element()) }
            .into_any_element()
    }
}

/// Desenha o filho no lugar e no recorte gravados neste quadro pela conversa, ou no último desenho dela se foi reusada.
/// Fica depois da conversa na árvore, então lê o lugar já atualizado; posição absoluta, fora do fluxo da janela.
struct FloatingMark { slot: MarkSlot, child: Option<AnyElement> }

impl IntoElement for FloatingMark {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for FloatingMark {
    type RequestLayoutState = ();
    type PrepaintState = Option<(AnyElement, Bounds<Pixels>)>;

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
        // O lugar deste quadro, se a conversa acabou de redesenhar; senão o que ela deixou.
        let (at, clip, _) = self.slot.get()?;
        let mut child = self.child.take()?;
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            child.layout_as_root(at.size.map(AvailableSpace::Definite), window, cx);
            child.prepaint_at(at.origin, window, cx);
        });
        Some((child, clip))
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (),
        state: &mut Self::PrepaintState, window: &mut Window, cx: &mut App) {
        if let Some((child, clip)) = state {
            window.with_content_mask(Some(ContentMask { bounds: *clip }), |window| child.paint(window, cx));
        }
    }
}
