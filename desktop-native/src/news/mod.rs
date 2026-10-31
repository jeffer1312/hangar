//! Animações das novidades da atualização: cada uma é desenhada com as mesmas partes do app que ela mostra.

mod account_tags;
mod auto_report;
mod codex_activity;
mod composer_history;
mod password_vault;
mod hide_from_list;
mod rename_suggestion;

use std::time::Instant;
use gpui_kit::*;

/// Desenha a animação na largura do cartão. `started` é quando o cartão apareceu: o ciclo conta dele.
pub type NewsAnimation = fn(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement;

/// A animação pelo nome do arquivo da novidade (`animacao:`); nome desconhecido = cartão só com texto.
pub fn animation(name: &str) -> Option<NewsAnimation> {
    match name {
        "historico-compositor" => Some(composer_history::render),
        "codex-atividade" => Some(codex_activity::render),
        "etiquetas-maquina-conta" => Some(account_tags::render),
        "sugestao-de-nome" => Some(rename_suggestion::render),
        "cofre-de-senhas" => Some(password_vault::render),
        "relatorio-automatico" => Some(auto_report::render),
        "ocultar-da-lista" => Some(hide_from_list::render),
        _ => None,
    }
}

/// `HANGAR_NATIVE_PREVIEW=<nome>` (build de debug, `cargo build` em desktop-native): abre a animação num diálogo, para
/// ver e fotografar sem o modal da atualização.
#[cfg(debug_assertions)]
pub fn open_preview(name: String, window: &mut Window, cx: &mut App) {
    use gpui_kit::component::WindowExt;
    let Some(draw) = animation(&name) else { eprintln!("novidades: animação desconhecida: {name}"); return; };
    let started = Instant::now();
    window.open_dialog(cx, move |dialog, window, cx| {
        crate::app::popup::dialog(dialog).w(px(560.)).title(name.clone()).child(draw(started, px(520.), window, cx))
    });
}
