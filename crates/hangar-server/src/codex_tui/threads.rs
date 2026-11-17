//! A escuta das threads principais que a TUI abre durante a vida do pane. Só olha: quem chama
//! `thread/start` é a TUI (thread de outro cliente sem turno não tem rollout e o `codex resume` a recusa).
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use hangar_codex::proto::{self as wire, ClientRequest};
use serde_json::{Value, json};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

use super::session::Session;
use super::{Pane, Screen};

/// Quanto esperar o app-server para começar a escutar; estourar não é fatal, só perde o vínculo.
const WAIT: Duration = Duration::from_secs(60);

/// A conversa de `thread/started` que é a principal desta sessão: mesma pasta, aberta pela TUI, sem pai.
pub(crate) fn primary(message: &Value, cwd: &str) -> Option<(String, String, Value)> {
    if message.get("method").and_then(Value::as_str) != Some("thread/started") {
        return None;
    }
    let thread = message.get("params").and_then(|params| params.get("thread")).filter(|thread| thread.is_object())?;
    if !crate::runtime::codex::primary_thread(thread, Some(cwd)) {
        return None;
    }
    Some((thread["id"].as_str()?.to_owned(), thread["path"].as_str()?.to_owned(), thread.clone()))
}

const FIRST_RESUME_ID: &str = "hangar-tui:first-resume";
const FIRST_TURN_ID: &str = "hangar-tui:first";

/// O que a entrega da primeira mensagem pede: um quadro ao app-server ou uma linha no pane.
#[derive(Debug, PartialEq)]
pub(crate) enum Out {
    Send(Value),
    Say(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Waiting,
    Resuming,
    Sent,
    Done,
}

/// A primeira mensagem que não pôde ir no argv da TUI (`codex.cmd` recusa quebra de linha): vai inteira
/// pelo app-server, uma vez, como o turno que o servidor abre numa sessão Codex com terminal.
pub(crate) struct FirstPrompt {
    text: String,
    resume: Option<String>,
    stage: Stage,
}

impl FirstPrompt {
    pub(crate) fn new(text: String, resume: Option<String>) -> Self { Self { text, resume, stage: Stage::Waiting } }

    fn turn(&self, thread: &str) -> Out {
        let (method, params) = ClientRequest::TurnStart(wire::TurnStartParams {
            thread_id: thread.into(),
            input: vec![json!({"type": "text", "text": self.text})],
            ..Default::default()
        })
        .into_parts();
        Out::Send(json!({"jsonrpc": "2.0", "id": FIRST_TURN_ID, "method": method, "params": params}))
    }

    fn refused(message: &Value) -> Option<String> {
        message.get("error").map(|error| error.get("message").and_then(Value::as_str).map_or_else(|| error.to_string(), str::to_owned))
    }

    /// Cada mensagem do app-server; `primary` é a thread principal que ela anunciou, se anunciou.
    pub(crate) fn message(&mut self, message: &Value, primary: Option<&str>) -> Vec<Out> {
        let id = message.get("id").filter(|_| message.get("method").is_none());
        match self.stage {
            // Retomando, o `thread/started` não vem: assina a thread logo depois do `initialize`.
            Stage::Waiting if self.resume.is_some() && id == Some(&json!(1)) => {
                self.stage = Stage::Resuming;
                let (method, params) = ClientRequest::ThreadResume(wire::ThreadResumeParams {
                    thread_id: self.resume.clone().unwrap_or_default(),
                    ..Default::default()
                })
                .into_parts();
                vec![Out::Send(json!({"jsonrpc": "2.0", "id": FIRST_RESUME_ID, "method": method, "params": params}))]
            }
            Stage::Waiting if self.resume.is_none() => match primary {
                Some(thread) => {
                    self.stage = Stage::Sent;
                    vec![self.turn(thread)]
                }
                None => vec![],
            },
            Stage::Resuming if id == Some(&json!(FIRST_RESUME_ID)) => match Self::refused(message) {
                Some(reason) => {
                    self.stage = Stage::Done;
                    vec![Out::Say(format!("erro: a primeira mensagem não chegou ao Codex (a conversa não abriu: {reason}); ela não foi cortada, mande de novo pela conversa"))]
                }
                None => {
                    self.stage = Stage::Sent;
                    vec![self.turn(&self.resume.clone().unwrap_or_default())]
                }
            },
            Stage::Sent if id == Some(&json!(FIRST_TURN_ID)) => {
                self.stage = Stage::Done;
                vec![Out::Say(match Self::refused(message) {
                    Some(reason) => format!("erro: a primeira mensagem não chegou ao Codex ({reason}); ela não foi cortada, mande de novo pela conversa"),
                    None => "primeira mensagem entregue pelo app-server".into(),
                })]
            }
            _ => vec![],
        }
    }

    /// O socket fechou ou a escuta desistiu: o que não foi confirmado aparece no pane.
    pub(crate) fn closed(&self) -> Option<String> {
        (self.stage != Stage::Done).then(|| "erro: a primeira mensagem não chegou ao Codex (o app-server saiu antes da confirmação); mande de novo pela conversa".into())
    }
}

/// `ready` é avisado quando a resposta do `initialize` chega (ou a escuta desiste): a TUI só sobe
/// depois, senão o `thread/started` da conversa nova sai antes de haver quem o escute.
pub(crate) async fn listen(session: Arc<Session>, cwd: String, gone: Arc<AtomicBool>, ready: Option<tokio::sync::oneshot::Sender<()>>,
    first: Option<FirstPrompt>) {
    let mut first = first;
    follow(&session, &cwd, &gone, ready, &mut first).await;
    if let Some(line) = first.as_ref().and_then(FirstPrompt::closed) {
        Pane.say(&line);
    }
}

async fn follow(session: &Session, cwd: &str, gone: &AtomicBool, ready: Option<tokio::sync::oneshot::Sender<()>>,
    first: &mut Option<FirstPrompt>) {
    let mut ready = ready;
    let config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let deadline = Instant::now() + WAIT;
    let mut socket = None;
    while socket.is_none() && Instant::now() < deadline {
        // Quem avisa da morte do app-server é a espera da porta, uma vez só.
        if gone.load(Ordering::Acquire) {
            return;
        }
        match tokio::time::timeout(Duration::from_secs(1), tokio_tungstenite::connect_async_with_config(session.endpoint.as_str(), Some(config), false)).await {
            Ok(Ok((ws, _))) => socket = Some(ws),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    let Some(mut ws) = socket else {
        Pane.say(&format!("app-server nao respondeu em {}", session.endpoint));
        return;
    };
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"clientInfo": crate::codex_setup::reload::client_info(), "capabilities": null}});
    if ws.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }
    // Socket fechado: o servidor foi embora (a TUI saiu e a saída o derrubou), nada a gravar.
    while let Some(Ok(message)) = ws.next().await {
        if let Some(ready) = ready.take() {
            let _ = ready.send(());
        }
        if !message.is_text() && !message.is_binary() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&message.into_data()) else { continue };
        let started = primary(&value, cwd);
        if let Some((id, path, thread)) = &started {
            session.save_thread(id, path, Some(thread), &Pane);
        }
        let Some(prompt) = first.as_mut() else { continue };
        for out in prompt.message(&value, started.as_ref().map(|(id, _, _)| id.as_str())) {
            match out {
                Out::Say(line) => Pane.say(&line),
                Out::Send(frame) => {
                    if ws.send(Message::Text(frame.to_string().into())).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "linha 1\r\nlinha 2\nlinha 3";

    fn sent(outs: &[Out]) -> Vec<&Value> {
        outs.iter().filter_map(|out| match out { Out::Send(frame) => Some(frame), Out::Say(_) => None }).collect()
    }

    /// PR-13 (decisão 26): conversa nova, a mensagem vai uma vez só, inteira, como `turn/start` na thread
    /// que a TUI anunciou; antes disso nada sai, e a confirmação aparece no pane.
    #[test]
    fn a_new_conversation_gets_the_whole_first_prompt_once_after_its_thread_starts() {
        let mut first = FirstPrompt::new(TEXT.into(), None);
        assert!(first.message(&json!({"id": 1, "result": {}}), None).is_empty());
        let outs = first.message(&json!({"method": "thread/started"}), Some("t1"));
        let frames = sent(&outs);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["method"], "turn/start");
        assert_eq!(frames[0]["params"]["threadId"], "t1");
        assert_eq!(frames[0]["params"]["input"], json!([{"type": "text", "text": TEXT}]));
        assert!(first.message(&json!({"method": "thread/started"}), Some("t2")).is_empty(), "uma vez só");
        assert!(first.closed().is_some(), "sem confirmação ainda é falha");
        assert_eq!(first.message(&json!({"id": FIRST_TURN_ID, "result": {"turn": {"id": "u1"}}}), None),
            vec![Out::Say("primeira mensagem entregue pelo app-server".into())]);
        assert_eq!(first.closed(), None);
    }

    /// Retomando, assina a thread depois do `initialize` e só então abre o turno.
    #[test]
    fn a_resumed_conversation_subscribes_before_the_first_prompt() {
        let mut first = FirstPrompt::new(TEXT.into(), Some("t-9".into()));
        let outs = first.message(&json!({"id": 1, "result": {}}), None);
        assert_eq!(sent(&outs)[0]["method"], "thread/resume");
        assert_eq!(sent(&outs)[0]["params"]["threadId"], "t-9");
        let outs = first.message(&json!({"id": FIRST_RESUME_ID, "result": {}}), None);
        assert_eq!(sent(&outs)[0]["method"], "turn/start");
        assert_eq!(sent(&outs)[0]["params"]["threadId"], "t-9");
    }

    /// Recusa do app-server, ou o socket fechando antes da confirmação, viram linha de erro no pane.
    #[test]
    fn a_refused_or_unconfirmed_first_prompt_is_an_error_line() {
        let mut first = FirstPrompt::new(TEXT.into(), None);
        first.message(&json!({"method": "thread/started"}), Some("t1"));
        let outs = first.message(&json!({"id": FIRST_TURN_ID, "error": {"code": -32600, "message": "thread not loaded"}}), None);
        assert!(matches!(&outs[..], [Out::Say(line)] if line.starts_with("erro:") && line.contains("thread not loaded")), "{outs:?}");
        assert_eq!(first.closed(), None, "a recusa já foi dita");
        let mut resumed = FirstPrompt::new(TEXT.into(), Some("t-9".into()));
        resumed.message(&json!({"id": 1, "result": {}}), None);
        let outs = resumed.message(&json!({"id": FIRST_RESUME_ID, "error": {"message": "no rollout found"}}), None);
        assert!(matches!(&outs[..], [Out::Say(line)] if line.starts_with("erro:") && line.contains("no rollout found")), "{outs:?}");
        assert!(FirstPrompt::new(TEXT.into(), None).closed().is_some_and(|line| line.starts_with("erro:")));
    }

    #[test]
    fn only_the_primary_thread_of_this_folder_is_taken() {
        let started = |thread: Value| json!({"method": "thread/started", "params": {"thread": thread}});
        let base = json!({"id": "t1", "path": "/r/t1", "cwd": "/w", "source": "cli", "threadSource": "user"});
        assert_eq!(primary(&started(base.clone()), "/w").map(|(id, path, _)| (id, path)), Some(("t1".into(), "/r/t1".into())));
        let with = |field: &str, value: Value| { let mut thread = base.clone(); thread[field] = value; started(thread) };
        for refused in [with("cwd", json!("/outra")), with("source", json!("exec")), with("parentThreadId", json!("t0")),
            with("threadSource", json!("subagent")), with("canAcceptDirectInput", json!(false)), with("path", json!("")),
            json!({"method": "thread/updated", "params": {"thread": base}}), json!({"method": "thread/started", "params": {"thread": "x"}}),
            json!([1])] {
            assert!(primary(&refused, "/w").is_none(), "{refused}");
        }
        assert!(primary(&with("source", json!("vscode")), "/w").is_some());
    }
}
