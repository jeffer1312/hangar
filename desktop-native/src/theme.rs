use gpui_kit::{component::{Theme, ThemeMode as KitMode}, *};
use std::sync::{OnceLock, RwLock, atomic::{AtomicBool, Ordering}};
use crate::appearance::{self, Background as Backdrop, BackgroundScope, DesktopText, Palette, Panels, Reading, SurfaceMaterial, Swatch, ThemeMode, Wallpaper};

// Cores dos mocks aprovados (Task 12): o padrão é "Colados", opaco; "Caixa solta", ou colados sobre imagem ou área de
// trabalho "em tudo", deixa passar o que está atrás nas medidas de Transparência e Solidez. O nome de cada função diz o
// papel, não a cor.
/// `--font-mono` do web; sem a fonte instalada, o GPUI cai na padrão.
pub const MONO: &str = "JetBrainsMono Nerd Font";
/// Sans embutida no binário (`assets/fonts`), a "Sistema" das configurações.
pub const SANS: &str = "Geist";
pub const CODE_MONO: &str = "JetBrains Mono";

/// Guarda a escolha resolvida pelo kit antes de aplicar a preferência de código.
pub fn original_code_typography(cx: &App) -> (SharedString, Pixels) {
    static ORIGINAL: OnceLock<(SharedString, Pixels)> = OnceLock::new();
    ORIGINAL.get_or_init(|| {
        let theme = Theme::global(cx);
        (theme.mono_font_family.clone(), theme.mono_font_size)
    }).clone()
}

/// Um conjunto completo de cores: as quatro fixas (Clássico/Neutro × escuro/claro) e a do desktop.
/// `float_*` são as superfícies da caixa solta, que ficam sobre o fundo transparente.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub dark: bool,
    bg: u32, chrome: u32, boxed: u32, inset: u32, hover: u32, bubble: u32, selected: u32, elevated: u32, raised: u32,
    float_bg: u32, float_chrome: u32, float_boxed: u32, float_bubble: u32,
    text: u32, muted: u32, faint: u32, line: u32, line_strong: u32, accent: u32, on_accent: u32,
}

/// Clássico escuro: as cores aprovadas na R1.
const CLASSIC_DARK: Colors = Colors { dark: true,
    bg: 0x121013, chrome: 0x18151a, boxed: 0x1f1b20, inset: 0x0e0c0f, hover: 0x262127, bubble: 0x2a272c, selected: 0x2c262d, elevated: 0x2c262d, raised: 0x262127,
    float_bg: 0x0d0c12, float_chrome: 0x1a181d, float_boxed: 0x26242c, float_bubble: 0x343038,
    text: 0xe6e0e2, muted: 0xa39a9e, faint: 0x8a8186, line: 0xfff8f4, line_strong: 0xfff8f4, accent: 0x7c87e8, on_accent: 0xfdf8f9 };
/// Clássico claro: papel quente do `[data-theme="light"]` do web.
const CLASSIC_LIGHT: Colors = Colors { dark: false,
    bg: 0xfffdfa, chrome: 0xf6f3ee, boxed: 0xfffefc, inset: 0xf0ebe3, hover: 0xebe5dc, bubble: 0xebe7e1, selected: 0xe6e0d6, elevated: 0xfffefc, raised: 0xf0ebe3,
    float_bg: 0xf8f6f2, float_chrome: 0xfffdfa, float_boxed: 0xfffefc, float_bubble: 0xe8e5e0,
    text: 0x221d1b, muted: 0x5f564f, faint: 0x6f6660, line: 0x322823, line_strong: 0x322823, accent: 0x5b6ad0, on_accent: 0xfffdfa };
/// Neutro: cinzas sem matiz e destaque azul, do `[data-palette="neutro"]` do web.
const NEUTRAL_DARK: Colors = Colors { dark: true,
    bg: 0x171717, chrome: 0x1c1c1c, boxed: 0x222222, inset: 0x121212, hover: 0x272727, bubble: 0x262626, selected: 0x2a2a2a, elevated: 0x2a2a2a, raised: 0x242424,
    float_bg: 0x171717, float_chrome: 0x1c1c1c, float_boxed: 0x262626, float_bubble: 0x303030,
    text: 0xebebeb, muted: 0xa0a0a0, faint: 0x8a8a8a, line: 0xebebeb, line_strong: 0xebebeb, accent: 0x459bf7, on_accent: 0xffffff };
const NEUTRAL_LIGHT: Colors = Colors { dark: false,
    bg: 0xffffff, chrome: 0xf7f7f7, boxed: 0xffffff, inset: 0xf2f2f2, hover: 0xececec, bubble: 0xececec, selected: 0xe8e8e8, elevated: 0xffffff, raised: 0xefefef,
    float_bg: 0xf7f7f7, float_chrome: 0xffffff, float_boxed: 0xffffff, float_bubble: 0xe8e8e8,
    text: 0x2b2b2b, muted: 0x5e5e5e, faint: 0x6f6f6f, line: 0x2b2b2b, line_strong: 0x2b2b2b, accent: 0x0863c4, on_accent: 0xffffff };

/// Destaques oferecidos em Aparência › Cor. O primeiro é trocado pelo destaque da paleta; no claro os tons
/// escurecem para o texto em destaque ler sobre fundo claro.
pub const ACCENTS: [u32; 7] = [0x7c87e8, 0x9b7cf0, 0xf08a4b, 0xe9b93f, 0x3fbf6f, 0x3cc4d6, 0xe070b0];
const ACCENTS_LIGHT: [u32; 7] = [0x5b6ad0, 0x7a55d6, 0xc8622a, 0xa87a12, 0x238a4f, 0x1a8a9a, 0xb8457f];
/// Tintas de fundo; a primeira é "sem tinta" e é trocada pela cor base na amostra.
pub const TINTS: [u32; 4] = [0x18151a, 0x1d1a2e, 0x2a1a1a, 0x1a2a20];
const TINTS_LIGHT: [u32; 4] = [0xf6f3ee, 0xdfe2f7, 0xf6dfdc, 0xdcefe2];

static SYSTEM_DARK: AtomicBool = AtomicBool::new(true);
static DESKTOP: RwLock<Option<Colors>> = RwLock::new(None);

static BACKDROP_READY: AtomicBool = AtomicBool::new(false);

/// A imagem do fundo (arquivo escolhido ou papel de parede em Vidro) está carregada e pode ser desenhada.
pub fn set_backdrop_ready(ready: bool) { BACKDROP_READY.store(ready, Ordering::Relaxed); }

/// Preferência clara/escura do sistema, lida da janela (no Wayland vem do portal).
pub fn set_system_dark(dark: bool) { SYSTEM_DARK.store(dark, Ordering::Relaxed); }

/// Paleta do papel de parede já traduzida; `None` volta a desenhar como Automático.
pub fn set_desktop(colors: Option<Colors>) { *DESKTOP.write().unwrap_or_else(|e| e.into_inner()) = colors; }

fn desktop() -> Option<Colors> { *DESKTOP.read().unwrap_or_else(|e| e.into_inner()) }

fn fixed(palette: Palette, dark: bool) -> Colors {
    match (palette, dark) {
        (Palette::Classic, true) => CLASSIC_DARK, (Palette::Classic, false) => CLASSIC_LIGHT,
        (Palette::Neutral, true) => NEUTRAL_DARK, (Palette::Neutral, false) => NEUTRAL_LIGHT,
    }
}

/// Desktop sem paleta (fora do ar, outro servidor, sem rice) desenha como Automático.
fn desktop_active() -> Option<Colors> {
    let a = appearance::get();
    if a.theme != ThemeMode::Desktop { return None; }
    let mut colors = desktop()?;
    if a.desktop_text == DesktopText::App {
        let app = fixed(a.palette, colors.dark);
        (colors.text, colors.muted, colors.faint) = (app.text, app.muted, app.faint);
    }
    Some(colors)
}

pub fn is_dark() -> bool {
    match appearance::get().theme {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::Auto => SYSTEM_DARK.load(Ordering::Relaxed),
        ThemeMode::Desktop => desktop().map_or(SYSTEM_DARK.load(Ordering::Relaxed), |c| c.dark),
    }
}

/// Superfície, seleção, hover e divisória exclusivos da barra de conversas.
pub fn conversation_sidebar() -> (Hsla, Hsla, Hsla, Hsla) {
    let dark = is_dark();
    let colors = if dark { [0x181818, 0x2f2f2f, 0x313131, 0x242424] }
        else { [0xf7f7f7, 0xe3e3e3, 0xeaeaea, 0xd9d9d9] };
    if see_through() {
        // Seleção e realce cheios virariam retângulos chapados sobre o painel translúcido.
        let ink = if dark { 1. } else { 0. };
        return (hex(colors[0], panel_alpha()), hsla(0., 0., ink, 0.10), hsla(0., 0., ink, 0.06), rgb(colors[3]).into());
    }
    (rgb(colors[0]).into(), rgb(colors[1]).into(), rgb(colors[2]).into(), rgb(colors[3]).into())
}

/// Estados da barra de conversas sobre a superfície neutra nos dois temas.
pub fn conversation_status(state: &str) -> Hsla {
    let dark = is_dark();
    if state == "idle" { return faint(); }
    rgb(match state {
        "input" => if dark { 0x70b0ff } else { 0x155eae },
        "failed" | "problem" | "dead" => if dark { 0xff8585 } else { 0xb42323 },
        "queued" | "uncertain" => if dark { 0xfbbf24 } else { 0x874b00 },
        "limited" => if dark { 0xc98cff } else { 0x7433ad },
        _ => if dark { 0xa0a0a0 } else { 0x565656 },
    }).into()
}

/// O Desktop está pintando agora com a paleta do papel de parede (não caiu no Automático).
pub fn desktop_painting() -> bool { desktop_active().is_some() }

fn colors() -> Colors { desktop_active().unwrap_or_else(|| fixed(appearance::get().palette, is_dark())) }

/// Cores das miniaturas da página de Aparência: (barra lateral, conversa, linhas de texto).
pub fn thumbnail(palette: Palette, dark: bool) -> (Hsla, Hsla, Hsla) {
    let c = fixed(palette, dark);
    (rgb(c.chrome).into(), rgb(c.bg).into(), hex(c.faint, 0.55))
}

/// Miniatura do Desktop: a paleta real quando já chegou; sem ela, um violeta de papel de parede genérico.
pub fn desktop_thumbnail() -> (Hsla, Hsla, Hsla) {
    match desktop() {
        Some(c) => (rgb(c.chrome).into(), rgb(c.bg).into(), hex(c.faint, 0.55)),
        None => (rgb(0x2a2346).into(), rgba(0x0a080e80).into(), rgba(0xffffff4d).into()),
    }
}

/// Miniatura dos painéis: fundo da janela e das barras, colados ou soltos sobre um fundo.
pub fn panels_thumbnail(floating: bool) -> (Hsla, Hsla, Hsla) {
    let c = colors();
    if floating { (hex(mix(c.accent, c.bg, 0.72), 1.), hex(c.float_chrome, 0.72), hex(c.line, 0.12)) }
    else { (rgb(c.inset).into(), rgb(c.chrome).into(), hex(c.line, 0.08)) }
}

fn floating() -> bool { appearance::get().panels == Panels::Floating }
pub fn is_floating() -> bool { floating() }

/// Imagem ou área de trabalho atrás da janela inteira: aí os painéis colados também deixam o fundo aparecer, como no web.
fn backdrop_everywhere() -> bool {
    let a = appearance::get();
    a.busy_background() && a.background_scope == BackgroundScope::Everywhere
}

/// Os painéis deixam passar o que está atrás: soltos sempre, colados só com fundo ocupado "em tudo".
fn see_through() -> bool { floating() || backdrop_everywhere() }

fn hex(value: u32, alpha: f32) -> Hsla { Hsla::from(rgb(value)).alpha(alpha) }

/// Tinta do modo atual já resolvida; o Desktop não tem tinta, a cor vem do papel de parede.
fn tint_color() -> Option<(u32, f32)> {
    if desktop_painting() { return None; }
    let a = appearance::get();
    let dark = is_dark();
    let mode = a.colors(dark);
    let color = match mode.tint {
        Swatch::Preset(0) => return None,
        Swatch::Preset(n) => *(if dark { &TINTS[..] } else { &TINTS_LIGHT[..] }).get(n)?,
        Swatch::Custom(hex) => hex.0,
    };
    Some((color, mode.tint_strength as f32 / 100.))
}

fn tinted(base: u32, alpha: f32) -> Hsla {
    hex(tint_color().map_or(base, |(t, strength)| mix(base, t, strength)), alpha)
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// Luminância relativa (WCAG) de uma cor sRGB.
fn luminance(c: u32) -> f32 {
    let lin = |shift: u32| {
        let v = ((c >> shift) & 0xff) as f32 / 255.;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(16) + 0.7152 * lin(8) + 0.0722 * lin(0)
}

fn solidity() -> f32 { appearance::get().solidity as f32 / 100. }

/// Vidro nos painéis: Vidro escolhido e uma imagem desenhada pela própria janela atrás deles "em tudo". A área de
/// trabalho crua (fundo Janela) fica fora da janela e não tem o que borrar.
pub fn panel_glass() -> bool {
    let a = appearance::get();
    a.surface_material == SurfaceMaterial::Glass && backdrop_everywhere() && BACKDROP_READY.load(Ordering::Relaxed)
        && !(a.background == Backdrop::Desktop && a.wallpaper == Wallpaper::Window)
}

/// Tinta sobre o desfoque: com ele segurando a leitura, desce até a do Zeron (0,15). O quadrado dá passos finos na
/// ponta leve da Solidez, e 100 continua opaco.
fn glass_tint() -> f32 { let s = solidity(); 0.15 + 0.85 * s * s }

/// Quanto os painéis translúcidos tapam o fundo: a tinta do vidro quando há desfoque atrás. Sem ele, no escuro, a do vidro
/// líquido do Electron (`--glass-bg`: 0,22 a 0,92 pela Solidez), para o mesmo número dar a mesma barra nos dois apps; o
/// claro do web usa outro vidro ali (branco fino deixa a tinta escura de trás atravessar) e fica na Solidez crua.
fn panel_alpha() -> f32 {
    if panel_glass() { return glass_tint(); }
    let s = solidity();
    if colors().dark { 0.22 + 0.70 * s } else { s }
}

/// Alfa do fundo da janela: a Transparência só vale com Imagem ou Desktop atrás; Liso, Textura e Luz são
/// opacos, senão o papel de parede do sistema vazaria por um fundo que a pessoa escolheu liso.
fn window_alpha() -> f32 {
    let a = appearance::get();
    if floating() && a.busy_background() { 1. - a.transparency as f32 / 100. } else { 1. }
}

/// Fundo da janela. Colados é opaco; na caixa solta a Transparência diz quanto da imagem ou do desktop aparece.
pub fn background() -> Hsla {
    let c = colors();
    tinted(if floating() { c.float_bg } else { c.bg }, window_alpha())
}

/// Pintura da raiz: a cor do Liso, o gradiente da Textura e da Luz, ou nada quando a camada de fundo desenha
/// imagem ou deixa ver a área de trabalho.
pub fn window_fill() -> Background {
    let a = appearance::get();
    match a.background {
        // Imagem que não abriu (sumiu, estragou, ainda lendo) cai no Liso, que é sempre legível.
        Backdrop::Image if !BACKDROP_READY.load(Ordering::Relaxed) => background().into(),
        Backdrop::Plain => background().into(),
        Backdrop::Image | Backdrop::Desktop => transparent_black().into(),
        Backdrop::Texture | Backdrop::Light => {
            let c = colors();
            let alpha = window_alpha();
            let base = if floating() { c.float_bg } else { c.bg };
            // Um degrau mais fundo em cima e um toque do destaque embaixo, como o gradiente da Textura do web.
            let top = mix(base, if c.dark { 0x000000 } else { 0x6b5f55 }, if c.dark { 0.22 } else { 0.03 });
            let bottom = mix(base, accent_hex(), if c.dark { 0.05 } else { 0.04 });
            linear_gradient(180., linear_color_stop(tinted(top, alpha), 0.), linear_color_stop(tinted(bottom, alpha), 1.))
        }
    }
}

/// Véu sobre a imagem ou a área de trabalho: a Transparência diz quanto dela atravessa, sem nunca chegar a crua.
pub fn veil() -> Hsla {
    let c = colors();
    tinted(if floating() { c.float_bg } else { c.bg }, 1. - appearance::get().transparency as f32 / 100. * 0.9)
}

/// Opacidade do grão: no claro ele aparece muito mais.
pub fn grain_opacity() -> f32 { if colors().dark { 0.05 } else { 0.03 } }

/// Luz fria da Luz, no alto à esquerda.
pub fn glow() -> Hsla { accent().alpha(if colors().dark { 0.30 } else { 0.20 }) }

/// Folha atrás da conversa na Leitura Folha: a Solidez diz quanto ela tapa o fundo.
pub fn sheet() -> Hsla { hex(colors().elevated, appearance::get().sheet_solidity as f32 / 100.) }
pub fn sheet_shadow() -> Vec<BoxShadow> {
    let alpha = if colors().dark { 0.28 } else { 0.10 };
    vec![BoxShadow { color: hsla(0., 0., 0., alpha), offset: point(px(0.), px(10.)), blur_radius: px(30.), spread_radius: px(0.), inset: false }]
}

/// No modo Texto a conversa volta para o branco (escuro) ou o preto (claro) na medida do Contraste;
/// o secundário e o apagado sobem num passo menor, como no web.
fn reading(color: u32, weight: f32) -> Hsla {
    let a = appearance::get();
    if a.effective_reading() != Reading::Text { return rgb(color).into(); }
    let dark = colors().dark;
    rgb(mix(color, if dark { 0xffffff } else { 0x000000 }, a.text_contrast as f32 / 100. * weight)).into()
}
/// Barra lateral, painel de contexto e navegação das configurações.
pub fn chrome() -> Hsla {
    let c = colors();
    tinted(if floating() { c.float_chrome } else { c.chrome }, if see_through() { panel_alpha() } else { 1. })
}
/// Visor de arquivo e cartões que cobrem a conversa: colados ficam cheios, o texto de baixo não atravessa.
pub fn surface() -> Hsla { let c = colors(); tinted(if floating() { c.float_chrome } else { c.chrome }, 1.) }
/// Caixas de conteúdo: compositor e grupos de configuração.
pub fn boxed() -> Hsla {
    let c = colors();
    hex(if floating() { c.float_boxed } else { c.boxed }, if see_through() { panel_alpha() * 0.9 } else { 1. })
}
pub fn inset() -> Hsla { let c = colors(); hex(c.inset, if see_through() { 0.55 } else { 1. }) }
/// Popovers e fundos de realce: sempre opacos, ficam sobre qualquer material.
pub fn elevated() -> Hsla { rgb(colors().elevated).into() }
pub fn raised() -> Hsla { rgb(colors().raised).into() }
/// Realce de passagem do ponteiro sobre linhas e botões quietos.
pub fn hover() -> Hsla { let c = colors(); if see_through() { hex(c.line, 0.06) } else { hex(c.hover, 1.) } }
pub fn user_bubble() -> Hsla { let c = colors(); if floating() { hex(c.float_bubble, 0.78) } else { hex(c.bubble, 1.) } }
/// Véu atrás de diálogos: a cor da janela quase opaca.
pub fn scrim() -> Hsla { hex(colors().bg, 0.87) }
pub fn text() -> Hsla { reading(colors().text, 1.) }
/// Texto corrido de documento: um degrau abaixo do `text`, que ofusca em parágrafo longo; títulos seguem no `text`.
pub fn body_text() -> Hsla { let c = colors(); reading(mix(c.text, c.muted, 0.35), 0.85) }
pub fn muted() -> Hsla { reading(colors().muted, 0.7) }
/// `--text-muted`: um degrau abaixo do secundário.
pub fn faint() -> Hsla { reading(colors().faint, 0.55) }

/// Destaque do modo atual já resolvido, em hexadecimal.
fn accent_hex() -> u32 {
    let c = colors();
    if desktop_painting() { return c.accent; }
    let dark = c.dark;
    match appearance::get().colors(dark).accent {
        Swatch::Preset(0) => c.accent,
        Swatch::Preset(n) => (if dark { &ACCENTS[..] } else { &ACCENTS_LIGHT[..] }).get(n).copied().unwrap_or(c.accent),
        Swatch::Custom(hex) => hex.0,
    }
}
pub fn accent() -> Hsla { rgb(accent_hex()).into() }
pub fn accent_dim() -> Hsla { accent().alpha(if colors().dark { 0.16 } else { 0.12 }) }
pub fn accent_press() -> Hsla { let a = accent(); hsla(a.h, a.s, (a.l - 0.06).max(0.), 1.) }
pub fn accent_focus() -> Hsla { accent().alpha(0.45) }
/// Texto sobre fundo de destaque suave: claro no escuro (`#c5cbf7` com o índigo), escuro no claro.
pub fn accent_text() -> Hsla {
    let a = accent();
    hsla(a.h, a.s.min(0.8), if colors().dark { 0.87 } else { 0.34 }, 1.)
}
/// Canto dos painéis soltos (barra, contexto, abas), o `--radius-xl` do web.
pub const PANEL_RADIUS: f32 = 24.;
/// `--elev-3` do web: a sombra da caixa solta e o brilho de 1 px na borda de cima, que é o que faz o painel ler como
/// vidro e não como recorte. Colado não tem nenhum dos dois, a borda separa.
pub fn panel_shadow() -> Vec<BoxShadow> {
    let mut shadow = card_shadow();
    if shadow.is_empty() { return shadow; }
    let rim = match (colors().dark, appearance::get().palette) { (false, _) => 0.95, (true, Palette::Neutral) => 0.18, (true, _) => 0.30 };
    shadow.push(BoxShadow { color: hsla(0., 0., 1., rim), offset: point(px(0.), px(1.)), blur_radius: px(1.), spread_radius: px(0.), inset: true });
    shadow
}
/// `--elev-2`: popovers e menus.
pub fn popover_shadow() -> Vec<BoxShadow> {
    let alpha = if colors().dark { 0.4 } else { 0.16 };
    vec![BoxShadow { color: hsla(0., 0., 0., alpha), offset: point(px(0.), px(8.)), blur_radius: px(28.), spread_radius: px(0.), inset: false }]
}
/// A tinta dos menus segue a dos painéis, com pelo menos metade, como no Zeron: o texto do menu disputa com o que
/// passa desfocado atrás. Opaco conserva o fundo anterior.
pub fn popup_fill(color: Hsla) -> Hsla {
    if appearance::get().surface_material == SurfaceMaterial::Glass {
        color.alpha(glass_tint().max(0.5))
    } else { color }
}
pub fn popup_content_fill() -> Hsla {
    if appearance::get().surface_material == SurfaceMaterial::Glass { transparent_black() }
    else { raised() }
}
/// Só a sombra da caixa solta, sem o brilho de borda: o compositor tem borda de foco própria.
pub fn card_shadow() -> Vec<BoxShadow> {
    if !floating() { return Vec::new(); }
    let alpha = if colors().dark { 0.35 } else { 0.13 };
    vec![BoxShadow { color: hsla(0., 0., 0., alpha), offset: point(px(0.), px(18.)), blur_radius: px(48.), spread_radius: px(0.), inset: false }]
}

/// Cor fixa de cada máquina na barra, a mesma do web (`serverColor` do core): o hash do id escolhe na mesma lista.
pub fn server_color(id: &str) -> Hsla {
    const COLORS: [u32; 6] = [0x7c6af7, 0x3ba55d, 0xe0a23b, 0xe0563b, 0x3b9fe0, 0xc43be0];
    let hash = id.encode_utf16().fold(0u32, |h, unit| h.wrapping_mul(31).wrapping_add(unit as u32));
    rgb(COLORS[hash as usize % COLORS.len()]).into()
}
/// Cor de uma etiqueta (`/api/tags`) pela chave, os tons do `TAG_COLOR_HEX` do core; chave desconhecida fica sem cor
/// e quem desenha usa a do hash.
pub fn tag_color(key: &str) -> Option<Hsla> {
    let hex = match key {
        "slate" => 0x64748b, "blue" => 0x3b82f6, "green" => 0x22c55e, "lime" => 0x84cc16,
        "amber" => 0xf59e0b, "orange" => 0xf97316, "pink" => 0xec4899, "violet" => 0x8b5cf6,
        _ => return None,
    };
    Some(rgb(hex).into())
}
pub fn border() -> Hsla { hex(colors().line, if floating() { 0.09 } else { 0.08 }) }
pub fn border_strong() -> Hsla { hex(colors().line_strong, if floating() { 0.16 } else { 0.14 }) }
pub fn glass_border() -> Hsla { border_strong() }
// Estados: no claro os tons descem para o texto ler sobre papel.
pub fn success() -> Hsla { rgb(if colors().dark { 0x34c759 } else { 0x1d8a3e }).into() }
pub fn warning() -> Hsla { rgb(if colors().dark { 0xff9f0a } else { 0xb25e00 }).into() }
pub fn danger() -> Hsla { rgb(if colors().dark { 0xff453a } else { 0xd12c21 }).into() }
/// Texto sobre fundo tingido da mesma cor (marca, linha de estado, nome em destaque): clareado no escuro, o `--success-text` do web.
pub fn success_text() -> Hsla { if colors().dark { rgb(mix(0x34c759, 0xffffff, 0.45)).into() } else { success() } }
pub fn warning_text() -> Hsla { if colors().dark { rgb(mix(0xff9f0a, 0xffffff, 0.4)).into() } else { warning() } }
/// Texto (e ponto de seleção) sobre um botão de cor cheia e escura, como o Enviar âmbar e o Salvar índigo.
pub fn on_press() -> Hsla { hsla(0., 0., 1., 1.) }
/// Fundo do botão âmbar (Responder, Enviar) com texto branco: o âmbar escurecido em 25%.
pub fn warning_press() -> Hsla { rgb(mix(if colors().dark { 0xff9f0a } else { 0xb25e00 }, 0x000000, 0.25)).into() }
/// Moldura do pedido que espera a pessoa (pergunta do agente, seletor do terminal).
pub fn ask_highlight() -> Hsla {
    match appearance::get().ask_highlight { appearance::AskHighlight::Accent => accent(), appearance::AskHighlight::Amber => warning() }
}
/// Vermelho das remoções no diff, mais claro que o de erro para ler em texto pequeno.
pub fn removed() -> Hsla { rgb(if colors().dark { 0xff6b61 } else { 0xc0392b }).into() }
/// Texto sobre o destaque cheio; um destaque claro (amarelo, cor livre) pede texto escuro.
pub fn on_accent() -> Hsla {
    let c = colors();
    rgb(if luminance(accent_hex()) > 0.45 { 0x1a1718 } else { c.on_accent }).into()
}
pub fn limited() -> Hsla { rgb(if colors().dark { 0xc98cff } else { 0x8a45c7 }).into() }
/// Linha selecionada: cinza elevado no colado opaco, destaque suave quando o painel é translúcido.
pub fn selected_row() -> Hsla { if see_through() { accent_dim() } else { rgb(colors().selected).into() } }
pub fn status(state: &str) -> Hsla {
    match state { "working" => accent(), "awaiting_input" => warning(), "idle" => success(), "dead" => danger(), _ => muted() }
}
/// Pílula de estado (`--pill-*`): fundo e texto.
pub fn pill(state: &str) -> (Hsla, Hsla) {
    match state {
        "working" => (accent_dim(), accent_text()),
        "idle" => (success().alpha(0.12), success()),
        "awaiting_input" => (warning().alpha(0.12), warning()),
        "dead" => (danger().alpha(0.12), danger()),
        "limited" => (limited().alpha(0.14), limited()),
        _ => (raised(), muted()),
    }
}
/// Cor de marca de cada provider, como no `ProviderGlyph` do web.
pub fn provider(name: &str) -> (Hsla, &'static str) {
    match name {
        "claude" => (rgb(0xd97757).into(), "C"),
        "codex" => (rgb(0x10a37f).into(), "X"),
        // O lilás claro do Kimi some sobre papel; no claro desce um tom.
        "kimi" => (rgb(if colors().dark { 0xc7bdf5 } else { 0x7061c2 }).into(), "K"),
        "pi" => (rgb(0x8b5cf6).into(), "π"),
        "omp" => (rgb(0xf59e0b).into(), "Ω"),
        // Sem marca de modelo: o orquestrador é código.
        "orq" => (muted(), "◇"),
        _ => (muted(), "?"),
    }
}

/// Amostras de destaque do modo: a primeira é o destaque da paleta.
pub fn accent_swatches(dark: bool) -> [u32; 7] {
    let mut list = if dark { ACCENTS } else { ACCENTS_LIGHT };
    list[0] = fixed(appearance::get().palette, dark).accent;
    list
}

/// Amostras de tinta do modo: a primeira é o fundo sem tinta.
pub fn tint_swatches(dark: bool) -> [u32; 4] {
    let mut list = if dark { TINTS } else { TINTS_LIGHT };
    list[0] = fixed(appearance::get().palette, dark).chrome;
    list
}

/// Tokens de `GET /api/desktop/palette` (Material You do rice) no papel de cada cor, como o `desktopTheme.ts` do web.
pub fn from_desktop(dark: bool, token: impl Fn(&str) -> Option<u32>) -> Option<Colors> {
    let [bg, low, container, high, on_surface, on_variant, outline, outline_variant, primary, on_primary] =
        ["background", "surfaceContainerLow", "surfaceContainer", "surfaceContainerHigh", "onSurface", "onSurfaceVariant",
            "outline", "outlineVariant", "primary", "onPrimary"].map(|name| token(name));
    // Meia paleta pintaria o fundo novo com o texto velho: falta um token, recusa inteira.
    let (bg, low, container, high, text, muted, faint, line, line_strong, accent, on_accent) =
        (bg?, low?, container?, high?, on_surface?, on_variant?, outline?, outline_variant?, outline?, primary?, on_primary?);
    let inset = mix(bg, if dark { 0x000000 } else { 0xffffff }, 0.25);
    Some(Colors { dark, bg, chrome: low, boxed: container, inset, hover: high, bubble: high, selected: high, elevated: container, raised: container,
        float_bg: bg, float_chrome: low, float_boxed: container, float_bubble: high,
        text, muted, faint, line, line_strong, accent, on_accent })
}

/// Markdown da conversa: parte das cores que o kit dá a todo texto e muda só a leitura longa. Títulos curtos,
/// 12 px entre blocos, código inline no destaque, tabela só com filetes, marcador de lista em coluna e faixa de
/// linguagem no código. Os dois últimos são campos da cópia do gpui-base que nenhuma outra tela liga.
/// Guardado pelo que ele lê (idioma do rótulo e cores do tema): cada linha da conversa pede o estilo a cada quadro.
pub fn conversation_markdown(cx: &App) -> gpui_kit::base::TextViewStyle {
    type Key = (bool, bool, [Hsla; 8], Pixels);
    thread_local! { static CACHED: std::cell::RefCell<Option<(Key, gpui_kit::base::TextViewStyle)>> = const { std::cell::RefCell::new(None) }; }
    let kit = Theme::global(cx);
    let radius = kit.semantic_tokens().radius.md;
    let key: Key = (crate::i18n::english(), kit.is_dark(),
        [kit.foreground, kit.muted_foreground, kit.link, kit.selection, elevated(), border(), accent_text(), accent_dim()], radius);
    CACHED.with_borrow_mut(|cached| match cached {
        Some((hit, style)) if *hit == key => style.clone(),
        _ => {
            let style = build_conversation_markdown(kit, radius);
            *cached = Some((key, style.clone()));
            style
        }
    })
}

fn build_conversation_markdown(kit: &Theme, radius: Pixels) -> gpui_kit::base::TextViewStyle {
    let mut code_block = StyleRefinement::default();
    code_block.corner_radii = CornersRefinement { top_left: Some(radius.into()), top_right: Some(radius.into()),
        bottom_left: Some(radius.into()), bottom_right: Some(radius.into()) };
    // Rolagem liga o layout que mede o texto real: o padrão divide a largura por contagem de letras e parte palavra da coluna curta.
    let mut table = StyleRefinement::default().border_0().bg(transparent_black());
    table.overflow.x = Some(Overflow::Scroll);
    gpui_kit::base::TextViewStyle::default()
        .with_foreground(kit.foreground)
        .with_muted_foreground(kit.muted_foreground)
        .with_link(kit.link)
        .with_selection(kit.selection)
        // Fundo e filete do app, como o `.code-block` do web: o `border` do kit tem o tom do `muted`, e a faixa sumia.
        .with_code_background(elevated())
        .with_border(border())
        .with_dark(kit.is_dark())
        .with_paragraph_gap(rems(0.75))
        // Em relação ao texto da resposta, como os `em` do `.prose` do web; lido no desenho, o tamanho muda sem refazer o estilo.
        .with_heading(|level| StyleRefinement::default().text_size(px(17. * appearance::get().text_size as f32 / 100.
            * match level { 1 => 1.4, 2 => 1.25, 3 => 1.1, _ => 1. })))
        .with_inline_code(HighlightStyle { color: Some(accent_text()), background_color: Some(accent_dim()), ..Default::default() })
        .with_code_block(code_block)
        .with_table(table)
        .with_table_head(StyleRefinement::default().bg(transparent_black()).font_weight(FontWeight::BOLD))
        .with_table_cell(StyleRefinement::default().border_r_0().px_3().py_1p5())
        .with_list_marker_width(Some(px(26.)))
        .with_list_marker_color(Some(accent_text()))
        .with_code_language_band(Some(crate::i18n::tr_web("comum_codigo", &Default::default()).unwrap_or_default().into()))
}

/// Tamanho base do leitor de documentos (o 100% do A−/A+), o do preview de Markdown do VS Code.
pub const READER_BASE_SIZE: u8 = 14;

/// Primeira fonte de `candidates` instalada, ou `fallback`.
pub fn first_installed(installed: &[String], candidates: &[&str], fallback: &str) -> SharedString {
    candidates.iter().copied().find(|name| installed.iter().any(|installed| installed == name)).unwrap_or(fallback).to_owned().into()
}

/// Fonte do leitor de documentos: a lista do preview de Markdown do VS Code, na ordem dele; a Geist é de interface e cansa
/// em parágrafo longo. Sem nenhuma instalada, a sans do sistema.
pub fn reading_font(cx: &App) -> SharedString {
    static FONT: OnceLock<SharedString> = OnceLock::new();
    FONT.get_or_init(|| {
        const SYSTEM: &str = ".SystemUIFont";
        if cfg!(target_os = "macos") { return SYSTEM.into(); }
        first_installed(&cx.text_system().all_font_names(),
            &["Segoe WPC", "Segoe UI", "Ubuntu", "Droid Sans", "Noto Sans", "DejaVu Sans"], SYSTEM)
    }).clone()
}

/// Documento tem escala própria: A−/A+ não alteram a conversa nem o editor.
pub fn document_markdown(cx: &App, font_size: u8) -> gpui_kit::base::TextViewStyle {
    let mut table = StyleRefinement::default().border_1().border_color(border_strong()).rounded_md().bg(transparent_black());
    table.overflow.x = Some(Overflow::Scroll);
    let (size, base) = (font_size as f32, READER_BASE_SIZE as f32);
    let (body, title, rule) = (body_text(), text(), border_strong());
    // Medidas do `markdown.css` do VS Code: H1 e H2 com fio embaixo, 24 px antes e 16 px depois de cada título. As margens
    // de lá se sobrepõem; aqui o espaço do parágrafo anterior soma, e o título só completa até os 24 px.
    conversation_markdown(cx)
        .with_foreground(body).with_muted_foreground(muted()).with_link(accent_text())
        .with_border(rule).with_code_background(inset())
        .with_paragraph_gap(rems(1.))
        .with_heading(move |level| {
            let em = size * match level { 1 => 2., 2 => 1.5, 3 => 1.25, 4 => 1., 5 => 0.875, _ => 0.85 };
            let heading = StyleRefinement::default().text_size(px(em)).font_weight(FontWeight::SEMIBOLD).text_color(title)
                .line_height(relative(1.25)).mt(px(if level == 1 { 0. } else { size * 8. / base })).mb(px(size * 16. / base));
            if level <= 2 { heading.pb(px(em * 0.3)).border_b_1().border_color(rule) } else { heading }
        })
        .with_inline_code(HighlightStyle { color: Some(body), background_color: Some(title.opacity(0.1)), ..Default::default() })
        .with_table(table)
        .with_table_head(StyleRefinement::default().bg(inset()).font_weight(FontWeight::SEMIBOLD))
        .with_table_cell(StyleRefinement::default().border_r_1().border_color(rule).px_3().py_2())
}

/// Leva modo claro/escuro, destaque e fonte para os componentes do gpui-kit (entrada, menus, botão primário).
pub fn sync_kit(window: Option<&mut Window>, cx: &mut App) {
    let (original_font, _) = original_code_typography(cx);
    let appearance = appearance::get();
    let dark = is_dark();
    if Theme::global(cx).is_dark() != dark {
        Theme::change(if dark { KitMode::Dark } else { KitMode::Light }, window, cx);
    }
    let theme = Theme::global_mut(cx);
    theme.primary = accent();
    theme.primary_hover = accent_press();
    theme.primary_active = accent_press();
    theme.primary_foreground = on_accent();
    // Checkbox, Radio e Switch do kit pintam o marcado pelo token, não pelo campo acima.
    theme.tokens.primary = theme.primary.into();
    theme.tokens.primary_hover = theme.primary_hover.into();
    theme.tokens.primary_active = theme.primary_active.into();
    theme.ring = accent_focus();
    theme.font_family = SANS.into();
    theme.mono_font_family = match appearance.code_font {
        appearance::CodeFont::JetBrainsMono => CODE_MONO.into(),
        appearance::CodeFont::System => original_font,
        appearance::CodeFont::Named(name) => name.0.into(),
    };
    theme.mono_font_size = px(appearance.code_size as f32 / 2.);
    Theme::sync_base(cx);
}

#[cfg(test)]
mod tests {
    #[test]
    fn mix_moves_each_channel() {
        assert_eq!(super::mix(0x000000, 0xff8000, 0.5), 0x804000);
        assert_eq!(super::mix(0x121013, 0x121013, 0.4), 0x121013);
    }

    #[test]
    fn desktop_palette_needs_every_token() {
        let full = |name: &str| Some(if name == "primary" { 0x8ab4f8 } else { 0x202124 });
        let colors = super::from_desktop(true, full).expect("all tokens present");
        assert_eq!((colors.accent, colors.bg), (0x8ab4f8, 0x202124));
        assert!(super::from_desktop(true, |name| (name != "outline").then_some(0x202124)).is_none());
    }

    #[test]
    fn light_accent_gets_dark_text() {
        assert!(super::luminance(0xe9b93f) > 0.45);
        assert!(super::luminance(0x5b6ad0) < 0.45);
    }

    #[test]
    fn server_color_matches_web_hash() {
        use gpui_kit::{Hsla, rgb};
        let color = |hex: u32| -> Hsla { rgb(hex).into() };
        assert_eq!(super::server_color("srv-kbueb3n5"), color(0xe0563b));
        assert_eq!(super::server_color("a"), color(0x3ba55d));
        assert_eq!(super::server_color("notebook"), color(0xc43be0));
    }
}
