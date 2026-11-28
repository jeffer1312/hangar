//! `--responder`: o `child_answer` do MCP pela linha de comando, para a sessão aberta antes de a tool existir. A rota
//! `child-answer` roda a mesma função da tool (quem chama pelos cabeçalhos de identidade, recusas, marca do pai e
//! `/answer`) e diz em `maybe_sent` se a resposta pode ter saído.
use std::io::Write;

use serde_json::{Value, json};

use crate::http::Failure;
use crate::{Ctx, identity, parse, path_segment};

const USAGE: &str = "uso: hangar-send --responder <filha> <interaction_id> <n.º da opção | \"texto:resposta livre\">... (uma resposta por item da pergunta)";
const MAY_HAVE_LEFT: &str = "a resposta pode ter saído: não repita; confira a filha (child_status ou --list)";

pub(crate) fn run(cx: &Ctx, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let [child, id, replies @ ..] = args else { return usage(err) };
    if child.is_empty() || id.is_empty() || replies.is_empty() { return usage(err); }
    if child.contains("::") {
        let _ = writeln!(err, "erro: --responder só alcança filha desta máquina");
        return 2;
    }
    // O servidor resolve quem chama pelos cabeçalhos; o cliente anexado é palpite e não responde por ninguém.
    let (me, attached) = identity::resolve(cx);
    if attached {
        let _ = writeln!(err, "erro: --responder não usa a identidade do cliente tmux anexado ('{me}'); rode dentro da sessão que abriu a filha, aberta pelo wrapper atualizado");
        return 2;
    }
    if me == "cli" {
        let _ = writeln!(err, "erro: --responder roda dentro da sessão que abriu a filha");
        return 2;
    }
    let body = json!({"interaction_id": id, "replies": replies}).to_string();
    let route = format!("/api/sessions/{}/child-answer", path_segment(child));
    let endpoint = cx.local();
    match cx.fetch_with(&endpoint, "POST", &route, Some(&body), &identity_headers(cx)) {
        Err(Failure::NoAnswer) => {
            let _ = writeln!(err, "erro: backend inacessível em {} (hangar rodando?)", endpoint.base);
            2
        }
        Err(Failure::Invalid(detail)) => {
            let _ = writeln!(err, "erro: {detail}");
            1
        }
        Err(Failure::Cut) => {
            let _ = writeln!(err, "erro: o servidor não respondeu no prazo; {MAY_HAVE_LEFT}");
            4
        }
        Ok((status, raw)) => outcome(child, status, &raw, out, err),
    }
}

/// Os cabeçalhos que o MCP lê para saber quem chama (`mcp::identity::resolve`).
fn identity_headers(cx: &Ctx) -> Vec<(&'static str, String)> {
    let key = cx.var("CP_SESSION_KEY").or_else(|| cx.var("HANGAR_CANO_KEY"));
    [("x-hangar-key", key), ("x-hangar-pane", cx.var("TMUX_PANE")), ("x-hangar-session", cx.var("CP_SESSION_NAME"))]
        .into_iter().filter_map(|(name, value)| Some((name, value?.to_owned()))).collect()
}

fn usage(err: &mut dyn Write) -> u8 {
    let _ = writeln!(err, "{USAGE}");
    2
}

/// 0 só para a resposta aceita; 1 só para a recusa que prova que nada saiu; o resto é 4.
fn outcome(child: &str, status: u16, raw: &[u8], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let value = parse(raw).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        let flag = |key: &str| value.get(key).and_then(Value::as_bool);
        if flag("ok") == Some(true) {
            let _ = writeln!(out, "respondida: {child}");
            return 0;
        }
        if flag("sent") == Some(true) && flag("confirmed") == Some(false) {
            let _ = writeln!(out, "enviada, sem confirmação: {child} (não tente de novo)");
            return 0;
        }
        let _ = writeln!(err, "erro: resposta sem confirmação de {child} ({}); {MAY_HAVE_LEFT}", String::from_utf8_lossy(raw).trim());
        return 4;
    }
    // Servidor sem o campo (antigo): só a recusa de quem nem abriu o pedido prova que nada saiu.
    let maybe_sent = value.get("maybe_sent").and_then(Value::as_bool).unwrap_or(!matches!(status, 401 | 403 | 404 | 422));
    let detail = value.get("detail");
    let code = detail.and_then(|d| d.get("code")).and_then(Value::as_str);
    let msg = detail.and_then(|d| d.get("msg")).and_then(Value::as_str);
    let said = match (code, msg) {
        (Some(code), Some(msg)) if msg.starts_with(code) => msg.to_owned(),
        (Some(code), Some(msg)) => format!("{code}: {msg}"),
        _ => format!("HTTP {status}: {}", String::from_utf8_lossy(raw).trim()),
    };
    if maybe_sent {
        let _ = writeln!(err, "erro: {said}; {MAY_HAVE_LEFT}");
        return 4;
    }
    let _ = writeln!(err, "erro: {said}");
    1
}
