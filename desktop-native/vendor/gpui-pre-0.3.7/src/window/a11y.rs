//! Accessibility support, provided by [AccessKit][accesskit].
//!
//! There are user-facing guide-level docs [here](crate::_accessibility).
//!
//! ## Architecture
//!
//! ```text
//!                              ┌────────────────────────────────┐   ┌─────────────────────┐
//!                           ┌─▶│ AccessKit Adapter (MacOS)      │◀─▶│ MacOS System APIs   │
//!                           │  └────────────────────────────────┘   └─────────────────────┘
//!                           │
//! ┌──────┐   ┌───────────┐  │  ┌────────────────────────────────┐   ┌─────────────────────┐
//! │ GPUI │◀─▶│ AccessKit │◀─┼─▶│ AccessKit Adapter (Windows)    │◀─▶│ Windows System APIs │
//! └──────┘   └───────────┘  │  └────────────────────────────────┘   └─────────────────────┘
//!                           │
//!                           │  ┌────────────────────────────────┐   ┌─────────────────────┐
//!                           └─▶│ AccessKit Adapter (Linux)      │◀─▶│ dbus                │
//!                              └────────────────────────────────┘   └─────────────────────┘
//! ```
//!
//! In order for GPUI apps to be usable for people using assistive technology,
//! we must do a few things:
//! - Inform the system when the UI changes meaningfully. This includes:
//!   - Reporting new/removed/changed UI elements
//!   - *Not* reporting irrelevant UI changes, e.g. an invisible `div()` being
//!     added.
//!   - Reporting the appearance and capabilities of each UI element. For example:
//!     - What does this piece of text say?
//!     - How far along is this progress bar?
//!     - Can this node be focused?
//!     - Can this node have a value directly assigned? (e.g. a slider)
//! - Allowing the system to interact with the UI by dispatching actions to
//!   nodes. Note that AccessKit has its own [`Action`] type, which is not the
//!   [`crate::Action`] trait.
//! - Activate and deactivate accessibility features when requested by the
//!   system.
//!
//! Activating and deactivating at the right time is trivial, so I won't go into
//! detail here. The other two are almost orthogonal in implementation.
//!
//! The state for both lives in the [`A11y`] struct in this module.
//!
//! ### Reporting UI changes
//!
//! Every frame, we build a [`TreeUpdate`] and send it to the platform-specific
//! adapter. A [`TreeUpdate`] is a representation of a subset of the UI tree.
//! When the adapter receives the update, it diffs it against the previous
//! update, and calls platform-specific APIs to inform screen readers about the
//! changes. Nodes may have been created, destroyed, or updated.
//!
//! Each node has an ID, and this ID *should* be stable across frames. If a
//! node's ID changes, then, from AccessKit's point of view, it is a different
//! node.
//!
//! We derive the node ID from the [`GlobalElementId`] in
//! [`GlobalElementId::accesskit_node_id`]. Nodes without [`GlobalElementId`]s
//! cannot produce an AccessKit [`NodeId`], and so are not included in the
//! accessibility tree. We try to warn when using accessibility APIs on
//! [`div()`] without setting an ID.
//!
//! This all happens in [`Drawable::prepaint`]. The [`A11y`] struct maintains a
//! stack of nodes during prepainting, which we can use to calculate the
//! [`NodeId`]s, and record parent-child relationships. Once all [`Element`]s in
//! a frame have been prepainted, we send the resulting [`TreeUpdate`] object to
//! the adapter and the screen reader can announce the changes.
//!
//! #### Synthetic children
//!
//! Additionally, some nodes can register "synthetic children" using
//! [`Element::a11y_synthetic_children`]. Normally, one accesskit node is pushed
//! for every [`Element`] with a role and id. However, sometimes a single
//! element may want to produce many accesskit nodes. These extra nodes are
//! referred to as "synthetic children" of the element providing a non-default
//! [`Element::a11y_synthetic_children`] implementation.
//!
//! The user is provided a builder-style API using [`A11ySubtreeBuilder`], which
//! allows them to create push nodes that are children of the current node, as
//! well as modify the current node itself.
//!
//! GPUI calls this callback *after* prepainting (and just before popping the
//! corresponding element), since this step may need prepaint information to be
//! available. In the future, we may want to add prepaint information more
//! generally to [`Element::write_a11y_info`], but for now that's not necessary.
//!
//! ### Responding to actions
//!
//! On adapter creation, we provide a callback to the adapter, which can be used
//! to dispatch actions. This callback forwards to [`A11y::action_listeners`], a
//! mapping from [`NodeId`]s to action handlers (basically just `Box<dyn
//! Fn()>`).
//!
//! This is populated in:
//! - [`Window::on_a11y_action`], which is called by:
//! - [`Interactivity::paint`], which is called by:
//! - [`StatefulInteractiveElement::on_a11y_action`], which is a public-facing API
//!
//! These are cleared at the start of a frame, and re-populated during painting.
//!
//! [`NodeId`]: accesskit::NodeId

use crate::*;

pub(crate) mod debug;
pub(crate) mod snapshot;

use crate::{App, Bounds, FocusId, Pixels, SharedString, Window};
use accesskit::{Action, NodeId, TreeUpdate};
use collections::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use std::hash::{Hash, Hasher};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// The fixed AccessKit node ID used for the root of every window's a11y tree.
pub(crate) const ROOT_NODE_ID: NodeId = NodeId(0);

/// A listener for an accessibility action on a specific node.
pub(crate) type A11yActionListener =
    Box<dyn FnMut(Option<&accesskit::ActionData>, &mut Window, &mut App) + 'static>;

/// Per-window accessibility state.
///
/// Manages the AccessKit tree that is built each frame and the mappings
/// needed to dispatch incoming action requests back to the right elements.
pub(crate) struct A11y {
    /// Whether accessibility has been [forcibly disabled] for this window.
    ///
    /// [forcibly disabled]: crate::Application::new_inaccessible
    force_disabled: bool,
    /// Whether a11y features have been requested by the system.
    ///
    /// Updated by AccessKit using callbacks provided to the adapter. Can change
    /// halfway through a frame.
    active_flag: Arc<AtomicBool>,
    /// Whether a11y features are active for *this specific frame*.
    ///
    /// At the start of each frame, we load [`Self::active_flag`] (using
    /// [`Self::sync_active_flag`]) and use this to determine whether we
    /// should construct a [`TreeUpdate`] for this frame. It's important that
    /// this value is stable within a frame, because the builder API exposed by
    /// this type maintains a stack of nodes and each must be pushed and popped
    /// exactly once.
    ///
    /// At the end of the frame, we re-call [`Self::sync_active_flag`] to
    /// determine whether we should actually send the finished [`TreeUpdate`].
    active_this_frame: bool,
    /// Build the tree even without assistive technology, for [`Window::a11y_snapshot`].
    retain: bool,
    pub(crate) nodes: A11yNodeBuilder,
    pub(crate) focus_ids: FxHashMap<NodeId, FocusId>,
    pub(crate) node_bounds: FxHashMap<NodeId, Bounds<Pixels>>,
    pub(crate) action_listeners: FxHashMap<NodeId, Vec<(Action, A11yActionListener)>>,
    /// Hitbox of the element behind each node: an accessibility click only lands when it is the one under the point.
    pub(crate) node_hitboxes: FxHashMap<NodeId, HitboxId>,
    prev_node_hitboxes: FxHashMap<NodeId, HitboxId>,
    prev_focus_ids: FxHashMap<NodeId, FocusId>,
    prev_node_bounds: FxHashMap<NodeId, Bounds<Pixels>>,
    prev_action_listeners: FxHashMap<NodeId, Vec<(Action, A11yActionListener)>>,
    /// The window's title, used to label the root node so assistive
    /// technology can tell windows apart.
    window_title: Option<SharedString>,
    /// The focus id we most recently reported as having no accessibility node,
    /// used to log at most once per focus change rather than every frame.
    last_focus_without_node: Option<FocusId>,
    /// Retains the last tree update (and, in debug builds, per-node provenance)
    /// so it can be dumped via [`crate::Window::debug_a11y_tree_json`].
    debug: debug::A11yDebug,
    /// Maps a view's [`EntityId`] to its `Render` type name
    #[cfg(debug_assertions)]
    pub(crate) view_type_names: FxHashMap<EntityId, &'static str>,
}

impl A11y {
    pub(crate) fn new(
        active_flag: Arc<AtomicBool>,
        force_disabled: bool,
        window_title: Option<SharedString>,
    ) -> Self {
        Self {
            force_disabled,
            active_flag,
            active_this_frame: false,
            retain: false,
            nodes: A11yNodeBuilder::new(),
            focus_ids: FxHashMap::default(),
            node_bounds: FxHashMap::default(),
            action_listeners: FxHashMap::default(),
            node_hitboxes: FxHashMap::default(),
            prev_node_hitboxes: FxHashMap::default(),
            prev_focus_ids: FxHashMap::default(),
            prev_node_bounds: FxHashMap::default(),
            prev_action_listeners: FxHashMap::default(),
            window_title,
            last_focus_without_node: None,
            debug: debug::A11yDebug::default(),
            #[cfg(debug_assertions)]
            view_type_names: FxHashMap::default(),
        }
    }

    /// Logs (once per focus change) that the focused element is not exposed to
    /// assistive technology because it has no accessibility node. When this
    /// happens, screen readers fall back to announcing the whole window instead
    /// of the focused element. The fix is to give the element both an
    /// `.id(...)` and a `.role(...)`.
    pub(crate) fn note_focus_without_node(&mut self, focus_id: FocusId, reason: &str) {
        if self.last_focus_without_node != Some(focus_id) {
            self.last_focus_without_node = Some(focus_id);
            log::info!(
                "a11y: focused element ({focus_id:?}) has no accessibility node \
                 ({reason}); assistive technology will announce the whole window \
                 instead. Give it both an `.id(...)` and a `.role(...)` to expose it."
            );
        }
    }

    pub(crate) fn set_window_title(&mut self, title: impl Into<SharedString>) {
        self.window_title = Some(title.into());
    }

    /// Ensures that [`Self::is_active`] returns up to date information.
    ///
    /// See the docs for [`Self::active_flag`] and [`Self::active_this_frame`]
    /// for more commentary.
    pub(crate) fn sync_active_flag(&mut self) {
        self.active_this_frame =
            self.is_enabled() && (self.retain || self.active_flag.load(Ordering::SeqCst));
    }

    pub(crate) fn is_enabled(&self) -> bool {
        !self.force_disabled
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active_this_frame
    }

    pub(crate) fn set_focusable(&mut self, node_id: NodeId, focus_id: FocusId) {
        self.focus_ids.insert(node_id, focus_id);
    }

    /// Report `node_id` as the currently-focused node, if it is present in the
    /// tree.
    ///
    /// Must only be called once per frame.
    pub(crate) fn set_focus(&mut self, node_id: NodeId) {
        // A focused node must have been registered as focusable this frame.
        if !self.focus_ids.contains_key(&node_id) {
            if cfg!(debug_assertions) {
                panic!("set_focus called for a node that was not registered with set_focusable");
            } else {
                log::warn!(
                    "a11y: set_focus called for a node that was not registered with \
                     set_focusable ({node_id:?})"
                );
            }
        }
        if self.nodes.has_node(node_id) {
            // The focused element is properly exposed; reset the dedup so a
            // later focus on a node-less element logs again.
            self.last_focus_without_node = None;
            self.nodes.set_focus(node_id);
        } else {
            // The element registered a focus handle and an id, but never got a
            // node because it has no role.
            if let Some(focus_id) = self.focus_ids.get(&node_id).copied() {
                self.note_focus_without_node(focus_id, "it has an id but no role");
            }
        }
    }

    pub(crate) fn set_active_descendant(&mut self, node_id: NodeId) {
        // The active descendant must be a descendant of the focused container,
        // not the focused node itself.
        if self.nodes.node_is_focused(node_id) {
            if cfg!(debug_assertions) {
                panic!("set_active_descendant called on the focused node");
            } else {
                log::warn!("a11y: set_active_descendant called on the focused node ({node_id:?})");
            }
            return;
        }
        if self.nodes.has_node(node_id) && self.nodes.focus_is_ancestor_of_current() {
            self.nodes.set_active_descendant(node_id);
        }
    }

    /// Clear per-frame state and push the root node to start a new frame.
    pub(crate) fn begin_frame(&mut self) {
        // O quadro anterior fica guardado para as views em cache, que não repintam e reaproveitam o que registraram nele.
        self.prev_focus_ids = std::mem::take(&mut self.focus_ids);
        self.prev_node_bounds = std::mem::take(&mut self.node_bounds);
        self.prev_action_listeners = std::mem::take(&mut self.action_listeners);
        self.prev_node_hitboxes = std::mem::take(&mut self.node_hitboxes);
        self.nodes.begin_frame(self.window_title.as_ref());
    }

    /// Marks where a cached view starts emitting nodes, so its subtree can be replayed on later frames.
    pub(crate) fn capture_start(&self) -> Option<(usize, usize, u32)> {
        let parent = self.nodes.nodes_stack.last()?;
        Some((self.nodes.all_nodes.len(), parent.children().len(), self.nodes.text_count()))
    }

    pub(crate) fn capture_end(&self, start: (usize, usize, u32)) -> A11yCapture {
        let roots = self
            .nodes
            .nodes_stack
            .last()
            .map(|parent| parent.children()[start.1..].to_vec())
            .unwrap_or_default();
        A11yCapture {
            nodes: self.nodes.all_nodes[start.0..].to_vec(),
            roots,
            texts: self.nodes.text_count() - start.2,
        }
    }

    /// Re-emits the subtree of a cached view that was reused without running prepaint/paint.
    pub(crate) fn replay(&mut self, capture: &A11yCapture, focused: Option<FocusId>) {
        let mut inserted = FxHashSet::default();
        for (id, node) in &capture.nodes {
            if !self.nodes.seen_ids.insert(*id) {
                continue;
            }
            inserted.insert(*id);
            self.nodes.all_nodes.push((*id, node.clone()));
            if let Some(bounds) = self.prev_node_bounds.remove(id) {
                self.node_bounds.insert(*id, bounds);
            }
            if let Some(hitbox) = self.prev_node_hitboxes.remove(id) {
                self.node_hitboxes.insert(*id, hitbox);
            }
            if let Some(listeners) = self.prev_action_listeners.remove(id) {
                self.action_listeners.insert(*id, listeners);
            }
            if let Some(focus_id) = self.prev_focus_ids.remove(id) {
                self.focus_ids.insert(*id, focus_id);
                if focused == Some(focus_id) {
                    self.set_focus(*id);
                }
            }
        }
        // Os textos que vierem depois da view seguem numerados como no quadro em que ela desenhou.
        if let Some(count) = self.nodes.text_counts.last_mut() {
            *count += capture.texts;
        }
        if let Some(parent) = self.nodes.nodes_stack.last_mut() {
            for root in &capture.roots {
                if inserted.contains(root) {
                    parent.push_child(*root);
                }
            }
        }
    }

    /// Finalize the tree and produce a [`TreeUpdate`] for the platform adapter.
    pub(crate) fn end_frame(&mut self, frame: debug::FrameDebugInfo) -> TreeUpdate {
        let update = self.nodes.finalize();
        self.debug.capture(
            &update,
            self.nodes.focus,
            self.nodes.active_descendant,
            self.window_title.as_ref(),
            frame,
        );
        #[cfg(debug_assertions)]
        self.debug.capture_node_info(&self.nodes.node_info);
        update
    }

    pub(crate) fn debug_tree_json(&self) -> Option<String> {
        self.debug.to_json()
    }
}

/// Enough of a control's visible text to name it.
const CONTENT_CAP: usize = 300;

/// Roles that take their name from the text inside them when nobody gave them a label.
fn named_by_content(role: accesskit::Role) -> bool {
    use accesskit::Role::*;
    matches!(
        role,
        Button | Tab | MenuItem | MenuItemCheckBox | MenuItemRadio | ListItem | ListBoxOption | TreeItem | Link | Cell
            | Row | CheckBox | RadioButton | Switch | Heading | ColumnHeader | RowHeader | Term | Definition | Tooltip
    )
}

/// The text is already the parent's name or value: a `Label`/`Text` node, or a named control that reads as its label.
fn text_belongs_to_parent(parent: &accesskit::Node) -> bool {
    use accesskit::Role::*;
    match parent.role() {
        Label | TextInput | MultilineTextInput | SearchInput | PasswordInput => true,
        Button | Tab | MenuItem | MenuItemCheckBox | MenuItemRadio | CheckBox | Switch | RadioButton | Link
        | ComboBox | ListBoxOption | TreeItem | ListItem => parent.label().is_some(),
        _ => false,
    }
}

impl Window {
    /// Exposes visible text as a `Label` leaf of the enclosing node. GPUI's text elements call it; custom elements that
    /// paint text themselves call it from `prepaint` with the text and its bounds.
    pub fn a11y_text(&mut self, text: &str, bounds: Bounds<Pixels>) {
        // Texto fora da área rolada também entra: está desenhado e o leitor chega nele rolando, como num navegador.
        if !self.a11y.is_active() {
            return;
        }
        let scale = self.scale_factor();
        self.a11y.nodes.push_text(
            text,
            accesskit::Rect {
                x0: (bounds.origin.x.0 * scale) as f64,
                y0: (bounds.origin.y.0 * scale) as f64,
                x1: ((bounds.origin.x.0 + bounds.size.width.0) * scale) as f64,
                y1: ((bounds.origin.y.0 + bounds.size.height.0) * scale) as f64,
            },
        );
    }

    /// The accessibility tree of the last frame as text, one node per line, indented by depth:
    /// `role "name" = "value" [states] #id`. `root` limits it to the subtree of the node whose id is `root` or ends
    /// with `›root` (e.g. `settings-dialog`). `None` while no tree was built (no assistive technology and
    /// [`Self::retain_a11y_tree`] off) or when `root` is not on screen.
    pub fn a11y_snapshot(&self, root: Option<&str>) -> Option<String> {
        self.a11y.debug.snapshot(root)
    }

    /// Role and name of the node `id` (or `…›id`) in the last frame. Errors: `no-tree`, `not-found`, `ambiguous`.
    pub fn a11y_target(&self, id: &str) -> Result<(String, Option<String>), &'static str> {
        let (_, node) = self.a11y.debug.node(id)?;
        Ok((format!("{:?}", node.role()), node.label().map(str::to_owned)))
    }

    /// Clicks the node `id` of the last frame by the same path as a screen reader. Besides the [`Self::a11y_target`]
    /// errors: `disabled`, and `covered` when the click would land on another element (popup, dialog, scrolled out).
    #[cfg(not(target_family = "wasm"))]
    pub fn a11y_click(&mut self, id: &str, cx: &mut App) -> Result<(), &'static str> {
        let (node_id, node) = self.a11y.debug.node(id)?;
        if node.is_disabled() {
            return Err("disabled");
        }
        let listens = self.a11y.action_listeners.get(&node_id).is_some_and(|l| l.iter().any(|(a, _)| *a == Action::Click));
        if !listens {
            // The built-in click drops itself with only a log line when it would miss: say it here instead.
            let bounds = self.a11y.node_bounds.get(&node_id).copied().ok_or("not-found")?;
            let center = bounds.center();
            let reachable = self.a11y.node_hitboxes.get(&node_id).map_or(true, |hitbox| {
                let hit = self.rendered_frame.hit_test(center);
                hit.ids.iter().take(hit.hover_hitbox_count).any(|id| id == hitbox)
            });
            if !reachable {
                return Err("covered");
            }
        }
        self.handle_a11y_action(
            accesskit::ActionRequest { action: Action::Click, target_tree: accesskit::TreeId::ROOT, target_node: node_id, data: None },
            cx,
        );
        Ok(())
    }

    /// Keeps building the accessibility tree without assistive technology attached, so [`Self::a11y_snapshot`]
    /// always has the last frame.
    pub fn retain_a11y_tree(&mut self, retain: bool) {
        self.a11y.retain = retain;
        self.refresh();
    }
}

/// Nodes a cached view emitted on the frame it last rendered.
pub(crate) struct A11yCapture {
    nodes: Vec<(NodeId, accesskit::Node)>,
    /// Top-level nodes of the view, children of whatever node encloses it.
    roots: Vec<NodeId>,
    /// Text leaves the view numbered under the enclosing node.
    texts: u32,
}

impl A11yCapture {
    pub(crate) fn roots(&self) -> Vec<NodeId> {
        self.roots.clone()
    }
}

/// Builder API for synthetic children. See the docs for
/// [`Element::a11y_synthetic_children`].
pub struct A11ySubtreeBuilder<'a> {
    parent_id: NodeId,
    nodes: &'a mut A11yNodeBuilder,
    /// Provenance of the real element whose `a11y_synthetic_children` is
    /// running.
    #[cfg(debug_assertions)]
    creator: debug::NodeCreator,
}

impl<'a> A11ySubtreeBuilder<'a> {
    pub(crate) fn new(parent_id: NodeId, nodes: &'a mut A11yNodeBuilder) -> Self {
        Self {
            parent_id,
            nodes,
            #[cfg(debug_assertions)]
            creator: debug::NodeCreator::default(),
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn with_creator(mut self, creator: debug::NodeCreator) -> Self {
        self.creator = creator;
        self
    }

    /// Derive a [`NodeId`] for a synthetic child.
    ///
    /// The generated ID is based on the hash of `key`, as well as the parent's
    /// ID. This means that `key`s must be unique within the same
    /// [`Element::a11y_synthetic_children`] call, but may be duplicated across
    /// different calls.
    pub fn synthetic_node_id(&self, key: impl Hash) -> NodeId {
        let mut hasher = std::hash::DefaultHasher::default();
        self.parent_id.0.hash(&mut hasher);
        key.hash(&mut hasher);
        NodeId(hasher.finish())
    }

    /// Append a synthetic leaf node as a child of this element's node.
    ///
    /// Returns `false` if a node with this id is already present in the tree,
    /// in which case the node is discarded.
    pub fn push_child(&mut self, id: NodeId, node: accesskit::Node) -> bool {
        let pushed = self.nodes.push_leaf(id, node);
        #[cfg(debug_assertions)]
        if pushed {
            self.nodes.record_node_info(
                id,
                debug::NodeDebugInfo {
                    synthetic: true,
                    view: self.creator.view,
                    element_id: self.creator.element_id.clone(),
                    source_location: self.creator.source_location,
                },
            );
        }
        pushed
    }

    /// A mutable reference to the parent node.
    pub fn parent_node(&mut self) -> &mut accesskit::Node {
        self.nodes
            .current_node_mut()
            .expect("A11ySubtreeBuilder exists only while its element's node is on the stack")
    }
}

pub(crate) struct A11yNodeBuilder {
    ids_stack: SmallVec<[NodeId; 16]>,
    nodes_stack: SmallVec<[accesskit::Node; 16]>,
    /// This is the exact type required by accesskit, so we can't just make it a
    /// `HashMap<NodeId, Node>` to remove the need for `seen_ids`
    all_nodes: Vec<(NodeId, accesskit::Node)>,
    seen_ids: FxHashSet<NodeId>,
    /// Text leaves already numbered under each node on the stack (parallel to `ids_stack`).
    text_counts: SmallVec<[u32; 16]>,
    /// Visible text gathered under each node on the stack, capped, to name controls that have no label.
    contents: SmallVec<[String; 16]>,
    /// Whether each node on the stack already has a child that is a control rather than text.
    has_controls: SmallVec<[bool; 16]>,
    /// Nodes a deferred draw emitted at the root, to hang under the node that deferred them.
    reparents: Vec<(NodeId, Vec<NodeId>)>,
    /// The node that GPUI considers focused. Note that this may be different to
    /// what is reported to accesskit - see [`Self::active_descendant`]
    focus: Option<NodeId>,
    /// If a node calls `.aria_active_descendant()`, AND an ancestor is focused,
    /// override it as the focused node. This supports the "active descendant"
    /// pattern, which allows a focused container to act as if a descendant is
    /// focused.
    active_descendant: Option<NodeId>,
    #[cfg(debug_assertions)]
    node_info: FxHashMap<NodeId, debug::NodeDebugInfo>,
}

impl A11yNodeBuilder {
    fn new() -> Self {
        Self {
            ids_stack: SmallVec::new(),
            nodes_stack: SmallVec::new(),
            all_nodes: Vec::new(),
            seen_ids: FxHashSet::default(),
            text_counts: SmallVec::new(),
            contents: SmallVec::new(),
            has_controls: SmallVec::new(),
            reparents: Vec::new(),
            focus: None,
            active_descendant: None,
            #[cfg(debug_assertions)]
            node_info: FxHashMap::default(),
        }
    }

    /// Records provenance for a node already pushed this frame. Debug builds only.
    #[cfg(debug_assertions)]
    pub(crate) fn record_node_info(&mut self, id: NodeId, info: debug::NodeDebugInfo) {
        self.node_info.insert(id, info);
    }

    #[must_use]
    fn can_push(&mut self, id: NodeId) -> bool {
        debug_assert!(!self.ids_stack.is_empty(), "node pushed before push_root");

        if !self.seen_ids.insert(id) {
            debug_assert!(
                false,
                "Duplicate a11y node id: {id:?}. In a release build, this node would be silently discarded from the a11y tree."
            );
            return false;
        }

        true
    }

    /// Push a new node onto the stack. It becomes a child of the current
    /// top-of-stack node.
    ///
    /// Returns `true` if the node was successfully pushed.
    pub(crate) fn push(&mut self, id: NodeId, node: accesskit::Node) -> bool {
        if !self.can_push(id) {
            return false;
        }

        if let Some(parent) = self.nodes_stack.last_mut() {
            parent.push_child(id);
        }
        self.ids_stack.push(id);
        self.nodes_stack.push(node);
        self.text_counts.push(0);
        self.contents.push(String::new());
        self.has_controls.push(false);
        true
    }

    /// Add a leaf node as a child of the current top-of-stack node, without
    /// pushing it onto the stack. Semantically equivalent to a [`Self::push`]
    /// followed by a [`Self::pop`].
    ///
    /// Returns `true` if the node was successfully pushed.
    pub(crate) fn push_leaf(&mut self, id: NodeId, node: accesskit::Node) -> bool {
        if !self.can_push(id) {
            return false;
        }

        if let Some(parent) = self.nodes_stack.last_mut() {
            parent.push_child(id);
        }
        self.all_nodes.push((id, node));
        true
    }

    pub(crate) fn current_node_mut(&mut self) -> Option<&mut accesskit::Node> {
        self.nodes_stack.last_mut()
    }

    /// Pop the current node off the stack and finalize it into the all_nodes
    /// list.
    pub(crate) fn pop(&mut self) {
        debug_assert!(self.ids_stack.len() > 1, "pop would remove the root node");

        if let (Some(id), Some(mut node)) = (self.ids_stack.pop(), self.nodes_stack.pop()) {
            let content = self.contents.pop().unwrap_or_default();
            let content = content.trim();
            let has_controls = self.has_controls.pop().unwrap_or(false);
            let clickable = node.supports_action(accesskit::Action::Click);
            let unnamed = node.label().is_none();
            // Área clicável que embrulha controles é nomeada por eles, não pela soma dos textos.
            if unnamed && !content.is_empty() && (named_by_content(node.role()) || clickable) && !(clickable && has_controls) {
                node.set_label(content.to_string());
            }
            // Botão sem nome do app que embrulha outros controles não é botão para o leitor (ele achata os filhos):
            // vira grupo, e continua clicável pela ação.
            if unnamed && node.role() == accesskit::Role::Button && has_controls {
                node.set_role(accesskit::Role::Group);
            }
            if node.role() != accesskit::Role::Label {
                if let Some(parent) = self.has_controls.last_mut() {
                    *parent = true;
                }
            }
            let said = node.label().or(node.value()).unwrap_or(content).to_string();
            self.gather(&said);
            self.all_nodes.push((id, node));
        }
        self.text_counts.pop();
    }

    fn gather(&mut self, text: &str) {
        if let Some(content) = self.contents.last_mut() {
            if !content.is_empty() && content.len() < CONTENT_CAP {
                content.push(' ');
            }
            for ch in text.chars() {
                if content.len() >= CONTENT_CAP {
                    break;
                }
                content.push(ch);
            }
        }
    }

    /// The node elements are being nested under right now.
    pub(crate) fn current_id(&self) -> Option<NodeId> {
        self.ids_stack.last().copied()
    }

    /// `kids`, emitted at the root by a deferred draw, belong under `parent`.
    pub(crate) fn reparent(&mut self, parent: NodeId, kids: Vec<NodeId>) {
        if parent != ROOT_NODE_ID && !kids.is_empty() {
            self.reparents.push((parent, kids));
        }
    }

    fn apply_reparents(&mut self) {
        if self.reparents.is_empty() {
            return;
        }
        let index: FxHashMap<NodeId, usize> =
            self.all_nodes.iter().enumerate().map(|(i, (id, _))| (*id, i)).collect();
        let Some(&root) = index.get(&ROOT_NODE_ID) else { return };
        for (parent, kids) in std::mem::take(&mut self.reparents) {
            // Pai fora da árvore (não desenhado neste quadro): os filhos ficam na raiz.
            let Some(&at) = index.get(&parent) else { continue };
            let root_children = self.all_nodes[root].1.children().to_vec();
            let moved: Vec<NodeId> = kids.into_iter().filter(|kid| root_children.contains(kid)).collect();
            if moved.is_empty() {
                continue;
            }
            self.all_nodes[root].1.set_children(root_children.into_iter().filter(|c| !moved.contains(c)).collect::<Vec<_>>());
            // Diálogo sem nome: o título chega no conteúdo adiado, e é o primeiro texto dele.
            let parent_node = &self.all_nodes[at].1;
            if matches!(parent_node.role(), accesskit::Role::Dialog | accesskit::Role::AlertDialog) && parent_node.label().is_none() {
                let title = moved.iter().find_map(|kid| {
                    let node = &self.all_nodes[*index.get(kid)?].1;
                    (node.role() == accesskit::Role::Label).then(|| node.value().map(str::to_owned)).flatten()
                });
                if let Some(title) = title {
                    self.all_nodes[at].1.set_label(title);
                }
            }
            for kid in moved {
                self.all_nodes[at].1.push_child(kid);
            }
        }
    }

    pub(crate) fn text_count(&self) -> u32 {
        self.text_counts.last().copied().unwrap_or(0)
    }

    /// Adds visible text as a `Label` leaf of the current node. The id comes from the parent and the text's position
    /// among the parent's texts, so it stays the same across frames while the text changes.
    pub(crate) fn push_text(&mut self, text: &str, bounds: accesskit::Rect) {
        let (Some(&parent_id), Some(parent)) = (self.ids_stack.last(), self.nodes_stack.last()) else {
            return;
        };
        if text.trim().is_empty() || text_belongs_to_parent(parent) {
            return;
        }
        // Diálogo sem nome: o primeiro texto dele é o título.
        if matches!(parent.role(), accesskit::Role::Dialog | accesskit::Role::AlertDialog) && parent.label().is_none() {
            if let Some(dialog) = self.nodes_stack.last_mut() {
                dialog.set_label(text.trim().to_string());
            }
        }
        let Some(count) = self.text_counts.last_mut() else {
            return;
        };
        let position = *count;
        *count += 1;
        let mut hasher = std::hash::DefaultHasher::default();
        (parent_id.0, "text", position).hash(&mut hasher);
        let mut node = accesskit::Node::new(accesskit::Role::Label);
        node.set_value(text.to_string());
        node.set_bounds(bounds);
        if self.push_leaf(NodeId(hasher.finish()), node) {
            self.gather(text);
        }
    }

    /// Push the root node to start a new frame.
    fn begin_frame(&mut self, window_title: Option<&SharedString>) {
        self.all_nodes.clear();
        self.ids_stack.clear();
        self.nodes_stack.clear();
        self.seen_ids.clear();
        #[cfg(debug_assertions)]
        self.node_info.clear();
        let mut root_node = accesskit::Node::new(accesskit::Role::Window);
        if let Some(title) = window_title {
            root_node.set_label(title.to_string());
        }

        self.ids_stack.push(ROOT_NODE_ID);
        self.nodes_stack.push(root_node);
        self.text_counts.clear();
        self.text_counts.push(0);
        self.contents.clear();
        self.contents.push(String::new());
        self.has_controls.clear();
        self.has_controls.push(false);
        self.reparents.clear();
        self.focus = None;
        self.active_descendant = None;
    }

    /// Returns whether a node with the given ID has been pushed in this frame.
    pub(crate) fn has_node(&self, id: NodeId) -> bool {
        id == ROOT_NODE_ID || self.seen_ids.contains(&id)
    }

    /// Returns whether `id` is the node currently reported as focused.
    pub(crate) fn node_is_focused(&self, id: NodeId) -> bool {
        self.focus == Some(id)
    }

    pub(crate) fn focus_is_ancestor_of_current(&self) -> bool {
        let Some(focus) = self.focus else {
            return false;
        };

        // The current node is on top of the stack; everything below it is an
        // ancestor.
        let ancestor_count = self.ids_stack.len().saturating_sub(1);
        self.ids_stack[..ancestor_count].contains(&focus)
    }

    pub(crate) fn set_active_descendant(&mut self, id: NodeId) {
        if self
            .active_descendant
            .is_some_and(|existing| existing != id)
        {
            if cfg!(debug_assertions) {
                panic!("active descendant claimed by multiple nodes in one frame");
            } else {
                log::warn!(
                    "a11y: multiple nodes claimed the active descendant this frame; \
                     using last-wins ({id:?})"
                );
            }
        }
        self.active_descendant = Some(id);
    }

    pub(crate) fn set_focus(&mut self, id: NodeId) {
        if self.focus.is_some() {
            if cfg!(debug_assertions) {
                panic!("set_focus called more than once in a single frame");
            } else {
                log::warn!(
                    "a11y: set_focus called more than once in a single frame; \
                     using last-wins ({id:?})"
                );
            }
        }
        self.focus = Some(id);
    }

    fn finalize(&mut self) -> TreeUpdate {
        // Stack should contain only the root node
        debug_assert_eq!(self.ids_stack.len(), 1);
        debug_assert_eq!(self.ids_stack[0], ROOT_NODE_ID);

        if self.ids_stack.len() != 1 {
            log::error!(
                "a11y: Stack imbalance at end of frame: expected 1 (root), got {}. \
                 Some elements may have pushed without popping.",
                self.ids_stack.len()
            );
        }

        // Pop remaining nodes (should just be the root).
        while !self.ids_stack.is_empty() {
            if let (Some(id), Some(node)) = (self.ids_stack.pop(), self.nodes_stack.pop()) {
                self.all_nodes.push((id, node));
            }
        }
        self.apply_reparents();

        let focus = match self.active_descendant {
            Some(id) if self.has_node(id) => id,
            Some(id) => {
                if cfg!(debug_assertions) {
                    panic!("active_descendant set to {id:?}, which is not in the tree");
                } else {
                    log::warn!("active_descendant set to {id:?}, which is not in the tree");
                    self.focus.unwrap_or(ROOT_NODE_ID)
                }
            }

            _ => self.focus.unwrap_or(ROOT_NODE_ID),
        };

        let nodes = std::mem::take(&mut self.all_nodes);
        let update = TreeUpdate {
            nodes,
            tree: Some(accesskit::Tree::new(ROOT_NODE_ID)),
            tree_id: accesskit::TreeId::ROOT,
            focus,
        };

        Self::repair_tree_update(update)
    }

    /// Accesskit panics on invalid [`TreeUpdate`]s. This function defensively
    /// checks invariants that accesskit panics on, and tries to fix them.
    fn repair_tree_update(mut update: TreeUpdate) -> TreeUpdate {
        let node_ids: FxHashSet<NodeId> = update.nodes.iter().map(|(id, _)| *id).collect();

        // Focus must point to a node in the tree.
        if !node_ids.contains(&update.focus) {
            log::error!(
                "a11y: Focused node {:?} is not in the tree ({} nodes). \
                 Falling back to root. This is a bug in the a11y tree builder.",
                update.focus,
                update.nodes.len()
            );
            update.focus = ROOT_NODE_ID;
        }

        // Every child reference must point to a node in the update.
        for (id, node) in &mut update.nodes {
            let has_invalid_child = node
                .children()
                .iter()
                .any(|child_id| !node_ids.contains(child_id));
            if has_invalid_child {
                let children = node.children();
                let invalid_count = children
                    .iter()
                    .filter(|child_id| !node_ids.contains(child_id))
                    .count();
                log::error!(
                    "a11y: Node {:?} references {} children not present in the tree. \
                     Stripping invalid child references.",
                    id,
                    invalid_count
                );
                let valid: Vec<NodeId> = children
                    .iter()
                    .copied()
                    .filter(|child_id| node_ids.contains(child_id))
                    .collect();
                node.set_children(valid);
            }
        }

        update
    }
}

#[cfg(test)]
mod tests {
    // Import specific items rather than glob-importing `super`, which would pull
    // in gpui's own `test` attribute macro and shadow the standard one.
    use super::{A11y, A11yNodeBuilder, ROOT_NODE_ID, snapshot};
    use crate::FocusId;
    use accesskit::{NodeId, Role};
    use std::sync::{Arc, atomic::AtomicBool};

    fn test_node() -> accesskit::Node {
        accesskit::Node::new(Role::GenericContainer)
    }

    fn new_builder() -> A11yNodeBuilder {
        let mut builder = A11yNodeBuilder::new();
        builder.begin_frame(None);
        builder
    }

    fn new_a11y() -> A11y {
        let mut a11y = A11y::new(Arc::new(AtomicBool::new(true)), false, None);
        a11y.begin_frame();
        a11y
    }

    fn rect() -> accesskit::Rect {
        accesskit::Rect { x0: 0., y0: 0., x1: 10., y1: 10. }
    }

    fn text_frame(a11y: &mut A11y, words: &[&str]) -> accesskit::TreeUpdate {
        a11y.begin_frame();
        let mut dialog = test_node();
        dialog.set_author_id("root›settings-dialog");
        assert!(a11y.nodes.push(NodeId(1), dialog));
        for word in words {
            a11y.nodes.push_text(word, rect());
        }
        let mut named = accesskit::Node::new(Role::Button);
        named.set_label("Salvar");
        assert!(a11y.nodes.push(NodeId(2), named));
        a11y.nodes.push_text("Salvar", rect());
        a11y.nodes.pop();
        assert!(a11y.nodes.push(NodeId(3), accesskit::Node::new(Role::ListItem)));
        a11y.nodes.push_text("Opus 5.5", rect());
        a11y.nodes.pop();
        a11y.nodes.pop();
        a11y.end_frame(Default::default())
    }

    #[test]
    fn visible_text_becomes_stable_label_leaves() {
        let mut a11y = new_a11y();
        let first = text_frame(&mut a11y, &["Avançado", "Porta 8765"]);
        let second = text_frame(&mut a11y, &["Avançado", "Porta 9000"]);
        let labels = |update: &accesskit::TreeUpdate| -> Vec<(NodeId, String)> {
            update.nodes.iter().filter(|(_, n)| n.role() == Role::Label)
                .map(|(id, n)| (*id, n.value().unwrap().to_string())).collect()
        };
        let (a, b) = (labels(&first), labels(&second));
        // Texto dentro de botão com nome não vira folha; item sem nome ganha folha e o texto como nome.
        assert_eq!(a.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["Avançado", "Porta 8765", "Opus 5.5"]);
        assert_eq!(a.iter().map(|(id, _)| *id).collect::<Vec<_>>(), b.iter().map(|(id, _)| *id).collect::<Vec<_>>());
        assert_eq!(b[1].1, "Porta 9000");
        let item = first.nodes.iter().find(|(id, _)| *id == NodeId(3)).unwrap();
        assert_eq!(item.1.label(), Some("Opus 5.5"));
    }

    #[test]
    fn snapshot_lists_the_subtree_one_line_per_node() {
        let mut a11y = new_a11y();
        let update = text_frame(&mut a11y, &["Avançado"]);
        let all = snapshot::snapshot_text(&update, None).unwrap();
        assert!(all.starts_with("Window\n"));
        let dialog = snapshot::snapshot_text(&update, Some("settings-dialog")).unwrap();
        assert_eq!(
            dialog,
            "GenericContainer #settings-dialog\n  Label = \"Avançado\"\n  Button \"Salvar\"\n  ListItem \"Opus 5.5\"\n    Label = \"Opus 5.5\"\n"
        );
        assert_eq!(snapshot::snapshot_text(&update, Some("missing")), None);
    }

    #[test]
    fn cached_subtree_replays_on_later_frames() {
        let mut a11y = new_a11y();
        let container = NodeId(1);
        let button = NodeId(2);
        let focus = FocusId::default();

        assert!(a11y.nodes.push(container, test_node()));
        let start = a11y.capture_start().unwrap();
        assert!(a11y.nodes.push(button, test_node()));
        a11y.set_focusable(button, focus);
        a11y.nodes.pop();
        let capture = a11y.capture_end(start);
        a11y.nodes.pop();
        a11y.end_frame(Default::default());

        for _ in 0..2 {
            a11y.begin_frame();
            assert!(a11y.nodes.push(container, test_node()));
            a11y.replay(&capture, Some(focus));
            a11y.nodes.pop();
            let update = a11y.end_frame(Default::default());

            let parent = update.nodes.iter().find(|(id, _)| *id == container).unwrap();
            assert_eq!(parent.1.children(), &[button]);
            assert!(update.nodes.iter().any(|(id, _)| *id == button));
            assert_eq!(update.focus, button);
            assert_eq!(a11y.focus_ids.get(&button), Some(&focus));
        }
    }

    #[test]
    fn accessibility_enabled_is_independent_of_activation() {
        for force_disabled in [false, true] {
            for active in [false, true] {
                let mut a11y = A11y::new(Arc::new(AtomicBool::new(active)), force_disabled, None);
                a11y.sync_active_flag();

                assert_eq!(a11y.is_enabled(), !force_disabled);
                assert_eq!(a11y.is_active(), !force_disabled && active);
            }
        }
    }

    #[test]
    fn active_descendant_honored_when_container_focused() {
        let mut builder = new_builder();
        let container = NodeId(1);
        let item = NodeId(2);

        assert!(builder.push(container, test_node()));
        builder.set_focus(container);
        assert!(builder.push(item, test_node()));

        // The item is on top of the stack; the focused container is its
        // ancestor, so the claim is honored.
        assert!(builder.focus_is_ancestor_of_current());
        builder.set_active_descendant(item);

        builder.pop(); // item
        builder.pop(); // container
        let update = builder.finalize();
        assert_eq!(update.focus, item);
    }

    #[test]
    fn active_descendant_honored_for_deep_descendant() {
        let mut builder = new_builder();
        let container = NodeId(1);
        let group = NodeId(2);
        let item = NodeId(3);

        assert!(builder.push(container, test_node()));
        builder.set_focus(container);
        assert!(builder.push(group, test_node()));
        assert!(builder.push(item, test_node()));

        // The item is a grandchild of the focused container; depth doesn't
        // matter, the focused ancestor is still on the stack.
        assert!(builder.focus_is_ancestor_of_current());
        builder.set_active_descendant(item);

        builder.pop(); // item
        builder.pop(); // group
        builder.pop(); // container
        let update = builder.finalize();
        assert_eq!(update.focus, item);
    }

    #[test]
    fn active_descendant_ignored_when_focus_in_other_subtree() {
        let mut builder = new_builder();
        let focused_container = NodeId(1);
        let focused_leaf = NodeId(2);
        let other_container = NodeId(3);
        let other_item = NodeId(4);

        // First subtree holds real focus.
        assert!(builder.push(focused_container, test_node()));
        assert!(builder.push(focused_leaf, test_node()));
        builder.set_focus(focused_leaf);
        builder.pop(); // focused_leaf
        builder.pop(); // focused_container

        // Second subtree: its item would claim the active descendant, but the
        // focus is not on any of its ancestors, so the gate rejects it.
        assert!(builder.push(other_container, test_node()));
        assert!(builder.push(other_item, test_node()));
        assert!(!builder.focus_is_ancestor_of_current());
        builder.pop(); // other_item
        builder.pop(); // other_container

        let update = builder.finalize();
        assert_eq!(update.focus, focused_leaf);
    }

    #[test]
    fn active_descendant_ignored_when_nothing_focused() {
        let mut builder = new_builder();
        let container = NodeId(1);
        let item = NodeId(2);

        assert!(builder.push(container, test_node()));
        assert!(builder.push(item, test_node()));

        // Nothing is focused (focus defaults to the root window node), so the
        // gate rejects the claim.
        assert!(!builder.focus_is_ancestor_of_current());
        builder.pop();
        builder.pop();

        let update = builder.finalize();
        assert_eq!(update.focus, ROOT_NODE_ID);
    }

    #[test]
    fn regular_focus_used_when_no_active_descendant() {
        let mut builder = new_builder();
        let focused = NodeId(1);

        assert!(builder.push(focused, test_node()));
        builder.set_focus(focused);
        builder.pop();

        let update = builder.finalize();
        assert_eq!(update.focus, focused);
    }

    #[test]
    fn focus_is_ancestor_excludes_self_and_non_ancestors() {
        let mut builder = new_builder();
        let container = NodeId(1);
        let item = NodeId(2);

        assert!(builder.push(container, test_node()));
        builder.set_focus(container);

        // With the focused container itself on top, it is not its own (strict)
        // ancestor, so the gate is false.
        assert!(!builder.focus_is_ancestor_of_current());

        assert!(builder.push(item, test_node()));
        // Now the focused container is a strict ancestor of the item on top.
        assert!(builder.focus_is_ancestor_of_current());

        builder.pop();
        builder.pop();
    }

    // The double-claim guard panics only in debug builds; in release it falls
    // back to last-wins with a warning.
    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "active descendant claimed by multiple nodes")
    )]
    fn multiple_active_descendant_claims_panic_in_debug() {
        let mut builder = new_builder();
        builder.set_active_descendant(NodeId(1));
        builder.set_active_descendant(NodeId(2));
    }

    // Setting focus twice in one frame means two elements both claimed window
    // focus; that panics in debug and falls back to last-wins in release.
    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "set_focus called more than once")
    )]
    fn setting_focus_twice_panics_in_debug() {
        let mut builder = new_builder();
        builder.set_focus(NodeId(1));
        builder.set_focus(NodeId(2));
    }

    // Focusing a node that was never registered as focusable is a bug: panic in
    // debug, warn in release.
    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "was not registered with set_focusable")
    )]
    fn set_focus_without_set_focusable() {
        let mut a11y = new_a11y();
        let node = NodeId(1);
        assert!(a11y.nodes.push(node, test_node()));
        // set_focusable was never called for `node`.
        a11y.set_focus(node);
    }

    // The focused node cannot also be its own active descendant: panic in
    // debug, warn in release.
    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "on the focused node"))]
    fn set_active_descendant_on_focused_node() {
        let mut a11y = new_a11y();
        let node = NodeId(1);
        assert!(a11y.nodes.push(node, test_node()));
        a11y.set_focusable(node, FocusId::default());
        a11y.set_focus(node);
        a11y.set_active_descendant(node);
    }

    // Two sibling children of a focused container both claim the active
    // descendant (both pass the focus gate). The second claim is a bug: panic
    // in debug, last-wins + warn in release.
    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "active descendant claimed by multiple nodes")
    )]
    fn two_siblings_claiming_active_descendant() {
        let mut a11y = new_a11y();
        let container = NodeId(1);
        let first = NodeId(2);
        let second = NodeId(3);

        assert!(a11y.nodes.push(container, test_node()));
        a11y.set_focusable(container, FocusId::default());
        a11y.set_focus(container);

        assert!(a11y.nodes.push(first, test_node()));
        a11y.set_active_descendant(first);
        a11y.nodes.pop(); // first

        assert!(a11y.nodes.push(second, test_node()));
        a11y.set_active_descendant(second);
        a11y.nodes.pop(); // second

        a11y.nodes.pop(); // container
    }

    // Node A is focused; node C (a child of the unfocused node B) claims the
    // active descendant. The final tree must still report A as focused.
    #[test]
    fn active_descendant_in_unfocused_subtree_keeps_real_focus() {
        let mut a11y = new_a11y();
        let a = NodeId(1);
        let b = NodeId(2);
        let c = NodeId(3);

        assert!(a11y.nodes.push(a, test_node()));
        a11y.set_focusable(a, FocusId::default());
        a11y.set_focus(a);
        a11y.nodes.pop(); // a

        assert!(a11y.nodes.push(b, test_node()));
        assert!(a11y.nodes.push(c, test_node()));
        a11y.set_active_descendant(c);
        a11y.nodes.pop(); // c
        a11y.nodes.pop(); // b

        let update = a11y.end_frame(Default::default());
        assert_eq!(update.focus, a);
    }
}
