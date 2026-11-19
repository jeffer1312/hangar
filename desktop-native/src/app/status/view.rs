//! Desenho da página Status sobre o modelo já montado: os blocos do mock aprovado
//! (`docs/superpowers/specs/2026-11-17-tela-status-mock.html`). Gráficos de série com o `AreaChart` do kit,
//! medidores com o `Progress`; barras divididas, colunas por hora, linha do tempo e cartões são `div`s.
use gpui_kit::component::{Icon, chart::AreaChart, progress::Progress, tooltip::Tooltip};

use hangar_api::status::ExitKind;

use super::*;
use super::model::{Block, Chip, ChipIcon, Days, Health, Host, Machines, Outside, ServerCards, Sessions, Seg, Slow, Spark, TileIcon, Tone};
use super::super::chrome::Skeleton;

/// O losango da atualização, desenhado por SVG: `div` não gira. Só a forma importa, a cor vem do `text_color`.
const DIAMOND: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 1 23 12 12 23 1 12Z" fill="black"/></svg>"#;

const SPARK_HEIGHT: f32 = 46.;

fn tone(t: Tone) -> Hsla {
    match t { Tone::Ok => theme::success(), Tone::Warn => theme::warning(), Tone::Bad => theme::danger(), Tone::Faint => theme::faint(), Tone::Accent => theme::accent() }
}

/// Cor de série: as quatro do gráfico do app, a quinta neutra, e `0` o espaço livre.
fn series(slot: usize) -> Hsla {
    match slot { 0 => theme::border_strong(), 1..=4 => super::super::costs::chart(slot), _ => theme::muted() }
}

/// Dado que só aparece no tooltip: o mesmo texto é o nome acessível, e o elemento entra no Tab, com o contorno do foco.
fn tip(el: Stateful<Div>, text: String) -> Stateful<Div> {
    el.aria_label(text.clone()).focusable().tab_stop(true).focus_visible(|el| el.border_1().border_color(theme::accent_focus()))
        .tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
}

fn card(flash: bool) -> Div {
    settings_box().px(px(16.)).py(px(14.)).when(flash, |el| el.border_color(theme::accent()))
}

fn card_head(title: String, hint: Option<String>) -> Div {
    div().flex().items_center().gap(px(8.)).mb(px(10.)).min_w_0()
        .child(div().flex_shrink_0().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(title))
        .children(hint.map(|hint| div().min_w_0().truncate().text_size(px(11.5)).text_color(theme::faint()).child(hint)))
}

fn swatch(color: Hsla, round: bool) -> Div { div().size(px(9.)).flex_shrink_0().rounded(px(if round { 5. } else { 3. })).bg(color) }

fn diamond(size: f32, color: Hsla) -> Icon { Icon::default().data(DIAMOND).size(px(size)).text_color(color) }

fn legend() -> Div { div().flex().flex_wrap().items_center().gap_x(px(14.)).gap_y(px(4.)).text_size(px(11.5)).text_color(theme::muted()) }

fn legend_item(color: Hsla, round: bool, label: String, extra: Option<String>) -> Div {
    div().flex().items_center().gap(px(6.)).child(swatch(color, round)).child(label)
        .children(extra.map(|extra| div().text_color(theme::faint()).child(extra)))
}

/// Barra dividida: alta com o valor dentro do pedaço largo, ou fina.
fn seg_bar(id: &str, segs: &[Seg], tall: bool) -> Div {
    let total: f32 = segs.iter().map(|s| s.weight).sum();
    div().flex().gap(px(2.)).h(px(if tall { 28. } else { 8. })).rounded(px(if tall { 6. } else { 4. })).overflow_hidden()
        .when(total <= 0., |el| el.bg(theme::border()))
        .children(segs.iter().enumerate().filter(|(_, s)| s.weight > 0. && total > 0.).map(|(i, s)| {
            let piece = div().id(SharedString::from(format!("{id}-{i}"))).h_full().min_w(px(3.)).flex_basis(px(0.)).flex_grow(s.weight / total)
                .bg(series(s.slot)).flex().items_center().overflow_hidden()
                .when(tall, |el| el.pl(px(7.)).children(s.inside.clone().map(|text| div().whitespace_nowrap().text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD).text_color(theme::on_press()).child(text))));
            tip(piece, s.tip.clone())
        }))
}

fn chip_row(chips: &[Chip]) -> Div {
    div().flex().flex_wrap().gap(px(8.)).mt(px(12.)).children(chips.iter().enumerate().map(|(i, chip)| {
        let el = div().id(SharedString::from(format!("status-chip-{i}-{}", chip.text))).h(px(24.)).px(px(9.)).rounded(px(12.)).bg(theme::inset())
            .flex().items_center().gap(px(6.)).text_size(px(12.)).text_color(theme::muted())
            .children(match chip.icon {
                ChipIcon::Clean => Some(chrome::small_icon(IconName::Trash, 13., theme::muted())),
                ChipIcon::Check => Some(chrome::small_icon(IconName::Check, 13., theme::success())),
                ChipIcon::None => None,
            })
            .children(chip.bold.clone().map(|bold| div().font_weight(FontWeight::SEMIBOLD)
                .text_color(if chip.tone == Tone::Bad { theme::danger() } else { theme::text() }).child(bold)))
            .child(chip.text.clone());
        match chip.tip.clone() { Some(text) => tip(el, text), None => el }
    }))
}

/// O nome acessível da curva: cada ponto com o valor e há quanto tempo, o que o mouse lê no tooltip.
fn spark_label(label: &str, spark: &Spark) -> String {
    let points = spark.points.iter().map(|p| format!("{} {}", p.text, p.ago)).collect::<Vec<_>>().join("; ");
    format!("{label}: {points}")
}

/// A curva da última hora, com a média como linha neutra e uma bolinha no valor de agora; passar o mouse
/// mostra o valor e há quanto tempo; pelo teclado, o foco na curva lê os mesmos pontos.
fn spark_chart(id: &'static str, label: &str, spark: &Spark) -> Stateful<Div> {
    #[derive(Clone)]
    struct Row { key: SharedString, v: f64, avg: f64, text: SharedString, ago: SharedString }
    let avg = spark.avg_value.unwrap_or(f64::NAN);
    let rows: Vec<Row> = spark.points.iter().enumerate().map(|(i, p)| Row {
        key: i.to_string().into(), v: p.v, avg, text: p.text.clone().into(), ago: p.ago.clone().into(),
    }).collect();
    let values = rows.iter().map(|r| r.v).chain(spark.avg_value);
    let (low, high) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
    // Faixa justa à curva, como o mock; série reta ganha folga, senão o kit não desenha nada.
    let (low, high) = if high - low < 1e-6 { (low - 1., high + 1.) } else { (low * 0.92, high * 1.05) };
    let color = series(spark.slot);
    let last = rows.last().map_or(low, |r| r.v);
    // Sem folga no gráfico, a escala é linear de `high` (topo) a `low` (base): a bolinha usa a mesma conta.
    let last_y = ((high - last) / (high - low)) as f32 * SPARK_HEIGHT;
    let chart = AreaChart::new(rows).id(id).x(|r: &Row| r.key.clone()).x_axis(false).grid(false)
        .y(|r: &Row| r.v).stroke(color).fill(color.opacity(0.12)).linear()
        .y_domain(low, high).y_padding(0., 0.)
        .tooltip_content(|r: &Row, _, _| div().flex().flex_col().gap(px(2.))
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(r.text.clone()))
            .child(div().text_size(px(12.)).text_color(theme::muted()).child(r.ago.clone())));
    let chart = if spark.avg_value.is_some() {
        chart.y(|r: &Row| r.avg).stroke(theme::border_strong()).fill(transparent_black()).linear()
    } else { chart };
    // A ponta da curva fica na borda direita do gráfico; o anel na cor do cartão a destaca da área.
    div().id(SharedString::from(format!("{id}-points"))).role(Role::Image).aria_label(spark_label(label, spark))
        .focusable().tab_stop(true).focus_visible(|el| el.border_1().border_color(theme::accent_focus()))
        .relative().w_full().h(px(SPARK_HEIGHT)).child(chart)
        .child(div().absolute().right(px(-5.)).top(px(last_y - 5.)).size(px(10.)).rounded_full()
            .border_2().border_color(theme::boxed()).bg(color))
}

impl Hangar {
    pub(in crate::app) fn render_status(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.status_ensure();
        // Servidor fora do ar: o painel dele no lugar dos blocos (o retrato não chega de um servidor parado).
        let down = if self.api.is_some() && !self.active_invite() { self.status_down_body(cx) } else { None };
        // O conserto pelo agente fica no lugar do diagnóstico e, consertado, até a pessoa voltar ao Status.
        let agent = if self.api.is_some() && !self.active_invite() { self.render_agent(cx) } else { None };
        let page = &self.device.status;
        let note = |text: String| settings_box().px_4().py(px(18.)).text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(text);
        let ready = matches!(page.view.value, Some(Ok(_)));
        let top = div().flex().items_baseline().gap(px(10.))
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr("status_title")))
            .when(ready && page.live, |el| el.child(div().ml_auto().flex().items_center().gap(px(6.)).text_size(px(11.5)).text_color(theme::faint())
                .child(div().size(px(6.)).rounded_full().bg(theme::success())).child(tr("status_live"))));
        let body: Vec<AnyElement> = match (&self.api, &page.view.value) {
            (None, _) => vec![note(tr("settings_offline")).into_any_element()],
            _ if self.active_invite() => vec![note(tr("status_owner_only")).into_any_element()],
            _ if agent.is_some() => agent.unwrap_or_default(),
            _ if down.is_some() => down.unwrap_or_default(),
            (_, Some(Ok(blocks))) => blocks.iter().map(|block| self.status_block(block, cx)).collect(),
            (_, Some(Err(text))) => vec![self.status_error(text.clone(), page.detail.clone(), cx)],
            (_, None) => self.status_loading(),
        };
        div().flex().flex_col().gap(px(12.)).child(top).children(body).into_any_element()
    }

    fn status_block(&self, block: &Block, cx: &mut Context<Self>) -> AnyElement {
        let flash = block.target().is_some_and(|t| self.device.status.flash == Some(t));
        let el = match block {
            Block::Health(h) => self.status_health(h, cx),
            Block::Server(s) => status_server(s, flash),
            Block::Days(d) => status_days(d, flash),
            Block::Machines(m) => status_machines(m, flash),
            Block::Sessions(s) => self.status_sessions(s, flash, cx),
            Block::Slow(s) => status_slow(s, flash),
            Block::Host(h) => status_host(h, flash),
            Block::Outside(o) => status_outside(o, flash),
        };
        let el = match block.target() { Some(target) => self.mark(el, super::target_key(target)), None => el };
        // A busca acha a CPU pela mesma faixa da memória.
        let el = if matches!(block, Block::Server(_)) { self.mark(el, "status_cpu") } else { el };
        el.into_any_element()
    }

    fn status_health(&self, h: &Health, cx: &mut Context<Self>) -> Div {
        let this = cx.entity().downgrade();
        health_card(h, cx, move |button, target| {
            let this = this.clone();
            button.on_click(move |_, _, cx| { let _ = this.update(cx, |this, cx| this.status_jump(target, cx)); })
        })
    }
}

/// O cartão do topo; `wire` liga cada ponto de atenção ao salto até o bloco dele.
pub(crate) fn health_card(h: &Health, cx: &App, wire: impl Fn(Button, AttentionTarget) -> Button) -> Div {
    let ok = h.items.is_empty();
    let color = if ok { theme::success() } else { theme::warning() };
    // A versão em fonte mono no meio da frase: o texto em volta dela sai da própria chave.
    let version = h.version.clone().map(|v| {
        let text = tr("status_version");
        let (before, after) = text.split_once("{version}").map_or((text.clone(), String::new()), |(b, a)| (b.trim_end().to_owned(), a.trim_start().to_owned()));
        div().flex().items_baseline().gap(px(4.)).when(!before.is_empty(), |el| el.child(before))
            .child(div().font_family(theme::MONO).child(v)).when(!after.is_empty(), |el| el.child(after))
    });
    let sub = (h.uptime.is_some() || version.is_some()).then(|| div().flex().items_baseline().gap(px(4.)).text_size(px(12.5)).text_color(theme::muted())
        .children(h.uptime.clone())
        .when(h.uptime.is_some() && version.is_some(), |el| el.child("·"))
        .children(version));
    let attention = (!ok).then(|| div().ml_auto().min_w_0().max_w(relative(0.56)).flex().flex_col().gap(px(6.))
        .children(h.items.iter().enumerate().map(|(i, item)| {
            let button = Button::new(SharedString::from(format!("status-attention-{i}")))
                .custom(ButtonCustomVariant::new(cx).color(theme::inset()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
                .w_full().h(px(28.)).px(px(9.)).rounded(px(8.)).accessibility_label(item.text.clone())
                .child(div().w_full().min_w_0().flex().items_center().gap(px(8.)).text_size(px(12.5))
                    .child(chrome::small_icon(IconName::TriangleAlert, 13., tone(item.tone)))
                    .child(div().flex_1().min_w_0().truncate().child(item.text.clone())));
            wire(button, item.target)
        })));
    settings_box().flex_row().items_center().gap(px(16.)).p(px(16.))
        .child(div().size(px(52.)).flex_shrink_0().rounded_full().bg(color.opacity(0.18)).flex().items_center().justify_center()
            .child(chrome::small_icon(if ok { IconName::Check } else { IconName::TriangleAlert }, if ok { 26. } else { 24. }, color)))
        .child(div().min_w_0().flex().flex_col().gap(px(3.))
            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(h.title.clone()))
            .children(sub))
        .children(attention)
}

impl Hangar {

    fn status_sessions(&self, s: &Sessions, flash: bool, cx: &mut Context<Self>) -> Div {
        let head = card_head(tr("status_sessions_title"), Some(s.hint.clone()));
        if s.empty {
            return card(flash).child(head).child(div().h(px(8.)).rounded(px(4.)).bg(theme::border()))
                .child(div().mt(px(10.)).text_size(px(12.)).text_color(theme::faint()).whitespace_normal().child(tr("status_sessions_empty")))
                .children((!s.chips.is_empty()).then(|| chip_row(&s.chips)));
        }
        let by_process = self.device.status.by_process;
        let tabs = super::super::settings::segments("status-split", &[tr("status_by_provider"), tr("status_by_process")], usize::from(by_process), 2, false,
            String::new(), |this, n, _, cx| { this.device.status.by_process = n == 1; cx.notify(); }, cx);
        let segs = if by_process { &s.by_process } else { &s.by_provider };
        card(flash).child(head.child(div().ml_auto().flex_shrink_0().child(tabs)))
            .child(seg_bar("status-sessions", segs, true))
            .child(legend().mt(px(8.)).children(segs.iter().map(|seg| legend_item(series(seg.slot), false, seg.label.clone(), seg.count.clone()))))
            .children((!s.chips.is_empty()).then(|| chip_row(&s.chips)))
    }

    fn status_loading(&self) -> Vec<AnyElement> {
        let block = |key: &'static str, height: f32| card(false)
            .child(Skeleton::new(SharedString::from(format!("{key}-title"))).w(px(120.)).h(px(12.)).rounded(px(6.)))
            .child(Skeleton::new(SharedString::from(format!("{key}-body"))).row(1).mt(px(12.)).h(px(height)).rounded(px(6.)));
        vec![
            settings_box().flex_row().items_center().gap(px(16.)).p(px(16.))
                .child(Skeleton::new("status-sk-icon").size(px(52.)).flex_shrink_0().rounded_full())
                .child(div().flex_1().flex().flex_col().gap(px(8.))
                    .child(Skeleton::new("status-sk-title").w(px(160.)).h(px(16.)).rounded(px(6.)))
                    .child(Skeleton::new("status-sk-sub").row(1).w(px(200.)).h(px(11.)).rounded(px(6.))))
                .into_any_element(),
            div().flex().gap(px(12.)).child(block("status-sk-mem", 60.).flex_1()).child(block("status-sk-cpu", 60.).flex_1()).into_any_element(),
            block("status-sk-days", 58.).into_any_element(),
            block("status-sk-machines", 110.).into_any_element(),
        ]
    }

    fn status_error(&self, text: String, detail: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let color = theme::warning();
        settings_box().flex_row().items_start().gap(px(16.)).p(px(16.))
            .child(div().size(px(52.)).flex_shrink_0().rounded_full().bg(color.opacity(0.18)).flex().items_center().justify_center()
                .child(chrome::small_icon(IconName::TriangleAlert, 24., color)))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(tr("status_error_title")))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text))
                .child(div().mt(px(9.)).flex().gap(px(8.))
                    .child(Button::new("status-retry").outline().small().icon(IconName::RotateCw).label(tr("status_retry"))
                        .on_click(cx.listener(|this, _, _, cx| this.status_retry(cx))))
                    .children(detail.map(|detail| Button::new("status-copy-detail").outline().small().icon(IconName::Copy).label(tr("status_copy_detail"))
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(detail.clone())))))))
            .into_any_element()
    }
}

fn stat_card(id: &'static str, label: String, spark: &Spark, flash: bool) -> Div {
    card(flash).flex_1().min_w_0()
        .child(div().text_size(px(12.)).text_color(theme::muted()).child(label.clone()))
        .child(div().mt(px(2.)).flex().items_baseline().gap(px(6.)).min_w_0()
            .child(div().text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).child(spark.value.clone()))
            .children(spark.average.clone().map(|average| div().truncate().text_size(px(12.)).text_color(theme::muted()).child(average))))
        // Uma amostra só não faz curva: a primeira aparece depois de um minuto de servidor.
        .when(spark.points.len() > 1, |el| el
            .child(spark_chart(id, &label, spark).mt(px(8.)))
            .child(div().mt(px(3.)).flex().justify_between().text_size(px(10.5)).text_color(theme::faint())
                .child(tr("status_hour_back")).child(tr("status_now"))))
}

pub(crate) fn status_server(s: &ServerCards, flash: bool) -> Div {
    div().flex().gap(px(12.))
        .child(stat_card("status-spark-memory", tr("status_memory"), &s.memory, flash))
        .child(stat_card("status-spark-cpu", tr("status_cpu"), &s.cpu, flash))
}

fn event_mark(kind: ExitKind) -> Div {
    match kind {
        ExitKind::Crash => div().size(px(18.)).ml(px(-9.)).top(px(14.)).rounded_full().border_2().border_color(theme::boxed()).bg(theme::danger()),
        // O anel na cor do cartão é um losango maior por baixo, como a borda dos círculos.
        ExitKind::Update => div().size(px(20.)).ml(px(-10.)).top(px(13.))
            .child(div().absolute().top(px(0.)).left(px(0.)).child(diamond(20., theme::boxed())))
            .child(div().absolute().top(px(3.)).left(px(3.)).child(diamond(14., theme::accent()))),
        ExitKind::Requested => div().size(px(14.)).ml(px(-7.)).top(px(16.)).rounded_full().border_2().border_color(theme::boxed()).bg(theme::muted()),
    }
}

pub(crate) fn status_days(d: &Days, flash: bool) -> Div {
    let timeline = d.timeline.as_ref().map(|t| {
        let line = div().relative().w_full().h(px(58.)).mt(px(4.))
            .child(div().absolute().left(px(0.)).right(px(0.)).top(px(22.)).h(px(2.)).rounded(px(1.)).bg(theme::border()))
            .children((0..7).map(|i| div().absolute().top(px(18.)).left(relative(i as f32 / 7.)).w(px(1.)).h(px(10.)).bg(theme::border())))
            .children(t.days.iter().enumerate().map(|(i, day)| div().absolute().top(px(30.)).left(relative(i as f32 / 7.)).w(relative(1. / 7.))
                .text_center().whitespace_nowrap().text_size(px(10.5)).text_color(theme::faint()).child(day.clone())))
            .children(t.events.iter().enumerate().map(|(i, e)| {
                let text = match &e.detail { Some(detail) => format!("{} · {detail}", e.title), None => e.title.clone() };
                tip(event_mark(e.kind).absolute().left(relative(e.pos)).id(SharedString::from(format!("status-event-{i}"))), text)
            }));
        let marks = legend()
            .child(legend_item(theme::danger(), true, tr("status_legend_crash"), None))
            .child(div().flex().items_center().gap(px(6.)).child(diamond(10., theme::accent())).child(tr("status_legend_update")))
            .when(t.has_restart, |el| el.child(legend_item(theme::muted(), true, tr("status_legend_restart"), None)));
        div().child(line).child(marks)
    });
    let has_timeline = timeline.is_some();
    let update = d.update.as_ref().map(|u| div().flex().items_center().gap(px(10.)).text_size(px(12.5))
        .when(has_timeline, |el| el.mt(px(12.)).pt(px(12.)).border_t_1().border_color(theme::border()))
        .child(div().size(px(28.)).flex_shrink_0().rounded(px(8.)).bg(theme::accent_dim()).flex().items_center().justify_center()
            .child(chrome::small_icon(IconName::ArrowUp, 15., theme::accent_text())))
        .child(div().min_w_0().flex().flex_col()
            .child(u.title.clone())
            .child(div().text_size(px(11.5)).text_color(theme::faint()).whitespace_normal().child(u.meta.clone()))));
    card(flash).child(card_head(tr("status_days_title"), d.hint.clone())).children(timeline).children(update)
}

fn signal(bars: u8) -> Div {
    div().flex().items_end().gap(px(2.)).h(px(11.)).ml(px(6.))
        .children([5., 8., 11.].into_iter().enumerate().map(|(i, h)| div().w(px(3.)).h(px(h)).rounded(px(1.))
            .bg(if (i as u8) < bars { theme::success() } else { theme::border_strong() })))
}

fn machine_line(icon: IconName, content: Div) -> Div {
    div().mt(px(7.)).flex().items_center().gap(px(6.)).min_w_0().whitespace_nowrap().overflow_hidden().text_size(px(11.5)).text_color(theme::muted())
        .child(chrome::small_icon(icon, 13., theme::faint())).child(content)
}

fn status_machines(m: &Machines, flash: bool) -> Div {
    let card_of = |c: &super::model::MachineCard| {
        let right = match (&c.latency, &c.state) {
            (Some((ms, bars)), _) => div().ml_auto().flex_shrink_0().flex().items_center().text_size(px(11.)).text_color(theme::muted())
                .child(ms.clone()).child(signal(*bars)),
            (None, Some((text, t))) => div().ml_auto().flex_shrink_0().text_size(px(11.)).text_color(if *t == Tone::Bad { theme::danger() } else { theme::muted() })
                .child(text.clone()),
            (None, None) => div(),
        };
        let server = div().flex().items_center().gap(px(6.)).min_w_0()
            .child(tr("status_machine_server"))
            .child(div().font_family(theme::MONO).text_color(theme::text()).child(c.server.clone()))
            .when(c.outdated, |el| el.child(div().px(px(5.)).rounded(px(4.)).bg(theme::warning().opacity(0.16)).text_size(px(10.5))
                .text_color(theme::warning_text()).child(tr("status_machine_old"))));
        let app = match &c.app {
            Some(app) => div().flex().items_center().gap(px(6.)).min_w_0().child(tr("status_machine_app")).child(div().truncate().text_color(theme::text()).child(app.clone())),
            None => div().text_color(theme::faint()).child(tr("status_machine_no_app")),
        };
        div().id(SharedString::from(format!("status-machine-{}", c.id))).flex_1().min_w_0().rounded(px(12.)).border_1().border_color(theme::border())
            .bg(theme::inset().opacity(0.45)).px(px(12.)).py(px(10.))
            .child(div().flex().items_center().gap(px(7.)).min_w_0().text_size(px(12.5)).font_weight(FontWeight::MEDIUM)
                .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(tone(c.tone)))
                .child(div().min_w_0().truncate().child(c.name.clone()))
                .children(c.tag.clone().map(|tag| div().flex_shrink_0().text_size(px(10.5)).font_weight(FontWeight::NORMAL).text_color(theme::faint()).child(tag)))
                .child(right))
            .child(machine_line(IconName::Server, server))
            .child(machine_line(IconName::Monitor, app))
    };
    // Duas por linha; a sobra da última fica vazia para as colunas não esticarem.
    let rows = m.cards.chunks(2).map(|pair| div().flex().gap(px(12.)).children(pair.iter().map(|c| card_of(c).into_any_element()))
        .when(pair.len() == 1, |el| el.child(div().flex_1())));
    card(flash).child(card_head(tr("status_machines_title"), Some(tr("status_machines_hint"))))
        .child(div().flex().flex_col().gap(px(12.)).children(rows))
        .when(m.alone, |el| el.child(div().mt(px(8.)).text_size(px(12.)).text_color(theme::faint()).child(tr("status_machines_alone"))))
}

fn status_slow(s: &Slow, flash: bool) -> Div {
    const H: f32 = 70.;
    let bar = |count: u32, color: Hsla| div().w(relative(0.6)).max_w(px(14.)).h(px(count as f32 / s.max as f32 * H)).rounded(px(3.)).bg(color);
    let columns = div().relative().w_full().h(px(H + 14.))
        .child(div().absolute().left(px(0.)).right(px(0.)).top(px(0.)).h(px(1.)).bg(theme::border()))
        .child(div().absolute().left(px(0.)).right(px(0.)).top(px(H / 2.)).h(px(1.)).bg(theme::border()))
        .child(div().absolute().left(px(0.)).right(px(0.)).top(px(H)).h(px(1.)).bg(theme::border_strong()))
        .child(div().absolute().left(px(0.)).right(px(0.)).top(px(0.)).h(px(H)).flex()
            .children(s.hours.iter().enumerate().map(|(i, hour)| tip(div().id(SharedString::from(format!("status-hour-{i}"))).flex_1().h_full()
                .flex().flex_col().justify_end().items_center().gap(px(2.))
                .when(hour.errors > 0, |el| el.child(bar(hour.errors, theme::danger())))
                .when(hour.slow > 0, |el| el.child(bar(hour.slow, theme::warning()))),
                if hour.when.is_empty() { hour.tip.clone() } else { format!("{} · {}", hour.tip, hour.when) }))))
        .child(div().absolute().left(px(0.)).right(px(0.)).top(px(H + 2.)).flex()
            .children(s.hours.iter().map(|hour| div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_center().text_size(px(10.))
                .text_color(theme::faint()).children(hour.label.clone()))));
    let marks = legend().mt(px(6.))
        .child(legend_item(theme::warning(), false, tr("status_slow_legend"), None))
        .child(legend_item(theme::danger(), false, tr("status_errors_legend"), None))
        .children(s.watchdog.clone().map(|text| div().ml_auto().flex().items_center().gap(px(6.))
            .child(chrome::small_icon(IconName::TriangleAlert, 12., theme::warning())).child(text)));
    let routes = (!s.routes.is_empty()).then(|| div().mt(px(10.)).flex().flex_col().gap(px(5.)).children(s.routes.iter().enumerate().map(|(i, r)| {
        let row = div().id(SharedString::from(format!("status-route-{i}"))).flex().items_center().gap(px(8.)).text_size(px(11.5))
            .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_color(theme::muted()).child(r.route.clone()))
            .child(div().flex_shrink_0().w(px(72. * r.share)).h(px(6.)).rounded(px(3.)).bg(if r.bad { theme::danger() } else { theme::warning() }))
            .child(div().flex_shrink_0().w(px(44.)).text_right().child(r.value.clone()));
        // A linha corta o caminho: o tooltip o leva inteiro, senão duas rotas de mesmo começo ficam iguais.
        tip(row, format!("{} · {}", r.route, r.tip))
    })));
    card(flash).child(card_head(tr("status_slow_title"), Some(s.hint.clone()))).child(columns).child(marks).children(routes)
}

fn status_host(h: &Host, flash: bool) -> Div {
    let meters = h.meters.iter().enumerate().map(|(i, m)| tip(div().id(SharedString::from(format!("status-meter-{i}"))).h(px(30.)).flex().items_center()
        .gap(px(10.)).text_size(px(12.5))
        .child(div().w(px(96.)).flex_shrink_0().truncate().text_color(theme::muted()).child(m.name.clone()))
        .child(div().flex_1().min_w_0().child(Progress::new(SharedString::from(format!("status-meter-bar-{i}"))).value(m.pct).color(tone(m.tone))
            .accessibility_label(format!("{}: {}", m.name, m.tip))))
        .child(div().w(px(104.)).flex_shrink_0().text_right().whitespace_nowrap().child(m.figure.clone())), m.tip.clone()));
    card(flash).child(card_head(tr("status_host_title"), Some(h.hint.clone()))).children(meters)
        .child(div().mt(px(10.)).text_size(px(12.)).text_color(theme::muted()).child(tr("status_disk_usage")))
        .child(div().mt(px(6.)).child(seg_bar("status-disk", &h.disk, false)))
        .child(legend().mt(px(8.)).children(h.disk.iter().map(|seg| legend_item(series(seg.slot), false, seg.label.clone(), seg.count.clone()))))
}

fn status_outside(o: &Outside, flash: bool) -> Div {
    let tiles = (!o.tiles.is_empty()).then(|| div().flex().gap(px(10.)).children(o.tiles.iter().map(|t| {
        let color = tone(t.tone);
        let icon = match t.icon { TileIcon::Globe => IconName::Globe, TileIcon::Tunnel => IconName::Link, TileIcon::Key => IconName::KeyRound };
        div().flex_1().min_w_0().flex().items_center().gap(px(10.)).px(px(12.)).py(px(10.)).rounded(px(12.)).border_1().border_color(theme::border())
            .bg(theme::inset().opacity(0.45))
            .child(div().size(px(30.)).flex_shrink_0().rounded(px(9.)).bg(color.opacity(0.16)).flex().items_center().justify_center()
                .child(chrome::small_icon(icon, 16., color)))
            .child(div().min_w_0().flex().flex_col()
                .child(div().truncate().text_size(px(12.)).text_color(theme::muted()).child(t.title.clone()))
                .child(div().whitespace_nowrap().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(t.value.clone())))
    })));
    let has_tiles = tiles.is_some();
    card(flash).child(card_head(tr("status_outside_title"), None)).children(tiles)
        .when(!o.chips.is_empty(), |el| el.child(if has_tiles { chip_row(&o.chips) } else { chip_row(&o.chips).mt(px(0.)) }))
}
