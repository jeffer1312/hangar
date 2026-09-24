//! Configurações do app nativo: página que ocupa a janela, como no Zeron. A barra lateral vira a
//! navegação das seções; o conteúdo fica no centro, em linhas com ícone, título e controle à direita.
//! Aparência, Geral, Diário de uso e Sobre funcionam; as demais páginas dizem que chegam depois, sem fingir.
use super::*;
use std::{cell::Cell, rc::Rc};
use crate::appearance::{self, Appearance, Background, DesktopText, Font, Hex, Navigation, Palette, Panels, Reading, SidebarHeight, Swatch,
    ThemeMode, ThinkingTools, ToolLook, Wallpaper};
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

/// Linhas da Aparência que a busca acha: título e descrição, como chaves de tradução. O título é também
/// o que a linha desenhada compara para se destacar.
const APPEARANCE_ROWS: [(&str, Option<&str>); 27] = [
    ("settings_live", None), ("settings_reset", Some("settings_reset_hint")), ("settings_theme", None),
    ("settings_panels", Some("settings_panels_floating_desc")), ("settings_palette", Some("settings_palette_desc")),
    ("settings_accent", None), ("settings_tint", Some("settings_tint_desc")), ("settings_tint_strength", None),
    ("settings_text_color", Some("settings_text_color_desc")), ("settings_background", None),
    ("settings_transparency", Some("settings_transparency_desc")), ("settings_solidity", None),
    ("settings_blur", Some("settings_blur_hint")), ("settings_wallpaper", Some("settings_wallpaper_desc")),
    ("settings_reading", Some("settings_reading_desc")), ("settings_sheet_solidity", None), ("settings_contrast", None),
    ("settings_font", None), ("settings_text_size", None), ("settings_line_height", None), ("settings_column", None),
    ("settings_tool_calls", None), ("settings_task_list", None), ("settings_thinking", None), ("settings_table_chart", None),
    ("settings_collapsed_nav", None), ("settings_sidebar_height", Some("settings_only_floating")),
];

/// Linhas das outras páginas prontas, no mesmo formato.
const PAGE_ROWS: [(Page, &[(&str, Option<&str>)]); 4] = [
    (Page::Appearance, &APPEARANCE_ROWS),
    (Page::General, &[("settings_language", Some("settings_language_desc")), ("settings_currency", Some("settings_currency_search"))]),
    (Page::Diary, &[("settings_diary_rules", Some("settings_diary_rule_private")), ("settings_diary_download", Some("settings_diary_rule_local")),
        ("settings_diary_recent", None)]),
    (Page::About, &[("settings_about_app", None), ("settings_about_server", None), ("settings_about_update", Some("settings_about_update_desc"))]),
];

/// Um resultado da busca: a linha de uma página, ou a própria página (`row: None`) quando ela ainda não tem linhas.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Found { page: Page, row: Option<&'static str> }

impl Found {
    fn label(self) -> String {
        match self.row { Some(row) => format!("{} › {}", self.page.title(), tr(row)), None => self.page.title() }
    }
}

/// Minúsculas e sem acento, para "aparencia" achar "Aparência".
fn fold(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).map(|c| match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a', 'é' | 'è' | 'ê' | 'ë' => 'e', 'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o', 'ú' | 'ù' | 'û' | 'ü' => 'u', 'ç' => 'c', 'ñ' => 'n', c => c,
    }).collect()
}

/// Índices dos textos (título, descrição) que contêm a busca; busca vazia não acha nada.
fn matching(query: &str, texts: &[(String, String)]) -> Vec<usize> {
    let query = fold(query.trim());
    if query.is_empty() { return Vec::new(); }
    texts.iter().enumerate().filter(|(_, (title, desc))| fold(title).contains(&query) || fold(desc).contains(&query)).map(|(n, _)| n).collect()
}

fn find(query: &str) -> Vec<Found> {
    let rows = PAGE_ROWS.iter().flat_map(|&(page, rows)| rows.iter()
        .map(move |&(title, desc)| (Found { page, row: Some(title) }, tr(title), desc.map(tr).unwrap_or_default())));
    // Páginas que ainda não têm linhas continuam achadas pelo nome e abrem no aviso delas.
    let pages = Page::DEVICE.into_iter().chain(Page::SERVER).map(|page| (Found { page, row: None }, page.title(), String::new()));
    let all: Vec<_> = rows.chain(pages).collect();
    let texts: Vec<(String, String)> = all.iter().map(|(_, title, desc)| (title.clone(), desc.clone())).collect();
    matching(query, &texts).into_iter().map(|n| all[n].0).collect()
}

/// Caixa do "Ver ao vivo": largura do web e o quanto dela precisa ficar dentro da janela.
const LIVE_WIDTH: f32 = 360.;
const LIVE_VISIBLE: [f32; 2] = [120., 40.];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Knob { TintStrength, Transparency, Solidity, SheetSolidity, Contrast, Size, Line, Column }

impl Knob {
    const ALL: [Knob; 8] = [Knob::TintStrength, Knob::Transparency, Knob::Solidity, Knob::SheetSolidity, Knob::Contrast, Knob::Size, Knob::Line, Knob::Column];

    // A força da tinta é do modo que está na tela (escuro ou claro), como a cor.
    fn read(self, a: &Appearance) -> u16 {
        match self { Knob::TintStrength => a.colors(theme::is_dark()).tint_strength, Knob::Transparency => a.transparency, Knob::Solidity => a.solidity,
            Knob::SheetSolidity => a.sheet_solidity, Knob::Contrast => a.text_contrast,
            Knob::Size => a.text_size, Knob::Line => a.line_height, Knob::Column => a.column }
    }

    fn write(self, a: &mut Appearance, value: u16) {
        match self { Knob::TintStrength => a.colors_mut(theme::is_dark()).tint_strength = value, Knob::Transparency => a.transparency = value,
            Knob::Solidity => a.solidity = value, Knob::SheetSolidity => a.sheet_solidity = value, Knob::Contrast => a.text_contrast = value,
            Knob::Size => a.text_size = value, Knob::Line => a.line_height = value, Knob::Column => a.column = value }
    }

    fn range(self) -> (f32, f32) {
        match self {
            Knob::TintStrength => (5., 100.),
            Knob::Transparency | Knob::Solidity | Knob::SheetSolidity | Knob::Contrast => (0., 100.),
            Knob::Size | Knob::Line | Knob::Column => (50., 150.),
        }
    }
}

/// Estado vivo da página: os controles deslizantes guardam posição e arrasto entre desenhos.
pub(super) struct SettingsUi {
    sliders: Vec<(Knob, Entity<SliderState>)>,
    /// Cor livre de Destaque e de Tinta.
    accent_picker: Entity<ColorPickerState>,
    tint_picker: Entity<ColorPickerState>,
    search: Entity<InputState>,
    /// Resultados da busca, refeitos quando o texto muda; o desenho só lê.
    found: Vec<Found>,
    /// Resultado marcado pelas setas.
    pick: usize,
    /// Linha levada pela busca: fica destacada até outra busca ou até sair da página.
    hit: Option<&'static str>,
    scroll: ScrollHandle,
    /// A posição da linha só é conhecida no desenho: este sinal pede a rolagem até ela lá.
    reveal: Rc<Cell<bool>>,
    /// A Aparência numa caixa sobre a conversa, em vez da página.
    pub live: bool,
    /// Arrasto da caixa: ponto onde começou e o canto que ela tinha.
    drag: Option<(Point<Pixels>, [f32; 2])>,
    /// Onde ligar o desfoque, aberto pelo botão da linha (o tooltip não chega pelo teclado).
    blur_hint: bool,
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
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(tr("settings_search")));
        subscriptions.push(cx.subscribe_in(&search, window, |this: &mut Hangar, state, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let ui = &mut this.settings_ui;
                ui.found = find(&state.read(cx).value());
                (ui.pick, ui.hit) = (0, None);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.search_go(None, cx),
            _ => {}
        }));
        Self { sliders, accent_picker, tint_picker, search, found: Vec::new(), pick: 0, hit: None, scroll: ScrollHandle::new(),
            reveal: Rc::new(Cell::new(false)), live: false, drag: None, blur_hint: false, _subscriptions: subscriptions }
    }

    fn slider(&self, knob: Knob) -> &Entity<SliderState> {
        &self.sliders.iter().find(|(k, _)| *k == knob).expect("every knob has a slider").1
    }
}

fn swatch_ring(selected: bool) -> Hsla { if selected { theme::text() } else { transparent_black() } }

impl Hangar {
    pub(super) fn open_settings(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = Some(page);
        self.settings_ui.live = false;
        self.settings_ui.hit = None;
        // Toda página abre do topo; a rolagem é uma só para todas as páginas.
        self.settings_ui.scroll.set_offset(point(px(0.), px(0.)));
        // Busca de uma abertura anterior não volta no lugar da navegação.
        self.settings_ui.search.update(cx, |input, cx| input.set_value("", window, cx));
        (self.settings_ui.found, self.settings_ui.pick) = (Vec::new(), 0);
        // O campo de mensagem sai da tela com a página aberta; com o foco nele, o Esc não chega à raiz.
        self.root_focus.focus(window, cx);
        self.close_controls();
        self.command_panel = false;
        self.recent = None;
        self.settings_opened(page, cx);
        cx.notify();
    }

    /// Idioma trocado: o texto guardado no campo de busca acompanha.
    pub(super) fn relabel_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_ui.search.update(cx, |input, cx| input.set_placeholder(tr("settings_search"), window, cx));
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            let ui = &mut self.settings_ui;
            (ui.live, ui.hit, ui.drag) = (false, None, None);
            self.root_focus.focus(window, cx);
            cx.notify();
        }
    }

    /// A caixa ao vivo está na tela: a conversa por baixo continua sendo a janela.
    pub(super) fn settings_live(&self) -> bool { self.settings.is_some() && self.settings_ui.live }

    /// Com o foco na busca, o Esc é dela (limpa antes de fechar); a raiz não fecha a página por cima.
    pub(super) fn search_focused(&self, window: &Window, cx: &App) -> bool {
        self.settings_ui.search.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Ctrl+F com a página aberta leva ao campo de busca.
    pub(super) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_none() || self.settings_ui.live { return; }
        self.settings_ui.search.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Leva ao resultado escolhido (ou ao marcado pelas setas): abre a página, rola até a linha e a destaca.
    /// O foco fica na busca para as setas continuarem escolhendo.
    fn search_go(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        let ui = &mut self.settings_ui;
        let Some(&found) = ui.found.get(index.unwrap_or(ui.pick)) else { return };
        if let Some(n) = index { ui.pick = n; }
        let opened = self.settings != Some(found.page);
        ui.hit = found.row;
        match found.row {
            Some(_) => ui.reveal.set(true),
            None => { ui.reveal.set(false); ui.scroll.set_offset(point(px(0.), px(0.))); }
        }
        self.settings = Some(found.page);
        if opened { self.settings_opened(found.page, cx); }
        cx.notify();
    }

    fn move_pick(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ui = &mut self.settings_ui;
        if ui.found.is_empty() { return; }
        ui.pick = (ui.pick as isize + delta).rem_euclid(ui.found.len() as isize) as usize;
        cx.notify();
    }

    /// Esc na busca: com texto, limpa; vazia, fecha a página como o Esc de fora.
    fn search_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_ui.search.read(cx).value().is_empty() { return self.close_settings(window, cx); }
        self.settings_ui.search.update(cx, |input, cx| input.set_value("", window, cx));
        let ui = &mut self.settings_ui;
        (ui.found, ui.pick, ui.hit) = (Vec::new(), 0, None);
        cx.notify();
    }

    /// Caixa ao vivo: canto guardado limitado à janela de agora, para ela nunca sumir fora da tela.
    pub(super) fn live_corner(&self, window: &Window) -> [f32; 2] {
        let size = window.viewport_size();
        let [right, bottom] = appearance::get().live_corner;
        [right.min((f32::from(size.width) - LIVE_VISIBLE[0]).max(0.)), bottom.min((f32::from(size.height) - LIVE_VISIBLE[1]).max(0.))]
    }

    /// Arrasto pelo cabeçalho: segue o ponteiro na janela toda; ao soltar, grava onde a caixa ficou.
    pub(super) fn drag_live(&mut self, at: Point<Pixels>, pressed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((start, [right, bottom])) = self.settings_ui.drag else { return };
        let mut next = appearance::get();
        next.live_corner = [right - f32::from(at.x - start.x), bottom - f32::from(at.y - start.y)];
        appearance::set(next);
        let corner = self.live_corner(window);
        next.live_corner = corner;
        if !pressed { self.settings_ui.drag = None; }
        self.apply_appearance(next, !pressed, cx);
    }

    pub(super) fn live_dragging(&self) -> bool { self.settings_ui.drag.is_some() }

    /// Aplica na hora (o tema lê a cada desenho) e grava fora da thread da janela quando `save`.
    pub(super) fn apply_appearance(&mut self, next: Appearance, save: bool, cx: &mut Context<Self>) {
        let before = appearance::get();
        appearance::set(next);
        theme::sync_kit(None, cx);
        // Quem muda as linhas ou o desenho delas refaz a conversa: a lista guarda a altura de cada linha.
        if (before.tool_look, before.task_list, before.thinking_tools, before.table_chart)
            != (next.tool_look, next.task_list, next.thinking_tools, next.table_chart) {
            self.sync_rows(cx);
            self.list_state.remeasure();
        }
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
        // Na caixa solta o destaque suave some sobre o vidro; o item da página aberta leva mais cor.
        let selected = if floating { theme::accent().alpha(0.26) } else { theme::selected_row() };
        let nav_item = |page_item: Page, current: Page, cx: &mut Context<Self>| {
            let on = page_item == current;
            Button::new(SharedString::from(format!("settings-nav-{}", page_item.key())))
                .custom(ButtonCustomVariant::new(cx).color(if on { selected } else { transparent_black() })
                    .foreground(if on { theme::text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
                .w_full().h(px(32.)).px(px(10.)).rounded(px(6.))
                // A cor da variante sai esmaecida pelo kit; o fundo no próprio botão é o que aparece.
                .when(on, |el| el.bg(selected))
                // O botão centraliza o conteúdo; a linha de navegação alinha ícone e nome à esquerda.
                .child(div().w_full().flex().items_center().gap(px(10.))
                    .child(chrome::small_icon(page_item.icon(), 16., if on { theme::text() } else { theme::faint() }))
                    .child(div().text_size(px(13.5)).child(page_item.title())))
                .on_click(cx.listener(move |this, _, window, cx| this.open_settings(page_item, window, cx)))
        };
        let searching = !self.settings_ui.search.read(cx).value().trim().is_empty();
        let group = |label: String| div().px(px(10.)).pt(px(12.)).pb(px(6.)).flex().items_center().gap_2()
            .text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::faint()).child(label);
        let server = self.server_label(cx);
        let nav = div().w(px(284.)).flex_shrink_0().h_full().flex().flex_col().px(px(10.)).bg(theme::chrome())
            .map(|el| if floating { el.rounded(px(18.)).border_1().border_color(theme::border()).shadow(theme::panel_shadow()) }
                else { el.border_r_1().border_color(theme::border()) })
            .child(div().h(px(44.)).flex_shrink_0().px(px(6.)).flex().items_center().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("settings")))
            // Setas escolhem o resultado antes do campo andar o cursor; Esc limpa ou fecha.
            .child(div().flex_shrink_0().mb(px(10.))
                .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_pick(-1, cx)))
                .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_pick(1, cx)))
                .capture_action(cx.listener(|this, _: &Escape, window, cx| this.search_escape(window, cx)))
                .child(Input::new(&self.settings_ui.search).h(px(32.)).aria_label(tr("settings_search"))
                    .prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))
                    .suffix(chrome::kbd("Ctrl F"))))
            .child(if searching { self.render_found(cx) } else {
                div().id("settings-nav").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(2.))
                    .child(group(tr("settings_group_device")))
                    .children(Page::DEVICE.map(|p| nav_item(p, page, cx)))
                    .child(group(tr("settings_group_server"))
                        .child(div().ml_auto().max_w(px(140.)).truncate().text_size(px(12.5)).text_color(theme::text()).child(server)))
                    .children(Page::SERVER.map(|p| nav_item(p, page, cx)))
            })
            .child(div().h(px(48.)).flex_shrink_0().mx(px(-10.)).px(px(10.)).border_t_1().border_color(theme::border()).flex().items_center()
                .child(Button::new("settings-back").ghost().w_full().h(px(32.))
                    .child(div().w_full().flex().items_center().gap_2()
                        .child(chrome::small_icon(IconName::ArrowLeft, 16., theme::muted()))
                        .child(div().flex_1().text_sm().text_color(theme::muted()).child(tr("settings_back")))
                        .child(chrome::kbd("Esc")))
                    .on_click(cx.listener(|this, _, window, cx| this.close_settings(window, cx)))));
        let body = match page {
            Page::Appearance => self.render_appearance(cx),
            Page::General => self.render_general(cx),
            Page::Diary => self.render_diary(cx),
            Page::About => self.render_about(cx),
            _ => self.render_page_soon(page, cx),
        };
        let content = div().id("settings-content").flex_1().min_w_0().h_full().overflow_y_scroll().track_scroll(&self.settings_ui.scroll)
            .child(div().w_full().flex().justify_center().child(div().w(px(720.)).max_w_full().px_4().pt(px(44.)).pb(px(40.)).child(body)));
        div().size_full().flex().when(floating, |el| el.p(px(10.)).gap(px(10.))).child(nav).child(content).into_any_element()
    }

    /// Resultados da busca no lugar da navegação: "Página › Linha", o marcado pelas setas em destaque.
    fn render_found(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let ui = &self.settings_ui;
        let list = div().id("settings-found").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(2.));
        if ui.found.is_empty() {
            return list.child(div().px(px(10.)).py(px(8.)).text_size(px(13.)).text_color(theme::muted()).child(tr("settings_search_none")));
        }
        list.children(ui.found.iter().enumerate().map(|(n, found)| {
            let on = n == ui.pick;
            Button::new(SharedString::from(format!("settings-found-{n}")))
                .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
                    .foreground(if on { theme::text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
                .w_full().h_auto().min_h(px(32.)).px(px(10.)).py(px(6.)).rounded(px(6.))
                .child(div().w_full().text_size(px(13.)).whitespace_normal().child(found.label()))
                .on_click(cx.listener(move |this, _, _, cx| this.search_go(Some(n), cx)))
        }))
    }

    /// Destaque da linha levada pela busca, e a rolagem até ela quando o desenho já sabe onde ela está.
    pub(super) fn mark(&self, el: Div, key: &str) -> Div {
        if self.settings_ui.hit != Some(key) { return el; }
        let (scroll, reveal) = (self.settings_ui.scroll.clone(), self.settings_ui.reveal.clone());
        el.relative().bg(theme::accent_dim()).child(canvas(move |bounds, window, _| {
            if !reveal.get() { return; }
            let area = scroll.bounds();
            // O contêiner ainda não foi medido neste desenho: tenta de novo no próximo.
            if area.size.height <= px(0.) { window.refresh(); return; }
            let (offset, max) = (scroll.offset(), scroll.max_offset());
            let y = (offset.y - (bounds.top() - area.top()) + px(96.)).clamp(-max.y, px(0.));
            scroll.set_offset(point(offset.x, y));
            reveal.set(false);
            window.refresh();
        }, |_, _, _, _| {}).absolute().inset_0())
    }

    /// A Aparência numa caixa de 360px sobre a conversa, arrastável pelo cabeçalho; a posição fica gravada.
    pub(super) fn render_live(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let [right, bottom] = self.live_corner(window);
        // Para abaixo da barra do topo (abas ou cabeçalho), que continua alcançável com a caixa aberta.
        let height = (f32::from(window.viewport_size().height) - bottom - 56.).max(LIVE_VISIBLE[1]);
        let header = div().id("live-header").h(px(40.)).flex_shrink_0().pl(px(14.)).pr(px(6.)).flex().items_center().gap_1()
            .border_b_1().border_color(theme::border()).cursor_move()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.settings_ui.drag = Some((event.position, this.live_corner(window)));
                cx.stop_propagation();
                cx.notify();
            }))
            .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("settings_page_appearance")))
            .child(chrome::icon_button("live-page", IconName::Maximize, tr("settings_live_back"), cx)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.settings_ui.live = false;
                    this.root_focus.focus(window, cx);
                    cx.notify();
                })))
            .child(chrome::icon_button("live-close", IconName::Close, tr("close"), cx)
                .on_click(cx.listener(|this, _, window, cx| this.close_settings(window, cx))));
        let body = self.render_appearance(cx);
        div().id("live-box").absolute().right(px(right)).bottom(px(bottom)).w(px(LIVE_WIDTH)).max_h(px(height))
            // Opaca mesmo na caixa solta: a conversa atrás não pode atravessar as linhas.
            .flex().flex_col().rounded(px(14.)).border_1().border_color(theme::border_strong()).bg(theme::raised())
            .shadow(theme::popover_shadow()).overflow_hidden().occlude()
            // O ponteiro fica sobre a própria caixa, que tapa a raiz: o arrasto e o fim dele moram aqui também.
            .when(self.live_dragging(), |el| el.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.drag_live(event.position, event.pressed_button == Some(MouseButton::Left), window, cx);
            })))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, window, cx| this.drag_live(event.position, false, window, cx)))
            .child(header)
            .child(div().id("live-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.settings_ui.scroll)
                .px(px(12.)).pb(px(14.)).child(body))
            .into_any_element()
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
        let live = self.settings_ui.live;
        let heading = |key: &'static str| self.mark(div().mt(px(if live { 20. } else { 28. })).mb(px(10.)).rounded(px(6.))
            .text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tr(key)), key);
        // Na caixa ao vivo as miniaturas encolhem para caber quatro temas em 360px.
        let thumb = px(if live { 60. } else { 92. });

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
                        .w_full().h(thumb).p(px(0.)).rounded(px(8.)).overflow_hidden()
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
            let mini = div().h(thumb).w_full().flex().justify_between().rounded(px(8.)).overflow_hidden().bg(backdrop)
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
                    .when(!live, |el| el.child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal()
                        .child(tr(&format!("settings_panels_{key}_desc"))))))
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
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.palette = if index == 0 { Palette::Neutral } else { Palette::Classic }; this.apply_appearance(next, true, cx); }, cx);
        let text_color = segmented("text-color", &[tr("settings_text_color_desktop"), tr("settings_text_color_app")],
            if a.desktop_text == DesktopText::App { 1 } else { 0 }, a.theme == ThemeMode::Desktop,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.desktop_text = if index == 1 { DesktopText::App } else { DesktopText::Desktop }; this.apply_appearance(next, true, cx); }, cx);
        let wallpaper_note = || tr("settings_palette_desc");
        let color_box = settings_box()
            .child(self.row(IconName::Palette, "settings_palette", Some(wallpaper_note()), !from_wallpaper, palette))
            .child(self.row_with(IconName::Droplet, "settings_accent",
                if from_wallpaper { div().child(wallpaper_note()) } else { mode_note }, !from_wallpaper, accents.into_any_element()))
            .child(self.row(IconName::Droplet, "settings_tint", Some(if from_wallpaper { wallpaper_note() } else { tr("settings_tint_desc") }),
                !from_wallpaper, tints.into_any_element()))
            // Sem tinta a força não muda nada; a linha fica visível e desligada.
            .child(self.slider_row(IconName::Droplet, "settings_tint_strength", no_tint.then(|| tr("settings_tint_strength_off")),
                Knob::TintStrength, !no_tint && !from_wallpaper, &a, cx))
            .child(self.row(IconName::Type, "settings_text_color", Some(tr("settings_text_color_desc")), a.theme == ThemeMode::Desktop, text_color));

        const BACKGROUNDS: [Background; 5] = [Background::Plain, Background::Texture, Background::Light, Background::Image, Background::Desktop];
        // Escolha, cópia ou remoção da imagem em andamento: o grupo fica travado, mostrando o que está escolhido.
        let busy = self.backdrop_busy;
        let background = segments("background", &[tr("settings_bg_plain"), tr("settings_bg_texture"), tr("settings_bg_light"), tr("settings_bg_image"), tr("settings_bg_desktop")],
            BACKGROUNDS.iter().position(|b| *b == a.background).unwrap_or(0), if busy.is_some() { 0 } else { BACKGROUNDS.len() }, busy.is_some(), tr("settings_next_version"),
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
            .child(Button::new("background-image-remove").outline().small().label(tr("settings_image_remove")).disabled(busy.is_some())
                .on_click(cx.listener(|this, _, _, cx| this.remove_backdrop(cx))));
        let desktop_background = a.background == Background::Desktop;
        let wallpaper = segmented("wallpaper", &[tr("settings_wallpaper_window"), tr("settings_wallpaper_glass")],
            if a.wallpaper == Wallpaper::Glass { 1 } else { 0 }, desktop_background && busy.is_none(),
            |this: &mut Hangar, index, window: &mut Window, cx| {
                let mut next = appearance::get();
                next.wallpaper = if index == 1 { Wallpaper::Glass } else { Wallpaper::Window };
                this.apply_appearance(next, true, cx);
                this.refresh_backdrop(window, cx);
            }, cx);
        // Com imagem ou área de trabalho atrás, a Transparência é o véu e vale também nos painéis colados.
        let see_through = floating || a.busy_background();
        let background_box = settings_box()
            .child(self.row(IconName::Image, "settings_background", busy.map(|b| b.note()), true, background))
            .when(a.background == Background::Image, |el| el.child(self.row(IconName::Image, "settings_image",
                Some(tr("settings_image_desc")), true, image_actions.into_any_element())))
            .child(self.slider_row(IconName::Layers, "settings_transparency",
                Some(tr(if see_through { "settings_transparency_desc" } else { "settings_transparency_off" })), Knob::Transparency, see_through, &a, cx))
            .child(self.slider_row(IconName::Layers, "settings_solidity", Some(tr("settings_only_floating")), Knob::Solidity, floating, &a, cx))
            // Nada a escolher aqui: a linha diz de quem é o desfoque; o botão (mouse ou teclado) abre onde ligar.
            .child(self.row(IconName::Layers, "settings_blur",
                Some(if self.settings_ui.blur_hint { format!("{} {}", tr("settings_blur_desc"), tr("settings_blur_hint")) } else { tr("settings_blur_desc") }), true,
                chrome::icon_button("blur-hint", IconName::Info, tr("settings_blur_hint_toggle"), cx).selected(self.settings_ui.blur_hint)
                    .on_click(cx.listener(|this, _, _, cx| { this.settings_ui.blur_hint = !this.settings_ui.blur_hint; cx.notify(); }))
                    .into_any_element()))
            .child(self.row(IconName::Monitor, "settings_wallpaper",
                Some(tr(if desktop_background { "settings_wallpaper_desc" } else { "settings_wallpaper_only_desktop" })), desktop_background, wallpaper));

        const READINGS: [Reading; 4] = [Reading::Auto, Reading::None, Reading::Text, Reading::Sheet];
        let reading = segmented("reading", &[tr("settings_reading_auto"), tr("settings_reading_none"), tr("settings_reading_text"), tr("settings_reading_sheet")],
            READINGS.iter().position(|r| *r == a.reading).unwrap_or(0), true,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.reading = READINGS[index]; this.apply_appearance(next, true, cx); }, cx);
        let sheet_on = a.reading == Reading::Sheet;
        let text_on = a.effective_reading() == Reading::Text;
        let reading_box = settings_box()
            .child(self.row(IconName::FileText, "settings_reading", Some(tr("settings_reading_desc")), true, reading))
            .child(self.slider_row(IconName::Layers, "settings_sheet_solidity", (!sheet_on).then(|| tr("settings_sheet_only")),
                Knob::SheetSolidity, sheet_on, &a, cx))
            .child(self.slider_row(IconName::Contrast, "settings_contrast", (!text_on).then(|| tr("settings_contrast_only")),
                Knob::Contrast, text_on, &a, cx));

        let font = segmented("font", &[tr("settings_font_system"), tr("settings_font_mono")], if a.font == Font::Mono { 1 } else { 0 }, true,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.font = if index == 1 { Font::Mono } else { Font::System }; this.apply_appearance(next, true, cx); }, cx);
        let text_box = settings_box()
            .child(self.row(IconName::Type, "settings_font", None, true, font))
            .child(self.slider_row(IconName::Type, "settings_text_size", None, Knob::Size, true, &a, cx))
            .child(self.slider_row(IconName::SlidersHorizontal, "settings_line_height", None, Knob::Line, true, &a, cx))
            .child(self.slider_row(IconName::PanelLeft, "settings_column", None, Knob::Column, true, &a, cx));

        const THINKING: [ThinkingTools; 3] = [ThinkingTools::None, ThinkingTools::Search, ThinkingTools::All];
        let conversation_box = settings_box()
            .child(self.row(IconName::Wrench, "settings_tool_calls", None, true,
                segmented("tool-calls", &[tr("settings_tool_calls_classic"), tr("settings_tool_calls_chips")], (a.tool_look == ToolLook::Chips) as usize, true,
                    |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.tool_look = if index == 1 { ToolLook::Chips } else { ToolLook::Classic }; this.apply_appearance(next, true, cx); }, cx)))
            .child(self.row(IconName::ListChecks, "settings_task_list", None, true,
                segmented("task-list", &[tr("settings_task_list_hide"), tr("settings_task_list_progress")], a.task_list as usize, true,
                    |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.task_list = index == 1; this.apply_appearance(next, true, cx); }, cx)))
            .child(self.row(IconName::Activity, "settings_thinking", None, true,
                segmented("thinking", &[tr("settings_thinking_none"), tr("settings_thinking_search"), tr("settings_thinking_all")],
                    THINKING.iter().position(|t| *t == a.thinking_tools).unwrap_or(1), true,
                    |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.thinking_tools = THINKING[index]; this.apply_appearance(next, true, cx); }, cx)))
            .child(self.row(IconName::ChartColumn, "settings_table_chart", None, true,
                segmented("table-chart", &[tr("settings_table_chart_hide"), tr("settings_table_chart_show")], a.table_chart as usize, true,
                    |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.table_chart = index == 1; this.apply_appearance(next, true, cx); }, cx)));

        let height = segmented("sidebar-height", &[tr("settings_sidebar_full"), tr("settings_sidebar_content")],
            if a.sidebar_height == SidebarHeight::Content { 1 } else { 0 }, floating,
            |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.sidebar_height = if index == 1 { SidebarHeight::Content } else { SidebarHeight::Full }; this.apply_appearance(next, true, cx); }, cx);
        let sidebar_box = settings_box()
            .child(self.row(IconName::PanelLeft, "settings_collapsed_nav", None, true,
                segmented("collapsed-nav", &[tr("settings_collapsed_sidebar"), tr("settings_collapsed_tabs")], (a.navigation == Navigation::Tabs) as usize, true,
                    |this: &mut Hangar, index, _: &mut Window, cx| { let mut next = appearance::get(); next.navigation = if index == 1 { Navigation::Tabs } else { Navigation::Sidebar }; this.apply_appearance(next, true, cx); }, cx)))
            .child(self.row(IconName::PanelLeft, "settings_sidebar_height", Some(tr("settings_only_floating")), floating, height));

        // Os dois botões do topo com a borda forte do mock.
        let top_button = |id: &'static str, key: &'static str| Button::new(id).outline().small().border_color(theme::border_strong()).label(tr(key));
        let reset = top_button("appearance-reset", "settings_reset").tooltip(tr("settings_reset_hint"))
            .on_click(cx.listener(|this, _, window, cx| this.reset_appearance(window, cx)));
        // A âncora da rolagem até os botões do topo é a faixa deles.
        let top_key = self.settings_ui.hit.filter(|k| matches!(*k, "settings_live" | "settings_reset")).unwrap_or("");
        let top = if live { div().pt(px(12.)).flex().justify_end().child(reset) } else {
            div().flex().items_end().gap_2()
                .child(div().flex_1().flex().flex_col()
                    .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr("settings_page_appearance")))
                    .child(div().mt(px(6.)).text_color(theme::muted()).child(tr("settings_appearance_lead"))))
                .child(top_button("appearance-live", "settings_live").on_click(cx.listener(|this, _, window, cx| {
                    this.settings_ui.live = true;
                    this.settings_ui.hit = None;
                    this.root_focus.focus(window, cx);
                    cx.notify();
                })))
                .child(reset)
        };
        div().flex().flex_col()
            .child(self.mark(top, top_key))
            .when_some(self.appearance_note.clone(), |el, note| el.child(div().mt_3().text_sm().text_color(theme::warning()).child(note)))
            // A caixa ao vivo não repete a prévia: a conversa de verdade está atrás dela.
            .when(!live, |el| el.child(preview))
            .child(heading("settings_theme")).child(theme_cards)
            .when_some(desktop_fallback, |el, note| el.child(div().mt_3().text_sm().text_color(theme::warning()).child(note)))
            .child(heading("settings_panels")).child(panel_cards)
            .child(heading("settings_color")).child(color_box)
            .child(heading("settings_background_group")).child(background_box)
            .when_some(self.backdrop_note.clone(), |el, note| el.child(div().mt_3().text_sm().text_color(theme::warning()).child(note)))
            .child(heading("settings_reading_group")).child(reading_box)
            .child(heading("settings_text_group")).child(text_box)
            .child(heading("settings_conversation_group")).child(conversation_box)
            .child(heading("settings_sidebar_group")).child(sidebar_box)
            .child(div().mt(px(14.)).text_size(px(12.5)).text_color(theme::faint()).child(tr("settings_web_only")))
            .into_any_element()
    }

    /// Linha de configuração: ícone numa caixa, título e descrição, controle à direita. `title` é a chave de
    /// tradução, a mesma que a busca usa para destacar a linha.
    pub(super) fn row(&self, icon: IconName, title: &'static str, description: Option<String>, enabled: bool, control: AnyElement) -> Div {
        self.row_with(icon, title, description.map_or_else(div, |d| div().child(d)), enabled, control)
    }

    /// Linha cuja descrição carrega um controle (o "Copiar do claro" do Destaque).
    pub(super) fn row_with(&self, icon: IconName, title: &'static str, description: Div, enabled: bool, control: AnyElement) -> Div {
        // Na caixa ao vivo (360px) o controle desce para baixo do título, senão espreme o texto.
        let live = self.settings_ui.live;
        let head = div().flex_1().min_w_0().flex().items_center().gap(px(if live { 10. } else { 14. }))
            .child(div().size(px(if live { 28. } else { 36. })).flex_shrink_0().rounded(px(if live { 8. } else { 10. })).border_1()
                .border_color(theme::border()).bg(theme::inset()).flex().items_center().justify_center().child(chrome::small_icon(icon, 16., theme::muted())))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().font_weight(FontWeight::MEDIUM).text_color(if enabled { theme::text() } else { theme::muted() }).child(tr(title)))
                .child(description.text_size(px(13.)).text_color(theme::muted())));
        // Divisória em cima de toda linha; a da primeira sobe 1px e some sob a borda da caixa.
        let row = div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex()
            .map(|el| if live { el.flex_col().gap(px(10.)).px_3().py(px(12.)) } else { el.items_center().gap(px(14.)).px_4().py(px(14.)) })
            .child(head)
            // Desligado, quem esmaece é o próprio controle; esmaecer a linha também somava as duas e sumia o texto.
            // `flex` no invólucro: embaixo do título o controle fica do tamanho dele, sem esticar a borda.
            .child(div().flex_shrink_0().when(live, |el| el.flex().pl(px(38.))).child(control));
        self.mark(row, title)
    }

    fn slider_row(&self, icon: IconName, title: &'static str, description: Option<String>, knob: Knob, enabled: bool, a: &Appearance,
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

pub(super) fn settings_box() -> Div {
    div().flex().flex_col().rounded(px(14.)).border_1().border_color(theme::border()).bg(theme::boxed()).overflow_hidden()
}

/// Controle segmentado: uma silhueta só, segmento escolhido com fundo de destaque suave.
fn segmented(id: &'static str, labels: &[String], selected: usize, enabled: bool,
    pick: impl Fn(&mut Hangar, usize, &mut Window, &mut Context<Hangar>) + Clone + 'static, cx: &mut Context<Hangar>) -> AnyElement {
    segments(id, labels, selected, if enabled { labels.len() } else { 0 }, false, tr("settings_next_version"), pick, cx)
}

/// `locked`: todos desligados por um instante (operação em andamento), mas a escolha atual continua marcada.
/// `off_note`: a dica das opções além de `available`, dizendo por que estão desligadas.
pub(super) fn segments(id: &'static str, labels: &[String], selected: usize, available: usize, locked: bool, off_note: String,
    pick: impl Fn(&mut Hangar, usize, &mut Window, &mut Context<Hangar>) + Clone + 'static, cx: &mut Context<Hangar>) -> AnyElement {
    let count = labels.len();
    div().flex().rounded(px(6.)).border_1().border_color(theme::border_strong()).overflow_hidden()
        .when(locked, |el| el.opacity(0.6))
        .children(labels.iter().enumerate().map(|(n, label)| {
            // Opção que ainda não chegou não mostra escolha nenhuma: o padrão do web pareceria o estado do app.
            let on = n == selected && (available > 0 || locked);
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
                .when(!enabled && available > 0, |el| el.tooltip(off_note.clone()))
                .on_click(cx.listener(move |this, _, window, cx| if enabled && !on { pick(this, n, window, cx) }))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{fold, matching};

    #[test]
    fn search_ignores_accents_and_case_and_reads_descriptions() {
        assert_eq!(fold("Transparência ÇÃO"), "transparencia cao");
        let texts = vec![("Transparência".to_owned(), String::new()), ("Desfoque do fundo".to_owned(), "No Hyprland: decoration:blur".to_owned()),
            ("Aparência".to_owned(), String::new())];
        assert_eq!(matching("transparencia", &texts), vec![0]);
        assert_eq!(matching("APARÊNCIA", &texts), vec![2]);
        assert_eq!(matching("hyprland", &texts), vec![1]);
        assert!(matching("  ", &texts).is_empty());
        assert!(matching("nada disso", &texts).is_empty());
    }
}
