//! Validação da importação nativa e espera pela conclusão da mesma operação.
use super::native::Native;
use serde_json::{Value, json};
use std::{future::Future, time::Duration};

trait Transport: Send + Sync {
    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> impl Future<Output = Result<Value, String>> + Send;
    fn discard_events(&mut self);
    fn event(&mut self) -> impl Future<Output = Option<(String, Value)>> + Send;
}

impl Transport for Native {
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        Native::request(self, method, params, timeout).await
    }
    fn discard_events(&mut self) {
        while self.events.try_recv().is_ok() {}
    }
    async fn event(&mut self) -> Option<(String, Value)> {
        self.events.recv().await
    }
}

pub async fn detect(native: &Native, timeout: Duration) -> Result<Vec<Value>, String> {
    detect_with(native, timeout).await
}
pub async fn histories(native: &Native, timeout: Duration) -> Result<Vec<Value>, String> {
    histories_with(native, timeout).await
}
pub async fn import(
    native: &mut Native,
    items: Vec<Value>,
    timeout: Duration,
) -> Result<Value, String> {
    import_with(native, items, timeout).await
}

fn objects(
    mut response: crate::config_sync::runtime_values::ValueGuard,
    field: &str,
    error: &str,
) -> Result<Vec<Value>, String> {
    if !response.is_object() {
        return Err("O Codex retornou uma resposta nativa inválida.".into());
    }
    if !response[field]
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_object))
    {
        return Err(error.into());
    }
    let Value::Array(items) = response.get_mut(field).unwrap().take() else {
        unreachable!()
    };
    Ok(items)
}

async fn detect_with(native: &impl Transport, timeout: Duration) -> Result<Vec<Value>, String> {
    let response = crate::config_sync::runtime_values::ValueGuard(
        native
            .request(
                "externalAgentConfig/detect",
                json!({"includeHome":true,"cwds":[],"maxSessions":0}),
                timeout,
            )
            .await?,
    );
    if ["sourceErrors", "errors", "warnings"]
        .into_iter()
        .any(|field| crate::config_sync::apply::truthy(&response[field]))
    {
        return Err(
            "O Codex informou falhas ou avisos na detecção; fontes existentes foram preservadas."
                .into(),
        );
    }
    objects(response, "items", "O Codex retornou uma detecção inválida.")
}

async fn histories_with(native: &impl Transport, timeout: Duration) -> Result<Vec<Value>, String> {
    let response = crate::config_sync::runtime_values::ValueGuard(
        native
            .request(
                "externalAgentConfig/import/readHistories",
                Value::Null,
                timeout,
            )
            .await?,
    );
    objects(
        response,
        "data",
        "O Codex retornou um histórico de importação inválido.",
    )
}

async fn import_with(
    native: &mut impl Transport,
    items: Vec<Value>,
    timeout: Duration,
) -> Result<Value, String> {
    native.discard_events();
    let operation = async {
        // O prazo externo cobre requisição e conclusão; o interno não antecipa seu diagnóstico.
        let response = crate::config_sync::runtime_values::ValueGuard(
            native
                .request(
                    "externalAgentConfig/import",
                    json!({"migrationItems":items,"source":"hangar"}),
                    timeout.saturating_add(Duration::from_secs(1)),
                )
                .await?,
        );
        let id = response["importId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("O Codex não retornou o identificador da importação.")?
            .to_owned();
        loop {
            let (method, params) = native
                .event()
                .await
                .ok_or("O Codex encerrou antes de concluir a importação.")?;
            let mut params = crate::config_sync::runtime_values::ValueGuard(params);
            if method != "externalAgentConfig/import/completed"
                || params["importId"].as_str() != Some(&id)
            {
                continue;
            }
            if !params["itemTypeResults"].is_array() {
                return Err("O Codex retornou uma conclusão inválida.".into());
            }
            return Ok(params.0.take());
        }
    };
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| "O Codex excedeu o tempo limite para concluir a importação.".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    struct Fixture {
        response: Value,
        events: Vec<Value>,
        delayed: bool,
        calls: Mutex<Vec<Value>>,
        sender: tokio::sync::mpsc::UnboundedSender<Option<Value>>,
        receiver: tokio::sync::mpsc::UnboundedReceiver<Option<Value>>,
    }
    impl Transport for Fixture {
        async fn request(
            &self,
            method: &str,
            params: Value,
            _timeout: Duration,
        ) -> Result<Value, String> {
            self.calls
                .lock()
                .unwrap()
                .push(json!({"method":method,"params":params}));
            let events = self.events.clone();
            let sender = self.sender.clone();
            let send = move || {
                for event in events {
                    sender.send((!event.is_null()).then_some(event)).unwrap();
                }
            };
            if self.delayed {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    send();
                });
            } else {
                send();
            }
            Ok(self.response.clone())
        }
        fn discard_events(&mut self) {
            while self.receiver.try_recv().is_ok() {}
        }
        async fn event(&mut self) -> Option<(String, Value)> {
            self.receiver
                .recv()
                .await
                .flatten()
                .map(|value| ("externalAgentConfig/import/completed".into(), value))
        }
    }

    #[tokio::test]
    async fn import_api_matches_original_validation_and_completion() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../backend/tests/fixtures/contract/codex-integration/import-api.json"
        ))
        .unwrap();
        let mut differences = Vec::new();
        for (index, row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut native = Fixture {
                response: row["response"].clone(),
                events: row["events"].as_array().cloned().unwrap_or_default(),
                delayed: row["delayed"].as_bool().unwrap(),
                calls: Mutex::new(Vec::new()),
                sender,
                receiver,
            };
            let timeout = Duration::from_millis(20);
            let result = match row["operation"].as_str().unwrap() {
                "detectar" => detect_with(&native, timeout).await.map(Value::Array),
                "historicos_importacao" => histories_with(&native, timeout).await.map(Value::Array),
                "importar" => import_with(&mut native, Vec::new(), timeout).await,
                _ => unreachable!(),
            };
            let actual = match result {
                Ok(value) => json!({"ok":value}),
                Err(error) => json!({"error":error}),
            };
            let calls = json!(*native.calls.lock().unwrap());
            if actual != row["result"] || calls != row["calls"] {
                differences.push((index, actual, calls));
            }
        }
        assert!(
            differences.is_empty(),
            "importação divergente: {differences:?}"
        );
    }
}
