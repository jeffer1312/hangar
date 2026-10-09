//! Painel da pergunta lateral (`/btw`) desenhado com o tema do app a partir do estado que o plugin manda no
//! `data` do painel. Cada ação é o botão de mesma `key` da árvore do terminal, pela rota de clique de sempre.
use std::rc::Rc;
use gpui_kit::{component::{button::*, text::TextView, *}, *};
use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder;
use serde_json::Value;
use super::{Control, View};
use crate::{i18n::tr, theme};

pub const SITE: &str = "hangar-btw";
const PLUGIN: &str = "hangar";
const RECENT: usize = 5;

/// As `RECENT` mais novas, da mais nova para trás, com o índice de cada uma na lista.
pub fn recent(entries: &[Value]) -> Vec<(usize, &Value)> {
    entries.iter().enumerate().rev().take(RECENT).collect()
}

/// Hora local da pergunta, `HH:MM`; vazio sem hora.
pub fn hhmm(ms: f64) -> String {
    if !ms.is_finite() || ms <= 0. { return String::new(); }
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default()
}

type Click = Rc<dyn Fn(&mut Window, &mut App)>;

/// O clique no botão `key` da árvore do plugin; sem `press` (sessão só leitura), nenhum.
fn press(view: &View, key: &str) -> Option<Click> {
    let press = view.press.clone()?;
    let control = Control { plugin: PLUGIN.into(), key: key.into() };
    Some(Rc::new(move |window: &mut Window, cx: &mut App| press(SITE, &control, window, cx)))
}

fn icon_button(id: &str, icon: IconName, tip: String, click: Option<Click>) -> Button {
    let enabled = click.is_some();
    Button::new(SharedString::from(format!("btw-{id}"))).ghost().xsmall().icon(icon).tooltip(tip).disabled(!enabled)
        .when_some(click, |b, f| b.on_click(move |_, window, cx| f(window, cx)))
}

fn action(id: &str, icon: IconName, label: String, click: Option<Click>) -> Button {
    let enabled = click.is_some();
    Button::new(SharedString::from(format!("btw-{id}"))).outline().xsmall().icon(icon).label(label).disabled(!enabled)
        .when_some(click, |b, f| b.on_click(move |_, window, cx| f(window, cx)))
}

fn note(border: Hsla, tint: Option<Hsla>, icon: Option<(IconName, Hsla)>, body: impl IntoElement) -> Div {
    div().flex().items_start().gap_2().px_3().py_2().rounded(px(8.)).border_1().border_color(border)
        .when_some(tint, |el, bg| el.bg(bg))
        .when_some(icon, |el, (icon, color)| el.child(Icon::new(icon).size(px(15.)).mt(px(2.)).flex_shrink_0().text_color(color)))
        .child(div().flex_1().min_w_0().child(body))
}

/// O painel inteiro. `tabs`: a fileira de abas quando há outros painéis; com ela, o `✕` é o dela.
pub fn panel(pane: &Value, view: &View, max_h: f32, tabs: Option<AnyElement>) -> Option<AnyElement> {
    let data = &pane["data"];
    let entries = data["entries"].as_array()?;
    let with_tabs = tabs.is_some();
    let close = (!with_tabs).then(|| view.close.clone()).flatten().map(|close| -> Click {
        Rc::new(move |window: &mut Window, cx: &mut App| close(SITE, window, cx))
    });
    let badge = div().flex().items_center().gap_1p5().text_color(theme::accent()).font_weight(FontWeight::SEMIBOLD).text_xs()
        .child(Icon::new(IconName::MessageCircleQuestionMark).size(px(14.))).child(tr("btw_title"));
    let current = data["current"].as_u64().map(|c| c as usize).filter(|c| *c < entries.len());
    let Some(index) = current else {
        let head = div().flex().items_center().gap_2().px_3().py_2().border_b_1().border_color(theme::border()).child(badge)
            .child(div().flex_1()).when_some(close, |el, f| el.child(icon_button("close", IconName::Close, tr("btw_close"), Some(f))));
        return Some(shell(max_h).children(tabs).child(head)
            .child(div().px_4().py_3().text_sm().text_color(theme::muted()).child(tr("btw_empty"))).into_any_element());
    };
    let entry = &entries[index];
    let status = entry["status"].as_str().unwrap_or("");
    let fork = entry["fork"].as_str().unwrap_or("");
    let count = tr("btw_count").replace("{n}", &(index + 1).to_string()).replace("{total}", &entries.len().to_string());
    let head = div().flex().items_center().gap_2().px_3().py_1p5().border_b_1().border_color(theme::border())
        .child(badge)
        .child(div().text_xs().text_color(theme::muted()).child(count))
        .child(div().flex_1())
        .child(icon_button("prev", IconName::ChevronLeft, tr("btw_prev"), (index > 0).then(|| press(view, "anterior")).flatten()))
        .child(icon_button("next", IconName::ChevronRight, tr("btw_next"), (index + 1 < entries.len()).then(|| press(view, "proxima")).flatten()))
        .child(icon_button("clear", IconName::Trash, tr("btw_clear"), press(view, "limpar")))
        .when_some(close, |el, f| el.child(icon_button("close", IconName::Close, tr("btw_close"), Some(f))));

    let question = div().flex().items_start().gap_2()
        .child(div().flex_shrink_0().mt(px(1.)).px(px(7.)).rounded_full().border_1().border_color(theme::border())
            .text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr("btw_you")))
        .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD).child(entry["question"].as_str().unwrap_or("").to_owned()));
    let main = match status {
        "pending" => div().flex().items_center().gap_2().text_color(theme::muted())
            .child(div().size(px(6.)).rounded_full().bg(theme::accent())).child(tr("btw_answering")).into_any_element(),
        "error" => note(theme::danger().alpha(0.45), Some(theme::danger().alpha(0.1)), None,
            div().text_sm().child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::danger()).child(tr("btw_failed")))
                .child(entry["error"].as_str().unwrap_or("").to_owned())).into_any_element(),
        _ => TextView::markdown(SharedString::from(format!("btw-md-{}", entry["id"])), entry["answer"].as_str().unwrap_or("").to_owned())
            .selectable(true).scrollable(false).into_any_element(),
    };
    let fork_note = match fork {
        "running" => Some(note(theme::border(), None, Some((IconName::GitFork, theme::accent())),
            div().text_sm().child(tr("btw_forking")).child(div().text_xs().text_color(theme::muted()).child(tr("btw_forking_sub"))))),
        "done" => Some(note(theme::success().alpha(0.45), None, Some((IconName::GitFork, theme::success())),
            div().text_sm().child(tr("btw_forked")))),
        "failed" => Some(note(theme::danger().alpha(0.45), None, Some((IconName::GitFork, theme::danger())),
            div().text_sm().child(entry["forkNote"].as_str().unwrap_or("").to_owned()))),
        _ => None,
    };
    let body = div().id("btw-body").flex().flex_col().gap_2p5().px_4().py_3().flex_1().min_h_0().overflow_y_scroll()
        .child(question).child(main).children(fork_note);

    let busy = status == "pending" || fork == "running";
    let foot = (!busy).then(|| div().flex().flex_wrap().items_center().gap_1p5().px_3().pb_2p5()
        .when(status == "done", |el| el.child(action("copy", IconName::Copy, tr("btw_copy"), press(view, "copiar"))))
        .child(action("retry", IconName::RefreshCw, tr("btw_retry"), press(view, "repetir")))
        .when(fork.is_empty() || fork == "failed", |el| el
            .child(action("fork", IconName::GitFork, tr("btw_fork"), press(view, "bifurcar")).tooltip(tr("btw_fork_tip")))
            .child(div().ml_auto().text_xs().text_color(theme::muted()).child(tr("btw_fork_hint")))));

    let recents = (entries.len() > 1).then(|| div().flex().flex_col().gap_0p5().px_2().pt_2().pb_2p5().border_t_1().border_color(theme::border())
        .child(div().px_1p5().pb_1().text_xs().text_color(theme::muted()).child(tr("btw_recent").to_uppercase()))
        .children(recent(entries).into_iter().map(|(i, e)| {
            let on = i == index;
            let dot = match e["status"].as_str() { Some("pending") => theme::accent(), Some("error") => theme::danger(), _ => theme::success() };
            let click = press(view, &format!("recente-{}", e["id"]));
            div().id(SharedString::from(format!("btw-recent-{}", e["id"]))).flex().items_center().gap_2().px_1p5().py_1().rounded(px(6.))
                .text_sm().text_color(if on { theme::text() } else { theme::muted() })
                .when(on, |el| el.bg(theme::accent_dim()))
                .when(!on, |el| el.hover(|s| s.bg(theme::raised()).text_color(theme::text())))
                .child(div().size(px(7.)).rounded_full().flex_shrink_0().bg(dot))
                .child(div().flex_1().min_w_0().truncate().child(e["question"].as_str().unwrap_or("").to_owned()))
                .child(div().text_xs().child(hhmm(e["askedAt"].as_f64().unwrap_or(0.))))
                .when_some(click.filter(|_| !on), |el, f| el.cursor_pointer().on_click(move |_, window, cx| f(window, cx)))
        })));

    Some(shell(max_h).children(tabs).child(head).child(body).children(foot).children(recents).into_any_element())
}

fn shell(max_h: f32) -> Div {
    div().flex().flex_col().w_full().min_h_0().max_h(px(max_h)).overflow_hidden().rounded(px(10.)).border_1()
        .border_color(theme::border()).bg(theme::inset()).text_color(theme::text())
}

#[cfg(test)]
mod tests {
    use super::{hhmm, recent};
    use core::prelude::v1::test;
    use serde_json::json;

    #[test]
    fn recents_are_the_five_newest_newest_first() {
        let entries: Vec<_> = (1..=7).map(|id| json!({"id": id})).collect();
        assert_eq!(recent(&entries).iter().map(|(i, e)| (*i, e["id"].as_u64().unwrap())).collect::<Vec<_>>(),
            [(6, 7), (5, 6), (4, 5), (3, 4), (2, 3)]);
        assert!(recent(&[]).is_empty());
    }

    #[test]
    fn time_is_local_hours_and_minutes_or_nothing() {
        assert_eq!(hhmm(0.), "");
        assert_eq!(hhmm(f64::NAN), "");
        assert_eq!(hhmm(1_791_480_000_000.).len(), 5);
    }
}
