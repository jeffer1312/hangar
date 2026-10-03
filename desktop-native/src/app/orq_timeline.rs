//! Conversa da sessão `orq` (`OrqTimelineEvent.svelte`): linha curta para o que o orquestrador faz e bolha para o
//! recado ao árbitro, com marcas, selo de quem decidiu, pergunta e parecer. Regras de exibição espelham
//! `packages/core/src/orqTimeline.ts`; os textos vêm das chaves `orq_*` que o web também usa.
use super::*;
use chrono::{Datelike, Local, TimeZone};

const PREVIEW_MAX: usize = 280;
/// Só o recado descartado some; o selo e o detalhe do Jev ficam no contraste normal.
const FADED: f32 = 0.62;

/// Corpo do recado cortado no último espaço antes do limite, sem deixar crase aberta (viraria código sem fim).
fn body_preview(body: &str, max: usize) -> (String, bool) {
    if body.chars().count() <= max { return (body.to_owned(), false); }
    let mut cut: String = body.chars().take(max).collect();
    if let Some(space) = cut.rfind(' ').filter(|at| *at > 0) { cut.truncate(space); }
    while cut.matches('`').count() % 2 == 1 {
        let at = cut.rfind('`').unwrap_or(0);
        cut.truncate(at);
    }
    (format!("{}…", cut.trim_end()), true)
}

#[derive(Debug, PartialEq)]
struct BadgeKey { key: String, p: Option<f64>, category: Option<String> }

/// Chave do selo de quem decidiu e os valores que entram nela (probabilidade, categoria da regex).
fn badge_key(decided: &OrqDecidedBy, kind: &str) -> BadgeKey {
    let plain = |key: &str| BadgeKey { key: key.to_owned(), p: None, category: None };
    match decided.source.as_str() {
        "rule" => return plain(if decided.rule.as_deref() == Some("mark") { "orq_badge_rule_mark" } else { "orq_badge_rule_orchestrator" }),
        "alarm" => return plain("orq_badge_alarm"),
        _ => {}
    }
    let outcome = if matches!(kind, "dropped" | "would_drop") { kind } else { "woke" };
    if decided.source == "regex" {
        let category = decided.regex.as_ref().and_then(|regex| regex.category.clone()).filter(|c| !c.is_empty());
        return BadgeKey { key: format!("orq_badge_regex_{outcome}"), p: None, category };
    }
    let jev = decided.jev.as_ref();
    let p = match jev {
        Some(jev) if !jev.probs.is_empty() => jev.choice.as_ref().and_then(|choice| jev.probs.get(choice).copied().flatten()),
        Some(jev) if jev.choice.as_deref() == Some("nothing") && outcome != "woke" => jev.p,
        _ => None,
    };
    match p {
        Some(p) => BadgeKey { key: format!("orq_badge_jev_{outcome}"), p: Some(p), category: None },
        None => plain(&if outcome == "woke" { "orq_badge_jev_woke_bare".to_owned() } else { format!("orq_badge_jev_{outcome}") }),
    }
}

pub(super) fn pct(p: f64) -> String { format!("{}%", (p * 100.).round()) }

/// Casa decimal com o idioma escolhido no app, como o `dec` do web.
fn dec(value: f64) -> String {
    let text = format!("{value:.2}");
    if crate::i18n::english() { text } else { text.replace('.', ",") }
}

fn badge_text(decided: &OrqDecidedBy, kind: &str) -> String {
    let badge = badge_key(decided, kind);
    let p = badge.p.map(pct).unwrap_or_default();
    let text = tr_shared(&badge.key, &[("p", &p), ("category", badge.category.as_deref().unwrap_or(""))]);
    // Sem probabilidade ou categoria a frase termina no separador; ele sai junto.
    text.trim_end().trim_end_matches('·').trim_end().to_owned()
}

/// O porquê da decisão, uma frase por parte; o que o backend não mandou fica de fora.
fn detail_lines(decided: &OrqDecidedBy) -> Vec<String> {
    let mut lines = Vec::new();
    let jev = decided.jev.as_ref();
    if let Some((jev, choice)) = jev.and_then(|jev| jev.choice.as_deref().map(|choice| (jev, choice))) {
        let p = jev.probs.get(choice).copied().flatten().or(if choice == "nothing" { jev.p } else { None }).map(pct).unwrap_or_default();
        let name = match choice {
            "nothing" | "act" | "none" => tr_shared(&format!("orq_choice_{choice}"), &[]),
            other => other.to_owned(),
        };
        lines.push(tr_shared("orq_detail_choice", &[("choice", &name), ("p", &p)]).trim().to_owned());
    }
    if let Some(veto) = jev.and_then(|jev| jev.veto.as_ref()).filter(|veto| !veto.is_empty()) {
        let value = |key: &str| veto.get(key).copied().flatten().map(dec).unwrap_or_else(|| "–".to_owned());
        lines.push(tr_shared("orq_detail_vetoes", &[("context", &value("context")), ("user", &value("user")),
            ("problem", &value("problem")), ("deviation", &value("deviation"))]));
    }
    if let Some(agreed) = decided.regex_agreed {
        lines.push(tr_shared(if agreed { "orq_detail_regex_agreed" } else { "orq_detail_regex_disagreed" }, &[]));
    }
    if let Some(error) = jev.and_then(|jev| jev.error.as_deref()) {
        lines.push(tr_shared("orq_detail_jev_error", &[("error", error)]));
    }
    lines
}

/// Frase curta pelo código; nomes e commit entre crases viram código no markdown.
fn line_text(line: &OrqLine) -> Option<String> {
    let code = |text: &str| if text.is_empty() { String::new() } else { format!("`{text}`") };
    // Campo que não veio deixa o separador pendurado no fim da frase; ele sai junto.
    let text = match line {
        OrqLine::Opened { sessions } => {
            let executor = code(sessions.first().map(|s| s.name.as_str()).unwrap_or(""));
            match sessions.get(1) {
                Some(reviewer) => tr_shared("orq_line_opened", &[("executor", &executor), ("reviewer", &code(&reviewer.name))]),
                None => tr_shared("orq_line_opened_solo", &[("executor", &executor)]),
            }
        }
        OrqLine::Integrated { merge } => tr_shared(if *merge { "orq_line_integrated" } else { "orq_line_integrated_direct" }, &[]),
        OrqLine::Delivered { round, commit } => tr_shared("orq_line_delivered", &[
            ("round", &round.map(|r| r.to_string()).unwrap_or_default()), ("commit", &code(commit.as_deref().unwrap_or("")))]),
        OrqLine::RedBack { executor } => tr_shared("orq_line_red_back", &[("executor", &code(executor.as_deref().unwrap_or("")))]),
        OrqLine::RedRetry => tr_shared("orq_line_red_retry", &[]),
        OrqLine::Unknown => return None,
    };
    Some(text.trim_end().trim_end_matches('·').trim_end().to_owned())
}

/// Ids dos eventos que abrem um dia novo (hora local), para o separador sair antes deles.
pub(super) fn day_starts<'a>(events: impl IntoIterator<Item = &'a ChatEvent>) -> HashSet<String> {
    let mut starts = HashSet::new();
    let mut last = None;
    for event in events {
        let Some(at) = event.ts.filter(|ts| ts.is_finite() && *ts > 0.).and_then(|ts| Local.timestamp_opt(ts as i64, 0).single()) else { continue };
        let day = at.date_naive();
        if last != Some(day) { starts.insert(event.id.clone()); last = Some(day); }
    }
    starts
}

fn day_label(ts: Option<f64>) -> Option<String> {
    let at = Local.timestamp_opt(ts.filter(|ts| ts.is_finite() && *ts > 0.)? as i64, 0).single()?;
    let time = clock(ts)?;
    let today = Local::now().date_naive();
    if at.date_naive() == today { return Some(tr_shared("orq_day_today", &[("time", &time)])); }
    if today.pred_opt() == Some(at.date_naive()) { return Some(tr_shared("orq_day_yesterday", &[("time", &time)])); }
    let date = tr("message_date").replace("{d}", &format!("{:02}", at.day())).replace("{m}", &format!("{:02}", at.month()));
    Some(tr_shared("orq_day_date", &[("date", &date), ("time", &time)]))
}

/// Linha curta sem `line` estruturado: o texto cru, sem o "T3:" que o `T3` em negrito já diz.
fn raw_line(text: &str, task: Option<u32>) -> String {
    if task.is_none() { return text.to_owned(); }
    let rest = text.strip_prefix('T').unwrap_or(text);
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    match rest[digits..].strip_prefix(':') {
        Some(after) if text.starts_with('T') && digits > 0 => after.trim_start().to_owned(),
        _ => text.to_owned(),
    }
}

impl Hangar {
    pub(super) fn render_orq_event(&mut self, id: &str, event_index: usize, cx: &mut Context<Self>) -> AnyElement {
        let event = &self.chat.events[event_index];
        let (ts, raw) = (event.ts, event.text.clone().unwrap_or_default());
        let Some(orq) = event.orq_entry() else { return div().into_any_element() };
        let day = self.orq_days.contains(id).then(|| day_label(ts)).flatten();
        let time = clock(ts);
        let is_line = matches!(orq.kind.as_str(), "advance" | "notice" | "");
        let body = if is_line { self.render_orq_line(id, &orq, raw, time, cx) } else { self.render_orq_bubble(id, &orq, time, cx) };
        let row = div().id(SharedString::from(format!("message-{id}"))).w_full().flex().flex_col().gap_1()
            .when_some(day, |el, label| el.child(div().w_full().flex().justify_center().pt_2().text_xs().text_color(theme::muted()).child(label)))
            .child(body);
        with_copy_menu(row, id.to_owned(), cx.weak_entity())
    }

    fn render_orq_line(&mut self, id: &str, orq: &OrqEntry, raw: String, time: Option<String>, cx: &mut Context<Self>) -> Div {
        let source = orq.line.as_ref().and_then(line_text).unwrap_or_else(|| raw_line(&raw, orq.task));
        let color = match orq.line.as_ref() {
            Some(OrqLine::Integrated { .. }) => theme::success(),
            Some(OrqLine::RedBack { .. } | OrqLine::RedRetry) => theme::danger(),
            _ if orq.kind == "advance" => theme::accent(),
            _ => theme::warning(),
        };
        let view = self.text_view(&format!("{id}#orq-line"), id, source, cx);
        div().w_full().flex().items_center().gap_2().pl(px(38.)).text_sm().text_color(theme::muted())
            .child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(color))
            .when_some(orq.task, |el, task| el.child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(format!("T{task}"))))
            .child(div().flex_1().min_w_0().child(TextView::new(&view).selectable(true).scrollable(false).text_sm().on_link_click(open_web_link)))
            .when_some(time, |el, time| el.child(div().flex_shrink_0().text_xs().text_color(theme::faint()).child(time)))
    }

    fn render_orq_bubble(&mut self, id: &str, orq: &OrqEntry, time: Option<String>, cx: &mut Context<Self>) -> Div {
        let dropped = orq.kind == "dropped";
        let label = match orq.kind.as_str() {
            "dropped" => match orq.sender.as_deref() {
                Some(sender) => tr_shared("orq_sender_to_arbiter", &[("sender", sender)]),
                None => tr_shared("orq_sender_unknown", &[]),
            },
            "failed" => tr_shared(if orq.origin.as_deref() == Some("notify") { "orq_label_not_delivered" } else { "orq_label_step_failed" }, &[]),
            _ => tr_shared("orq_label_woke", &[]),
        };
        let (glyph, tint) = if orq.rejected_round.is_some() { ("✕", theme::danger()) }
            else if dropped { ("◌", theme::muted()) }
            else if orq.kind == "failed" { ("!", theme::danger()) }
            else { ("⚖", theme::warning()) };
        let tag = |text: String, color: Hsla| div().flex_shrink_0().px(px(6.)).rounded(px(6.)).bg(color.opacity(0.18)).text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD).text_color(color).child(text).when(dropped, |el| el.opacity(FADED));

        let detail = orq.decided_by.as_ref().map(detail_lines).unwrap_or_default();
        // Descartado nasce com o porquê à vista, e o clique inverte o que estava.
        let detail_key = format!("{id}#orq-detail");
        let detail_open = self.expanded.contains(&detail_key) != dropped;
        let badge = orq.decided_by.as_ref().map(|decided| {
            let chip = badge(badge_text(decided, &orq.kind), theme::muted());
            if detail.is_empty() { return chip.into_any_element(); }
            let key = detail_key.clone();
            chip.id(SharedString::from(format!("orq-badge-{id}"))).cursor_pointer().role(Role::Button)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(key.clone(), cx))).into_any_element()
        });

        let more_key = format!("{id}#orq-more");
        let more_open = self.expanded.contains(&more_key);
        let (preview, cut) = body_preview(&orq.body, PREVIEW_MAX);
        let shown = if more_open || !cut { orq.body.as_str() } else { preview.as_str() };
        let body_source = safe_markdown(&composer::citation_markdown(shown));
        let body_view = (!orq.body.is_empty()).then(|| self.text_view(&format!("{id}#orq-body"), id, body_source, cx));
        let question_view = orq.question.as_ref().filter(|q| !q.is_empty())
            .map(|question| self.text_view(&format!("{id}#orq-question"), id, safe_markdown(question), cx));

        let parecer = orq.parecer.clone().filter(|path| !path.is_empty()).map(|path| {
            let name = composer::basename(&path).to_owned();
            Button::new(SharedString::from(format!("orq-parecer-{id}"))).small().outline()
                .child(crate::fileicons::citation_icon(&name)).child(name)
                .on_click(cx.listener(move |this, _, window, cx| this.open_file(path.clone(), None, window, cx)))
        });

        let content = div().min_w_0().flex_1().flex().flex_col().gap_1().px(px(12.)).py(px(8.)).rounded(px(10.)).bg(theme::raised())
            .border_1().border_color(theme::border()).text_sm()
            .child(div().flex().flex_wrap().items_center().gap_2().text_xs().text_color(theme::muted())
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).when(dropped, |el| el.opacity(FADED)).child(label))
                .when(orq.mark.as_deref() == Some("decisao"), |el| el.child(tag(tr_shared("orq_tag_decision", &[]), theme::warning())))
                .when(orq.alarm, |el| el.child(tag(tr_shared("orq_tag_alarm", &[]), theme::warning())))
                .when_some(orq.task, |el, task| el.child(tag(format!("T{task}"), theme::muted())))
                .when_some(orq.rejected_round, |el, round| el.child(tag(tr_shared("orq_tag_rejected", &[("round", &round.to_string())]), theme::danger())))
                .when_some(badge, |el, badge| el.child(badge))
                .when_some(time, |el, time| el.child(div().ml_auto().flex_shrink_0().text_color(theme::faint()).when(dropped, |el| el.opacity(FADED)).child(time))))
            .when_some(body_view, |el, view| el.child(div().when(dropped, |el| el.opacity(FADED))
                .child(chat_text(&view, cx).markdown_extensions(citation_extensions(id, cx.weak_entity())))))
            .when(cut, |el| el.child(div().flex().when(dropped, |el| el.opacity(FADED)).child(Button::new(SharedString::from(format!("orq-more-{id}"))).ghost().xsmall()
                .label(tr_shared(if more_open { "orq_see_less" } else { "orq_see_all" }, &[]))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(more_key.clone(), cx))))))
            .when_some(question_view, |el, view| el.child(div().flex().flex_wrap().gap_1().when(dropped, |el| el.opacity(FADED))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(tr_shared("orq_question", &[])))
                .child(div().min_w_0().flex_1().child(TextView::new(&view).selectable(true).scrollable(false).text_sm().on_link_click(open_web_link)))))
            .when(orq.kind == "failed", |el| el.when_some(orq.error.as_deref(), |el, error|
                el.child(div().child(tr_shared("orq_failed_reason", &[("error", error)])))))
            .when_some(parecer, |el, button| el.child(div().flex().when(dropped, |el| el.opacity(FADED)).child(button)))
            .when(detail_open && !detail.is_empty(), |el| el.child(div().text_xs().text_color(theme::muted()).child(detail.join(" · "))));

        div().w_full().max_w(px(760.)).flex().items_start().gap(px(10.))
            .child(div().size(px(28.)).mt(px(2.)).flex_shrink_0().rounded_full().flex().items_center().justify_center().when(dropped, |el| el.opacity(FADED))
                .bg(tint.opacity(0.18)).text_color(tint).text_size(px(13.)).child(glyph))
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::{badge_key, badge_text, body_preview, day_starts, detail_lines, line_text, raw_line};
    use crate::{api::dto::*, i18n::tr_shared};
    use chrono::{Local, TimeZone};
    use std::collections::HashSet;

    fn decided(source: &str) -> OrqDecidedBy { OrqDecidedBy { source: source.into(), ..Default::default() } }
    fn jev(choice: &str, p: Option<f64>, probs: &[(&str, f64)]) -> OrqDecidedBy {
        OrqDecidedBy { jev: Some(OrqJev { choice: Some(choice.into()), p, probs: probs.iter().map(|(k, v)| ((*k).into(), Some(*v))).collect(), ..Default::default() }),
            ..decided("jev") }
    }

    #[test]
    fn preview_cuts_at_the_last_space_and_never_leaves_an_open_backtick() {
        assert_eq!(body_preview("curto", 280), ("curto".into(), false));
        let long = format!("{} `codigo aberto que passa do limite", "palavra ".repeat(33));
        let (text, cut) = body_preview(&long, 280);
        assert!(cut && text.ends_with('…') && text.chars().count() <= 281);
        assert_eq!(text.matches('`').count() % 2, 0);
        let (text, _) = body_preview(&"a".repeat(300), 280);
        assert_eq!(text.chars().count(), 281);
    }

    #[test]
    fn badge_key_names_who_decided_and_carries_the_number() {
        let key = |d: &OrqDecidedBy, kind| badge_key(d, kind);
        let mark = OrqDecidedBy { rule: Some("mark".into()), ..decided("rule") };
        let orchestrator = OrqDecidedBy { rule: Some("orchestrator".into()), ..decided("rule") };
        assert_eq!(key(&mark, "woke").key, "orq_badge_rule_mark");
        assert_eq!(key(&orchestrator, "woke").key, "orq_badge_rule_orchestrator");
        assert_eq!(key(&decided("alarm"), "woke").key, "orq_badge_alarm");
        let dropped = key(&jev("nothing", Some(0.97), &[]), "dropped");
        assert_eq!((dropped.key.as_str(), dropped.p), ("orq_badge_jev_dropped", Some(0.97)));
        let woke = key(&jev("act", None, &[("act", 0.94), ("nothing", 0.06)]), "woke");
        assert_eq!((woke.key.as_str(), woke.p), ("orq_badge_jev_woke", Some(0.94)));
        assert_eq!(key(&jev("nothing", None, &[]), "woke").key, "orq_badge_jev_woke_bare");
        let regex = OrqDecidedBy { regex: Some(OrqRegex { verdict: "drop".into(), category: Some("janela".into()) }), ..decided("regex") };
        let badge = key(&regex, "would_drop");
        assert_eq!((badge.key.as_str(), badge.category.as_deref()), ("orq_badge_regex_would_drop", Some("janela")));
    }

    #[test]
    fn badge_text_never_ends_with_a_dangling_separator() {
        let no_number = jev("act", None, &[]);
        let text = badge_text(&no_number, "dropped");
        assert!(!text.ends_with('·') && !text.ends_with(' '), "{text:?}");
        assert!(!badge_text(&OrqDecidedBy { regex: Some(OrqRegex { verdict: "drop".into(), category: None }), ..decided("regex") }, "would_drop").ends_with('·'));
        assert!(badge_text(&jev("nothing", Some(0.97), &[]), "dropped").ends_with("97%"));
    }

    #[test]
    fn detail_uses_the_loose_p_only_for_nothing() {
        // Registro antigo que acordou: choice act com p 0 e sem probs não vira "agir · 0%".
        let old = detail_lines(&jev("act", Some(0.), &[])).join(" ");
        assert!(!old.contains("0%"), "{old}");
        assert!(detail_lines(&jev("nothing", Some(0.9), &[])).join(" ").contains("90%"));
    }

    #[test]
    fn day_starts_marks_the_first_event_of_each_local_day() {
        let at = |day, hour| Local.with_ymd_and_hms(2026, 9, day, hour, 0, 0).unwrap().timestamp() as f64;
        let event = |id: &str, ts| ChatEvent { id: id.into(), ts: Some(ts), ..Default::default() };
        let events = [event("a", at(29, 21)), event("b", at(29, 22)), event("c", at(30, 9))];
        assert_eq!(day_starts(&events), HashSet::from(["a".to_owned(), "c".to_owned()]));
    }

    #[test]
    fn line_text_puts_names_in_backticks() {
        let session = |name: &str| OrqSessionRef { name: name.into(), ..Default::default() };
        let both = line_text(&OrqLine::Opened { sessions: vec![session("exec"), session("rev")] }).unwrap();
        assert_eq!(both, tr_shared("orq_line_opened", &[("executor", "`exec`"), ("reviewer", "`rev`")]));
        assert!(both.contains("`exec`") && both.contains("`rev`"));
        let solo = line_text(&OrqLine::Opened { sessions: vec![session("exec")] }).unwrap();
        assert_eq!(solo, tr_shared("orq_line_opened_solo", &[("executor", "`exec`")]));
        assert_ne!(solo, both);
        assert!(line_text(&OrqLine::Unknown).is_none());
        let bare = line_text(&OrqLine::Delivered { round: Some(2), commit: None }).unwrap();
        assert!(!bare.contains('`') && !bare.ends_with('·') && !bare.ends_with(' '), "{bare}");
        assert!(line_text(&OrqLine::Delivered { round: Some(2), commit: Some("abc123".into()) }).unwrap().ends_with("`abc123`"));
        assert_eq!(line_text(&OrqLine::Opened { sessions: vec![] }).unwrap().matches('`').count(), 0);
    }

    #[test]
    fn raw_line_drops_the_task_prefix_only_with_a_task() {
        assert_eq!(raw_line("T3: entregou", Some(3)), "entregou");
        assert_eq!(raw_line("T3: entregou", None), "T3: entregou");
        assert_eq!(raw_line("Tarefa: x", Some(3)), "Tarefa: x");
    }
}
