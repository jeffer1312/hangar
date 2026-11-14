//! Largura do detalhe, alça e cartão que acompanha o ponteiro.
use super::*;

pub(crate) fn detail_width(saved: Option<f32>,available: f32,expanded: bool) -> f32 {
    let available=available.max(1.);
    if expanded {return available;}
    let minimum=available.min(320.);
    let maximum=(available-250.).max(minimum);
    saved.unwrap_or(480.).clamp(minimum,maximum)
}

#[derive(Clone)]
pub(super) struct TicketDrag {
    pub(super) server: String,
    pub(super) connector: String,
    pub(super) ticket: String,
    pub(super) card: Ticket,
    pub(super) waiting: bool,
    pub(super) evidence: usize,
    pub(super) session: Option<SessionInfo>,
    pub(super) review: Option<Review>,
}

pub(super) struct TicketDragChip {pub(super) drag: TicketDrag}
impl Render for TicketDragChip {
    fn render(&mut self,_: &mut Window,_: &mut Context<Self>) -> impl IntoElement {
        let drag=&self.drag;
        ticket_card_view("tickets-drag-card".into(),drag.card.key.clone(),drag.card.title.clone(),drag.card.status.name.clone(),true,drag.waiting,drag.evidence)
            .w(px(250.)).bg(theme::raised()).shadow(theme::popover_shadow())
            .children(drag.session.as_ref().map(session_line))
            .children(drag.review.as_ref().map(review_line))
    }
}

impl Hangar {
    fn ticket_available_width(&self,window: &Window) -> f32 {
        (f32::from(window.viewport_size().width)-220.).max(1.)
    }

    pub(super) fn ticket_detail_width(&self,window: &Window) -> f32 {
        let expanded = self.mission.detail_expanded;
        let available = self.ticket_available_width(window) - if expanded { 0. } else { 40. };
        detail_width(appearance::get().ticket_detail_width,available,expanded)
    }

    pub(super) fn change_ticket_width(&mut self,width: f32,persist: bool,window: &Window,cx: &mut Context<Self>) {
        self.mission.detail_expanded=false;
        let mut next=appearance::get();
        next.ticket_detail_width=Some(detail_width(Some(width),self.ticket_available_width(window)-40.,false));
        self.apply_appearance(next,persist,cx);
    }

    pub(in crate::app) fn ticket_detail_resizing(&self) -> bool {self.mission.detail_resize.is_some()}

    pub(in crate::app) fn drag_ticket_detail(&mut self,x: f32,pressed: bool,window: &Window,cx: &mut Context<Self>) {
        let Some((start,width))=self.mission.detail_resize else {return;};
        if !pressed {self.mission.detail_resize=None;}
        self.change_ticket_width(width+start-x,!pressed,window,cx);
    }

    pub(super) fn ticket_resize_handle(&self,width: f32,cx: &mut Context<Self>) -> AnyElement {
        let focus=self.mission.detail_focus.clone();
        div().id("tickets-detail-resize").role(Role::Splitter).aria_label(tr("mission_detail_resize"))
            .absolute().left_0().top_0().bottom_0().w(px(8.)).cursor_col_resize().tab_index(0)
            .when_some(focus.clone(),|el,focus|el.track_focus(&focus))
            .hover(|el|el.bg(theme::accent_dim()))
            .on_mouse_down(MouseButton::Left,cx.listener(move |this,event: &MouseDownEvent,window,cx| {
                if let Some(focus)=focus.as_ref() {focus.focus(window,cx);}
                this.mission.detail_resize=Some((f32::from(event.position.x),width));
                cx.stop_propagation();cx.notify();
            }))
            .on_key_down(cx.listener(|this,event: &KeyDownEvent,window,cx| {
                let width=this.ticket_detail_width(window);
                let next=match event.keystroke.key.as_str() {"left"=>width+20.,"right"=>width-20.,"home"=>320.,"end"=>f32::MAX,_=>return};
                this.change_ticket_width(next,true,window,cx);cx.stop_propagation();
            })).into_any_element()
    }

    pub(super) fn ticket_drag_scroll(&mut self,event: &DragMoveEvent<TicketDrag>,cx: &mut Context<Self>) {
        let x=event.event.position.x;
        self.mission.drag_edge=if !event.bounds.contains(&event.event.position) {0}
            else if x<event.bounds.left()+px(32.) {-1} else if x>event.bounds.right()-px(32.) {1} else {0};
        if self.mission.drag_edge==0 || self.mission.drag_scroll.is_some() {return;}
        self.mission.drag_scroll=Some(cx.spawn(async move |this,cx|loop {
            cx.background_executor().timer(Duration::from_millis(40)).await;
            let again=this.update(cx,|this,cx| {
                if !cx.has_active_drag() || !this.mission.open || this.mission.drag_edge==0 {
                    this.mission.drag_scroll=None;return false;
                }
                let (offset,max)=(this.mission.lanes_scroll.offset(),this.mission.lanes_scroll.max_offset());
                let x=(offset.x-px(16.*f32::from(this.mission.drag_edge))).clamp(-max.x,px(0.));
                this.mission.lanes_scroll.set_offset(point(x,offset.y));cx.notify();true
            }).unwrap_or(false);
            if !again {break;}
        }));
    }

    pub(in crate::app) fn ticket_drag_scrolling(&self) -> bool {self.mission.drag_scroll.is_some()}
    pub(in crate::app) fn end_ticket_drag(&mut self) {self.mission.drag_edge=0;self.mission.drag_scroll=None;}
}

#[cfg(test)]
mod tests {
    use super::detail_width;

    #[test]
    fn ticket_detail_width_preserves_a_column_and_can_expand_to_the_available_space() {
        assert_eq!(detail_width(None,1000.,false),480.);
        assert_eq!(detail_width(Some(900.),1000.,false),750.);
        assert_eq!(detail_width(Some(720.),1000.,false),720.);
        assert_eq!(detail_width(None,1000.,true),1000.);
        assert_eq!(detail_width(Some(480.),500.,false),320.);
        assert_eq!(detail_width(None,250.,false),250.);
    }
}
