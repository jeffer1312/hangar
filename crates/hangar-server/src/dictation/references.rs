//! Referências de grafia: só texto da conversa, limitado e congelado por tentativa.
use super::{
    context,
    model::{OrganizationRequest, ReferenceMessage},
};
use crate::{
    routes::AppState,
    transcript::{HistoryRequest, Provider, TAIL_WINDOW},
};
use hangar_api::chat::ChatKind;

pub async fn read(
    state: &AppState,
    request: &OrganizationRequest,
) -> Result<Vec<ReferenceMessage>, &'static str> {
    if let Some(snapshot) = &request.recent_messages {
        if snapshot.len() > 3
            || snapshot.iter().any(|m| {
                !matches!(m.role.as_str(), "user" | "assistant") || m.text.chars().count() > 2000
            })
        {
            return Err("dictation_context_unavailable");
        }
        return Ok(snapshot.clone());
    }
    let context = context::resolve(state, request).await?;
    let Some(jsonl) = context.jsonl else {
        return Ok(Vec::new());
    };
    let provider = Provider::parse(&context.provider).ok_or("dictation_context_unavailable")?;
    tokio::task::spawn_blocking(move || {
        let request = HistoryRequest {
            provider,
            jsonl,
            queue: None,
            limit: Some(usize::MAX),
            tail_window: TAIL_WINDOW,
        };
        let history = crate::transcript::history::merged_history_capped(&request, 2 * 1024 * 1024)
            .map_err(|_| "dictation_context_unavailable")?;
        let mut selected = history
            .into_iter()
            .rev()
            .filter_map(|event| {
                let role = match event.kind {
                    ChatKind::UserMsg => "user",
                    ChatKind::AssistantMsg => "assistant",
                    _ => return None,
                };
                let text = event.text.filter(|text| !text.trim().is_empty())?;
                Some(ReferenceMessage {
                    role: role.into(),
                    text: text.chars().take(2000).collect(),
                })
            })
            .take(3)
            .collect::<Vec<_>>();
        selected.reverse();
        Ok(selected)
    })
    .await
    .map_err(|_| "dictation_context_unavailable")?
}
