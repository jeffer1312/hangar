use std::{fmt, time::Duration};
use async_channel::{Receiver, Sender};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{Client, StatusCode, header};
use ring::{digest, rand::{SecureRandom, SystemRandom}};
use tokio::{io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt}, runtime::Handle,
    sync::oneshot, task::JoinHandle, time::timeout};
use crate::api::Api;

const MAX_MESSAGE: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error { Network, Handshake, Protocol, Utf8, TooLarge, QueueFull, Closed, Http(u16) }

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Http(401) => "Autenticação do terminal recusada.",
            Self::Http(403) => "Acesso ao terminal recusado; confira a sessão e a autenticação.",
            Self::Http(404) => "Sessão do terminal não encontrada.",
            Self::Http(409) => "O terminal está indisponível por um conflito de sessão.",
            Self::Http(_) | Self::Handshake => "O servidor recusou a conexão WebSocket.",
            Self::Network => "A conexão do terminal foi interrompida.",
            Self::Protocol | Self::Utf8 => "O servidor enviou um quadro WebSocket inválido.",
            Self::TooLarge => "A mensagem do terminal excede o limite de tamanho.",
            Self::QueueFull => "A fila do terminal está cheia; aguarde antes de enviar.",
            Self::Closed => "A conexão do terminal está fechada.",
        })
    }
}

pub enum Event { Connected, Data(Vec<u8>), Closed }
pub enum TextEvent { Connected, Text(String), Closed }
pub(crate) struct Frame { pub opcode: u8, fin: bool, pub data: Vec<u8> }

/// O que cada socket entrega: o terminal aceita texto e binário; o de texto recusa binário.
pub(crate) trait Incoming: Send + 'static {
    const CONNECTED: Self;
    const CLOSED: Self;
    fn message(opcode: u8, data: Vec<u8>) -> Result<Self, Error> where Self: Sized;
}

impl Incoming for Event {
    const CONNECTED: Self = Self::Connected;
    const CLOSED: Self = Self::Closed;
    fn message(_: u8, data: Vec<u8>) -> Result<Self, Error> { Ok(Self::Data(data)) }
}

impl Incoming for TextEvent {
    const CONNECTED: Self = Self::Connected;
    const CLOSED: Self = Self::Closed;
    fn message(opcode: u8, data: Vec<u8>) -> Result<Self, Error> {
        if opcode != 1 { return Err(Error::Protocol); }
        String::from_utf8(data).map(Self::Text).map_err(|_| Error::Utf8)
    }
}

/// A janela possui este cliente; descartá-lo cancela inclusive a conexão pendente.
pub struct Socket<E> {
    commands: Sender<Frame>,
    events: Receiver<Result<E, Error>>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

pub type Terminal = Socket<Event>;
pub type TextSocket = Socket<TextEvent>;

impl Terminal {
    /// `token` é a credencial da conexão de `api`, nunca o texto atual do formulário.
    /// `shortcut`: terminal de atalho da sessão; o backend confere o dono antes de anexar. Com `hangar`, é o terminal
    /// No Hangar `shortcut`, que não pertence a sessão nenhuma.
    #[allow(clippy::too_many_arguments)]
    pub fn open(runtime: &Handle, api: &Api, session: &str, shortcut: Option<&str>, hangar: bool, token: String, cols: u16, rows: u16) -> Self {
        Self::connect(runtime, terminal_url(api, session, shortcut, hangar, &token, cols, rows))
    }

    pub fn send(&self, bytes: &[u8]) -> Result<(), Error> { self.enqueue(2, bytes) }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), Error> {
        let (cols, rows) = dimensions(cols, rows);
        let text = serde_json::json!({"t": "resize", "cols": cols, "rows": rows});
        self.enqueue(1, text.to_string().as_bytes())
    }
}

impl TextSocket {
    /// `path` já leva a query; o token entra como no terminal, credencial da conexão de `api`.
    pub fn open(runtime: &Handle, api: &Api, path: &str, token: String) -> Self {
        Self::connect(runtime, text_url(api, path, &token))
    }

    pub fn send(&self, text: &str) -> Result<(), Error> { self.enqueue(1, text.as_bytes()) }
}

impl<E: Incoming> Socket<E> {
    fn connect(runtime: &Handle, url: url::Url) -> Self {
        let (commands, input) = async_channel::bounded(8);
        let (output, events) = async_channel::bounded(8);
        let (stop, mut stopped) = oneshot::channel();
        let task = runtime.spawn(async move {
            let connected = tokio::select! {
                biased;
                _ = &mut stopped => return,
                result = handshake(url) => result,
            };
            let result = match connected {
                Err(error) => Err(error),
                Ok(mut stream) => match pump(&mut stream, input, &output, &mut stopped).await {
                    Err(Error::Closed) => write_frame(&mut stream, 8, &1000u16.to_be_bytes()).await
                        .map(|()| E::CLOSED),
                    Err(error @ (Error::Protocol | Error::Utf8 | Error::TooLarge)) => {
                        let code: u16 = match error { Error::Utf8 => 1007, Error::TooLarge => 1009, _ => 1002 };
                        let _ = write_frame(&mut stream, 8, &code.to_be_bytes()).await;
                        Err(error)
                    }
                    result => result.map(|_| E::CLOSED),
                },
            };
            // O consumidor parado não pode manter a tarefa viva; EOF também sinaliza desconexão.
            let _ = timeout(IO_TIMEOUT, output.send(result)).await;
        });
        Self { commands, events, stop: Some(stop), task }
    }

    pub fn events(&self) -> Receiver<Result<E, Error>> { self.events.clone() }

    fn enqueue(&self, opcode: u8, bytes: &[u8]) -> Result<(), Error> {
        if self.stop.is_none() || self.task.is_finished() { return Err(Error::Closed); }
        if bytes.len() > MAX_MESSAGE { return Err(Error::TooLarge); }
        self.commands.try_send(Frame { opcode, fin: true, data: bytes.to_vec() })
            .map_err(|error| if error.is_full() { Error::QueueFull } else { Error::Closed })
    }

    pub fn close(&mut self) { if let Some(stop) = self.stop.take() { let _ = stop.send(()); } }
}

impl<E> Drop for Socket<E> { fn drop(&mut self) { self.task.abort(); } }

fn dimensions(cols: u16, rows: u16) -> (u16, u16) { (cols.clamp(20, 500), rows.clamp(5, 200)) }

/// Estende o caminho da base, como o terminal: servidor atrás de prefixo continua alcançável.
fn text_url(api: &Api, path: &str, token: &str) -> url::Url {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let mut url = api.route();
    url.path_segments_mut().expect("validated HTTP base").pop_if_empty()
        .extend(path.split('/').filter(|part| !part.is_empty()));
    url.set_query((!query.is_empty()).then_some(query));
    url.query_pairs_mut().append_pair("token", token.trim());
    url
}

/// O terminal No Hangar mora fora de qualquer sessão, na própria rota; o de atalho e o da sessão, na dela.
fn terminal_url(api: &Api, session: &str, shortcut: Option<&str>, hangar: bool, token: &str, cols: u16, rows: u16) -> url::Url {
    let (cols, rows) = dimensions(cols, rows);
    let mut url = if let (true, Some(id)) = (hangar, shortcut) {
        let mut url = api.route();
        url.path_segments_mut().expect("validated HTTP base").pop_if_empty()
            .extend(["api", "hangar-terminals", id, "term"]);
        url
    } else { api.endpoint(Some(session), Some("term")) };
    url.query_pairs_mut().append_pair("token", token.trim())
        .append_pair("cols", &cols.to_string()).append_pair("rows", &rows.to_string());
    if let (false, Some(id)) = (hangar, shortcut) { url.query_pairs_mut().append_pair("shortcut", id); }
    url
}

pub(crate) fn accept_key(key: &str) -> String {
    let bytes = format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    STANDARD.encode(digest::digest(&digest::SHA1_FOR_LEGACY_USE_ONLY, bytes.as_bytes()))
}

fn check_handshake(status: StatusCode, headers: &header::HeaderMap, key: &str) -> Result<(), Error> {
    if status != StatusCode::SWITCHING_PROTOCOLS { return Err(Error::Http(status.as_u16())); }
    let contains = |name, expected: &str| headers.get_all(name).iter().filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(',')).any(|v| v.trim().eq_ignore_ascii_case(expected));
    let accepts: Vec<_> = headers.get_all(header::SEC_WEBSOCKET_ACCEPT).iter().collect();
    if !contains(header::UPGRADE, "websocket") || !contains(header::CONNECTION, "upgrade")
        || accepts.len() != 1 || accepts[0].to_str().ok().map(str::trim) != Some(accept_key(key).as_str())
        || headers.contains_key(header::SEC_WEBSOCKET_EXTENSIONS) || headers.contains_key(header::SEC_WEBSOCKET_PROTOCOL) {
        return Err(Error::Handshake);
    }
    Ok(())
}

async fn handshake(url: url::Url) -> Result<reqwest::Upgraded, Error> {
    let mut nonce = [0; 16];
    SystemRandom::new().fill(&mut nonce).map_err(|_| Error::Network)?;
    let key = STANDARD.encode(nonce);
    let client = Client::builder().http1_only().redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10)).build().map_err(|_| Error::Network)?;
    // Nunca transportar erros do reqwest: eles podem conter a URL autenticada.
    timeout(Duration::from_secs(10), async {
        let response = client.get(url).header(header::UPGRADE, "websocket").header(header::CONNECTION, "Upgrade")
            .header(header::SEC_WEBSOCKET_VERSION, "13").header(header::SEC_WEBSOCKET_KEY, &key)
            .send().await.map_err(|_| Error::Network)?;
        check_handshake(response.status(), response.headers(), &key)?;
        response.upgrade().await.map_err(|_| Error::Network)
    }).await.map_err(|_| Error::Network)?
}

fn encode(opcode: u8, bytes: &[u8], mask: [u8; 4]) -> Vec<u8> { frame_bytes(opcode, bytes, Some(mask)) }

/// Cliente mascara o que escreve; servidor, nunca (RFC 6455, 5.1).
fn frame_bytes(opcode: u8, bytes: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let bit = if mask.is_some() { 0x80 } else { 0 };
    let mut frame = vec![0x80 | opcode];
    match bytes.len() {
        n @ 0..=125 => frame.push(bit | n as u8),
        n @ 126..=65535 => { frame.push(bit | 126); frame.extend_from_slice(&(n as u16).to_be_bytes()); }
        n => { frame.push(bit | 127); frame.extend_from_slice(&(n as u64).to_be_bytes()); }
    }
    match mask {
        Some(mask) => {
            frame.extend_from_slice(&mask);
            frame.extend(bytes.iter().enumerate().map(|(ix, byte)| byte ^ mask[ix % 4]));
        }
        None => frame.extend_from_slice(bytes),
    }
    frame
}

/// Escrita do lado servidor (o `/cdp` do navegador): sem máscara.
pub(crate) async fn write_server_frame(writer: &mut (impl AsyncWrite + Unpin), opcode: u8, bytes: &[u8]) -> Result<(), Error> {
    timeout(IO_TIMEOUT, async {
        writer.write_all(&frame_bytes(opcode, bytes, None)).await?;
        writer.flush().await
    }).await
        .map_err(|_| Error::Network)?.map_err(|_| Error::Network)
}

async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), opcode: u8, bytes: &[u8]) -> Result<(), Error> {
    let mut mask = [0; 4];
    SystemRandom::new().fill(&mut mask).map_err(|_| Error::Network)?;
    timeout(IO_TIMEOUT, async {
        writer.write_all(&encode(opcode, bytes, mask)).await?;
        writer.flush().await
    }).await
        .map_err(|_| Error::Network)?.map_err(|_| Error::Network)
}

async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<Frame, Error> { read_frame_as(reader, false).await }

/// Leitura do lado servidor: o cliente é obrigado a mascarar.
pub(crate) async fn read_client_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<Frame, Error> { read_frame_as(reader, true).await }

async fn read_frame_as(reader: &mut (impl AsyncRead + Unpin), masked: bool) -> Result<Frame, Error> {
    let mut head = [0; 2];
    reader.read_exact(&mut head).await.map_err(|_| Error::Network)?;
    let (opcode, fin, short) = (head[0] & 15, head[0] & 128 != 0, head[1] & 127);
    if head[0] & 0x70 != 0 || (head[1] & 128 != 0) != masked || !matches!(opcode, 0 | 1 | 2 | 8 | 9 | 10)
        || (opcode >= 8 && (!fin || short > 125)) { return Err(Error::Protocol); }
    let len = match short {
        126 => reader.read_u16().await.map(u64::from),
        127 => reader.read_u64().await,
        n => Ok(u64::from(n)),
    }.map_err(|_| Error::Network)?;
    if (short == 126 && len < 126) || (short == 127 && (len < 65536 || len >> 63 != 0)) {
        return Err(Error::Protocol);
    }
    if len > MAX_MESSAGE as u64 { return Err(Error::TooLarge); }
    let mut mask = [0; 4];
    if masked { reader.read_exact(&mut mask).await.map_err(|_| Error::Network)?; }
    let mut data = vec![0; len as usize];
    reader.read_exact(&mut data).await.map_err(|_| Error::Network)?;
    if masked { for (ix, byte) in data.iter_mut().enumerate() { *byte ^= mask[ix % 4]; } }
    Ok(Frame { opcode, fin, data })
}

#[derive(Default)]
pub(crate) struct Message { opcode: Option<u8>, data: Vec<u8> }

impl Message {
    pub(crate) fn push(&mut self, frame: Frame) -> Result<Option<Vec<u8>>, Error> {
        match (self.opcode, frame.opcode) {
            (None, 1 | 2) => self.opcode = Some(frame.opcode),
            (Some(_), 0) => {},
            _ => return Err(Error::Protocol),
        }
        if self.data.len() + frame.data.len() > MAX_MESSAGE { return Err(Error::TooLarge); }
        self.data.extend(frame.data);
        if !frame.fin { return Ok(None); }
        if self.opcode == Some(1) && std::str::from_utf8(&self.data).is_err() { return Err(Error::Utf8); }
        self.opcode = None;
        Ok(Some(std::mem::take(&mut self.data)))
    }
}

pub(crate) fn close_code(bytes: &[u8]) -> Result<Option<u16>, Error> {
    if bytes.is_empty() { return Ok(None); }
    if bytes.len() == 1 { return Err(Error::Protocol); }
    let code = u16::from_be_bytes([bytes[0], bytes[1]]);
    if !matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999) { return Err(Error::Protocol); }
    std::str::from_utf8(&bytes[2..]).map_err(|_| Error::Utf8)?;
    Ok(Some(code))
}

async fn deliver<E>(output: &Sender<Result<E, Error>>, event: E, stop: &mut oneshot::Receiver<()>) -> Result<(), Error> {
    tokio::select! {
        biased;
        _ = stop => Err(Error::Closed),
        result = timeout(IO_TIMEOUT, output.send(Ok(event))) => result.map_err(|_| Error::QueueFull)?
            .map_err(|_| Error::Closed),
    }
}

async fn pump<E: Incoming>(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), input: Receiver<Frame>,
    output: &Sender<Result<E, Error>>, stop: &mut oneshot::Receiver<()>) -> Result<Option<u16>, Error> {
    deliver(output, E::CONNECTED, stop).await?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut message = Message::default();
    // `Message` esquece o opcode ao completar; a continuação (0) herda o do primeiro quadro.
    let mut kind = 0;
    loop {
        let frame = {
            // Preserva a leitura parcial enquanto chegam teclas ou redimensionamentos.
            let next = read_frame(&mut reader);
            tokio::pin!(next);
            loop {
                tokio::select! {
                    _ = &mut *stop => return Err(Error::Closed),
                    frame = &mut next => break frame?,
                    command = input.recv() => {
                        let command = command.map_err(|_| Error::Closed)?;
                        write_frame(&mut writer, command.opcode, &command.data).await?;
                    }
                }
            }
        };
        match frame.opcode {
            8 => {
                let code = close_code(&frame.data)?;
                write_frame(&mut writer, 8, &frame.data).await?;
                return Ok(code);
            }
            9 => write_frame(&mut writer, 10, &frame.data).await?,
            10 => {},
            _ => {
                if frame.opcode != 0 { kind = frame.opcode; }
                if let Some(bytes) = message.push(frame)? { deliver(output, E::message(kind, bytes)?, stop).await?; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    fn server_frame(opcode: u8, fin: bool, data: &[u8]) -> Vec<u8> {
        let mut wire = encode(opcode, data, [0; 4]);
        let offset = match wire[1] & 127 { 126 => 4, 127 => 10, _ => 2 };
        wire[0] = opcode | if fin { 128 } else { 0 };
        wire[1] &= 127;
        wire.drain(offset..offset + 4);
        wire
    }

    #[test]
    fn hangar_terminal_uses_its_own_route_and_never_the_session_one() {
        let api = Api::new("http://127.0.0.1:8765", "").unwrap();
        let path = |url: url::Url| (url.path().to_owned(), url.query().unwrap_or("").to_owned());
        assert_eq!(path(terminal_url(&api, "pm-1", Some("ab12"), true, " tok ", 80, 24)),
            ("/api/hangar-terminals/ab12/term".to_owned(), "token=tok&cols=80&rows=24".to_owned()));
        assert_eq!(path(terminal_url(&api, "pm-1", Some("ab12"), false, "tok", 80, 24)),
            ("/api/sessions/pm-1/term".to_owned(), "token=tok&cols=80&rows=24&shortcut=ab12".to_owned()));
        assert_eq!(path(terminal_url(&api, "pm-1", None, false, "tok", 80, 24)).0, "/api/sessions/pm-1/term");
    }

    #[tokio::test]
    async fn framing_lengths_masks_fragments_and_rejections() {
        let mut buffered = tokio::io::BufWriter::new(Vec::new());
        write_frame(&mut buffered, 2, b"flush").await.unwrap();
        assert_eq!(buffered.get_ref().len(), 11);
        assert_eq!(encode(1, b"Hello", [0x37, 0xfa, 0x21, 0x3d]),
            [0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58]);
        for len in [0, 125, 126, 65535, 65536] {
            let bytes = vec![42; len];
            let encoded = encode(2, &bytes, [1, 2, 3, 4]);
            assert_ne!(encoded[1] & 128, 0);
            assert_eq!(encoded[1] & 127, if len <= 125 { len as u8 } else if len <= 65535 { 126 } else { 127 });
            let offset = encoded.len() - len;
            assert_eq!(encoded[offset..].iter().enumerate().map(|(ix, b)| b ^ [1, 2, 3, 4][ix % 4]).collect::<Vec<_>>(), bytes);
            assert_eq!(read_frame(&mut server_frame(2, true, &bytes).as_slice()).await.unwrap().data, bytes);
        }
        let mut message = Message::default();
        for (op, fin, bytes) in [(1, false, &[0xc3][..]), (0, true, &[0xa9][..])] {
            let frame = read_frame(&mut server_frame(op, fin, bytes).as_slice()).await.unwrap();
            assert_eq!(message.push(frame).unwrap(), if fin { Some("é".as_bytes().to_vec()) } else { None });
        }
        for wire in [&[0x82, 0x80][..], &[0xc2, 0], &[0x83, 0], &[0x09, 0], &[0x89, 126], &[0x82, 126, 0, 1]] {
            assert!(matches!(read_frame(&mut &wire[..]).await, Err(Error::Protocol)));
        }
        let mut oversized = vec![0x82, 127];
        oversized.extend_from_slice(&(MAX_MESSAGE as u64 + 1).to_be_bytes());
        assert!(matches!(read_frame(&mut oversized.as_slice()).await, Err(Error::TooLarge)));
        let mut fragments = Message { opcode: Some(2), data: vec![0; MAX_MESSAGE] };
        assert!(matches!(fragments.push(Frame { opcode: 0, fin: true, data: vec![0] }), Err(Error::TooLarge)));
        assert!(matches!(message.push(Frame { opcode: 0, fin: true, data: vec![] }), Err(Error::Protocol)));
        assert!(matches!(message.push(Frame { opcode: 1, fin: true, data: vec![255] }), Err(Error::Utf8)));
        assert_eq!(close_code(&[3, 232]), Ok(Some(1000)));
        assert_eq!(close_code(&[]), Ok(None));
        for bytes in [&[0][..], &[3, 237], &[3, 232, 255]] { assert!(close_code(bytes).is_err()); }
    }

    #[test]
    fn handshake_and_safe_http_errors() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        assert_eq!(accept_key(key), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        let mut headers = header::HeaderMap::new();
        headers.insert(header::UPGRADE, "websocket".parse().unwrap());
        headers.insert(header::CONNECTION, "keep-alive, Upgrade".parse().unwrap());
        headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept_key(key).parse().unwrap());
        assert_eq!(check_handshake(StatusCode::SWITCHING_PROTOCOLS, &headers, key), Ok(()));
        headers.insert(header::SEC_WEBSOCKET_ACCEPT, "invalid".parse().unwrap());
        assert_eq!(check_handshake(StatusCode::SWITCHING_PROTOCOLS, &headers, key), Err(Error::Handshake));
        for status in [401, 403, 404, 409] {
            let error = check_handshake(StatusCode::from_u16(status).unwrap(), &headers, key).unwrap_err();
            assert_eq!(error, Error::Http(status));
            assert!(!error.to_string().is_empty());
        }
    }

    #[tokio::test]
    async fn ping_between_fragments_and_close_are_masked() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let (_commands, input) = async_channel::bounded(1);
        let (output, events) = async_channel::bounded(4);
        let (_stop, mut stopped) = oneshot::channel();
        let peer = tokio::spawn(async move {
            server.write_all(&[0x02, 1, b'a', 0x89, 1, b'p', 0x80, 1, b'b', 0x88, 2, 3, 232]).await.unwrap();
            let mut pong = [0; 7];
            server.read_exact(&mut pong).await.unwrap();
            assert_eq!(&pong[..2], &[0x8a, 0x81]);
            assert_eq!(pong[6] ^ pong[2], b'p');
            let mut close = [0; 8];
            server.read_exact(&mut close).await.unwrap();
            assert_eq!(&close[..2], &[0x88, 0x82]);
            assert_eq!([close[6] ^ close[2], close[7] ^ close[3]], [3, 232]);
        });
        assert_eq!(pump(&mut client, input, &output, &mut stopped).await, Ok(Some(1000)));
        assert!(matches!(events.recv().await, Ok(Ok(Event::Connected))));
        assert!(matches!(events.recv().await, Ok(Ok(Event::Data(data))) if data == b"ab"));
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn real_upgrade_input_resize_and_local_close() {
        timeout(Duration::from_secs(3), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = Api::new(&format!("http://{}", listener.local_addr().unwrap()), "").unwrap();
            let peer = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") { request.push(socket.read_u8().await.unwrap()); }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("GET /api/sessions/fixture/term?token="));
                assert!(request.lines().next().unwrap().contains(&STANDARD.encode([17; 24])));
                assert!(request.lines().next().unwrap().contains("&cols=500&rows=5 HTTP/1.1"));
                let key = request.lines().find_map(|line| line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key")).map(|(_, v)| v.trim())).unwrap();
                assert_eq!(STANDARD.decode(key).unwrap().len(), 16);
                socket.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_key(key)).as_bytes()).await.unwrap();
                for opcode in [2, 1, 8] {
                    let mut head = [0; 2];
                    socket.read_exact(&mut head).await.unwrap();
                    assert_eq!(head[0], 128 | opcode);
                    assert_ne!(head[1] & 128, 0);
                    let mut mask = [0; 4];
                    socket.read_exact(&mut mask).await.unwrap();
                    let mut data = vec![0; (head[1] & 127) as usize];
                    socket.read_exact(&mut data).await.unwrap();
                    for (ix, b) in data.iter_mut().enumerate() { *b ^= mask[ix % 4]; }
                    match opcode {
                        2 => assert_eq!(data, b"echo hi\r"),
                        1 => assert_eq!(serde_json::from_slice::<serde_json::Value>(&data).unwrap(),
                            serde_json::json!({"t":"resize", "cols":500, "rows":5})),
                        _ => assert_eq!(data, [3, 232]),
                    }
                    if opcode == 1 { socket.write_all(&[0x82, 1, b'!']).await.unwrap(); }
                }
            });
            let mut terminal = Terminal::open(&Handle::current(), &api, "fixture", None, false, STANDARD.encode([17; 24]), 999, 0);
            let events = terminal.events();
            assert!(matches!(events.recv().await, Ok(Ok(Event::Connected))));
            terminal.send(b"echo hi\r").unwrap();
            terminal.resize(999, 0).unwrap();
            assert!(matches!(events.recv().await, Ok(Ok(Event::Data(bytes))) if bytes == b"!"));
            terminal.close();
            assert_eq!(terminal.send(b"late"), Err(Error::Closed));
            assert!(matches!(events.recv().await, Ok(Ok(Event::Closed))));
            peer.await.unwrap();
            (&mut terminal.task).await.unwrap();
        }).await.unwrap();
    }

    /// Aceita um cliente em `/api/voice?token=t` e devolve cada quadro de texto recebido.
    async fn echo_text_server() -> (std::net::SocketAddr, JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") { request.push(socket.read_u8().await.unwrap()); }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("GET /api/voice?token=t HTTP/1.1"));
            let key = request.lines().find_map(|line| line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key")).map(|(_, v)| v.trim())).unwrap();
            socket.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_key(key)).as_bytes()).await.unwrap();
            while let Ok(frame) = read_client_frame(&mut socket).await {
                if frame.opcode != 1 { break; }
                write_server_frame(&mut socket, 1, &frame.data).await.unwrap();
            }
        });
        (addr, server)
    }

    #[tokio::test]
    async fn text_socket_round_trips_text_frames() {
        timeout(Duration::from_secs(3), async {
            let (addr, _server) = echo_text_server().await;
            let api = Api::new(&format!("http://{addr}"), "t").unwrap();
            let socket = TextSocket::open(&Handle::current(), &api, "/api/voice", "t".to_owned());
            let events = socket.events();
            assert!(matches!(events.recv().await, Ok(Ok(TextEvent::Connected))));
            socket.send(r#"{"type":"ping"}"#).unwrap();
            assert!(matches!(events.recv().await, Ok(Ok(TextEvent::Text(t))) if t == r#"{"type":"ping"}"#));
        }).await.unwrap();
    }
}
