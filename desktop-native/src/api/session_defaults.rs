//! Os padrões são da máquina de destino; o arquivo local antigo só alimenta a primeira importação.
use super::{Api, Failure};
use serde::{Deserialize, Serialize};
use serde_json::json;

type Choice = (String, String, String);

#[derive(Serialize, Deserialize)]
struct Defaults { model: String, effort: String, permission: String }

#[derive(Deserialize)]
struct Selection { configured: bool, defaults: Option<Defaults> }

fn parse(value: serde_json::Value) -> Result<Selection, Failure> {
    serde_json::from_value(value).map_err(|_| Failure::local("invalid_response"))
}

/// As opções da tela Beta que valem para toda abertura no servidor (`/api/session-options`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOptions { pub conversation_beta: bool, pub code_mode: bool }

#[derive(Deserialize)]
struct OptionsSelection { options: SessionOptions }

impl Api {
    pub async fn import_session_defaults(&self) -> Result<(), Failure> {
        let server = self.identity();
        let entries = tokio::task::spawn_blocking(move || crate::appearance::legacy_harness_defaults(&server)).await
            .map_err(|_| Failure::local("invalid_response"))?;
        // A escolha da tela Beta guardada neste computador vai ao servidor uma vez, sem pisar na de lá.
        let look = crate::appearance::get();
        let options = self.save_session_options(SessionOptions { conversation_beta: look.conversation_beta, code_mode: look.claude_code_mode }, true).await;
        let defaults = self.import_session_default_entries(entries).await;
        options.and(defaults)
    }

    pub async fn read_session_options(&self) -> Result<SessionOptions, Failure> {
        let selection: OptionsSelection = serde_json::from_value(self.server_read(&["session-options"], &[], 10).await?)
            .map_err(|_| Failure::local("invalid_response"))?;
        Ok(selection.options)
    }

    pub async fn save_session_options(&self, options: SessionOptions, if_absent: bool) -> Result<SessionOptions, Failure> {
        let selection: OptionsSelection = serde_json::from_value(self.server_send_query(reqwest::Method::PUT, &["session-options"], &[],
            Some(json!({"options": options, "if_absent": if_absent})), 10).await?).map_err(|_| Failure::local("invalid_response"))?;
        Ok(selection.options)
    }

    async fn import_session_default_entries(&self, entries: Vec<(String, Choice)>) -> Result<(), Failure> {
        let mut first_error = None;
        for (scope, choice) in entries {
            let Some((provider, engine)) = scope.split_once(':') else { continue; };
            let (engine, account) = engine.split_once(":account:").unwrap_or((engine, ""));
            let mut query = vec![("provider", provider)];
            if !engine.is_empty() { query.push(("engine", engine)); }
            if !account.is_empty() { query.push(("engine_account", account)); }
            if let Err(error) = self.save_session_default(&query, Some(choice), true).await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub async fn read_session_default(&self, query: &[(&str, &str)], legacy: Option<Choice>) -> Result<Option<Choice>, Failure> {
        let selection = parse(self.server_read(&["session-defaults"], query, 10).await?)?;
        if !selection.configured && legacy.is_some() {
            return self.save_session_default(query, legacy, true).await;
        }
        Ok(selection.defaults.map(|d| (d.model, d.effort, d.permission)))
    }

    pub async fn save_session_default(&self, query: &[(&str, &str)], choice: Option<Choice>, if_absent: bool) -> Result<Option<Choice>, Failure> {
        let defaults = choice.map(|(model, effort, permission)| Defaults { model, effort, permission });
        let selection = parse(self.server_send_query(reqwest::Method::PUT, &["session-defaults"], query,
            Some(json!({"defaults": defaults, "if_absent": if_absent})), 10).await?)?;
        Ok(selection.defaults.map(|d| (d.model, d.effort, d.permission)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn invalid_legacy_entry_does_not_block_other_defaults_or_hide_the_first_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let received = calls.clone();
        let server = tokio::spawn(async move {
            for (status, reply) in [
                ("400 Bad Request", json!({"detail":"session_defaults_invalid"})),
                ("200 OK", json!({"configured":true, "defaults":{"model":"opus", "effort":"high", "permission":"bypassPermissions"}})),
                ("503 Service Unavailable", json!({"detail":"session_defaults_unavailable"})),
            ] {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    assert!(stream.read_line(&mut line).await.unwrap() > 0);
                    if line == "\r\n" { break; }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                received.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let reply = reply.to_string();
                stream.get_mut().write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes()).await.unwrap();
            }
        });
        let api = Api::new(&format!("http://{address}"), "fixture-owner").unwrap();
        let entries = ["obsolete:", "claude:", "codex:"].into_iter()
            .map(|scope| (scope.into(), ("opus".into(), "high".into(), "bypassPermissions".into()))).collect();
        let error = api.import_session_default_entries(entries).await.unwrap_err();
        let count = calls.load(std::sync::atomic::Ordering::SeqCst);
        server.abort();
        assert_eq!(error.status, Some(400));
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn desktop_imports_once_and_keeps_a_newer_default_or_an_explicit_removal() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in [
                json!({"configured":false, "defaults":null}),
                json!({"configured":true, "defaults":{"model":"sonnet", "effort":"low", "permission":"manual"}}),
                json!({"configured":true, "defaults":null}),
                json!({"configured":true, "defaults":null}),
            ] {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    assert!(stream.read_line(&mut line).await.unwrap() > 0);
                    head.push_str(&line);
                    if line == "\r\n" { break; }
                }
                assert!(head.to_ascii_lowercase().contains("authorization: bearer fixture-owner\r\n"));
                let length = head.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:")
                    .map(|n| n.trim().parse::<usize>().unwrap())).unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                requests.push((head.lines().next().unwrap().to_owned(), (!body.is_empty())
                    .then(|| serde_json::from_slice::<serde_json::Value>(&body).unwrap())));
                let reply = reply.to_string();
                stream.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        let api = Api::new(&format!("http://{address}"), "fixture-owner").unwrap();
        let query = [("provider", "claude"), ("engine", "proxy"), ("engine_account", "one")];
        let legacy = Some(("opus".into(), "high".into(), "bypassPermissions".into()));
        assert_eq!(api.read_session_default(&query, legacy.clone()).await.unwrap(), Some(("sonnet".into(), "low".into(), "manual".into())));
        assert_eq!(api.save_session_default(&query, None, false).await.unwrap(), None);
        assert_eq!(api.read_session_default(&query, legacy).await.unwrap(), None);
        let requests = server.await.unwrap();
        assert_eq!(requests.iter().map(|(head, _)| head.split_whitespace().next().unwrap()).collect::<Vec<_>>(), ["GET", "PUT", "PUT", "GET"]);
        assert_eq!(requests[1].1, Some(json!({"defaults":{"model":"opus", "effort":"high", "permission":"bypassPermissions"}, "if_absent":true})));
        assert_eq!(requests[2].1, Some(json!({"defaults":null, "if_absent":false})));
        for (head, _) in requests {
            let uri = url::Url::parse(&format!("http://fixture{}", head.split_whitespace().nth(1).unwrap())).unwrap();
            assert_eq!(uri.path(), "/api/session-defaults");
            assert_eq!(uri.query_pairs().collect::<std::collections::BTreeMap<_, _>>(),
                [("provider".into(), "claude".into()), ("engine".into(), "proxy".into()), ("engine_account".into(), "one".into())].into());
        }
    }
}
