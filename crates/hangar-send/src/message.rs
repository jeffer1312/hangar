//! Recado 1:1: nesta máquina, noutra do `peers.json` ou a um par externo. O transporte é do servidor;
//! aqui só se entrega e se diz o que aconteceu.
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::http::{Endpoint, Failure};
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
    // Lida uma vez: cada leitura custa até quatro chamadas ao tmux. O aviso só sai se o recado seguir.
    let (short, attached) = identity::resolve(cx);
    let server_id = cx.settings.server_id.as_str();
    if *target == short || (!server_id.is_empty() && *target == format!("{server_id}::{short}")) {
        let _ = writeln!(err, "recusado: '{target}' é esta sessão — o recado voltaria pra você. Quem é quem: hangar-send --list");
        return 1;
    }
    if attached {
        identity::warn_attached(&short, err);
    }
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
                // Servidor daqui fora: nada saiu, e quem chama pode tentar de novo.
                return if status == list::UNREACHABLE { 2 } else { 1 };
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
    // Id do servidor que recebe o recado: lá, `[de: home::x]` e `[de: x]` são a mesma sessão.
    let home = target.split_once("::").map_or(cx.settings.server_id.as_str(), |(server, _)| server);
    let started = unix_now();
    let fetched = cx.fetch(endpoint, "POST", &path, Some(&body));
    if matches!(fetched, Err(Failure::Cut)) {
        // Nunca um segundo POST: só a conversa do destino diz se o recado entrou.
        return match check_arrival(cx, endpoint, home, session, text, started) {
            Arrival::Arrived => {
                let _ = writeln!(out, "chegou -> {target} (conferido na conversa depois do corte)");
                0
            }
            // O corte veio depois de o pedido sair: o servidor pode gravar o recado depois da janela.
            Arrival::NotSeen => {
                let _ = writeln!(err, "erro: sem resposta de {} no prazo e o recado não apareceu na conversa em {} s; o servidor pode ainda estar processando — confira com hangar-send --list antes de reenviar", endpoint.base, cx.check_window());
                4
            }
            Arrival::Unknown => {
                maybe_delivered(endpoint, err);
                4
            }
        };
    }
    match crate::http::report(endpoint, fetched, err) {
        Ok(response) => report_delivery(&response, target, out, err),
        Err(code) => code,
    }
}

/// A conexão abriu e a resposta não veio: reenviar às cegas pode duplicar o recado.
pub(crate) fn maybe_delivered(endpoint: &Endpoint, err: &mut dyn Write) {
    let _ = match &endpoint.peer {
        Some(peer) => writeln!(err, "⚠ servidor '{peer}' não respondeu no prazo ({}) — {MAYBE_DELIVERED}", endpoint.base),
        None => writeln!(err, "erro: sem resposta de {} no prazo — {MAYBE_DELIVERED}", endpoint.base),
    };
}

pub(crate) const MAYBE_DELIVERED: &str = "o recado pode ter chegado; confira com hangar-send --list antes de reenviar";

/// O que a conversa do destino disse depois de um envio cortado.
enum Arrival {
    Arrived,
    NotSeen,
    Unknown,
}

/// Uma leitura da cauda da conversa.
enum Tail {
    Proof,
    Absent,
    Doubtful,
    Unreadable,
}

fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// Lê a conversa no mesmo servidor e com o mesmo token do envio, até a primeira prova ou o fim da
/// janela. Nada daqui afirma "não chegou": o servidor pode gravar o recado depois dela.
fn check_arrival(cx: &Ctx, endpoint: &Endpoint, home: &str, session: &str, text: &str, started: f64) -> Arrival {
    let path = format!("/api/sessions/{}/history?limit=40", path_segment(session));
    let deadline = Instant::now() + Duration::from_secs_f64(cx.check_window());
    let (mut seen, mut doubtful) = (false, false);
    loop {
        let round = Instant::now();
        let left = deadline.saturating_duration_since(round).as_secs_f64();
        if left <= 0.0 {
            break;
        }
        if let Ok((200, body)) = cx.fetch(&endpoint.with_total(left.min(5.0)), "GET", &path, None) {
            match read_tail(&body, text, home, started) {
                Tail::Proof => return Arrival::Arrived,
                Tail::Absent => seen = true,
                Tail::Doubtful => doubtful = true,
                Tail::Unreadable => {}
            }
        }
        let next = round + Duration::from_secs(1);
        if next >= deadline {
            break;
        }
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    if doubtful || !seen { Arrival::Unknown } else { Arrival::NotSeen }
}

/// Prova é o horário depois do envio (com folga para o relógio do outro lado), o da fala ou, na linha
/// da fila (`queued-…`), o da entrada nela. Igual sem horário pode ser um recado de antes: não prova nem nega.
fn read_tail(body: &[u8], sent: &str, home: &str, started: f64) -> Tail {
    let Some(Value::Array(events)) = parse(body) else { return Tail::Unreadable };
    if events.is_empty() {
        return Tail::Unreadable;
    }
    let mut untimed = false;
    for event in &events {
        let Value::Object(event) = event else { continue };
        if event.get("kind").and_then(Value::as_str) != Some("user_msg") {
            continue;
        }
        let Some(said) = event.get("text").and_then(Value::as_str) else { continue };
        if !same_message(said, sent, home) {
            continue;
        }
        let time = ["ts", "queued_ts"].into_iter().find_map(|key| event.get(key).and_then(Value::as_f64));
        match time {
            Some(ts) if ts >= started - 30.0 => return Tail::Proof,
            Some(_) => {}
            None => untimed = true,
        }
    }
    if untimed { Tail::Doubtful } else { Tail::Absent }
}

/// O mesmo recado, inclusive com o remetente escrito com ou sem o `home::` do servidor que o recebeu.
/// Outro remetente é outro recado, mesmo com o texto igual.
fn same_message(event_text: &str, sent: &str, home: &str) -> bool {
    let (said, sent) = (event_text.trim(), sent.trim());
    if said == sent {
        return true;
    }
    match (split_sender(said), split_sender(sent)) {
        (Some((a, said_body)), Some((b, sent_body))) => said_body == sent_body && local_name(a, home) == local_name(b, home),
        _ => false,
    }
}

fn split_sender(text: &str) -> Option<(&str, &str)> {
    let (sender, body) = text.strip_prefix("[de: ")?.split_once("] ")?;
    Some((sender.trim(), body.trim()))
}

fn local_name<'a>(sender: &'a str, home: &str) -> &'a str {
    match sender.split_once("::") {
        Some((server, name)) if !home.is_empty() && server == home => name,
        _ => sender,
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
        Err(Failure::NoAnswer) => {
            let _ = writeln!(err, "erro: backend inacessível em {} (hangar rodando?)", endpoint.base);
            return 2;
        }
        // O par externo não tem leitura da conversa daqui: fica incerto, sem reenviar.
        Err(Failure::Cut) => {
            maybe_delivered(&endpoint, err);
            return 4;
        }
        Err(Failure::Invalid(detail)) => {
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
