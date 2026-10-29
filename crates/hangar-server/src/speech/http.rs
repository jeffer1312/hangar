//! A borda de leitura e texto é atendida no Rust, sem repasse ao backend Python.
use std::{collections::HashSet, net::SocketAddr, path::Path, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use hangar_api::chat::{ChatEvent, ChatKind};
use md5::{Digest, Md5};
use serde_json::{Value, json};

use super::{SpeechError, body::Schema};
use crate::{
    routes::{AppState, cors, who},
    session_write::{detail, json_response},
    share::caller::Caller,
};

const BODY_LIMIT: usize = 100 * 1024 * 1024;

pub fn matches(_method: &Method, path: &str) -> bool {
    matches!(
        path,
        "/api/tts"
            | "/api/tts/narrar"
            | "/api/tts/voices"
            | "/api/tts/saldo"
            | "/api/pensamento/pt"
            | "/api/config-sync/translate"
            | "/api/pair/task-suggestion"
    ) || path
        .strip_prefix("/api/tts/audio/")
        .is_some_and(|hash| !hash.is_empty() && !hash.contains('/'))
}

fn failure(error: SpeechError) -> Response {
    json_response(
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_GATEWAY),
        json!({"detail":error.detail}),
    )
}

pub async fn public(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    if let Some(response) = crate::share::gate::preflight(&req) {
        return response;
    }
    let headers = req.headers().clone();
    let mut response = public_inner(st, peer, req).await;
    cors(&headers, response.headers_mut());
    response
}

async fn public_inner(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let path = req.uri().path().to_owned();
    let read = path.starts_with("/api/tts/audio/")
        || matches!(path.as_str(), "/api/tts/voices" | "/api/tts/saldo");
    let method = if read { Method::GET } else { Method::POST };
    if *req.method() != method {
        let mut response = json_response(
            StatusCode::METHOD_NOT_ALLOWED,
            json!({"detail":"Method Not Allowed"}),
        );
        response
            .headers_mut()
            .insert(header::ALLOW, method.as_str().parse().unwrap());
        if req.method() == Method::HEAD {
            response
                .headers_mut()
                .insert(header::CONTENT_LENGTH, "31".parse().unwrap());
            *response.body_mut() = Body::empty();
        }
        return response;
    }
    if req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| {
            s.trim_start_matches('0').len() > 9
                || s.parse::<u64>().is_ok_and(|n| n > BODY_LIMIT as u64)
        })
    {
        return crate::session_write::too_large(req.headers());
    }
    let (forward, caller) = who(&st, peer, &req);
    let Some(caller) = caller else {
        if !crate::auth::is_loopback(&forward.client_ip) && st.auth.blocked(&forward.client_ip) {
            let mut response = detail(
                StatusCode::TOO_MANY_REQUESTS,
                "erro_muitas_tentativas",
                "muitas tentativas — aguarde",
                json!({}),
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, "30".parse().unwrap());
            return response;
        }
        st.auth.record_fail(&forward.client_ip);
        return detail(
            StatusCode::UNAUTHORIZED,
            "erro_nao_autorizado",
            "unauthorized",
            json!({}),
        );
    };
    if let Caller::Guest(grant) = &caller {
        let chat = path == "/api/tts"
            || path == "/api/tts/narrar"
            || path == "/api/pensamento/pt"
            || path.starts_with("/api/tts/audio/");
        if !grant.invite || !chat {
            return json_response(StatusCode::FORBIDDEN, json!({"detail":"Forbidden"}));
        }
    }
    if path.starts_with("/api/tts/audio/") {
        let hash = &path["/api/tts/audio/".len()..];
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return detail(
                StatusCode::BAD_REQUEST,
                "erro_tts_audio_invalido",
                "identificador de áudio inválido",
                json!({}),
            );
        }
        let config = match super::configuration(&st).await {
            Ok(c) => c,
            Err(e) => return failure(e),
        };
        return audio(&super::tts::cache_path(&config, hash), req.headers()).await;
    }
    if read {
        let config = match super::configuration(&st).await {
            Ok(c) => c,
            Err(e) => return failure(e),
        };
        let voices = path == "/api/tts/voices";
        let result = if voices {
            st.speech.tts.voices(&config).await
        } else {
            st.speech.tts.balance(&config).await
        };
        return match result {
            Ok(value) => json_response(
                StatusCode::OK,
                if voices {
                    json!({"voices":value})
                } else {
                    value
                },
            ),
            Err(error) => failure(error),
        };
    }
    let schema = match path.as_str() {
        "/api/tts" => Schema::Synthesis,
        "/api/tts/narrar" => Schema::Narration,
        "/api/pensamento/pt" => Schema::Thought,
        "/api/config-sync/translate" => Schema::Translation,
        _ => Schema::Suggestion,
    };
    let value = match body(req, schema).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if path == "/api/pair/task-suggestion" {
        let names = strings(&value["sessions"]);
        let conversations = match conversations(&st, names).await {
            Ok(c) => c,
            Err(e) => return failure(e),
        };
        if conversations.is_empty() {
            return detail(
                StatusCode::UNPROCESSABLE_ENTITY,
                "erro_grupo_sem_conversa",
                "nenhuma das sessões tem conversa para resumir",
                json!({}),
            );
        }
        let config = match super::configuration(&st).await {
            Ok(c) => c,
            Err(e) => return failure(e),
        };
        return match super::suggestion::suggest(&config, &conversations).await {
            Ok(task) => json_response(StatusCode::OK, json!({"task":task})),
            Err(error) => failure(error),
        };
    }
    // Validação e cortes antecedem a configuração, como nos handlers originais.
    if path == "/api/tts" {
        return synthesize(&st, &value).await;
    }
    let config = match super::configuration(&st).await {
        Ok(c) => c,
        Err(e) => return failure(e),
    };
    match path.as_str() {
        "/api/tts/narrar" => match super::narration::narrate(
            &config,
            value["text"].as_str().unwrap(),
            &strings(&value["code_blocks"]),
            value["instruction"].as_str().unwrap(),
        )
        .await
        {
            Ok(value) => json_response(StatusCode::OK, value),
            Err(error) => failure(error),
        },
        "/api/pensamento/pt" => {
            let texts: Vec<_> = strings(&value["textos"])
                .into_iter()
                .map(|text| text.chars().take(4000).collect())
                .collect();
            let translated = if config.boolean("traduzir_pensamento", true) {
                st.speech.thought.translate(&config, &texts).await
            } else {
                texts
            };
            json_response(StatusCode::OK, json!({"textos":translated}))
        }
        _ => {
            let (texts, error) = st
                .speech
                .translations
                .translate(
                    &config,
                    &strings(&value["texts"]),
                    value["lang"].as_str().unwrap(),
                )
                .await;
            json_response(StatusCode::OK, json!({"texts":texts,"error":error}))
        }
    }
}

async fn body(req: Request, schema: Schema) -> Result<Value, Response> {
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let headers = req.headers().clone();
    let bytes = to_bytes(req.into_body(), BODY_LIMIT)
        .await
        .map_err(|_| crate::session_write::too_large(&headers))?;
    let json = content_type.is_empty()
        || content_type.split(';').next().is_some_and(|v| {
            v == "application/json" || v.starts_with("application/") && v.ends_with("+json")
        });
    let value = if json || bytes.is_empty() {
        crate::orchestration::body::parse(&bytes)
            .map_err(|(status, value)| json_response(status, value))?
    } else {
        Value::String(String::from_utf8_lossy(&bytes).into_owned())
    };
    super::body::validate(schema, value)
        .map_err(|errors| json_response(StatusCode::UNPROCESSABLE_ENTITY, errors))
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

async fn synthesize(st: &AppState, value: &Value) -> Response {
    let text = super::tts::prepare(value["text"].as_str().unwrap());
    let size = text.chars().count();
    if size == 0 {
        return detail(
            StatusCode::BAD_REQUEST,
            "erro_tts_sem_texto",
            "não sobrou nada pra falar depois de limpar o texto",
            json!({}),
        );
    }
    if size > super::tts::CHARACTER_LIMIT {
        return detail(
            StatusCode::PAYLOAD_TOO_LARGE,
            "erro_tts_teto",
            &format!(
                "texto com {size} caracteres passa do teto de {} — selecione um trecho menor",
                super::tts::CHARACTER_LIMIT
            ),
            json!({"n":size,"teto":super::tts::CHARACTER_LIMIT}),
        );
    }
    let config = match super::configuration(st).await {
        Ok(c) => c,
        Err(e) => return failure(e),
    };
    let limit = config.integer("tts_max_chars", 5000);
    let limit = if limit > 0 { limit as u64 } else { 5000 };
    if size as u64 > limit && !value["confirm"].as_bool().unwrap_or(false) {
        return detail(
            StatusCode::CONFLICT,
            "erro_tts_limite",
            &format!("são {size} caracteres, acima do limite de {limit} — confirme para gerar"),
            json!({"n":size,"limite":limit}),
        );
    }
    match st
        .speech
        .tts
        .synthesize(
            &config,
            &text,
            value["voice"].as_str().unwrap_or(""),
            value["provider"].as_str().unwrap_or("elevenlabs"),
            value["instruction"].as_str().unwrap_or(""),
        )
        .await
    {
        Ok(result) => json_response(
            StatusCode::OK,
            json!({"url":format!("/api/tts/audio/{}",result.hash),"chars":size,"cached":result.cached,"provider":result.provider}),
        ),
        Err(error) => failure(error),
    }
}

/// Conserva o streaming e os ranges compartilhados, sem mudar o validador do áudio já publicado.
async fn audio(path: &Path, headers: &HeaderMap) -> Response {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(_) => {
            return detail(
                StatusCode::NOT_FOUND,
                "erro_tts_sem_cache",
                "áudio não está mais em cache",
                json!({}),
            );
        }
    };
    let metadata = match file.metadata().await {
        Ok(meta) => meta,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .unwrap_or_default();
    let mtime = crate::transcript::pyjson::float_repr(modified.as_secs_f64());
    let etag = format!(
        "\"{:x}\"",
        Md5::digest(format!("{mtime}-{}", metadata.len()).as_bytes())
    );
    let last = chrono::DateTime::<chrono::Utc>::from_timestamp(modified.as_secs() as i64, 0)
        .map(|t| t.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default();
    let mut request_headers = headers.clone();
    // FileResponse não implementa o 304 da StaticFiles.
    request_headers.remove(header::IF_NONE_MATCH);
    if let Some(value) = request_headers.remove(header::IF_RANGE) {
        if value != etag.as_str() && value != last.as_str() {
            request_headers.remove(header::RANGE);
        }
    }
    if let Some(value) = request_headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
    {
        let value = bounded_range(value);
        if let Ok(value) = value.parse() {
            request_headers.insert(header::RANGE, value);
        }
    }
    let media = if path.extension().is_some_and(|v| v == "wav") {
        "audio/wav"
    } else {
        "audio/mpeg"
    };
    let mut response =
        crate::workspace_routes::serve_open_file(file, path, &request_headers, false, Some(media))
            .await;
    if let Some(boundary) = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|value| value.strip_prefix("multipart/byteranges; boundary="))
        .map(str::to_owned)
        && let Some(ranges) = request_headers
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|value| crate::workspace_routes::ranges(value, metadata.len()).ok())
    {
        let length = ranges.iter().map(|(start,end)| format!("--{boundary}\r\nContent-Type: {media}\r\nContent-Range: bytes {start}-{}/{}\r\n\r\n",i128::from(*end)-1,metadata.len()).len() as u64 + end - start + 2).sum::<u64>()
            + format!("--{boundary}--").len() as u64;
        let mut remaining = length;
        let stream = std::mem::replace(response.body_mut(), Body::empty())
            .into_data_stream()
            .map(move |chunk| {
                chunk.map(|mut chunk| {
                    let size = remaining.min(chunk.len() as u64) as usize;
                    chunk.truncate(size);
                    remaining -= size as u64;
                    chunk
                })
            });
        *response.body_mut() = Body::from_stream(stream);
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, length.to_string().parse().unwrap());
    }
    if response.headers().contains_key(header::ETAG) {
        response
            .headers_mut()
            .insert(header::ETAG, etag.parse().unwrap());
    }
    for name in [header::CACHE_CONTROL, header::REFERRER_POLICY] {
        response.headers_mut().remove(name);
    }
    response.headers_mut().remove("x-content-type-options");
    response
}

/// O Python lê inteiros arbitrários no Range; para um arquivo, saturar conserva as comparações.
fn bounded_range(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character.is_ascii_digit() {
            let mut number = u64::from(character as u8 - b'0');
            while chars.peek().is_some_and(char::is_ascii_digit) {
                number = number
                    .saturating_mul(10)
                    .saturating_add(u64::from(chars.next().unwrap() as u8 - b'0'));
            }
            output.push_str(&number.to_string());
        } else {
            output.push(character);
        }
    }
    output
}

async fn conversations(
    st: &AppState,
    names: Vec<String>,
) -> Result<Vec<(String, String)>, SpeechError> {
    let mut seen = HashSet::new();
    let names: Vec<_> = names
        .into_iter()
        .filter(|n| seen.insert(n.clone()))
        .collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let rows = st
        .list
        .discover(None)
        .await
        .map_err(|_| SpeechError::new(503, "Não foi possível ler as conversas das sessões."))?;
    let claude = st
        .list
        .env()
        .dirs
        .as_ref()
        .map(|dirs| dirs.claude.clone())
        .unwrap_or_else(|| st.accounts.env.claude_base.clone());
    let home = st.accounts.env.home.clone();
    let dirs = st.list.env().dirs.clone();
    tokio::task::spawn_blocking(move || {
        let mut rows = rows.as_ref().clone();
        if let Some(dirs) = dirs {
            let (overrides, _) = crate::list::local_transfers::transfers(&rows, &dirs)
                .map_err(std::io::Error::other)?;
            for row in overrides {
                if let Some(current) = rows.iter_mut().find(|current| current.name == row.name) {
                    *current = row;
                } else {
                    rows.push(row);
                }
            }
        }
        let requests: Vec<_> = names
            .into_iter()
            .filter_map(|name| {
                let row = rows.iter().find(|row| row.name == name)?.clone();
                row.jsonl.as_ref()?;
                Some((name, row))
            })
            .collect();
        requests
            .into_iter()
            .map(|(name, row)| {
                super::history::read_row(&row, &home, &claude, 30)
                    .map(|events| (name, summarize(&events)))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|conversations| {
                conversations
                    .into_iter()
                    .filter(|(_, text)| !text.is_empty())
                    .collect()
            })
    })
    .await
    .map_err(|_| SpeechError::new(503, "A leitura das conversas foi interrompida."))?
    .map_err(|_| {
        SpeechError::new(
            503,
            "Não foi possível ler a conversa para sugerir a tarefa.",
        )
    })
}

fn summarize(events: &[ChatEvent]) -> String {
    let messages: Vec<_> = events
        .iter()
        .filter_map(|event| {
            let speaker = match event.kind {
                ChatKind::UserMsg => "usuário",
                ChatKind::AssistantMsg => "assistente",
                _ => return None,
            };
            let text = crate::transcript::py::strip(event.text.as_deref()?);
            (!text.is_empty()).then_some((speaker, text))
        })
        .collect();
    let Some(((speaker, last), previous)) = messages.split_last() else {
        return String::new();
    };
    let earlier = previous
        .iter()
        .map(|(speaker, text)| format!("{speaker}: {}", text.chars().take(500).collect::<String>()))
        .collect::<Vec<_>>()
        .join("\n");
    let earlier: String = earlier
        .chars()
        .skip(earlier.chars().count().saturating_sub(3000))
        .collect();
    let last = format!(
        "ÚLTIMA MENSAGEM ({speaker}): {}",
        last.chars().take(2000).collect::<String>()
    );
    if earlier.is_empty() {
        last
    } else {
        format!("{earlier}\n{last}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_paths(value: &Value, home: &Path) -> Value {
        match value {
            Value::String(value) => {
                let path = match value.as_str() {
                    "@SOURCE@" => Some(home.join("snapshot.jsonl")),
                    "@ROLLOUT@" => Some(home.join("rollout.jsonl")),
                    "@CODEX@" => Some(home.join("codex")),
                    "@HOME@" => Some(home.to_owned()),
                    "@ORIGIN@" => Some(home.join("original.jsonl")),
                    _ => None,
                };
                path.map(|path| json!(path)).unwrap_or_else(|| json!(value))
            }
            Value::Object(values) => Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), expand_paths(value, home)))
                    .collect(),
            ),
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|value| expand_paths(value, home))
                    .collect(),
            ),
            value => value.clone(),
        }
    }

    #[test]
    fn transferred_group_context_matches_python_sources_boundary_queue_and_failures() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/speech/http_transfer_history.json"
        ))
        .unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let directory = tempfile::tempdir().unwrap();
            let home = directory.path();
            std::fs::create_dir_all(home.join("codex")).unwrap();
            std::fs::create_dir_all(home.join(".hangar/conversation-transfers")).unwrap();
            std::fs::create_dir_all(home.join(".claude/.hangar-queue")).unwrap();
            std::fs::write(
                home.join("snapshot.jsonl"),
                case["source_bytes"].as_str().unwrap(),
            )
            .unwrap();
            std::fs::write(
                home.join("rollout.jsonl"),
                case["rollout_bytes"].as_str().unwrap(),
            )
            .unwrap();
            let record = expand_paths(&case["record"], home);
            let id = record["id"].as_str().unwrap();
            std::fs::write(
                home.join(".hangar/conversation-transfers")
                    .join(format!("{id}.json")),
                record.to_string(),
            )
            .unwrap();
            let queue = case["queue"]
                .as_array()
                .unwrap()
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            std::fs::write(home.join(".claude/.hangar-queue/s.jsonl"), queue).unwrap();
            let row = serde_json::from_value(
                json!({"name":"s","provider":"codex","jsonl":home.join("rollout.jsonl"),
                "codex_home":home.join("codex"),"transfer_id":id,"lifecycle_id":"k:original"}),
            )
            .unwrap();
            let result = super::super::history::read_row(&row, home, &home.join(".claude"), 30);
            if case.get("error").is_some() {
                assert!(
                    result.is_err(),
                    "a fonte inválida virou contexto: {}",
                    case["name"]
                );
                continue;
            }
            let events = result.unwrap_or_else(|error| panic!("{}: {error}", case["name"]));
            let projection: Vec<_> = events
                .iter()
                .map(|event| json!({"kind":event.kind,"text":event.text,"ts":event.ts}))
                .collect();
            assert_eq!(
                json!(projection),
                case["events"],
                "eventos transferidos: {}",
                case["name"]
            );
            assert_eq!(
                summarize(&events),
                case["summary"],
                "contexto transferido: {}",
                case["name"]
            );
        }
    }

    #[test]
    fn group_context_matches_the_python_window_hook_filter_and_queue() {
        for source in [
            include_str!("../../tests/fixtures/speech/http_history.json"),
            include_str!("../../tests/fixtures/speech/http_history_windows.json"),
        ] {
            let fixtures: Value = serde_json::from_str(source).unwrap();
            for case in fixtures["cases"].as_array().unwrap() {
                let directory = tempfile::tempdir().unwrap();
                let transcript = directory.path().join("conversation.jsonl");
                let queue = directory.path().join("queue.jsonl");
                for (path, rows) in [(&transcript, &case["records"]), (&queue, &case["queue"])] {
                    let lines = rows
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(Value::to_string)
                        .collect::<Vec<_>>()
                        .join("\n")
                        + "\n";
                    std::fs::write(path, lines).unwrap();
                }
                let info = crate::transcript::InternalInfo {
                    provider: case["provider"].as_str().unwrap().into(),
                    jsonl: Some(transcript),
                    session_key: String::new(),
                    history: json!({"queue":queue}),
                    headless: false,
                    lifecycle_id: None,
                    cwd: None,
                };
                let limit = case["limit"].as_u64().unwrap() as usize;
                let events = super::super::history::read(&info, limit).unwrap();
                let projection: Vec<_> = events
                    .iter()
                    .skip(
                        events
                            .len()
                            .saturating_sub(case["events"].as_array().unwrap().len()),
                    )
                    .map(|event| json!({"kind":event.kind,"text":event.text,"ts":event.ts}))
                    .collect();
                assert_eq!(
                    json!(projection),
                    case["events"],
                    "eventos: {}",
                    case["name"]
                );
                assert_eq!(
                    summarize(&events),
                    case["summary"],
                    "contexto: {}",
                    case["name"]
                );
            }
        }
    }
}
