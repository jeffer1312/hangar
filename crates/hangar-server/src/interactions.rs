//! Adaptação da identidade das interações ao runtime e ao HTTP do servidor.
pub use hangar_interactions::{
    ExpectedInteraction, InteractionBinding, InteractionError, InteractionKind, InteractionStore,
    InteractionTarget, error_code, native_target, plugin_semantic, plugin_target,
};

pub fn runtime_error(error: InteractionError) -> crate::runtime::protocol::RuntimeError {
    crate::runtime::protocol::RuntimeError::new(error.code(), error.message())
}

/// Cartão exibido que não ganhou ID: o código vai à faixa sem apagar um problema que já estava lá.
/// `runtime_falhou` porque é o código que web, app e nativo traduzem; quem chama tira as opções.
pub fn mark_unidentified(state: &mut hangar_api::state::StateEvent, error: InteractionError) {
    let code = error.code();
    if state.problema.is_none() {
        state.problema = Some("runtime_falhou".into());
        state.problema_detalhe = Some(code.into());
        return;
    }
    state.problema_detalhe = Some(match state.problema_detalhe.take() {
        Some(detail) if detail.contains(code) => detail,
        Some(detail) if !detail.is_empty() => format!("{detail} · {code}"),
        _ => code.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::InteractionError;
    use serde_json::{Value, json};

    #[tokio::test]
    async fn conversation_identity_errors_keep_the_public_envelope() {
        use axum::body::to_bytes;
        let cases = [
            (InteractionError::Missing, 409, "erro_interacao_sem_identidade", "Atualize o app ou recarregue a página antes de responder."),
            (InteractionError::Changed, 409, "erro_interacao_desatualizada", "Essa pergunta mudou ou já foi encerrada. Confira a pergunta atual."),
            (InteractionError::Unavailable, 503, "erro_interacao_indisponivel", "Não foi possível conferir a pergunta. Nenhuma resposta foi enviada."),
        ];
        for (error, status, code, message) in cases {
            let response = crate::session_write::detail(error.status(), error.code(), error.message(), json!({}));
            assert_eq!(response.status().as_u16(), status);
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), json!({
                "detail": {"code": code, "params": {}, "msg": message}
            }));
        }
    }
}
