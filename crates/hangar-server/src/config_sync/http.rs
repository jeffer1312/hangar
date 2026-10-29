//! Borda HTTP; a leitura acontece no Rust mesmo quando o Python está indisponível.
use super::{
    bundle::{Error, ITEMS},
    export::{self, Context},
    paths::Roots,
};
use crate::routes::{AppState, cors, owner_gate};
use crate::session_write::{detail_body, json_response};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

pub fn context(st: &AppState) -> Context {
    if let Some(ctx) = &st.config_sync_context {
        return ctx.clone();
    }
    let env = &st.accounts.env;
    let root =
        crate::install_root::root(&env.home).unwrap_or_else(|| env.home.join(".hangar/current"));
    let roots = Roots {
        home: env.home.to_string_lossy().into_owned(),
        claude: env.home.join(".claude").to_string_lossy().into_owned(),
        codex: env.codex_default.to_string_lossy().into_owned(),
        hangar: root.to_string_lossy().into_owned(),
    };
    let mut context = Context::new(roots);
    context.preferences = env.claude_base.join("runtime-config.json");
    if let Some(path) = env.base.get("CP_ENGINES_FILE").filter(|s| !s.is_empty()) {
        context.engines = PathBuf::from(path);
    }
    if cfg!(windows) {
        if let Some(appdata) = env.base.get("APPDATA").filter(|s| !s.is_empty()) {
            context.native = Path::new(appdata).join("hangar-native");
        }
    } else if let Some(xdg) = env
        .base
        .get("XDG_CONFIG_HOME")
        .filter(|s| Path::new(s).is_absolute())
    {
        context.native = Path::new(xdg).join("hangar-native");
    }
    context
}
pub fn failure(status: StatusCode, error: Error) -> Response {
    json_response(
        status,
        detail_body(error.code, &error.message, error.params),
    )
}
pub fn items(raw: &str) -> Result<Vec<String>, Error> {
    let items: Vec<_> = raw
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let unknown: Vec<_> = items
        .iter()
        .filter(|i| !ITEMS.contains(&i.as_str()))
        .cloned()
        .collect();
    if items.is_empty() || !unknown.is_empty() {
        let listed = if unknown.is_empty() {
            "(nenhum)".to_owned()
        } else {
            unknown.join(", ")
        };
        Err(Error::new(
            "config_sync_unknown_item",
            format!("item desconhecido: {listed}"),
            json!({"items":listed}),
        ))
    } else {
        Ok(items)
    }
}

pub async fn handle(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return response;
    }
    let headers = req.headers().clone();
    let path = req.uri().path().to_owned();
    let query = req.uri().query().map(str::to_owned);
    let mut response = if path.ends_with("/translate") && req.method() == Method::POST {
        translate_request(&st, req).await
    } else if req.method() != Method::GET && req.method() != Method::HEAD {
        StatusCode::METHOD_NOT_ALLOWED.into_response()
    } else if path.ends_with("/manifest")
        && crate::auth::query_param(query.as_deref(), "stream")
            .is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
    {
        manifest_stream(st.clone())
    } else if path.ends_with("/manifest") {
        let ctx = context(&st);
        let machine = st.current_server_id();
        match tokio::task::spawn_blocking(move || {
            export::export(
                &ctx,
                &ITEMS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                None,
                false,
            )
        })
        .await
        {
            Ok(Ok(bundle)) => {
                let mut manifest = bundle.manifest();
                manifest["machine"] = json!(machine);
                json_response(StatusCode::OK, manifest)
            }
            Ok(Err(error)) => failure(StatusCode::INTERNAL_SERVER_ERROR, error),
            Err(_) => failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                Error::new(
                    "config_sync_failed",
                    "falha ao ler a configuração",
                    json!({}),
                ),
            ),
        }
    } else {
        let selected =
            items(&crate::auth::query_param(query.as_deref(), "items").unwrap_or_default());
        let keep = crate::auth::query_param(query.as_deref(), "keys").unwrap_or_default();
        let keep = if keep.is_empty() {
            Ok(None)
        } else {
            serde_json::from_str::<Value>(&keep)
                .ok()
                .and_then(|v| v.as_object().cloned())
                .filter(|m| {
                    m.iter().all(|(k, v)| {
                        ITEMS.contains(&k.as_str())
                            && v.as_array().is_some_and(|a| a.iter().all(Value::is_string))
                    })
                })
                .map(Some)
                .ok_or_else(|| {
                    Error::new(
                        "config_sync_invalid_keys",
                        "escolha de entradas inválida",
                        json!({}),
                    )
                })
        };
        match (selected, keep) {
            (Err(error), _) | (_, Err(error)) => failure(StatusCode::BAD_REQUEST, error),
            (Ok(selected), Ok(keep)) => {
                let ctx = context(&st);
                match tokio::task::spawn_blocking(move || {
                    export::export(&ctx, &selected, keep.as_ref(), true)?.pack()
                })
                .await
                {
                    Ok(Ok(bytes)) => (
                        [
                            ("content-type", "application/gzip"),
                            ("content-encoding", "identity"),
                            ("cache-control", "no-store"),
                        ],
                        bytes,
                    )
                        .into_response(),
                    Ok(Err(error)) => failure(
                        if error.code == "config_sync_bundle_too_big" {
                            StatusCode::PAYLOAD_TOO_LARGE
                        } else {
                            StatusCode::INTERNAL_SERVER_ERROR
                        },
                        error,
                    ),
                    Err(_) => failure(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Error::new(
                            "config_sync_failed",
                            "falha ao ler a configuração",
                            json!({}),
                        ),
                    ),
                }
            }
        }
    };
    cors(&headers, response.headers_mut());
    response
}

fn manifest_stream(st: Arc<AppState>) -> Response {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let mut context = context(&st);
    context.progress = Some(sender.clone());
    let machine = st.current_server_id();
    // A tarefa possui o trabalho; fechar a conexão só deixa de receber o progresso.
    tokio::spawn(async move {
        let work = tokio::task::spawn_blocking(move || {
            export::export(
                &context,
                &ITEMS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                None,
                false,
            )
        })
        .await;
        let final_event = match work {
            Ok(Ok(bundle)) => {
                let mut manifest = bundle.manifest();
                manifest["machine"] = json!(machine);
                json!({"type":"done","result":manifest})
            }
            Ok(Err(error)) => {
                json!({"type":"error","status":500,"detail":error.message.chars().take(300).collect::<String>()})
            }
            Err(_) => json!({"type":"error","status":500,"detail":"falha ao ler a configuração"}),
        };
        let _ = sender.send(final_event);
    });
    let stream =
        futures_util::stream::unfold((receiver, false), |(mut receiver, done)| async move {
            if done {
                return None;
            }
            let mut event = receiver.recv().await?;
            let final_event = event.get("type").is_some();
            if !final_event {
                event["type"] = json!("progress");
            }
            let line = format!("{}\n", serde_json::to_string(&event).unwrap());
            Some((
                Ok::<_, std::convert::Infallible>(line),
                (receiver, final_event),
            ))
        });
    (
        [
            ("content-type", "application/x-ndjson"),
            ("content-encoding", "identity"),
            ("cache-control", "no-store"),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

async fn translate_request(st: &AppState, req: Request) -> Response {
    let bytes = match axum::body::to_bytes(req.into_body(), 100 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let data: Value = match crate::orchestration::body::parse(&bytes) {
        Ok(value) => value,
        Err((status, detail)) => return json_response(status, detail),
    };
    let mut errors = Vec::new();
    if data.is_null() {
        errors.push(json!({"type":"missing","loc":["body"],"msg":"Field required","input":null}));
    } else if !data.is_object() {
        errors.push(json!({"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":data}));
    } else {
        match data.get("lang") {
            None => errors.push(json!({"type":"missing","loc":["body","lang"],"msg":"Field required","input":data})),
            Some(lang) if lang != "pt" && lang != "en" => errors.push(json!({"type":"literal_error","loc":["body","lang"],"msg":"Input should be 'pt' or 'en'","input":lang,"ctx":{"expected":"'pt' or 'en'"}})),
            _ => {}
        }
        match data.get("texts") {
            None => errors.push(json!({"type":"missing","loc":["body","texts"],"msg":"Field required","input":data})),
            Some(Value::Array(texts)) => {
                for (i,text) in texts.iter().enumerate() {
                    if let Some(text) = text.as_str() {
                        if text.chars().count() > 2000 { errors.push(json!({"type":"string_too_long","loc":["body","texts",i],"msg":"String should have at most 2000 characters","input":text,"ctx":{"max_length":2000}})); }
                    } else { errors.push(json!({"type":"string_type","loc":["body","texts",i],"msg":"Input should be a valid string","input":text})); }
                }
                if texts.len() > 2000 { errors.push(json!({"type":"too_long","loc":["body","texts"],"msg":format!("List should have at most 2000 items after validation, not {}", texts.len()),"input":texts,"ctx":{"field_type":"List","max_length":2000,"actual_length":texts.len()}})); }
            }
            Some(value) => errors.push(json!({"type":"list_type","loc":["body","texts"],"msg":"Input should be a valid list","input":value})),
        }
        for (key, value) in data.as_object().unwrap() {
            if key != "lang" && key != "texts" {
                errors.push(json!({"type":"extra_forbidden","loc":["body",key],"msg":"Extra inputs are not permitted","input":value}));
            }
        }
    }
    if !errors.is_empty() {
        return json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail":errors}));
    }
    let texts: Vec<_> = data["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let cache = st
        .accounts
        .env
        .home
        .join(".hangar/config-sync/traducoes.json");
    let result = st
        .config_sync_translation
        .translate(
            &cache,
            &texts,
            data["lang"].as_str().unwrap(),
            |system, prompt| super::translate::chat(st, system, prompt),
        )
        .await;
    json_response(StatusCode::OK, result)
}
