//! Configurações do app nativo: página que ocupa a janela, como no Zeron. A barra lateral vira a
//! navegação das seções; o conteúdo fica no centro, em linhas com ícone, título e controle à direita.
//! As páginas de configuração usam a navegação lateral e mostram o conteúdo do servidor conectado.
use super::*;
use std::{cell::Cell, rc::Rc};
use crate::appearance::{self, Appearance, AskHighlight, Background, BackgroundScope, CodeFont, DesktopText, Font, Hex, Navigation, Palette, Panels, Reading, SidebarHeight, SurfaceMaterial, Swatch,
    ThemeMode, ThinkingTools, ToolLook, Wallpaper};
use gpui_kit::base::AccordionTrigger;
use gpui_kit::component::{color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState}, slider::{Slider, SliderEvent, SliderState}, tooltip::Tooltip,
    IndexPath, select::{Select, SelectEvent, SelectState}, searchable_list::{SearchableListItem, SearchableVec}};

mod appearance_page;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    General, Appearance, Diary, About, Migration,
    Servers, Sync, Connect, SharedConfig, Accounts, Orchestration, Harnesses, Voice, Jev, Windows, Notifications, Shortcuts, Attachments, Advanced,
}

impl Page {
    const DEVICE: [Page; 4] = [Page::General, Page::Appearance, Page::Diary, Page::About];
    const SERVER: [Page; 14] = [Page::Servers, Page::Sync, Page::Connect, Page::SharedConfig, Page::Accounts, Page::Orchestration, Page::Harnesses, Page::Voice,
        Page::Jev, Page::Windows, Page::Notifications, Page::Shortcuts, Page::Attachments, Page::Advanced];

    /// As seções da navegação, na ordem dela; a voz abre uma pelo `key`.
    pub(super) fn sections() -> impl Iterator<Item = Page> { Self::DEVICE.into_iter().chain(Self::SERVER) }

    pub(super) fn key(self) -> &'static str {
        match self {
            Page::General => "general", Page::Appearance => "appearance", Page::Diary => "diary", Page::About => "about", Page::Migration => "migration",
            Page::Servers => "servers", Page::Sync => "sync", Page::Connect => "connect", Page::SharedConfig => "shared_config", Page::Accounts => "accounts", Page::Orchestration => "orchestration",
            Page::Harnesses => "harnesses", Page::Voice => "voice", Page::Jev => "jev", Page::Windows => "windows", Page::Notifications => "notifications",
            Page::Shortcuts => "shortcuts", Page::Attachments => "attachments", Page::Advanced => "advanced",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Page::General => IconName::Globe, Page::Appearance => IconName::Palette, Page::Diary => IconName::FileText,
            Page::About => IconName::Info, Page::Migration => IconName::Activity, Page::Servers => IconName::Server, Page::Sync => IconName::RefreshCw, Page::Connect => IconName::Globe, Page::SharedConfig => IconName::Layers,
            Page::Accounts => IconName::User, Page::Orchestration => IconName::Users, Page::Harnesses => IconName::Activity,
            Page::Voice => IconName::Mic, Page::Jev => IconName::Zap, Page::Windows => IconName::Monitor, Page::Notifications => IconName::Bell,
            Page::Shortcuts => IconName::Keyboard, Page::Attachments => IconName::Paperclip, Page::Advanced => IconName::SlidersHorizontal,
        }
    }

    pub(super) fn title(self) -> String {
        // O nome é o do web: uma frase, um dicionário.
        if self == Page::Jev { return tr_shared("jev_title", &[]); }
        if self == Page::SharedConfig { return tr_shared("shared_config_title", &[]); }
        if self == Page::Migration { return tr_shared("migration_title", &[]); }
        tr(&format!("settings_page_{}", self.key()))
    }
}

/// Linhas da Aparência que a busca acha: título e descrição, como chaves de tradução. O título é também
/// o que a linha desenhada compara para se destacar.
const APPEARANCE_ROWS: [(&str, Option<&str>); 38] = [
    ("settings_live", None), ("settings_reset", Some("settings_reset_hint")),
    ("settings_style", Some("settings_style_hint")), ("settings_theme", None),
    ("settings_panels", Some("settings_panels_floating_desc")), ("settings_palette", Some("settings_palette_desc")),
    ("settings_accent", None), ("settings_tint", Some("settings_tint_desc")), ("settings_tint_strength", None),
    ("settings_text_color", Some("settings_text_color_desc")), ("settings_background", None),
    ("settings_background_scope", None), ("settings_image", Some("settings_image_desc")),
    ("settings_background_effect", None),
    ("settings_transparency", Some("settings_transparency_desc")), ("settings_surface_material", Some("settings_surface_material_desc")), ("settings_solidity", None),
    ("settings_blur", Some("settings_blur_hint")), ("settings_wallpaper", Some("settings_wallpaper_desc")),
    ("settings_reading", Some("settings_reading_desc")), ("settings_sheet_solidity", None), ("settings_contrast", None),
    ("settings_font", Some("settings_font_desc")), ("settings_text_size", None), ("settings_code_font", Some("settings_code_font_desc")),
    ("settings_code_size", None), ("settings_terminal_font", Some("settings_terminal_font_desc")),
    ("settings_terminal_size", None), ("settings_line_height", None), ("settings_column", None),
    ("settings_tool_calls", None), ("settings_task_list", None), ("settings_thinking", None), ("settings_table_chart", None),
    ("settings_ask_highlight", Some("settings_ask_highlight_desc")),
    ("settings_collapsed_nav", None), ("settings_sidebar_density", Some("settings_sidebar_density_hint")), ("settings_sidebar_height", Some("settings_only_floating")),
];

/// Linhas das outras páginas prontas, no mesmo formato.
const PAGE_ROWS: &[(Page, &[(&str, Option<&str>)])] = &[
    // As entradas de Máquinas do web (`BuscaConfig.svelte`); a busca só abre a página, sem abrir o detalhe.
    (Page::Servers, &[("machines_others", None), ("machines_id", Some("machines_id_legend")),
        ("server_term_origins", Some("server_term_origins_help")), ("machines_sign_out_title", None), ("machines_reconnect", None),
        ("machines_search_tailscale", Some("machines_search_tailscale_help"))]),
    (Page::Sync, &[("sync_config_ativa", Some("sync_config_ganho")), ("sync_config_ativar", Some("sync_config_direta")),
        ("sync_config_desativar", Some("sync_config_desativar_aviso")), ("sync_config_copiar", None)]),
    (Page::Connect, &[("connect_code", Some("connect_code_help"))]),
    (Page::Appearance, &APPEARANCE_ROWS),
    (Page::General, &[("settings_language", Some("settings_language_desc")), ("settings_currency", Some("settings_currency_search")),
        ("settings_tray", Some("settings_tray_desc")), ("settings_chrome_autofill", Some("settings_chrome_autofill_desc"))]),
    (Page::Diary, &[("settings_diary_rules", Some("settings_diary_rule_private")), ("settings_diary_download", Some("settings_diary_rule_local")),
        ("settings_diary_recent", None)]),
    (Page::About, &[("settings_about_app", None), ("settings_about_server", None), ("settings_about_update", Some("settings_about_update_desc")),
        ("settings_channel_title", Some("settings_channel_help"))]),
    (Page::Accounts, &[("accounts_subscriptions", Some("accounts_menu_note")), ("accounts_models", None),
        ("accounts_others", Some("accounts_others_empty")), ("accounts_density", None), ("accounts_refresh", None)]),
    (Page::Orchestration, &[("orchestration_intro", None), ("orchestration_unrestricted", None)]),
    (Page::Shortcuts, &[("shortcuts_add", Some("shortcuts_lead")), ("shortcuts_restore", Some("shortcuts_restore_help")),
        ("keyboard_title", Some("keyboard_lead")), ("keyboard_hold_title", Some("keyboard_hold_help"))]),
    (Page::Harnesses, &[("harness_legend", None)]),
    (Page::Voice, &[("voice_transcribe", Some("voice_transcribe_help")), ("voice_groq", Some("voice_groq_help")),
        ("voice_transcription_endpoint", Some("voice_transcription_endpoint_help")), ("voice_transcription_model", Some("voice_transcription_model_help")),
        ("voice_hands_free", Some("voice_hands_free_help")), ("voice_style", Some("voice_style_help")), ("voice_vocabulary", Some("voice_vocabulary_help")),
        ("voice_cleanup", Some("voice_cleanup_help")), ("voice_llm_endpoint", Some("voice_llm_endpoint_help")), ("voice_llm_key", Some("voice_llm_key_help")),
        ("voice_llm_model", Some("voice_llm_model_help")), ("voice_llm_effort", Some("voice_llm_effort_help")),
        ("voice_briefing_endpoint", Some("voice_briefing_endpoint_help")), ("voice_briefing_key", Some("voice_briefing_key_help")),
        ("voice_briefing_model", Some("voice_briefing_model_help")), ("voice_read", Some("voice_read_help")),
        ("voice_elevenlabs_key", Some("voice_elevenlabs_key_help")), ("voice_voice", Some("voice_voice_help")),
        ("voice_tune_stability", Some("voice_tune_stability_help")), ("voice_tune_similarity", Some("voice_tune_similarity_help")),
        ("voice_tune_style", Some("voice_tune_style_help")), ("voice_tune_speed", Some("voice_tune_speed_help")),
        ("voice_local_cmd", Some("voice_local_cmd_help")), ("voice_max_chars", Some("voice_max_chars_help"))]),
    (Page::Notifications, &[("server_notify_finished", Some("server_notify_finished_help")), ("server_short_turn", Some("server_short_turn_help")),
        ("server_notify_dead", Some("server_notify_dead_help")), ("server_stall", Some("server_stall_help")), ("server_quiet", Some("server_quiet_why"))]),
    (Page::Attachments, &[("server_keep_attachments", Some("server_keep_attachments_help"))]),
    (Page::Windows, &[("computer_control_enable", Some("computer_control_enable_hint")),
        ("computer_control_install", None), ("computer_control_dir", None), ("computer_control_target", Some("computer_control_target_hint"))]),
    (Page::Advanced, &[("server_automations", Some("server_automations_help")), ("server_thinking", Some("server_thinking_help")),
        ("server_translate_thinking", Some("server_translate_thinking_help")), ("server_editor", Some("server_editor_help")),
        ("server_roots", Some("server_roots_help")), ("server_machine_only", Some("server_machine_only_help")), ("server_env", Some("server_env_help"))]),
    (Page::Jev, &[("server_jev_key", Some("server_jev_key_help")), ("server_jev_default", Some("server_jev_default_help")),
        ("server_jev_endpoint", Some("server_jev_endpoint_help")), ("server_jev_model", Some("server_jev_model_help")),
        ("server_jev_text_endpoint", Some("server_jev_text_endpoint_help")), ("server_jev_text_key", Some("server_jev_text_key_help")),
        ("server_jev_text_model", Some("server_jev_text_model_help")), ("server_jev_cmd", Some("server_jev_cmd_help")),
        ("server_jev_windows_key", Some("server_jev_windows_key_help"))]),
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
        // Sem bandeja no sistema a linha não é desenhada.
        .filter(|(title, _)| crate::tray::SUPPORTED || *title != "settings_tray")
        .filter(|(title, _)| cfg!(target_os = "linux") || *title != "settings_chrome_autofill")
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
enum Knob { TintStrength, Transparency, Solidity, SheetSolidity, Contrast, Line, Column }

impl Knob {
    const ALL: [Knob; 7] = [Knob::TintStrength, Knob::Transparency, Knob::Solidity, Knob::SheetSolidity, Knob::Contrast, Knob::Line, Knob::Column];

    // A força da tinta é do modo que está na tela (escuro ou claro), como a cor.
    fn read(self, a: &Appearance) -> u16 {
        match self { Knob::TintStrength => a.colors(theme::is_dark()).tint_strength, Knob::Transparency => a.transparency, Knob::Solidity => a.solidity,
            Knob::SheetSolidity => a.sheet_solidity, Knob::Contrast => a.text_contrast, Knob::Line => a.line_height, Knob::Column => a.column }
    }

    fn write(self, a: &mut Appearance, value: u16) {
        match self { Knob::TintStrength => a.colors_mut(theme::is_dark()).tint_strength = value, Knob::Transparency => a.transparency = value,
            Knob::Solidity => a.solidity = value, Knob::SheetSolidity => a.sheet_solidity = value, Knob::Contrast => a.text_contrast = value,
            Knob::Line => a.line_height = value, Knob::Column => a.column = value }
    }

    fn range(self) -> (f32, f32) {
        match self {
            Knob::TintStrength => (5., 100.),
            Knob::Transparency | Knob::Solidity | Knob::SheetSolidity | Knob::Contrast => (0., 100.),
            Knob::Line | Knob::Column => (50., 150.),
        }
    }
}

/// As três áreas com fonte e tamanho escolhidos em listas, como no Zeron.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Area { Text, Code, Terminal }

impl Area {
    const ALL: [Area; 3] = [Area::Text, Area::Code, Area::Terminal];

    /// Tamanhos oferecidos, como o arquivo os grava: texto em % de 14 px, código em meio pixel, terminal em px.
    fn sizes(self) -> Vec<u16> {
        match self {
            Area::Text => (9..=20).map(|px| (px as f32 * 100. / 14.).round() as u16).collect(),
            Area::Code => (20..=36).collect(),
            Area::Terminal => (8..=24).collect(),
        }
    }

    fn px(self, stored: u16) -> f32 {
        match self { Area::Text => 14. * stored as f32 / 100., Area::Code => stored as f32 / 2., Area::Terminal => stored as f32 }
    }

    /// O tamanho digitado em "Personalizado", preso na faixa que o arquivo aceita (`Appearance::clamped`).
    fn stored(self, px: f32) -> u16 {
        match self {
            Area::Text => (px.clamp(7., 21.) * 100. / 14.).round() as u16,
            Area::Code => (px.clamp(8., 24.) * 2.).round() as u16,
            Area::Terminal => px.clamp(8., 24.).round() as u16,
        }
    }

    fn size(self, a: &Appearance) -> u16 {
        match self { Area::Text => a.text_size, Area::Code => a.code_size, Area::Terminal => a.terminal_size }
    }

    fn set_size(self, a: &mut Appearance, value: u16) {
        match self { Area::Text => a.text_size = value, Area::Code => a.code_size = value, Area::Terminal => a.terminal_size = value }
    }

    fn set_font(self, a: &mut Appearance, family: &str) {
        match self {
            Area::Text => a.font = Font::from_family(family),
            Area::Code => a.code_font = CodeFont::from_family(family),
            Area::Terminal => a.terminal_font = CodeFont::from_family(family),
        }
    }

    /// A família que o desenho usa hoje, com "Sistema" já resolvida.
    fn family(self, a: &Appearance, window: &Window, cx: &App) -> SharedString {
        match self {
            Area::Text => a.font.family().into(),
            Area::Code => match a.code_font {
                CodeFont::JetBrainsMono => theme::CODE_MONO.into(),
                CodeFont::System => theme::original_code_typography(cx).0,
                CodeFont::Named(name) => name.0.into(),
            },
            Area::Terminal => crate::term_view::terminal_font(window).family,
        }
    }

    /// Fontes instaladas com a de agora marcada; a de agora entra no topo se não estiver instalada.
    fn font_items(self, fonts: &[SharedString], window: &Window, cx: &App) -> (SearchableVec<SharedString>, IndexPath) {
        let current = self.family(&appearance::get(), window, cx);
        let mut items = fonts.to_vec();
        let at = items.iter().position(|f| *f == current).unwrap_or_else(|| { items.insert(0, current); 0 });
        (SearchableVec::new(items), IndexPath::new(at))
    }

    /// Lista de tamanhos e, no fim, "Personalizado": marcado quando o gravado não está na lista (aí leva o valor no
    /// rótulo) ou quando a pessoa acabou de escolhê-lo.
    fn size_items(self, custom_open: bool) -> (Vec<SizeChoice>, IndexPath) {
        let (sizes, stored) = (self.sizes(), self.size(&appearance::get()));
        let exact = sizes.iter().position(|s| *s == stored);
        let custom = SizeChoice { label: match exact { Some(_) => tr("settings_size_custom"), None => px_label(self.px(stored)) }.into(), value: CUSTOM_SIZE };
        let at = exact.filter(|_| !custom_open).unwrap_or(sizes.len());
        let mut items: Vec<SizeChoice> = sizes.into_iter().map(|value| SizeChoice { label: px_label(self.px(value)).into(), value }).collect();
        items.push(custom);
        (items, IndexPath::new(at))
    }

    /// O campo de "Personalizado" aparece com ele escolhido ou com um valor gravado fora da lista.
    fn custom_shown(self, custom_open: bool) -> bool {
        custom_open || !self.sizes().contains(&self.size(&appearance::get()))
    }
}

/// "12,5 px": meio pixel é o passo mais fino que as listas oferecem.
fn px_label(px: f32) -> String { format!("{} px", px_number(px)) }

fn px_number(px: f32) -> String { ((px * 2.).round() / 2.).to_string().replace('.', &tr("decimal")) }

/// Valor do item "Personalizado": nenhum tamanho gravado vale 0.
const CUSTOM_SIZE: u16 = 0;

#[derive(Clone)]
struct SizeChoice { label: SharedString, value: u16 }

impl SearchableListItem for SizeChoice {
    type Value = u16;
    fn title(&self) -> SharedString { self.label.clone() }
    fn value(&self) -> &u16 { &self.value }
}

type FontPick = Entity<SelectState<SearchableVec<SharedString>>>;
type SizePick = Entity<SelectState<Vec<SizeChoice>>>;

/// Estado vivo da página: os controles deslizantes guardam posição e arrasto entre desenhos.
pub(super) struct SettingsUi {
    sliders: Vec<(Knob, Entity<SliderState>)>,
    /// Fontes instaladas, lidas uma vez ao abrir o app.
    fonts: Vec<SharedString>,
    type_picks: Vec<(Area, FontPick, SizePick)>,
    /// Campo do tamanho personalizado de cada área e as áreas em que ele foi escolhido na lista.
    custom_sizes: Vec<(Area, Entity<InputState>)>,
    custom_open: Vec<Area>,
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
    /// Seção da Aparência levada pelo atalho: rola até ela como a busca, sem o destaque.
    jump: Option<&'static str>,
    /// Seção da Aparência que a prévia ao lado mostra: a do último atalho ou clique.
    preview: Option<&'static str>,
    /// A Aparência numa caixa sobre a conversa, em vez da página.
    pub live: bool,
    /// Arrasto da caixa: ponto onde começou e o canto que ela tinha.
    drag: Option<(Point<Pixels>, [f32; 2])>,
    /// Onde ligar o desfoque, aberto pelo botão da linha (o tooltip não chega pelo teclado).
    blur_hint: bool,
    _subscriptions: Vec<Subscription>,
    _disk_watch: Task<()>,
}

/// Aparência gravada por fora com o app aberto (configuração compartilhada): sem reler, o próximo ajuste aqui
/// gravaria por cima o valor antigo da memória.
fn watch_disk(window: &mut Window, cx: &mut Context<Hangar>) -> Task<()> {
    cx.spawn_in(window, async move |this, cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let read = cx.background_executor().spawn(async { appearance::changed_on_disk().then(appearance::load) }).await;
            let Some(read) = read else { continue };
            if this.update_in(cx, |this, window, cx| this.receive_disk_appearance(read, window, cx)).is_err() { break; }
        }
    })
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
        let fonts: Vec<SharedString> = window.text_system().all_font_names().into_iter().map(SharedString::from).collect();
        let mut type_picks = Vec::new();
        let mut custom_sizes = Vec::new();
        for area in Area::ALL {
            let (items, at) = area.font_items(&fonts, window, cx);
            let font = cx.new(|cx| SelectState::new(items, Some(at), window, cx).searchable(true));
            subscriptions.push(cx.subscribe_in(&font, window, move |this: &mut Hangar, _, event: &SelectEvent<SearchableVec<SharedString>>, _, cx| {
                let SelectEvent::Confirm(Some(family)) = event else { return };
                let mut next = appearance::get();
                area.set_font(&mut next, family);
                this.apply_appearance(next, true, cx);
            }));
            let (items, at) = area.size_items(false);
            let size = cx.new(|cx| SelectState::new(items, Some(at), window, cx));
            let custom = cx.new(|cx| InputState::new(window, cx).placeholder("px")
                .default_value(px_number(area.px(area.size(&appearance::get())))));
            subscriptions.push(cx.subscribe_in(&size, window, move |this: &mut Hangar, _, event: &SelectEvent<Vec<SizeChoice>>, window, cx| {
                let SelectEvent::Confirm(Some(value)) = event else { return };
                this.settings_ui.custom_open.retain(|a| *a != area);
                if *value == CUSTOM_SIZE {
                    this.settings_ui.custom_open.push(area);
                    if let Some((_, input)) = this.settings_ui.custom_sizes.iter().find(|(a, _)| *a == area) {
                        input.update(cx, |input, cx| input.focus(window, cx));
                    }
                    cx.notify();
                    return;
                }
                let mut next = appearance::get();
                area.set_size(&mut next, *value);
                this.apply_appearance(next, true, cx);
                this.sync_type_picks(window, cx);
            }));
            subscriptions.push(cx.subscribe_in(&custom, window, move |this: &mut Hangar, input, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) { return; }
                let typed = input.read(cx).value().trim().replace(',', ".");
                // O gravado é mais fino que o meio pixel à vista: sair do campo sem mudar o número não pode arredondá-lo.
                let shown = (area.px(area.size(&appearance::get())) * 2.).round() / 2.;
                if let Some(px) = typed.trim_end_matches("px").trim().parse::<f32>().ok().filter(|px| px.is_finite() && *px != shown) {
                    let mut next = appearance::get();
                    area.set_size(&mut next, area.stored(px));
                    this.apply_appearance(next, true, cx);
                }
                // Valor que caiu num da lista volta a aparecer como item dela; fora dela o campo continua à vista.
                this.settings_ui.custom_open.retain(|a| *a != area);
                this.sync_type_picks(window, cx);
            }));
            type_picks.push((area, font, size));
            custom_sizes.push((area, custom));
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
        Self { sliders, fonts, type_picks, custom_sizes, custom_open: Vec::new(),
            accent_picker, tint_picker, search, found: Vec::new(), pick: 0, hit: None, scroll: ScrollHandle::new(),
            reveal: Rc::new(Cell::new(false)), jump: None, preview: None, live: false, drag: None, blur_hint: false, _subscriptions: subscriptions,
            _disk_watch: watch_disk(window, cx) }
    }

    fn slider(&self, knob: Knob) -> &Entity<SliderState> {
        &self.sliders.iter().find(|(k, _)| *k == knob).expect("every knob has a slider").1
    }
}

fn swatch_ring(selected: bool) -> Hsla { if selected { theme::text() } else { transparent_black() } }

impl Hangar {
    pub(super) fn open_settings(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_session_numbers(cx);
        self.keyboard.cancel_edit();
        self.costs.view = None;
        (self.worktrees.view, self.worktrees.open) = (None, None);
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
        self.close_recent();
        self.settings_opened(page, cx);
        cx.notify();
    }

    /// Idioma trocado: o texto guardado no campo de busca acompanha.
    pub(super) fn relabel_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_ui.search.update(cx, |input, cx| input.set_placeholder(tr("settings_search"), window, cx));
        self.sync_type_picks(window, cx);
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            self.keyboard.cancel_edit();
            self.accounts_page_left();
            self.sync_page_left();
            self.connect_page_left();
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

    /// Ctrl+F com a página aberta leva ao campo de busca dela; fora dela, busca na conversa aberta.
    pub(super) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection_dialog || window.has_active_dialog(cx) { return; }
        if self.find_in_file(window, cx) { return; }
        if self.settings.is_none() || self.settings_ui.live { self.open_find(window, cx); return; }
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
            // Linha dentro de seção fechada da Voz: a seção abre, senão não há o que rolar até ela.
            Some(row) => { ui.reveal.set(true); self.server_config.reveal(row); }
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
        if (before.tool_look, before.task_list, before.thinking_tools, before.table_chart, before.code_font, before.code_size)
            != (next.tool_look, next.task_list, next.thinking_tools, next.table_chart, next.code_font, next.code_size) {
            self.chat.invalidate();
            self.sync_rows(cx);
            self.list_state.remeasure();
            self.restyle_subagent(cx);
        }
        if before.keep_in_tray != next.keep_in_tray { self.sync_tray(cx); }
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

    fn receive_disk_appearance(&mut self, read: Result<Appearance, String>, window: &mut Window, cx: &mut Context<Self>) {
        let next = match read {
            Ok(next) => next,
            Err(reason) => return window.push_notification(Notification::warning(tr("appearance_disk_failed").replace("{reason}", &reason)), cx),
        };
        let before = appearance::get();
        self.apply_appearance(next, false, cx);
        if next.language != before.language { self.set_language(next.language, window, cx); }
        self.sync_sliders(window, cx);
        // A imagem pode ter mudado sem mudar a aparência: a configuração compartilhada só toca o arquivo.
        self.refresh_backdrop(window, cx);
    }

    fn reset_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = appearance::get().reset_keeping_choices();
        self.apply_appearance(next, true, cx);
        self.sync_sliders(window, cx);
    }

    /// Traz do app Electron a aparência e o servidor ativo. Das Configurações pergunta antes: tudo o que tem par é trocado.
    pub(super) fn import_electron(&mut self, ask: bool, window: &mut Window, cx: &mut Context<Self>) {
        if ask {
            let weak = cx.entity().downgrade();
            chrome::confirm_alert(window, cx, tr("electron_import_title"), tr("electron_import_desc"), tr("electron_import_ok"),
                ButtonVariant::Primary, move |window, cx| { let _ = weak.update(cx, |this, cx| this.import_electron(false, window, cx)); true });
            return;
        }
        let base = appearance::get();
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || { let _ = done.send(crate::electron::load(base).and_then(crate::electron::save_image)); });
        cx.spawn_in(window, async move |this, cx| {
            let result = result.await.unwrap_or_else(|_| Err(crate::electron::Failure::Read(tr("electron_import_stopped"))));
            let _ = this.update_in(cx, |this, window, cx| this.receive_electron(result, window, cx));
        }).detach();
    }

    fn receive_electron(&mut self, result: Result<crate::electron::Imported, crate::electron::Failure>, window: &mut Window, cx: &mut Context<Self>) {
        let imported = match result {
            Ok(imported) => imported,
            Err(failure) => {
                let note = match failure {
                    crate::electron::Failure::Missing => tr("electron_import_missing"),
                    crate::electron::Failure::Read(reason) => tr("electron_import_failed").replace("{reason}", &reason),
                };
                if self.connection_dialog { self.error = Some(note.clone()); }
                window.push_notification(Notification::warning(note), cx);
                cx.notify();
                return;
            }
        };
        let before = appearance::get();
        let next = imported.appearance;
        let mut what: Vec<String> = crate::electron::changed(&before, &next, imported.image.is_some()).into_iter().map(tr).collect();
        self.apply_appearance(next, true, cx);
        if next.language != before.language { self.set_language(next.language, window, cx); }
        self.sync_sliders(window, cx);
        self.refresh_backdrop(window, cx);
        self.electron_offer = false;
        let before_servers = self.servers.len();
        self.merge_servers(imported.servers, cx);
        if self.servers.len() > before_servers {
            what.push(tr("electron_import_servers").replace("{n}", &(self.servers.len() - before_servers).to_string()));
        }
        if let Some((address, token)) = imported.server {
            let same = self.active_token == token
                && self.api.as_ref().is_some_and(|api| api.identity().trim_end_matches('/') == address.trim_end_matches('/'));
            if !same {
                what.push(tr("electron_import_server").replace("{address}", address.trim_start_matches("http://").trim_start_matches("https://")));
                self.address.update(cx, |input, cx| input.set_value(address, window, cx));
                self.token.update(cx, |input, cx| input.set_value(token, window, cx));
                self.connect(window, cx);
                if !self.connection_dialog { self.forget_question(); window.close_all_dialogs(cx); }
            }
        }
        let note = if what.is_empty() { tr("electron_import_same") } else { tr("electron_import_done").replace("{what}", &what.join(", ")) };
        window.push_notification(Notification::success(note), cx);
        cx.notify();
    }

    /// Recoloca os controles deslizantes no valor salvo: depois do reset ou quando o modo escuro/claro muda
    /// (a força da tinta é de cada modo).
    pub(super) fn sync_sliders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = appearance::get();
        let shown = self.backdrop.as_ref().is_some_and(|(_, image)| crate::effects::shows(image, current.background_effect, !theme::is_dark()));
        if current.background == Background::Image && (!shown || self.backdrop_pending) { self.refresh_backdrop(window, cx); }
        for (knob, state) in self.settings_ui.sliders.clone() {
            state.update(cx, |slider, cx| slider.set_value(knob.read(&current) as f32, window, cx));
        }
        self.sync_type_picks(window, cx);
        // Os seletores de cor livre mostram a do modo na tela.
        let colors = *current.colors(theme::is_dark());
        for (state, saved) in [(self.settings_ui.accent_picker.clone(), colors.accent), (self.settings_ui.tint_picker.clone(), colors.tint)] {
            state.update(cx, |picker, cx| match saved {
                Swatch::Custom(Hex(hex)) => picker.set_value(rgb(hex), window, cx),
                Swatch::Preset(_) => picker.clear_value(window, cx),
            });
        }
    }

    /// Listas de fonte e tamanho no valor gravado, com o rótulo no idioma da tela ("12,5 px" / "12.5 px").
    fn sync_type_picks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (area, font, size) in self.settings_ui.type_picks.clone() {
            let (items, at) = area.font_items(&self.settings_ui.fonts, window, cx);
            font.update(cx, |select, cx| { select.set_items(items, window, cx); select.set_selected_index(Some(at), window, cx); });
            let (items, at) = area.size_items(self.settings_ui.custom_open.contains(&area));
            size.update(cx, |select, cx| { select.set_items(items, window, cx); select.set_selected_index(Some(at), window, cx); });
        }
        for (area, input) in self.settings_ui.custom_sizes.clone() {
            let text = px_number(area.px(area.size(&appearance::get())));
            input.update(cx, |input, cx| input.set_value(text, window, cx));
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

    /// Máquina que o grupo Servidor configura; troca como o seletor do web (convite e desligada ficam fora).
    fn render_server_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = servers::norm(&self.address.read(cx).value());
        let eligible: Vec<ServerEntry> = self.servers.iter().filter(|s| !s.invite && !s.disabled).cloned().collect();
        let active = self.servers.iter().find(|s| servers::norm(&s.address) == current);
        let label = active.map(|s| s.label.clone()).unwrap_or_else(|| self.server_label(cx));
        let dot = |id: &str| div().size(px(7.)).flex_shrink_0().rounded_full().bg(theme::server_color(id));
        let text = div().min_w_0().truncate().text_size(px(12.5)).text_color(theme::text()).child(label);
        if !eligible.iter().any(|s| servers::norm(&s.address) != current) {
            return div().ml_auto().max_w(px(140.)).flex().items_center().gap(px(6.))
                .children(active.map(|s| dot(&s.id))).child(text).into_any_element();
        }
        let weak = cx.entity().downgrade();
        Button::new("settings-server-picker").ghost().xsmall().ml_auto().max_w(px(160.))
            .accessibility_label(tr("settings_switch_server")).tooltip(tr("settings_switch_server"))
            .child(div().min_w_0().flex().items_center().gap(px(6.))
                .children(active.map(|s| dot(&s.id))).child(text)
                .child(chrome::small_icon(IconName::ChevronDown, 12., theme::muted())))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let mut menu = sidebar::menu_style(menu).min_w(px(220.)).label(tr("settings_group_server"));
                for entry in eligible.clone() {
                    let (weak, on, id, name) = (weak.clone(), servers::norm(&entry.address) == current, entry.id.clone(), entry.label.clone());
                    menu = menu.item(PopupMenuItem::element(move |_, _| div().w_full().flex().items_center().gap(px(8.))
                            .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(theme::server_color(&id)))
                            .child(div().flex_1().min_w_0().truncate().child(name.clone())))
                        .checked(on)
                        .on_click(move |_, window, cx| { let _ = weak.update(cx, |this, cx| this.activate_server(entry.clone(), window, cx)); }));
                }
                menu
            })
            .into_any_element()
    }

    pub(super) fn render_settings(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // `fade-quick` do kit: a tela ao abrir e o corpo a cada página.
        let shown = motion::enter("settings-screen", motion::FADE_QUICK, window, cx);
        let body_in = motion::enter(SharedString::from(format!("settings-page-{}", page.key())), motion::FADE_QUICK, window, cx);
        let floating = theme::is_floating();
        // Na caixa solta o destaque suave some sobre o vidro; o item da página aberta leva mais cor.
        let selected = if floating { theme::accent().alpha(0.26) } else { theme::selected_row() };
        let nav_item = |page_item: Page, current: Page, cx: &mut Context<Self>| {
            let on = page_item == current;
            Button::new(SharedString::from(format!("settings-nav-{}", page_item.key())))
                .role(Role::Tab).aria_selected(on).accessibility_id(format!("settings-tab-{}", page_item.key()))
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
        let server = self.render_server_picker(cx);
        let nav =div().w(px(284.)).flex_shrink_0().h_full().flex().flex_col().px(px(10.)).bg(theme::chrome())
            .map(|el| if floating { el.rounded(px(theme::PANEL_RADIUS)).border_1().border_color(theme::border()).shadow(theme::panel_shadow()) }
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
                div().id("settings-nav").role(Role::TabList).aria_label(tr("settings")).flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(2.))
                    .child(group(tr("settings_group_device")))
                    .children(Page::DEVICE.map(|p| nav_item(p, page, cx)))
                    .child(group(tr("settings_group_server")).child(server))
                    .children(Page::SERVER.map(|p| nav_item(p, page, cx)))
            })
            .child(div().h(px(48.)).flex_shrink_0().mx(px(-10.)).px(px(10.)).border_t_1().border_color(theme::border()).flex().items_center()
                .child(Button::new("settings-back").ghost().w_full().h(px(32.))
                    .child(div().w_full().flex().items_center().gap_2()
                        .child(chrome::small_icon(IconName::ArrowLeft, 16., theme::muted()))
                        .child(div().flex_1().text_sm().text_color(theme::muted()).child(tr("settings_back")))
                        .child(chrome::kbd("Esc")))
                    .on_click(cx.listener(|this, _, window, cx| this.close_settings(window, cx)))));
        // Com espaço para a coluna da prévia ao lado do conteúdo de 720px; a caixa ao vivo nunca tem.
        let wide = page == Page::Appearance && !self.settings_ui.live && window.viewport_size().width >= px(1400.);
        // Contas cabe em colunas (uma por janela de cota) quando sobra largura ao lado da navegação.
        let accounts_wide = page == Page::Accounts && window.viewport_size().width >= px(1320.);
        // Servidores põe o detalhe num painel ao lado da lista: usa a largura que houver.
        let servers_wide = page == Page::Servers;
        let body = match page {
            Page::Appearance => self.render_appearance(wide, cx),
            Page::General => self.render_general(cx),
            Page::Diary => self.render_diary(cx),
            Page::About => self.render_about(cx),
            Page::Migration => self.render_migration(cx),
            Page::Accounts => self.render_accounts(accounts_wide, window, cx),
            Page::Orchestration => self.render_orchestration(cx),
            Page::Shortcuts => self.render_shortcuts_page(cx),
            Page::Harnesses => self.render_harness(cx),
            Page::Voice | Page::Jev | Page::Notifications | Page::Attachments | Page::Advanced => self.render_server_page(page, cx),
            Page::Servers => self.render_machines(window.viewport_size().width, cx),
            Page::Sync => self.render_sync(cx),
            Page::Connect => self.render_connect(cx),
            Page::SharedConfig => self.render_shared_config(cx),
            Page::Windows => self.render_computer(cx),
        };
        let scroll = div().id("settings-content").role(Role::TabPanel).aria_label(page.title())
            .accessibility_id(format!("settings-page-{}", page.key())).flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.settings_ui.scroll)
            .child(div().w_full().flex().justify_center().child(motion::fade_quick(div().w(px(if accounts_wide || servers_wide { 1040. } else { 720. })).max_w_full().px_4().pt(px(44.)).pb(px(40.)), body_in).child(body)));
        let tabs = (page == Page::Appearance).then(|| div().w_full().flex_shrink_0().flex().justify_center().px_4().pt(px(14.)).pb(px(6.))
            .child(div().w(px(720.)).max_w_full().child(self.section_tabs(cx))));
        let content = div().flex_1().min_w_0().h_full().flex().flex_col().children(tabs).child(scroll).children(self.server_config_footer(page, cx));
        // A prévia fica fora da rolagem: continua à vista enquanto a página desce.
        let content = if wide {
            div().flex_1().min_w_0().h_full().flex().child(content)
                .child(div().id("appearance-aside").w(px(360.)).flex_shrink_0().h_full().overflow_y_scroll().pt(px(44.)).pr(px(24.)).pb(px(40.))
                    .child(self.render_appearance_aside(cx)))
        } else { content };
        div().id("settings-dialog").role(Role::Dialog).aria_label(tr("settings")).size_full().flex().opacity(shown).when(floating, |el| el.p(px(10.)).gap(px(10.))).child(nav).child(content).into_any_element()
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

    pub(super) fn search_hit(&self, key: &str) -> bool { self.settings_ui.hit == Some(key) }

    /// Atalho de seção de outras páginas: rola até a linha marcada com `key`.
    pub(super) fn jump_to(&mut self, key: &'static str) {
        self.settings_ui.hit = None;
        self.settings_ui.jump = Some(key);
        self.settings_ui.reveal.set(true);
    }

    pub(super) fn jumped(&self) -> Option<&'static str> { self.settings_ui.jump }

    /// Destaque da linha levada pela busca, e a rolagem até ela quando o desenho já sabe onde ela está.
    pub(super) fn mark(&self, el: Div, key: &str) -> Div {
        let hit = self.settings_ui.hit == Some(key);
        // O atalho de seção só vale sem busca: os dois disputariam o mesmo pedido de rolagem.
        let jump = self.settings_ui.hit.is_none() && self.settings_ui.jump == Some(key);
        if !hit && !jump { return el; }
        let (scroll, reveal) = (self.settings_ui.scroll.clone(), self.settings_ui.reveal.clone());
        el.relative().when(hit, |el| el.bg(theme::accent_dim())).child(canvas(move |bounds, window, _| {
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
        // O `dialog-in` do kit: aparece subindo 2 px até o canto gravado.
        let shown = motion::enter("live-box-in", motion::DIALOG_IN, window, cx);
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
        let body = self.render_appearance(false, cx);
        div().id("live-box").absolute().right(px(right)).bottom(px(bottom - 2. * (1. - shown))).opacity(shown).w(px(LIVE_WIDTH)).max_h(px(height))
            // Opaca mesmo na caixa solta: a conversa atrás não pode atravessar as linhas.
            .flex().flex_col().rounded(px(14.)).border_1().border_color(theme::border_strong()).bg(theme::raised())
            .shadow(theme::popover_shadow()).overflow_hidden().occlude()
            // O ponteiro fica sobre a própria caixa, que tapa a raiz: o arrasto e o fim dele moram aqui também.
            .when(self.live_dragging(), |el| el.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.drag_live(event.position, event.pressed_button == Some(MouseButton::Left), window, cx);
            })))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, window, cx| this.drag_live(event.position, false, window, cx)))
            .child(header)
            .child(div().flex_shrink_0().px(px(12.)).pt(px(10.)).child(self.section_tabs(cx)))
            .child(div().id("live-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.settings_ui.scroll)
                .px(px(12.)).pb(px(14.)).child(body))
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
}

pub(super) fn settings_box() -> Div {
    div().flex().flex_col().rounded(px(14.)).border_1().border_color(theme::border()).bg(theme::boxed()).overflow_hidden()
}

/// Cabeçalho de seção das Configurações: ícone no quadradinho de destaque, título e a linha que explica. É ele que faz as
/// páginas parecerem da mesma família; seção nova usa este, não um desenho próprio.
pub(super) fn section_head(icon: IconName, title: String, subtitle: Option<String>, extra: Option<AnyElement>, inset: Pixels) -> Div {
    div().flex().items_center().gap(px(12.)).px(inset).py(px(14.))
        .child(div().size(px(32.)).flex_shrink_0().rounded(px(9.)).bg(theme::accent_dim()).flex().items_center().justify_center()
            .child(chrome::small_icon(icon, 17., theme::accent_text())))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .when_some(subtitle, |el, s| el.child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(s))))
        .children(extra)
}

/// Botão que abre e fecha um trecho (seção da Voz, "por quê?"): o `AccordionTrigger` do kit anuncia aberto/fechado, que o
/// `Button` não expõe, e o foco próprio — guardado pelo kit entre desenhos, como o do `Button` — o põe no Tab com Enter e Espaço.
#[derive(IntoElement)]
pub(super) struct Disclosure {
    id: SharedString,
    open: bool,
    label: String,
    /// O "por quê?" (20px, texto miúdo); sem isso, o tamanho do disparador de seção.
    small: bool,
    name: Option<String>,
    on_change: Option<Rc<dyn Fn(bool, &mut App)>>,
}

impl Disclosure {
    pub(super) fn new(id: impl Into<SharedString>, open: bool, label: String, small: bool) -> Self {
        Self { id: id.into(), open, label, small, name: None, on_change: None }
    }

    /// Nome acessível quando o rótulo sozinho é ambíguo ("Por que: Automações").
    pub(super) fn name(mut self, name: String) -> Self { self.name = Some(name); self }

    /// Recebe o estado pedido: o contrário do atual.
    pub(super) fn on_change(mut self, handler: impl Fn(bool, &mut App) + 'static) -> Self { self.on_change = Some(Rc::new(handler)); self }
}

impl RenderOnce for Disclosure {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self { id, open, label, small, name, on_change } = self;
        let focus = window.use_keyed_state(SharedString::from(format!("{id}-focus")), cx, |_, cx| cx.focus_handle().tab_stop(true)).read(cx).clone();
        AccordionTrigger::new(id).open(open).track_focus(&focus)
            .when_some(name, |el, name| el.aria_label(name))
            .flex().items_center().rounded(px(6.)).border_1().border_color(transparent_black()).text_color(theme::text()).cursor_pointer()
            .map(|el| if small { el.gap(px(4.)).h(px(20.)).px(px(4.)).text_size(px(12.)) }
                else { el.gap(px(6.)).h(px(28.)).px(px(8.)).text_sm().font_weight(FontWeight::MEDIUM) })
            .hover(|el| el.bg(theme::hover())).focus_visible(|el| el.border_color(theme::accent_focus()))
            .child(chrome::small_icon(if open { IconName::ChevronUp } else { IconName::ChevronDown }, if small { 12. } else { 14. },
                if small { theme::text() } else { theme::muted() }))
            .child(label)
            .when_some(on_change, |el, handler| el.on_change(move |open, _, _, cx| handler(open, cx)))
    }
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
    segments_with_hints(id, labels, &[], selected, available, locked, off_note, pick, cx)
}

/// `hints`: a explicação de cada opção, no tooltip e como nome acessível dela (o `aria` do `SegmentedPicker` do web).
#[allow(clippy::too_many_arguments)]
pub(super) fn segments_with_hints(id: &'static str, labels: &[String], hints: &[String], selected: usize, available: usize, locked: bool,
    off_note: String, pick: impl Fn(&mut Hangar, usize, &mut Window, &mut Context<Hangar>) + Clone + 'static, cx: &mut Context<Hangar>)
    -> AnyElement {
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
                .when_some(hints.get(n).filter(|_| enabled), |el, hint| el.tooltip(hint.clone()).accessibility_label(hint.clone()))
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

    #[test]
    fn general_search_finds_the_tray_option() {
        let (_, rows) = super::PAGE_ROWS.iter().find(|(page, _)| *page == super::Page::General).expect("Geral na busca");
        assert!(rows.iter().any(|(row, _)| *row == "settings_tray"), "settings_tray fora da busca");
    }

    #[test]
    fn jev_search_finds_every_jev_field() {
        let (_, rows) = super::PAGE_ROWS.iter().find(|(page, _)| *page == super::Page::Jev).expect("Jev na busca");
        for label in ["server_jev_key", "server_jev_default", "server_jev_endpoint", "server_jev_model", "server_jev_text_endpoint", "server_jev_text_key", "server_jev_text_model", "server_jev_cmd", "server_jev_windows_key"] {
            assert!(rows.iter().any(|(row, _)| *row == label), "{label} fora da busca");
        }
    }
}
