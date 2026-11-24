//! Cliente do Agent Client Protocol falado pelo `agy acp`: JSON-RPC 2.0, uma mensagem por linha no stdin/stdout.
//! `Session` é a máquina de uma conversa, sem I/O: recebe as linhas do agente e devolve o que escrever e o que
//! aconteceu. Abertura: `initialize` → `authenticate` → `session/resume` (conversa conhecida) ou `session/new`.
use std::collections::HashMap;

use serde_json::{json, Value};

/// O agy 1.5.13 responde 1 a um pedido de 2; as mensagens usadas aqui são as mesmas nas duas.
pub const PROTOCOL_VERSION: u64 = 2;
pub const AUTH_PERSONAL: &str = "oauth-personal";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice { pub id: String, pub name: String }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOption { pub id: String, pub name: String, pub kind: String }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage { Initialize, Authenticate, Open, Prompt, SetModel, SetMode, Logout }

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Ready { session_id: String, resumed: bool, models: Vec<Choice>, model: Option<String>, modes: Vec<Choice>, mode: Option<String>, tier: Option<String> },
    TextDelta(String),
    ThoughtDelta(String),
    ToolStarted { id: String, name: Option<String>, title: Option<String>, kind: Option<String>, input: Option<Value> },
    ToolUpdated { id: String, status: Option<String>, output: Option<String> },
    /// `rpc_id` volta igual na resposta: o agy manda texto (`"1"`), não número.
    PermissionAsked { rpc_id: Value, tool_call_id: Option<String>, title: Option<String>, input: Option<Value>, options: Vec<PermissionOption>, warning: Option<String> },
    TurnEnded { stop_reason: String, usage: Option<Value> },
    ModeChanged(String),
    ModelChanged(String),
    Commands(Vec<String>),
    Title(String),
    ContextUsage { used: u64, size: u64 },
    Failed { stage: Stage, code: Option<i64>, message: String },
}

#[derive(Debug, PartialEq)]
pub enum Out {
    Write(String),
    Event(Event),
}

enum Waiting { Initialize, Authenticate, Open { resumed: bool }, Prompt, SetModel(String), SetMode(String), Logout }

pub struct Session {
    next_id: u64,
    waiting: HashMap<u64, Waiting>,
    cwd: String,
    resume: Option<String>,
    session_id: Option<String>,
    tier: Option<String>,
    prompting: bool,
}

impl Session {
    pub fn new(cwd: impl Into<String>, resume: Option<String>) -> Self {
        Self { next_id: 1, waiting: HashMap::new(), cwd: cwd.into(), resume, session_id: None, tier: None, prompting: false }
    }

    pub fn session_id(&self) -> Option<&str> { self.session_id.as_deref() }
    pub fn is_ready(&self) -> bool { self.session_id.is_some() }
    pub fn is_prompting(&self) -> bool { self.prompting }

    pub fn start(&mut self) -> Vec<Out> {
        vec![self.request("initialize", json!({
            "protocolVersion": PROTOCOL_VERSION,
            // Com `readTextFile` ligado o agy pede `fs/read_text_file` ao cliente e o turno espera a resposta.
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
            "clientInfo": { "name": "hangar", "version": env!("CARGO_PKG_VERSION") },
        }), Waiting::Initialize)]
    }

    /// Com turno em andamento o agy trata o prompt novo como orientação do turno atual.
    pub fn prompt(&mut self, text: &str) -> Option<Out> {
        let session_id = self.session_id.clone()?;
        self.prompting = true;
        Some(self.request("session/prompt", json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": text }] }), Waiting::Prompt))
    }

    pub fn cancel(&self) -> Option<Out> {
        let session_id = self.session_id.as_ref()?;
        Some(Out::Write(line(&json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session_id } }))))
    }

    pub fn set_model(&mut self, model: &str) -> Option<Out> {
        let session_id = self.session_id.clone()?;
        Some(self.request("session/set_model", json!({ "sessionId": session_id, "modelId": model }), Waiting::SetModel(model.to_owned())))
    }

    pub fn set_mode(&mut self, mode: &str) -> Option<Out> {
        let session_id = self.session_id.clone()?;
        Some(self.request("session/set_mode", json!({ "sessionId": session_id, "modeId": mode }), Waiting::SetMode(mode.to_owned())))
    }

    pub fn logout(&mut self) -> Out { self.request("logout", json!({}), Waiting::Logout) }

    /// `option` `None` = cancelada (a pessoa fechou a pergunta, ou o turno foi interrompido).
    pub fn answer_permission(rpc_id: &Value, option: Option<&str>) -> Out {
        let outcome = match option {
            Some(id) => json!({ "outcome": "selected", "optionId": id }),
            None => json!({ "outcome": "cancelled" }),
        };
        Out::Write(line(&json!({ "jsonrpc": "2.0", "id": rpc_id, "result": { "outcome": outcome } })))
    }

    pub fn feed(&mut self, text: &str) -> Vec<Out> {
        let Ok(Value::Object(msg)) = serde_json::from_str::<Value>(text.trim()) else { return Vec::new() };
        match (msg.get("method").and_then(Value::as_str), msg.get("id")) {
            (Some(method), Some(id)) => self.agent_request(method, id.clone(), msg.get("params").unwrap_or(&Value::Null)),
            (Some("session/update"), None) => self.update(msg.get("params").map(|p| &p["update"]).unwrap_or(&Value::Null)),
            (Some(_), None) => Vec::new(),
            (None, Some(id)) => match id.as_u64().and_then(|id| self.waiting.remove(&id)) {
                Some(waiting) => self.response(waiting, msg.get("result"), msg.get("error")),
                None => Vec::new(),
            },
            (None, None) => Vec::new(),
        }
    }

    fn request(&mut self, method: &str, params: Value, waiting: Waiting) -> Out {
        let id = self.next_id;
        self.next_id += 1;
        self.waiting.insert(id, waiting);
        Out::Write(line(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })))
    }

    fn agent_request(&mut self, method: &str, rpc_id: Value, params: &Value) -> Vec<Out> {
        if method != "session/request_permission" {
            // Pedido que não sabemos atender responde erro: pendurado, ele prende o turno.
            return vec![Out::Write(line(&json!({ "jsonrpc": "2.0", "id": rpc_id, "error": { "code": -32601, "message": format!("method not supported: {method}") } })))];
        }
        let call = &params["toolCall"];
        let options: Vec<PermissionOption> = params["options"].as_array().into_iter().flatten().filter_map(|o| Some(PermissionOption {
            id: o["optionId"].as_str()?.to_owned(),
            name: o["name"].as_str().unwrap_or_default().to_owned(),
            kind: o["kind"].as_str().unwrap_or_default().to_owned(),
        })).collect();
        let warning = params["options"].as_array().into_iter().flatten().chain(std::iter::once(params))
            .find_map(|o| o["_meta"]["agy.security.warning"].as_str().map(str::to_owned));
        vec![Out::Event(Event::PermissionAsked {
            rpc_id,
            tool_call_id: text_of(&call["toolCallId"]),
            title: text_of(&call["title"]),
            input: call.get("rawInput").filter(|v| !v.is_null()).cloned(),
            options,
            warning,
        })]
    }

    fn response(&mut self, waiting: Waiting, result: Option<&Value>, error: Option<&Value>) -> Vec<Out> {
        let failed = |stage: Stage| -> Vec<Out> {
            let error = error.unwrap_or(&Value::Null);
            vec![Out::Event(Event::Failed {
                stage,
                code: error["code"].as_i64(),
                message: error["message"].as_str().unwrap_or("sem resultado").to_owned(),
            })]
        };
        let result = match (result, error) {
            (Some(result), None) => result,
            _ => return match waiting {
                Waiting::Initialize => failed(Stage::Initialize),
                Waiting::Authenticate => failed(Stage::Authenticate),
                Waiting::Open { .. } => failed(Stage::Open),
                Waiting::Prompt => { self.prompting = false; failed(Stage::Prompt) }
                Waiting::SetModel(_) => failed(Stage::SetModel),
                Waiting::SetMode(_) => failed(Stage::SetMode),
                Waiting::Logout => failed(Stage::Logout),
            },
        };
        match waiting {
            Waiting::Initialize => vec![self.request("authenticate", json!({ "methodId": AUTH_PERSONAL }), Waiting::Authenticate)],
            Waiting::Authenticate => {
                self.tier = text_of(&result["_meta"]["antigravity"]["tier"]["name"]);
                let mcp: Vec<Value> = Vec::new();
                let out = match self.resume.clone() {
                    Some(id) => self.request("session/resume", json!({ "sessionId": id, "cwd": self.cwd, "mcpServers": mcp }), Waiting::Open { resumed: true }),
                    None => self.request("session/new", json!({ "cwd": self.cwd, "mcpServers": mcp }), Waiting::Open { resumed: false }),
                };
                vec![out]
            }
            Waiting::Open { resumed } => {
                let Some(session_id) = text_of(&result["sessionId"]).or_else(|| self.resume.clone()) else { return failed(Stage::Open) };
                self.session_id = Some(session_id.clone());
                let models = choices(&result["models"]["availableModels"], "modelId");
                let modes = choices(&result["modes"]["availableModes"], "id");
                vec![Out::Event(Event::Ready {
                    session_id,
                    resumed,
                    models,
                    model: text_of(&result["models"]["currentModelId"]),
                    modes,
                    mode: text_of(&result["modes"]["currentModeId"]),
                    tier: self.tier.clone(),
                })]
            }
            Waiting::Prompt => {
                self.prompting = false;
                vec![Out::Event(Event::TurnEnded {
                    stop_reason: text_of(&result["stopReason"]).unwrap_or_else(|| "end_turn".to_owned()),
                    usage: result.get("usage").filter(|v| !v.is_null()).cloned(),
                })]
            }
            Waiting::SetModel(model) => vec![Out::Event(Event::ModelChanged(model))],
            Waiting::SetMode(mode) => vec![Out::Event(Event::ModeChanged(mode))],
            Waiting::Logout => Vec::new(),
        }
    }

    fn update(&mut self, update: &Value) -> Vec<Out> {
        let event = match update["sessionUpdate"].as_str() {
            Some("agent_message_chunk") => update["content"]["text"].as_str().map(|t| Event::TextDelta(t.to_owned())),
            Some("agent_thought_chunk") => update["content"]["text"].as_str().map(|t| Event::ThoughtDelta(t.to_owned())),
            Some("tool_call") => text_of(&update["toolCallId"]).map(|id| Event::ToolStarted {
                id,
                name: text_of(&update["name"]),
                title: text_of(&update["title"]),
                kind: text_of(&update["kind"]),
                input: update.get("rawInput").filter(|v| !v.is_null()).cloned(),
            }),
            Some("tool_call_update") => text_of(&update["toolCallId"]).map(|id| Event::ToolUpdated {
                id,
                status: text_of(&update["status"]),
                output: tool_output(update),
            }),
            Some("current_mode_update") => text_of(&update["currentModeId"]).map(Event::ModeChanged),
            Some("available_commands_update") => Some(Event::Commands(
                update["availableCommands"].as_array().into_iter().flatten().filter_map(|c| text_of(&c["name"])).collect(),
            )),
            Some("session_info_update") => text_of(&update["title"]).map(Event::Title),
            Some("usage_update") => match (update["used"].as_u64(), update["size"].as_u64()) {
                (Some(used), Some(size)) => Some(Event::ContextUsage { used, size }),
                _ => None,
            },
            _ => None,
        };
        event.map(Out::Event).into_iter().collect()
    }
}

fn line(value: &Value) -> String {
    let mut text = value.to_string();
    text.push('\n');
    text
}

fn text_of(value: &Value) -> Option<String> { value.as_str().filter(|s| !s.is_empty()).map(str::to_owned) }

fn choices(list: &Value, id_field: &str) -> Vec<Choice> {
    list.as_array().into_iter().flatten().filter_map(|c| Some(Choice {
        id: c[id_field].as_str()?.to_owned(),
        name: c["name"].as_str().unwrap_or_default().to_owned(),
    })).collect()
}

/// Texto da saída: os blocos `content` de texto, senão o `rawOutput.output`. Vazio conta como sem saída.
fn tool_output(update: &Value) -> Option<String> {
    let blocks: Vec<&str> = update["content"].as_array().into_iter().flatten()
        .filter_map(|b| b["content"]["text"].as_str()).filter(|t| !t.is_empty()).collect();
    if !blocks.is_empty() { return Some(blocks.join("\n")); }
    update["rawOutput"]["output"].as_str().filter(|t| !t.is_empty()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(path).unwrap().lines().map(str::to_owned).collect()
    }

    /// Repete a conversa gravada: o prompt sai quando a sessão fica pronta, como faz o ator.
    fn replay(name: &str) -> (Vec<Out>, Vec<Event>) {
        let mut session = Session::new("/tmp/agy-prova", None);
        let mut writes = session.start();
        let mut events = Vec::new();
        for text in fixture(name) {
            for out in session.feed(&text) {
                match out {
                    Out::Event(event) => {
                        if matches!(event, Event::Ready { .. }) { writes.extend(session.prompt("oi")); }
                        events.push(event);
                    }
                    write => writes.push(write),
                }
            }
        }
        (writes, events)
    }

    fn written(writes: &[Out]) -> Vec<Value> {
        writes.iter().map(|o| match o { Out::Write(t) => serde_json::from_str(t).unwrap(), _ => unreachable!() }).collect()
    }

    #[test]
    fn opens_a_new_session_and_asks_the_permission_of_the_recorded_turn() {
        let (writes, events) = replay("acp-permission.jsonl");
        let methods: Vec<_> = written(&writes).iter().map(|w| w["method"].as_str().unwrap_or("").to_owned()).collect();
        assert_eq!(methods, ["initialize", "authenticate", "session/new", "session/prompt"]);
        let Some(Event::Ready { session_id, models, modes, mode, .. }) = events.iter().find(|e| matches!(e, Event::Ready { .. })) else { panic!("sem Ready") };
        assert_eq!(session_id, "9abf1fce-b58c-43c2-98be-8071a4843f8d");
        assert!(models.iter().any(|m| m.id == "gemini-4-neon-high"));
        assert_eq!(modes.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["default", "accept-edits", "auto", "plan"]);
        assert_eq!(mode.as_deref(), Some("default"));
        let Some(Event::PermissionAsked { rpc_id, options, title, .. }) = events.iter().find(|e| matches!(e, Event::PermissionAsked { .. })) else { panic!("sem permissão") };
        assert_eq!(rpc_id, &Value::String("1".into()));
        assert_eq!(title.as_deref(), Some("rm -f /tmp/agy-prova/nao-existe.txt"));
        assert_eq!(options.iter().map(|o| (o.id.as_str(), o.kind.as_str())).collect::<Vec<_>>(), [("allow", "allow_once"), ("deny", "reject_once")]);
        assert!(events.iter().any(|e| matches!(e, Event::ToolUpdated { status: Some(s), .. } if s == "failed")));
        assert!(matches!(events.last(), Some(Event::TurnEnded { stop_reason, .. }) if stop_reason == "end_turn"));
    }

    #[test]
    fn a_text_turn_streams_the_answer() {
        let (_, events) = replay("acp-text.jsonl");
        let text: String = events.iter().filter_map(|e| match e { Event::TextDelta(t) => Some(t.as_str()), _ => None }).collect();
        assert!(!text.trim().is_empty());
        assert!(matches!(events.last(), Some(Event::TurnEnded { .. })));
    }

    #[test]
    fn a_known_conversation_is_resumed() {
        let mut session = Session::new("/w", Some("904fd70e-847b-49ed-9999-a13d7a4d9a98".into()));
        session.start();
        let auth = session.feed(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#);
        assert_eq!(written(&auth)[0]["method"], "authenticate");
        let open = session.feed(r#"{"jsonrpc":"2.0","id":2,"result":{"_meta":{"antigravity":{"tier":{"name":"Google AI Pro"}}}}}"#);
        let open = &written(&open)[0];
        assert_eq!(open["method"], "session/resume");
        assert_eq!(open["params"]["sessionId"], "904fd70e-847b-49ed-9999-a13d7a4d9a98");
        let ready = session.feed(r#"{"jsonrpc":"2.0","id":3,"result":{"sessionId":"904fd70e-847b-49ed-9999-a13d7a4d9a98"}}"#);
        assert!(matches!(&ready[0], Out::Event(Event::Ready { resumed: true, tier: Some(t), .. }) if t == "Google AI Pro"));
    }

    #[test]
    fn a_failed_login_is_reported_and_nothing_else_is_sent() {
        let mut session = Session::new("/w", None);
        session.start();
        session.feed(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        let out = session.feed(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"Authentication required"}}"#);
        assert_eq!(out, vec![Out::Event(Event::Failed { stage: Stage::Authenticate, code: Some(-32000), message: "Authentication required".into() })]);
        assert!(!session.is_ready());
        assert!(session.prompt("x").is_none());
    }

    #[test]
    fn an_unexpected_agent_request_gets_an_error_instead_of_hanging() {
        let mut session = Session::new("/w", None);
        let out = session.feed(r#"{"jsonrpc":"2.0","id":"7","method":"fs/read_text_file","params":{"path":"/etc/passwd"}}"#);
        let reply = &written(&out)[0];
        assert_eq!(reply["id"], "7");
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[test]
    fn the_permission_answer_echoes_the_text_id() {
        let Out::Write(text) = Session::answer_permission(&Value::String("1".into()), Some("deny")) else { panic!() };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["id"], "1");
        assert_eq!(v["result"]["outcome"]["optionId"], "deny");
        let Out::Write(text) = Session::answer_permission(&Value::String("2".into()), None) else { panic!() };
        assert!(text.contains(r#""outcome":"cancelled""#));
    }
}
