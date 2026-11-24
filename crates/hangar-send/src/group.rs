//! Grupo e par (`--pair`, `--unpair`, `--aceitar-par`, `--group`, `--contrato`) e o `--close`.
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::http::Endpoint;
use crate::{Ctx, fields, identity, list, missing, msys_drive_path, parse, path_segment, read_text_universal, text, truthy, unreadable};

const PAIR_USAGE: &str = "uso: hangar-send --pair <sessao|servidor::sessao> [tarefa...] [--substituir-tarefa] [--orq] | --pair --orq [tarefa...]";
const ACCEPT_USAGE: &str = "uso: hangar-send --aceitar-par <link https://….ts.net:8443/par/… ou https://…--guest.….hangar.dev.br/par/…>";
const CLOSE_USAGE: &str = "uso: hangar-send --close <sessao|servidor::sessao>";
const CONTRACT_USAGE: &str = "uso: hangar-send --contrato [--gravar <arquivo> [--mtime <valor da leitura>]]";
const GROUP_USAGE: &str = "uso: hangar-send --group [--tmux] <mensagem...>";

fn usage(err: &mut dyn Write, text: &str) -> u8 {
    let _ = writeln!(err, "{text}");
    2
}

fn object(body: &[u8]) -> Option<Map<String, Value>> {
    match parse(body)? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

pub(crate) fn pair(cx: &Ctx, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    // `--pair --orq [tarefa...]` sem sessão: grupo de orquestração só com esta.
    let (peer, rest) = match args.first().map(String::as_str) {
        Some("--orq") => ("", args),
        Some(peer) if !peer.is_empty() => (peer, &args[1..]),
        _ => return usage(err, PAIR_USAGE),
    };
    let (mut replace, mut orq, mut task) = (false, false, Vec::new());
    for arg in rest {
        match arg.as_str() {
            "--substituir-tarefa" => replace = true,
            "--orq" => orq = true,
            _ => task.push(arg.as_str()),
        }
    }
    let mut body = Map::new();
    body.insert("peer".into(), peer.into());
    body.insert("task".into(), task.join(" ").into());
    body.insert("replace_task".into(), replace.into());
    body.insert("orq".into(), orq.into());
    let me = identity::me(cx, false, err);
    let path = format!("/api/sessions/{}/pair", path_segment(&me));
    let response = match cx.api(&cx.local(), "POST", &path, Some(&Value::Object(body).to_string()), err) {
        Ok(response) => response,
        Err(code) => return code,
    };
    if peer.is_empty() {
        let Some(result) = object(&response) else { return unreadable(err, &path) };
        let gid = if truthy(result.get("gid")) { text(&result["gid"]) } else { String::new() };
        let _ = writeln!(out, "grupo de orquestração criado: {me}, sozinha, gid {gid} (ninguém recebe aviso; o time entra depois)");
    } else if orq {
        let _ = writeln!(out, "pareado: {me} <-> {peer} (grupo de orquestração: ninguém recebe aviso; o kick-off traz o protocolo)");
    } else {
        let _ = writeln!(out, "pareado: {me} <-> {peer} (registrado no app; quem estava sem grupo recebe o protocolo)");
    }
    0
}

pub(crate) fn accept(cx: &Ctx, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some(link) = args.first().filter(|link| !link.is_empty()) else { return usage(err, ACCEPT_USAGE) };
    let me = identity::me(cx, false, err);
    let mut body = Map::new();
    body.insert("link".into(), link.as_str().into());
    let path = format!("/api/sessions/{}/pair-accept", path_segment(&me));
    let Ok(response) = cx.api(&cx.local(), "POST", &path, Some(&Value::Object(body).to_string()), err) else { return 1 };
    let Some(result) = object(&response) else { return unreadable(err, &path) };
    let [owner, session, alias] = match fields(&result, &["owner", "session", "alias"]) {
        Ok(found) => [text(found[0]), text(found[1]), text(found[2])],
        Err(field) => return missing(err, &path, field),
    };
    let _ = writeln!(out, "pareada com {owner} (sessão {session}) — responda com: hangar-send {alias}::{session} \"…\"");
    0
}

pub(crate) fn unpair(cx: &Ctx, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let me = identity::me(cx, false, err);
    let path = format!("/api/sessions/{}/pair", path_segment(&me));
    if let Err(code) = cx.api(&cx.local(), "DELETE", &path, None, err) {
        return code;
    }
    let _ = writeln!(out, "despareado");
    0
}

pub(crate) fn close(cx: &Ctx, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some(label) = args.first().filter(|target| !target.is_empty()) else { return usage(err, CLOSE_USAGE) };
    let mut target = label.as_str();
    let mut remote = "";
    if let Some((server, session)) = label.split_once("::") {
        if server.is_empty() || session.is_empty() || session.contains("::") {
            let _ = writeln!(err, "erro: nome inválido '{label}': use servidor::sessao (um '::' só)");
            return 2;
        }
        target = session;
        // O id desta máquina fica aqui, senão <id>::eu escapava da recusa de fechar a si mesma.
        if server != cx.settings.server_id {
            remote = server;
        }
    }
    let endpoint = if remote.is_empty() {
        // Fechar a si mesma mata o processo no meio do comando; o carimbo segura quando a identidade cai em "cli".
        let me = identity::me(cx, false, err);
        if target == me || Some(target) == cx.var("CP_SESSION_NAME") {
            let _ = writeln!(err, "recusado: '{target}' é esta sessão — peça pra outra fechar");
            return 2;
        }
        cx.local()
    } else {
        let (base, token) = match cx.peer_config(remote, err) {
            Ok(found) => found,
            Err(code) => return code,
        };
        Endpoint { base, token, connect: 8.0, total: cx.total(), peer: Some(remote.to_owned()) }
    };
    // O DELETE responde ok para nome que não existe: nome errado não pode virar "fechada".
    let Ok(alive) = cx.api(&endpoint, "GET", "/api/sessions", None, err) else { return 1 };
    let Some(Value::Array(alive)) = parse(&alive) else { return unreadable(err, "/api/sessions") };
    if !alive.iter().any(|session| session.get("name").and_then(Value::as_str) == Some(target)) {
        let _ = writeln!(err, "erro: sessão '{label}' não existe (hangar-send --list)");
        return 1;
    }
    let response = match cx.api(&endpoint, "DELETE", &format!("/api/sessions/{}", path_segment(target)), None, err) {
        Ok(response) => response,
        Err(code) => {
            // Sem resposta, o fechamento pode ter acontecido lá: repetir só diria "não existe".
            if !remote.is_empty() && code == 2 {
                let _ = writeln!(err, "aviso: '{label}' pode ter fechado mesmo assim — confira com hangar-send --list");
            }
            return 1;
        }
    };
    let _ = writeln!(out, "fechada: {label}");
    // Fechou, mas desfazer o par remoto dela pode ter falhado: isso não some calado.
    if let Some(Value::Object(result)) = parse(&response)
        && let Some(warning) = result.get("warning").filter(|warning| hangar_core::pycompat::truthy(warning))
    {
        let shown = match warning {
            Value::String(message) => message.clone(),
            Value::Object(fields) if truthy(fields.get("msg")) => text(&fields["msg"]),
            other => hangar_core::pycompat::pyjson::dumps_unicode(other, false),
        };
        let _ = writeln!(err, "aviso: {shown}");
    }
    0
}

pub(crate) fn contract(cx: &Ctx, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let (mut write, mut mtime) = (None::<&str>, None::<&str>);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let slot = match arg.as_str() {
            "--gravar" => &mut write,
            "--mtime" => &mut mtime,
            other => {
                let _ = writeln!(err, "erro: argumento inesperado '{other}' — {CONTRACT_USAGE}");
                return 2;
            }
        };
        match rest.next().filter(|value| !value.is_empty()) {
            Some(value) => *slot = Some(value.as_str()),
            None => return usage(err, CONTRACT_USAGE),
        }
    }
    let me = identity::me(cx, false, err);
    let path = format!("/api/sessions/{}/pair/contract", path_segment(&me));
    let Some(file) = write else {
        if mtime.is_some() {
            let _ = writeln!(err, "erro: --mtime só vale com --gravar — {CONTRACT_USAGE}");
            return 2;
        }
        let Ok(response) = cx.api(&cx.local(), "GET", &path, None, err) else { return 1 };
        let Some(result) = object(&response) else { return unreadable(err, &path) };
        // Só o conteúdo no stdout: `hangar-send --contrato > c.md` sai pronto para editar e gravar.
        if truthy(result.get("content")) {
            let _ = out.write_all(text(&result["content"]).as_bytes());
        }
        if truthy(result.get("owner")) {
            let _ = writeln!(err, "# contrato guardado na máquina {}", text(&result["owner"]));
        }
        if truthy(result.get("path")) {
            let _ = writeln!(err, "# arquivo nesta máquina (pode editar direto): {}", text(&result["path"]));
        }
        if result.contains_key("mtime") {
            let mtime = if truthy(result.get("mtime")) { format!(" --mtime {}", text(&result["mtime"])) } else { String::new() };
            let _ = writeln!(err, "# para gravar: hangar-send --contrato --gravar <arquivo>{mtime}");
        }
        return 0;
    };
    let local = resolve(cx, &msys_drive_path(file, cfg!(windows)));
    if !local.is_file() {
        let _ = writeln!(err, "erro: arquivo '{file}' não existe");
        return 2;
    }
    let Some(content) = std::fs::read(&local).ok().and_then(|bytes| read_text_universal(&bytes)) else {
        let _ = writeln!(err, "erro: '{file}' não é texto UTF-8");
        return 2;
    };
    let mut body = Map::new();
    body.insert("content".into(), content.into());
    body.insert("mtime".into(), mtime.map_or(Value::Null, Value::from));
    let Ok(response) = cx.api(&cx.local(), "PUT", &path, Some(&Value::Object(body).to_string()), err) else { return 1 };
    let Some(result) = object(&response) else { return unreadable(err, &path) };
    let _ = writeln!(out, "contrato gravado (mtime {})", text(result.get("mtime").unwrap_or(&Value::Null)));
    0
}

/// Caminho relativo vale contra a pasta de quem chamou. O `PWD` lógico só quando é a pasta real do
/// processo (um `PWD` herdado de outro lugar mandaria outro arquivo); senão, a pasta real.
fn resolve(cx: &Ctx, file: &str) -> PathBuf {
    let path = Path::new(file);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let real = Some(cx.inv.cwd.clone()).filter(|cwd| !cwd.as_os_str().is_empty()).or_else(|| std::env::current_dir().ok());
    let pwd = cx.var("PWD").map(Path::new).filter(|pwd| pwd.is_absolute())
        .filter(|pwd| real.is_none() || std::fs::canonicalize(pwd).ok() == real.as_deref().and_then(|dir| std::fs::canonicalize(dir).ok()));
    match pwd.map(Path::to_path_buf).or(real) {
        Some(dir) => dir.join(path),
        None => path.to_path_buf(),
    }
}

pub(crate) fn group(cx: &Ctx, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let (force_tmux, words) = match args.first().map(String::as_str) {
        Some("--tmux") => (true, &args[1..]),
        _ => (false, args),
    };
    if words.is_empty() {
        return usage(err, GROUP_USAGE);
    }
    let me = identity::me(cx, false, err);
    let mut body = Map::new();
    body.insert("text".into(), words.join(" ").into());
    body.insert("remetente_nativo".into(), cx.var("CLAUDE_CODE_MESSAGING_SOCKET").is_some().into());
    body.insert("forcar_tmux".into(), force_tmux.into());
    let path = format!("/api/sessions/{}/group-message", path_segment(&me));
    let response = match cx.api(&cx.local(), "POST", &path, Some(&Value::Object(body).to_string()), err) {
        Ok(response) => response,
        Err(code) => return code,
    };
    let Some(result) = object(&response) else { return unreadable(err, &path) };
    // `pulados` = membros de máquina que não respondeu: o aviso não fica esperando por eles.
    let skipped: &[Value] = match result.get("pulados") {
        Some(Value::Array(items)) => items,
        _ => &[],
    };
    let peers = match result.get("peers") {
        Some(Value::Array(peers)) => peers,
        Some(_) => return unreadable(err, &path),
        None => return missing(err, &path, "peers"),
    };
    let delivered: Vec<String> = peers.iter().filter(|peer| !skipped.contains(peer)).map(text).collect();
    if !delivered.is_empty() {
        let _ = writeln!(out, "aviso enviado ao grupo: {}", delivered.join(", "));
    }
    if !skipped.is_empty() {
        let _ = writeln!(out, "  ⚠ sem o aviso (máquina fora do ar): {}", list::joined(&Value::Array(skipped.to_vec())));
    }
    if let Some(warning) = result.get("warning").filter(|warning| hangar_core::pycompat::truthy(warning)) {
        let shown = match warning {
            Value::Object(fields) => text(fields.get("msg").unwrap_or(warning)),
            other => text(other),
        };
        let _ = writeln!(out, "  ⚠ {shown}");
    }
    0
}
