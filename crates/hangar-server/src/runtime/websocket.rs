//! Transporte WebSocket do ator, para o app-server do Codex que o Rust adota e não possui: o quadro
//! do motor sai intacto (o id é dele) e a confirmação de escrita vem do recibo do socket, nunca do
//! enfileiramento. Parar o transporte fecha a conexão; nenhum processo morre por isso.
use super::cano::{IoEvent, IoTasks, WireFrame, UI_CAPACITY};
use super::protocol::{RuntimeError, WriteOutcome, MAX_FRAME};
use hangar_codex::client::{ClientError, RawWebSocket, RawWrite};
use serde_json::Value;
use std::collections::HashSet;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

/// O mesmo prazo do ACK do cano: socket parado (app-server vivo que não lê) não segura a operação
/// nem as escritas atrás dela.
const ACK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

pub struct WebSocketConnection {
    socket: RawWebSocket,
}

/// Enfileirar não é escrever: só o recibo do `send` no socket prova `Written`.
fn queued_write_outcome(queued: bool, receipt: Option<Result<(), ClientError>>) -> WriteOutcome {
    if !queued { return WriteOutcome::NotWritten; }
    match receipt {
        Some(Ok(())) => WriteOutcome::Written,
        Some(Err(ClientError::NotWritten { .. })) => WriteOutcome::NotWritten,
        Some(Err(_)) | None => WriteOutcome::Unknown,
    }
}

impl WebSocketConnection {
    pub fn new(socket: RawWebSocket) -> Self { Self { socket } }

    /// Só o código no erro: a mensagem crua da conexão pode trazer endereço ou texto do servidor.
    pub async fn connect(url: &str) -> Result<Self, RuntimeError> { Self::connect_with(url, None).await }

    /// `token`: o do app-server desta vida, lido do arquivo que o sidecar aponta.
    pub async fn connect_with(url: &str, token: Option<&str>) -> Result<Self, RuntimeError> {
        hangar_codex::client::connect_raw_ws_with(url, token).await.map(Self::new)
            .map_err(|_| RuntimeError::new("websocket_connect", "não foi possível conectar ao app-server do Codex"))
    }

    pub fn start(self, _generation: u64, capacity: usize) -> IoTasks {
        let RawWebSocket { mut incoming, outgoing, tasks, close_code } = self.socket;
        let (writer, mut commands) = mpsc::channel::<WireFrame>(capacity.max(1));
        let (ui_writer, mut ui_commands) = mpsc::channel::<WireFrame>(UI_CAPACITY);
        let (events_tx, events) = mpsc::channel(capacity.max(1));
        let (stop, stop_rx) = watch::channel(false);
        let reader_task = {
            let events = events_tx.clone();
            let mut stop = stop_rx.clone();
            tokio::spawn(async move {
                let work = async {
                    while let Some(text) = incoming.recv().await {
                        match serde_json::from_str::<Value>(&text) {
                            Ok(frame) if frame.is_object() => if events.send(IoEvent::Line(frame)).await.is_err() { return; },
                            _ => tracing::warn!("mensagem do app-server do Codex fora do JSON esperado; leitura continua"),
                        }
                    }
                    let code = close_code.lock().unwrap_or_else(|e| e.into_inner()).map(i64::from);
                    let _ = events.send(IoEvent::End { code }).await;
                };
                tokio::select! { _ = work => {}, _ = stop.changed() => {} }
            })
        };
        let writer_task = tokio::spawn(async move {
            // Fim do escritor (parada, prazo estourado): as tarefas do transporte bruto saem com ele, inclusive
            // a presa num `send` que o app-server não lê.
            let mut tasks = Some(tasks);
            let mut stop = stop_rx;
            let work = async {
                let mut used = HashSet::new();
                loop {
                    // A conversa sai sempre antes: uma rajada de desenhos não atrasa a mensagem da pessoa.
                    let command = tokio::select! {
                        biased;
                        Some(command) = commands.recv() => command,
                        Some(command) = ui_commands.recv() => command,
                        else => break,
                    };
                    let ephemeral = command.ephemeral;
                    if command.until.is_some_and(|until| Instant::now() >= until) { continue; }
                    if !ephemeral && !used.insert(command.operation_id.clone()) { continue; }
                    let text = command.frame.to_string();
                    if !command.frame.is_object() || text.len() > MAX_FRAME {
                        if !ephemeral {
                            let _ = events_tx.send(IoEvent::WriteAck { operation_id: command.operation_id, outcome: WriteOutcome::NotWritten }).await;
                        }
                        continue;
                    }
                    let (receipt, written) = oneshot::channel();
                    // Socket parado (enfileirar ou o recibo passam do prazo): o transporte acaba aqui, com `End`,
                    // e o ator reconecta; seguir deixaria os `ui_*` encherem a fila e prenderem as escritas.
                    let mut stalled = false;
                    let queued = match tokio::time::timeout(ACK_WAIT, outgoing.send(RawWrite { text, receipt, guard: command.guard })).await {
                        Ok(sent) => sent.is_ok(),
                        Err(_) => { stalled = true; false }
                    };
                    if ephemeral {
                        if stalled {
                            // O socket parado sai já, antes do `End`: nenhuma reconexão encontra o cliente antigo vivo.
                            if let Some(tasks) = tasks.take() { tasks.abort_now(); }
                            let _ = events_tx.send(IoEvent::End { code: None }).await;
                            break;
                        }
                        continue;
                    }
                    let receipt = if queued {
                        match tokio::time::timeout(ACK_WAIT, written).await { Ok(receipt) => receipt.ok(), Err(_) => { stalled = true; None } }
                    } else { None };
                    let event = match receipt {
                        Some(Err(ClientError::NotWritten { code })) => IoEvent::WriteGuardAck {
                            operation_id: command.operation_id, outcome: WriteOutcome::NotWritten,
                            error: RuntimeError::new(&code, "interação mudou antes da escrita"),
                        },
                        Some(Err(ClientError::WriteUnknown { code })) => IoEvent::WriteGuardAck {
                            operation_id: command.operation_id, outcome: WriteOutcome::Unknown,
                            error: RuntimeError::new(&code, "interação mudou após possível escrita"),
                        },
                        receipt => IoEvent::WriteAck { operation_id: command.operation_id, outcome: queued_write_outcome(queued, receipt) },
                    };
                    if stalled { if let Some(tasks) = tasks.take() { tasks.abort_now(); } }
                    if events_tx.send(event).await.is_err() { break; }
                    if stalled { let _ = events_tx.send(IoEvent::End { code: None }).await; break; }
                }
            };
            tokio::select! { _ = work => {}, _ = stop.changed() => {} }
        });
        IoTasks { writer, ui_writer, events, stop, reader_task, writer_task }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn conversation_enqueue_is_not_a_write_receipt() {
        assert_eq!(queued_write_outcome(false, None), WriteOutcome::NotWritten);
        assert_eq!(queued_write_outcome(true, None), WriteOutcome::Unknown);
        assert_eq!(queued_write_outcome(true, Some(Ok(()))), WriteOutcome::Written);
        assert_eq!(queued_write_outcome(true, Some(Err(ClientError::Closed))), WriteOutcome::Unknown);
        assert_eq!(queued_write_outcome(true, Some(Err(ClientError::NotWritten { code: "erro_interacao_desatualizada".into() }))), WriteOutcome::NotWritten);
    }

    /// Servidor falso: devolve ao cliente cada quadro recebido como `result`, com o mesmo id.
    async fn echo_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use futures_util::{SinkExt, StreamExt};
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(message)) = ws.next().await {
                let Ok(text) = message.to_text() else { continue };
                let Ok(frame) = serde_json::from_str::<Value>(text) else { continue };
                let reply = json!({"id": frame["id"], "result": {"seen": frame}});
                if ws.send(tokio_tungstenite::tungstenite::Message::text(reply.to_string())).await.is_err() { break; }
            }
        });
        format!("ws://{address}")
    }

    #[tokio::test]
    async fn engine_id_reaches_the_socket_and_the_reply_comes_back_intact() {
        let url = echo_server().await;
        let mut io = WebSocketConnection::connect(&url).await.unwrap().start(7, 8);
        let frame = json!({"jsonrpc": "2.0", "id": "hangar:7:op-1", "method": "turn/start", "params": {}});
        assert!(io.try_send(WireFrame { operation_id: "wire-1".into(), frame: frame.clone(), ephemeral: false, until: None, guard: None }).is_ok());
        let (mut ack, mut line) = (None, None);
        while ack.is_none() || line.is_none() {
            match tokio::time::timeout(Duration::from_secs(5), io.events.recv()).await.unwrap() {
                Some(IoEvent::WriteAck { operation_id, outcome }) => ack = Some((operation_id, outcome)),
                Some(IoEvent::Line(value)) => line = Some(value),
                _ => panic!("transporte encerrou antes do eco"),
            }
        }
        assert_eq!(ack, Some(("wire-1".to_owned(), WriteOutcome::Written)));
        let line = line.unwrap();
        assert_eq!(line["id"], "hangar:7:op-1");
        assert_eq!(line["result"]["seen"], frame);
        io.stop().await;
    }

    #[tokio::test]
    async fn closed_socket_never_reports_written() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            drop(tokio_tungstenite::accept_async(stream).await.unwrap());
        });
        let mut io = WebSocketConnection::connect(&format!("ws://{address}")).await.unwrap().start(1, 8);
        // Sem quadro de fechamento: o código da queda (1006) vai ao diário, como o `codex.connection_closed` do Python.
        assert!(matches!(tokio::time::timeout(Duration::from_secs(5), io.events.recv()).await.unwrap(), Some(IoEvent::End { code: Some(1006) })));
        let frame = json!({"id": "hangar:1:late", "method": "turn/start"});
        assert!(io.try_send(WireFrame { operation_id: "late".into(), frame, ephemeral: false, until: None, guard: None }).is_ok());
        match tokio::time::timeout(Duration::from_secs(5), io.events.recv()).await.unwrap() {
            Some(IoEvent::WriteAck { operation_id, outcome }) => {
                assert_eq!(operation_id, "late");
                assert_ne!(outcome, WriteOutcome::Written);
            }
            _ => panic!("escrita sem desfecho depois da queda"),
        }
        io.stop().await;
    }

    #[tokio::test]
    async fn stalled_socket_write_turns_unknown_at_the_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            // App-server vivo que nunca lê: o buffer do TCP enche e o `send` fica parado.
            let _ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            std::future::pending::<()>().await;
        });
        let mut io = WebSocketConnection::connect(&format!("ws://{address}")).await.unwrap().start(1, 8);
        tokio::time::pause();
        // Maiores que os buffers do loopback, abaixo de `MAX_FRAME`. Três: o Winsock aceita inteiros os dois
        // primeiros `send` com o par sem ler, de qualquer tamanho, e só o terceiro para; no Linux o primeiro já prende.
        for id in ["fill-1", "fill-2", "stuck"] {
            let frame = json!({"id": format!("hangar:1:{id}"), "params": {"blob": "a".repeat(15 << 20)}});
            assert!(io.try_send(WireFrame { operation_id: id.into(), frame, ephemeral: false, until: None, guard: None }).is_ok());
        }
        loop {
            match tokio::time::timeout(ACK_WAIT * 2, io.events.recv()).await.expect("escrita parada sem desfecho") {
                // Só os de enchimento podem ter saído inteiros antes do socket parar.
                Some(IoEvent::WriteAck { operation_id, outcome: WriteOutcome::Written }) => assert!(operation_id.starts_with("fill-"), "{operation_id}"),
                Some(IoEvent::WriteAck { outcome, .. }) => { assert_eq!(outcome, WriteOutcome::Unknown); break; }
                _ => panic!("transporte encerrou em vez de dar o prazo"),
            }
        }
        // O primeiro incerto por prazo encerra o transporte: a fila não fica presa atrás do socket parado.
        assert!(matches!(tokio::time::timeout(ACK_WAIT, io.events.recv()).await.expect("sem fim depois do prazo"), Some(IoEvent::End { code: None })));
        io.stop().await;
    }

    #[tokio::test]
    async fn stalled_ui_frames_end_the_transport_instead_of_blocking() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            std::future::pending::<()>().await;
        });
        let mut io = WebSocketConnection::connect(&format!("ws://{address}")).await.unwrap().start(1, 8);
        tokio::time::pause();
        // Desenhos grandes que ninguém lê: o primeiro trava o socket e os seguintes enchem a fila bruta.
        let feeder = {
            let ui = io.ui_writer.clone();
            tokio::spawn(async move {
                for n in 0..400 {
                    let frame = json!({"method": "ui_draw", "params": {"n": n, "blob": "a".repeat(64 << 10)}});
                    if ui.send(WireFrame { operation_id: format!("ui:{n}"), frame, ephemeral: true, until: None, guard: None }).await.is_err() { return; }
                }
            })
        };
        match tokio::time::timeout(ACK_WAIT * 4, io.events.recv()).await.expect("socket parado sem fim") {
            Some(IoEvent::End { code: None }) => {}
            _ => panic!("esperava o fim do transporte"),
        }
        feeder.abort();
        io.stop().await;
    }
}
