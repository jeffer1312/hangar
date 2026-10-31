//! O que a pessoa já viu do tour, guardado também em cada servidor (`crates/hangar-server/src/tour_progress.rs`): outro
//! aparelho da mesma pessoa não mostra de novo o que este já mostrou.
use super::{Api, Failure};
use crate::tour::state::Seen;

fn parse(value: serde_json::Value) -> Result<Seen, Failure> {
    serde_json::from_value(value).map_err(|_| Failure::local("invalid_response"))
}

impl Api {
    /// `None`: servidor de versão antiga, sem a rota; vale o local.
    pub async fn read_tour_progress(&self) -> Result<Option<Seen>, Failure> {
        match self.server_read(&["tour-progress"], &[], 10).await {
            Err(error) if error.status == Some(404) => Ok(None),
            result => result.and_then(parse).map(Some),
        }
    }

    /// Devolve o estado já juntado pelo servidor.
    pub async fn save_tour_progress(&self, seen: &Seen) -> Result<Seen, Failure> {
        let body = serde_json::to_value(seen.for_server()).map_err(|_| Failure::local("invalid_request"))?;
        parse(self.server_send(reqwest::Method::PUT, &["tour-progress"], Some(body), 10).await?)
    }

    /// Lê o servidor, junta com o local e manda a união de volta. O `PUT` que falha só vai ao diário: a leitura já
    /// trouxe o que o servidor sabia.
    pub async fn sync_tour_progress(&self, local: Seen) -> Result<Option<Seen>, Failure> {
        let Some(server) = self.read_tour_progress().await? else { return Ok(None) };
        let mut union = local;
        union.merge(server);
        match self.save_tour_progress(&union).await {
            Ok(joined) => union.merge(joined),
            Err(error) => crate::log_line(&format!("tour: o visto não foi enviado ao servidor: {} {}",
                error.status.unwrap_or(0), error.detail)),
        }
        Ok(Some(union))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tour::state::Outcome;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    /// Responde em ordem e devolve a linha de pedido e o corpo de cada um.
    fn fake_server(listener: tokio::net::TcpListener, replies: Vec<(&'static str, Value)>)
        -> tokio::task::JoinHandle<Vec<(String, Option<Value>)>> {
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, reply) in replies {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    assert!(stream.read_line(&mut line).await.unwrap() > 0);
                    head.push_str(&line);
                    if line == "\r\n" { break; }
                }
                let length = head.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:")
                    .map(|n| n.trim().parse::<usize>().unwrap())).unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                requests.push((head.lines().next().unwrap().to_owned(), (!body.is_empty())
                    .then(|| serde_json::from_slice::<Value>(&body).unwrap())));
                let reply = reply.to_string();
                stream.get_mut().write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes()).await.unwrap();
            }
            requests
        })
    }

    #[tokio::test]
    async fn server_done_wins_over_local_nothing_and_the_union_goes_back_without_devices() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let done = json!({"tour": "feito", "dicas": ["fila"], "dicas_desligadas": null});
        let server = fake_server(listener, vec![
            ("200 OK", done),
            ("200 OK", json!({"tour": "feito", "dicas": ["citar", "fila"], "dicas_desligadas": null})),
        ]);
        let api = Api::new(&format!("http://{address}"), "fixture-owner").unwrap();
        let local = Seen { dicas: ["citar".to_owned()].into(), aparelhos: ["ios:abc".to_owned()].into(), ..Default::default() };
        let joined = api.sync_tour_progress(local).await.unwrap().unwrap();
        assert_eq!(joined.tour, Some(Outcome::Feito));
        assert_eq!(joined.dicas, ["citar".to_owned(), "fila".to_owned()].into());

        let requests = server.await.unwrap();
        assert_eq!(requests.iter().map(|(head, _)| head.split_whitespace().take(2).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>(),
            ["GET /api/tour-progress", "PUT /api/tour-progress"]);
        let body = requests[1].1.as_ref().unwrap();
        assert_eq!(body["tour"], "feito");
        assert!(body.get("aparelhos").is_none(), "o servidor recusa campo a mais");

        // O local passa a `feito` e o tour que ainda não apareceu não abre.
        let step = crate::tour::Step { id: "new", anchor: crate::tour::anchors::COMPOSER, title_key: "tour_later", body_key: "tour_later" };
        let mut tour = crate::tour::Tour::with_steps(Seen::default(), vec![step]);
        assert!(tour.running());
        assert!(tour.absorb(joined));
        assert_eq!(tour.seen.tour, Some(Outcome::Feito));
        assert!(!tour.running());
        assert!(tour.server_read);
    }

    #[tokio::test]
    async fn old_server_without_the_route_is_no_state_and_no_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = fake_server(listener, vec![("404 Not Found", json!({"detail": "Not Found"}))]);
        let api = Api::new(&format!("http://{address}"), "fixture-owner").unwrap();
        assert_eq!(api.sync_tour_progress(Seen::default()).await.unwrap(), None);
        assert_eq!(server.await.unwrap().len(), 1, "sem a rota, nada de PUT");
    }

    #[tokio::test]
    async fn unreadable_on_the_server_is_an_error_for_the_caller_to_log() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = fake_server(listener, vec![("503 Service Unavailable", json!({"detail": {"code": "tour_progress_unreadable"}}))]);
        let api = Api::new(&format!("http://{address}"), "fixture-owner").unwrap();
        assert_eq!(api.sync_tour_progress(Seen::default()).await.unwrap_err().status, Some(503));
        server.abort();
    }
}
