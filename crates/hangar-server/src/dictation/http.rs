//! Borda pública do ditado: a transcrição e a organização compartilham os motores em processo.
use crate::{
    routes::{AppState, cors, pass, who},
    session_write::{detail_body, json_response},
    share::caller::{Caller, GuestGrant},
    transcription::model::{Profile, TranscriptionError},
};
use axum::{
    body::to_bytes,
    extract::Request,
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

const AUDIO_LIMIT: usize = 100 * 1024 * 1024;

pub fn matches(method: &Method, path: &str) -> bool {
    match method.as_str() {
        "GET" => matches!(
            path,
            "/api/dictation/models" | "/api/transcription/providers/status"
        ),
        "POST" => {
            matches!(path, "/api/dictation/transcribe" | "/api/ditado/relimpar")
                || provider_id(path).is_some()
        }
        _ => false,
    }
}
fn provider_id(path: &str) -> Option<String> {
    path.strip_prefix("/api/transcription/providers/")?
        .strip_suffix("/test")
        .filter(|id| !id.is_empty() && !id.contains('/'))
        .map(|id| {
            percent_encoding::percent_decode_str(id)
                .decode_utf8_lossy()
                .into_owned()
        })
}
fn reply(status: u16, value: Value) -> Response {
    json_response(
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
        value,
    )
}
fn failure(error: TranscriptionError) -> Response {
    reply(
        error.status,
        detail_body(&error.code, &error.detail, json!({})),
    )
}
fn forbidden(message: &str) -> Response {
    reply(403, json!({"detail":message}))
}

pub async fn public(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let (forward, caller) = who(&st, peer, &req);
    let Some(caller) = caller else {
        return pass(&st, req, &forward).await;
    };
    let headers = req.headers().clone();
    let grant = caller.grant().cloned();
    let mut response = serve(&st, req, matches!(caller, Caller::Guest(_)), grant).await;
    cors(&headers, response.headers_mut());
    response
}

/// Na porta privada, a borda Python já conferiu a sessão do corpo para o convidado com login.
pub async fn serve(
    st: &AppState,
    req: Request,
    guest: bool,
    grant: Option<Arc<GuestGrant>>,
) -> Response {
    let path = req.uri().path().to_owned();
    let query: HashMap<String, String> =
        form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    if path == "/api/ditado/relimpar" {
        return reorganize(st, req, guest, grant).await;
    }
    if path == "/api/dictation/models" {
        let Some(session) = query.get("session") else {
            return reply(
                422,
                json!({"detail":[{"type":"missing","loc":["query","session"],"msg":"Field required","input":null}]}),
            );
        };
        let fresh = match crate::query::bool_param(&query, "fresh") {
            Ok(v) => v,
            Err(e) => return *e,
        };
        if guest {
            return forbidden("O catálogo de organização é exclusivo do dono.");
        }
        let request = super::model::OrganizationRequest {
            session: Some(session.clone()),
            ..Default::default()
        };
        let context = match super::context::resolve(st, &request).await {
            Ok(c) => c,
            Err(code) => {
                return reply(
                    409,
                    json!({"detail":{"code":code,"msg":super::harness::detail(code)}}),
                );
            }
        };
        return match st
            .dictation_models
            .get_context(&st.accounts, &st.dictation_slots, &context, fresh)
            .await
        {
            Ok((models, _)) => reply(
                200,
                json!({"provider":context.provider,"models":models,"generation":context.generation}),
            ),
            Err(code) => reply(
                502,
                json!({"detail":{"code":code,"msg":super::harness::detail(code)}}),
            ),
        };
    }
    if path == "/api/transcription/providers/status" {
        if guest {
            return forbidden("A configuração de transcrição é exclusiva do dono.");
        }
        let Ok(_slot) = st.transcription_slots.clone().try_acquire_owned() else {
            return busy();
        };
        return match crate::transcription::routes::configuration(st).await {
            Ok(config) => {
                st.transcription.configure(config).await;
                reply(200, json!({"providers":st.transcription.status().await}))
            }
            Err(e) => failure(e),
        };
    }
    if let Some(provider) = provider_id(&path) {
        if guest {
            return forbidden("O teste de transcrição é exclusivo do dono.");
        }
        let filename = req
            .headers()
            .get("x-filename")
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .unwrap_or("audio.wav")
            .to_owned();
        let data = match to_bytes(req.into_body(), AUDIO_LIMIT).await {
            Ok(v) => v,
            Err(_) => return reply(413, json!({"detail":"O áudio excede o limite de 100 MiB."})),
        };
        let Ok(_slot) = st.transcription_slots.clone().try_acquire_owned() else {
            return busy();
        };
        let config = match crate::transcription::routes::configuration(st).await {
            Ok(v) => v,
            Err(e) => return failure(e),
        };
        st.transcription.configure(config).await;
        return match st
            .transcription
            .transcribe_provider(&provider, data, Some(filename))
            .await
        {
            Ok(result) => reply(200, json!(result)),
            Err(error) => failure(error),
        };
    }
    let clean = match crate::query::bool_param_or(&query, "limpar", true) {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let include = match query
        .contains_key("include_recent_messages")
        .then(|| crate::query::bool_param(&query, "include_recent_messages"))
        .transpose()
    {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let declared = req
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > AUDIO_LIMIT as u64) {
        return reply(
            413,
            detail_body(
                "erro_arquivo_grande",
                "arquivo maior que 100 MiB",
                json!({}),
            ),
        );
    }
    let filename = req
        .headers()
        .get("x-filename")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .or_else(|| query.get("name").cloned());
    let data = match to_bytes(req.into_body(), AUDIO_LIMIT).await {
        Ok(v) => v,
        Err(_) => {
            return reply(
                413,
                detail_body(
                    "transcription_audio_too_large",
                    "O áudio excede o limite de 100 MiB.",
                    json!({}),
                ),
            );
        }
    };
    let transcription =
        match crate::transcription::routes::transcribe_here(st, data, filename, Profile::Dictation)
            .await
        {
            Ok(v) => v,
            Err(e) => return failure(e),
        };
    let result = if !clean {
        json!({"text":transcription.text})
    } else {
        organize(st,json!({"raw":transcription.text,"style":query.get("estilo"),"mode":query.get("organization_mode"),
            "session":null,"generation":null,"model":query.get("organization_model"),"account":null,"harness_allowed":!guest,
            "include_recent_messages":include,"recent_messages":null})).await
    };
    reply(
        200,
        crate::uploads::transcribe::with_provider(
            result,
            &transcription.provider,
            transcription.aviso.as_deref(),
        ),
    )
}
fn busy() -> Response {
    failure(TranscriptionError {
        status: 503,
        code: "transcription_busy".into(),
        detail: "As vagas de transcrição estão ocupadas. Tente novamente.".into(),
    })
}

async fn reorganize(
    st: &AppState,
    req: Request,
    guest: bool,
    grant: Option<Arc<GuestGrant>>,
) -> Response {
    let bytes = match to_bytes(req.into_body(), usize::MAX).await {
        Ok(v) => v,
        Err(_) => return reply(400, json!({"detail":"There was an error parsing the body"})),
    };
    let value = match crate::orchestration::body::parse(&bytes) {
        Ok(v) => v,
        Err((status, body)) => return json_response(status, body),
    };
    let value = match super::http_body::validate(value) {
        Ok(v) => v,
        Err(e) => return reply(422, e),
    };
    let style = value["estilo"].as_str().unwrap_or("");
    if !matches!(style, "limpar" | "prosa" | "briefing") {
        return reply(
            400,
            detail_body(
                "erro_estilo_invalido",
                &format!("Estilo de organização inválido: '{style}'."),
                json!({}),
            ),
        );
    }
    if let Some(grant) = grant
        && value["session"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && !grant.sessions.contains_key(s))
    {
        return forbidden("A conversa de destino não está autorizada.");
    }
    reply(200,organize(st,json!({"raw":value["texto"],"style":value["estilo"],"mode":value["organization_mode"],
        "session":value["session"],"generation":value["generation"],"model":value["organization_model"],"account":value["organization_account"],
        "harness_allowed":!guest,"include_recent_messages":value["include_recent_messages"],"recent_messages":value["recent_messages"]})).await)
}

/// O tamanho e os erros da antiga ponte são parte da resposta pública: o cru nunca se perde.
async fn organize(st: &AppState, payload: Value) -> Value {
    let original = |code: &str, message: &str| json!({"text":payload["raw"],"raw":payload["raw"],"aviso":message,"estilo_aplicado":"cru","organization_mode":payload["mode"],"organization_code":code});
    if crate::orchestration::codec::dumps(&payload, false).len() > 1024 * 1024 {
        return original(
            "dictation_text_too_large",
            "A transcrição excede o limite de organização.",
        );
    }
    match serde_json::from_value(payload.clone()) {
        Ok(request) => {
            json!(super::routes::organize_request(st, request, tokio::time::Instant::now()).await)
        }
        Err(_) => original(
            "dictation_request_invalid",
            "Pedido de organização inválido.",
        ),
    }
}
