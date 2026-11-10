//! Resumo da máquina ativa na tela inicial, independente dos filtros de Custos.
use super::*;
use super::costs::{Bucket, Report, dec, money2, tok, web, web_with};
use super::device::Remote;
use chrono::{Datelike, NaiveDate};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct HomeUsage {
    connection: Option<u64>,
    period: Period,
    models: bool,
    report: Remote<Summary>,
    warming: Option<(u64, u64)>,
    tries: u32,
    refreshed_at: Option<Instant>,
    /// Dia sob o mouse no gráfico: a linha abaixo dele mostra o uso desse dia.
    hover_day: Option<NaiveDate>,
}

/// O uso de um dia no gráfico de atividade.
#[derive(Clone, Copy, Default)]
struct DayUse { tokens: f64, cost: f64, sessions: f64 }

#[derive(Clone, Copy, Default, PartialEq)]
enum Period { #[default] All, Month, Week, Day }

impl Period {
    fn key(self) -> &'static str { match self { Self::All => "all", Self::Month => "30d", Self::Week => "7d", Self::Day => "1d" } }
    fn label(self) -> String { web(match self { Self::All => "home_usage_all", Self::Month => "home_usage_30d", Self::Week => "home_usage_7d", Self::Day => "home_usage_1d" }) }
}

struct Summary {
    totals: Bucket,
    models: Vec<Bucket>,
    active_days: usize,
    days: BTreeMap<NaiveDate, DayUse>,
    rate: Option<f64>,
    partial_cost: bool,
}

impl Summary {
    fn parse(value: Value, period: Period) -> Result<Self, String> {
        if value.pointer("/applied/period").and_then(Value::as_str) != Some(period.key()) {
            return Err(web("home_usage_period_unsupported"));
        }
        if !value.get("totals").is_some_and(Value::is_object) { return Err(tr("invalid_response")); }
        let report: Report = serde_json::from_value(value).map_err(|_| tr("invalid_response"))?;
        let mut models = report.by_model;
        models.retain(|b| b.raw() > 0.);
        models.sort_by(|a, b| b.raw().total_cmp(&a.raw()).then(a.key.cmp(&b.key)));
        let mut days: BTreeMap<NaiveDate, DayUse> = BTreeMap::new();
        for bucket in report.by_day {
            let day = NaiveDate::parse_from_str(&bucket.key, "%Y-%m-%d").map_err(|_| tr("invalid_response"))?;
            let entry = days.entry(day).or_default();
            entry.tokens += bucket.raw();
            entry.cost += bucket.cost;
            entry.sessions += bucket.sessions;
        }
        let active_days = days.values().filter(|d| d.tokens > 0.).count();
        Ok(Self { totals: report.totals, models, active_days, days, rate: report.usd_brl, partial_cost: !report.sem_tarifa.is_empty() })
    }
}

impl Hangar {
    pub(super) fn home_usage_opened(&mut self, cx: &mut Context<Self>) {
        if self.home_usage.connection != Some(self.connection) {
            self.home_usage.connection = Some(self.connection);
            self.home_usage.report.reset();
            self.home_usage.hover_day = None;
        }
        let stale = self.home_usage.refreshed_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(60));
        if self.api.is_some() && (self.home_usage.report.value.is_none() || stale) && !self.home_usage.report.loading {
            self.load_home_usage(cx);
        }
    }

    fn load_home_usage(&mut self, cx: &mut Context<Self>) {
        let seq = self.home_usage.report.start();
        self.home_usage.warming = None;
        self.home_usage.tries = 0;
        self.fetch_home_usage(seq, cx);
    }

    fn fetch_home_usage(&mut self, seq: u64, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else {
            self.home_usage.report.finish(seq, Err(tr("choose_session")));
            cx.notify();
            return;
        };
        let (connection, period) = (self.connection, self.home_usage.period);
        let task = self.runtime.spawn(async move { api.server_read(&["costs"], &[("period", period.key()), ("view", "summary")], 120).await });
        cx.spawn(async move |this, cx| {
            let result = match task.await {
                Ok(result) => result.map_err(|error| Self::failure(&error)),
                Err(_) => Err(web("home_usage_load_failed")),
            };
            let _ = this.update(cx, |this, cx| {
                if this.connection != connection || this.home_usage.report.seq != seq { return; }
                this.home_usage.refreshed_at = Some(Instant::now());
                match result {
                    Ok(value) if value.get("aquecendo") == Some(&Value::Bool(true)) => {
                        let read = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
                        this.home_usage.warming = Some((read("lidos"), read("total")));
                        if this.home_usage.tries >= 100 || !this.new_chat_screen() {
                            this.home_usage.warming = None;
                            this.home_usage.report.finish(seq, Err(web("home_usage_warming_timeout")));
                        } else {
                            this.home_usage.tries += 1;
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(Duration::from_secs(3)).await;
                                let _ = this.update(cx, |this, cx| {
                                    if this.connection == connection && this.home_usage.report.seq == seq {
                                        if this.new_chat_screen() { this.fetch_home_usage(seq, cx); }
                                        else { this.home_usage.report.reset(); this.home_usage.warming = None; }
                                    }
                                });
                            }).detach();
                        }
                    }
                    result => {
                        this.home_usage.warming = None;
                        this.home_usage.report.finish(seq, result.and_then(|v| Summary::parse(v, period)));
                    }
                }
                cx.notify();
            });
        }).detach();
    }

    pub(super) fn render_home_usage(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.home_usage;
        let header = div().flex().flex_wrap().items_center().justify_between().gap_2()
            .child(div().flex().gap_1().children([(false, "home_usage_overview"), (true, "home_usage_models")].map(|(models, key)| {
                tab_button(key.into(), web(key), state.models == models, cx)
                    .on_click(cx.listener(move |this, _, _, cx| { this.home_usage.models = models; cx.notify(); }))
            })))
            .child(div().flex().gap_1().children([Period::All, Period::Month, Period::Week, Period::Day].map(|period| {
                tab_button(SharedString::from(format!("home-usage-{}", period.key())), period.label(), state.period == period, cx)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Período novo: os números do anterior ficariam errados na tela enquanto carrega.
                        if this.home_usage.period != period {
                            this.home_usage.period = period;
                            this.home_usage.report.reset();
                            // A grade é refeita e o `on_hover(false)` do dia antigo não chega.
                            this.home_usage.hover_day = None;
                            this.load_home_usage(cx);
                            cx.notify();
                        }
                    }))
            })));
        // O mesmo material do compositor: sem ele, os números e o mapa somem sobre o papel de parede.
        let mut card = div().w_full().flex().flex_col().gap_3().p_3().rounded_lg().bg(theme::boxed())
            .border_1().border_color(theme::border_strong()).child(header);
        // A recarga de cada minuto mantém o resumo na tela; "carregando" só sem nada para mostrar.
        if state.report.loading && state.report.value.is_none() {
            let text = state.warming.map_or_else(|| tr("loading"), |(read, total)|
                web_with("home_usage_warming", &[("read", read.to_string()), ("total", total.to_string())]));
            return card.child(div().id("home-usage-loading").role(Role::Status).text_sm().text_color(theme::muted()).child(text));
        }
        let report = match &state.report.value {
            Some(Ok(report)) => report,
            Some(Err(error)) => return card.child(div().flex().flex_col().gap_2()
                .child(div().id("home-usage-error").role(Role::Alert).text_sm().text_color(theme::warning()).child(error.clone()))
                .child(Button::new("home-usage-retry").ghost().small().label(tr("retry"))
                    .on_click(cx.listener(|this, _, _, cx| { this.load_home_usage(cx); cx.notify(); })))),
            None => return card,
        };
        if report.totals.sessions == 0. && report.totals.raw() == 0. {
            return card.child(div().text_sm().text_color(theme::muted()).child(web("home_usage_empty")));
        }
        // Trocar a aba não deve deslocar o compositor.
        card = card.min_h(rems(24.));
        if state.models {
            if report.models.is_empty() {
                return card.child(div().text_sm().text_color(theme::muted()).child(web("home_usage_empty")));
            }
            let max = report.models.first().map_or(1., Bucket::raw).max(1.);
            card = card.child(div().id("home-usage-model-list").max_h(rems(14.)).overflow_y_scroll().flex().flex_col().gap_2()
                .children(report.models.iter().map(|model| div().flex().flex_col().gap_1()
                    .child(div().flex().items_center().gap_2().text_sm()
                        .child(div().min_w_0().flex_1().truncate().child(model.label.clone().unwrap_or_else(|| crate::status::model_label(&model.key))))
                        .child(tok(model.raw())))
                    .child(div().w_full().h_1().rounded_full().bg(theme::inset())
                        .child(div().h_full().rounded_full().bg(theme::accent()).w(relative((model.raw() / max) as f32)))))));
        } else {
            let favorite = report.models.first().map(|m| m.label.clone().unwrap_or_else(|| crate::status::model_label(&m.key))).unwrap_or_else(|| "—".into());
            let metrics = [("home_usage_sessions", dec(report.totals.sessions, 0)), ("home_usage_tokens", tok(report.totals.raw())),
                ("home_usage_cost", money2(report.totals.cost, report.rate)), ("home_usage_active_days", report.active_days.to_string()),
                ("home_usage_model_count", report.models.len().to_string()), ("home_usage_top_model", favorite)];
            for row in metrics.chunks(3) {
                card = card.child(div().flex().gap_3().children(row.iter().map(|(key, value)|
                    div().flex_1().min_w_0().flex().flex_col().gap_1()
                        .child(div().text_xs().text_color(theme::muted()).child(web(key)))
                        .child(div().flex().items_center().gap(px(6.)).min_w_0()
                            .child(div().id(SharedString::from(format!("home-{key}"))).min_w_0().text_sm().font_weight(FontWeight::MEDIUM)
                                .truncate().child(value.clone()).tooltip({ let value = value.clone(); move |w, cx| gpui_kit::component::tooltip::Tooltip::new(value.clone()).build(w, cx) }))
                            // Preço de tabela, não fatura: a nota do rodapé ficava longe do número.
                            .when(*key == "home_usage_cost", |el| el.child(div().flex_shrink_0().px(px(5.)).rounded(px(5.))
                                .border_1().border_color(theme::border_strong()).text_size(px(10.5)).text_color(theme::faint())
                                .child(tr("home_usage_cost_tag"))))))));
            }
            card = card.child(self.activity_calendar(report, cx));
        }
        card.child(div().text_xs().text_color(if report.partial_cost { theme::warning() } else { theme::muted() })
            .child(web(if report.partial_cost { "home_usage_partial" } else { "home_usage_method" })))
    }
}

/// Aba e período do resumo: o escolhido em destaque suave, como os segmentos das Configurações; o `selected` do ghost
/// some sobre o papel de parede.
fn tab_button(id: SharedString, label: String, on: bool, cx: &App) -> Button {
    Button::new(id).small().aria_selected(on).label(label)
        .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
            .foreground(if on { theme::accent_text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
        // Cor e fundo no próprio botão: o `selected` do kit pinta o fundo com a cor de pressionado.
        .text_color(if on { theme::accent_text() } else { theme::muted() })
        .when(on, |el| el.bg(theme::accent_dim()))
}

// A data vem do histórico: o cliente não desloca os dias agregados no fuso do servidor.
fn calendar_bounds<V>(days: &BTreeMap<NaiveDate, V>) -> Option<(NaiveDate, NaiveDate)> {
    Some((*days.first_key_value()?.0, *days.last_key_value()?.0))
}

/// "06/08/2026 · 28,9 Mi tokens · R$ 12,30 · 14 sessões", ou o dia sem uso.
fn day_detail(day: NaiveDate, usage: Option<&DayUse>, rate: Option<f64>) -> String {
    let date = day.format("%d/%m/%Y").to_string();
    match usage.filter(|u| u.tokens > 0.) {
        Some(u) => tr("home_usage_day_detail").replace("{date}", &date).replace("{tokens}", &tok(u.tokens))
            .replace("{cost}", &money2(u.cost, rate)).replace("{sessions}", &dec(u.sessions, 0)),
        None => tr("home_usage_day_empty").replace("{date}", &date),
    }
}

impl Hangar {
    /// Um quadrado por dia, semanas em colunas (segunda em cima). A linha de baixo mostra o dia sob o
    /// mouse na hora e, sem mouse, o dia de mais uso: dica flutuante demorava e saía fora do tema.
    fn activity_calendar(&self, report: &Summary, cx: &mut Context<Self>) -> Div {
        let Some((start, end)) = calendar_bounds(&report.days) else { return div(); };
        let first = start - chrono::Duration::days(start.weekday().num_days_from_monday() as i64);
        let weeks = ((end - first).num_days() / 7 + 1) as usize;
        let max = report.days.values().map(|d| d.tokens).fold(1., f64::max);
        let months: Vec<String> = tr("home_usage_months").split(',').map(str::to_owned).collect();
        let weekdays: Vec<String> = tr("home_usage_weekdays").split(',').map(str::to_owned).collect();
        let label = |text: String| div().h(px(12.)).text_size(px(9.5)).line_height(px(12.)).text_color(theme::faint())
            .whitespace_nowrap().child(text);
        // Segunda, quarta e sexta, como no GitHub: as sete linhas sem rótulo não diziam qual dia era qual.
        let day_names = div().flex().flex_col().gap_1().flex_shrink_0().child(label(String::new()))
            .children((0..7).map(|row| label(if row % 2 == 0 && row < 6 { weekdays.get(row / 2).cloned().unwrap_or_default() } else { String::new() })));
        let mut grid = div().id("home-usage-calendar").min_w_0().overflow_x_scroll().flex().gap_1();
        let mut shown_month = None;
        // O último dia visível da coluna: o mês que começa no meio da semana já leva o nome nela.
        let month_at = |week: usize| (first + chrono::Duration::days((week * 7 + 6) as i64)).min(end).month0();
        for week in 0..weeks {
            let month = (shown_month != Some(month_at(week))).then(|| {
                shown_month = Some(month_at(week));
                months.get(month_at(week) as usize).cloned().unwrap_or_default()
            })
            // A primeira semana só leva o nome se o mês dura três colunas: senão ele cobriria o seguinte.
            .filter(|_| week > 0 || weeks < 3 || month_at(2) == month_at(0));
            // Largura da coluna: o nome do mês passa por cima das semanas seguintes sem alargar a dele.
            grid = grid.child(div().flex().flex_col().gap_1().child(label(month.unwrap_or_default()).w(px(12.))).children((0..7).map(|weekday| {
                let day = first + chrono::Duration::days((week * 7 + weekday) as i64);
                let usage = report.days.get(&day);
                let value = usage.map_or(0., |d| d.tokens);
                let visible = day >= start && day <= end;
                let hovered = self.home_usage.hover_day == Some(day);
                div().id(SharedString::from(format!("home-day-{day}"))).size_3().flex_shrink_0().rounded_sm()
                    .bg(if value > 0. { theme::accent().opacity(0.3 + 0.7 * (value / max) as f32) } else { theme::inset() })
                    .when(hovered, |el| el.border_1().border_color(theme::text()))
                    .when(!visible, |el| el.opacity(0.))
                    .aria_label(day_detail(day, usage, report.rate))
                    .when(visible, |el| el.on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                        if *inside { this.home_usage.hover_day = Some(day); }
                        else if this.home_usage.hover_day == Some(day) { this.home_usage.hover_day = None; }
                        cx.notify();
                    })))
            })));
        }
        let shown = self.home_usage.hover_day.filter(|day| *day >= start && *day <= end);
        let line = match shown {
            Some(day) => day_detail(day, report.days.get(&day), report.rate),
            None => report.days.iter().filter(|(_, usage)| usage.tokens > 0.).max_by(|a, b| a.1.tokens.total_cmp(&b.1.tokens))
                .map(|(day, usage)| tr("home_usage_busiest").replace("{detail}", &day_detail(*day, Some(usage), report.rate)))
                .unwrap_or_default(),
        };
        let swatch = |level: Option<f32>| div().size(px(10.)).rounded_sm()
            .bg(level.map_or_else(theme::inset, |l| theme::accent().opacity(l)));
        let legend = div().ml_auto().flex().items_center().gap(px(3.)).text_size(px(10.5)).text_color(theme::faint())
            .child(tr("home_usage_less")).child(swatch(None))
            .children([0.4, 0.6, 0.8, 1.].map(|l| swatch(Some(l)))).child(tr("home_usage_more"));
        div().flex().flex_col().gap_2()
            .child(div().flex().items_center().gap_2()
                .child(div().text_xs().text_color(theme::muted()).child(web_with("home_usage_activity", &[
                    ("start", start.format("%d/%m/%Y").to_string()), ("end", end.format("%d/%m/%Y").to_string())])))
                .child(legend))
            .child(div().flex().gap(px(6.)).child(day_names).child(grid))
            // Altura fixa: trocar de dia não pode empurrar o compositor.
            .child(div().id("home-usage-day").h(px(16.)).text_xs().truncate()
                .text_color(if shown.is_some() { theme::text() } else { theme::muted() }).child(line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn summary_uses_all_token_kinds_and_ranks_by_tokens_not_cost() {
        let value = serde_json::json!({"applied":{"period":"7d"}, "totals":{"sessions":3,"input":1,"output":2,"cache_write":3,"cache_read":4},
            "by_model":[{"key":"expensive","input":10,"cost":20},{"key":"popular","input":2,"cache_read":100,"cost":1}],
            "by_day":[{"key":"2026-09-28","input":10},{"key":"2026-09-28","output":5},{"key":"2026-09-30","cache_read":10}],"sem_tarifa":["popular"]});
        let summary = Summary::parse(value.clone(), Period::Week).unwrap();
        assert_eq!(summary.totals.raw(), 10.);
        assert_eq!(summary.models[0].key, "popular");
        assert_eq!(summary.active_days, 2);
        let dia = summary.days[&NaiveDate::from_ymd_opt(2026, 9, 28).unwrap()];
        assert_eq!(dia.tokens, 15.);
        assert!(summary.partial_cost);
        assert_eq!(calendar_bounds(&summary.days).unwrap().0.to_string(), "2026-09-28");
        assert_eq!(calendar_bounds(&summary.days).unwrap().1.to_string(), "2026-09-30");
        assert!(Summary::parse(value, Period::Month).is_err());
        assert!(calendar_bounds(&BTreeMap::<NaiveDate, DayUse>::new()).is_none());
    }
}
