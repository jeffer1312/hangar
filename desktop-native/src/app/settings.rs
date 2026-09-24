//! Configurações do app nativo: página que ocupa a janela, como no Zeron. A barra lateral vira a
//! navegação das seções; o conteúdo fica no centro, em linhas com ícone, título e controle à direita.
//! Nesta versão só a Aparência funciona; as demais páginas dizem que chegam depois, sem fingir.
use super::*;
use crate::appearance::{self, Appearance, DesktopText, Font, Hex, Palette, Panels, SidebarHeight, Swatch, ThemeMode};
use gpui_kit::component::{color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState}, slider::{Slider, SliderEvent, SliderState}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    General, Appearance, Diary, About,
    Servers, Sync, Accounts, Orchestration, Harnesses, Voice, Windows, Notifications, Shortcuts, Attachments, Advanced,
}

impl Page {
    const DEVICE: [Page; 4] = [Page::General, Page::Appearance, Page::Diary, Page::About];
    const SERVER: [Page; 11] = [Page::Servers, Page::Sync, Page::Accounts, Page::Orchestration, Page::Harnesses, Page::Voice,
        Page::Windows, Page::Notifications, Page::Shortcuts, Page::Attachments, Page::Advanced];

    fn key(self) -> &'static str {
        match self {
            Page::General => "general", Page::Appearance => "appearance", Page::Diary => "diary", Page::About => "about",
            Page::Servers => "servers", Page::Sync => "sync", Page::Accounts => "accounts", Page::Orchestration => "orchestration",
            Page::Harnesses => "harnesses", Page::Voice => "voice", Page::Windows => "windows", Page::Notifications => "notifications",
            Page::Shortcuts => "shortcuts", Page::Attachments => "attachments", Page::Advanced => "advanced",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Page::General => IconName::Globe, Page::Appearance => IconName::Palette, Page::Diary => IconName::FileText,
            Page::About => IconName::Info, Page::Servers => IconName::Server, Page::Sync => IconName::RefreshCw,
            Page::Accounts => IconName::User, Page::Orchestration => IconName::Users, Page::Harnesses => IconName::Activity,
            Page::Voice => IconName::Mic, Page::Windows => IconName::Monitor, Page::Notifications => IconName::Bell,
            Page::Shortcuts => IconName::Keyboard, Page::Attachments => IconName::Paperclip, Page::Advanced => IconName::SlidersHorizontal,
        }
    }

    fn title(self) -> String { tr(&format!("settings_page_{}", self.key())) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Knob { TintStrength, Transparency, Solidity, Size, Line, Column }

impl Knob {
    const ALL: [Knob; 6] = [Knob::TintStrength, Knob::Transparency, Knob::Solidity, Knob::Size, Knob::Line, Knob::Column];

    // A força da tinta é do modo que está na tela (escuro ou claro), como a cor.
    fn read(self, a: &Appearance) -> u16 {
        match self { Knob::TintStrength => a.colors(theme::is_dark()).tint_strength, Knob::Transparency => a.transparency, Knob::Solidity => a.solidity,
            Knob::Size => a.text_size, Knob::Line => a.line_height, Knob::Column => a.column }
    }

    fn write(self, a: &mut Appearance, value: u16) {
        match self { Knob::TintStrength => a.colors_mut(theme::is_dark()).tint_strength = value, Knob::Transparency => a.transparency = value,
            Knob::Solidity => a.solidity = value, Knob::Size => a.text_size = value, Knob::Line => a.line_height = value, Knob::Column => a.column = value }
    }

    fn range(self) -> (f32, f32) {
        match self { Knob::TintStrength => (5., 100.), Knob::Transparency | Knob::Solidity => (0., 100.), _ => (50., 150.) }
    }
}

/// Estado vivo da página: os controles deslizantes guardam posição e arrasto entre desenhos.
pub(super) struct SettingsUi {
    sliders: Vec<(Knob, Entity<SliderState>)>,
    /// Cor livre de Destaque e de Tinta.
    accent_picker: Entity<ColorPickerState>,
    tint_picker: Entity<ColorPickerState>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
enum Custom { Accent, Tint }

fn to_hex(color: Hsla) -> u32 {
    let c = color.to_rgb();
    let channel = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    (channel(c.r) << 16) | (channel(c.g) << 8) | channel(c.b)
}

impl SettingsUi {
    pub fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let current = appearance::get();
        let mut sliders = Vec::new();
        let mut subscriptions = Vec::new();
        for knob in Knob::ALL {
            let (min, max) = knob.range();
            let state = cx.new(|_| SliderState::new().min(min).max(max).step(1.).default_value(knob.read(&current) as f32));
            subscriptions.push(cx.subscribe_in(&state, window, move |this: &mut Hangar, _, event: &SliderEvent, _, cx| {
                // Mudança aparece enquanto arrasta; o arquivo só é gravado ao soltar.
                let (value, done) = match event { SliderEvent::Change(v) => (v.start(), false), SliderEvent::Release(v) => (v.start(), true) };
                let mut next = appearance::get();
                knob.write(&mut next, value.round().max(0.) as u16);
                this.apply_appearance(next, done, cx);
            }));
            sliders.push((knob, state));
        }
        let mut picker = |which: Custom, cx: &mut Context<Hangar>| {
            let colors = *current.colors(theme::is_dark());
            let saved = match which { Custom::Accent => colors.accent, Custom::Tint => colors.tint };
            let state = cx.new(|cx| {
                let mut state = ColorPickerState::new(window, cx);
                if let Swatch::Custom(Hex(hex)) = saved { state.set_value(rgb(hex), window, cx); }
                state
            });
            subscriptions.push(cx.subscribe_in(&state, window, move |this: &mut Hangar, _, event: &ColorPickerEvent, _, cx| {
                let ColorPickerEvent::Change(Some(color)) = event else { return };
                let mut next = appearance::get();
                let mode = next.colors_mut(theme::is_dark());
                let swatch = Swatch::Custom(Hex(to_hex(*color)));
                match which { Custom::Accent => mode.accent = swatch, Custom::Tint => mode.tint = swatch }
                this.apply_appearance(next, true, cx);
            }));
            state
        };
        let accent_picker = picker(Custom::Accent, cx);
        let tint_picker = picker(Custom::Tint, cx);
        Self { sliders, accent_picker, tint_picker, _subscriptions: subscriptions }
    }

    fn slider(&self, knob: Knob) -> &Entity<SliderState> {
        &self.sliders.iter().find(|(k, _)| *k == knob).expect("every knob has a slider").1
    }
}

fn swatch_ring(selected: bool) -> Hsla { if selected { theme::text() } else { transparent_black() } }

impl Hangar {
    pub(super) fn open_settings(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = Some(page);
        // O campo de mensagem sai da tela com a página aberta; com o foco nele, o Esc não chega à raiz.
        self.root_focus.focus(window, cx);
        self.close_controls();
        self.command_panel = false;
        self.recent = None;
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            self.root_focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Aplica na hora (o tema lê a cada desenho) e grava fora da thread da janela quando `save`.
    pub(super) fn apply_appearance(&mut self, next: Appearance, save: bool, cx: &mut Context<Self>) {
        appearance::set(next);
        theme::sync_kit(None, cx);
        if save {
            let (connection, tx) = (self.connection, self.tx.clone());
            self.runtime.spawn(async move {
                let result = tokio::task::spawn_blocking(appearance::save).await
                    .map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::AppearanceSaved(result) }).await;
            });
        }
        cx.notify();
    }

    fn reset_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = appearance::get().reset_keeping_choices();
        self.apply_appearance(next, true, cx);
        self.sync_sliders(window, cx);
    }

    /// Recoloca os controles deslizantes no valor salvo: depois do reset ou quando o modo escuro/claro muda
    /// (a força da tinta é de cada modo).
    pub(super) fn sync_sliders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = appearance::get();
        for (knob, state) in self.settings_ui.sliders.clone() {
            state.update(cx, |slider, cx| slider.set_value(knob.read(&current) as f32, window, cx));
        }
        // Os seletores de cor livre mostram a do modo na tela.
        let colors = *current.colors(theme::is_dark());
        for (state, saved) in [(self.settings_ui.accent_picker.clone(), colors.accent), (self.settings_ui.tint_picker.clone(), colors.tint)] {
            state.update(cx, |picker, cx| match saved {
                Swatch::Custom(Hex(hex)) => picker.set_value(rgb(hex), window, cx),
                Swatch::Preset(_) => picker.clear_value(window, cx),
            });
        }
    }

    fn set_theme(&mut self, mode: ThemeMode, window: &mut Window, cx: &mut Context<Self>) {
        let mut next = appearance::get();
        next.theme = mode;
        self.apply_appearance(next, true, cx);
        self.refresh_desktop_palette(cx);
        self.sync_sliders(window, cx);
    }

    /// Clique no fim do trilho: o controle do kit arredonda pela posição e para um passo antes do extremo.
    fn slider_edge(&self, knob: Knob, max: bool, enabled: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let (min, top) = knob.range();
        div().id(SharedString::from(format!("slider-edge-{knob:?}-{max}"))).w(px(10.)).h(px(20.)).flex_shrink_0()
            .when(enabled, |el| el.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                let value = if max { top } else { min };
                let mut next = appearance::get();
                knob.write(&mut next, value as u16);
                this.apply_appearance(next, true, cx);
                this.sync_sliders(window, cx);
            })))
    }

    pub(super) fn render_settings(&mut self, page: Page, cx: &mut Context<Self>) -> AnyElement {
        let floating = theme::is_floating();
        let nav_item = |page_item: Page, current: Page, cx: &mut Context<Self>| {
            let on = page_item == current;
            Button::new(SharedString::from(format!("settings-nav-{}", page_item.key())))
                .custom(ButtonCustomVariant::new(cx).color(if on { theme::selected_row() } else { transparent_black() })
                    .foreground(if on { theme::text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
                .w_full().h(px(32.)).px(px(10.)).rounded(px(6.))
                // O botão centraliza o conteúdo; a linha de navegação alinha ícone e nome à esquerda.
                .child(div().w_full().flex().items_center().gap(px(10.))
                    .child(chrome::small_icon(page_item.icon(), 16., if on { theme::text() } else { theme::faint() }))
                    .child(div().text_size(px(13.5)).child(page_item.title())))
                .on_click(cx.listener(move |this, _, window, cx| this.open_settings(page_item, window, cx)))
        };
        let group = |label: String| div().px(px(10.)).pt(px(12.)).pb(px(6.)).flex().items_center().gap_2()
            .text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::faint()).child(label);
        let server = self.server_label(cx);
        let nav = div().w(px(284.)).flex_shrink_0().h_full().flex().flex_col().px(px(10.)).bg(theme::chrome())
            .map(|el| if floating { el.rounded(px(18.)).border_1().border_color(theme::border()).shadow(theme::panel_shadow()) }
                else { el.border_r_1().border_color(theme::border()) })
            .child(div().h(px(44.)).flex_shrink_0().px(px(6.)).flex().items_center().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("settings")))
            // ponytail: a busca de ajustes chega com a R3; o campo aparece desligado e diz isso.
            .child(div().id("settings-search").h(px(32.)).flex_shrink_0().mb(px(10.)).px(px(10.)).flex().items_center().gap_2()
                .rounded(px(6.)).bg(theme::inset()).border_1().border_color(theme::border()).opacity(0.6)
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(tr("settings_next_version")).build(window, cx))
                .child(chrome::small_icon(IconName::Search, 14., theme::faint()))
                .child(div().flex_1().text_size(px(13.)).text_color(theme::faint()).child(tr("settings_search")))
                .child(chrome::kbd("Ctrl F")))
            .child(div().id("settings-nav").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(2.))
                .child(group(tr("settings_group_device")))
                .children(Page::DEVICE.map(|p| nav_item(p, page, cx)))
                .child(group(tr("settings_group_server"))
                    .child(div().ml_auto().max_w(px(140.)).truncate().text_size(px(12.5)).text_color(theme::text()).child(server)))
                .children(Page::SERVER.map(|p| nav_item(p, page, cx))))
            .child(div().h(px(48.)).flex_shrink_0().mx(px(-10.)).px(px(10.)).border_t_1().border_color(theme::border()).flex().items_center()
                .child(Button::new("settings-back").ghost().w_full().h(px(32.))
                    .child(div().w_full().flex().items_center().gap_2()
                        .child(chrome::small_icon(IconName::ArrowLeft, 16., theme::muted()))
                        .child(div().flex_1().text_sm().text_color(theme::muted()).child(tr("settings_back")))
                        .child(chrome::kbd("Esc")))
                    .on_click(cx.listener(|this, _, window, cx| this.close_settings(window, cx)))));
        let body = if page == Page::Appearance { self.render_appearance(cx) } else { self.render_page_soon(page, cx) };
        let content = div().id("settings-content").flex_1().min_w_0().h_full().overflow_y_scroll()
            .child(div().w_full().flex().justify_center().child(div().w(px(720.)).max_w_full().px_4().pt(px(44.)).pb(px(40.)).child(body)));
        div().size_full().flex().when(floating, |el| el.p(px(10.)).gap(px(10.))).child(nav).child(content).into_any_element()
    }

    fn render_page_soon(&self, page: Page, cx: &mut Context<Self>) -> AnyElement {
        div().flex().flex_col()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(page.title()))
            .child(div().mt(px(6.)).text_color(theme::muted()).child(tr("settings_soon")))
            // A conexão de hoje continua a um clique enquanto a página de servidores não chega.
            .when(page == Page::Servers, |el| el.child(div().mt_4().flex().child(Button::new("settings-connection").outline().small()
                .icon(chrome::small_icon(IconName::Plug, 14., theme::muted())).label(tr("settings_connection"))
                .on_click(cx.listener(|this, _, window, cx| this.open_connection(window, cx))))))
            .into_any_element()
    }

    fn render_appearance(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let a = appearance::get();
        let floating = a.panels == Panels::Floating;
        let heading = |text: String| div().mt(px(28.)).mb(px(10.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(text);
        let next_version = tr("settings_next_version");

        let preview = div().mt(px(18.)).p(px(14.)).flex().flex_col().gap(px(10.)).rounded(px(14.)).border_1().border_color(theme::border()).bg(theme::inset())
            .font_family(if a.font == Font::Mono { theme::MONO } else { theme::SANS })
            .text_size(px(14. * a.text_size as f32 / 100.)).line_height(relative(1.45 * a.line_height as f32 / 100.))
            .child(div().flex().justify_end().child(div().px(px(12.)).py(px(8.)).rounded(px(16.)).bg(theme::user_bubble()).child(tr("settings_preview_question"))))
            .child(div().child(tr("settings_preview_answer")))
            .child(div().font_family(theme::MONO).text_size(px(12.5)).child("npm run check"));

        // Miniatura: barra lateral à esquerda e conversa à direita, com duas linhas de texto em cada.
        let mini_window = |(sidebar, main, line): (Hsla, Hsla, Hsla)| {
            let bars = |color: Hsla| div().flex().flex_col().gap(px(5.)).pt(px(4.))
                .child(div().h(px(4.)).w(relative(0.8)).rounded(px(2.)).bg(color)).child(div().h(px(4.)).w(relative(0.6)).rounded(px(2.)).bg(color));
            // O GPUI recorta em retângulo: quem pinta o canto arredonda o próprio canto, senão cobre a moldura.
            div().size_full().p(px(8.)).flex().gap(px(6.)).bg(sidebar).rounded(px(8.))
                .child(div().w(relative(0.3)).child(bars(line)))
                .child(div().flex_1().rounded(px(6.)).p(px(8.)).bg(main).child(bars(line)))
        };
        let themes = [ThemeMode::Auto, ThemeMode::Light, ThemeMode::Dark, ThemeMode::Desktop];
        let theme_cards = div().flex().gap(px(12.)).children(themes.map(|mode| {
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
            div().flex_1().flex().flex_col().items_center().gap_2()
                .child(div().w_full().rounded(px(10.)).border_2().border_color(if selected { theme::accent() } else { theme::border_strong() })
                    .child(Button::new(SharedString::from(format!("theme-{key}")))
                        .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
                        .w_full().h(px(92.)).p(px(0.)).rounded(px(8.)).overflow_hidden()
                        .accessibility_label(tr(&format!("settings_theme_{key}")))
                        .child(art)
                        .on_click(cx.listener(move |this, _, window, cx| this.set_theme(mode, window, cx)))))
                .child(div().text_size(px(13.)).text_color(if selected { theme::text() } else { theme::muted() })
                    .when(selected, |el| el.font_weight(FontWeight::MEDIUM)).child(tr(&format!("settings_theme_{key}"))))
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
            let mini = div().h(px(92.)).w_full().flex().justify_between().rounded(px(8.)).overflow_hidden().bg(backdrop)
                .when(loose, |el| el.p(px(6.)))
                .child(side().when(!loose, |el| el.border_r_1().rounded_l(px(8.)))).child(side().when(!loose, |el| el.border_l_1().rounded_r(px(8.))));
            // A moldura mora fora do botão: o hover do botão redefine a cor da própria borda.
            div().flex_1().rounded(px(10.)).border_2().border_color(if selected { theme::accent() } else { theme::border_strong() })
                .child(Button::new(SharedString::from(format!("panels-{key}")))
                .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
                .w_full().h_auto().p(px(10.)).rounded(px(8.))
                .accessibility_label(tr(&format!("settings_panels_{key}")))
                .child(div().w_full().flex().flex_col().gap_2()
                    .child(mini)
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(tr(&format!("settings_panels_{key}"))))
                    .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr(&format!("settings_panels_{key}_desc")))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut next = appearance::get();
                    next.panels = value;
                    this.apply_appearance(next, true, cx);
                })))
        };
        let panel_cards = div().flex().gap(px(12.)).child(panel_card(Panels::Attached, cx)).child(panel_card(Panels::Floating, cx));

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
        let accents = div().flex().items_center().gap(px(6.))
            .children(theme::accent_swatches(dark).into_iter().enumerate().map(|(n, color)| swatch(format!("accent-{n}"),
                tr("settings_accent_n").replace("{n}", &(n + 1).to_string()), color, colors.accent == Swatch::Preset(n), false, Swatch::Preset(n), false, cx)))
            .child(picker(&self.settings_ui.accent_picker,matches!(colors.accent, Swatch::Custom(_))));
        let tints = div().flex().items_center().gap(px(6.))
            .children(theme::tint_swatches(dark).into_iter().enumerate().map(|(n, color)| swatch(format!("tint-{n}"),
                tr(if n == 0 { "settings_tint_none" } else { "settings_tint_n" }).replace("{n}", &n.to_string()), color,
                colors.tint == Swatch::Preset(n), n == 0, Swatch::Preset(n), true, cx)))
            .child(picker(&self.settings_ui.tint_picker,matches!(colors.tint, Swatch::Custom(_))));
        let no_tint = colors.tint == Swatch::Preset(0);
        // "Vale para o modo escuro. Copiar do claro": copia destaque, tinta e força do outro modo.
        let mode_note = div().flex().items_center().gap_1()
            .child(tr(if dark { "settings_colors_for_dark" } else { "settings_colors_for_light" }))
            .child(Button::new("copy-other-mode").ghost().xsmall().text_color(theme::text())
                .label(tr(if dark { "settings_copy_from_light" } else { "settings_copy_from_dark" }))
                .on_click(cx.listener(|this, _, window, cx| {
                    let mut next = appearance::get();
                    let dark = theme::is_dark();
                    *next.colors_mut(dark) = *next.colors(!dark);
                    this.apply_appearance(next, true, cx);
                    this.sync_sliders(window, cx);
                })));

        let palette = segmented("palette", &[tr("settings_palette_neutral"), tr("settings_palette_classic")],
            if a.palette == Palette::Neutral { 0 } else { 1 }, !from_wallpaper,
            |this: &mut Hangar, index, cx| { let mut next = appearance::get(); next.palette = if index == 0 { Palette::Neutral } else { Palette::Classic }; this.apply_appearance(next, true, cx); }, cx);
        let text_color = segmented("text-color", &[tr("settings_text_color_desktop"), tr("settings_text_color_app")],
            if a.desktop_text == DesktopText::App { 1 } else { 0 }, a.theme == ThemeMode::Desktop,
            |this: &mut Hangar, index, cx| { let mut next = appearance::get(); next.desktop_text = if index == 1 { DesktopText::App } else { DesktopText::Desktop }; this.apply_appearance(next, true, cx); }, cx);
        let wallpaper_note = || tr("settings_palette_desc");
        let color_box = settings_box()
            .child(self.row(IconName::Palette, tr("settings_palette"), Some(wallpaper_note()), !from_wallpaper, palette))
            .child(self.row_with(IconName::Droplet, tr("settings_accent"),
                if from_wallpaper { div().child(wallpaper_note()) } else { mode_note }, !from_wallpaper, accents.into_any_element()))
            .child(self.row(IconName::Droplet, tr("settings_tint"), Some(if from_wallpaper { wallpaper_note() } else { tr("settings_tint_desc") }),
                !from_wallpaper, tints.into_any_element()))
            // Sem tinta a força não muda nada; a linha fica visível e desligada.
            .child(self.slider_row(IconName::Droplet, tr("settings_tint_strength"), no_tint.then(|| tr("settings_tint_strength_off")),
                Knob::TintStrength, !no_tint && !from_wallpaper, &a, cx))
            .child(self.row(IconName::Type, tr("settings_text_color"), Some(tr("settings_text_color_desc")), a.theme == ThemeMode::Desktop, text_color));

        let background_box = settings_box()
            .child(self.row(IconName::Image, tr("settings_background"), Some(tr("settings_background_desc")), true,
                segmented_partial("background", &[tr("settings_bg_plain"), tr("settings_bg_texture"), tr("settings_bg_light"), tr("settings_bg_image"), tr("settings_bg_desktop")], 0, 1, cx)))
            .child(self.slider_row(IconName::Layers, tr("settings_transparency"), Some(tr("settings_transparency_desc")), Knob::Transparency, floating, &a, cx))
            .child(self.slider_row(IconName::Layers, tr("settings_solidity"), Some(tr("settings_only_floating")), Knob::Solidity, floating, &a, cx))
            .child(self.row(IconName::Layers, tr("settings_blur"), Some(tr("settings_blur_desc")), true, div().into_any_element()))
            .child(self.row(IconName::Monitor, tr("settings_wallpaper"), Some(next_version.clone()), false,
                segmented("wallpaper", &[tr("settings_wallpaper_window"), tr("settings_wallpaper_glass")], 0, false, |_, _, _| {}, cx)));

        let reading_box = settings_box()
            .child(self.row(IconName::FileText, tr("settings_reading"), Some(next_version.clone()), false,
                segmented("reading", &[tr("settings_reading_auto"), tr("settings_reading_none"), tr("settings_reading_text"), tr("settings_reading_sheet")], 0, false, |_, _, _| {}, cx)))
            .child(self.row(IconName::Layers, tr("settings_sheet_solidity"), Some(next_version.clone()), false, static_slider(60)))
            .child(self.row(IconName::Contrast, tr("settings_contrast"), Some(next_version.clone()), false, static_slider(30)));

        let font = segmented("font", &[tr("settings_font_system"), tr("settings_font_mono")], if a.font == Font::Mono { 1 } else { 0 }, true,
            |this: &mut Hangar, index, cx| { let mut next = appearance::get(); next.font = if index == 1 { Font::Mono } else { Font::System }; this.apply_appearance(next, true, cx); }, cx);
        let text_box = settings_box()
            .child(self.row(IconName::Type, tr("settings_font"), None, true, font))
            .child(self.slider_row(IconName::Type, tr("settings_text_size"), None, Knob::Size, true, &a, cx))
            .child(self.slider_row(IconName::SlidersHorizontal, tr("settings_line_height"), None, Knob::Line, true, &a, cx))
            .child(self.slider_row(IconName::PanelLeft, tr("settings_column"), None, Knob::Column, true, &a, cx));

        let conversation_box = settings_box()
            .child(self.row(IconName::Keyboard, tr("settings_tool_calls"), Some(next_version.clone()), false,
                segmented("tool-calls", &[tr("settings_tool_calls_classic"), tr("settings_tool_calls_chips")], 1, false, |_, _, _| {}, cx)))
            .child(self.row(IconName::FileText, tr("settings_task_list"), Some(next_version.clone()), false,
                segmented("task-list", &[tr("settings_task_list_hide"), tr("settings_task_list_progress")], 1, false, |_, _, _| {}, cx)))
            .child(self.row(IconName::Activity, tr("settings_thinking"), Some(next_version.clone()), false,
                segmented("thinking", &[tr("settings_thinking_none"), tr("settings_thinking_search"), tr("settings_thinking_all")], 1, false, |_, _, _| {}, cx)))
            .child(self.row(IconName::SlidersHorizontal, tr("settings_table_chart"), Some(next_version.clone()), false,
                segmented("table-chart", &[tr("settings_table_chart_hide"), tr("settings_table_chart_show")], 0, false, |_, _, _| {}, cx)));

        let height = segmented("sidebar-height", &[tr("settings_sidebar_full"), tr("settings_sidebar_content")],
            if a.sidebar_height == SidebarHeight::Content { 1 } else { 0 }, floating,
            |this: &mut Hangar, index, cx| { let mut next = appearance::get(); next.sidebar_height = if index == 1 { SidebarHeight::Content } else { SidebarHeight::Full }; this.apply_appearance(next, true, cx); }, cx);
        let sidebar_box = settings_box()
            .child(self.row(IconName::PanelLeft, tr("settings_collapsed_nav"), Some(next_version.clone()), false,
                segmented("collapsed-nav", &[tr("settings_collapsed_sidebar"), tr("settings_collapsed_tabs")], 0, false, |_, _, _| {}, cx)))
            .child(self.row(IconName::PanelLeft, tr("settings_sidebar_height"), Some(tr("settings_only_floating")), floating, height));

        div().flex().flex_col()
            .child(div().flex().items_end().gap_2()
                .child(div().flex_1().flex().flex_col()
                    .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr("settings_page_appearance")))
                    .child(div().mt(px(6.)).text_color(theme::muted()).child(tr("settings_appearance_lead"))))
                .child(Button::new("appearance-live").outline().small().label(tr("settings_live")).disabled(true).text_color(theme::faint()).tooltip(next_version.clone()))
                .child(Button::new("appearance-reset").outline().small().label(tr("settings_reset"))
                    .tooltip(tr("settings_reset_hint"))
                    .on_click(cx.listener(|this, _, window, cx| this.reset_appearance(window, cx)))))
            .when_some(self.appearance_note.clone(), |el, note| el.child(div().mt_3().text_sm().text_color(theme::warning()).child(note)))
            .child(preview)
            .child(heading(tr("settings_theme"))).child(theme_cards)
            .when_some(desktop_fallback, |el, note| el.child(div().mt_3().text_sm().text_color(theme::warning()).child(note)))
            .child(heading(tr("settings_panels"))).child(panel_cards)
            .child(heading(tr("settings_color"))).child(color_box)
            .child(heading(tr("settings_background_group"))).child(background_box)
            .child(heading(tr("settings_reading_group"))).child(reading_box)
            .child(heading(tr("settings_text_group"))).child(text_box)
            .child(heading(tr("settings_conversation_group"))).child(conversation_box)
            .child(heading(tr("settings_sidebar_group"))).child(sidebar_box)
            .child(div().mt(px(14.)).text_size(px(12.5)).text_color(theme::faint()).child(tr("settings_web_only")))
            .into_any_element()
    }

    /// Linha de configuração: ícone numa caixa, título e descrição, controle à direita.
    fn row(&self, icon: IconName, title: String, description: Option<String>, enabled: bool, control: AnyElement) -> Div {
        self.row_with(icon, title, description.map_or_else(div, |d| div().child(d)), enabled, control)
    }

    /// Linha cuja descrição carrega um controle (o "Copiar do claro" do Destaque).
    fn row_with(&self, icon: IconName, title: String, description: Div, enabled: bool, control: AnyElement) -> Div {
        // Divisória em cima de toda linha; a da primeira sobe 1px e some sob a borda da caixa.
        div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_center().gap(px(14.)).px_4().py(px(14.))
            .child(div().size(px(36.)).flex_shrink_0().rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
                .flex().items_center().justify_center().child(chrome::small_icon(icon, 16., theme::muted())))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().font_weight(FontWeight::MEDIUM).text_color(if enabled { theme::text() } else { theme::muted() }).child(title))
                .child(description.text_size(px(13.)).text_color(theme::muted())))
            // Desligado, quem esmaece é o próprio controle; esmaecer a linha também somava as duas e sumia o texto.
            .child(div().flex_shrink_0().child(control))
    }

    fn slider_row(&self, icon: IconName, title: String, description: Option<String>, knob: Knob, enabled: bool, a: &Appearance,
        cx: &mut Context<Self>) -> Div {
        let state = self.settings_ui.slider(knob);
        let control = div().w(px(230.)).flex().items_center()
            .child(self.slider_edge(knob, false, enabled, cx))
            .child(Slider::new(state).flex_1().bg(theme::accent()).text_color(theme::text()).disabled(!enabled))
            .child(self.slider_edge(knob, true, enabled, cx))
            .child(div().w(px(28.)).text_right().text_size(px(12.5)).text_color(theme::muted()).child(knob.read(a).to_string()));
        self.row(icon, title, description, enabled, control.into_any_element())
    }
}

fn settings_box() -> Div {
    div().flex().flex_col().rounded(px(14.)).border_1().border_color(theme::border()).bg(theme::boxed()).overflow_hidden()
}

/// Controle inerte de uma opção que ainda não chegou: mostra o valor padrão do web, sem aceitar arrasto.
fn static_slider(value: u8) -> AnyElement {
    div().w(px(230.)).flex().items_center().gap(px(10.)).opacity(0.6)
        .child(div().flex_1().h(px(4.)).rounded_full().bg(theme::hover())
            .child(div().h_full().rounded_full().bg(theme::accent()).w(relative(value as f32 / 100.))))
        .child(div().w(px(28.)).text_right().text_size(px(12.5)).text_color(theme::muted()).child(value.to_string()))
        .into_any_element()
}

/// Controle segmentado: uma silhueta só, segmento escolhido com fundo de destaque suave.
fn segmented(id: &'static str, labels: &[String], selected: usize, enabled: bool,
    pick: impl Fn(&mut Hangar, usize, &mut Context<Hangar>) + Clone + 'static, cx: &mut Context<Hangar>) -> AnyElement {
    segments(id, labels, selected, if enabled { labels.len() } else { 0 }, pick, cx)
}

/// Segmentado em que só os `available` primeiros já funcionam; os outros ficam visíveis e desligados.
fn segmented_partial(id: &'static str, labels: &[String], selected: usize, available: usize, cx: &mut Context<Hangar>) -> AnyElement {
    segments(id, labels, selected, available, |_, _, _| {}, cx)
}

fn segments(id: &'static str, labels: &[String], selected: usize, available: usize,
    pick: impl Fn(&mut Hangar, usize, &mut Context<Hangar>) + Clone + 'static, cx: &mut Context<Hangar>) -> AnyElement {
    let count = labels.len();
    div().flex().rounded(px(6.)).border_1().border_color(theme::border_strong()).overflow_hidden()
        .children(labels.iter().enumerate().map(|(n, label)| {
            // Opção que ainda não chegou não mostra escolha nenhuma: o padrão do web pareceria o estado do app.
            let on = n == selected && available > 0;
            let enabled = n < available;
            let pick = pick.clone();
            Button::new(SharedString::from(format!("{id}-{n}")))
                .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
                    .foreground(if on { theme::accent_text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
                // `small` dá ao rótulo o tamanho dos outros controles; o kit ignora `text_size` no rótulo.
                .small().h(px(28.)).px(px(11.)).rounded(px(0.))
                // Cor e fundo no próprio botão: vencem o estilo de desligado do kit, que apagava o texto a 50%.
                .text_color(if on { theme::accent_text() } else if enabled { theme::muted() } else { theme::faint() })
                .when(on, |el| el.bg(theme::accent_dim()))
                .when(n + 1 < count, |el| el.border_r_1().border_color(theme::border_strong()))
                .disabled(!enabled).label(label.clone())
                .when(!enabled && available > 0, |el| el.tooltip(tr("settings_next_version")))
                .on_click(cx.listener(move |this, _, _, cx| if enabled && !on { pick(this, n, cx) }))
        }))
        .into_any_element()
}
