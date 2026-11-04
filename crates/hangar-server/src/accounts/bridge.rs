//! Ponte privada de contas para o Python, e a espera pelos fatos de uso (`usage.rs`).
pub use super::types::UsageFacts;
use std::{net::SocketAddr, time::Duration};

/// Cliente privado, vinculado à instância.
#[derive(Clone)]
pub struct AccountsBridge {
    client: reqwest::Client,
    upstream: SocketAddr,
    secret: String,
    instance: String,
}

impl AccountsBridge {
    pub fn instance(&self) -> &str {
        &self.instance
    }

    async fn prepare_request(
        &self,
        suffix: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, &'static str> {
        if !self.upstream.ip().is_loopback() {
            return Err("account_bridge_address");
        }
        let response = self
            .client
            .post(format!(
                "http://{}/internal/accounts/prepare{suffix}",
                self.upstream
            ))
            .header("x-hangar-internal", &self.secret)
            .header("x-hangar-runtime-instance", &self.instance)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|_| "account_prepare_unavailable")?;
        if !response.status().is_success() {
            return Err("account_prepare_rejected");
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "account_prepare_unavailable")?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| "account_prepare_invalid")?;
        if !matches!(
            value["status"].as_str(),
            Some("running" | "ready" | "partial" | "error" | "unknown")
        ) {
            return Err("account_prepare_invalid");
        }
        Ok(value)
    }
    pub async fn preparation_start(
        &self,
        request: &super::preparation::PrepareRequest,
    ) -> Result<serde_json::Value, &'static str> {
        self.prepare_request(
            "",
            serde_json::to_value(request).map_err(|_| "account_prepare_invalid")?,
        )
        .await
    }
    pub async fn preparation_wait(
        &self,
        operation: &str,
    ) -> Result<serde_json::Value, &'static str> {
        self.prepare_request("/wait", serde_json::json!({"operation":operation}))
            .await
    }
    pub fn new(
        upstream: SocketAddr,
        secret: String,
        instance: String,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: crate::http_client()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
            upstream,
            secret,
            instance,
        })
    }
}

/// Relê os fatos até a conta ficar livre ou o prazo vencer. Quem chama segura a guarda exclusiva,
/// então o Hangar não lança nada novo na conta: o `claude` de uma renovação ou janela de login que
/// acabou de fechar, e os MCPs dele, somem sozinhos em segundos; um deles morrendo no meio da
/// varredura deixa a leitura incompleta. Sessão é uso de verdade e recusa na hora.
pub(crate) async fn settle_usage<F, Fut>(mut read: F, budget: Duration) -> UsageFacts
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = UsageFacts>,
{
    let deadline = tokio::time::Instant::now() + budget;
    // Cada leitura varre os processos da máquina: a espera cresce até 2 s.
    let mut pause = Duration::from_millis(250);
    let mut facts = read().await;
    loop {
        let passing = facts.sessions.is_empty()
            && (!facts.complete || !facts.pids.is_empty() || !facts.holders.is_empty());
        let now = tokio::time::Instant::now();
        if !passing || now >= deadline {
            return facts;
        }
        tokio::time::sleep(pause.min(deadline - now)).await;
        pause = (pause * 2).min(Duration::from_secs(2));
        // A releitura cabe no que sobra do prazo; a que não volta fica com a leitura anterior.
        match tokio::time::timeout_at(deadline, read()).await {
            Ok(next) => facts = next,
            Err(_) => return facts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{UsageFacts, settle_usage};
    use std::{cell::Cell, time::Duration};

    fn facts(sessions: &[&str], pids: &[u32], holders: &[u32]) -> UsageFacts {
        UsageFacts {
            complete: true,
            sessions: sessions.iter().map(|s| (*s).to_owned()).collect(),
            pids: pids.to_vec(),
            holders: holders.to_vec(),
        }
    }

    /// Entrega as leituras em ordem e repete a última; devolve também quantas foram feitas.
    async fn settle(readings: Vec<UsageFacts>) -> (UsageFacts, usize) {
        let reads = Cell::new(0);
        let result = settle_usage(
            || {
                let next = readings[reads.get().min(readings.len() - 1)].clone();
                reads.set(reads.get() + 1);
                async move { next }
            },
            Duration::from_secs(10),
        )
        .await;
        (result, reads.get())
    }

    #[tokio::test(start_paused = true)]
    async fn passing_processes_of_a_closed_cli_do_not_block_deletion() {
        // `claude mcp list` da renovação e os MCPs dele somem em segundos: esperar libera a exclusão.
        let busy = facts(&[], &[2508068], &[2508068, 2508228, 2508229]);
        let (result, reads) = settle(vec![busy.clone(), busy, facts(&[], &[], &[])]).await;
        assert_eq!(result, facts(&[], &[], &[]));
        assert_eq!(reads, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_live_session_refuses_without_waiting() {
        let (result, reads) = settle(vec![facts(&["work"], &[7], &[7])]).await;
        assert_eq!(result.sessions, ["work"]);
        assert_eq!(reads, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_process_that_stays_still_refuses_after_the_wait() {
        let started = tokio::time::Instant::now();
        let (result, _) = settle(vec![facts(&[], &[], &[83])]).await;
        assert_eq!(result.holders, [83]);
        assert!(started.elapsed() >= Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn an_incomplete_reading_is_read_again() {
        // Processo que morre no meio da varredura deixa a leitura incompleta; a seguinte já vem limpa.
        let incomplete = UsageFacts { complete: false, ..facts(&[], &[], &[]) };
        let (result, reads) = settle(vec![incomplete, facts(&[], &[], &[])]).await;
        assert_eq!((result, reads), (facts(&[], &[], &[]), 2));
    }

    #[tokio::test(start_paused = true)]
    async fn a_reading_that_never_returns_ends_at_the_deadline() {
        // A releitura não pode esperar o prazo inteiro da ponte depois de vencido o da exclusão.
        let busy = facts(&[], &[7], &[7]);
        let reads = Cell::new(0);
        let started = tokio::time::Instant::now();
        let result = settle_usage(
            || {
                reads.set(reads.get() + 1);
                let first = reads.get() == 1;
                let busy = busy.clone();
                async move {
                    if !first {
                        std::future::pending::<()>().await;
                    }
                    busy
                }
            },
            Duration::from_secs(10),
        )
        .await;
        assert_eq!(result, busy);
        assert!(started.elapsed() <= Duration::from_secs(10), "{:?}", started.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn a_free_account_is_read_once() {
        let (result, reads) = settle(vec![facts(&[], &[], &[])]).await;
        assert_eq!((result, reads), (facts(&[], &[], &[]), 1));
    }
}
