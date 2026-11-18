//! Antes da TUI, aceita os hooks pendentes que o Hangar sincroniza (os da conta e os de plugin), pelo
//! escritor do próprio Codex (`config/batchWrite`). Sem isto a TUI para em "Hooks need review" antes da
//! thread e quem abriu pelo app vê a sessão travada. Falha nunca impede a abertura: a TUI pergunta.
use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::codex_setup::reload::{self, RpcFailure};

/// Prazo total da troca: notificações em sequência não seguram a abertura.
const DEADLINE: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Trust {
    /// Aceitos pelo Hangar e os de outra origem, que ficam para a TUI perguntar.
    Done { accepted: usize, manual: usize },
    /// A etapa em que parou e o que aconteceu, sem a mensagem do erro.
    Failed { stage: &'static str, detail: String },
}

fn detail(failure: RpcFailure) -> String {
    match failure {
        RpcFailure::Refused(Some(code)) => format!("código {code}"),
        RpcFailure::Refused(None) => "recusado".into(),
        RpcFailure::Timeout => "TimeoutError".into(),
        RpcFailure::Closed => "ConnectionClosed".into(),
        RpcFailure::Transport => "ConnectionError".into(),
        RpcFailure::Invalid => "ValueError".into(),
    }
}

pub(crate) async fn trust(endpoint: &str, cwd: &str, home: &Path, account: &Path) -> Trust {
    let deadline = Instant::now() + DEADLINE;
    let mut ws = match reload::connect(endpoint).await {
        Ok(ws) => ws,
        Err(failure) => return Trust::Failed { stage: "conectar", detail: detail(failure) },
    };
    let mut stage = "initialize";
    let result = async {
        reload::request(&mut ws, 1, "initialize", json!({"clientInfo": reload::client_info(), "capabilities": null}), deadline).await?;
        stage = "hooks/list";
        let data = reload::request(&mut ws, 2, "hooks/list", json!({"cwds": [cwd]}), deadline).await?;
        let (edits, manual) = crate::codex_setup::trust::edits(&data, home, account, false).map_err(|_| RpcFailure::Invalid)?;
        let accepted = edits.len();
        if !edits.is_empty() {
            stage = "config/batchWrite";
            reload::request(&mut ws, 3, "config/batchWrite", json!({"edits": edits, "reloadUserConfig": true}), deadline).await?;
        }
        Ok::<_, RpcFailure>((accepted, manual))
    }.await;
    reload::close(ws, result.is_ok()).await;
    match result {
        Ok((accepted, manual)) => Trust::Done { accepted, manual },
        Err(failure) => Trust::Failed { stage, detail: detail(failure) },
    }
}

/// As linhas do pane.
pub(crate) fn lines(trust: &Trust) -> Vec<String> {
    match trust {
        Trust::Done { accepted, manual } => {
            let mut lines = Vec::new();
            if *accepted > 0 { lines.push(format!("{accepted} hooks sincronizados pelo Hangar foram aceitos")); }
            if *manual > 0 { lines.push(format!("{manual} hooks de outra origem ficam para o Codex perguntar no terminal")); }
            lines
        }
        Trust::Failed { stage, detail } => vec![format!("não deu para aceitar os hooks ({stage}, {detail}); o Codex vai perguntar no terminal")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use tokio_tungstenite::tungstenite::Message;

    /// Um app-server falso: responde `initialize`, devolve `hooks/list` e o `batchWrite` pedido, e grava o que recebeu.
    async fn fake(hooks: Value, batch: Value) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut seen = Vec::new();
            // Notificações antes da resposta não atrapalham.
            for _ in 0..3 { ws.send(Message::Text(json!({"method": "x/notified", "params": {}}).to_string().into())).await.unwrap(); }
            while let Some(Ok(message)) = ws.next().await {
                if !message.is_text() { break; }
                let request: Value = serde_json::from_slice(&message.into_data()).unwrap();
                let reply = match request["method"].as_str().unwrap() {
                    "initialize" => json!({"id": request["id"], "result": {}}),
                    "hooks/list" => json!({"id": request["id"], "result": hooks}),
                    _ => { let mut reply = batch.clone(); reply["id"] = request["id"].clone(); reply }
                };
                seen.push(request);
                ws.send(Message::Text(reply.to_string().into())).await.unwrap();
            }
            seen
        });
        (endpoint, task)
    }

    fn hooks(home: &Path) -> Value {
        // `launcher__hooks-trust-ok`: três do Hangar (conta, plugin, o `hooks.json` da casa listado como de
        // projeto), um de projeto pendente, um desligado e um já confiado.
        let own = format!("{}:stop:0:0", home.join(".codex/hooks.json").to_string_lossy().replace('\\', "/"));
        json!({"data": [{"cwd": "/w", "hooks": [
            {"key": "user:Stop:0", "source": "user", "enabled": true, "trustStatus": "untrusted", "currentHash": "h1"},
            {"key": "plugin:x:Stop:0", "source": "plugin", "enabled": true, "trustStatus": "modified", "currentHash": "h2"},
            {"key": "project:Stop:0", "source": "project", "enabled": true, "trustStatus": "untrusted", "currentHash": "h3"},
            {"key": own, "source": "project", "enabled": true, "trustStatus": "untrusted", "currentHash": "h4"},
            {"key": "user:Pre:0", "source": "user", "enabled": false, "trustStatus": "untrusted", "currentHash": "h5"},
            {"key": "user:Post:0", "source": "user", "enabled": true, "trustStatus": "trusted", "currentHash": "h6"}]}]})
    }

    #[tokio::test]
    async fn only_the_hooks_synced_by_the_hangar_are_accepted() {
        let home = tempfile::tempdir().unwrap();
        let account = home.path().join(".codex");
        let (endpoint, seen) = fake(hooks(home.path()), json!({"result": {}})).await;
        let trust = trust(&endpoint, "/w", home.path(), &account).await;
        assert_eq!(lines(&trust), ["3 hooks sincronizados pelo Hangar foram aceitos", "1 hooks de outra origem ficam para o Codex perguntar no terminal"]);
        let seen = seen.await.unwrap();
        assert_eq!(seen[0]["params"], json!({"clientInfo": {"name": "hangar", "title": null, "version": "0.1.0"}, "capabilities": null}));
        assert_eq!(seen[1]["params"], json!({"cwds": ["/w"]}));
        let keys: Vec<&str> = seen[2]["params"]["edits"].as_array().unwrap().iter().map(|edit| edit["value"].as_str().unwrap()).collect();
        assert_eq!(keys, ["h1", "h2", "h4"]);
        assert_eq!(seen[2]["params"]["reloadUserConfig"], true);
    }

    /// `launcher__hooks-trust-batch-error` e `-no-code`: a etapa e o código, nunca a mensagem do erro.
    #[tokio::test]
    async fn a_refused_write_shows_the_stage_and_the_code_only() {
        let home = tempfile::tempdir().unwrap();
        let account = home.path().join(".codex");
        for (batch, wanted) in [(json!({"error": {"code": -32603, "message": "segredo interno"}}), "config/batchWrite, código -32603"),
            (json!({"error": {"message": "segredo interno"}}), "config/batchWrite, recusado")] {
            let (endpoint, _seen) = fake(hooks(home.path()), batch).await;
            let trust = trust(&endpoint, "/w", home.path(), &account).await;
            assert_eq!(lines(&trust), [format!("não deu para aceitar os hooks ({wanted}); o Codex vai perguntar no terminal")]);
        }
        // `launcher__hooks-list-with-errors`: entrada com erro recusa a lista inteira.
        let (endpoint, _seen) = fake(json!({"data": [{"cwd": "/w", "errors": ["x"], "hooks": []}]}), json!({"result": {}})).await;
        assert_eq!(lines(&trust(&endpoint, "/w", home.path(), &account).await),
            ["não deu para aceitar os hooks (hooks/list, ValueError); o Codex vai perguntar no terminal"]);
        // `launcher__hooks-list-empty`: nada a dizer.
        let (endpoint, _seen) = fake(json!({"data": []}), json!({"result": {}})).await;
        assert!(lines(&trust(&endpoint, "/w", home.path(), &account).await).is_empty());
    }
}
