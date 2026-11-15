//! O cartão real muda de etapa; o comando continua em um botão separado.
use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use gpui_kit::{component::{button::*, *}, assets::IconName};
use crate::{app, i18n::tr, theme};

pub(super) fn render(started: Instant,width: Pixels,window: &mut Window,cx: &mut App) -> AnyElement {
    let elapsed=started.elapsed().as_secs_f32()%6.;
    let moving=!cx.reduce_motion() && (1.0..3.0).contains(&elapsed);
    let moved=cx.reduce_motion() || elapsed>=3.;
    if !cx.reduce_motion() {super::repaint_in(if moving {1./60.} else if elapsed<1. {1.-elapsed} else {6.-elapsed},window,cx);}
    let card=app::ticket_card_view("news-ticket".into(),"PM-123".into(),tr("news_tickets_sample"),tr("news_tickets_status"),true,false,0);
    let mut card=Some(card);
    let lanes=[tr("news_tickets_plan"),tr("news_tickets_execute")].into_iter().enumerate().map(|(index,label)| {
        div().flex_1().min_w_0().h_full().p_3().rounded(px(8.)).border_1().border_color(theme::border()).flex().flex_col().gap_3()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(label))
            .when(index==usize::from(moved),|el|el.children(card.take()))
    }).collect::<Vec<_>>();
    let lane_width=(f32::from(width)-36.)/2.;
    let progress=((elapsed-1.)/2.).clamp(0.,1.);
    let progress=progress*progress*(3.-2.*progress);
    div().relative().w(width).h(px(290.)).p_3().flex().flex_col().gap_3()
        .child(div().flex_1().min_h_0().flex().gap_3().children(lanes))
        .when(moving,|el|el.child(div().absolute().left(px(24.+(lane_width+12.)*progress)).top(px(56.)).w(px(lane_width-24.))
            .child(app::ticket_card_view("news-ticket-moving".into(),"PM-123".into(),tr("news_tickets_sample"),tr("news_tickets_status"),true,false,0)
                .bg(theme::raised()).shadow(theme::popover_shadow()))))
        .child(div().flex().justify_end().child(Button::new("news-ticket-run").small().primary().tab_stop(false)
            .icon(IconName::Play).label(tr("mission_action_run"))))
        .into_any_element()
}
