//! Marcadores portáveis de caminho (`path_markers`). Casos e valores esperados de
//! `backend/tests/test_config_sync_paths.py` e `test_install_root.py` (ponta 7ef9011ba).
use hangar_core::path_markers::{Roots, canonicalize, resolve};

fn roots(hangar: &str, claude: &str, codex: &str, home: &str) -> Roots {
    Roots { hangar: hangar.into(), claude: claude.into(), codex: codex.into(), home: home.into() }
}

fn src() -> Roots {
    roots("/home/ana/hangar", "/home/ana/.claude", "/home/ana/.codex", "/home/ana")
}

fn win() -> Roots {
    roots("C:\\hangar", "C:\\Users\\bia\\.claude", "C:\\Users\\bia\\.codex", "C:\\Users\\bia")
}

fn dst() -> Roots {
    roots("/opt/hangar", "/home/bia/.claude", "/home/bia/.codex", "/home/bia")
}

/// ATA-99 (`test_canonicalize_uses_most_specific_root`): cada raiz vira o próprio marcador, a mais
/// longa primeiro; CODEX também.
#[test]
fn canonicalize_marks_every_root_most_specific_first() {
    let command = "python3 /home/ana/.claude/hooks/x.py && node /home/ana/hangar/scripts/s.js /home/ana/.orca/h.sh";
    assert_eq!(canonicalize(command, &src()), "python3 ⟦CLAUDE⟧/hooks/x.py && node ⟦HANGAR⟧/scripts/s.js ⟦HOME⟧/.orca/h.sh");
    assert_eq!(canonicalize("/home/ana/.codex/config.toml", &src()), "⟦CODEX⟧/config.toml");
}

/// ATA-99 (`test_marcador_hangar_reconhece_current_e_versao`): o `current` do pacote e a pasta real
/// de uma versão em `~/.hangar/versions/<v>` são o mesmo `⟦HANGAR⟧`.
#[test]
fn canonicalize_package_current_and_version_are_hangar() {
    let package = roots("/h/.hangar/current", "/h/.claude", "/h/.codex", "/h");
    let text = "\"/h/.hangar/current/backend/hooks/a.py\" /h/.hangar/versions/X/scripts/b.py /h/x";
    assert_eq!(canonicalize(text, &package), "\"⟦HANGAR⟧/backend/hooks/a.py\" ⟦HANGAR⟧/scripts/b.py ⟦HOME⟧/x");
}

/// `test_canonicalize_windows_source_both_separators`.
#[test]
fn canonicalize_windows_source_both_separators() {
    assert_eq!(canonicalize("C:\\Users\\bia\\x.py C:/Users/bia/y.py", &win()), "⟦HOME⟧\\x.py ⟦HOME⟧/y.py");
}

/// `test_resolve_to_linux_target_turns_backslashes_into_slashes`.
#[test]
fn resolve_to_linux_target_turns_backslashes_into_slashes() {
    assert_eq!(
        resolve("⟦HOME⟧\\x.py '⟦CLAUDE⟧/hooks/a b.sh'", &dst()),
        "/home/bia/x.py '/home/bia/.claude/hooks/a b.sh'"
    );
}

/// `test_resolve_to_windows_target_uses_forward_slashes`.
#[test]
fn resolve_to_windows_target_uses_forward_slashes() {
    assert_eq!(resolve("⟦HOME⟧/.orca/h.sh", &win()), "C:/Users/bia/.orca/h.sh");
}

/// `test_resolve_leaves_shell_variables_alone`.
#[test]
fn resolve_leaves_shell_variables_alone() {
    assert_eq!(resolve("echo ${HOME} {HOME}", &dst()), "echo ${HOME} {HOME}");
}
