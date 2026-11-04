use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Size, Style, Window, px, size,
};

use super::{
    document::NodeRenderOptions,
    node::{BlockNode, NodeContext},
};

/// Below this a document costs too little for skipping blocks to pay.
pub(super) const MIN_BLOCKS: usize = 8;
/// A block laid out alone can round apart from its place in the column.
const DRIFT: Pixels = px(1.);

#[cfg(test)]
thread_local! {
    static BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Where a document's blocks sit in their column, from the last frame that
/// laid all of them out.
#[derive(Default)]
pub(crate) struct BlockPlaces {
    document: usize,
    size: Size<Pixels>,
    places: Vec<Bounds<Pixels>>,
    /// Frames in a row that measured the same document at the same width.
    /// While a window resizes or a message streams this stays at zero, so
    /// the column keeps laying everything out.
    settled: u32,
}

impl BlockPlaces {
    pub(crate) fn forget(&mut self) {
        self.places.clear();
        self.settled = 0;
    }

    fn known(&self, document: usize) -> bool {
        self.document == document && self.settled > 0 && !self.places.is_empty()
    }

    fn measured(&mut self, document: usize, size: Size<Pixels>, places: Vec<Bounds<Pixels>>) {
        let same = self.document == document && self.size.width == size.width;
        self.settled = if same { self.settled + 1 } else { 0 };
        self.document = document;
        self.size = size;
        self.places = places;
    }
}

/// The blocks of a document that does not scroll by itself. A chat row holds
/// a whole message, so every block used to be built, laid out and painted each
/// frame, on screen or not. Once their places are known only the blocks inside
/// the content mask are; the rest of the column is just its height.
///
/// Skipping stays off whenever something needs every block: a selection
/// learns its text from painted blocks, and a reveal or a fade from its own
/// block. `skip_unseen` says so.
pub(super) struct BlockColumn {
    id: ElementId,
    blocks: Arc<Vec<BlockNode>>,
    node_cx: NodeContext,
    places: Rc<RefCell<BlockPlaces>>,
    skip_unseen: bool,
    children: Vec<AnyElement>,
}

pub(crate) enum BlockColumnLayout {
    All(Vec<LayoutId>),
    Seen,
}

impl BlockColumn {
    pub(super) fn new(
        id: impl Into<ElementId>,
        blocks: Arc<Vec<BlockNode>>,
        node_cx: NodeContext,
        places: Rc<RefCell<BlockPlaces>>,
        skip_unseen: bool,
    ) -> Self {
        Self {
            id: id.into(),
            blocks,
            node_cx,
            places,
            skip_unseen,
            children: Vec::new(),
        }
    }

    fn document(&self) -> usize {
        Arc::as_ptr(&self.blocks) as usize
    }

    fn render(&self, ix: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        #[cfg(test)]
        BUILT.with(|built| built.set(built.get() + 1));
        self.blocks[ix].render_block(
            NodeRenderOptions {
                ix,
                is_last: ix + 1 == self.blocks.len(),
                ..Default::default()
            },
            &self.node_cx,
            window,
            cx,
        )
    }
}

impl IntoElement for BlockColumn {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for BlockColumn {
    type RequestLayoutState = BlockColumnLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let places = self.places.borrow();
        if self.skip_unseen && places.known(self.document()) {
            let whole = places.size;
            let layout = window.request_measured_layout(
                Style::default(),
                move |known, available, _, _| {
                    let width = match available.width {
                        AvailableSpace::Definite(width) => width,
                        _ => whole.width,
                    };
                    size(
                        known.width.unwrap_or(width),
                        known.height.unwrap_or(whole.height),
                    )
                },
            );
            return (layout, BlockColumnLayout::Seen);
        }
        drop(places);

        self.children = (0..self.blocks.len())
            .map(|ix| self.render(ix, window, cx))
            .collect();
        let ids: Vec<_> = self
            .children
            .iter_mut()
            .map(|child| child.request_layout(window, cx))
            .collect();
        let layout = window.request_layout(Style::default(), ids.iter().copied(), cx);
        (layout, BlockColumnLayout::All(ids))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let document = self.document();
        match layout {
            BlockColumnLayout::All(ids) => {
                let places = ids
                    .iter()
                    .map(|id| {
                        let place = window.layout_bounds(*id);
                        Bounds::new(place.origin - bounds.origin, place.size)
                    })
                    .collect();
                self.places.borrow_mut().measured(document, bounds.size, places);
                for child in &mut self.children {
                    child.prepaint(window, cx);
                }
            }
            BlockColumnLayout::Seen => {
                let mask = window.content_mask().bounds;
                let places = self.places.borrow().places.clone();
                let mut moved = self.places.borrow().size.width != bounds.size.width;
                for (ix, place) in places.iter().enumerate() {
                    let top = bounds.origin.y + place.origin.y;
                    if top >= mask.bottom() || top + place.size.height <= mask.top() {
                        continue;
                    }
                    let mut block = self.render(ix, window, cx);
                    let space = size(
                        AvailableSpace::Definite(place.size.width),
                        AvailableSpace::MinContent,
                    );
                    let laid = block.layout_as_root(space, window, cx);
                    moved |= (laid.height - place.size.height).abs() > DRIFT;
                    block.prepaint_at(bounds.origin + place.origin, window, cx);
                    self.children.push(block);
                }
                // An image that loaded or a width that changed: the next
                // frames lay everything out again.
                if moved {
                    self.places.borrow_mut().forget();
                    window.request_animation_frame();
                }
            }
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        for child in &mut self.children {
            child.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BUILT;
    use crate::text::{TextView, TextViewState};
    use gpui::{
        AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
        ParentElement as _, Pixels, Render, ScrollHandle, StatefulInteractiveElement as _,
        Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
    };

    const BLOCKS: usize = 60;

    struct Row {
        state: Entity<TextViewState>,
        scroll: ScrollHandle,
        width: Pixels,
    }

    impl Render for Row {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(self.width).h(px(200.)).child(
                div().id("viewport").size_full().overflow_y_scroll().track_scroll(&self.scroll)
                    .child(TextView::new(&self.state).scrollable(false)),
            )
        }
    }

    fn document() -> String {
        (0..BLOCKS)
            .map(|ix| format!("Block {ix} says enough words to wrap on a narrow column of text."))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn open(cx: &mut TestAppContext) -> (Entity<Row>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (row, cx) = cx.add_window_view(|_, cx| Row {
            state: cx.new(|cx| TextViewState::markdown(&document(), cx)),
            scroll: ScrollHandle::new(),
            width: px(300.),
        });
        cx.run_until_parked();
        (row, cx)
    }

    /// Blocks built by one frame.
    fn frame(cx: &mut VisualTestContext) -> usize {
        BUILT.with(|built| built.set(0));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        BUILT.with(|built| built.get())
    }

    fn settle(cx: &mut VisualTestContext) {
        for _ in 0..4 {
            frame(cx);
        }
    }

    fn bottom(row: &Entity<Row>, cx: &VisualTestContext) -> Pixels {
        row.read_with(cx, |row, _| row.scroll.max_offset().y)
    }

    #[gpui::test]
    fn only_the_blocks_on_screen_are_built_once_their_places_are_known(cx: &mut TestAppContext) {
        let (row, cx) = open(cx);
        assert_eq!(frame(cx), BLOCKS, "the first frame measures every block");
        let whole = bottom(&row, cx);
        settle(cx);

        let seen = frame(cx);
        assert!(seen > 0 && seen < BLOCKS / 2, "built {seen} of {BLOCKS}");
        assert_eq!(bottom(&row, cx), whole, "skipping keeps the column's height");
    }

    #[gpui::test]
    fn scrolling_builds_the_blocks_that_come_into_view(cx: &mut TestAppContext) {
        let (row, cx) = open(cx);
        settle(cx);
        let whole = bottom(&row, cx);
        row.update(cx, |row, _| row.scroll.set_offset(point(px(0.), -whole)));

        let seen = frame(cx);
        assert!(seen > 0 && seen < BLOCKS / 2, "built {seen} of {BLOCKS}");
        assert_eq!(bottom(&row, cx), whole);
    }

    /// A selection learns its text from painted blocks.
    #[gpui::test]
    fn a_selection_builds_every_block(cx: &mut TestAppContext) {
        let (row, cx) = open(cx);
        settle(cx);
        row.update(cx, |row, cx| row.state.update(cx, |state, _| state.is_selecting = true));
        row.update(cx, |_, cx| cx.notify());
        assert_eq!(frame(cx), BLOCKS);
    }

    #[gpui::test]
    fn another_width_measures_again_and_settles(cx: &mut TestAppContext) {
        let (row, cx) = open(cx);
        settle(cx);
        let wide = bottom(&row, cx);
        row.update(cx, |row, cx| {
            row.width = px(150.);
            cx.notify();
        });
        settle(cx);
        settle(cx);

        assert!(bottom(&row, cx) > wide, "a narrower column wraps into more lines");
        let seen = frame(cx);
        assert!(seen < BLOCKS / 2, "built {seen} of {BLOCKS} after settling");
    }

    #[gpui::test]
    fn new_text_measures_again(cx: &mut TestAppContext) {
        let (row, cx) = open(cx);
        settle(cx);
        let before = bottom(&row, cx);
        row.update(cx, |row, cx| {
            row.state.update(cx, |state, cx| state.set_text(&format!("{}\n\nOne more block.", document()), cx));
        });
        cx.run_until_parked();
        settle(cx);
        assert!(bottom(&row, cx) > before);
    }
}
