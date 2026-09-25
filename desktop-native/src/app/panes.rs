// Áreas da janela em views próprias: o que se mexe numa área (streaming, rolagem, digitação, animação do diálogo) não
// redesenha as outras. O estado continua no `Hangar`; cada view só guarda o desenho dela entre quadros.
use std::{cell::Cell, rc::Rc, sync::OnceLock};
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
    /// Área que desenhou texto selecionável no último desenho. A seleção do kit apaga, depois de todo quadro, o texto
    /// que não se pintou nele: área assim não pode ser reusada do cache, ou perde a seleção e redesenha no quadro seguinte.
    pub bottom_text: Cell<bool>,
    pub side_text: Cell<bool>,
    drawing: Cell<Option<Area>>,
}

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
            overlay: pane(Area::Overlay), bottom_height: Rc::new(Cell::new(120.)),
            bottom_text: Cell::new(false), side_text: Cell::new(false), drawing: Cell::new(None),
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
        let Some(hangar) = self.hangar.upgrade() else { return div().into_any_element() };
        hangar.update(cx, |this, cx| {
            this.panes.drawing.set(Some(area));
            match area { Area::Bottom => this.panes.bottom_text.set(false), Area::Side => this.panes.side_text.set(false), _ => {} }
            let element = this.render_area(area, window, cx);
            this.panes.drawing.set(None);
            element
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

    /// Chamar ao montar texto selecionável na faixa de baixo ou no painel: no próximo quadro a área sai do cache.
    pub(super) fn saw_selectable_text(&self) {
        match self.panes.drawing.get() {
            Some(Area::Bottom) => self.panes.bottom_text.set(true),
            Some(Area::Side) => self.panes.side_text.set(true),
            _ => {}
        }
    }

    /// A área guardada entre quadros, ou desenhada em todo quadro quando mostrou texto selecionável no último desenho.
    pub(super) fn pane_element(&self, area: Area, style: StyleRefinement) -> AnyElement {
        let (pane, text) = match area {
            Area::Bottom => (&self.panes.bottom, self.panes.bottom_text.get()),
            Area::Side => (&self.panes.side, self.panes.side_text.get()),
            Area::Nav => (&self.panes.nav, false),
            // A conversa sempre tem texto selecionável; o diálogo lê o estado do `Root`, que não o avisa.
            Area::Conversation => (&self.panes.conversation, true),
            Area::Overlay => (&self.panes.overlay, true),
        };
        if !text { return pane.clone().cached(style).into_any_element(); }
        let mut frame = div();
        frame.style().refine(&style);
        frame.child(pane.clone()).into_any_element()
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
