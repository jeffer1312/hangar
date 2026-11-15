//! Quando a poda roda: uma rodada na hora, depois uma por dia; rodada que não aconteceu tenta de novo logo.
use std::future::Future;
use std::time::Duration;

/// Uma por dia basta: a leitura já recusa sidecar velho, a poda é higiene.
pub const INTERVAL: Duration = Duration::from_secs(24 * 3_600);
/// Rodada que falhou não espera o dia: o servidor reinicia antes disso e a poda nunca rodaria.
pub const RETRY: Duration = Duration::from_secs(5 * 60);

/// Roda `round` agora e depois para sempre; `round` devolve se a rodada aconteceu.
pub async fn every<F, Fut>(mut round: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    loop {
        let wait = if round().await { INTERVAL } else { RETRY };
        tokio::time::sleep(wait).await;
    }
}
