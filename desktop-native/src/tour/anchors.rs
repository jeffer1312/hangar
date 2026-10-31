//! Ids das âncoras que o tour, as dicas e o "Mostrar no app" das novidades apontam. O `ancora:` de `docs/novidades/*.md`
//! tem de estar em `ALL` (teste abaixo).
// Parte dos ids só é lida pelos passos do tour e pelas dicas.
#![allow(dead_code)]

/// Os dois botões de nova sessão (lista e barra de abas): só um aparece por vez.
pub const NEW_SESSION: &str = "new-session";
pub const TOPBAR_SEARCH: &str = "topbar-search";
pub const TOPBAR_PRESENCE: &str = "topbar-presence";
pub const COMPOSER: &str = "composer";
pub const COMPOSER_ACCOUNT: &str = "composer-account";
/// O microfone do compositor; o da animação de novidade não registra.
pub const DICTATION_TOGGLE: &str = "dictation-toggle";
pub const CTL_MODE: &str = "ctl-mode";
pub const CTL_PERMISSION: &str = "ctl-permission";
pub const QUEUE_PANEL: &str = "queue-panel";
/// Família das linhas e abas de sessão; a âncora de cada uma é `session_row`.
pub const SESSION_ROW: &str = "session-row";

pub const ALL: &[&str] = &[NEW_SESSION, TOPBAR_SEARCH, TOPBAR_PRESENCE, COMPOSER, COMPOSER_ACCOUNT, DICTATION_TOGGLE, CTL_MODE,
    CTL_PERMISSION, QUEUE_PANEL, SESSION_ROW];

/// A linha (ou aba) da sessão `name` da máquina `server`, em qualquer modo de navegação.
pub fn session_row(server: &str, name: &str) -> String { format!("{SESSION_ROW}:{server}::{name}") }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_row_carries_server_and_name() {
        assert_eq!(session_row("notebook", "hangar"), "session-row:notebook::hangar");
    }

    /// O `ancora:` do frontmatter de cada novidade aponta para uma âncora que o app registra.
    #[test]
    fn every_news_anchor_is_known() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/novidades");
        let mut unknown = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") { continue; }
            let text = std::fs::read_to_string(&path).unwrap();
            let Some(front) = text.strip_prefix("---").and_then(|rest| rest.split_once("\n---")).map(|(front, _)| front) else { continue };
            for line in front.lines() {
                let Some(value) = line.strip_prefix("ancora:") else { continue };
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                if !ALL.contains(&value) { unknown.push(format!("{}: {value}", path.display())); }
            }
        }
        assert!(unknown.is_empty(), "ancora fora de tour::anchors::ALL: {unknown:?}");
    }
}
