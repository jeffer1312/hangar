//! `--list`: as sessões daqui, as das máquinas do `peers.json` e os pares externos, nas colunas que o
//! cartão do app lê (`hangarCmd.ts`).
use std::io::Write;
use std::path::Path;

use serde_json::{Map, Value};

use crate::http::Endpoint;
use crate::{Ctx, fields, identity, missing, parse, text, truthy, unreadable};

const SESSIONS: &str = "/api/sessions";
const EXTERNAL: &str = "/api/external-pairs";

pub(crate) fn run(cx: &Ctx, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let body = match cx.api(&cx.local(), "GET", SESSIONS, None, err) {
        Ok(body) => body,
        Err(code) => return code,
    };
    let Some(sessions) = objects(&body) else { return unreadable(err, SESSIONS) };
    let me = identity::me(cx, true, err);
    for session in &sessions {
        let [name, state, cwd] = match fields(session, &["name", "state", "cwd"]) {
            Ok(found) => [found[0], found[1], found[2]],
            Err(field) => return missing(err, SESSIONS, field),
        };
        let you = if name.as_str() == Some(me.as_str()) { "  (você)" } else { "" };
        let _ = writeln!(out, "{:<23}  {:<14}  {:<17}  {}{you}", text(name), text(state), harness(session), text(cwd));
    }
    // Entrada e saída não são avisadas: é aqui que a sessão vê com quem está.
    if let Some(mine) = sessions.iter().find(|session| session.get("name").and_then(Value::as_str) == Some(me.as_str()))
        && truthy(mine.get("pair_peers"))
    {
        let owner = if truthy(mine.get("pair_owner")) { format!(" (guardado na máquina {})", text(&mine["pair_owner"])) } else { String::new() };
        let _ = writeln!(out, "# seu grupo: {}{owner}", joined(&mine["pair_peers"]));
    }
    scan_peers(cx, out, err);
    external_pairs(cx, out, err)
}

/// `<provider ou ?>[:<motor>]/<headless|tmux>`.
pub(crate) fn harness(session: &Map<String, Value>) -> String {
    let provider = if truthy(session.get("provider")) { text(&session["provider"]) } else { "?".into() };
    let engine = if truthy(session.get("engine")) { format!(":{}", text(&session["engine"])) } else { String::new() };
    let mode = if truthy(session.get("headless")) { "headless" } else { "tmux" };
    format!("{provider}{engine}/{mode}")
}

/// `", ".join(...)` de uma lista do JSON.
pub(crate) fn joined(value: &Value) -> String {
    match value {
        Value::Array(items) => items.iter().map(text).collect::<Vec<_>>().join(", "),
        other => text(other),
    }
}

fn objects(body: &[u8]) -> Option<Vec<Map<String, Value>>> {
    match parse(body)? {
        Value::Array(items) => items.into_iter().map(|item| match item {
            Value::Object(object) => Some(object),
            _ => None,
        }).collect(),
        _ => None,
    }
}

/// Uma máquina por pedido, todas juntas; a saída volta na ordem do `peers.json`.
fn scan_peers(cx: &Ctx, out: &mut dyn Write, err: &mut dyn Write) {
    let found = cx.host.peers(&["path"]);
    let path = String::from_utf8_lossy(&found.stdout).trim_end_matches(['\n', '\r']).to_owned();
    if found.code != 0 || !Path::new(&path).is_file() {
        let _ = err.write_all(&found.stderr);
        return;
    }
    let listed = cx.host.peers(&["tsv"]);
    let _ = err.write_all(&listed.stderr);
    let rows = if listed.code == 0 {
        String::from_utf8_lossy(&listed.stdout).into_owned()
    } else {
        let _ = writeln!(err, "{path} inválido — servidores remotos ignorados");
        String::new()
    };
    let peers: Vec<Endpoint> = rows.lines().filter_map(|row| {
        let mut parts = row.trim_matches('\t').splitn(3, '\t');
        let id = parts.next().filter(|id| !id.is_empty())?;
        Some(Endpoint {
            base: parts.next().unwrap_or("").to_owned(),
            token: parts.next().unwrap_or("").trim_start_matches('\t').to_owned(),
            connect: cx.connect(),
            total: 8.0,
            peer: Some(id.to_owned()),
        })
    }).collect();
    let answers = cx.runtime.block_on(futures_util::future::join_all(
        peers.iter().map(|peer| crate::http::fetch(peer, "GET", SESSIONS, None)),
    ));
    for (peer, fetched) in peers.iter().zip(answers) {
        let mut lines = Vec::new();
        let mut warnings = Vec::new();
        if let Ok(body) = crate::http::report(peer, fetched, &mut warnings) {
            peer_lines(peer, &body, &mut lines, &mut warnings);
        }
        let _ = out.write_all(&lines);
        let _ = err.write_all(&warnings);
    }
}

fn peer_lines(peer: &Endpoint, body: &[u8], out: &mut dyn Write, err: &mut dyn Write) {
    // Máquina que respondeu algo ilegível não derruba a lista: a linha dela só não sai.
    let Some(sessions) = objects(body) else { return };
    let id = peer.peer.as_deref().unwrap_or("");
    for session in &sessions {
        let [name, state, cwd] = match fields(session, &["name", "state", "cwd"]) {
            Ok(found) => [found[0], found[1], found[2]],
            Err(field) => {
                let _ = writeln!(err, "⚠ servidor '{id}' respondeu sem o campo '{field}' ({})", peer.base);
                return;
            }
        };
        let address = format!("{id}::{}", text(name));
        let _ = writeln!(out, "{address:<23}  {:<14}  {:<17}  {}", text(state), harness(session), text(cwd));
    }
}

/// O motivo de `external_get` quando o servidor daqui não atendeu.
pub(crate) const UNREACHABLE: &str = "backend inacessível";

/// Pares externos do servidor daqui: (`corpo`, `None`) ou o motivo da falha.
pub(crate) fn external_get(cx: &Ctx) -> Result<Vec<u8>, String> {
    let endpoint = Endpoint { connect: 3.0, total: 8.0, ..cx.local() };
    let (status, body) = match cx.fetch(&endpoint, "GET", EXTERNAL, None) {
        Ok(answer) => answer,
        Err(crate::http::Failure::NoAnswer | crate::http::Failure::Cut) => return Err(UNREACHABLE.into()),
        Err(crate::http::Failure::Invalid(detail) | crate::http::Failure::TooLarge(detail)) => return Err(detail),
    };
    if (200..300).contains(&status) {
        return Ok(body);
    }
    // Servidor sem a rota (mais antigo) não tem par nenhum.
    if status == 405 || (status == 404 && !String::from_utf8_lossy(&body).contains("\"code\"")) {
        return Ok(b"[]".to_vec());
    }
    Err(format!("HTTP {status}"))
}

fn external_pairs(cx: &Ctx, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let body = external_get(cx).unwrap_or_else(|status| {
        let _ = writeln!(err, "⚠ pares externos indisponíveis ({status}): a lista pode estar incompleta");
        b"[]".to_vec()
    });
    let Some(Value::Array(pairs)) = parse(&body) else { return 0 };
    for pair in &pairs {
        let Value::Object(pair) = pair else { return unreadable(err, EXTERNAL) };
        let [alias, session, local, owner] = match fields(pair, &["alias", "session", "local_session", "owner"]) {
            Ok(found) => [found[0], found[1], found[2], found[3]],
            Err(field) => return missing(err, EXTERNAL, field),
        };
        let address = format!("{}::{}", text(alias), text(session));
        let _ = writeln!(out, "{address:<23}  par externo de {} (máquina {})", text(local), text(owner));
    }
    0
}
