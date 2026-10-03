//! O que o cano observa nos dois sentidos para montar o snapshot, e as linhas que ele mesmo
//! escreve. Port de `cano.py:103-158` e `cano.py:183,213-221`.

use std::collections::VecDeque;

use serde::Serialize;
use serde_json::{Map, Value};

/// `cano.py:32`. O adapter reabre a sessão ociosa quando o snapshot traz outra versão.
pub const VERSION: u32 = 1;

#[derive(Default)]
pub struct Tracker {
    pub init: Option<String>,
    pub turn_open: bool,
    // Ordem de inserção, como o dict do Python: o snapshot lista os pendentes nessa ordem.
    pending: Vec<(String, String)>,
    pub last_result: Option<String>,
    pub rate_limit: Option<String>,
}

impl Tracker {
    /// Linha que o filho escreveu (`cano.py:103`).
    pub fn observe_child(&mut self, line: &str) {
        let Some(ev) = parse_object(line) else { return };
        match ev.get("type").and_then(Value::as_str) {
            Some("system") if str_field(&ev, "subtype") == Some("init") => {
                self.init = Some(line.to_owned());
            }
            Some("command_lifecycle") if str_field(&ev, "state") == Some("started") => {
                self.turn_open = true;
            }
            Some("control_request" | "sdk_control_request") => {
                self.put(py_str(ev.get("request_id")), line);
            }
            Some("control_cancel_request") => self.remove(&py_str(ev.get("request_id"))),
            Some("result") => {
                self.turn_open = false;
                self.last_result = Some(line.to_owned());
                self.pending.clear();
            }
            Some("rate_limit_event") => self.rate_limit = Some(line.to_owned()),
            // Guarda que falha cai aqui, como o `elif` do Python.
            _ if ev.contains_key("method") => self.observe_rpc(&ev, line),
            _ => {}
        }
    }

    /// JSON-RPC do app-server do Codex (`cano.py:125-142`).
    fn observe_rpc(&mut self, ev: &Map<String, Value>, line: &str) {
        if let Some(id) = ev.get("id").filter(|v| !v.is_null()) {
            self.put(py_str(Some(id)), line);
            return;
        }
        match ev.get("method").and_then(Value::as_str) {
            Some("serverRequest/resolved") => {
                self.remove(&py_str(params(ev).and_then(|p| p.get("requestId"))));
            }
            Some("turn/completed") => {
                // Turno fechado leva os pedidos da thread junto, senão o snapshot repovoa um
                // cartão que o servidor já esqueceu.
                let thread = params(ev).and_then(|p| p.get("threadId")).cloned().unwrap_or(Value::Null);
                self.pending.retain(|(_, raw)| match parse_object(raw) {
                    None => true,
                    Some(req) => params(&req).and_then(|p| p.get("threadId")).unwrap_or(&Value::Null) != &thread,
                });
            }
            _ => {}
        }
    }

    /// Linha que o cliente mandou ao filho (`cano.py:144`).
    pub fn observe_client(&mut self, line: &str) {
        let Some(ev) = parse_object(line) else { return };
        match ev.get("type") {
            Some(Value::String(t)) if t == "user" => self.turn_open = true,
            Some(Value::String(t)) if t == "control_response" => {
                let rid = ev.get("response").and_then(Value::as_object).and_then(|r| r.get("request_id"));
                self.remove(&py_str(rid));
            }
            // Resposta JSON-RPC a um pedido do servidor.
            None | Some(Value::Null) if !ev.contains_key("method") => {
                if let Some(id) = ev.get("id").filter(|v| !v.is_null()) {
                    self.remove(&py_str(Some(id)));
                }
            }
            _ => {}
        }
    }

    pub fn snapshot(&self, pid: u32, stderr_tail: &VecDeque<String>, exited: Option<i64>) -> String {
        to_json(&Snapshot {
            kind: "cano_snapshot",
            versao: VERSION,
            pid,
            init: self.init.as_deref(),
            aberto: self.turn_open,
            pendentes: self.pending.iter().map(|(_, raw)| raw.as_str()).collect(),
            ultimo_result: self.last_result.as_deref(),
            rate_limit: self.rate_limit.as_deref(),
            stderr_tail,
            saiu: exited,
        })
    }

    fn put(&mut self, key: String, line: &str) {
        // Chave repetida troca o valor e mantém a posição, como no dict.
        match self.pending.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = line.to_owned(),
            None => self.pending.push((key, line.to_owned())),
        }
    }

    fn remove(&mut self, key: &str) {
        self.pending.retain(|(k, _)| k != key);
    }
}

/// Campos e ordem de `cano.py:214-221`.
#[derive(Serialize)]
struct Snapshot<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    versao: u32,
    pid: u32,
    init: Option<&'a str>,
    aberto: bool,
    pendentes: Vec<&'a str>,
    ultimo_result: Option<&'a str>,
    rate_limit: Option<&'a str>,
    stderr_tail: &'a VecDeque<String>,
    saiu: Option<i64>,
}

#[derive(Serialize)]
struct StderrLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    linha: &'a str,
}

#[derive(Serialize)]
struct ExitLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    rc: Option<i64>,
    stderr_tail: &'a VecDeque<String>,
}

/// `cano.py:101`.
pub fn stderr_line(line: &str) -> String {
    to_json(&StderrLine { kind: "cano_stderr", linha: line })
}

/// `cano.py:183`.
pub fn exit_line(rc: Option<i64>, stderr_tail: &VecDeque<String>) -> String {
    to_json(&ExitLine { kind: "cano_saiu", rc, stderr_tail })
}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("struct sem mapa sempre vira JSON")
}

fn str_field<'a>(ev: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    ev.get(key).and_then(Value::as_str)
}

/// `ev.get("params") or {}`: o que não é objeto conta como vazio.
fn params(ev: &Map<String, Value>) -> Option<&Map<String, Value>> {
    ev.get("params").and_then(Value::as_object)
}

/// `str()` do Python sobre o valor: a chave nunca sai do processo, só precisa casar entre os
/// dois sentidos (`str(1)` e `str("1")` são a mesma chave lá e aqui).
fn py_str(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".to_owned(),
        Some(Value::Bool(true)) => "True".to_owned(),
        Some(Value::Bool(false)) => "False".to_owned(),
        Some(Value::String(s)) => s.clone(),
        // ponytail: número sai como o serde_json escreve; lista ou objeto como id não acontece.
        Some(other) => other.to_string(),
    }
}

fn parse_object(line: &str) -> Option<Map<String, Value>> {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        // O `json.loads` aceita `\ud800` sem par e o serde_json não; a linha guardada é a crua,
        // então trocar o escape só para ler os campos não muda nada do que sai.
        Err(_) => serde_json::from_str(&scrub_lone_surrogates(line)?).ok()?,
    };
    match value {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// Troca escape de surrogate (D800-DFFF) sem par pelo escape de U+FFFD. None quando não havia nada para trocar.
fn scrub_lone_surrogates(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut copied, mut changed) = (0, 0, false);
    while i < b.len() {
        if b[i] != b'\\' {
            i += 1;
            continue;
        }
        if b.get(i + 1) != Some(&b'u') {
            i += 2; // outro escape, inclusive `\\`
            continue;
        }
        let Some(cp) = hex4(b, i + 2) else {
            i += 2;
            continue;
        };
        if (0xD800..0xDC00).contains(&cp)
            && b.get(i + 6) == Some(&b'\\')
            && b.get(i + 7) == Some(&b'u')
            && hex4(b, i + 8).is_some_and(|lo| (0xDC00..0xE000).contains(&lo))
        {
            i += 12;
            continue;
        }
        if (0xD800..0xE000).contains(&cp) {
            out.push_str(&s[copied..i]);
            out.push_str("\\ufffd");
            copied = i + 6;
            changed = true;
        }
        i += 6;
    }
    if !changed {
        return None;
    }
    out.push_str(&s[copied..]);
    Some(out)
}

fn hex4(b: &[u8], at: usize) -> Option<u32> {
    let digits = std::str::from_utf8(b.get(at..at + 4)?).ok()?;
    u32::from_str_radix(digits, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(t: &Tracker) -> Value {
        serde_json::from_str(&t.snapshot(42, &VecDeque::from(["aviso".to_owned()]), None)).unwrap()
    }

    fn pending_ids(t: &Tracker) -> Vec<Value> {
        snap(t)["pendentes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|raw| {
                let ev: Value = serde_json::from_str(raw.as_str().unwrap()).unwrap();
                ev.get("request_id").or_else(|| ev.get("id")).cloned().unwrap()
            })
            .collect()
    }

    #[test]
    fn snapshot_has_the_fields_and_types_of_cano_py() {
        let mut t = Tracker::default();
        let init = r#"{"type": "system", "subtype": "init", "session_id": "sid-1"}"#;
        t.observe_child(init);
        let s = snap(&t);
        let keys: Vec<&str> = s.as_object().unwrap().keys().map(String::as_str).collect();
        let mut expected = vec![
            "type", "versao", "pid", "init", "aberto", "pendentes", "ultimo_result", "rate_limit",
            "stderr_tail", "saiu",
        ];
        let mut got = keys.clone();
        got.sort();
        expected.sort();
        assert_eq!(got, expected);
        assert_eq!(s["type"], "cano_snapshot");
        assert_eq!(s["versao"], 1);
        assert_eq!(s["pid"], 42);
        assert_eq!(s["init"], init); // a linha crua, como string
        assert_eq!(s["aberto"], false);
        assert_eq!(s["pendentes"], serde_json::json!([]));
        assert!(s["ultimo_result"].is_null() && s["rate_limit"].is_null() && s["saiu"].is_null());
        assert_eq!(s["stderr_tail"], serde_json::json!(["aviso"]));
    }

    #[test]
    fn claude_turn_with_pending_permission_then_answered() {
        let mut t = Tracker::default();
        t.observe_client(r#"{"type": "user", "message": {"content": []}}"#);
        t.observe_child(r#"{"type": "control_request", "request_id": "perm-1", "request": {"subtype": "can_use_tool"}}"#);
        assert_eq!(snap(&t)["aberto"], true);
        assert_eq!(pending_ids(&t), vec![Value::from("perm-1")]);
        t.observe_client(r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "perm-1"}}"#);
        assert!(pending_ids(&t).is_empty());
        let result = r#"{"type": "result", "subtype": "success"}"#;
        t.observe_child(result);
        let s = snap(&t);
        assert_eq!(s["aberto"], false);
        assert_eq!(s["ultimo_result"], result);
    }

    #[test]
    fn result_clears_pending_and_lifecycle_opens_turn() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"type": "command_lifecycle", "state": "started"}"#);
        t.observe_child(r#"{"type": "sdk_control_request", "request_id": 7}"#);
        t.observe_child(r#"{"type": "command_lifecycle", "state": "finished"}"#);
        assert_eq!(snap(&t)["aberto"], true);
        assert_eq!(pending_ids(&t), vec![Value::from(7)]);
        t.observe_child(r#"{"type": "result"}"#);
        assert_eq!(snap(&t)["aberto"], false);
        assert!(pending_ids(&t).is_empty());
    }

    #[test]
    fn cancel_and_rate_limit() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"type": "control_request", "request_id": "a"}"#);
        t.observe_child(r#"{"type": "control_request", "request_id": "b"}"#);
        t.observe_child(r#"{"type": "control_cancel_request", "request_id": "a"}"#);
        assert_eq!(pending_ids(&t), vec![Value::from("b")]);
        let rl = r#"{"type": "rate_limit_event", "rate_limit_info": {}}"#;
        t.observe_child(rl);
        assert_eq!(snap(&t)["rate_limit"], rl);
    }

    #[test]
    fn repeated_id_keeps_its_position() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"type": "control_request", "request_id": "a", "v": 1}"#);
        t.observe_child(r#"{"type": "control_request", "request_id": "b"}"#);
        t.observe_child(r#"{"type": "control_request", "request_id": "a", "v": 2}"#);
        let s = snap(&t);
        let first: Value = serde_json::from_str(s["pendentes"][0].as_str().unwrap()).unwrap();
        assert_eq!((first["request_id"].clone(), first["v"].clone()), (Value::from("a"), Value::from(2)));
        assert_eq!(pending_ids(&t), vec![Value::from("a"), Value::from("b")]);
    }

    #[test]
    fn jsonrpc_requests_resolutions_and_turn_completed() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 0, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th"}}"#);
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 1, "method": "item/fileChange/requestApproval", "params": {"threadId": "th"}}"#);
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 2, "method": "item/fileChange/requestApproval", "params": {"threadId": "outra"}}"#);
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 3, "method": "x/requestApproval", "params": {"threadId": "th"}}"#);
        assert_eq!(pending_ids(&t).len(), 4);
        // resposta do cliente: sem type e sem method
        t.observe_client(r#"{"jsonrpc": "2.0", "id": 0, "result": {"decision": "accept"}}"#);
        // outro cliente respondeu: o servidor avisa
        t.observe_child(r#"{"jsonrpc": "2.0", "method": "serverRequest/resolved", "params": {"threadId": "th", "requestId": 3}}"#);
        assert_eq!(pending_ids(&t), vec![Value::from(1), Value::from(2)]);
        t.observe_child(r#"{"jsonrpc": "2.0", "method": "turn/completed", "params": {"threadId": "th"}}"#);
        assert_eq!(pending_ids(&t), vec![Value::from(2)]);
    }

    #[test]
    fn integer_and_string_ids_are_the_same_key_like_python_str() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 1, "method": "m"}"#);
        t.observe_client(r#"{"id": "1"}"#);
        assert!(pending_ids(&t).is_empty());
    }

    #[test]
    fn client_line_with_method_is_not_an_answer() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"jsonrpc": "2.0", "id": 5, "method": "m"}"#);
        t.observe_client(r#"{"jsonrpc": "2.0", "id": 5, "method": "turn/start"}"#);
        assert_eq!(pending_ids(&t), vec![Value::from(5)]);
    }

    #[test]
    fn failed_guard_falls_through_to_jsonrpc_like_elif() {
        let mut t = Tracker::default();
        t.observe_child(r#"{"type": "system", "subtype": "status", "method": "x", "id": 9}"#);
        assert!(snap(&t)["init"].is_null());
        assert_eq!(pending_ids(&t), vec![Value::from(9)]);
    }

    #[test]
    fn lone_surrogate_line_is_still_observed_and_kept_raw() {
        let mut t = Tracker::default();
        let line = "{\"type\": \"control_request\", \"request_id\": \"p\", \"input\": \"corte \\ud83d\"}";
        t.observe_child(line);
        assert_eq!(snap(&t)["pendentes"][0], line);
        let result = "{\"type\": \"result\", \"result\": \"fim \\udc00 e \\\\ud800\"}";
        t.observe_child(result);
        assert_eq!(snap(&t)["aberto"], false);
        assert_eq!(snap(&t)["ultimo_result"], result);
    }

    #[test]
    fn scrub_keeps_valid_pairs_and_escaped_backslashes() {
        let pair = "\"\\ud83d\\ude00\"";
        assert_eq!(scrub_lone_surrogates(pair), None);
        assert_eq!(scrub_lone_surrogates("\"\\\\ud800\""), None); // barra escapada: texto, não escape
        let lone_then_pair = "\"a\\ud800\\ud83d\\ude00b\"";
        let fixed = "\"a\\ufffd\\ud83d\\ude00b\"";
        assert_eq!(scrub_lone_surrogates(lone_then_pair).as_deref(), Some(fixed));
    }

    #[test]
    fn invalid_or_non_object_lines_are_ignored() {
        let mut t = Tracker::default();
        t.observe_child("não é json");
        t.observe_child("[1, 2]");
        t.observe_client(r#""user""#);
        let s = snap(&t);
        assert_eq!(s["aberto"], false);
        assert!(s["init"].is_null());
    }

    #[test]
    fn stderr_and_exit_lines() {
        let s: Value = serde_json::from_str(&stderr_line("tchau")).unwrap();
        assert_eq!(s, serde_json::json!({"type": "cano_stderr", "linha": "tchau"}));
        let tail = VecDeque::from(["tchau".to_owned()]);
        let e: Value = serde_json::from_str(&exit_line(Some(3), &tail)).unwrap();
        assert_eq!(e, serde_json::json!({"type": "cano_saiu", "rc": 3, "stderr_tail": ["tchau"]}));
    }
}
