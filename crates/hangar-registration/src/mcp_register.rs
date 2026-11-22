//! `hangar-server mcp-register`: o MCP `hangar` em cada conta do Claude (`mcpServers.hangar`, com
//! `headersHelper`: o token nunca entra no ambiente da sessão) e no `config.toml` de cada conta do
//! Codex (bloco marcado com `http_headers`), mais a liberação das ferramentas de recado no
//! `~/.claude/settings.json` principal. Idempotente; uma conta com problema não impede as outras.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hangar_core::orq_codec::codec;
use serde_json::{Value, json};

use crate::{Invocation, files};

const START: &str = "# >>> hangar: mcp";
const END: &str = "# <<< hangar: mcp";
/// A tool do cofre espera o dono por até 10 min; sem isso o Claude e o Codex cortam a chamada em 60 s.
const TOOL_TIMEOUT_S: u64 = 660;
/// Só as ferramentas de recado: `Bash(hangar-send:*)` ou `mcp__hangar__*` liberariam também criar
/// sessão em bypass, e uma sessão que pede aprovação escaparia por ela.
const PERMISSIONS: [&str; 4] = ["mcp__hangar__send", "mcp__hangar__group", "mcp__hangar__sessions", "mcp__hangar__who_am_i"];

pub fn cli(args: &[String]) -> ExitCode {
    crate::cli(args, "mcp-register", false, run)
}

pub fn run(invocation: &Invocation, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let env_path = files::env_file(invocation);
    let env = match files::read_env(&env_path) {
        Ok(env) => env,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = writeln!(err, "erro: .env não encontrado: {}", env_path.display());
            return 1;
        }
        Err(error) => {
            let _ = writeln!(err, "erro: não consegui ler {}: {}", env_path.display(), files::os_error(&error, &env_path));
            return 1;
        }
    };
    let Some(token) = env.get("cp_auth_token").filter(|token| !token.is_empty()) else {
        let _ = writeln!(err, "erro: CP_AUTH_TOKEN ausente no backend/.env");
        return 1;
    };
    let port = env.get("cp_port").filter(|port| !port.is_empty()).map_or("8765", String::as_str);
    let url = format!("http://127.0.0.1:{port}/mcp/");
    let home = invocation.target_home();
    // O Windows não executa o atalho sem extensão; o `.cmd` vem do instalador.
    let helper = home.join(".local").join("bin").join(if invocation.windows { "hangar-mcp-headers.cmd" } else { "hangar-mcp-headers" });
    let entry = json!({"type": "http", "url": url, "headersHelper": helper.to_string_lossy(), "timeout": TOOL_TIMEOUT_S * 1000});
    // Com `--home` a casa não é a do processo: o `CLAUDE_CONFIG_DIR` dele apontaria para fora dela.
    let config_dir = invocation.home_arg.is_none().then(|| invocation.vars.get("CLAUDE_CONFIG_DIR")).flatten();
    let claude_ok = claude(&home, config_dir.map(String::as_str), &entry, out, err);
    let settings_ok = claude_permissions(&home.join(".claude").join("settings.json"), out, err);
    let codex_ok = codex(&home, &codex_block(&url, token), out, err);
    if !claude_ok {
        let _ = writeln!(err, "erro: alguma conta do Claude ficou sem o MCP hangar (veja acima)");
    }
    if !codex_ok {
        let _ = writeln!(err, "erro: alguma conta do Codex ficou sem o MCP hangar (veja acima)");
    }
    u8::from(!(claude_ok && settings_ok && codex_ok))
}

/// `~/.claude.json`, o que a conta padrão lê com `CLAUDE_CONFIG_DIR` (se já existe) e cada
/// `~/.claude-*/.claude.json` que já existe; pasta `.lock` não é conta.
fn claude_accounts(home: &Path, config_dir: Option<&str>) -> Vec<PathBuf> {
    let mut accounts: Vec<PathBuf> = std::fs::read_dir(home).into_iter().flatten().flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(".claude-") && !name.ends_with(".lock")
        })
        .map(|entry| entry.path().join(".claude.json"))
        .filter(|file| file.is_file())
        .collect();
    accounts.sort();
    let main = home.join(".claude.json");
    let default = hangar_core::account_env::default_claude_json(home, config_dir);
    let same = |file: &Path| hangar_core::fs::same_file(file, &default);
    let listed = !default.is_file() || same(&main) || accounts.iter().any(|account| same(account));
    std::iter::once(main).chain((!listed).then_some(default)).chain(accounts).collect()
}

fn claude(home: &Path, config_dir: Option<&str>, entry: &Value, out: &mut dyn Write, err: &mut dyn Write) -> bool {
    let mut all_ok = true;
    for file in claude_accounts(home, config_dir) {
        if let Err(detail) = claude_account(&file, entry, out, err) {
            let _ = writeln!(err, "erro: {}: MCP hangar não registrado: {detail}", file.display());
            all_ok = false;
        }
    }
    all_ok
}

fn claude_account(file: &Path, entry: &Value, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), String> {
    // Link segue link: grava-se no arquivo de verdade, com o modo dele.
    let target = files::through_links(file);
    let exists = target.exists();
    let mut data = if exists {
        let bytes = std::fs::read(&target).map_err(|error| files::os_error(&error, file))?;
        let parsed = std::str::from_utf8(&bytes).ok().and_then(codec::loads);
        let Some(parsed) = parsed else {
            // Pular segue saindo 0, como antes; o `erro:` faz o instalador contar a conta como pendência.
            let _ = writeln!(err, "erro: {} não é JSON válido — pulado", file.display());
            return Ok(());
        };
        parsed
    } else {
        json!({})
    };
    // As frases do `AttributeError` de antes: o JSON é válido, mas não tem o formato de uma conta.
    let kind = files::py_type(&data);
    let Value::Object(map) = &mut data else {
        return Err(format!("'{kind}' object has no attribute 'setdefault'"));
    };
    let servers = map.entry("mcpServers").or_insert_with(|| json!({}));
    let kind = files::py_type(servers);
    let Value::Object(servers) = servers else {
        return Err(format!("'{kind}' object has no attribute 'get'"));
    };
    if servers.get("hangar") == Some(entry) {
        return Ok(());
    }
    servers.insert("hangar".into(), entry.clone());
    // O .claude.json guarda dados da conta: o modo do original fica, e o temporário nasce fechado.
    let mode = if exists { files::mode_of(&target).unwrap_or(0o600) } else { 0o600 };
    files::replace(&target, codec::dumps_indented_ascii(&data, 2).as_bytes(), mode)?;
    let _ = writeln!(out, "ok: MCP hangar em {}", file.display());
    Ok(())
}

/// Acrescenta as liberações que faltam em `permissions.allow`. `Some(true)` = mudou; `None` = formato
/// inesperado.
pub fn with_permissions(data: &mut Value) -> Option<bool> {
    let Value::Object(map) = data else { return None };
    let Value::Object(permissions) = map.entry("permissions").or_insert_with(|| json!({})) else { return None };
    let Value::Array(allow) = permissions.entry("allow").or_insert_with(|| json!([])) else { return None };
    let missing: Vec<Value> = PERMISSIONS.iter().map(|p| json!(p)).filter(|p| !allow.contains(p)).collect();
    let changed = !missing.is_empty();
    allow.extend(missing);
    Some(changed)
}

/// Só o settings.json principal: o espelho do `hangar-conta` leva às contas, e a cópia delas é refeita.
fn claude_permissions(file: &Path, out: &mut dyn Write, err: &mut dyn Write) -> bool {
    let mut data = if file.exists() {
        let parsed = std::fs::read(file).ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| codec::loads(&text));
        let Some(parsed) = parsed else {
            let _ = writeln!(err, "erro: {} ilegível — liberação do recado não gravada", file.display());
            return true;
        };
        parsed
    } else {
        json!({})
    };
    match with_permissions(&mut data) {
        None => {
            let _ = writeln!(err, "erro: {} tem permissions num formato inesperado — liberação não gravada", file.display());
            return true;
        }
        Some(false) => return true,
        Some(true) => {}
    }
    let written = (|| {
        let text = codec::dumps_indented_utf8(&data, 2).map_err(|detail| detail.to_owned())?;
        // Como o `.claude.json`: grava no alvo do link (o link fica) e mantém o modo do arquivo.
        let target = files::through_links(file);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|error| files::os_error(&error, dir))?;
        }
        let mode = files::mode_of(&target).unwrap_or(0o644);
        files::replace(&target, format!("{text}\n").as_bytes(), mode)
    })();
    match written {
        Ok(()) => {
            let _ = writeln!(out, "ok: recado pelo MCP hangar liberado em {}", file.display());
            true
        }
        Err(detail) => {
            let _ = writeln!(err, "erro: {}: liberação do recado não gravada: {detail}", file.display());
            false
        }
    }
}

/// O bloco marcado do Codex. Token e porta vão escapados: um `"` no token não quebra o TOML.
pub fn codex_block(url: &str, token: &str) -> String {
    [
        START,
        "[mcp_servers.hangar]",
        &format!("url = {}", files::toml_string(url)),
        &format!("tool_timeout_sec = {TOOL_TIMEOUT_S}"),
        &format!("http_headers = {{ \"Authorization\" = {} }}", files::toml_string(&format!("Bearer {token}"))),
        "env_http_headers = { \"X-Hangar-Key\" = \"CP_SESSION_KEY\", \"X-Hangar-Pane\" = \"TMUX_PANE\", \
         \"X-Hangar-Session\" = \"CP_SESSION_NAME\", \"X-Hangar-Jev\" = \"HANGAR_JEV\" }",
        END,
        "",
    ]
    .join("\n")
}

/// `~/.codex` e cada `~/.codex-*`. Pasta `.lock` e conta adicional vazia não são conta do Codex.
fn codex_homes(home: &Path) -> Vec<PathBuf> {
    let mut extra: Vec<PathBuf> = std::fs::read_dir(home).into_iter().flatten().flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(".codex-") && !name.ends_with(".lock")
        })
        .map(|entry| entry.path())
        .filter(|dir| dir.is_dir() && std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some()))
        .collect();
    extra.sort();
    let main = home.join(".codex");
    std::iter::once(main).filter(|dir| dir.is_dir()).chain(extra).collect()
}

fn codex(home: &Path, block: &str, out: &mut dyn Write, err: &mut dyn Write) -> bool {
    let mut all_ok = true;
    for dir in codex_homes(home) {
        let file = dir.join("config.toml");
        if let Err(detail) = codex_account(&file, block, out) {
            let _ = writeln!(err, "erro: {}: MCP hangar não registrado: {detail}", file.display());
            all_ok = false;
        }
    }
    all_ok
}

fn codex_account(file: &Path, block: &str, out: &mut dyn Write) -> Result<(), String> {
    let target = files::through_links(file);
    let raw = if target.exists() {
        std::fs::read(&target).map_err(|error| files::os_error(&error, file))?
    } else {
        Vec::new()
    };
    let raw = String::from_utf8(raw).map_err(|_| "o arquivo não é texto UTF-8".to_owned())?;
    // O app do Codex grava CRLF no Windows: a linha nova sai no mesmo fim de linha do arquivo.
    let crlf = raw.contains("\r\n");
    let text = files::universal_newlines(&raw);
    let updated = codex_config(&text, block);
    if updated == text {
        return Ok(());
    }
    let updated = if crlf { updated.replace('\n', "\r\n") } else { updated };
    // O config.toml guarda credencial: 0600 sempre.
    files::replace(&target, updated.as_bytes(), 0o600)?;
    let _ = writeln!(out, "ok: MCP hangar em {}", file.display());
    Ok(())
}

/// Config com um único `[mcp_servers.hangar]`: o do app, se já bater, senão o bloco marcado no fim.
pub fn codex_config(text: &str, block: &str) -> String {
    let unmarked = without_marked(text);
    if unmarked == text && codex_has_hangar(text, block) {
        return text.to_owned();
    }
    let base = without_unmarked(&unmarked);
    let base = base.trim_end_matches('\n');
    if base.is_empty() { block.to_owned() } else { format!("{base}\n\n{block}") }
}

/// Tira cada bloco entre os marcadores (com a quebra de linha depois do fim).
fn without_marked(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(START) {
        let Some(end) = rest[start + START.len()..].find(END) else { break };
        let mut stop = start + START.len() + end + END.len();
        if rest[stop..].starts_with('\n') {
            stop += 1;
        }
        out.push_str(&rest[..start]);
        rest = &rest[stop..];
    }
    out.push_str(rest);
    out
}

/// `[mcp_servers.hangar]` e `[mcp_servers.hangar.*]` sem marcador, cada um até a próxima seção: o app
/// do Codex reescreve o arquivo sem comentários e com os cabeçalhos em subtabelas. A seção na última
/// linha, sem quebra, também sai: ficando, o bloco novo a duplicaria e o Codex descartaria o arquivo.
fn without_unmarked(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut removing = false;
    for line in text.split_inclusive('\n') {
        if hangar_header(line) {
            removing = true;
            continue;
        }
        if removing && !line.starts_with('[') {
            continue;
        }
        removing = false;
        out.push_str(line);
    }
    out
}

fn hangar_header(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("[mcp_servers.hangar") else { return false };
    rest.starts_with(']') || rest.strip_prefix('.').is_some_and(|tail| tail.contains(']'))
}

/// O config já registra o MCP `hangar` com tudo o que o bloco declara, em qualquer formato TOML.
/// Conferir só URL e token deixaria passar um servidor sem os cabeçalhos de identidade.
fn codex_has_hangar(text: &str, block: &str) -> bool {
    fn hangar(text: &str) -> Option<toml::Table> {
        let mut doc: toml::Table = toml::from_str(text).ok()?;
        let toml::Value::Table(mut servers) = doc.remove("mcp_servers")? else { return None };
        match servers.remove("hangar")? {
            toml::Value::Table(server) => Some(server),
            _ => None,
        }
    }
    let (Some(server), Some(expected)) = (hangar(text), hangar(block)) else { return false };
    expected.iter().all(|(key, value)| server.get(key) == Some(value))
}
