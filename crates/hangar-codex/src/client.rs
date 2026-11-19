//! Cliente JSON-RPC do app-server do Codex. O núcleo fala por dois canais de texto (uma mensagem
//! JSON por item); stdio e WebSocket só convertem o transporte nesses canais.
use crate::proto::{ClientRequest,RequestId};
use serde::{Deserialize,de::DeserializeOwned};
use serde_json::{Value,json};
use std::collections::HashMap;
use std::sync::{Arc,Mutex,atomic::{AtomicI64,Ordering}};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt,AsyncRead,AsyncReadExt,AsyncWrite,AsyncWriteExt,BufReader};
use tokio::sync::{mpsc,oneshot};

pub const MAX_LINE:usize = 16 * 1024 * 1024;
pub const INCOMING_CAPACITY:usize = 1024;
const OUTGOING_CAPACITY:usize = 256;

#[derive(Debug)]
pub enum ClientError { Timeout, Closed, Rpc { code:i64, message:String }, Decode(String), Io(String), NotWritten { code:String }, WriteUnknown { code:String } }

#[derive(Debug)]
pub enum Incoming { Notification { method:String, params:Value }, Request { id:RequestId, method:String, params:Value } }

type Pending = Arc<Mutex<Option<HashMap<RequestId,oneshot::Sender<Result<Value,ClientError>>>>>>;

#[derive(Clone)]
pub struct Client { out:mpsc::Sender<String>, pending:Pending, next:Arc<AtomicI64> }

impl Client {
    /// Núcleo: `lines_in` fecha quando a conexão acaba; aí todo pedido em voo falha com `Closed`.
    fn start(mut lines_in:mpsc::Receiver<String>,out:mpsc::Sender<String>) -> (Self,mpsc::Receiver<Incoming>) {
        let pending:Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let (tx,rx) = mpsc::channel(INCOMING_CAPACITY);
        let reader_pending = pending.clone();
        // Fraco: o leitor não pode manter o escritor vivo depois que o último `Client` sair.
        let reader_out = out.downgrade();
        tokio::spawn(async move {
            while let Some(line) = lines_in.recv().await {
                let mut msg = match serde_json::from_str::<Value>(&line) {
                    Ok(Value::Object(msg)) => msg,
                    Ok(_) => { tracing::warn!(bytes=line.len(),error="not_object","mensagem do app-server do Codex descartada"); continue; }
                    Err(e) => { tracing::warn!(bytes=line.len(),error=crate::proto::error_kind(&e),"mensagem do app-server do Codex descartada"); continue; }
                };
                let has_id = msg.get("id").is_some_and(|id|!id.is_null());
                let id = msg.get("id").filter(|id|!id.is_null()).and_then(|id|RequestId::deserialize(id).ok());
                let method = match msg.remove("method") { Some(Value::String(method)) => Some(method), _ => None };
                if has_id && id.is_none() {
                    tracing::warn!(request=method.is_some(),"id do app-server do Codex inválido; mensagem descartada");
                    // Pedido sem id legível não pode virar notificação: o Codex ficaria esperando a resposta.
                    // `try_send`: o leitor não pode travar atrás de um escritor parado.
                    if method.is_some() {
                        match reader_out.upgrade() {
                            None => tracing::warn!("resposta ao id inválido do Codex não enviada: cliente encerrado"),
                            Some(out) => if out.try_send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"id inválido"}}).to_string()).is_err() {
                                tracing::warn!("resposta ao id inválido do Codex não coube na fila de saída");
                            },
                        }
                    }
                    continue;
                }
                let params = msg.remove("params").unwrap_or(Value::Null);
                match (id,method) {
                    (Some(id),Some(method)) => {
                        if tx.send(Incoming::Request { id,method,params }).await.is_err() { break; }
                    }
                    (None,Some(method)) => {
                        if tx.send(Incoming::Notification { method,params }).await.is_err() { break; }
                    }
                    (Some(id),None) => {
                        let waiter = reader_pending.lock().unwrap().as_mut().and_then(|map|map.remove(&id));
                        if let Some(waiter) = waiter {
                            let outcome = match msg.get("error").filter(|e|!e.is_null()) {
                                Some(error) => Err(ClientError::Rpc { code:error["code"].as_i64().unwrap_or(0),
                                    message:error["message"].as_str().unwrap_or("").into() }),
                                None => Ok(msg.remove("result").unwrap_or(Value::Null)),
                            };
                            let _ = waiter.send(outcome);
                        }
                    }
                    (None,None) => {},
                }
            }
            // Conexão acabou: quem espera resposta sai agora, não no prazo.
            close_pending(&reader_pending);
        });
        (Self { out,pending,next:Arc::new(AtomicI64::new(1)) },rx)
    }

    /// Quem recebe o `Receiver<Incoming>` tem de consumi-lo numa tarefa própria e nunca esperar um
    /// `request` dentro desse laço: o canal tem limite, a leitura espera quando ele enche e as
    /// respostas na fila atrás das notificações esperam junto. Soltar o `Receiver` encerra o
    /// cliente na próxima mensagem do servidor; a partir daí todo `request` devolve `Closed`.
    pub fn over_lines(reader:impl AsyncRead+Unpin+Send+'static,mut writer:impl AsyncWrite+Unpin+Send+'static) -> (Self,mpsc::Receiver<Incoming>) {
        let (lines_tx,lines_rx) = mpsc::channel::<String>(INCOMING_CAPACITY);
        let transport = tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let mut buffer = Vec::new();
            loop {
                buffer.clear();
                // `take` limita a linha: acima do teto a conexão é encerrada.
                let read = (&mut reader).take(MAX_LINE as u64 + 1).read_until(b'\n',&mut buffer).await;
                match read {
                    Ok(0) => break,
                    Err(e) => { tracing::warn!(error=?e.kind(),"leitura do app-server do Codex falhou; conexão encerrada"); break; }
                    Ok(_) if buffer.len() > MAX_LINE => { tracing::warn!("linha do app-server do Codex acima do teto; conexão encerrada"); break; }
                    Ok(_) => {
                        let Ok(text) = std::str::from_utf8(&buffer) else {
                            tracing::warn!(bytes=buffer.len(),error="utf8","mensagem do app-server do Codex descartada");
                            continue;
                        };
                        let text = text.trim_end();
                        if !text.is_empty() && lines_tx.send(text.to_owned()).await.is_err() { break; }
                    }
                }
            }
        });
        let (out_tx,mut out_rx) = mpsc::channel::<String>(OUTGOING_CAPACITY);
        let (client,incoming) = Self::start(lines_rx,out_tx);
        let pending = client.pending.clone();
        tokio::spawn(async move {
            while let Some(line) = out_rx.recv().await {
                let written = async { writer.write_all(line.as_bytes()).await?; writer.write_all(b"\n").await?; writer.flush().await }.await;
                if let Err(e) = written {
                    tracing::warn!(error=?e.kind(),"escrita para o app-server do Codex falhou; conexão encerrada");
                    // Parar a leitura fecha o `Incoming`: sem isso quem consome nunca sabe que o transporte caiu.
                    transport.abort();
                    break;
                }
            }
            // Sem escritor nenhum pedido chega ao Codex: quem espera sai agora, não no prazo.
            close_pending(&pending);
        });
        (client,incoming)
    }

    /// O `Receiver<Incoming>` segue o mesmo contrato de [`Client::over_lines`].
    pub fn spawn_stdio(mut command:tokio::process::Command) -> std::io::Result<(Self,mpsc::Receiver<Incoming>,tokio::process::Child)> {
        // stderr cru do Codex não pode cair no diário do serviço; diagnóstico privado fica com quem chama.
        command.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().ok_or_else(||std::io::Error::other("sem stdout"))?;
        let stdin = child.stdin.take().ok_or_else(||std::io::Error::other("sem stdin"))?;
        let (client,incoming) = Self::over_lines(stdout,stdin);
        Ok((client,incoming,child))
    }

    /// O `Receiver<Incoming>` segue o mesmo contrato de [`Client::over_lines`].
    pub async fn connect_ws(url:&str) -> Result<(Self,mpsc::Receiver<Incoming>),ClientError> {
        let raw = connect_raw_ws(url).await?;
        let (out_tx,mut out_rx) = mpsc::channel::<String>(OUTGOING_CAPACITY);
        let (client,incoming) = Self::start(raw.incoming,out_tx);
        let (outgoing,pending,tasks) = (raw.outgoing,client.pending.clone(),raw.tasks);
        tokio::spawn(async move {
            // A ponte segura o transporte: quando ela acaba, as tarefas dele também.
            let _tasks = tasks;
            // O cliente tipado espera a resposta RPC, não o recibo: ele é descartado.
            while let Some(text) = out_rx.recv().await {
                let (receipt,_) = oneshot::channel();
                if outgoing.send(RawWrite { text,receipt,guard:None }).await.is_err() { break; }
            }
            close_pending(&pending);
        });
        Ok((client,incoming))
    }

    pub async fn request<R:DeserializeOwned>(&self,request:ClientRequest,timeout:Duration) -> Result<R,ClientError> {
        let (method,params) = request.into_parts();
        self.request_method(method,params,timeout).await
    }

    pub async fn request_method<R:DeserializeOwned>(&self,method:&str,params:Value,timeout:Duration) -> Result<R,ClientError> {
        let id = RequestId::Integer(self.next.fetch_add(1,Ordering::Relaxed));
        let (tx,rx) = oneshot::channel();
        match self.pending.lock().unwrap().as_mut() { Some(map) => { map.insert(id.clone(),tx); }, None => return Err(ClientError::Closed) }
        // Futuro cancelado por quem chama (select!, timeout de fora) não pode deixar a entrada no mapa.
        let _forget = ForgetOnDrop { pending:&self.pending,id:&id };
        let line = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string();
        // O prazo cobre também a fila de saída cheia (escritor parado).
        let exchange = async {
            self.out.send(line).await.map_err(|_|ClientError::Closed)?;
            rx.await.map_err(|_|ClientError::Closed)?
        };
        let value = tokio::time::timeout(timeout,exchange).await.map_err(|_|ClientError::Timeout)??;
        serde_json::from_value(value).map_err(|e|ClientError::Decode(format!("{method}: {}",crate::proto::error_kind(&e))))
    }

    pub async fn notify(&self,method:&str,params:Value) -> Result<(),ClientError> {
        self.out.send(json!({"jsonrpc":"2.0","method":method,"params":params}).to_string()).await.map_err(|_|ClientError::Closed)
    }

    pub async fn respond(&self,id:RequestId,outcome:Result<Value,(i64,String)>) -> Result<(),ClientError> {
        let line = match outcome {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err((code,message)) => json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
        };
        self.out.send(line.to_string()).await.map_err(|_|ClientError::Closed)
    }
}

/// Guarda relida pelo escritor, depois de esperar a prontidão do socket.
pub type RawWriteGuard = Arc<dyn Fn(&std::task::Waker) -> Result<(),String> + Send + Sync>;

/// Quadro de texto a escrever no WebSocket; `receipt` só conclui depois do `send` no socket.
pub struct RawWrite {
    pub text:String,
    pub receipt:oneshot::Sender<Result<(),ClientError>>,
    pub guard:Option<RawWriteGuard>,
}

/// WebSocket sem JSON-RPC: não gera id nem consome resposta; cada item é um quadro de texto inteiro.
/// Soltar os dois lados fecha a conexão.
pub struct RawWebSocket {
    pub incoming:mpsc::Receiver<String>,
    pub outgoing:mpsc::Sender<RawWrite>,
    /// Código de fechamento do WebSocket quando a leitura acaba (1006 = caiu sem quadro de fechamento), para o diário.
    pub close_code:Arc<Mutex<Option<u16>>>,
    /// Quem segura o transporte segura também as tarefas dele: soltar encerra as duas.
    pub tasks:RawTasks,
}

/// Folga para o escritor fechar o socket com educação depois de soltas as filas; menor que a primeira
/// espera de reconexão de quem usa o transporte, para nunca haver dois clientes ao mesmo tempo.
const RAW_CLOSE_GRACE:Duration = Duration::from_millis(500);

/// Tarefas internas do transporte bruto. Soltas, o escritor tem a folga para fechar sozinho; preso num
/// `send` que não anda (app-server que não lê), é abortado e o socket sai junto. O recibo da escrita em
/// curso é solto sem conclusão, que quem espera lê como incerto.
pub struct RawTasks { reader:tokio::task::AbortHandle, writer:tokio::task::AbortHandle }

impl RawTasks {
    /// Encerra já, sem folga: socket parado não fecha com educação, e o socket sai antes de qualquer reconexão.
    pub fn abort_now(self) { self.writer.abort(); self.reader.abort(); }
}

impl Drop for RawTasks {
    fn drop(&mut self) {
        let (reader,writer) = (self.reader.clone(),self.writer.clone());
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => { handle.spawn(async move { tokio::time::sleep(RAW_CLOSE_GRACE).await; writer.abort(); reader.abort(); }); }
            Err(_) => { writer.abort(); reader.abort(); }
        }
    }
}

pub type WsRequest = tokio_tungstenite::tungstenite::handshake::client::Request;

/// O pedido de abertura do WebSocket, com o token do app-server (`--ws-auth capability-token`) quando ele tem.
pub fn ws_request(url:&str,bearer:Option<&str>) -> Result<WsRequest,ClientError> {
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest,http::{HeaderValue,header::AUTHORIZATION}};
    let mut request = url.into_client_request().map_err(|error|ClientError::Io(error.to_string()))?;
    if let Some(token) = bearer {
        let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_|ClientError::Io("token do app-server inválido".into()))?;
        request.headers_mut().insert(AUTHORIZATION,value);
    }
    Ok(request)
}

pub async fn connect_raw_ws(url:&str) -> Result<RawWebSocket,ClientError> { connect_raw_ws_with(url,None).await }

pub async fn connect_raw_ws_with(url:&str,bearer:Option<&str>) -> Result<RawWebSocket,ClientError> {
    use futures_util::{SinkExt,StreamExt};
    use tokio_tungstenite::tungstenite::{Message,protocol::WebSocketConfig};
    // Sem teto, como o `max_size=None` do Python: o `thread/resume` devolve o histórico inteiro, e uma conversa
    // longa passa de 16 MiB; o teto derrubava a conexão a cada assinatura.
    let config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let request = ws_request(url,bearer)?;
    // O transporte não tem TLS: wss:// sairia em texto puro para uma porta TLS.
    if request.uri().scheme_str() != Some("ws") { return Err(ClientError::Io("só ws:// é aceito (transporte sem TLS)".into())); }
    let host = request.uri().host().ok_or_else(||ClientError::Io("WebSocket sem host".into()))?;
    let port = request.uri().port_u16().unwrap_or(80);
    let tcp = tokio::net::TcpStream::connect((host.trim_start_matches('[').trim_end_matches(']'),port)).await.map_err(|error|ClientError::Io(error.to_string()))?;
    let write_state = Arc::new(Mutex::new(RawWriteState::default()));
    let stream = GuardedSocket { stream:tcp,state:write_state.clone() };
    let (socket,_) = tokio_tungstenite::client_async_with_config(request,stream,Some(config)).await.map_err(|error|ClientError::Io(error.to_string()))?;
    let (mut sink,mut stream) = socket.split();
    let (incoming_tx,incoming) = mpsc::channel::<String>(INCOMING_CAPACITY);
    let close_code = Arc::new(Mutex::new(None));
    let closed = close_code.clone();
    let mut reader = tokio::spawn(async move {
        let mut code = 1006;
        while let Some(message) = stream.next().await {
            match message {
                Ok(Message::Text(text)) => { if incoming_tx.send(text.to_string()).await.is_err() { break; } }
                Ok(Message::Close(frame)) => { code = frame.map_or(1005,|frame|u16::from(frame.code)); break; }
                Ok(Message::Binary(data)) => tracing::warn!(bytes=data.len(),error="binary","mensagem do app-server do Codex descartada"),
                Ok(_) => {},
                Err(e) => { tracing::warn!(error=ws_error_kind(&e),"leitura do WebSocket do Codex falhou; conexão encerrada"); break; }
            }
        }
        // Antes de soltar o canal: quem vê a leitura acabar já encontra o código.
        *closed.lock().unwrap_or_else(|e|e.into_inner()) = Some(code);
        drop(incoming_tx);
    });
    let reader_abort = reader.abort_handle();
    let (outgoing,mut writes) = mpsc::channel::<RawWrite>(OUTGOING_CAPACITY);
    let writer = tokio::spawn(async move {
        let mut failed = false;
        loop {
            let write = tokio::select! {
                write = writes.recv() => match write { Some(write) => write, None => break },
                // Leitura acabou: a conexão caiu e nada mais na fila pode ser escrito.
                _ = &mut reader => break,
            };
            *write_state.lock().unwrap() = RawWriteState { guard:write.guard, ..RawWriteState::default() };
            let result = sink.send(Message::text(write.text)).await.map_err(|error|ClientError::Io(error.to_string()));
            let result = raw_write_result(result,&write_state);
            failed = result.is_err();
            if !failed { write_state.lock().unwrap().guard = None; }
            if failed { tracing::warn!(error="write","escrita no WebSocket do Codex falhou; conexão encerrada"); }
            let _ = write.receipt.send(result);
            if failed { break; }
        }
        // Sem close após efeito incerto: ele poderia descarregar o quadro recusado ainda no buffer.
        reader.abort();
        if !failed { let _ = sink.close().await; }
    });
    Ok(RawWebSocket { incoming,outgoing,close_code,tasks:RawTasks { reader:reader_abort,writer:writer.abort_handle() } })
}

#[derive(Default)]
pub struct RawWriteState { guard:Option<RawWriteGuard>, bytes:usize, refusal:Option<String> }

impl RawWriteState {
    pub fn prepare(&mut self,guard:Option<RawWriteGuard>) { *self = Self { guard,..Self::default() }; }
}

pub fn raw_write_result(result:Result<(),ClientError>,state:&Mutex<RawWriteState>) -> Result<(),ClientError> {
    let state = state.lock().unwrap();
    if let Some(code) = &state.refusal {
        return Err(if state.bytes == 0 { ClientError::NotWritten { code:code.clone() } }
            else { ClientError::WriteUnknown { code:code.clone() } });
    }
    result
}

/// A guarda fica abaixo dos buffers do WebSocket/TLS e da trava entre leitor e escritor.
pub struct GuardedSocket<S> { stream:S, state:Arc<Mutex<RawWriteState>> }
impl<S> GuardedSocket<S> {
    pub fn new(stream:S) -> (Self,Arc<Mutex<RawWriteState>>) {
        let state = Arc::new(Mutex::new(RawWriteState::default()));
        (Self { stream,state:state.clone() },state)
    }
}
impl<S:AsyncRead+Unpin> AsyncRead for GuardedSocket<S> {
    fn poll_read(mut self:std::pin::Pin<&mut Self>,cx:&mut std::task::Context<'_>,buffer:&mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_read(cx,buffer)
    }
}
impl<S:AsyncWrite+Unpin> AsyncWrite for GuardedSocket<S> {
    fn poll_write(mut self:std::pin::Pin<&mut Self>,cx:&mut std::task::Context<'_>,bytes:&[u8]) -> std::task::Poll<std::io::Result<usize>> {
        use std::{pin::Pin,task::Poll};
        let this = &mut *self;
        let mut state = this.state.lock().unwrap();
        if let Some(guard) = &state.guard && let Err(code) = guard(cx.waker()) {
            state.refusal = Some(code);
            return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"interação mudou antes da escrita")));
        }
        let result = Pin::new(&mut this.stream).poll_write(cx,bytes);
        if let Poll::Ready(Ok(count)) = &result { state.bytes += *count; }
        result
    }
    fn poll_flush(mut self:std::pin::Pin<&mut Self>,cx:&mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self:std::pin::Pin<&mut Self>,cx:&mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// Fecha o mapa: os pedidos em voo saem com `Closed` e os novos já nascem recusados.
fn close_pending(pending:&Pending) {
    if let Some(map) = pending.lock().unwrap().take() {
        for (_,waiter) in map { let _ = waiter.send(Err(ClientError::Closed)); }
    }
}

/// Só a categoria: a mensagem do erro pode ecoar o quadro recebido.
fn ws_error_kind(error:&tokio_tungstenite::tungstenite::Error) -> &'static str {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::ConnectionClosed | Error::AlreadyClosed => "closed",
        Error::Io(_) => "io",
        Error::Protocol(_) => "protocol",
        Error::Capacity(_) => "capacity",
        Error::Utf8(_) => "utf8",
        _ => "other",
    }
}

struct ForgetOnDrop<'a> { pending:&'a Pending, id:&'a RequestId }

impl Drop for ForgetOnDrop<'_> {
    fn drop(&mut self) { if let Some(map) = self.pending.lock().unwrap().as_mut() { map.remove(self.id); } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn raw_guard_is_rechecked_at_the_bytes_after_socket_wait_and_buffering() {
        use std::{pin::Pin,task::{Context,Poll},sync::atomic::AtomicBool};
        struct Socket { current:Arc<AtomicBool>, waited:bool, bytes:Vec<u8>, partial:bool }
        impl AsyncWrite for Socket {
            fn poll_write(mut self:Pin<&mut Self>,cx:&mut Context<'_>,bytes:&[u8]) -> Poll<std::io::Result<usize>> {
                if !self.waited {
                    self.waited = true;
                    self.current.store(false,Ordering::Release);
                    if !self.partial { cx.waker().wake_by_ref(); return Poll::Pending; }
                    self.bytes.push(bytes[0]);
                    return Poll::Ready(Ok(1));
                }
                self.bytes.extend_from_slice(bytes);
                Poll::Ready(Ok(bytes.len()))
            }
            fn poll_flush(self:Pin<&mut Self>,_:&mut Context<'_>) -> Poll<std::io::Result<()>> { Poll::Ready(Ok(())) }
            fn poll_shutdown(self:Pin<&mut Self>,_:&mut Context<'_>) -> Poll<std::io::Result<()>> { Poll::Ready(Ok(())) }
        }
        for partial in [false,true] {
            let current = Arc::new(AtomicBool::new(true));
            let source = current.clone();
            let guard:RawWriteGuard = Arc::new(move |_| if source.load(Ordering::Acquire) { Ok(()) } else { Err("erro_interacao_desatualizada".into()) });
            let state = Arc::new(Mutex::new(RawWriteState { guard:Some(guard), ..RawWriteState::default() }));
            let mut socket = GuardedSocket { stream:Socket { current,waited:false,bytes:vec![],partial },state:state.clone() };
            let result = socket.write_all(b"buffered answer A").await.map_err(|error|ClientError::Io(error.to_string()));
            let result = raw_write_result(result,&state);
            assert!(if partial { matches!(result,Err(ClientError::WriteUnknown { code }) if code == "erro_interacao_desatualizada") }
                else { matches!(result,Err(ClientError::NotWritten { code }) if code == "erro_interacao_desatualizada") });
            assert_eq!(socket.stream.bytes.len(),usize::from(partial));
        }
    }

    #[tokio::test]
    async fn cancelled_request_leaves_no_pending_entry() {
        let (ours,_theirs) = tokio::io::duplex(1 << 16);
        let (r,w) = tokio::io::split(ours);
        let (client,_incoming) = Client::over_lines(r,w);
        let call = client.request::<Value>(ClientRequest::ModelList(Default::default()),Duration::from_secs(60));
        assert!(tokio::time::timeout(Duration::from_millis(20),call).await.is_err());
        assert!(client.pending.lock().unwrap().as_ref().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deadline_covers_a_full_outgoing_queue() {
        let (out,_unread) = mpsc::channel(1);
        let (_lines,lines_in) = mpsc::channel(1);
        let (client,_incoming) = Client::start(lines_in,out);
        let short = Duration::from_millis(10);
        assert!(matches!(client.request::<Value>(ClientRequest::ModelList(Default::default()),short).await,Err(ClientError::Timeout)));
        let call = client.request::<Value>(ClientRequest::ModelList(Default::default()),short);
        let outcome = tokio::time::timeout(Duration::from_secs(1),call).await.expect("prazo do pedido não cobriu o envio");
        assert!(matches!(outcome,Err(ClientError::Timeout)));
    }
}
