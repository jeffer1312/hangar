//! Página Aparência: cada grupo num cartão com ícone próprio, as escolhas que mudam o desenho em miniatura, o ajuste
//! que depende de outro logo abaixo dele, e a prévia ao lado mostrando onde a seção mexida aparece.
use super::*;

/// Seções na ordem da página: a chave do título é também a âncora do atalho e o que a prévia acompanha.
const SECTIONS: [&str; 7] = ["settings_theme_panels_group", "settings_color", "settings_background_group", "settings_reading_group",
    "settings_text_group", "settings_conversation_group", "settings_sidebar_group"];

fn bars(color: Hsla, widths: &[f32]) -> Div {
    div().flex().flex_col().gap(px(5.)).children(widths.iter().map(|w| div().h(px(4.)).w(relative(*w)).rounded(px(2.)).bg(color)))
}

/// Janela em miniatura: barra lateral à esquerda e conversa à direita, com duas linhas de texto em cada.
fn mini_window((sidebar, main, line): (Hsla, Hsla, Hsla)) -> Div {
    // O GPUI recorta em retângulo: quem pinta o canto arredonda o próprio canto, senão cobre a moldura.
    div().size_full().p(px(8.)).flex().gap(px(6.)).bg(sidebar).rounded(px(8.))
        .child(div().w(relative(0.3)).pt(px(4.)).child(bars(line, &[0.8, 0.6])))
        .child(div().flex_1().rounded(px(6.)).p(px(8.)).bg(main).child(bars(line, &[0.8, 0.6])))
}

fn art_background(background: Background) -> Div {
    let base = div().size_full().relative().overflow_hidden().rounded(px(7.)).bg(theme::inset());
    match background {
        Background::Plain => base,
        Background::Texture => base.p(px(6.)).flex().flex_wrap().gap(px(5.))
            .children((0..48).map(|_| div().size(px(2.)).rounded_full().bg(theme::faint().alpha(0.55)))),
        Background::Light => base.child(div().absolute().top(px(-22.)).right(px(-12.)).size(px(64.)).rounded_full().bg(theme::accent().alpha(0.32))),
        Background::Image => base.flex().items_center().justify_center().child(chrome::small_icon(IconName::Image, 18., theme::faint())),
        Background::Desktop => mini_window(theme::desktop_thumbnail()),
    }
}

/// A foto de fundo das miniaturas de Leitura: duas manchas de cor fixas, o texto é que muda.
fn photo() -> Div {
    div().size_full().relative().overflow_hidden().rounded(px(7.)).bg(rgb(0x2a3b55))
        .child(div().absolute().right(px(-12.)).bottom(px(-14.)).w(px(64.)).h(px(40.)).rounded_full().bg(rgb(0x3d4f3a)))
}

fn art_reading(reading: Reading) -> Div {
    let ink = Hsla::white();
    let lines = |halo: bool| div().absolute().left(px(8.)).top(px(12.)).right(px(16.)).flex().flex_col().gap(px(5.))
        .children([1.0, 0.7].map(|w| div().w(relative(w)).when(halo, |el| el.p(px(2.)).rounded(px(3.)).bg(Hsla::black().alpha(0.55)))
            .child(div().h(px(4.)).rounded(px(2.)).bg(ink.alpha(if halo { 1. } else { 0.8 })))));
    match reading {
        Reading::None => photo().child(lines(false)),
        Reading::Text => photo().child(lines(true)),
        Reading::Sheet => photo().child(div().absolute().left(px(5.)).top(px(6.)).right(px(10.)).bottom(px(6.)).rounded(px(5.))
            .bg(Hsla::black().alpha(0.72)).p(px(6.)).child(bars(ink, &[1.0, 0.7]))),
        // Automática escolhe sozinha: o selo marca que é o app quem decide.
        Reading::Auto => photo().child(lines(false)).child(div().absolute().top(px(4.)).right(px(4.)).size(px(16.)).rounded(px(4.))
            .bg(Hsla::black().alpha(0.6)).flex().items_center().justify_center().child(chrome::small_icon(IconName::Sparkles, 11., ink))),
    }
}

fn art_tools(look: ToolLook) -> Div {
    let tone = theme::faint().alpha(0.6);
    let bar = move |w: f32| div().h(px(4.)).w(relative(w)).rounded(px(2.)).bg(tone);
    let base = div().size_full().rounded(px(7.)).bg(theme::inset()).p(px(8.));
    match look {
        ToolLook::Classic => base.flex().flex_col().gap(px(6.)).children([0.6, 0.75, 0.45].map(|w| div().flex().items_center().gap(px(5.))
            .child(div().size(px(7.)).flex_shrink_0().rounded(px(2.)).bg(tone)).child(div().flex_1().child(bar(w))))),
        ToolLook::Chips => base.flex().flex_wrap().gap(px(5.))
            .children([40., 28., 48., 34.].map(|w| div().h(px(12.)).w(px(w)).rounded_full().bg(theme::faint().alpha(0.35)))),
        ToolLook::Tree => base.flex().flex_col().gap(px(5.)).child(bar(0.55))
            .children([0.6, 0.45, 0.7].map(|w| div().ml(px(3.)).pl(px(6.)).border_l_1().border_color(tone).child(bar(w)))),
        ToolLook::Terminal => base.flex().flex_col().gap(px(5.))
            .child(div().flex().items_center().gap(px(5.)).child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(theme::success())).child(bar(0.5)))
            .children([theme::removed(), theme::success()].map(|c| div().ml(px(11.)).h(px(5.)).w(relative(0.7)).rounded(px(2.)).bg(c.opacity(0.45)))),
    }
}

fn art_navigation(navigation: Navigation) -> Div {
    let strip = theme::faint().alpha(0.3);
    let base = div().size_full().rounded(px(7.)).overflow_hidden().bg(theme::inset()).flex();
    match navigation {
        Navigation::Sidebar => base.child(div().w(px(14.)).h_full().bg(strip).flex().flex_col().items_center().gap(px(5.)).pt(px(7.))
            .children((0..3).map(|_| div().size(px(5.)).rounded(px(2.)).bg(theme::faint())))),
        Navigation::Tabs | Navigation::BottomTabs => {
            let bottom = navigation == Navigation::BottomTabs;
            base.flex_col().when(bottom, |el| el.justify_end()).child(div().h(px(14.)).w_full().bg(strip).flex()
                .map(|el| if bottom { el.items_start() } else { el.items_end() }).gap(px(3.)).px(px(5.))
                .children([true, false, false].map(|on| div().w(px(22.)).h(px(9.))
                    .map(|el| if bottom { el.rounded_b(px(3.)) } else { el.rounded_t(px(3.)) })
                    .bg(theme::faint().alpha(if on { 1. } else { 0.4 })))))
        }
        Navigation::Conversations => base.child(div().w(relative(0.38)).h_full().bg(strip).p(px(6.)).child(bars(theme::faint(), &[1.0, 0.8, 0.65, 0.75]))),
    }
}

/// Escolha em miniaturas: mesma regra do `segments` (`available`, `locked`, `off_note`), com o desenho de cada opção.
#[allow(clippy::too_many_arguments)]
fn tiles(id: &'static str, labels: &[String], arts: Vec<Div>, selected: usize, available: usize, locked: bool, off_note: String, live: bool,
    pick: impl Fn(&mut Hangar, usize, &mut Window, &mut Context<Hangar>) + Clone + 'static, cx: &mut Context<Hangar>) -> Div {
    let art_height = px(if live { 40. } else { 56. });
    div().px(px(if live { 12. } else { 16. })).pb(px(14.)).flex().gap(px(if live { 6. } else { 10. })).when(locked, |el| el.opacity(0.6))
        .children(arts.into_iter().zip(labels.iter()).enumerate().map(|(n, (art, label))| {
            let on = n == selected && (available > 0 || locked);
            let enabled = n < available;
            let pick = pick.clone();
            // A moldura mora fora do botão: o hover do botão redefine a cor da própria borda.
            div().flex_1().min_w_0().rounded(px(11.)).border_2().border_color(if on { theme::accent() } else { theme::border_strong() })
                .when(!enabled && !on, |el| el.opacity(0.5))
                .child(Button::new(SharedString::from(format!("{id}-{n}")))
                    .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
                        .foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
                    .w_full().h_auto().p(px(5.)).rounded(px(9.)).disabled(!enabled).accessibility_label(label.clone())
                    .when(!enabled && available > 0, |el| el.tooltip(off_note.clone()))
                    .child(div().w_full().flex().flex_col().gap(px(6.))
                        .child(div().w_full().h(art_height).child(art))
                        .child(div().w_full().flex().items_center().justify_center().gap(px(4.)).text_size(px(12.5))
                            .text_color(if on { theme::text() } else { theme::muted() }).when(on, |el| el.font_weight(FontWeight::MEDIUM))
                            .when(on && !live, |el| el.child(chrome::small_icon(IconName::Check, 13., theme::accent())))
                            .child(div().min_w_0().truncate().child(label.clone()))))
                    .on_click(cx.listener(move |this, _, window, cx| if enabled && !on { pick(this, n, window, cx) })))
        }))
}

impl Hangar {
    /// Linha sem ícone próprio: o ícone é da seção. `nested` é o ajuste que depende do de cima e fica recuado sob ele.
    fn line_with(&self, title: &'static str, description: Option<Div>, enabled: bool, control: AnyElement, nested: bool) -> Div {
        let live = self.settings_ui.live;
        let text = div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().font_weight(FontWeight::MEDIUM).text_size(px(if nested { 13.5 } else { 14. }))
                .text_color(if enabled { theme::text() } else { theme::muted() }).child(tr(title)))
            .when_some(description, |el, d| el.child(d.text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()));
        let head = div().flex_1().min_w_0().flex().items_start().gap(px(10.))
            .when(nested, |el| el.pl(px(8.)).child(div().pt(px(2.)).flex_shrink_0().child(chrome::small_icon(IconName::CornerDownRight, 15., theme::faint()))))
            .child(text);
        // O ajuste dependente continua a linha de cima: sem divisória entre os dois.
        let row = div().flex().when(!nested, |el| el.mt(px(-1.)).border_t_1().border_color(theme::border()))
            .map(|el| if live { el.flex_col().gap(px(10.)).px_3().py(px(12.)) }
                else { el.items_center().gap(px(14.)).px_4().py(px(if nested { 10. } else { 14. })) })
            .child(head)
            .child(div().flex_shrink_0().when(live, |el| el.flex().pl(px(if nested { 33. } else { 0. }))).child(control));
        self.mark(row, title)
    }

    fn line(&self, title: &'static str, description: Option<String>, enabled: bool, control: AnyElement, nested: bool) -> Div {
        self.line_with(title, description.map(|d| div().child(d)), enabled, control, nested)
    }

    fn slider_line(&self, title: &'static str, description: Option<String>, knob: Knob, enabled: bool, a: &Appearance, nested: bool,
        cx: &mut Context<Self>) -> Div {
        let state = self.settings_ui.slider(knob);
        let control = div().w(px(230.)).flex().items_center()
            .child(self.slider_edge(knob, false, enabled, cx))
            .child(Slider::new(state).aria_label(tr(title)).flex_1().bg(theme::accent()).text_color(theme::text()).disabled(!enabled))
            .child(self.slider_edge(knob, true, enabled, cx))
            .child(div().w(px(52.)).flex_shrink_0().text_right().text_size(px(12.5)).text_color(theme::muted()).child(format!("{}%", knob.read(a))));
        self.line(title, description, enabled, control.into_any_element(), nested)
    }

    /// Rótulo de uma escolha em miniaturas; leva a chave da busca, que destaca e rola até ele.
    fn sublabel(&self, key: &'static str) -> Div {
        let live = self.settings_ui.live;
        self.mark(div().px(px(if live { 12. } else { 16. })).pt(px(4.)).pb(px(8.)).text_size(px(12.5)).font_weight(FontWeight::MEDIUM)
            .text_color(theme::muted()).child(tr(key)), key)
    }

    /// Cartão da seção. Qualquer clique dentro dele troca a prévia para esta seção.
    fn section(&self, key: &'static str, icon: IconName, subtitle: Option<String>, extra: Option<AnyElement>, body: Div,
        cx: &mut Context<Self>) -> Div {
        let live = self.settings_ui.live;
        let head = section_head(icon, tr(key), subtitle, extra, px(if live { 12. } else { 16. }));
        settings_box().mt(px(16.)).child(self.mark(head, key)).child(body)
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| if this.settings_ui.preview != Some(key) {
                this.settings_ui.preview = Some(key);
                cx.notify();
            }))
    }

    /// Atalhos para as seções: a página é longa e a busca só acha uma linha por vez. Mora fora da rolagem, sempre à vista.
    pub(in crate::app) fn section_tabs(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let current = self.settings_ui.jump;
        div().id("appearance-sections").max_w_full().flex().gap(px(4.)).p(px(3.)).rounded(px(10.)).border_1()
            .border_color(theme::border()).bg(theme::inset()).overflow_x_scroll()
            .children(SECTIONS.map(|key| {
                let on = current == Some(key);
                Button::new(SharedString::from(format!("appearance-jump-{key}")))
                    .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
                        .foreground(if on { theme::accent_text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
                    .small().h(px(28.)).px(px(11.)).rounded(px(7.)).flex_shrink_0().label(tr(key))
                    .when(on, |el| el.bg(theme::accent_dim()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_ui.hit = None;
                        this.settings_ui.jump = Some(key);
                        this.settings_ui.preview = Some(key);
                        this.settings_ui.reveal.set(true);
                        cx.notify();
                    }))
            }))
    }

    fn style_button(&self, cx: &mut Context<Self>) -> Button {
        let a = appearance::get();
        let compact = a == a.compact_style();
        Button::new("appearance-style-compact").outline().small().label(tr("settings_style_compact"))
            .selected(compact).when(compact, |el| el.icon(IconName::Check))
            .on_click(cx.listener(|this, _, window, cx| {
                this.apply_appearance(appearance::get().compact_style(), true, cx);
                this.sync_sliders(window, cx);
            }))
    }

    fn conversation_sample(&self) -> Div {
        div().p(px(14.)).flex().flex_col().gap(px(10.)).rounded(px(12.)).border_1().border_color(theme::border()).bg(theme::inset())
            .child(div().flex().justify_end().child(conversation_text(div(), true).px(px(12.)).py(px(8.)).rounded(px(16.)).bg(theme::user_bubble())
                .child(tr("settings_preview_question"))))
            .child(conversation_text(div(), false).child(tr("settings_preview_answer")))
            .child(div().font_family(theme::MONO).text_size(px(12.5)).child("npm run check"))
    }

    /// Chamadas de ferramenta no estilo escolhido, a lista de tarefas e a tabela: onde a seção Conversa aparece.
    fn tools_sample(&self, a: &Appearance) -> Div {
        let calls = [(IconName::FileText, tr("settings_sample_read")), (IconName::SquareTerminal, tr("settings_sample_ran")),
            (IconName::Pencil, tr("settings_sample_edited"))];
        let eyebrow = |key: &str| div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::faint()).child(tr(key));
        let row = |(icon, label): (IconName, String)| div().flex().items_center().gap(px(8.)).text_size(px(13.)).text_color(theme::muted())
            .child(chrome::small_icon(icon, 14., theme::faint())).child(label);
        let tools = match a.tool_look {
            ToolLook::Classic => div().flex().flex_col().gap(px(7.)).children(calls.map(row)),
            ToolLook::Chips => div().flex().flex_wrap().gap(px(6.)).children(calls.map(|(icon, label)| div().h(px(24.)).px(px(9.)).flex().items_center()
                .gap(px(6.)).rounded_full().border_1().border_color(theme::border_strong()).bg(theme::inset()).text_size(px(12.5)).text_color(theme::muted())
                .child(chrome::small_icon(icon, 12., theme::faint())).child(label))),
            ToolLook::Tree => div().flex().flex_col().gap(px(7.))
                .child(div().flex().items_center().gap(px(6.)).text_size(px(13.))
                    .child(chrome::small_icon(IconName::ChevronDown, 14., theme::faint())).child(tr("settings_sample_calls")))
                .child(div().ml(px(6.)).pl(px(12.)).border_l_1().border_color(theme::border_strong()).flex().flex_col().gap(px(7.)).children(calls.map(row))),
            ToolLook::Terminal => div().flex().flex_col().gap(px(7.)).children(calls.map(|(_, label)| div().flex().items_center().gap(px(8.))
                .font_family(theme::MONO).text_size(px(12.5)).text_color(theme::muted())
                .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(theme::success())).child(label))),
        };
        let table = div().flex().gap(px(10.)).items_end()
            .child(div().flex_1().flex().flex_col().gap(px(5.)).children((0..3).map(|_| div().flex().gap(px(5.))
                .child(div().h(px(6.)).w(relative(0.45)).rounded(px(2.)).bg(theme::faint().alpha(0.5)))
                .child(div().h(px(6.)).w(relative(0.3)).rounded(px(2.)).bg(theme::faint().alpha(0.3))))))
            .when(a.table_chart, |el| el.child(div().h(px(34.)).flex().items_end().gap(px(4.))
                .children([18., 30., 12., 24.].map(|h| div().w(px(8.)).h(px(h)).rounded_t(px(2.)).bg(theme::accent().alpha(0.7))))));
        div().flex().flex_col().gap(px(14.))
            .child(div().flex().flex_col().gap(px(8.)).child(eyebrow("settings_tool_calls")).child(tools))
            .when(a.task_list, |el| el.child(div().flex().flex_col().gap(px(6.)).child(eyebrow("settings_task_list"))
                .child(div().text_size(px(13.)).text_color(theme::muted()).child(tr("settings_sample_tasks")))
                .child(div().h(px(4.)).w_full().rounded(px(2.)).bg(theme::faint().alpha(0.3))
                    .child(div().h_full().w(relative(0.4)).rounded(px(2.)).bg(theme::accent())))))
            .child(div().flex().flex_col().gap(px(8.)).child(eyebrow("settings_table_chart")).child(table))
    }

    /// Coluna ao lado da página larga: a prévia acompanha a seção que se está mexendo, e o Estilo fica à mão.
    pub(in crate::app) fn render_appearance_aside(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let a = appearance::get();
        let compact = a == a.compact_style();
        let section = self.settings_ui.preview.unwrap_or(SECTIONS[0]);
        let window_colors = if a.theme == ThemeMode::Desktop { theme::desktop_thumbnail() } else { theme::thumbnail(a.palette, theme::is_dark()) };
        let sample = match section {
            "settings_conversation_group" => self.tools_sample(&a),
            "settings_reading_group" => div().flex().flex_col().gap(px(12.)).child(div().h(px(120.)).child(art_reading(a.reading))).child(self.conversation_sample()),
            "settings_sidebar_group" => div().h(px(140.)).child(art_navigation(a.navigation)),
            "settings_text_group" => self.conversation_sample(),
            "settings_background_group" => div().flex().flex_col().gap(px(12.)).child(div().h(px(120.)).child(art_background(a.background))).child(self.conversation_sample()),
            _ => div().flex().flex_col().gap(px(12.)).child(div().h(px(120.)).child(mini_window(window_colors))).child(self.conversation_sample()),
        };
        let eyebrow = div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::faint()).child(tr("settings_preview"));
        div().flex().flex_col().gap(px(16.))
            .child(settings_box().p(px(16.)).flex().flex_col().gap(px(12.))
                .child(div().flex().items_center().justify_between().gap_2().child(eyebrow)
                    .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(tr(section))))
                .child(sample))
            // A coluna tem rolagem própria e fica sempre à vista: a busca só destaca, sem rolar a página ao lado.
            .child(settings_box().p(px(16.)).flex().flex_col().gap(px(10.)).when(self.search_hit("settings_style"), |el| el.bg(theme::accent_dim()))
                .child(div().flex().items_center().gap(px(10.))
                    .child(div().size(px(30.)).flex_shrink_0().rounded(px(8.)).bg(theme::accent_dim()).flex().items_center().justify_center()
                        .child(chrome::small_icon(IconName::Sparkles, 16., theme::accent_text())))
                    .child(div().flex_1().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(tr("settings_style")))
                    .child(self.style_button(cx)))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
                    .child(tr(if compact { "settings_style_applied" } else { "settings_style_hint" }))))
            .into_any_element()
    }

    /// `wide`: a página tem a coluna ao lado com a prévia e o Estilo; sem ela (janela estreita ou ao vivo), ficam no corpo.
    pub(in crate::app) fn render_appearance(&mut self, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let a = appearance::get();
        let compact = a == a.compact_style();
        let floating = a.panels == Panels::Floating;
        let live = self.settings_ui.live;
        let inset = px(if live { 12. } else { 16. });
        // Na caixa ao vivo as miniaturas encolhem para caber quatro temas em 360px.
        let thumb = px(if live { 60. } else { 84. });

        let themes = [ThemeMode::Auto, ThemeMode::Light, ThemeMode::Dark, ThemeMode::Desktop];
        let theme_cards = div().px(inset).flex().gap(px(if live { 6. } else { 12. })).children(themes.map(|mode| {
            let key = match mode { ThemeMode::Auto => "auto", ThemeMode::Light => "light", ThemeMode::Dark => "dark", ThemeMode::Desktop => "desktop" };
            let selected = a.theme == mode;
            let art = match mode {
                // Automático é metade claro, metade escuro: é o sistema que decide.
                ThemeMode::Auto => div().size_full().flex()
                    .child(div().w(relative(0.5)).h_full().child(mini_window(theme::thumbnail(a.palette, false)).rounded_tr(px(0.)).rounded_br(px(0.))))
                    .child(div().flex_1().h_full().child(mini_window(theme::thumbnail(a.palette, true)).rounded_tl(px(0.)).rounded_bl(px(0.)))),
                ThemeMode::Light => div().size_full().child(mini_window(theme::thumbnail(a.palette, false))),
                ThemeMode::Dark => div().size_full().child(mini_window(theme::thumbnail(a.palette, true))),
                ThemeMode::Desktop => div().size_full().child(mini_window(theme::desktop_thumbnail())),
            };
            div().flex_1().min_w_0().flex().flex_col().items_center().gap_2()
                .child(div().w_full().rounded(px(10.)).border_2().border_color(if selected { theme::accent() } else { theme::border_strong() })
                    .child(Button::new(SharedString::from(format!("theme-{key}")))
                        .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
                        .w_full().h(thumb).p(px(0.)).rounded(px(8.)).overflow_hidden()
                        .accessibility_label(tr(&format!("settings_theme_{key}")))
                        .child(art)
                        .on_click(cx.listener(move |this, _, window, cx| this.set_theme(mode, window, cx)))))
                .child(div().flex().items_center().gap(px(4.)).text_size(px(13.)).text_color(if selected { theme::text() } else { theme::muted() })
                    .when(selected, |el| el.font_weight(FontWeight::MEDIUM))
                    .when(selected && !live, |el| el.child(chrome::small_icon(IconName::Check, 13., theme::accent())))
                    .child(tr(&format!("settings_theme_{key}"))))
        }));
        // Desktop escolhido e sem paleta: diz por quê e com o que está desenhando.
        let desktop_fallback = (a.theme == ThemeMode::Desktop && !theme::desktop_painting())
            .then(|| tr("settings_desktop_fallback").replace("{reason}", &self.desktop_note.clone()
                .unwrap_or_else(|| tr(if self.api.is_none() { "settings_desktop_offline" } else { "settings_desktop_loading" }))));

        let panel_card = |value: Panels, cx: &mut Context<Self>| {
            let selected = a.panels == value;
            let key = if value == Panels::Attached { "attached" } else { "floating" };
            let loose = value == Panels::Floating;
            let (backdrop, side_bg, side_line) = theme::panels_thumbnail(loose);
            let side = || div().h_full().w(relative(if loose { 0.27 } else { 0.28 })).bg(side_bg)
                .map(|el| if loose { el.rounded(px(7.)).border_1().border_color(side_line) } else { el })
                .when(!loose, |el| el.border_color(side_line));
            let mini = div().h(px(if live { 52. } else { 64. })).w(px(if live { 84. } else { 112. })).flex_shrink_0().flex().justify_between()
                .rounded(px(8.)).overflow_hidden().bg(backdrop).when(loose, |el| el.p(px(6.)))
                .child(side().when(!loose, |el| el.border_r_1().rounded_l(px(8.)))).child(side().when(!loose, |el| el.border_l_1().rounded_r(px(8.))));
            // A moldura mora fora do botão: o hover do botão redefine a cor da própria borda.
            div().flex_1().min_w_0().rounded(px(10.)).border_2().border_color(if selected { theme::accent() } else { theme::border_strong() })
                .child(Button::new(SharedString::from(format!("panels-{key}")))
                .custom(ButtonCustomVariant::new(cx).color(if selected { theme::accent_dim() } else { transparent_black() })
                    .foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
                .w_full().h_auto().p(px(10.)).rounded(px(8.))
                .accessibility_label(tr(&format!("settings_panels_{key}")))
                .child(div().w_full().flex().items_center().gap(px(12.))
                    .child(mini)
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                        .child(div().flex().items_center().gap(px(5.)).text_size(px(13.5)).font_weight(FontWeight::MEDIUM)
                            .when(selected, |el| el.child(chrome::small_icon(IconName::Check, 13., theme::accent())))
                            .child(tr(&format!("settings_panels_{key}"))))
                        .when(!live, |el| el.child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
                            .child(tr(&format!("settings_panels_{key}_desc")))))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut next = appearance::get();
                    next.panels = value;
                    this.apply_appearance(next, true, cx);
                })))
        };
        let panel_cards = div().px(inset).flex().gap(px(if live { 6. } else { 12. }))
            .map(|el| if live { el.flex_col() } else { el })
            .child(panel_card(Panels::Attached, cx)).child(panel_card(Panels::Floating, cx));
        let theme_body = div().pb(px(16.))
            .child(self.sublabel("settings_theme")).child(theme_cards)
            .when_some(desktop_fallback, |el, note| el.child(div().px(inset).mt_3().text_sm().text_color(theme::warning()).child(note)))
            .child(div().mt(px(14.)).child(self.sublabel("settings_panels"))).child(panel_cards);

        // Destaque e tinta são do modo na tela; no Desktop as cores vêm do papel de parede.
        let dark = theme::is_dark();
        let from_wallpaper = theme::desktop_painting();
        let colors = *a.colors(dark);
        let swatch = |id: String, label: String, color: u32, selected: bool, empty: bool, pick: Swatch, tint: bool, cx: &mut Context<Self>| {
            // No Desktop a cor vem do papel de parede: amostra esmaecida e sem anel de escolha.
            div().rounded(px(8.)).border_2().border_color(swatch_ring(selected && !from_wallpaper)).when(from_wallpaper, |el| el.opacity(0.5))
                .child(Button::new(SharedString::from(id))
                .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
                .size(px(24.)).p(px(1.)).rounded(px(6.)).accessibility_label(label).disabled(from_wallpaper)
                .child(div().size(px(20.)).rounded(px(6.)).bg(rgb(color)).when(empty, |el| el.border_1().border_color(theme::border_strong())))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut next = appearance::get();
                    let mode = next.colors_mut(theme::is_dark());
                    if tint { mode.tint = pick } else { mode.accent = pick }
                    this.apply_appearance(next, true, cx);
                })))
        };
        // O kit não desliga o seletor: no Desktop ele sai e fica só o ícone, esmaecido como as amostras.
        let picker = |state: &Entity<ColorPickerState>, selected: bool| {
            div().rounded(px(8.)).border_2().border_color(swatch_ring(selected && !from_wallpaper)).p(px(1.))
                .map(|el| if from_wallpaper {
                    el.opacity(0.5).child(div().size(px(24.)).flex().items_center().justify_center()
                        .child(chrome::small_icon(IconName::Palette, 16., theme::muted())))
                } else {
                    // Com cor livre escolhida o gatilho mostra a própria cor; sem ela, o ícone de paleta.
                    el.child(ColorPicker::new(state).small().accessibility_label(tr("settings_custom_color"))
                        .when(!selected, |picker| picker.icon(IconName::Palette)))
                })
        };
        let accents = div().flex().flex_wrap().items_center().gap(px(6.))
            .children(theme::accent_swatches(dark).into_iter().enumerate().map(|(n, color)| swatch(format!("accent-{n}"),
                tr("settings_accent_n").replace("{n}", &(n + 1).to_string()), color, colors.accent == Swatch::Preset(n), false, Swatch::Preset(n), false, cx)))
            .child(picker(&self.settings_ui.accent_picker, matches!(colors.accent, Swatch::Custom(_))));
        let tints = div().flex().flex_wrap().items_center().gap(px(6.))
            .children(theme::tint_swatches(dark).into_iter().enumerate().map(|(n, color)| swatch(format!("tint-{n}"),
                tr(if n == 0 { "settings_tint_none" } else { "settings_tint_n" }).replace("{n}", &n.to_string()), color,
                colors.tint == Swatch::Preset(n), n == 0, Swatch::Preset(n), true, cx)))
            .child(picker(&self.settings_ui.tint_picker, matches!(colors.tint, Swatch::Custom(_))));
        let no_tint = colors.tint == Swatch::Preset(0);
        // "Vale para o modo escuro. Copiar do claro": copia destaque, tinta e força do outro modo. No cabeçalho da seção,
        // porque vale para a seção inteira; na caixa ao vivo o cabeçalho não tem largura e ela desce para o corpo.
        let mode_note = (!from_wallpaper).then(|| div().flex().flex_wrap().items_center().gap_1().text_size(px(12.5)).text_color(theme::muted())
            .child(tr(if dark { "settings_colors_for_dark" } else { "settings_colors_for_light" }))
            .child(Button::new("copy-other-mode").outline().xsmall().text_color(theme::text())
                .label(tr(if dark { "settings_copy_from_light" } else { "settings_copy_from_dark" }))
                .on_click(cx.listener(|this, _, window, cx| {
                    let mut next = appearance::get();
                    let dark = theme::is_dark();
                    *next.colors_mut(dark) = *next.colors(!dark);
                    this.apply_appearance(next, true, cx);
                    this.sync_sliders(window, cx);
                }))));
        let (mode_head, mode_body) = if live { (None, mode_note) } else { (mode_note.map(IntoElement::into_any_element), None) };
        let palette = segmented("palette", &[tr("settings_palette_neutral"), tr("settings_palette_classic")],
            if a.palette == Palette::Neutral { 0 } else { 1 }, !from_wallpaper,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.palette = if index == 0 { Palette::Neutral } else { Palette::Classic }; this.apply_appearance(next, true, cx); }, cx);
        let text_color = segmented("text-color", &[tr("settings_text_color_desktop"), tr("settings_text_color_app")],
            if a.desktop_text == DesktopText::App { 1 } else { 0 }, a.theme == ThemeMode::Desktop,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.desktop_text = if index == 1 { DesktopText::App } else { DesktopText::Desktop }; this.apply_appearance(next, true, cx); }, cx);
        // No Desktop a explicação vale para paleta, destaque e tinta: um aviso só, em vez de repetir em cada linha.
        let color_body = div().pb(px(4.))
            .when(from_wallpaper, |el| el.child(div().mx(inset).mb(px(12.)).px(px(12.)).py(px(10.)).rounded(px(10.)).bg(theme::accent_dim())
                .flex().items_start().gap(px(10.)).text_size(px(13.)).text_color(theme::text())
                .child(div().pt(px(1.)).flex_shrink_0().child(chrome::small_icon(IconName::Info, 15., theme::accent_text())))
                .child(div().flex_1().min_w_0().whitespace_normal().child(tr("settings_color_desktop_note")))))
            .when_some(mode_body, |el, note| el.child(note.px(inset).pb(px(10.))))
            .child(self.line("settings_palette", None, !from_wallpaper, palette, false))
            .child(self.line("settings_accent", None, !from_wallpaper, accents.into_any_element(), false))
            .child(self.line("settings_tint", (!from_wallpaper).then(|| tr("settings_tint_desc")), !from_wallpaper, tints.into_any_element(), false))
            // Sem tinta a força não muda nada; a linha fica visível e desligada.
            .child(self.slider_line("settings_tint_strength", no_tint.then(|| tr("settings_tint_strength_off")),
                Knob::TintStrength, !no_tint && !from_wallpaper, &a, true, cx))
            .child(self.line("settings_text_color", Some(tr("settings_text_color_desc")), a.theme == ThemeMode::Desktop, text_color, false));

        const BACKGROUNDS: [Background; 5] = [Background::Plain, Background::Texture, Background::Light, Background::Image, Background::Desktop];
        // Escolha, cópia ou remoção da imagem em andamento: o grupo fica travado, mostrando o que está escolhido.
        let busy = self.backdrop_busy;
        let background = tiles("background", &[tr("settings_bg_plain"), tr("settings_bg_texture"), tr("settings_bg_light"), tr("settings_bg_image"), tr("settings_bg_desktop")],
            Vec::from(BACKGROUNDS.map(art_background)),
            BACKGROUNDS.iter().position(|b| *b == a.background).unwrap_or(0), if busy.is_some() { 0 } else { BACKGROUNDS.len() }, busy.is_some(),
            tr("settings_next_version"), live,
            |this: &mut Hangar, index, window: &mut Window, cx| {
                if this.backdrop_busy.is_some() { return; }
                let choice = BACKGROUNDS[index];
                // Imagem sem cópia guardada começa pela escolha do arquivo; o fundo só muda quando ela der certo.
                if choice == Background::Image && !appearance::image_path().is_some_and(|p| p.is_file()) { return this.pick_backdrop(cx); }
                let mut next = appearance::get();
                next.background = choice;
                this.apply_appearance(next, true, cx);
                this.refresh_backdrop(window, cx);
            }, cx);
        let image_actions = div().flex().gap_2()
            .child(Button::new("background-image-pick").outline().small().label(tr("settings_image_pick")).disabled(busy.is_some())
                .on_click(cx.listener(|this, _, _, cx| this.pick_backdrop(cx))))
            .child(Button::new("background-image-remove").outline().small().label(tr("settings_image_remove")).disabled(busy.is_some() || a.background != Background::Image)
                .on_click(cx.listener(|this, _, _, cx| this.remove_backdrop(cx))));
        let image_name = if a.background == Background::Image {
            appearance::image_name().unwrap_or_else(|| tr("settings_image_unnamed"))
        } else { tr("settings_image_none") };
        let image_description = div().flex().items_center().gap_2().min_w_0()
            .when(a.background == Background::Image, |el| el.when_some(self.backdrop.as_ref(), |el, (_, image)| el
                .child(div().w(px(48.)).h(px(32.)).flex_shrink_0().rounded(px(4.)).overflow_hidden()
                    .child(img(image.clone()).size_full().object_fit(ObjectFit::Cover)))))
            .child(div().id("background-image-name").flex_1().min_w_0().truncate().child(image_name.clone())
                .tooltip(move |window, cx| Tooltip::new(image_name.clone()).build(window, cx)));
        let background_scope = segmented("background-scope", &[tr("settings_background_chat"), tr("settings_background_everywhere")],
            if a.background_scope == BackgroundScope::Chat { 0 } else { 1 }, busy.is_none(),
            |this: &mut Hangar, index, _: &mut Window, cx| {
                let mut next = appearance::get();
                next.background_scope = if index == 0 { BackgroundScope::Chat } else { BackgroundScope::Everywhere };
                this.apply_appearance(next, true, cx);
            }, cx);
        let desktop_background = a.background == Background::Desktop;
        let wallpaper = segmented("wallpaper", &[tr("settings_wallpaper_window"), tr("settings_wallpaper_glass")],
            if a.wallpaper == Wallpaper::Glass { 1 } else { 0 }, desktop_background && busy.is_none(),
            |this: &mut Hangar, index, window: &mut Window, cx| {
                let mut next = appearance::get();
                next.wallpaper = if index == 1 { Wallpaper::Glass } else { Wallpaper::Window };
                this.apply_appearance(next, true, cx);
                this.refresh_backdrop(window, cx);
            }, cx);
        // A Transparência só deixa ver Imagem ou Desktop; com fundo Liso, Textura ou Luz a janela é opaca.
        let see_through = a.busy_background();
        // Colados só ficam translúcidos com o fundo ocupado atrás da janela inteira; no Vidro a Solidez também dá a tinta dos menus.
        let panels_see_through = floating || a.busy_background() && a.background_scope == BackgroundScope::Everywhere
            || a.surface_material == SurfaceMaterial::Glass;
        let surface_material = segmented("surface-material", &[tr("settings_surface_glass"), tr("settings_surface_opaque")],
            if a.surface_material == SurfaceMaterial::Glass { 0 } else { 1 }, true,
            |this: &mut Hangar, index, _: &mut Window, cx| {
                let mut next = appearance::get();
                next.surface_material = if index == 0 { SurfaceMaterial::Glass } else { SurfaceMaterial::Opaque };
                this.apply_appearance(next, true, cx);
            }, cx);
        let effect_owner = cx.entity().downgrade();
        let effect_label = crate::effects::CHOICES.iter().find(|(effect, _)| *effect == a.background_effect).unwrap().1;
        let background_effect = Button::new("background-effect").outline().small()
            .label(tr(effect_label)).icon(IconName::ChevronDown).disabled(busy.is_some())
            .accessibility_label(format!("{}: {}", tr("settings_background_effect"), tr(effect_label)))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                crate::effects::CHOICES.iter().fold(sidebar::menu_style(menu), |menu, &(effect, label)| {
                    let owner = effect_owner.clone();
                    menu.item(PopupMenuItem::new(tr(label)).checked(effect == a.background_effect).on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.backdrop_busy.is_some() { return; }
                            let mut next = appearance::get();
                            if next.background != Background::Image || next.background_effect == effect { return; }
                            next.background_effect = effect;
                            this.apply_appearance(next, true, cx);
                            this.refresh_backdrop(window, cx);
                        });
                    }))
                })
            });
        let glass_label = div().mt(px(-1.)).border_t_1().border_color(theme::border()).px(inset).pt(px(16.)).pb(px(4.))
            .text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::faint()).child(tr("settings_glass_group").to_uppercase());
        let background_body = div().pb(px(4.))
            .child(self.sublabel("settings_background")).child(background)
            .when_some(busy.map(|b| b.note()), |el, note| el.child(div().px(inset).pb(px(10.)).text_size(px(12.5)).text_color(theme::muted()).child(note)))
            .child(self.line("settings_wallpaper", Some(tr(if desktop_background { "settings_wallpaper_desc" } else { "settings_wallpaper_only_desktop" })),
                desktop_background, wallpaper, true))
            .when(a.background == Background::Image, |el| el.child(self.line("settings_background_effect", None, busy.is_none(),
                background_effect.into_any_element(), true)))
            .child(self.line("settings_background_scope", None, true, background_scope, false))
            .child(self.line_with("settings_image", Some(image_description), true, image_actions.into_any_element(), false))
            .when_some(self.backdrop_note.clone(), |el, note| el.child(div().px(inset).pb(px(10.)).text_sm().text_color(theme::warning()).child(note)))
            .child(glass_label)
            .child(self.slider_line("settings_transparency",
                Some(tr(if see_through { "settings_transparency_desc" } else { "settings_transparency_off" })), Knob::Transparency, see_through, &a, false, cx))
            .child(self.line("settings_surface_material", Some(tr("settings_surface_material_desc")), true, surface_material, false))
            .child(self.slider_line("settings_solidity",
                Some(tr(if panels_see_through { "settings_solidity_desc" } else { "settings_solidity_off" })), Knob::Solidity, panels_see_through, &a, false, cx))
            // Nada a escolher aqui: a linha diz de quem é o desfoque; o botão (mouse ou teclado) abre onde ligar.
            .child(self.line("settings_blur",
                Some(if self.settings_ui.blur_hint { format!("{} {}", tr("settings_blur_desc"), tr("settings_blur_hint")) } else { tr("settings_blur_desc") }), true,
                chrome::icon_button("blur-hint", IconName::Info, tr("settings_blur_hint_toggle"), cx).selected(self.settings_ui.blur_hint)
                    .on_click(cx.listener(|this, _, _, cx| { this.settings_ui.blur_hint = !this.settings_ui.blur_hint; cx.notify(); }))
                    .into_any_element(), false));

        const READINGS: [Reading; 4] = [Reading::Auto, Reading::None, Reading::Text, Reading::Sheet];
        let reading = tiles("reading", &[tr("settings_reading_auto"), tr("settings_reading_none"), tr("settings_reading_text"), tr("settings_reading_sheet")],
            Vec::from(READINGS.map(art_reading)), READINGS.iter().position(|r| *r == a.reading).unwrap_or(0), READINGS.len(), false,
            tr("settings_next_version"), live,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.reading = READINGS[index]; this.apply_appearance(next, true, cx); }, cx);
        let sheet_on = a.reading == Reading::Sheet;
        let text_on = a.effective_reading() == Reading::Text;
        let reading_body = div().pb(px(4.))
            .child(self.mark(div().child(reading), "settings_reading"))
            .child(self.slider_line("settings_sheet_solidity", (!sheet_on).then(|| tr("settings_sheet_only")), Knob::SheetSolidity, sheet_on, &a, true, cx))
            .child(self.slider_line("settings_contrast", (!text_on).then(|| tr("settings_contrast_only")), Knob::Contrast, text_on, &a, true, cx));

        // Fonte e tamanho na mesma linha; a busca pelo tamanho ("Tamanho do código") destaca a linha da fonte.
        let type_row = |area: Area, title: &'static str, description: &'static str, size_key: &'static str| {
            let (_, font, size) = self.settings_ui.type_picks.iter().find(|(a, ..)| *a == area).expect("every area has pickers");
            let custom = self.settings_ui.custom_sizes.iter().find(|(a, _)| *a == area).map(|(_, input)| input)
                .filter(|_| area.custom_shown(self.settings_ui.custom_open.contains(&area)));
            let control = div().flex().items_center().gap_2()
                .child(Select::new(font).id(SharedString::from(format!("font-{area:?}"))).small().w(px(if live { 172. } else { 200. }))
                    .menu_width(px(260.)).search_placeholder(tr("settings_font_search")).accessibility_label(tr(title)))
                .child(Select::new(size).id(SharedString::from(format!("font-size-{area:?}"))).small().w(px(112.)).accessibility_label(tr(size_key)))
                .when_some(custom, |el, input| el.child(Input::new(input).small().w(px(56.)).aria_label(tr("settings_size_custom"))))
                .into_any_element();
            self.mark(self.line(title, Some(tr(description)), true, control, false), size_key)
        };
        let text_body = div().pb(px(4.))
            .child(type_row(Area::Text, "settings_font", "settings_font_desc", "settings_text_size"))
            .child(type_row(Area::Code, "settings_code_font", "settings_code_font_desc", "settings_code_size"))
            .child(type_row(Area::Terminal, "settings_terminal_font", "settings_terminal_font_desc", "settings_terminal_size"))
            .child(self.slider_line("settings_line_height", None, Knob::Line, true, &a, false, cx))
            .child(self.slider_line("settings_column", None, Knob::Column, true, &a, false, cx));

        const THINKING: [ThinkingTools; 3] = [ThinkingTools::None, ThinkingTools::Search, ThinkingTools::All];
        const LOOKS: [ToolLook; 4] = [ToolLook::Classic, ToolLook::Chips, ToolLook::Tree, ToolLook::Terminal];
        // Na Árvore o raciocínio já entra no grupo com todas as chamadas: a escolha do pensamento fica sem efeito.
        let thinking_on = a.tool_look != ToolLook::Tree;
        let tool_calls = tiles("tool-calls", &[tr("settings_tool_calls_classic"), tr("settings_tool_calls_chips"), tr("settings_tool_calls_tree"), tr("settings_tool_calls_terminal")],
            Vec::from(LOOKS.map(art_tools)), LOOKS.iter().position(|l| *l == a.tool_look).unwrap_or(0), LOOKS.len(), false, tr("settings_next_version"), live,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.tool_look = LOOKS[index]; this.apply_appearance(next, true, cx); }, cx);
        let conversation_body = div().pb(px(4.))
            .child(self.sublabel("settings_tool_calls")).child(tool_calls)
            .child(self.line("settings_thinking", (!thinking_on).then(|| tr("settings_thinking_tree")), thinking_on,
                segmented("thinking", &[tr("settings_thinking_none"), tr("settings_thinking_search"), tr("settings_thinking_all")],
                    THINKING.iter().position(|t| *t == a.thinking_tools).unwrap_or(1), thinking_on,
                    |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.thinking_tools = THINKING[index]; this.apply_appearance(next, true, cx); }, cx),
                true))
            .child(self.line("settings_task_list", None, true,
                segmented("task-list", &[tr("settings_task_list_hide"), tr("settings_task_list_progress")], a.task_list as usize, true,
                    |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.task_list = index == 1; this.apply_appearance(next, true, cx); }, cx),
                false))
            .child(self.line("settings_table_chart", None, true,
                segmented("table-chart", &[tr("settings_table_chart_hide"), tr("settings_table_chart_show")], a.table_chart as usize, true,
                    |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.table_chart = index == 1; this.apply_appearance(next, true, cx); }, cx),
                false))
            .child(self.line("settings_ask_highlight", Some(tr("settings_ask_highlight_desc")), true,
                segmented("ask-highlight", &[tr("settings_ask_highlight_accent"), tr("settings_ask_highlight_amber")], (a.ask_highlight == AskHighlight::Amber) as usize, true,
                    |this: &mut Hangar, index, _: &mut Window, cx| {
                        let mut next = appearance::get();
                        next.ask_highlight = if index == 1 { AskHighlight::Amber } else { AskHighlight::Accent };
                        this.apply_appearance(next, true, cx);
                    }, cx),
                false));

        const NAVIGATION: [Navigation; 4] = [Navigation::Sidebar, Navigation::Tabs, Navigation::BottomTabs, Navigation::Conversations];
        let navigation = tiles("collapsed-nav", &[tr("settings_collapsed_sidebar"), tr("settings_collapsed_tabs"), tr("settings_collapsed_bottom_tabs"),
            tr("settings_nav_conversations")],
            Vec::from(NAVIGATION.map(art_navigation)), NAVIGATION.iter().position(|n| *n == a.navigation).unwrap_or(0), NAVIGATION.len(), false,
            tr("settings_next_version"), live,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.navigation = NAVIGATION[index]; this.apply_appearance(next, true, cx); this.recents_sessions_changed(false, cx); }, cx);
        let conversations_nav = a.navigation == Navigation::Conversations;
        let height = segmented("sidebar-height", &[tr("settings_sidebar_full"), tr("settings_sidebar_content")],
            if a.sidebar_height == SidebarHeight::Content { 1 } else { 0 }, floating,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.sidebar_height = if index == 1 { SidebarHeight::Content } else { SidebarHeight::Full }; this.apply_appearance(next, true, cx); }, cx);
        let sidebar_body = div().pb(px(4.))
            .child(self.sublabel("settings_collapsed_nav")).child(navigation)
            .child(self.line("settings_sidebar_density", Some(tr("settings_sidebar_density_hint")), conversations_nav,
                segmented("sidebar-density", &[tr("settings_sidebar_normal"), tr("settings_sidebar_compact")], a.sidebar_compact as usize, conversations_nav,
                    |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.sidebar_compact = index == 1; this.apply_appearance(next, true, cx); }, cx),
                true))
            .child(self.line("settings_sidebar_height", Some(tr("settings_only_floating")), floating, height, false));

        // Os botões do topo com a borda forte do mock.
        let top_button = |id: &'static str, key: &'static str, icon: IconName| Button::new(id).outline().small().border_color(theme::border_strong())
            .icon(icon).label(tr(key));
        let reset = top_button("appearance-reset", "settings_reset", IconName::RotateCcw).tooltip(tr("settings_reset_hint"))
            .on_click(cx.listener(|this, _, window, cx| this.reset_appearance(window, cx)));
        let import = top_button("appearance-electron", "electron_import", IconName::Download).tooltip(tr("electron_import_hint"))
            .on_click(cx.listener(|this, _, window, cx| this.import_electron(true, window, cx)));
        // A âncora da rolagem até os botões do topo é a faixa deles.
        let top_key = self.settings_ui.hit.filter(|k| matches!(*k, "settings_live" | "settings_reset")).unwrap_or("");
        let top = if live { div().pt(px(12.)).flex().justify_end().child(reset) } else {
            // Título em cima e botões embaixo, como nas outras páginas: ao lado do título eles passavam por baixo da prévia.
            div().flex().flex_col().gap(px(14.))
                .child(div().flex().flex_col()
                    .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr("settings_page_appearance")))
                    .child(div().mt(px(6.)).text_color(theme::muted()).whitespace_normal().child(tr("settings_appearance_lead"))))
                .child(div().flex().flex_wrap().items_center().gap_2()
                    .child(top_button("appearance-live", "settings_live", IconName::Eye).on_click(cx.listener(|this, _, window, cx| {
                        this.settings_ui.live = true;
                        this.settings_ui.hit = None;
                        this.root_focus.focus(window, cx);
                        cx.notify();
                    })))
                    .child(import)
                    .child(reset))
        };
        // Sem a coluna ao lado, o Estilo e a prévia ficam no corpo; a caixa ao vivo não repete a prévia, a conversa está atrás dela.
        let style = (!wide).then(|| settings_box().mt(px(18.)).child(self.row(IconName::Sparkles, "settings_style",
            Some(tr(if compact { "settings_style_applied" } else { "settings_style_hint" })), true, self.style_button(cx).into_any_element())));
        div().flex().flex_col()
            .child(self.mark(top, top_key))
            .when_some(self.appearance_note.clone(), |el, note| el.child(div().mt_3().text_sm().text_color(theme::warning()).child(note)))
            .children(style)
            .when(!wide && !live, |el| el.child(self.conversation_sample().mt(px(18.))))
            .child(self.section(SECTIONS[0], IconName::Monitor, None, None, theme_body, cx))
            .child(self.section(SECTIONS[1], IconName::Palette, None, mode_head, color_body, cx))
            .child(self.section(SECTIONS[2], IconName::Image, None, None, background_body, cx))
            .child(self.section(SECTIONS[3], IconName::BookOpen, (!live).then(|| tr("settings_reading_desc")), None, reading_body, cx))
            .child(self.section(SECTIONS[4], IconName::Type, None, None, text_body, cx))
            .child(self.section(SECTIONS[5], IconName::MessageSquare, None, None, conversation_body, cx))
            .child(self.section(SECTIONS[6], IconName::PanelLeft, None, None, sidebar_body, cx))
            .child(div().mt(px(14.)).text_size(px(12.5)).text_color(theme::faint()).child(tr("settings_web_only")))
            .into_any_element()
    }
}
