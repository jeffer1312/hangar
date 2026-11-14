//! Página Atualizações: o que mudou no Hangar por dia, do mais novo para o mais antigo. A novidade com mais a mostrar
//! (texto longo ou animação) abre página própria; as outras mostram o resumo na própria linha.
use super::*;
use super::guide::{self, NewsItem};
use crate::app::vault::muted;

const FILTER_LABELS: [&str; 4] = ["updates_filter_all", "updates_filter_new", "updates_filter_improvements", "updates_filter_fixes"];

impl Hangar {
    pub(in crate::app) fn render_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(id) = self.guide.news_open.clone() { return self.render_news_page(&id, window, cx); }
        let query = self.guide_query(false, window, cx);
        let text = query.read(cx).value().to_string();
        let labels = FILTER_LABELS.map(tr);
        let bar = div().mt(px(18.)).flex().flex_wrap().items_center().gap_3()
            .child(segments("updates-filter", &labels, self.guide.filter, labels.len(), false, String::new(),
                |this, n, _, cx| { this.guide.filter = n; cx.notify(); }, cx))
            .child(div().ml_auto().w(px(240.)).max_w_full().child(Input::new(&query).small().cleanable(true).aria_label(tr("updates_search"))
                .prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))));
        let page = div().flex().flex_col().child(self.page_top("settings_page_updates", tr("updates_lead"))).child(bar);
        if let Some(wait) = self.guide_wait(&self.guide.news, "updates", Hangar::load_news, cx) { return page.child(wait).into_any_element(); }
        let news = self.guide.news.ok().cloned().unwrap_or_default();
        let shown = guide::shown_news(&news, self.guide.filter, &text);
        if shown.is_empty() {
            let empty = if news.is_empty() { "updates_empty" } else { "updates_none_match" };
            return page.child(div().mt(px(18.)).child(muted(tr(empty)))).into_any_element();
        }
        let today = chrono::Local::now().date_naive();
        let mut list = div().flex().flex_col();
        for (date, items) in guide::by_day(shown) {
            let (day, weekday) = guide::day_label(date, today, false).unwrap_or_else(|| (date.to_owned(), String::new()));
            let mut group = settings_box();
            for item in items { group = group.child(self.news_row(item, cx)); }
            list = list.child(guide::group_heading(day, Some(weekday))).child(group);
        }
        page.child(list).into_any_element()
    }

    /// Ícone, título com a bolinha da não vista e o selo; a que abre leva o "Ver ›", as outras o resumo embaixo.
    fn news_row(&self, item: &NewsItem, cx: &mut Context<Self>) -> AnyElement {
        let opens = guide::opens(item);
        let unread = self.guide.unread.contains(&item.id);
        let title = div().flex().flex_wrap().items_center().gap_x(px(8.)).gap_y(px(2.))
            .when(unread, |el| el.child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(theme::accent())))
            .child(div().min_w_0().whitespace_normal().child(item.title.clone()))
            .children(guide::kind_pill(&item.kind, cx));
        let description = if opens || item.summary.is_empty() { div() } else { div().whitespace_normal().child(item.summary.clone()) };
        if !opens {
            return setting_row(false, guide::kind_icon(&item.kind), title, description, true, div().into_any_element()).into_any_element();
        }
        let name = if unread { format!("{} · {}", item.title, tr("updates_unread")) } else { item.title.clone() };
        let (button_id, row_id) = (item.id.clone(), item.id.clone());
        let view = Button::new(SharedString::from(format!("updates-view-{}", item.id))).ghost().small().label(tr("updates_view"))
            .accessibility_label(name)
            .on_click(cx.listener(move |this, _, window, cx| this.show_news_page(button_id.clone(), true, window, cx)));
        // O botão é o caminho do teclado; a linha inteira atende o clique do mouse.
        setting_row(false, guide::kind_icon(&item.kind), title, description, true, view.into_any_element())
            .id(SharedString::from(format!("updates-row-{}", item.id))).cursor_pointer().hover(|el| el.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, window, cx| this.show_news_page(row_id.clone(), true, window, cx)))
            .into_any_element()
    }

    /// A página da novidade: data, título e selo, a animação grande, o texto inteiro e as ações.
    fn render_news_page(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let back = Button::new("updates-back").ghost().small().icon(IconName::ArrowLeft).label(Page::Updates.title())
            .on_click(cx.listener(|this, _, _, cx| this.close_news_page(cx)));
        let page = div().flex().flex_col().child(div().flex().ml(px(-8.)).child(back));
        if let Some(wait) = self.guide_wait(&self.guide.news, "updates-detail", Hangar::load_news, cx) { return page.child(wait).into_any_element(); }
        let Some(item) = self.guide.news.ok().and_then(|news| news.iter().find(|n| n.id == id)).cloned() else {
            return page.child(div().mt(px(18.)).child(muted(tr("updates_not_found")))).into_any_element();
        };
        let date = guide::day_label(&item.date, chrono::Local::now().date_naive(), true)
            .map(|(day, weekday)| format!("{day} · {weekday}")).unwrap_or_else(|| item.date.clone());
        let started = *self.guide.started.get_or_insert_with(Instant::now);
        let animation = item.animation.as_deref().and_then(crate::news::animation);
        let body = if item.text.is_empty() { item.summary.clone() } else { item.text.clone() };
        let note = guide::note(&item.id, item.anchor.as_deref(), &item.title, item.try_it.clone().unwrap_or_else(|| item.summary.clone()));
        let show = note.map(|note| self.show_in_app_button("updates-show-in-app", note, cx));
        let manual = item.manual.clone().map(|article| Button::new("updates-read-manual").outline().small().icon(IconName::BookOpen)
            .label(tr("updates_read_manual"))
            .on_click(cx.listener(move |this, _, window, cx| this.show_article(article.clone(), window, cx))));
        page.child(div().mt(px(12.)).text_size(px(13.)).text_color(theme::muted()).child(date))
            .child(div().mt(px(4.)).flex().flex_wrap().items_center().gap_2()
                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(item.title.clone()))
                .children(guide::kind_pill(&item.kind, cx)))
            // Largura do primeiro quadro: a coluna menos o respiro; depois vale a medida da própria animação.
            .when_some(animation, |el, draw| el.child(div().mt(px(16.))
                .child(crate::update::confirm::stage_slot(&item.id, px(688.), draw, started, window, cx))))
            .when(!body.is_empty(), |el| el.child(guide::markdown_block(format!("updates-text-{}", item.id), body).mt(px(16.))))
            .when_some(item.try_it.clone(), |el, text| el.child(div().mt(px(10.)).text_size(px(13.5)).text_color(theme::muted())
                .whitespace_normal().child(tr_shared("update_notes_try_it", &[("text", &text)]))))
            .when(show.is_some() || manual.is_some(), |el| el.child(div().mt(px(16.)).flex().flex_wrap().gap_2().children(show).children(manual)))
            .into_any_element()
    }
}
