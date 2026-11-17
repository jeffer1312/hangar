//! Recado 1:1: nesta máquina, noutra do `peers.json` ou a um par externo. O transporte é do servidor;
//! aqui só se entrega e se diz o que aconteceu.
use std::io::Write;

use serde_json::{Map, Value, json};

use crate::http::Endpoint;
use crate::{Ctx, identity, list, parse, path_segment, truthy};

const USAGE: &str = "uso: hangar-send [--tmux] <sessao|servidor::sessao> <mensagem...>";

pub(crate) fn run(cx: &Ctx, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    // `--tmux` fica aceito e é ignorado: o orq e o vigia ainda o mandam.
    let args = match args.first().map(String::as_str) {
        Some("--tmux") => &args[1..],
        _ => args,
    };
    let Some((target, words)) = args.split_first() else {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    };
    // Flag errada virava nome de sessão e o recado ia para "--foo".
    if target.starts_with('-') {
        let _ = writeln!(err, "erro: opção desconhecida '{target}' (veja hangar-send --help)");
        return 2;
    }
    if words.is_empty() {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    }
    let message = words.join(" ");
    if cx.inv.vars.get("HANGAR_SEND_PAINEL").map(String::as_str) == Some("1") {
        // Com "[de: X]" na frente, o destino responderia a X achando que foi X quem escreveu.
        if !message.starts_with("[painel:") {
            let _ = writeln!(err, "erro: HANGAR_SEND_PAINEL=1 exige texto começando com [painel: …]");
            return 2;
        }
        if target.contains("::") {
            let _ = writeln!(err, "erro: aviso de painel só vai a sessão desta máquina");
            return 2;
        }
        return deliver(cx, &cx.local(), target, target, &message, out, err);
    }
    let me = identity::me(cx, true, err);
    let server_id = cx.settings.server_id.as_str();
    if *target == me || (!server_id.is_empty() && *target == format!("{server_id}::{me}")) {
        let _ = writeln!(err, "recusado: '{target}' é esta sessão — o recado voltaria pra você. Quem é quem: hangar-send --list");
        return 1;
    }
    let short = identity::me(cx, false, err);
    let mut sender = short.clone();
    let mut endpoint = cx.local();
    let mut session = target.as_str();
    if let Some((server, rest)) = target.split_once("::") {
        if server.is_empty() || rest.is_empty() || rest.contains("::") {
            let _ = writeln!(err, "erro: nome inválido '{target}': use servidor::sessao (um '::' só)");
            return 2;
        }
        session = rest;
        // Par externo primeiro: só o servidor daqui sabe chamar a máquina de outra pessoa.
        let pairs = match list::external_get(cx) {
            Ok(body) => body,
            Err(status) => {
                let _ = writeln!(err, "erro: não consegui consultar os pares externos ({status})");
                return 1;
            }
        };
        if is_external_alias(&pairs, server) {
            let text = strip_sender(&message, &[short.as_str()]);
            return send_external(cx, target, &short, &text, out, err);
        }
        if server_id.is_empty() {
            let _ = writeln!(err, "erro: CP_SERVER_ID ausente no backend/.env — obrigatório pra envio cross-server (endereço de resposta [de: <id>::{short}])");
            return 1;
        }
        let (base, token) = match cx.peer_config(server, err) {
            Ok(found) => found,
            Err(code) => return code,
        };
        endpoint = Endpoint { base, token, ..endpoint };
        sender = format!("{server_id}::{short}");
    }
    // O modelo que já escreveu "[de: eu] …" não ganha o prefixo em dobro.
    let text = format!("[de: {sender}] {}", strip_sender(&message, &[sender.as_str(), short.as_str()]));
    deliver(cx, &endpoint, target, session, &text, out, err)
}

fn strip_sender(message: &str, senders: &[&str]) -> String {
    let mut text = message.to_owned();
    for sender in senders {
        for prefix in [format!("[de: {sender}] "), format!("[de: {sender}]")] {
            if let Some(rest) = text.strip_prefix(&prefix) {
                text = rest.to_owned();
            }
        }
    }
    text
}

fn is_external_alias(body: &[u8], alias: &str) -> bool {
    let Some(Value::Array(pairs)) = parse(body) else { return false };
    for pair in &pairs {
        let Value::Object(pair) = pair else { return false };
        if pair.get("alias").and_then(Value::as_str) == Some(alias) {
            return true;
        }
    }
    false
}

fn deliver(cx: &Ctx, endpoint: &Endpoint, target: &str, session: &str, text: &str, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let body = json!({"text": text, "steer": true}).to_string();
    let path = format!("/api/sessions/{}/input", path_segment(session));
    match cx.api(endpoint, "POST", &path, Some(&body), err) {
        Ok(response) => report_delivery(&response, target, out, err),
        Err(code) => code,
    }
}

fn send_external(cx: &Ctx, target: &str, sender: &str, text: &str, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let mut body = Map::new();
    body.insert("sender".into(), sender.into());
    body.insert("target".into(), target.into());
    body.insert("text".into(), text.into());
    let endpoint = Endpoint { connect: 3.0, total: 30.0, ..cx.local() };
    let (status, response) = match cx.fetch(&endpoint, "POST", "/api/external-pairs/send", Some(&Value::Object(body).to_string())) {
        Ok(answer) => answer,
        Err(crate::http::Failure::NoAnswer) => {
            let _ = writeln!(err, "erro: backend inacessível em {} (hangar rodando?)", endpoint.base);
            return 1;
        }
        Err(crate::http::Failure::Invalid(detail)) => {
            let _ = writeln!(err, "erro: {detail}");
            return 1;
        }
    };
    if (200..300).contains(&status) {
        return report_delivery(&response, target, out, err);
    }
    let _ = write!(err, "erro HTTP {status}: ");
    let _ = err.write_all(&response);
    let _ = err.write_all(b"\n");
    1
}

/// O que o servidor fez com o recado; resposta ilegível não vira "na fila".
pub(crate) fn report_delivery(response: &[u8], target: &str, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some(Value::Object(result)) = parse(response) else {
        let _ = writeln!(err, "erro: resposta ilegível do envio para {target} — não dá para dizer se foi entregue; confira com hangar-send --list");
        return 1;
    };
    let line = if truthy(result.get("native")) {
        format!("entregue agora -> {target} (no socket do Claude dele, entra no meio do turno; retenção ou recusa pela política de lá chega depois como aviso [painel: entrega de recado])")
    } else if truthy(result.get("steered")) {
        format!("entregue agora -> {target}")
    } else if truthy(result.get("delivered")) {
        format!("entregue -> {target} (não orientado no turno atual)")
    } else {
        format!("na fila -> {target} (não orientado; entrega no próximo idle)")
    };
    let _ = writeln!(out, "{line}");
    0
}
