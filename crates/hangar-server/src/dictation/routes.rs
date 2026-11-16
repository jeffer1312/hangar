use super::{model::*, service};
use crate::routes::AppState;

/// A causa vai ao diário (nunca o corpo, que leva chaves); quem chama só preserva o cru.
pub(crate) async fn configuration(state: &AppState) -> Result<OrganizationConfig, ()> {
    let failed = |cause: &str| {
        tracing::warn!(
            code = "dictation_rust_unavailable",
            cause,
            "configuração da organização do ditado indisponível"
        );
    };
    let mut snapshot = crate::transcription::routes::configuration_snapshot(state)
        .await.map_err(|_| failed("configuration"))?;
    serde_json::from_value(snapshot["organization"].take())
        .map_err(|_| failed("organization_invalid"))
}

/// A organização de um pedido já lido; falha nunca vira erro, só o texto cru com o motivo.
pub(crate) async fn organize_request(
    state: &AppState,
    request: OrganizationRequest,
    started: tokio::time::Instant,
) -> OrganizationResult {
    if request.mode == Some(OrganizationMode::None) {
        return OrganizationResult::original(request.raw, OrganizationMode::None);
    }
    use super::timing::{Step, timed};
    let deadline = started + std::time::Duration::from_secs(120);
    let result = match tokio::time::timeout_at(deadline, timed(Step::Config, configuration(state))).await {
        Ok(Ok(config)) => timed(Step::Organize, service::organize_started(state, request, &config, started)).await,
        Err(_) => OrganizationResult::original(request.raw, request.mode.unwrap_or_default())
            .failed("dictation_organization_timeout", super::harness::detail("dictation_organization_timeout")),
        Ok(Err(())) => OrganizationResult::original(request.raw, request.mode.unwrap_or_default())
            .failed("dictation_rust_unavailable", "A configuração de organização está indisponível; foi mantida a transcrição original."),
    };
    super::timing::organized(result.organization_code.as_deref());
    result
}
