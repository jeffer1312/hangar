//! Animações das novidades da atualização: cada uma é desenhada com as mesmas partes do app que ela mostra.

mod account_tags;
mod agent_fix;
mod agy_terms;
mod antigravity;
mod auto_report;
mod claude_notices;
mod cliproxy_wizard;
mod notice_actions;
mod codex_activity;
mod code_comments;
mod compact_idle;
mod composer_history;
mod connect_approval;
mod connect_sign_in;
mod conversation_gallery;
mod desktop_tool_look;
mod dictation_pill;
mod login_browser;
mod memory_dream;
mod model_handoff;
mod claude_providers;
mod new_chat_mode;
mod new_conversation;
mod no_project;
mod password_vault;
mod plugins;
mod proxy_update;
mod adopt_cliproxy;
mod scheduled_tasks;
mod hide_from_list;
mod image_reveal;
mod kimi_permissions;
mod rename_suggestion;
mod search_conversations;
mod server_status;
mod session_ports;
mod sleep_hidden;
mod speech_pause;
mod speed_choice;
mod session_vault;
mod telemetry_consent;
mod tour;
mod updates_manual;
mod worktree_fresh;
mod reviews;
mod tickets;
mod work_review;
mod whatsapp_readonly;
mod whatsapp_draft;

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::*;

thread_local! {
    /// O redesenho já marcado: cada render só marca outro se o dele cair antes. Um redesenho vale para todas as
    /// animações da janela, e cada uma marca de novo o seu próximo degrau.
    static NEXT_REPAINT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Redesenha a janela daqui a `secs`, no próximo degrau do roteiro. Só é marcado enquanto a animação é desenhada:
/// fechou o modal, para.
pub(super) fn repaint_in(secs: f32, window: &mut Window, cx: &mut App) {
    // Margem para o timer não acordar um pouco antes do degrau e desenhar o quadro velho.
    let at = Instant::now() + Duration::from_secs_f32(secs) + Duration::from_millis(5);
    if NEXT_REPAINT.get().is_some_and(|next| next > Instant::now() && next <= at) { return; }
    NEXT_REPAINT.set(Some(at));
    window.spawn(cx, async move |cx| {
        cx.background_executor().timer(at.saturating_duration_since(Instant::now())).await;
        let _ = cx.update(|window, _| window.refresh());
    }).detach();
}

/// Desenha a animação na largura do cartão. `started` é quando o cartão apareceu: o ciclo conta dele.
pub type NewsAnimation = fn(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement;

/// A animação pelo nome do arquivo da novidade (`animacao:`); nome desconhecido = cartão só com texto.
pub fn animation(name: &str) -> Option<NewsAnimation> {
    match name {
        "kimi-permissions" => Some(kimi_permissions::render),
        "historico-compositor" => Some(composer_history::render),
        "codex-atividade" => Some(codex_activity::render),
        "etiquetas-maquina-conta" => Some(account_tags::render),
        "sugestao-de-nome" => Some(rename_suggestion::render),
        "tour-nativo" => Some(tour::render),
        "cofre-de-senhas" => Some(password_vault::render),
        "relatorio-automatico" => Some(auto_report::render),
        "pedido-na-sessao" => Some(session_vault::render),
        "login-navegador" => Some(login_browser::render),
        "ocultar-da-lista" => Some(hide_from_list::render),
        "busca-conversas" => Some(search_conversations::render),
        "revelacao-imagem" => Some(image_reveal::render),
        "adormecer-ocultas" => Some(sleep_hidden::render),
        "sem-projeto" => Some(no_project::render),
        "modo-da-conversa" => Some(new_chat_mode::render),
        "pilula-do-ditado" => Some(dictation_pill::render),
        "pausar-fala" => Some(speech_pause::render),
        "hangar-connect" => Some(connect_approval::render),
        "worktree-atualizada" => Some(worktree_fresh::render),
        "compactar-parada" => Some(compact_idle::render),
        "plugins-do-claude" => Some(plugins::render),
        "revisoes-pr-mr" => Some(reviews::render),
        "tickets-board" => Some(tickets::render),
        "scheduled-tasks" => Some(scheduled_tasks::render),
        "atualizacoes-e-manual" => Some(updates_manual::render),
        "estilo-claude-desktop" => Some(desktop_tool_look::render),
        "sonho-da-memoria" => Some(memory_dream::render),
        "avisos-do-claude" => Some(claude_notices::render),
        "acoes-dos-avisos" => Some(notice_actions::render),
        "tela-de-status" => Some(server_status::render),
        "consertar-com-agente" => Some(agent_fix::render),
        "revisao-do-trabalho" => Some(work_review::render),
        "comentar-linhas" => Some(code_comments::render),
        "seletor-de-modelos" => Some(model_handoff::render),
        "provedores-do-claude" => Some(claude_providers::render),
        "cliproxy-assistente" => Some(cliproxy_wizard::render),
        "velocidade-ultra-fast" => Some(speed_choice::render),
        "atualizar-cliproxy" => Some(proxy_update::render),
        "entregar-cliproxy" => Some(adopt_cliproxy::render),
        "conversa-nova" => Some(new_conversation::render),
        "galeria-da-conversa" => Some(conversation_gallery::render),
        "whatsapp-somente-leitura" => Some(whatsapp_readonly::render),
        "whatsapp-draft" => Some(whatsapp_draft::render),
        "entrar-pelo-connect" => Some(connect_sign_in::render),
        "portas-da-sessao" => Some(session_ports::render),
        "antigravity" => Some(antigravity::render),
        "termos-antigravity" => Some(agy_terms::render),
        "contagens-anonimas" => Some(telemetry_consent::render),
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
