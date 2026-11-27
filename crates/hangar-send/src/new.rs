//! `--new`: cria uma sessão (aqui ou noutra máquina) ou retoma uma conversa (`--resume`).
use std::io::Write;

use serde_json::{Map, Value};

use crate::http::Endpoint;
use crate::{Ctx, identity, msys_drive_path, parse, text, truthy};

const USAGE: &str = "uso: hangar-send --new <nome> [cwd] [--engine <motor>] [--provider <claude|codex|pi|omp|kimi>] [--conta <nome>] [--model <id>] [--effort <nivel>] [--permissao <modo>] [--subagente <modelo>] [--service-tier <default|priority>] [--profile <nome>] [--headless|--terminal] [--jev|--sem-jev] [--code-mode|--sem-code-mode] [--read-only] [--inherit|--no-inherit] [--engine-account <id>] [--branch <nome> [--new-branch [--base <ref>]]] [--customizations <json>] [--prompt <texto>] [--remember-provider] [--link] [--forcar-conta] [--machine <id|auto>] [--resume <id da conversa>]";
/// Preparar conta e worktree passa dos 30 s; desistir antes dizia "inacessível" com a sessão nascendo.
/// Acima do pior caminho do servidor ao abrir noutra máquina: fatos, clone lá e abertura.
const CREATE_TIMEOUT: f64 = 900.0;

#[derive(Default)]
struct Options {
    cwd: String,
    engine: String,
    provider: String,
    account: String,
    config_dir: String,
    model: String,
    effort: String,
    permission: String,
    profile: String,
    read_only: bool,
    /// "true" (`--headless`), "false" (`--terminal`) ou vazio.
    headless: String,
    /// "true", "false" ou vazio (vale o padrão do servidor).
    jev: String,
    /// "true", "false" ou vazio (vale o padrão do servidor).
    code_mode: String,
    subagent: String,
    codex_account: String,
    service_tier: String,
    remember_provider: bool,
    /// "true" (`--no-inherit`), "mode" (`--inherit`) ou vazio.
    inherit: String,
    engine_account: String,
    branch: String,
    new_branch: bool,
    base: String,
    customizations: String,
    prompt: String,
    resume: String,
    link: bool,
    force_account: bool,
    /// Id da máquina onde abrir, ou `auto`; vazio = o servidor escolhe (`auto`).
    machine: String,
}

fn fail(err: &mut dyn Write, text: &str, code: u8) -> u8 {
    let _ = writeln!(err, "{text}");
    code
}

/// Valor obrigatório: ausente ou vazio é erro de uso, como o `${2:?}` do script.
fn required(value: Option<&str>, usage: &str, err: &mut dyn Write) -> Result<String, u8> {
    value.map(str::to_owned).ok_or_else(|| fail(err, usage, 2))
}

fn twice(flag: &str, err: &mut dyn Write) -> u8 {
    fail(err, &format!("erro: {flag} informado duas vezes"), 2)
}

/// As flags em qualquer ordem, com as mesmas recusas do script; `Err` = código já explicado.
fn parse_options(args: &[String], err: &mut dyn Write) -> Result<Options, u8> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let value = args.get(index + 1).map(String::as_str).filter(|value| !value.is_empty());
        let mut step = 2;
        match arg {
            "--no-inherit" | "--inherit" => {
                if !options.inherit.is_empty() {
                    return Err(fail(err, "erro: use só um entre --inherit e --no-inherit", 2));
                }
                options.inherit = if arg == "--inherit" { "mode".into() } else { "true".into() };
                step = 1;
            }
            // Nestas o valor é conferido antes da repetição, como o `once` do script.
            "--engine-account" | "--branch" | "--base" | "--customizations" | "--prompt" | "--resume" => {
                let usage = match arg {
                    "--engine-account" => "uso: --engine-account <id>",
                    "--branch" => "uso: --branch <nome>",
                    "--base" => "uso: --base <ref>",
                    "--customizations" => "uso: --customizations <json>",
                    "--prompt" => "uso: --prompt <texto>",
                    _ => "uso: --resume <id da conversa>",
                };
                let found = required(value, usage, err)?;
                let slot = match arg {
                    "--engine-account" => &mut options.engine_account,
                    "--branch" => &mut options.branch,
                    "--base" => &mut options.base,
                    "--customizations" => &mut options.customizations,
                    "--prompt" => &mut options.prompt,
                    _ => &mut options.resume,
                };
                if !slot.is_empty() {
                    return Err(twice(arg, err));
                }
                *slot = found;
            }
            "--new-branch" | "--link" | "--remember-provider" | "--read-only" | "--forcar-conta" => {
                let slot = match arg {
                    "--new-branch" => &mut options.new_branch,
                    "--link" => &mut options.link,
                    "--forcar-conta" => &mut options.force_account,
                    "--remember-provider" => &mut options.remember_provider,
                    _ => &mut options.read_only,
                };
                if *slot {
                    return Err(twice(arg, err));
                }
                *slot = true;
                step = 1;
            }
            "--jev" | "--sem-jev" => {
                if !options.jev.is_empty() {
                    return Err(twice("--jev/--sem-jev", err));
                }
                options.jev = if arg == "--jev" { "true".into() } else { "false".into() };
                step = 1;
            }
            "--code-mode" | "--sem-code-mode" => {
                if !options.code_mode.is_empty() {
                    return Err(twice("--code-mode/--sem-code-mode", err));
                }
                options.code_mode = if arg == "--code-mode" { "true".into() } else { "false".into() };
                step = 1;
            }
            "--headless" | "--terminal" => {
                if !options.headless.is_empty() {
                    return Err(twice("--headless/--terminal", err));
                }
                options.headless = if arg == "--headless" { "true".into() } else { "false".into() };
                step = 1;
            }
            "--service-tier" | "--subagente" | "--engine" | "--profile" | "--conta" | "--provider" | "--model" | "--effort" | "--permissao" | "--machine" => {
                let (slot, usage) = match arg {
                    "--service-tier" => (&mut options.service_tier, "uso: --service-tier <default|priority>"),
                    "--subagente" => (&mut options.subagent, "uso: --subagente <modelo>"),
                    "--engine" => (&mut options.engine, "uso: --engine <motor>"),
                    "--profile" => (&mut options.profile, "uso: --profile <nome>"),
                    "--conta" => (&mut options.account, "uso: --conta <nome>"),
                    "--provider" => (&mut options.provider, "uso: --provider <claude|codex|pi|omp|kimi|antigravity>"),
                    "--model" => (&mut options.model, "uso: --model <id>"),
                    "--effort" => (&mut options.effort, "uso: --effort <nivel>"),
                    "--machine" => (&mut options.machine, "uso: --machine <id|auto>"),
                    _ => (&mut options.permission, "uso: --permissao <acceptEdits|auto|bypassPermissions|manual|dontAsk|plan>"),
                };
                if !slot.is_empty() {
                    return Err(twice(arg, err));
                }
                *slot = required(value, usage, err)?;
            }
            // Token com '-' que não é flag é opção errada; como cwd, o erro apontaria o token seguinte.
            other if other.starts_with('-') => return Err(fail(err, &format!("erro: opção desconhecida '{other}'"), 2)),
            other => {
                if !options.cwd.is_empty() {
                    return Err(fail(err, &format!("erro: argumento inesperado '{other}'"), 2));
                }
                options.cwd = other.to_owned();
                step = 1;
            }
        }
        index += step;
    }
    Ok(options)
}

pub(crate) fn run(cx: &Ctx, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some(requested) = args.first().filter(|name| !name.is_empty()) else { return fail(err, USAGE, 2) };
    // servidor::nome cria a sessão NOUTRA máquina, pela API dela (peers.json), igual ao recado.
    let (remote, mut name) = match requested.split_once("::") {
        Some((server, rest)) => {
            if server.is_empty() || rest.is_empty() || rest.contains("::") {
                return fail(err, "erro: use servidor::nome (um '::' só)", 2);
            }
            (server.to_owned(), rest.to_owned())
        }
        None => (String::new(), requested.clone()),
    };
    // Nome com '-' na frente é uma flag que virou nome: `--new --engine kimi` criava a sessão "--engine".
    if name.starts_with('-') {
        let _ = writeln!(err, "erro: nome de sessão não pode começar com '-' (esqueceu o nome?)");
        return fail(err, USAGE, 2);
    }
    let mut options = match parse_options(&args[1..], err) {
        Ok(options) => options,
        Err(code) => return code,
    };
    if !remote.is_empty() && options.link {
        return fail(err, "erro: --link só vale nesta máquina (sem servidor::)", 2);
    }
    if !remote.is_empty() && !options.machine.is_empty() {
        return fail(err, "erro: use servidor::nome ou --machine, não os dois", 2);
    }
    let mut endpoint = cx.local();
    if !remote.is_empty() {
        let (base, token) = match cx.peer_config(&remote, err) {
            Ok(found) => found,
            Err(code) => return code,
        };
        endpoint = Endpoint { base, token, connect: 8.0, total: cx.total(), peer: None };
    }
    // Na abertura nova o provedor omitido é do servidor (o da criadora, o lembrado ou o padrão); só a retomada o pede.
    if options.provider.is_empty() {
        // --conta e --subagente sem --provider sempre foram Claude; o padrão lembrado não os reinterpreta.
        if !options.engine.is_empty() || !options.account.is_empty() || !options.subagent.is_empty() {
            options.provider = "claude".into();
        } else if !options.profile.is_empty() {
            options.provider = "omp".into();
        } else if !options.resume.is_empty() {
            match default_provider(cx, &endpoint, err) {
                Ok(provider) => options.provider = provider,
                Err(code) => return code,
            }
        }
    }
    // O `agy` com terminal ignora o `GEMINI_HOME`: `--conta` não escolhe nada, e nunca prepara conta Claude no lugar.
    if options.provider == "antigravity" && !options.account.is_empty() {
        return fail(err, "erro: --conta não vale para o Antigravity: ele só usa a conta padrão (a do ~/.gemini)", 2);
    }
    if options.provider == "codex" && !options.account.is_empty() {
        if options.account == "auto" {
            return fail(err, "erro: --conta auto só vale para Claude", 2);
        }
        options.codex_account = if options.account == "openai-codex" { "default".into() } else { options.account.clone() };
        options.account.clear();
    }
    if !remote.is_empty() {
        // Pasta padrão e conta vêm de LÁ: o servidor de lá expande o `~`.
        if options.cwd.is_empty() && options.resume.is_empty() {
            options.cwd = "~".into();
        }
        if !options.account.is_empty() && options.account != "auto" {
            match remote_account(cx, &endpoint, &remote, &options.account, err) {
                Ok(dir) => options.config_dir = dir,
                Err(code) => return code,
            }
        }
    }
    if !options.resume.is_empty() && !options.cwd.is_empty() {
        return fail(err, "erro: com --resume a pasta vem da conversa; tire o cwd", 2);
    }
    if options.resume.is_empty() && options.cwd.is_empty() {
        options.cwd = cx.var("HOME").map_or_else(|| cx.inv.home.to_string_lossy().into_owned(), str::to_owned);
    }
    options.cwd = msys_drive_path(&options.cwd, cfg!(windows));
    if (options.new_branch && options.branch.is_empty()) || (!options.base.is_empty() && !options.new_branch) {
        return fail(err, "erro: --new-branch pede --branch <nome>; --base só vale com --new-branch", 2);
    }
    let customizations = if options.customizations.is_empty() {
        None
    } else {
        match serde_json::from_str::<Value>(&options.customizations) {
            Ok(Value::Object(object)) => Some(Value::Object(object)),
            _ => return fail(err, "erro: --customizations precisa ser um objeto JSON", 2),
        }
    };
    if options.headless == "true" && !matches!(options.provider.as_str(), "" | "claude" | "codex" | "antigravity") {
        return fail(err, &format!("erro: --headless só vale com provider claude, codex ou antigravity (veio: {})", options.provider), 2);
    }
    // Sem conta com leitura de cota, para aqui em vez de cair calado na padrão (talvez a esgotada).
    if options.account == "auto" {
        let Ok(suggestion) = cx.api(&endpoint, "GET", "/api/cotas/sugestao", None, err) else {
            return fail(err, "erro: nenhuma conta com leitura de cota — escolha uma: hangar-conta --list", 2);
        };
        match quota_account(&suggestion) {
            Some((dir, label)) => {
                let _ = writeln!(err, "conta escolhida pela cota: {label}");
                options.config_dir = dir;
                options.account = label;
            }
            None => return fail(err, &format!("erro: resposta da cota ilegível em {} — escolha uma conta: --conta <nome>", endpoint.base), 2),
        }
    }
    if !options.account.is_empty() && options.config_dir.is_empty() {
        let prepared = cx.host.account_prep(&options.account);
        let _ = err.write_all(&prepared.stderr);
        if prepared.code != 0 {
            return fail(err, &format!("erro: conta '{}' indisponível — veja as configuradas com: hangar-conta --list", options.account), 2);
        }
        options.config_dir = String::from_utf8_lossy(&prepared.stdout).trim_end_matches('\n').to_owned();
    }
    // A sessão que roda o comando é a criadora; noutra máquina ela não existe, e "cli" é não saber.
    let mut creator = String::new();
    if remote.is_empty() {
        creator = identity::me(cx, false, err);
        if creator == "cli" {
            creator.clear();
        }
        if creator.is_empty() && (cx.var("TMUX").is_some() || cx.var("CP_SESSION_KEY").is_some() || cx.var("HANGAR_CANO_KEY").is_some()) {
            let _ = writeln!(err, "aviso: não identifiquei esta sessão; conta, modo de permissão e sem terminal não serão herdados");
        }
    }
    if !options.resume.is_empty() {
        return resume(cx, &endpoint, &remote, &name, &options, out, err);
    }
    // Sem servidor:: nem --machine, o servidor daqui escolhe a máquina; o modo do agente gravado nele diz se balanceia.
    if remote.is_empty() && options.machine.is_empty() {
        options.machine = "auto".into();
    }
    let body = creation_body(&name, &options, &creator, customizations);
    let created = match cx.api(&endpoint.with_total(CREATE_TIMEOUT), "POST", "/api/sessions", Some(body.as_str()), err) {
        Ok(created) => created,
        Err(_) => return 1,
    };
    let result = match parse(&created) {
        Some(Value::Object(result)) => Some(result),
        _ => None,
    };
    // O nome que vale é o que o servidor deu (ele limpa acento e espaço).
    if let Some(given) = result.as_ref().and_then(|result| result.get("name")).filter(|given| hangar_core::pycompat::truthy(given)) {
        name = text(given);
    }
    let inherited = match result.as_ref().and_then(|result| suffixes(result, &options.headless, err)) {
        Some(inherited) => inherited,
        None => {
            let _ = writeln!(err, "aviso: a criação foi confirmada, mas não consegui ler os avisos da resposta");
            String::new()
        }
    };
    if !remote.is_empty() {
        name = format!("{remote}::{name}");
    }
    // Aberta noutra máquina pela escolha: o nome que vale em recado e fechamento é o `alvo`.
    if let Some(alvo) = result.as_ref().and_then(|result| result.get("alvo")).and_then(Value::as_str).filter(|alvo| !alvo.is_empty()) {
        name = alvo.to_owned();
    }
    // O provedor que o servidor escolheu é o que a linha mostra; o herdado já sai no sufixo.
    if options.provider.is_empty() && let Some(result) = result.as_ref()
        && result.get("provider_source").and_then(Value::as_str) != Some("inherited") && truthy(result.get("provider")) {
        options.provider = text(&result["provider"]);
    }
    // Ligação pedida e resultado ilegível: não dá para dizer que ela vale.
    let (mut link_failed, mut link_unread) = (String::new(), false);
    if options.link {
        match result.as_ref() {
            None => link_unread = true,
            Some(result) => match result.get("link_error") {
                Some(error) if !hangar_core::pycompat::truthy(error) => {}
                None => {}
                Some(Value::Object(error)) => {
                    let code = if truthy(error.get("code")) { text(&error["code"]) } else { "?".into() };
                    let detail = if truthy(error.get("detail")) { text(&error["detail"]) } else { String::new() };
                    link_failed = format!("{code}: {detail}");
                }
                Some(_) => link_unread = true,
            },
        }
    }
    let _ = writeln!(out, "{}", created_line(&name, &options, &inherited));
    if let Some(line) = result.as_ref().and_then(placement_line) {
        let _ = writeln!(out, "{line}");
    }
    if link_unread {
        let _ = writeln!(err, "erro: a sessão {name} abriu, mas não consegui ler o resultado da ligação na resposta do servidor (confira com hangar-send --list)");
        return 1;
    }
    if !link_failed.is_empty() {
        let _ = writeln!(err, "erro: a sessão {name} abriu SEM a ligação — {link_failed}");
        return 1;
    }
    0
}

/// O primeiro provider marcado como padrão em `/api/providers`, senão claude.
fn default_provider(cx: &Ctx, endpoint: &Endpoint, err: &mut dyn Write) -> Result<String, u8> {
    let Ok(body) = cx.api(endpoint, "GET", "/api/providers", None, err) else { return Err(1) };
    let Some(Value::Object(providers)) = parse(&body) else { return Err(crate::unreadable(err, "/api/providers")) };
    for (provider, probe) in &providers {
        let Value::Object(probe) = probe else { return Err(crate::unreadable(err, "/api/providers")) };
        if truthy(probe.get("default")) {
            return Ok(provider.clone());
        }
    }
    Ok("claude".into())
}

/// A conta de lá pelo caminho, rótulo, nome da pasta ou `.claude-<nome>`: a primeira linha que bate,
/// a mesma regra do `remote_account` do MCP.
fn remote_account(cx: &Ctx, endpoint: &Endpoint, remote: &str, account: &str, err: &mut dyn Write) -> Result<String, u8> {
    let Ok(body) = cx.api(endpoint, "GET", "/api/claude-configs", None, err) else { return Err(1) };
    let Some(Value::Array(configs)) = parse(&body) else { return Err(crate::unreadable(err, "/api/claude-configs")) };
    let mut labels = Vec::new();
    for config in &configs {
        let Some(path) = config.get("path").and_then(Value::as_str) else { return Err(crate::unreadable(err, "/api/claude-configs")) };
        let folder = path.replace('\\', "/").rsplit('/').next().unwrap_or("").to_owned();
        let label = config.get("label");
        if account == path || label.and_then(Value::as_str) == Some(account) || account == folder || folder == format!(".claude-{account}") {
            return Ok(path.to_owned());
        }
        labels.push(label.map(text).unwrap_or_default());
    }
    Err(fail(err, &format!("erro: conta '{account}' não existe em '{remote}' — lá há: {}", labels.join(", ")), 2))
}

/// A conta sugerida pela cota: (caminho, rótulo), ou `None` se a resposta não os traz.
fn quota_account(body: &[u8]) -> Option<(String, String)> {
    let Some(Value::Object(suggestion)) = parse(body) else { return None };
    let path = suggestion.get("path")?.as_str().filter(|path| !path.is_empty())?;
    let label = suggestion.get("label")?.as_str().filter(|label| !label.is_empty())?;
    Some((path.to_owned(), label.to_owned()))
}

fn resume(cx: &Ctx, endpoint: &Endpoint, remote: &str, name: &str, options: &Options, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let id = options.resume.as_str();
    if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return fail(err, &format!("erro: id de conversa inválido: {id}"), 2);
    }
    let refused = [
        ("--effort", !options.effort.is_empty()),
        ("--permissao", !options.permission.is_empty()),
        ("--profile", !options.profile.is_empty()),
        ("--read-only", options.read_only),
        ("--jev/--sem-jev", !options.jev.is_empty()),
        ("--code-mode/--sem-code-mode", !options.code_mode.is_empty()),
        ("--subagente", !options.subagent.is_empty()),
        ("--service-tier", !options.service_tier.is_empty()),
        ("--inherit/--no-inherit", !options.inherit.is_empty()),
        ("--branch", !options.branch.is_empty()),
        ("--new-branch", options.new_branch),
        ("--base", !options.base.is_empty()),
        ("--customizations", !options.customizations.is_empty()),
        ("--prompt", !options.prompt.is_empty()),
        ("--remember-provider", options.remember_provider),
        ("--link", options.link),
        ("--forcar-conta", options.force_account),
        ("--machine", !options.machine.is_empty()),
    ];
    if let Some((flag, _)) = refused.iter().find(|(_, given)| *given) {
        return fail(err, &format!("erro: {flag} não combina com --resume"), 2);
    }
    let mut body = Map::new();
    body.insert("name".into(), name.into());
    body.insert("provider".into(), options.provider.as_str().into());
    for (key, value) in [("engine", &options.engine), ("config_dir", &options.config_dir), ("model", &options.model),
                         ("codex_account", &options.codex_account), ("engine_account", &options.engine_account)] {
        if !value.is_empty() {
            body.insert(key.into(), value.as_str().into());
        }
    }
    if !options.headless.is_empty() {
        body.insert("headless".into(), (options.headless == "true").into());
    }
    // A rota do Arquivo, com `-` no lugar do projeto: o servidor acha a pasta pela conversa.
    let path = format!("/api/archive/-/{id}/resume");
    let Ok(response) = cx.api(&endpoint.with_total(CREATE_TIMEOUT), "POST", &path, Some(&Value::Object(body).to_string()), err) else { return 1 };
    let result = match parse(&response) {
        Some(Value::Object(result)) => result,
        // A retomada aconteceu: o nome pedido vale, mas a pasta fica sem saber, e isso aparece.
        _ => {
            let _ = writeln!(err, "aviso: a retomada foi confirmada, mas não consegui ler a resposta (nome e pasta)");
            Map::new()
        }
    };
    let mut shown = if truthy(result.get("name")) { text(&result["name"]) } else { name.to_owned() };
    let cwd = if truthy(result.get("cwd")) { text(&result["cwd"]) } else { String::new() };
    if !remote.is_empty() {
        shown = format!("{remote}::{shown}");
    }
    let _ = writeln!(out, "sessão retomada: {shown} ({cwd}) na conversa {id} — fala com ela via: hangar-send {shown} \"msg\"");
    0
}

/// O corpo do `POST /api/sessions`: campo vazio some (ausente = padrão do servidor ou da criadora).
fn creation_body(name: &str, options: &Options, creator: &str, customizations: Option<Value>) -> String {
    let mut body = Map::new();
    let mut put = |key: &str, value: Value| {
        body.insert(key.into(), value);
    };
    put("name", name.into());
    put("cwd", options.cwd.as_str().into());
    put("engine", if options.engine.is_empty() { Value::Null } else { options.engine.as_str().into() });
    for (key, value) in [("provider", &options.provider), ("config_dir", &options.config_dir), ("model", &options.model),
                         ("effort", &options.effort), ("permission_mode", &options.permission), ("omp_profile", &options.profile)] {
        if !value.is_empty() {
            put(key, value.as_str().into());
        }
    }
    if options.read_only {
        put("read_only", true.into());
    }
    if !options.headless.is_empty() {
        put("headless", (options.headless == "true").into());
    }
    // Ausente, e não `false`: o campo ausente é o que faz o servidor aplicar o padrão dele.
    if !options.jev.is_empty() {
        put("jev", (options.jev == "true").into());
    }
    if !options.code_mode.is_empty() {
        put("code_mode", (options.code_mode == "true").into());
    }
    if !options.subagent.is_empty() {
        put("subagent_model", options.subagent.as_str().into());
    }
    if !options.codex_account.is_empty() {
        put("codex_account", options.codex_account.as_str().into());
    }
    if options.remember_provider {
        put("remember_provider", true.into());
    }
    if options.force_account {
        put("force_account", true.into());
    }
    if !creator.is_empty() {
        put("creator", creator.into());
    }
    if !options.service_tier.is_empty() {
        put("service_tier", options.service_tier.as_str().into());
    }
    match options.inherit.as_str() {
        "true" => put("inherit", false.into()),
        "mode" => put("inherit", true.into()),
        _ => {}
    }
    if !options.engine_account.is_empty() {
        put("engine_account", options.engine_account.as_str().into());
    }
    if !options.branch.is_empty() {
        put("branch", options.branch.as_str().into());
    }
    if options.new_branch {
        put("new_branch", true.into());
    }
    if !options.base.is_empty() {
        put("base", options.base.as_str().into());
    }
    if let Some(customizations) = customizations {
        put("claude_customizations", customizations);
    }
    if !options.prompt.is_empty() {
        put("initial_prompt", options.prompt.as_str().into());
    }
    if options.link {
        put("link", true.into());
    }
    if !options.machine.is_empty() {
        put("machine", options.machine.as_str().into());
    }
    Value::Object(body).to_string()
}

/// Os avisos da resposta no stderr e os sufixos de herança; `None` = resposta fora do formato.
fn suffixes(result: &Map<String, Value>, headless_flag: &str, err: &mut dyn Write) -> Option<String> {
    match result.get("avisos") {
        None => {}
        Some(Value::Array(warnings)) => {
            for warning in warnings {
                let _ = writeln!(err, "aviso: {}", warning.as_str()?);
            }
        }
        Some(_) => return None,
    }
    let field = |key: &str| if truthy(result.get(key)) { Some(text(&result[key])) } else { None };
    let source = |key: &str| result.get(key).and_then(Value::as_str);
    let mut parts = Vec::new();
    match source("provider_source") {
        Some("inherited") => parts.push(format!("agente herdado: {}", field("provider").unwrap_or_else(|| "?".into()))),
        Some("remembered" | "default") => parts.push(format!("agente padrão do servidor: {}", field("provider").unwrap_or_else(|| "?".into()))),
        _ => {}
    }
    if truthy(result.get("headless")) && headless_flag.is_empty() {
        parts.push("sem terminal".into());
    }
    match source("account_source") {
        Some("inherited") => parts.push(format!("conta herdada: {}", field("config_dir").unwrap_or_else(|| "a padrão".into()))),
        Some("quota") => parts.push(format!("conta trocada pela cota: {}", field("config_dir").unwrap_or_else(|| "a padrão".into()))),
        _ => {}
    }
    Some(parts.iter().map(|part| format!(" [{part}]")).collect())
}

/// Onde a escolha abriu e por quê; nada quando o pedido ficou fora do balanceamento.
fn placement_line(result: &Map<String, Value>) -> Option<String> {
    let placement = result.get("placement")?;
    if placement["reason"]["code"] == "not_balanced" {
        return None;
    }
    Some(format!("máquina: {} — {}", placement["server"].as_str()?, placement["text"].as_str().unwrap_or("")))
}

fn created_line(name: &str, options: &Options, inherited: &str) -> String {
    let mut line = format!("sessão criada: {name} ({})", options.cwd);
    if options.headless == "true" {
        line.push_str(" [sem terminal]");
    }
    line.push_str(inherited);
    for (label, value) in [("", &options.provider), ("motor: ", &options.engine), ("conta: ", &options.account),
                           ("conta codex: ", &options.codex_account), ("modelo: ", &options.model), ("esforço: ", &options.effort),
                           ("permissão: ", &options.permission), ("subagentes: ", &options.subagent),
                           ("service tier: ", &options.service_tier), ("perfil: ", &options.profile), ("jev: ", &options.jev)] {
        if !value.is_empty() {
            line.push_str(&format!(" [{label}{value}]"));
        }
    }
    line.push_str(&format!(" — fala com ela via: hangar-send {name} \"msg\""));
    line
}
