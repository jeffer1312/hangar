use std::time::{Duration, Instant};
use async_channel::Sender;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::oneshot;
use super::{Api, Failure, dto::{ChatEvent, ChatEventExt}};

pub struct Frame {
    pub event: String,
    pub data: Value,
    pub applied: oneshot::Sender<bool>,
}

pub enum Update {
    Online,
    Frame(Frame),
    Offline(Failure),
}

/// Espera entre tentativas, dobrando até 30 s. Só o tempo com o fluxo aberto zera a contagem: o prazo de conexão
/// estourado também dura 10 s, e não é sinal de máquina de volta.
#[derive(Default)]
struct Backoff { attempt: u32, up_since: Option<Instant> }

impl Backoff {
    fn attempt_started(&mut self) { self.up_since = None; }
    fn stream_opened(&mut self, now: Instant) { self.up_since = Some(now); }
    fn failed(&mut self, now: Instant, retry_after: Option<u64>) -> u64 {
        if self.up_since.is_some_and(|since| now.duration_since(since) >= Duration::from_secs(10)) { self.attempt = 0; }
        let delay = retry_after.map(|seconds| seconds.max(1)).unwrap_or_else(|| (1u64 << self.attempt.min(5)).min(30));
        self.attempt = self.attempt.saturating_add(1);
        delay
    }
}

pub async fn run(api: Api, name: Option<String>, tx: Sender<Update>) {
    let mut cursor = String::new();
    let mut backoff = Backoff::default();
    loop {
        super::route::ensure(&api).await;
        backoff.attempt_started();
        let outcome = async {
            let response = api.stream(name.as_deref(), &cursor).await?;
            backoff.stream_opened(Instant::now());
            if tx.send(Update::Online).await.is_err() { return Ok(()); }
            let mut events = response.bytes_stream().eventsource();
            let mut wire_id = cursor.clone();
            loop {
                let next = tokio::time::timeout(Duration::from_secs(25), events.next()).await
                    .map_err(|_| Failure::local("network_error"))?;
                let item = match next {
                    Some(Ok(item)) => item,
                    Some(Err(_)) => return Err(Failure::local("invalid_response")),
                    None => return Err(Failure::local("network_error")),
                };
                let data: Value = serde_json::from_str(&item.data).map_err(|_| Failure::local("invalid_response"))?;
                let chat_event = if item.event == "message" || item.event == "queue_confirmed" {
                    Some(serde_json::from_value::<ChatEvent>(data.clone()).map_err(|_| Failure::local("invalid_response"))?)
                } else { None };
                let candidate = if name.is_some() && item.event == "message" && item.id != wire_id
                    && valid_cursor(&item.id) && chat_event.as_ref().is_some_and(|event| !event.queued()) {
                    Some(item.id.clone())
                } else { None };
                wire_id = item.id;
                let (applied, received) = oneshot::channel();
                if tx.send(Update::Frame(Frame { event: item.event.clone(), data, applied })).await.is_err() { return Ok(()); }
                if !received.await.unwrap_or(false) { return Err(Failure::local("invalid_response")); }
                if item.event == "reset" { cursor.clear(); }
                else if let Some(next) = candidate { cursor = next; }
            }
        }.await;
        if tx.is_closed() { return; }
        let failure = match outcome { Ok(()) => return, Err(e) => e };
        super::route::forget(&api);
        // 410 é convite encerrado: tentar de novo não o traz de volta.
        let stop = matches!(failure.status, Some(401 | 403 | 410));
        let delay = backoff.failed(Instant::now(), failure.retry_after);
        if tx.send(Update::Offline(failure)).await.is_err() || stop { return; }
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

fn valid_cursor(id: &str) -> bool {
    id.rsplit_once(':').is_some_and(|(stem, offset)| !stem.is_empty()
        && !offset.is_empty() && offset.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn connect_timeout_keeps_the_wait_growing_and_a_stream_that_held_resets_it() {
        let mut backoff = Backoff::default();
        // Máquina desligada: cada tentativa gasta os 10 s do prazo de conexão sem abrir o fluxo.
        let delays: Vec<u64> = (0..4).map(|_| {
            backoff.attempt_started();
            backoff.failed(Instant::now() + Duration::from_secs(10), None)
        }).collect();
        assert_eq!(delays, [1, 2, 4, 8]);
        backoff.attempt_started();
        let opened = Instant::now();
        backoff.stream_opened(opened);
        assert_eq!(backoff.failed(opened + Duration::from_secs(10), None), 1);
        assert_eq!(backoff.failed(opened + Duration::from_secs(10), Some(0)), 1, "retry_after manda, com piso de 1 s");
    }

    #[test]
    fn cursor_requires_stem_and_decimal_offset() {
        assert!(valid_cursor("abc:40"));
        for value in ["", "abc:", ":40", "abc:4x"] {
            assert!(!valid_cursor(value), "accepted {value:?}");
        }
    }

    #[tokio::test]
    async fn parser_keeps_split_utf8_crlf_multiline_data_and_inherited_id() {
        let source = ": comment\r\nevent: message\r\ndata: {\"text\":\"a ☃\",\r\ndata: \"id\":1}\r\nid: abc:40\r\n\r\nevent: ping\r\ndata: {}\r\n\r\n";
        let bytes = source.as_bytes();
        let snowman = bytes.windows(3).position(|window| window == "☃".as_bytes()).unwrap();
        let chunks: Vec<Result<Vec<u8>, std::io::Error>> = vec![
            Ok(bytes[..snowman + 1].to_vec()),
            Ok(bytes[snowman + 1..snowman + 2].to_vec()),
            Ok(bytes[snowman + 2..].to_vec()),
        ];
        let mut events = futures::stream::iter(chunks).eventsource();
        let first = events.next().await.unwrap().unwrap();
        assert_eq!(first.event, "message");
        assert_eq!(first.id, "abc:40");
        assert_eq!(first.data, "{\"text\":\"a ☃\",\n\"id\":1}");
        let second = events.next().await.unwrap().unwrap();
        assert_eq!(second.event, "ping");
        assert_eq!(second.id, "abc:40");
        assert_eq!(second.data, "{}");
        assert!(events.next().await.is_none());
    }
}
