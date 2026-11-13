//! Dados e navegação das páginas Atualizações e Manual. Vêm do servidor desta máquina (sem ele, do ativo), já só com o
//! que vale para o desktop. As duas páginas dividem o estado: uma abre o detalhe da outra.
use super::*;
use crate::app::device::Remote;
use crate::app::vault::{alert, muted};
use chrono::Datelike;
use serde::Deserialize;

/// Tipo de cada opção do filtro, na ordem dos segmentos (Tudo, Novos, Melhorias, Correções).
pub(super) const FILTER_KINDS: [Option<&str>; 4] = [None, Some("new"), Some("improvement"), Some("fix")];

/// Largura a partir da qual o artigo leva o "Nesta página" ao lado do texto.
const ARTICLE_WIDE: f32 = 1320.;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct NewsItem {
    pub(super) id: String,
    /// `AAAA-MM-DD`: quando a novidade foi escrita.
    pub(super) date: String,
    pub(super) title: String,
    pub(super) kind: String,
    #[serde(default)]
    pub(super) platforms: String,
    #[serde(default)]
    pub(super) summary: String,
    #[serde(default)]
    pub(super) text: String,
    #[serde(default)]
    pub(super) more: bool,
    #[serde(default)]
    pub(super) try_it: Option<String>,
    #[serde(default)]
    pub(super) animation: Option<String>,
    #[serde(default)]
    pub(super) anchor: Option<String>,
    /// Artigo do Manual que explica a novidade.
    #[serde(default)]
    pub(super) manual: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct NewsRef { pub(super) id: String, pub(super) date: String, pub(super) title: String }

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Section { pub(super) id: String, pub(super) title: String, #[serde(default)] pub(super) markdown: String }

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Article {
    pub(super) id: String,
    pub(super) title: String,
    #[serde(default)]
    pub(super) summary: String,
    #[serde(default)]
    pub(super) icon: Option<String>,
    #[serde(default)]
    pub(super) platforms: String,
    #[serde(default)]
    pub(super) anchor: Option<String>,
    #[serde(default)]
    pub(super) intro: String,
    #[serde(default)]
    pub(super) sections: Vec<Section>,
    #[serde(default)]
    pub(super) news: Vec<NewsRef>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Category { pub(super) id: String, #[serde(default)] pub(super) articles: Vec<Article> }

/// Estado das duas páginas. Outro servidor começa do zero; os campos de busca ficam.
#[derive(Default)]
pub(in crate::app) struct Guide {
    /// `Api::identity` do servidor lido, decidido ao abrir as Configurações.
    server: Option<String>,
    api: Option<Api>,
    pub(super) news: Remote<Vec<NewsItem>>,
    pub(super) manual: Remote<Vec<Category>>,
    /// Ids que chegaram desde a última visita (`/api/update/arrived`): a bolinha da linha.
    pub(super) unread: HashSet<String>,
    arrived_seq: u64,
    through: Option<u64>,
    /// `through` já marcado como visto: o contador da navegação some.
    seen: Option<u64>,
    /// O servidor recusou (convidado): as duas entradas somem.
    forbidden: bool,
    pub(super) filter: usize,
    pub(super) category: Option<String>,
    pub(super) news_open: Option<String>,
    pub(super) article_open: Option<String>,
    /// Rolagem da lista quando o detalhe abriu: o voltar devolve a pessoa ao mesmo ponto.
    pub(super) list_offset: Option<Point<Pixels>>,
    /// Quando o detalhe abriu: o ciclo da animação conta daqui.
    pub(super) started: Option<Instant>,
    /// Seção do artigo pedida no "Nesta página".
    pub(super) section: Option<String>,
    updates_query: Option<Entity<InputState>>,
    pub(super) manual_query: Option<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
}

impl Guide {
    /// O "N novas" da navegação: zera quando a página marcou visto.
    pub(in crate::app) fn new_count(&self) -> usize {
        if self.seen.is_some() && self.seen == self.through { 0 } else { self.unread.len() }
    }
}

pub(super) fn for_desktop(platforms: &str) -> bool { matches!(platforms, "desktop" | "all") }

fn parse_news(value: Value) -> Result<Vec<NewsItem>, String> {
    #[derive(Deserialize)]
    struct Wire { news: Vec<NewsItem> }
    let Wire { mut news } = serde_json::from_value(value).map_err(|_| tr("invalid_response"))?;
    news.retain(|item| for_desktop(&item.platforms));
    Ok(news)
}

fn parse_manual(value: Value) -> Result<Vec<Category>, String> {
    #[derive(Deserialize)]
    struct Wire { categories: Vec<Category> }
    let Wire { mut categories } = serde_json::from_value(value).map_err(|_| tr("invalid_response"))?;
    for category in &mut categories { category.articles.retain(|article| for_desktop(&article.platforms)); }
    categories.retain(|category| !category.articles.is_empty());
    Ok(categories)
}

/// Os ids não vistos que valem para o desktop e o `through` a marcar; resposta fora do formato não marca nada.
fn parse_arrived(value: Value) -> Option<(HashSet<String>, u64)> {
    #[derive(Deserialize)]
    struct Item { id: String, #[serde(default)] platforms: String }
    #[derive(Deserialize)]
    struct Wire { #[serde(default)] news: Vec<Item>, through: u64 }
    let wire: Wire = serde_json::from_value(value).ok()?;
    Some((wire.news.into_iter().filter(|item| for_desktop(&item.platforms)).map(|item| item.id).collect(), wire.through))
}

/// A linha abre página própria quando há mais a mostrar que o resumo.
pub(super) fn opens(item: &NewsItem) -> bool { item.more || item.animation.as_deref().and_then(crate::news::animation).is_some() }

/// `filter` é o índice em `FILTER_KINDS`; a busca olha título, resumo e texto, sem acento nem caixa.
pub(super) fn shown_news<'a>(news: &'a [NewsItem], filter: usize, query: &str) -> Vec<&'a NewsItem> {
    let kind = FILTER_KINDS.get(filter).copied().flatten();
    let query = fold(query.trim());
    news.iter().filter(|item| kind.is_none_or(|kind| item.kind == kind))
        .filter(|item| query.is_empty() || [&item.title, &item.summary, &item.text].iter().any(|text| fold(text).contains(&query)))
        .collect()
}

/// Agrupa por dia na ordem que o servidor mandou (já do mais novo para o mais antigo).
pub(super) fn by_day(items: Vec<&NewsItem>) -> Vec<(&str, Vec<&NewsItem>)> {
    let mut days: Vec<(&str, Vec<&NewsItem>)> = Vec::new();
    for item in items {
        if let Some((date, list)) = days.last_mut() && *date == item.date { list.push(item); continue; }
        days.push((item.date.as_str(), vec![item]));
    }
    days
}

/// Artigos que têm a busca no título, no resumo, na abertura ou em alguma seção.
pub(super) fn manual_matches<'a>(categories: &'a [Category], query: &str) -> Vec<(&'a Category, &'a Article)> {
    let query = fold(query.trim());
    categories.iter().flat_map(|category| category.articles.iter().map(move |article| (category, article)))
        .filter(|(_, article)| query.is_empty() || [&article.title, &article.summary, &article.intro].iter().any(|text| fold(text).contains(&query))
            || article.sections.iter().any(|s| fold(&s.title).contains(&query) || fold(&s.markdown).contains(&query)))
        .collect()
}

/// A categoria aberta: a escolhida, se ainda existe, senão a primeira.
pub(super) fn current_category<'a>(categories: &'a [Category], picked: Option<&str>) -> Option<&'a Category> {
    categories.iter().find(|c| Some(c.id.as_str()) == picked).or_else(|| categories.first())
}

pub(super) fn category_title(id: &str) -> String {
    match id {
        "comecar" | "sessoes" | "ferramentas" | "problemas" => tr(&format!("manual_category_{id}")),
        other => other.to_owned(),
    }
}

/// "12 de novembro" e o dia da semana. O ano entra no `full` ou quando não é o de `today`.
pub(super) fn day_label(date: &str, today: chrono::NaiveDate, full: bool) -> Option<(String, String)> {
    let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let months = tr("guide_months");
    let month = months.split(',').nth(day.month0() as usize)?;
    let weekdays = tr("guide_weekdays");
    let weekday = weekdays.split(',').nth(day.weekday().num_days_from_sunday() as usize)?;
    let key = if full || day.year() != today.year() { "guide_day_year" } else { "guide_day" };
    Some((tr(key).replace("{day}", &day.day().to_string()).replace("{month}", month).replace("{year}", &day.year().to_string()),
        weekday.to_owned()))
}

/// "12 nov": as três primeiras letras do mês. Fora do formato, a data como veio.
pub(super) fn day_short(date: &str) -> String {
    let Ok(day) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else { return date.to_owned() };
    let months = tr("guide_months");
    let month: String = months.split(',').nth(day.month0() as usize).unwrap_or_default().chars().take(3).collect();
    tr("guide_day_short").replace("{day}", &day.day().to_string()).replace("{month}", &month)
}

/// O "Mostrar no app": só com âncora que o app desenha.
pub(super) fn note(id: &str, anchor: Option<&str>, title: &str, body: String) -> Option<crate::tour::Note> {
    let anchor = anchor.filter(|anchor| crate::tour::anchors::ALL.contains(anchor))?;
    Some(crate::tour::Note { id: id.to_owned(), anchor: anchor.to_owned(), title: title.to_owned(), body })
}

/// Selo do tipo, nas cores de estado do tema.
pub(crate) fn kind_pill(kind: &str, cx: &App) -> Option<Div> {
    let (key, color) = match kind {
        "new" => ("guide_kind_new", theme::success_text()),
        "improvement" => ("guide_kind_improvement", cx.theme().info),
        "fix" => ("guide_kind_fix", theme::warning_text()),
        _ => return None,
    };
    Some(crate::app::vault::chip(tr(key), color, color.alpha(0.16)).rounded_full().font_weight(FontWeight::SEMIBOLD))
}

pub(crate) fn kind_icon(kind: &str) -> IconName {
    match kind { "new" => IconName::Sparkles, "improvement" => IconName::Zap, "fix" => IconName::Wrench, _ => IconName::Info }
}

// shortcut: só os ícones que os artigos usam hoje; ícone fora da lista sai como livro. Artigo com ícone novo entra aqui.
pub(super) fn article_icon(name: Option<&str>) -> IconName {
    match name.unwrap_or_default() {
        "smartphone" => IconName::Smartphone, "key-round" => IconName::KeyRound, "user" => IconName::User, "users" => IconName::Users,
        "mic" => IconName::Mic, "globe" => IconName::Globe, "eye-off" => IconName::EyeOff, "eye" => IconName::Eye,
        "message-square" => IconName::MessageSquare, "message-circle" => IconName::MessageCircle, "circle-alert" => IconName::CircleAlert,
        "triangle-alert" => IconName::TriangleAlert, "clock" => IconName::Clock, "monitor" => IconName::Monitor, "server" => IconName::Server,
        "git-branch" => IconName::GitBranch, "package" => IconName::Package, "layers" => IconName::Layers, "bell" => IconName::Bell,
        "zap" => IconName::Zap, "wrench" => IconName::Wrench, "folder" => IconName::Folder, "keyboard" => IconName::Keyboard,
        "sparkles" => IconName::Sparkles, "info" => IconName::Info, "search" => IconName::Search, "settings" => IconName::Settings,
        _ => IconName::BookOpen,
    }
}

pub(crate) fn markdown_block(id: String, text: String) -> Div {
    div().text_color(theme::body_text()).child(TextView::markdown(SharedString::from(id), text).selectable(true).scrollable(false))
}

/// Título de grupo da página, no desenho do `heading` das Configurações.
pub(crate) fn group_heading(title: String, aside: Option<String>) -> Div {
    div().mt(px(28.)).mb(px(10.)).flex().items_center().gap_2().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(title)
        .when_some(aside, |el, aside| el.child(div().text_size(px(12.)).font_weight(FontWeight::NORMAL).text_color(theme::faint()).child(aside)))
}

impl Hangar {
    /// Configurações abertas: decide o servidor das duas páginas e relê o que chegou, para o contador da navegação.
    pub(in crate::app) fn guide_settings_opened(&mut self, cx: &mut Context<Self>) {
        // Uma vez por abertura: montar o `Api` das máquinas a cada desenho seria caro.
        let api = self.this_machine_api().or_else(|| self.api.clone().filter(|_| !self.active_invite()));
        let server = api.as_ref().map(Api::identity);
        if server != self.guide.server {
            let guide = &mut self.guide;
            let kept = (guide.updates_query.take(), guide.manual_query.take(), std::mem::take(&mut guide._subscriptions));
            *guide = Guide { server, ..Guide::default() };
            (guide.updates_query, guide.manual_query, guide._subscriptions) = kept;
        }
        self.guide.api = api;
        self.load_arrived(cx);
    }

    /// Sem servidor do dono para ler, ou recusado por ele, as duas entradas não aparecem.
    pub(in crate::app) fn guide_visible(&self, page: Page) -> bool {
        !matches!(page, Page::Updates | Page::Manual) || (self.guide.api.is_some() && !self.guide.forbidden)
    }

    pub(in crate::app) fn updates_opened(&mut self, cx: &mut Context<Self>) {
        (self.guide.news_open, self.guide.list_offset) = (None, None);
        self.load_news(cx);
        self.guide_mark_seen();
    }

    pub(in crate::app) fn manual_opened(&mut self, cx: &mut Context<Self>) {
        (self.guide.article_open, self.guide.section, self.guide.list_offset) = (None, None, None);
        self.load_manual(cx);
        // O bloco "Últimas atualizações" do Manual vem da mesma lista.
        if self.guide.news.value.is_none() && !self.guide.news.loading { self.load_news(cx); }
    }

    pub(in crate::app) fn article_wide(&self, window: &Window) -> bool {
        self.guide.article_open.is_some() && window.viewport_size().width >= px(ARTICLE_WIDE)
    }

    pub(in crate::app) fn relabel_guide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (input, key) in [(self.guide.updates_query.clone(), "updates_search"), (self.guide.manual_query.clone(), "manual_search")] {
            if let Some(input) = input { input.update(cx, |input, cx| input.set_placeholder(tr(key), window, cx)); }
        }
    }

    /// O campo de busca de cada página, criado no primeiro desenho dela (precisa da janela).
    pub(super) fn guide_query(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(input) = if manual { &self.guide.manual_query } else { &self.guide.updates_query } { return input.clone(); }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr(if manual { "manual_search" } else { "updates_search" })));
        self.guide._subscriptions.push(cx.subscribe(&input, |_: &mut Hangar, _, event: &InputEvent, cx| if matches!(event, InputEvent::Change) { cx.notify(); }));
        if manual { self.guide.manual_query = Some(input.clone()); } else { self.guide.updates_query = Some(input.clone()); }
        input
    }

    /// Carregando, erro (com o motivo e tentar de novo) ou sem servidor; `None` quando há dados para desenhar.
    pub(super) fn guide_wait<T>(&self, remote: &Remote<T>, id: &'static str, retry: fn(&mut Hangar, &mut Context<Hangar>),
        cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.guide.api.is_none() { return Some(div().mt(px(18.)).child(muted(tr("settings_offline"))).into_any_element()); }
        match &remote.value {
            Some(Ok(_)) => None,
            None => Some(div().id(SharedString::from(format!("{id}-loading"))).role(Role::Status).mt(px(18.)).child(muted(tr("loading"))).into_any_element()),
            Some(Err(error)) => Some(div().mt(px(18.)).flex().flex_col().gap_3().child(alert(SharedString::from(format!("{id}-error")), error.clone()))
                .child(div().child(Button::new(SharedString::from(format!("{id}-retry"))).outline().small().label(tr("retry")).loading(remote.loading)
                    .on_click(cx.listener(move |this, _, _, cx| retry(this, cx)))))
                .into_any_element()),
        }
    }

    pub(super) fn load_news(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.guide.api.clone() else { return };
        let (seq, server) = (self.guide.news.start(), self.guide.server.clone());
        self.guide_read(async move { api.server_read(&["news"], &[], 20).await }, cx, move |this, result, _| {
            if this.guide.server != server { return; }
            let value = this.guide_result(result, "news_unavailable").and_then(parse_news);
            this.guide.news.finish(seq, value);
        });
        cx.notify();
    }

    pub(super) fn load_manual(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.guide.api.clone() else { return };
        let (seq, server) = (self.guide.manual.start(), self.guide.server.clone());
        self.guide_read(async move { api.server_read(&["manual"], &[], 20).await }, cx, move |this, result, _| {
            if this.guide.server != server { return; }
            let value = this.guide_result(result, "manual_unavailable").and_then(parse_manual);
            this.guide.manual.finish(seq, value);
        });
        cx.notify();
    }

    fn load_arrived(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(device)) = (self.guide.api.clone(), crate::update::confirm::device_id()) else { return };
        self.guide.arrived_seq += 1;
        let (seq, server) = (self.guide.arrived_seq, self.guide.server.clone());
        self.guide_read(async move { api.server_read(&["update", "arrived"], &[("device", &device)], 20).await }, cx, move |this, result, _| {
            if this.guide.server != server || this.guide.arrived_seq != seq { return; }
            match result {
                Ok(value) => match parse_arrived(value) {
                    Some((unread, through)) => {
                        (this.guide.unread, this.guide.through) = (unread, Some(through));
                        this.guide_mark_seen();
                    }
                    None => crate::log_line("guia: resposta do que chegou fora do formato"),
                },
                Err(error) if error.status == Some(403) => this.guide.forbidden = true,
                // Servidor sem a rota: sem bolinhas, as páginas continuam.
                Err(error) if matches!(error.status, Some(404 | 405)) => {}
                Err(error) => crate::log_line(&format!("guia: leitura do que chegou falhou: {}", error.detail)),
            }
        });
    }

    /// Com a página Atualizações aberta, o que chegou passa a visto no servidor; as bolinhas ficam até a próxima abertura.
    fn guide_mark_seen(&mut self) {
        if self.settings != Some(Page::Updates) || self.guide.unread.is_empty() { return; }
        let (Some(through), Some(api)) = (self.guide.through, self.guide.api.clone()) else { return };
        if self.guide.seen == Some(through) { return; }
        let Some(device) = crate::update::confirm::device_id() else { return };
        self.guide.seen = Some(through);
        crate::update::mark_seen(&self.runtime, api, device, through);
    }

    /// 403 esconde as entradas; 404/405 é servidor anterior às rotas.
    fn guide_result(&mut self, result: Result<Value, Failure>, unavailable: &str) -> Result<Value, String> {
        result.map_err(|error| match error.status {
            Some(403) => { self.guide.forbidden = true; tr("guide_forbidden") }
            Some(404 | 405) => tr("guide_server_outdated"),
            _ if error.code.as_deref() == Some(unavailable) || error.detail == unavailable => tr("guide_unavailable"),
            _ => Hangar::fetch_failure(&error),
        })
    }

    /// Pedido no runtime do tokio, sem janela (a abertura das Configurações não tem); a volta redesenha.
    fn guide_read<F>(&self, request: F, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, Result<Value, Failure>, &mut Context<Self>) + 'static)
    where F: Future<Output = Result<Value, Failure>> + Send + 'static {
        let task = self.runtime.spawn(request);
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(Failure::local(error.to_string())));
            let _ = this.update(cx, |this, cx| { then(this, result, cx); cx.notify(); });
        }).detach();
    }

    /// Abre a página da novidade. `from_list`: veio da lista de Atualizações, e o voltar devolve a rolagem dela.
    pub(super) fn show_news_page(&mut self, id: String, from_list: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings != Some(Page::Updates) { self.open_settings(Page::Updates, window, cx); }
        else if from_list && self.guide.news_open.is_none() { self.guide.list_offset = Some(self.settings_ui.scroll.offset()); }
        self.guide.news_open = Some(id);
        self.guide.started = Some(Instant::now());
        self.settings_ui.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    pub(super) fn close_news_page(&mut self, cx: &mut Context<Self>) {
        self.guide.news_open = None;
        self.settings_ui.scroll.set_offset(self.guide.list_offset.take().unwrap_or_else(|| point(px(0.), px(0.))));
        cx.notify();
    }

    pub(super) fn show_article(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings != Some(Page::Manual) { self.open_settings(Page::Manual, window, cx); }
        else if self.guide.article_open.is_none() { self.guide.list_offset = Some(self.settings_ui.scroll.offset()); }
        (self.guide.article_open, self.guide.section) = (Some(id), None);
        self.settings_ui.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    pub(super) fn close_article(&mut self, cx: &mut Context<Self>) {
        (self.guide.article_open, self.guide.section) = (None, None);
        self.settings_ui.scroll.set_offset(self.guide.list_offset.take().unwrap_or_else(|| point(px(0.), px(0.))));
        cx.notify();
    }

    /// "Mostrar no app" das duas páginas: a página sai da frente e o balão aponta a peça.
    pub(super) fn show_in_app_button(&self, id: &'static str, note: crate::tour::Note, cx: &mut Context<Self>) -> Button {
        Button::new(id).outline().small().icon(IconName::Eye).label(tr("news_show_in_app"))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.close_settings(window, cx);
                this.show_news(note.clone(), window, cx);
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    fn item(id: &str, date: &str, kind: &str, title: &str) -> NewsItem {
        serde_json::from_value(serde_json::json!({"id": id, "date": date, "title": title, "kind": kind, "platforms": "all"})).unwrap()
    }

    #[test]
    fn news_filter_search_and_days() {
        let news = vec![item("a", "2026-11-12", "new", "Pedidos na hora certa"), item("b", "2026-11-12", "fix", "Conta disponível"),
            item("c", "2026-11-10", "improvement", "Nova conversa mais direta")];
        let ids = |list: Vec<&NewsItem>| list.into_iter().map(|n| n.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(shown_news(&news, 0, "")), ["a", "b", "c"]);
        assert_eq!(ids(shown_news(&news, 3, "")), ["b"], "Correções");
        assert_eq!(ids(shown_news(&news, 0, "CONTA disponivel")), ["b"], "sem acento nem caixa");
        let days = by_day(shown_news(&news, 0, ""));
        assert_eq!(days.iter().map(|(d, l)| (*d, l.len())).collect::<Vec<_>>(), [("2026-11-12", 2), ("2026-11-10", 1)]);
    }

    #[test]
    fn only_desktop_items_and_the_row_opens_with_more_or_a_known_animation() {
        let value = serde_json::json!({"news": [
            {"id": "a", "date": "2026-11-12", "title": "t", "kind": "new", "platforms": "mobile"},
            {"id": "b", "date": "2026-11-12", "title": "t", "kind": "new", "platforms": "desktop", "animation": "scheduled-tasks"},
            {"id": "c", "date": "2026-11-12", "title": "t", "kind": "fix", "platforms": "all", "animation": "nao-existe"},
            {"id": "d", "date": "2026-11-12", "title": "t", "kind": "fix", "platforms": "all", "more": true}]});
        let news = parse_news(value).unwrap();
        assert_eq!(news.iter().map(|n| (n.id.as_str(), opens(n))).collect::<Vec<_>>(), [("b", true), ("c", false), ("d", true)]);
        let arrived = parse_arrived(serde_json::json!({"news": [{"id": "a", "platforms": "mobile"}, {"id": "b", "platforms": "all"}], "through": 9}));
        assert_eq!(arrived, Some((HashSet::from(["b".to_owned()]), 9)));
        assert_eq!(parse_arrived(serde_json::json!({"news": []})), None, "sem `through` não marca nada");
    }

    #[test]
    fn manual_drops_mobile_articles_and_searches_sections() {
        let value = serde_json::json!({"categories": [
            {"id": "comecar", "articles": [{"id": "x", "title": "Conectar", "platforms": "mobile"}]},
            {"id": "ferramentas", "articles": [{"id": "cofre", "title": "Cofre", "platforms": "all",
                "sections": [{"id": "s", "title": "Guardar", "markdown": "Toque em **Nova senha**."}]}]}]});
        let categories = parse_manual(value).unwrap();
        assert_eq!(categories.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["ferramentas"], "categoria sem artigo do desktop sai");
        assert_eq!(manual_matches(&categories, "nova senha").len(), 1);
        assert!(manual_matches(&categories, "tailscale").is_empty());
    }

    #[test]
    fn day_labels_follow_the_language_and_show_the_year_only_when_needed() {
        crate::i18n::english_in_this_thread(false);
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        assert_eq!(day_label("2026-11-12", today, false), Some(("12 de novembro".into(), "quinta".into())));
        assert_eq!(day_label("2025-11-12", today, false).unwrap().0, "12 de novembro de 2025");
        assert_eq!(day_short("2026-11-07"), "7 nov");
        assert_eq!(day_label("ontem", today, false), None);
    }
}
