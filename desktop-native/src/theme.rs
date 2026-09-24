use gpui_kit::*;

// Valores de `frontend/src/app.css` (tema escuro); o nome diz o token do web.
/// `--font-mono` do web; sem a fonte instalada, o GPUI cai na padrão.
pub const MONO: &str = "JetBrainsMono Nerd Font";
pub fn background() -> Hsla {
    if cfg!(target_os = "linux") { rgba(0x100e11ed).into() } else { rgb(0x100e11).into() }
}
pub fn surface() -> Hsla { rgba(0x1a171aee).into() }
/// `--chrome-bg`: fundo da barra lateral, do compositor e do painel de contexto.
pub fn chrome() -> Hsla { rgba(0x18171cf0).into() }
pub fn inset() -> Hsla { rgb(0x100e11).into() }
pub fn elevated() -> Hsla { rgb(0x221d22).into() }
pub fn raised() -> Hsla { rgb(0x2a242a).into() }
pub fn text() -> Hsla { rgb(0xeee8e9).into() }
pub fn muted() -> Hsla { rgb(0xa0989b).into() }
/// `--text-muted`: um degrau abaixo do secundário.
pub fn faint() -> Hsla { rgb(0x8d8489).into() }
pub fn accent() -> Hsla { rgb(0x7c87e8).into() }
pub fn accent_dim() -> Hsla { rgba(0x7c87e82e).into() }
pub fn accent_press() -> Hsla { rgb(0x6e79d6).into() }
pub fn accent_hover() -> Hsla { rgba(0x7c87e847).into() }
pub fn accent_focus() -> Hsla { rgba(0x7c87e873).into() }
/// Faixa do cabeçalho do painel: `--bg-elevated` a 52%.
pub fn header_band() -> Hsla { rgba(0x221d2285).into() }
/// `--elev-3`, sombra do painel de contexto.
pub fn panel_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow { color: rgba(0x00000057).into(), offset: point(px(0.), px(18.)), blur_radius: px(44.), spread_radius: px(0.), inset: false }]
}
/// `--elev-2`: popovers e menus.
pub fn popover_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow { color: rgba(0x00000066).into(), offset: point(px(0.), px(8.)), blur_radius: px(28.), spread_radius: px(0.), inset: false }]
}
/// Sombra do cartão do compositor: `0 12px 40px rgba(0,0,0,.42)`.
pub fn card_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow { color: rgba(0x0000006b).into(), offset: point(px(0.), px(12.)), blur_radius: px(40.), spread_radius: px(0.), inset: false }]
}
pub fn border() -> Hsla { rgba(0xfff8f412).into() }
pub fn border_strong() -> Hsla { rgba(0xfff8f41f).into() }
pub fn glass_border() -> Hsla { rgba(0xffffff1a).into() }
pub fn success() -> Hsla { rgb(0x34c759).into() }
pub fn warning() -> Hsla { rgb(0xff9f0a).into() }
pub fn danger() -> Hsla { rgb(0xff453a).into() }
pub fn on_accent() -> Hsla { rgb(0xfdf8f9).into() }
pub fn limited() -> Hsla { rgb(0xc98cff).into() }
/// Linha selecionada: accent a 10%.
pub fn selected_row() -> Hsla { rgba(0x7c87e81a).into() }
/// Linha aguardando você: warning a 7%.
pub fn awaiting_row() -> Hsla { rgba(0xff9f0a12).into() }
pub fn status(state: &str) -> Hsla {
    match state { "working" => accent(), "awaiting_input" => warning(), "idle" => success(), "dead" => danger(), _ => muted() }
}
/// Pílula de estado (`--pill-*`): fundo e texto.
pub fn pill(state: &str) -> (Hsla, Hsla) {
    match state {
        "working" => (rgba(0x7c87e829).into(), rgb(0xaab2f3).into()),
        "idle" => (rgba(0x34c7591f).into(), success()),
        "awaiting_input" => (rgba(0xff9f0a1f).into(), warning()),
        "dead" => (rgba(0xff453a1f).into(), danger()),
        "limited" => (rgba(0xc98cff24).into(), limited()),
        _ => (raised(), muted()),
    }
}
/// Cor de marca de cada provider, como no `ProviderGlyph` do web.
pub fn provider(name: &str) -> (Hsla, &'static str) {
    match name {
        "claude" => (rgb(0xd97757).into(), "C"),
        "codex" => (rgb(0x10a37f).into(), "X"),
        "kimi" => (rgb(0xc7bdf5).into(), "K"),
        "pi" => (rgb(0x8b5cf6).into(), "π"),
        "omp" => (rgb(0xf59e0b).into(), "Ω"),
        _ => (muted(), "?"),
    }
}
