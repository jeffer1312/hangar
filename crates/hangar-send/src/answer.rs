//! `--responder`: o `child_answer` do MCP pela linha de comando, para a sessão aberta antes de a tool existir. A rota
//! `child-answer` roda a mesma função da tool: recusas, marca do pai e `/answer`.
use std::io::Write;

use serde_json::{Value, json};

use crate::http::{self, Failure};
use crate::{Ctx, identity, parse, path_segment, unreadable};

const USAGE: &str = "uso: hangar-send --responder <filha> <interaction_id> <n.º da opção | \"texto\">... (uma resposta por item da pergunta)";

pub(crate) fn run(cx: &Ctx, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let [child, id, replies @ ..] = args else { return usage(err) };
    if child.is_empty() || id.is_empty() || replies.is_empty() { return usage(err); }
    if child.contains("::") {
        let _ = writeln!(err, "erro: --responder só alcança filha desta máquina");
        return 2;
    }
    // O servidor confere se a filha é ligada a quem assina: fora de sessão não há pai.
    let me = identity::me(cx, false, err);
    if me == "cli" {
        let _ = writeln!(err, "erro: --responder roda dentro da sessão que abriu a filha");
        return 2;
    }
    let body = json!({"parent": me, "interaction_id": id, "replies": replies}).to_string();
    let (endpoint, route) = (cx.local(), format!("/api/sessions/{}/child-answer", path_segment(child)));
    match cx.fetch(&endpoint, "POST", &route, Some(&body)) {
        Err(Failure::Cut) => {
            let _ = writeln!(err, "erro: o servidor não respondeu no prazo e a resposta pode ter saído; confira a filha (child_status ou --list) antes de repetir");
            4
        }
        Ok((status, raw)) if !(200..300).contains(&status) => refused(status, &raw, err),
        fetched => match http::report(&endpoint, fetched, err) {
            Err(code) => code,
            Ok(raw) => answered(child, &raw, &route, out, err),
        },
    }
}

fn usage(err: &mut dyn Write) -> u8 {
    let _ = writeln!(err, "{USAGE}");
    2
}

fn answered(child: &str, raw: &[u8], route: &str, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some(value) = parse(raw) else { return unreadable(err, route) };
    let _ = if value.get("confirmed") == Some(&Value::Bool(false)) {
        writeln!(out, "enviada, sem confirmação: {child} (não tente de novo)")
    } else {
        writeln!(out, "respondida: {child}")
    };
    0
}

/// A recusa com o código do servidor; a frase da tool já começa por ele.
fn refused(status: u16, raw: &[u8], err: &mut dyn Write) -> u8 {
    let detail = parse(raw).and_then(|value| value.get("detail").cloned());
    let code = detail.as_ref().and_then(|d| d.get("code")).and_then(Value::as_str);
    let msg = detail.as_ref().and_then(|d| d.get("msg")).and_then(Value::as_str);
    let _ = match (code, msg) {
        (Some(code), Some(msg)) if msg.starts_with(code) => writeln!(err, "erro: {msg}"),
        (Some(code), Some(msg)) => writeln!(err, "erro: {code}: {msg}"),
        _ => writeln!(err, "erro HTTP {status}: {}", String::from_utf8_lossy(raw)),
    };
    1
}
