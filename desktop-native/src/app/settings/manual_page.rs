//! Página Manual: artigos por tarefa, em categorias nas abas do topo, e o artigo com o índice "Nesta página" ao lado.
use super::*;
use super::guide::{self, Article, Category};
use crate::app::vault::muted;

impl Hangar {
    pub(in crate::app) fn render_manual(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(id) = self.guide.article_open.clone() { return self.render_article(&id, window, cx); }
        let query = self.guide_query(true, window, cx);
        let text = query.read(cx).value().trim().to_string();
        let page = div().flex().flex_col().child(self.page_top("settings_page_manual", tr("manual_lead")))
            .child(div().mt(px(18.)).child(Input::new(&query).small().cleanable(true).aria_label(tr("manual_search"))
                .prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))));
        if let Some(wait) = self.guide_wait(&self.guide.manual, "manual", Hangar::load_manual, cx) { return page.child(wait).into_any_element(); }
        let categories = self.guide.manual.ok().cloned().unwrap_or_default();
        let Some(current) = guide::current_category(&categories, self.guide.category.as_deref()) else {
            return page.child(div().mt(px(18.)).child(muted(tr("manual_empty")))).into_any_element();
        };
        // Com busca, o resultado vem de todas as categorias; sem ela, a da aba.
        let (heading, articles): (String, Vec<(&Category, &Article)>) = if text.is_empty() {
            (guide::category_title(&current.id), current.articles.iter().map(|article| (current, article)).collect())
        } else {
            (tr("manual_results"), guide::manual_matches(&categories, &text))
        };
        let list = if articles.is_empty() { muted(tr("manual_none_match")) } else {
            let mut group = settings_box();
            for (category, article) in articles { group = group.child(self.article_row(category, article, cx)); }
            group
        };
        page.child(guide::group_heading(heading, None)).child(list).children(self.latest_news(cx)).into_any_element()
    }

    /// As categorias, no desenho das abas da Aparência; só na lista, não no artigo.
    pub(in crate::app) fn manual_tabs(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if self.guide.article_open.is_some() { return None; }
        let categories = self.guide.manual.ok()?;
        let current = guide::current_category(categories, self.guide.category.as_deref())?.id.clone();
        let mut strip = tab_strip("manual-categories");
        for category in categories {
            let id = category.id.clone();
            strip = strip.child(strip_tab(SharedString::from(format!("manual-category-{id}")), guide::category_title(&id), id == current, cx)
                .on_click(cx.listener(move |this, _, window, cx| this.pick_category(id.clone(), window, cx))));
        }
        Some(strip)
    }

    fn pick_category(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.guide.category = Some(id);
        // A aba mostra a categoria inteira: a busca de antes sairia por cima dela.
        if let Some(input) = self.guide.manual_query.clone() { input.update(cx, |input, cx| input.set_value("", window, cx)); }
        self.settings_ui.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    fn article_row(&self, category: &Category, article: &Article, cx: &mut Context<Self>) -> AnyElement {
        let (button_id, row_id) = (article.id.clone(), article.id.clone());
        let open = Button::new(SharedString::from(format!("manual-open-{}-{}", category.id, article.id))).ghost().small()
            .icon(IconName::ChevronRight).accessibility_label(format!("{} · {}", tr("manual_open"), article.title))
            .on_click(cx.listener(move |this, _, window, cx| this.show_article(button_id.clone(), window, cx)));
        let description = if article.summary.is_empty() { div() } else { div().whitespace_normal().child(article.summary.clone()) };
        setting_row(false, guide::article_icon(article.icon.as_deref()), article.title.clone(), description, true, open.into_any_element())
            .id(SharedString::from(format!("manual-row-{}-{}", category.id, article.id))).cursor_pointer().hover(|el| el.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, window, cx| this.show_article(row_id.clone(), window, cx)))
            .into_any_element()
    }

    /// As três novidades de tipo novo mais recentes; abrem a página delas em Atualizações.
    fn latest_news(&self, cx: &mut Context<Self>) -> Option<Div> {
        let latest: Vec<_> = self.guide.news.ok()?.iter().filter(|item| item.kind == "new").take(3).cloned().collect();
        if latest.is_empty() { return None; }
        let mut group = settings_box();
        for item in latest {
            group = group.child(self.news_link(&item.id, &item.date, &item.title, "manual-latest", cx));
        }
        Some(div().child(guide::group_heading(tr("manual_latest"), None)).child(group))
    }

    /// Linha que leva à página de uma novidade: título, data curta e "›".
    fn news_link(&self, id: &str, date: &str, title: &str, prefix: &str, cx: &mut Context<Self>) -> AnyElement {
        let (button_id, row_id) = (id.to_owned(), id.to_owned());
        let view = Button::new(SharedString::from(format!("{prefix}-view-{id}"))).ghost().small()
            .label(tr("guide_date_link").replace("{date}", &guide::day_short(date))).accessibility_label(title.to_owned())
            .on_click(cx.listener(move |this, _, window, cx| this.show_news_page(button_id.clone(), false, window, cx)));
        setting_row(false, IconName::Sparkles, title.to_owned(), div(), true, view.into_any_element())
            .id(SharedString::from(format!("{prefix}-row-{id}"))).cursor_pointer().hover(|el| el.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, window, cx| this.show_news_page(row_id.clone(), false, window, cx)))
            .into_any_element()
    }

    /// O artigo: título, resumo, abertura e uma seção por bloco; "Nesta página" ao lado (ou acima, em janela estreita).
    fn render_article(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let categories = self.guide.manual.ok().cloned().unwrap_or_default();
        let found = categories.iter().find_map(|c| c.articles.iter().find(|a| a.id == id).map(|a| (c.id.clone(), a.clone())));
        let back_label = match &found {
            Some((category, _)) => format!("{} · {}", Page::Manual.title(), guide::category_title(category)),
            None => Page::Manual.title(),
        };
        let back = Button::new("manual-back").ghost().small().icon(IconName::ArrowLeft).label(back_label)
            .on_click(cx.listener(|this, _, _, cx| this.close_article(cx)));
        let page = div().flex().flex_col().child(div().flex().ml(px(-8.)).child(back));
        if let Some(wait) = self.guide_wait(&self.guide.manual, "manual-article", Hangar::load_manual, cx) { return page.child(wait).into_any_element(); }
        let Some((_, article)) = found else {
            return page.child(div().mt(px(18.)).child(muted(tr("manual_not_found")))).into_any_element();
        };
        let note = guide::note(&article.id, article.anchor.as_deref(), &article.title, article.summary.clone());
        let show = note.map(|note| self.show_in_app_button("manual-show-in-app", note, cx));
        let (scroll, reveal) = (self.settings_ui.scroll.clone(), self.settings_ui.reveal.clone());
        let target = self.guide.section.clone();
        let mut content = div().flex_1().min_w_0().flex().flex_col()
            .child(div().mt(px(12.)).text_xl().font_weight(FontWeight::SEMIBOLD).child(article.title.clone()))
            .when(!article.summary.is_empty(), |el| el.child(div().mt(px(6.)).text_color(theme::muted()).whitespace_normal().child(article.summary.clone())))
            .children(show.map(|button| div().mt(px(12.)).flex().child(button)))
            .when(!article.intro.is_empty(), |el| el.child(guide::markdown_block(format!("manual-{}-intro", article.id), article.intro.clone()).mt(px(16.))));
        for section in &article.sections {
            let here = target.as_deref() == Some(section.id.as_str());
            content = content.child(div().relative().mt(px(24.)).flex().flex_col().gap(px(8.))
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(section.title.clone()))
                .child(guide::markdown_block(format!("manual-{}-{}", article.id, section.id), section.markdown.clone()))
                .when(here, |el| el.child(reveal_here(scroll.clone(), reveal.clone()))));
        }
        // `parse_manual` já deixou só as novidades que o desktop mostra.
        let changed = article.news.clone();
        if !changed.is_empty() {
            let mut group = settings_box();
            for item in &changed { group = group.child(self.news_link(&item.id, &item.date, &item.title, "manual-changed", cx)); }
            content = content.child(guide::group_heading(tr("manual_changed"), None)).child(group);
        }
        let wide = self.article_wide(window);
        let toc = (!article.sections.is_empty()).then(|| {
            let current = target.clone().or_else(|| article.sections.first().map(|s| s.id.clone()));
            let mut toc = div().w(px(200.)).flex_shrink_0().flex().flex_col().gap(px(2.)).map(|el| if wide { el.pt(px(56.)) } else { el.mt(px(16.)) })
                .child(div().mb(px(6.)).px(px(8.)).text_size(px(11.)).text_color(theme::faint()).child(tr("manual_on_this_page").to_uppercase()));
            for section in &article.sections {
                let (on, id) = (current.as_deref() == Some(section.id.as_str()), section.id.clone());
                toc = toc.child(Button::new(SharedString::from(format!("manual-toc-{}", section.id))).ghost().small()
                    .w_full().h_auto().min_h(px(28.)).px(px(8.)).py(px(4.)).selected(on)
                    .child(div().w_full().text_size(px(13.)).text_color(if on { theme::text() } else { theme::muted() }).whitespace_normal()
                        .child(section.title.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.guide.section = Some(id.clone());
                        this.settings_ui.reveal.set(true);
                        cx.notify();
                    })));
            }
            toc
        });
        let layout = if wide { div().flex().gap(px(34.)).child(content).children(toc) } else { div().flex().flex_col().children(toc).child(content) };
        page.child(layout).into_any_element()
    }
}
