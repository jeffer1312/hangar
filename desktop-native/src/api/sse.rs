use std::time::Duration;
use async_channel::Sender;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::oneshot;
use super::{Api, Failure, dto::ChatEvent};

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

pub async fn run(api: Api, name: Option<String>, tx: Sender<Update>) {
    let mut cursor = String::new();
    let mut attempt = 0u32;
    loop {
        let connected_at = tokio::time::Instant::now();
        let outcome = async {
            let response = api.stream(name.as_deref(), &cursor).await?;
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
        let stop = matches!(failure.status, Some(401 | 403));
        if connected_at.elapsed() >= Duration::from_secs(10) { attempt = 0; }
        let delay = failure.retry_after.map(|seconds| seconds.max(1))
            .unwrap_or_else(|| (1u64 << attempt.min(5)).min(30));
        if tx.send(Update::Offline(failure)).await.is_err() || stop { return; }
        attempt = attempt.saturating_add(1);
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
