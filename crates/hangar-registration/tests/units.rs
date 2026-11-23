//! As correções da migração, cada uma num teste que falharia no comportamento do Python, e as
//! entradas lidas de arquivo (`.env`, `.claude.json`, `config.toml`, cabeçalho do agente) sob proptest.
use std::collections::BTreeMap;
use std::path::Path;

use hangar_registration::{Invocation, agents_install, claude_md_block, mcp_headers, mcp_register, toml_string};
use proptest::prelude::*;

const BLOCK: &str = "<!-- claude-pocket:sessoes-irmas:start -->\n# Sessões-irmãs (hangar)\n- novo\n<!-- claude-pocket:sessoes-irmas:end -->";

fn invocation(home: &Path, stdin: &str) -> Invocation {
    Invocation {
        home: home.to_path_buf(),
        home_arg: None,
        root: Some(home.join("inst")),
        vars: BTreeMap::new(),
        stdin: stdin.as_bytes().to_vec(),
        windows: false,
    }
}

fn run(runner: fn(&Invocation, &mut dyn std::io::Write, &mut dyn std::io::Write) -> u8, invocation: &Invocation) -> (u8, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = runner(invocation, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn with_token(home: &Path, env: &str) {
    write(&home.join(".hangar/config/.env"), env);
}

/// L-30: sem a linha do token o script saía 1 sem dizer nada.
#[test]
fn headers_without_the_token_line_say_why() {
    let dir = tempfile::tempdir().unwrap();
    with_token(dir.path(), "CP_PORT=1\n");
    let (code, out, err) = run(mcp_headers::run, &invocation(dir.path(), ""));
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.starts_with("hangar-mcp-headers: CP_AUTH_TOKEN ausente em "), "{err}");
}

#[test]
fn headers_keep_the_session_key_before_the_cano_key() {
    let dir = tempfile::tempdir().unwrap();
    with_token(dir.path(), "CP_AUTH_TOKEN=t\n");
    let mut inv = invocation(dir.path(), "");
    inv.vars.insert("CP_SESSION_KEY".into(), String::new());
    inv.vars.insert("HANGAR_CANO_KEY".into(), "cano".into());
    inv.vars.insert("CP_SESSION_NAME".into(), "sessão".into());
    let (code, out, _) = run(mcp_headers::run, &inv);
    assert_eq!(code, 0);
    assert_eq!(out, "{\"Authorization\": \"Bearer t\", \"X-Hangar-Key\": \"cano\", \"X-Hangar-Session\": \"sess\\u00e3o\"}\n");
}

/// Um `"` no token não derruba o passo do Codex depois de gravar o Claude.
#[test]
fn token_with_quote_still_registers_codex_with_valid_toml() {
    let dir = tempfile::tempdir().unwrap();
    with_token(dir.path(), "CP_AUTH_TOKEN='a\"b\\c'\n");
    std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
    let (code, _, err) = run(mcp_register::run, &invocation(dir.path(), ""));
    assert_eq!(code, 0, "{err}");
    let text = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
    let doc: toml::Table = toml::from_str(&text).unwrap();
    assert_eq!(doc["mcp_servers"]["hangar"]["http_headers"]["Authorization"].as_str(), Some("Bearer a\"b\\c"));
}

/// Com `CLAUDE_CONFIG_DIR` a conta padrão lê o `.claude.json` de dentro da pasta: ele recebe o MCP junto
/// com o `~/.claude.json`, e nasce com ele quando o CLI ainda não o criou. Com `--home` a variável é do
/// processo, não da casa, e fica de fora.
#[test]
fn the_file_the_default_account_reads_gets_the_mcp() {
    for (home_arg, exists) in [(false, true), (false, false), (true, true)] {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        with_token(home, "CP_AUTH_TOKEN=t\n");
        if exists {
            write(&home.join(".claude/.claude.json"), "{\"oauthAccount\": {}}");
        } else {
            std::fs::create_dir_all(home.join(".claude")).unwrap();
        }
        let mut inv = invocation(home, "");
        inv.vars.insert("CLAUDE_CONFIG_DIR".into(), home.join(".claude").to_string_lossy().into_owned());
        if home_arg {
            inv.home_arg = Some(home.to_path_buf());
        }
        let (code, _, err) = run(mcp_register::run, &inv);
        assert_eq!(code, 0, "{err}");
        assert!(std::fs::read_to_string(home.join(".claude.json")).unwrap().contains("headersHelper"));
        let inner = std::fs::read_to_string(home.join(".claude/.claude.json")).unwrap_or_default();
        assert_eq!(inner.contains("headersHelper"), !home_arg, "home_arg={home_arg} exists={exists}: {inner}");
    }
}

/// Gravar não troca o link por um arquivo comum.
#[test]
fn linked_claude_json_and_codex_config_stay_links() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    with_token(home, "CP_AUTH_TOKEN=t\n");
    write(&home.join("dotfiles/claude.json"), "{}");
    write(&home.join("dotfiles/config.toml"), "a = 1\n");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(home.join("dotfiles/claude.json"), home.join(".claude.json")).unwrap();
        std::os::unix::fs::symlink(home.join("dotfiles/config.toml"), home.join(".codex/config.toml")).unwrap();
        let (code, _, err) = run(mcp_register::run, &invocation(home, ""));
        assert_eq!(code, 0, "{err}");
        assert!(std::fs::symlink_metadata(home.join(".claude.json")).unwrap().file_type().is_symlink());
        assert!(std::fs::symlink_metadata(home.join(".codex/config.toml")).unwrap().file_type().is_symlink());
        assert!(std::fs::read_to_string(home.join("dotfiles/claude.json")).unwrap().contains("headersHelper"));
        assert!(std::fs::read_to_string(home.join("dotfiles/config.toml")).unwrap().starts_with("a = 1\n\n# >>> hangar: mcp\n"));
    }
}

/// O `settings.json` principal linkado continua link e com o modo dele (o Python o trocava por
/// arquivo 0644).
#[cfg(unix)]
#[test]
fn linked_settings_json_stays_a_link_with_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    with_token(home, "CP_AUTH_TOKEN=t\n");
    let real = home.join("dotfiles/settings.json");
    write(&real, "{\"theme\": \"dark\"}");
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::os::unix::fs::symlink(&real, home.join(".claude/settings.json")).unwrap();
    let (code, _, err) = run(mcp_register::run, &invocation(home, ""));
    assert_eq!(code, 0, "{err}");
    assert!(std::fs::symlink_metadata(home.join(".claude/settings.json")).unwrap().file_type().is_symlink());
    assert!(std::fs::read_to_string(&real).unwrap().contains("mcp__hangar__send"));
    assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
}

/// A casa vem do USERPROFILE no Windows: o HOME=/c/Users/x do Git Bash viraria C:\c\Users\x.
#[test]
fn home_follows_the_account_rule() {
    let vars = BTreeMap::from([("HOME".to_owned(), "/c/Users/x".to_owned()), ("USERPROFILE".to_owned(), r"C:\Users\x".to_owned())]);
    let home = hangar_registration::home_of(&vars);
    if cfg!(windows) {
        assert_eq!(home, Path::new(r"C:\Users\x"));
    } else {
        assert_eq!(home, Path::new("/c/Users/x"));
    }
}

/// No Windows o Python em modo texto passava o CLAUDE.md inteiro a CRLF; agora o LF fica LF.
#[cfg(windows)]
#[test]
fn block_keeps_lf_on_windows() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".claude/CLAUDE.md");
    write(&file, "antes\n<!-- claude-pocket:sessoes-irmas:start -->\nvelho\n<!-- claude-pocket:sessoes-irmas:end -->\ndepois\n");
    let (code, _, err) = run(claude_md_block::run, &invocation(dir.path(), BLOCK));
    assert_eq!(code, 0, "{err}");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains('\r'), "{text:?}");
    assert_eq!(text, format!("antes\n{BLOCK}\ndepois\n"));
}

/// `.codex-*.lock` e conta adicional vazia não recebem config.toml.
#[test]
fn lock_and_empty_codex_accounts_get_no_config() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    with_token(home, "CP_AUTH_TOKEN=t\n");
    std::fs::create_dir_all(home.join(".codex-x.lock")).unwrap();
    std::fs::create_dir_all(home.join(".codex-vazia")).unwrap();
    write(&home.join(".codex-cheia/auth.json"), "{}");
    let (code, _, err) = run(mcp_register::run, &invocation(home, ""));
    assert_eq!(code, 0, "{err}");
    assert!(!home.join(".codex-x.lock/config.toml").exists());
    assert!(!home.join(".codex-vazia/config.toml").exists());
    assert!(home.join(".codex-cheia/config.toml").exists());
}

/// O config.toml em CRLF não é reescrito em LF.
#[test]
fn codex_config_keeps_crlf() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    with_token(home, "CP_AUTH_TOKEN=t\n");
    write(&home.join(".codex/config.toml"), "model = \"x\"\r\n");
    run(mcp_register::run, &invocation(home, ""));
    let text = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(text.contains("# >>> hangar: mcp\r\n"), "{text:?}");
    assert!(!text.replace("\r\n", "").contains('\n'), "{text:?}");
}

/// Pasta do Codex sem escrita é `erro:`, e as outras seguem.
#[cfg(unix)]
#[test]
fn codex_dir_without_write_is_an_error_line_and_the_rest_continues() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    with_token(home, "CP_AUTH_TOKEN=t\n");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    write(&home.join(".codex-b/config.toml"), "");
    std::fs::set_permissions(home.join(".codex"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let (code, _, err) = run(mcp_register::run, &invocation(home, ""));
    std::fs::set_permissions(home.join(".codex"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code, 1);
    assert!(err.contains("/.codex/config.toml: MCP hangar não registrado: [Errno 13]"), "{err}");
    assert!(!err.contains("Traceback"));
    assert!(std::fs::read_to_string(home.join(".codex-b/config.toml")).unwrap().contains("# >>> hangar: mcp"));
}

/// A conta com `.claude.json` cortado e o settings ilegível seguem saindo 0, como antes,
/// mas as linhas dizem `erro:` para o instalador contar a pendência; nada é gravado neles.
#[test]
fn skipped_account_files_are_error_lines_and_stay_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    with_token(home, "CP_AUTH_TOKEN=t\n");
    write(&home.join(".claude.json"), "{\"mcpServers\": {");
    write(&home.join(".claude/settings.json"), "{\"permissions\": ");
    let (code, _, err) = run(mcp_register::run, &invocation(home, ""));
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("erro: ") && err.contains(".claude.json não é JSON válido — pulado"), "{err}");
    assert!(err.contains("erro: ") && err.contains("settings.json ilegível"), "{err}");
    assert!(!err.contains("aviso: "), "{err}");
    assert_eq!(std::fs::read_to_string(home.join(".claude.json")).unwrap(), "{\"mcpServers\": {");
    assert_eq!(std::fs::read_to_string(home.join(".claude/settings.json")).unwrap(), "{\"permissions\": ");
}

/// Início sem fim deixa o arquivo igual e não diz ATUALIZADO.
#[test]
fn block_start_without_end_leaves_the_file_and_fails() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".claude/CLAUDE.md");
    let before = "antes\n<!-- claude-pocket:sessoes-irmas:start -->\nmeio\n";
    write(&file, before);
    let (code, out, err) = run(claude_md_block::run, &invocation(dir.path(), BLOCK));
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.starts_with("aviso: "), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

/// O CLAUDE.md global é gravado por temporário + rename. Quando o
/// temporário não pode ser escrito (aqui, um diretório no lugar dele), a troca falha com `erro:` e o
/// arquivo do usuário fica inteiro, na troca e no acréscimo.
#[test]
fn block_write_failure_leaves_the_global_claude_md_whole() {
    for before in ["antes\n<!-- claude-pocket:sessoes-irmas:start -->\nvelho\n<!-- claude-pocket:sessoes-irmas:end -->\ndepois\n", "só texto do usuário\n"] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".claude/CLAUDE.md");
        write(&file, before);
        std::fs::create_dir_all(dir.path().join(".claude/CLAUDE.md.hangar-novo")).unwrap();
        let (code, out, err) = run(claude_md_block::run, &invocation(dir.path(), BLOCK));
        assert_eq!((code, out.as_str()), (1, ""), "{err}");
        assert!(err.starts_with("erro: não consegui gravar "), "{err}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    }
}

/// O bloco trocado num CLAUDE.md em CRLF continua em CRLF.
#[test]
fn block_keeps_crlf_when_replacing_and_appending() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".claude/CLAUDE.md");
    write(&file, "a\r\n<!-- claude-pocket:sessoes-irmas:start -->\r\nvelho\r\n<!-- claude-pocket:sessoes-irmas:end -->\r\nb\r\n");
    let (code, out, _) = run(claude_md_block::run, &invocation(dir.path(), &format!("{BLOCK}\n")));
    assert_eq!(code, 0);
    assert!(out.contains("ATUALIZADO"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("a\r\n{}\r\nb\r\n", BLOCK.replace('\n', "\r\n")));
    write(&file, "x\r\n");
    run(claude_md_block::run, &invocation(dir.path(), BLOCK));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("x\r\n\r\n{}\r\n", BLOCK.replace('\n', "\r\n")));
}

#[test]
fn block_without_text_on_stdin_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _, err) = run(claude_md_block::run, &invocation(dir.path(), "\n"));
    assert_eq!(code, 2, "{err}");
    assert!(!dir.path().join(".claude/CLAUDE.md").exists());
}

/// O DEL sai escapado: cru, o Codex descartaria o arquivo do agente.
#[test]
fn agent_toml_escapes_del_and_parses() {
    let agent = agents_install::codex_agent("n", "d", "DEL[\u{7f}] ctrl[\u{1}] \"\"\" \\ 🚀\n");
    let doc: toml::Table = toml::from_str(&agent).unwrap();
    assert_eq!(doc["developer_instructions"].as_str(), Some("DEL[\u{7f}] ctrl[\u{1}] \"\"\" \\ 🚀\n"));
    assert!(agent.contains("\\u007f"));
}

/// Destino que é pasta vira frase, não traceback que derruba o instalador.
#[test]
fn agent_destination_that_is_a_directory_is_an_error_line() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    write(&home.join("inst/skills/s/agents/a.md"), "---\nname: a\n---\ncorpo\n");
    std::fs::create_dir_all(home.join(".claude/agents/a.md")).unwrap();
    let (code, _, err) = run(agents_install::run, &invocation(home, ""));
    assert_eq!(code, 1);
    assert!(err.starts_with("erro: ") && err.contains("agente a não instalado"), "{err}");
}

#[test]
fn settings_gains_only_the_missing_permissions_in_order() {
    let mut data = serde_json::json!({"permissions": {"allow": ["x", "mcp__hangar__group"]}});
    assert_eq!(mcp_register::with_permissions(&mut data), Some(true));
    assert_eq!(data["permissions"]["allow"], serde_json::json!(["x", "mcp__hangar__group", "mcp__hangar__send",
        "mcp__hangar__sessions", "mcp__hangar__who_am_i"]));
    assert_eq!(mcp_register::with_permissions(&mut data), Some(false));
    assert_eq!(mcp_register::with_permissions(&mut serde_json::json!({"permissions": []})), None);
}

/// Linhas de config.toml sem os marcadores do bloco (um início solto casaria com o fim do bloco novo,
/// como no Python).
fn toml_line() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("[mcp_servers.hangar]".to_owned()),
        Just("[mcp_servers.hangar.http_headers]".to_owned()),
        Just("[mcp_servers.node_repl]".to_owned()),
        Just("url = \"x\"".to_owned()),
        Just(String::new()),
        "[a-z]{1,5} = [0-9]{1,3}",
        "[a-z =\"\\[\\]]{0,20}",
    ]
}

proptest! {
    /// Qualquer texto vira uma string TOML que volta igual.
    #[test]
    fn toml_string_round_trips(text in any::<String>()) {
        let doc: toml::Table = toml::from_str(&format!("k = {}", toml_string(&text))).unwrap();
        prop_assert_eq!(doc["k"].as_str(), Some(text.as_str()));
    }

    /// O config do Codex sai com um único bloco marcado, e uma segunda passada não muda nada.
    #[test]
    fn codex_config_is_idempotent(lines in proptest::collection::vec(toml_line(), 0..12), newline in any::<bool>()) {
        let mut text = lines.join("\n");
        if newline { text.push('\n'); }
        let block = mcp_register::codex_block("http://127.0.0.1:8765/mcp/", "t");
        let once = mcp_register::codex_config(&text, &block);
        prop_assert_eq!(mcp_register::codex_config(&once, &block), once.clone());
        if once != text {
            prop_assert!(once.ends_with(&block));
            prop_assert_eq!(once.matches("# >>> hangar: mcp").count(), 1);
        }
    }

    /// O cabeçalho do agente nunca derruba o instalador, e o nome escrito volta.
    #[test]
    fn front_matter_reads_the_name(name in "[a-z][a-z0-9-]{0,12}", body in "(.|\n){0,40}") {
        let (fields, rest) = agents_install::front_matter(&format!("---\nname: {name}\n---\n{body}"));
        prop_assert_eq!(fields.get("name").map(String::as_str), Some(name.as_str()));
        prop_assert_eq!(rest, body.trim_start_matches('\n'));
    }

    /// Qualquer config.toml, marcadores incluídos: o resultado termina no bloco.
    #[test]
    fn codex_config_ends_with_the_block(text in any::<String>()) {
        let block = mcp_register::codex_block("http://127.0.0.1:8765/mcp/", "t");
        let updated = mcp_register::codex_config(&text, &block);
        prop_assert!(updated == text || updated.ends_with(&block));
    }

    #[test]
    fn front_matter_never_panics(text in any::<String>()) {
        let _ = agents_install::front_matter(&text);
    }

    /// Qualquer `.env` dá a linha JSON ou a frase da falta do token.
    #[test]
    fn headers_from_any_env(env in any::<String>()) {
        let dir = tempfile::tempdir().unwrap();
        with_token(dir.path(), &env);
        let (code, out, err) = run(mcp_headers::run, &invocation(dir.path(), ""));
        if code == 0 {
            let value: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
            prop_assert!(value["Authorization"].as_str().unwrap().starts_with("Bearer "));
        } else {
            prop_assert!(err.starts_with("hangar-mcp-headers: "), "{}", err);
        }
    }

    /// Qualquer `.claude.json` válido: objeto ganha a entrada, o resto é aviso ou erro com código.
    #[test]
    fn claude_json_of_any_shape(value in json_value()) {
        let dir = tempfile::tempdir().unwrap();
        with_token(dir.path(), "CP_AUTH_TOKEN=t\n");
        write(&dir.path().join(".claude.json"), &value.to_string());
        let (code, _, err) = run(mcp_register::run, &invocation(dir.path(), ""));
        let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join(".claude.json")).unwrap()).unwrap();
        let entry_ok = value.as_object().is_some_and(|map| map.get("mcpServers").is_none_or(serde_json::Value::is_object));
        if entry_ok {
            prop_assert_eq!(code, 0, "{}", err);
            prop_assert_eq!(after["mcpServers"]["hangar"]["timeout"].as_u64(), Some(660_000));
        } else {
            prop_assert_eq!(code, 1);
            prop_assert_eq!(after, value);
        }
    }
}

fn json_value() -> impl Strategy<Value = serde_json::Value> {
    use serde_json::json;
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(|b| json!(b)),
        any::<i64>().prop_map(|n| json!(n)),
        "[a-zA-Zçã🚀 ]{0,8}".prop_map(|s| json!(s)),
    ];
    leaf.prop_recursive(3, 16, 4, |inner| prop_oneof![
        proptest::collection::vec(inner.clone(), 0..4).prop_map(serde_json::Value::Array),
        proptest::collection::btree_map("[a-zA-Z]{1,6}", inner, 0..4)
            .prop_map(|map| serde_json::Value::Object(map.into_iter().collect())),
    ])
}
