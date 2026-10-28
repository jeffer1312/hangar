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
    response::{IntoResponse, Response},
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
    if declared_too_large(&req) {
        return crate::session_write::too_large(req.headers());
    }
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
    if declared_too_large(&req) {
        return crate::session_write::too_large(req.headers());
    }
    let path = req.uri().path().to_owned();
    let query: HashMap<String, String> =
        form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    if path == "/api/ditado/relimpar" {
        return reorganize(st, req, guest, grant).await;
    }
    if path == "/api/dictation/models" {
        let mut errors = Vec::new();
        if !query.contains_key("session") {
            errors.push(json!({"type":"missing","loc":["query","session"],"msg":"Field required","input":null}));
        }
        let fresh = boolean(&query, "fresh", false, &mut errors);
        if !errors.is_empty() {
            return reply(422, json!({"detail":errors}));
        }
        let session = &query["session"];
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
            Err(_) => return audio_body_error(),
        };
        let result = within_public_deadline(async {
            let _slot = st
                .transcription_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| TranscriptionError {
                    status: 503,
                    code: "transcription_busy".into(),
                    detail: "As vagas de transcrição estão ocupadas. Tente novamente.".into(),
                })?;
            let config = crate::transcription::routes::configuration(st).await?;
            st.transcription.configure(config).await;
            st.transcription
                .transcribe_provider(&provider, data, Some(filename))
                .await
        })
        .await;
        return match result {
            Ok(result) => reply(200, json!(result)),
            Err(error) => failure(error),
        };
    }

    let mut errors = Vec::new();
    let clean = boolean(&query, "limpar", true, &mut errors);
    let include = query
        .contains_key("include_recent_messages")
        .then(|| boolean(&query, "include_recent_messages", false, &mut errors));
    if !errors.is_empty() {
        return reply(422, json!({"detail":errors}));
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
        Err(_) => return audio_body_error(),
    };
    let transcription = match within_public_deadline(crate::transcription::routes::transcribe_here(
        st,
        data,
        filename,
        Profile::Dictation,
    ))
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
    let content_type = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let json_body = content_type.is_empty()
        || content_type.split(';').next().is_some_and(|mime| {
            mime == "application/json"
                || mime.starts_with("application/") && mime.ends_with("+json")
        });
    let bytes = match to_bytes(req.into_body(), AUDIO_LIMIT).await {
        Ok(v) => v,
        Err(_) => return reply(400, json!({"detail":"There was an error parsing the body"})),
    };
    let value = if json_body || bytes.is_empty() {
        match crate::orchestration::body::parse(&bytes) {
            Ok(v) => v,
            Err((status, body)) => return json_response(status, body),
        }
    } else {
        Value::String(String::from_utf8_lossy(&bytes).into_owned())
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

fn boolean(
    query: &HashMap<String, String>,
    name: &str,
    default: bool,
    errors: &mut Vec<Value>,
) -> bool {
    match query.get(name) {
        None=>default,
        Some(value)=>crate::query::fastapi_bool(value).unwrap_or_else(||{
            errors.push(json!({"type":"bool_parsing","loc":["query",name],"msg":"Input should be a valid boolean, unable to interpret input","input":value}));default
        }),
    }
}

fn declared_too_large(req: &Request) -> bool {
    req.headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            if v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()) {
                return false;
            }
            let digits = v.trim_start_matches('0');
            digits.len() > 9 || digits.parse::<u64>().is_ok_and(|n| n > AUDIO_LIMIT as u64)
        })
}

// Preserva a resposta pública do middleware original até a mudança de contrato ser aprovada.
fn audio_body_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
}

/// O prazo público inclui configuração e todas as tentativas; cancelar solta o future e sua vaga.
async fn within_public_deadline<F>(
    operation: F,
) -> Result<crate::transcription::model::Transcription, TranscriptionError>
where
    F: std::future::Future<
            Output = Result<crate::transcription::model::Transcription, TranscriptionError>,
        >,
{
    tokio::time::timeout(std::time::Duration::from_secs(180),operation).await.unwrap_or_else(|_|Err(TranscriptionError{
        status:503,code:"transcription_rust_unavailable".into(),detail:"Não foi possível confirmar a transcrição no Rust. O áudio foi preservado para tentar novamente.".into(),
    }))
}
