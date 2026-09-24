use gpui_kit::{component::Theme, *};
use crate::appearance::{self, Panels};

// Cores dos mocks aprovados (Task 12): o padrão é "Colados", opaco; "Caixa solta" deixa passar o que está
// atrás da janela nas medidas de Transparência e Solidez. O nome de cada função diz o papel, não a cor.
/// `--font-mono` do web; sem a fonte instalada, o GPUI cai na padrão.
pub const MONO: &str = "JetBrainsMono Nerd Font";
/// Sans embutida no binário (`assets/fonts`), a "Sistema" das configurações.
pub const SANS: &str = "Geist";

/// Destaques oferecidos em Aparência › Cor; o primeiro é o índigo do Hangar.
pub const ACCENTS: [u32; 7] = [0x7c87e8, 0x9b7cf0, 0xf08a4b, 0xe9b93f, 0x3fbf6f, 0x3cc4d6, 0xe070b0];
/// Tintas de fundo; a primeira é "sem tinta" e mostra a cor base.
pub const TINTS: [u32; 4] = [0x18151a, 0x1d1a2e, 0x2a1a1a, 0x1a2a20];

fn floating() -> bool { appearance::get().panels == Panels::Floating }
pub fn is_floating() -> bool { floating() }

fn hex(value: u32, alpha: f32) -> Hsla { Hsla::from(rgb(value)).alpha(alpha) }

fn tinted(base: u32, alpha: f32) -> Hsla {
    let a = appearance::get();
    let color = match TINTS.get(a.tint).filter(|_| a.tint > 0) {
        Some(&t) => mix(base, t, a.tint_strength as f32 / 100.),
        None => base,
    };
    hex(color, alpha)
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

fn solidity() -> f32 { appearance::get().solidity as f32 / 100. }

/// Fundo da janela. Colados é opaco; na caixa solta a Transparência diz quanto do fundo do sistema aparece.
pub fn background() -> Hsla {
    if floating() { tinted(0x0d0c12, 1. - appearance::get().transparency as f32 / 100.) } else { tinted(0x121013, 1.) }
}
/// Barra lateral, painel de contexto e navegação das configurações.
pub fn chrome() -> Hsla { if floating() { tinted(0x1a181d, solidity()) } else { tinted(0x18151a, 1.) } }
pub fn surface() -> Hsla { chrome() }
/// Caixas de conteúdo: compositor e grupos de configuração.
pub fn boxed() -> Hsla { if floating() { hex(0x26242c, solidity() * 0.9) } else { hex(0x1f1b20, 1.) } }
pub fn inset() -> Hsla { if floating() { hex(0x0e0c10, 0.55) } else { hex(0x0e0c0f, 1.) } }
/// Popovers e fundos de realce: sempre opacos, ficam sobre qualquer material.
pub fn elevated() -> Hsla { rgb(0x2c262d).into() }
pub fn raised() -> Hsla { rgb(0x262127).into() }
/// Realce de passagem do ponteiro sobre linhas e botões quietos.
pub fn hover() -> Hsla { if floating() { hex(0xffffff, 0.06) } else { hex(0x262127, 1.) } }
pub fn user_bubble() -> Hsla { if floating() { hex(0x343038, 0.78) } else { hex(0x2a272c, 1.) } }
pub fn text() -> Hsla { rgb(0xe6e0e2).into() }
pub fn muted() -> Hsla { rgb(0xa39a9e).into() }
/// `--text-muted`: um degrau abaixo do secundário.
pub fn faint() -> Hsla { rgb(0x8a8186).into() }
pub fn accent() -> Hsla {
    let n = appearance::get().accent;
    rgb(ACCENTS.get(n).copied().unwrap_or(ACCENTS[0])).into()
}
pub fn accent_dim() -> Hsla { accent().alpha(0.16) }
pub fn accent_press() -> Hsla { let a = accent(); hsla(a.h, a.s, (a.l - 0.06).max(0.), 1.) }
pub fn accent_focus() -> Hsla { accent().alpha(0.45) }
/// Texto sobre fundo de destaque suave (`#c5cbf7` com o índigo).
pub fn accent_text() -> Hsla { let a = accent(); hsla(a.h, a.s.min(0.8), 0.87, 1.) }
/// Sombra das caixas soltas; colado não tem sombra, a borda separa.
pub fn panel_shadow() -> Vec<BoxShadow> {
    if !floating() { return Vec::new(); }
    vec![BoxShadow { color: rgba(0x00000059).into(), offset: point(px(0.), px(18.)), blur_radius: px(48.), spread_radius: px(0.), inset: false }]
}
/// `--elev-2`: popovers e menus.
pub fn popover_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow { color: rgba(0x00000066).into(), offset: point(px(0.), px(8.)), blur_radius: px(28.), spread_radius: px(0.), inset: false }]
}
pub fn card_shadow() -> Vec<BoxShadow> { panel_shadow() }
pub fn border() -> Hsla { if floating() { hex(0xffffff, 0.09) } else { hex(0xfff8f4, 0.08) } }
pub fn border_strong() -> Hsla { if floating() { hex(0xffffff, 0.16) } else { hex(0xfff8f4, 0.14) } }
pub fn glass_border() -> Hsla { border_strong() }
pub fn success() -> Hsla { rgb(0x34c759).into() }
pub fn warning() -> Hsla { rgb(0xff9f0a).into() }
pub fn danger() -> Hsla { rgb(0xff453a).into() }
/// Vermelho das remoções no diff, mais claro que o de erro para ler em texto pequeno.
pub fn removed() -> Hsla { rgb(0xff6b61).into() }
pub fn on_accent() -> Hsla { rgb(0xfdf8f9).into() }
pub fn limited() -> Hsla { rgb(0xc98cff).into() }
/// Linha selecionada: cinza elevado no colado, destaque suave na caixa solta.
pub fn selected_row() -> Hsla { if floating() { accent_dim() } else { hex(0x2c262d, 1.) } }
pub fn status(state: &str) -> Hsla {
    match state { "working" => accent(), "awaiting_input" => warning(), "idle" => success(), "dead" => danger(), _ => muted() }
}
/// Pílula de estado (`--pill-*`): fundo e texto.
pub fn pill(state: &str) -> (Hsla, Hsla) {
    match state {
        "working" => (accent_dim(), accent_text()),
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

/// Leva o destaque e a fonte da aparência para os componentes do gpui-kit (botão primário, anel de foco).
pub fn sync_kit(cx: &mut App) {
    let theme = Theme::global_mut(cx);
    theme.primary = accent();
    theme.primary_hover = accent_press();
    theme.primary_active = accent_press();
    theme.ring = accent_focus();
    theme.font_family = SANS.into();
}

#[cfg(test)]
mod tests {
    #[test]
    fn mix_moves_each_channel() {
        assert_eq!(super::mix(0x000000, 0xff8000, 0.5), 0x804000);
        assert_eq!(super::mix(0x121013, 0x121013, 0.4), 0x121013);
    }
}
