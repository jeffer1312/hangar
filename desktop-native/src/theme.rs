use gpui_kit::{component::{Theme, ThemeMode as KitMode}, *};
use std::sync::{RwLock, atomic::{AtomicBool, Ordering}};
use crate::appearance::{self, Background as Backdrop, DesktopText, Palette, Panels, Reading, Swatch, ThemeMode};

// Cores dos mocks aprovados (Task 12): o padrão é "Colados", opaco; "Caixa solta" deixa passar o que está
// atrás da janela nas medidas de Transparência e Solidez. O nome de cada função diz o papel, não a cor.
/// `--font-mono` do web; sem a fonte instalada, o GPUI cai na padrão.
pub const MONO: &str = "JetBrainsMono Nerd Font";
/// Sans embutida no binário (`assets/fonts`), a "Sistema" das configurações.
pub const SANS: &str = "Geist";

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

/// Fundo da janela. Colados é opaco; na caixa solta a Transparência diz quanto do fundo do sistema aparece.
pub fn background() -> Hsla {
    let c = colors();
    if floating() { tinted(c.float_bg, 1. - appearance::get().transparency as f32 / 100.) } else { tinted(c.bg, 1.) }
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
            let alpha = if floating() { 1. - a.transparency as f32 / 100. } else { 1. };
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
pub fn chrome() -> Hsla { let c = colors(); if floating() { tinted(c.float_chrome, solidity()) } else { tinted(c.chrome, 1.) } }
pub fn surface() -> Hsla { chrome() }
/// Caixas de conteúdo: compositor e grupos de configuração.
pub fn boxed() -> Hsla { let c = colors(); if floating() { hex(c.float_boxed, solidity() * 0.9) } else { hex(c.boxed, 1.) } }
pub fn inset() -> Hsla { let c = colors(); hex(c.inset, if floating() { 0.55 } else { 1. }) }
/// Popovers e fundos de realce: sempre opacos, ficam sobre qualquer material.
pub fn elevated() -> Hsla { rgb(colors().elevated).into() }
pub fn raised() -> Hsla { rgb(colors().raised).into() }
/// Realce de passagem do ponteiro sobre linhas e botões quietos.
pub fn hover() -> Hsla { let c = colors(); if floating() { hex(c.line, 0.06) } else { hex(c.hover, 1.) } }
pub fn user_bubble() -> Hsla { let c = colors(); if floating() { hex(c.float_bubble, 0.78) } else { hex(c.bubble, 1.) } }
/// Véu atrás de diálogos: a cor da janela quase opaca.
pub fn scrim() -> Hsla { hex(colors().bg, 0.87) }
pub fn text() -> Hsla { reading(colors().text, 1.) }
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
/// Sombra das caixas soltas; colado não tem sombra, a borda separa.
pub fn panel_shadow() -> Vec<BoxShadow> {
    if !floating() { return Vec::new(); }
    let alpha = if colors().dark { 0.35 } else { 0.13 };
    vec![BoxShadow { color: hsla(0., 0., 0., alpha), offset: point(px(0.), px(18.)), blur_radius: px(48.), spread_radius: px(0.), inset: false }]
}
/// `--elev-2`: popovers e menus.
pub fn popover_shadow() -> Vec<BoxShadow> {
    let alpha = if colors().dark { 0.4 } else { 0.16 };
    vec![BoxShadow { color: hsla(0., 0., 0., alpha), offset: point(px(0.), px(8.)), blur_radius: px(28.), spread_radius: px(0.), inset: false }]
}
pub fn card_shadow() -> Vec<BoxShadow> { panel_shadow() }
pub fn border() -> Hsla { hex(colors().line, if floating() { 0.09 } else { 0.08 }) }
pub fn border_strong() -> Hsla { hex(colors().line_strong, if floating() { 0.16 } else { 0.14 }) }
pub fn glass_border() -> Hsla { border_strong() }
// Estados: no claro os tons descem para o texto ler sobre papel.
pub fn success() -> Hsla { rgb(if colors().dark { 0x34c759 } else { 0x1d8a3e }).into() }
pub fn warning() -> Hsla { rgb(if colors().dark { 0xff9f0a } else { 0xb25e00 }).into() }
pub fn danger() -> Hsla { rgb(if colors().dark { 0xff453a } else { 0xd12c21 }).into() }
/// Vermelho das remoções no diff, mais claro que o de erro para ler em texto pequeno.
pub fn removed() -> Hsla { rgb(if colors().dark { 0xff6b61 } else { 0xc0392b }).into() }
/// Texto sobre o destaque cheio; um destaque claro (amarelo, cor livre) pede texto escuro.
pub fn on_accent() -> Hsla {
    let c = colors();
    rgb(if luminance(accent_hex()) > 0.45 { 0x1a1718 } else { c.on_accent }).into()
}
pub fn limited() -> Hsla { rgb(if colors().dark { 0xc98cff } else { 0x8a45c7 }).into() }
/// Linha selecionada: cinza elevado no colado, destaque suave na caixa solta.
pub fn selected_row() -> Hsla { if floating() { accent_dim() } else { rgb(colors().selected).into() } }
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

/// Leva modo claro/escuro, destaque e fonte para os componentes do gpui-kit (entrada, menus, botão primário).
pub fn sync_kit(window: Option<&mut Window>, cx: &mut App) {
    let dark = is_dark();
    if Theme::global(cx).is_dark() != dark {
        Theme::change(if dark { KitMode::Dark } else { KitMode::Light }, window, cx);
    }
    let theme = Theme::global_mut(cx);
    theme.primary = accent();
    theme.primary_hover = accent_press();
    theme.primary_active = accent_press();
    theme.primary_foreground = on_accent();
    theme.ring = accent_focus();
    theme.font_family = SANS.into();
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
}
