use super::{
    body::{self, Schema},
    mirrors, probe, proxy, store,
};
use crate::{routes::AppState, session_write::json_response};
use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Value, json};

fn visible(st: &crate::accounts::AccountService) -> Value {
    let mut entries = store::visible(&st.env);
    for (_, item) in &mut entries {
        let result = proxy::is_local(&st.env, item["base_url"].as_str().unwrap_or(""));
        match result {
            Ok(true) => {
                item["cliproxy_accounts"] = json!([]);
                match proxy::list_accounts(st) {
                    Ok(accounts) if accounts.is_empty() => {
                        item["cliproxy_error"] = json!(
                            "nenhuma conta ChatGPT do proxy corresponde às contas cadastradas no Hangar"
                        )
                    }
                    Ok(accounts) => item["cliproxy_accounts"] = json!(accounts),
                    Err(e) => item["cliproxy_error"] = json!(e),
                }
            }
            Err(e) => item["cliproxy_error"] = json!(e),
            _ => {}
        }
    }
    json!(entries)
}

fn coded(status: u16, code: &str, msg: &str) -> Response {
    crate::provider_models::ModelError::coded(status, code, msg, json!({})).response()
}

pub fn matches(_method: &Method, path: &str) -> bool {
    path == "/api/providers"
        || path == "/api/engines"
        || path.starts_with("/api/engines/")
        || matches!(
            path,
            "/api/credenciais/sincronizar" | "/api/credenciais/apelido"
        )
        || path.starts_with("/api/credenciais/kimi/")
}

pub async fn serve(st: &AppState, req: Request) -> Response {
    let env = st.accounts.env.clone();
    let method = req.method().clone();
    let path = percent_encoding::percent_decode_str(req.uri().path())
        .decode_utf8_lossy()
        .into_owned();
    let headers = req.headers().clone();
    let bytes = if matches!(method, Method::POST | Method::PUT) {
        match crate::http_body_limit::read(req.into_body()).await {
            Ok(bytes) => bytes,
            Err(error) if crate::http_body_limit::is_limit(&error) => {
                return crate::http_body_limit::rejected();
            }
            Err(_) => return body::internal_error(),
        }
    } else {
        axum::body::Bytes::new()
    };
    if path == "/api/providers" && method == Method::GET {
        return match super::providers::read(&st.accounts).await {
            Ok(snapshot) => json_response(StatusCode::OK,snapshot.response()),
            Err(error) => body::json_error(StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),error.body()),
        };
    }
    if path == "/api/credenciais/apelido" && method == Method::PUT {
        let value = match body::typed(Schema::Alias, &bytes, &headers) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let (id, alias) = (
            value["id"].as_str().unwrap().to_owned(),
            value["apelido"].as_str().unwrap().to_owned(),
        );
        return match tokio::task::spawn_blocking(move || {
            crate::accounts::aliases::set(&env, &id, Some(&alias))
                .map(|aliases| json!({"id":id,"apelido":aliases.get(&id)}))
        })
        .await
        {
            Ok(Ok(value)) => json_response(StatusCode::OK, value),
            _ => body::internal_error(),
        };
    }
    if path == "/api/credenciais/sincronizar" && method == Method::POST {
        let body = match body::typed(Schema::Sync, &bytes, &headers) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let id = body["id"].as_str().unwrap();
        let Some(name) = id.strip_prefix("chave:") else {
            return coded(
                400,
                "erro_credencial_sem_chave",
                "só credencial de chave de API pode ser sincronizada",
            );
        };
        let catalog = store::state(&env).0;
        let Some(saved) = catalog.get(name).filter(|v| probe::truthy(v)) else {
            return crate::provider_models::ModelError::coded(
                404,
                "erro_credencial_inexistente",
                &format!("credencial {name} não existe"),
                json!({"nome":name}),
            )
            .response();
        };
        let (base, key) = (
            saved["base_url"].as_str().unwrap_or(""),
            saved["api_key"].as_str().unwrap_or(""),
        );
        if base.is_empty() || key.is_empty() {
            return coded(
                409,
                "erro_credencial_incompleta",
                "credencial sem endereço ou sem chave",
            );
        }
        let models = probe::fetch(base, key).await.unwrap_or_default();
        let mut targets: Vec<String> = body["alvos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|v| ["pi", "kimi", "codex"].contains(v))
            .map(str::to_owned)
            .collect();
        if targets.is_empty() {
            targets = ["pi", "kimi", "codex"].map(str::to_owned).to_vec();
        }
        let (id, name, base, key) = (
            id.to_owned(),
            name.to_owned(),
            base.to_owned(),
            key.to_owned(),
        );
        let count = models.len();
        return match tokio::task::spawn_blocking(move || {
            mirrors::sync(&env, &name, &base, &key, &models, &targets)
        })
        .await
        {
            Ok(result) => json_response(
                StatusCode::OK,
                json!({"id":id,"modelos":count,"resultado":result}),
            ),
            Err(_) => coded(
                500,
                "erro_credencial_sincronizacao",
                "falha ao sincronizar credencial",
            ),
        };
    }
    if let Some(name) = path.strip_prefix("/api/credenciais/kimi/") {
        if method != Method::DELETE {
            return json_response(
                StatusCode::METHOD_NOT_ALLOWED,
                json!({"detail":"Method Not Allowed"}),
            );
        }
        let name = name.to_owned();
        return match tokio::task::spawn_blocking(move || {
            mirrors::remove(&env, &name, &["kimi".into()])
        })
        .await
        {
            Ok(result) if result["kimi"]["ok"] == true => {
                json_response(StatusCode::OK, json!({"ok":true}))
            }
            Ok(result) => coded(
                if ["nao-gerenciado", "nao-instalado"]
                    .contains(&result["kimi"]["motivo"].as_str().unwrap_or(""))
                {
                    404
                } else {
                    400
                },
                "erro_credencial_nao_gerenciada",
                "credencial nao gerenciada pelo app",
            ),
            Err(_) => coded(
                500,
                "erro_credencial_sincronizacao",
                "falha ao remover credencial",
            ),
        };
    }
    if path == "/api/engines/modelos" && method == Method::POST {
        let body = match body::typed(Schema::Probe, &bytes, &headers) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let (base, key) = if let Some(name) = body["nome"].as_str().filter(|s| !s.is_empty()) {
            if probe::truthy(&body["base_url"]) || probe::truthy(&body["api_key"]) {
                return coded(
                    400,
                    "erro_motor_nome_com_dados",
                    "nome já usa o motor salvo; não envie base_url/api_key junto",
                );
            }
            let catalog = store::state(&env).0;
            let Some(saved) = catalog.get(name).filter(|v| probe::truthy(v)) else {
                return coded(404, "erro_motor_nao_encontrado", "motor nao encontrado");
            };
            (
                saved["base_url"].as_str().unwrap_or("").to_owned(),
                saved["api_key"].as_str().unwrap_or("").to_owned(),
            )
        } else {
            let (base, key) = (
                body["base_url"].as_str().unwrap_or(""),
                body["api_key"].as_str().unwrap_or(""),
            );
            if base.is_empty() || key.is_empty() {
                return coded(
                    400,
                    "erro_motor_nome_ou_dados",
                    "informe nome de um motor salvo, ou base_url + api_key",
                );
            }
            (base.into(), key.into())
        };
        let base = match store::base_url(&base) {
            Ok(b) => b,
            Err(e) => return crate::provider_models::ModelError::text(400, e).response(),
        };
        return match probe::fetch(&base, &key).await {
            Ok(models) => json_response(
                StatusCode::OK,
                json!({"modelos":models,"gateway":proxy::is_local(&env,&base).unwrap_or(false).then_some("cliproxyapi")}),
            ),
            Err(e) => e.response(),
        };
    }
    if path == "/api/engines/cliproxy/management-key" && method == Method::PUT {
        let body = match body::typed(Schema::Management, &bytes, &headers) {
            Ok(value) => value,
            Err(response) => return response,
        };
        if let Err(e) =
            proxy::set_management_key(&env, body["management_key"].as_str().unwrap_or(""))
        {
            return crate::provider_models::ModelError::text(400, e).response();
        }
        return json_response(StatusCode::OK, proxy::naming(&st.accounts).await);
    }
    if path == "/api/engines/cliproxy" && method == Method::GET {
        let local = match proxy::local(&env) {
            Ok(Some(local)) => local,
            result => {
                return json_response(
                    StatusCode::OK,
                    json!({"found":false,"base_url":null,"models":[],"error":result.err()}),
                );
            }
        };
        let models = match probe::fetch(&local.base_url, &local.api_key).await {
            Ok(models) => models,
            Err(e) => {
                return json_response(
                    StatusCode::OK,
                    json!({"found":true,"base_url":local.base_url,"models":[],"error":e.detail.as_str().unwrap_or("").replace(&local.api_key,"***")}),
                );
            }
        };
        let mut result = proxy::naming(&st.accounts).await;
        result["found"] = json!(true);
        result["base_url"] = json!(local.base_url);
        result["error"] = Value::Null;
        result["models"] = json!(
            models
                .into_iter()
                .filter(|m| m["id"]
                    .as_str()
                    .is_some_and(|s| !s.starts_with("gpt-image-")))
                .collect::<Vec<_>>()
        );
        return json_response(StatusCode::OK, result);
    }
    let raw_body = if method == Method::PUT {
        match body::raw(&bytes) {
            Ok(value) => value,
            Err(response) => return response,
        }
    } else {
        Value::Null
    };
    let accounts = st.accounts.clone();
    let result = tokio::task::spawn_blocking(move || {
        if path == "/api/engines" && method == Method::GET {
            return (200, json!({"motores":visible(&accounts),"arquivo_corrompido":store::state(&env).1,"arquivo_caminho":store::path(&env)}));
        }
        let name = path.strip_prefix("/api/engines/").unwrap_or("");
        if method == Method::PUT && !name.is_empty() {
            let body = raw_body.as_object().cloned();
            let Some(mut body) = body else { return (400, json!({"detail":{"code":"erro_corpo_deve_ser_objeto","msg":"corpo deve ser um objeto","params":{}}})); };
            if body.remove("use_cliproxy_key") == Some(Value::Bool(true)) {
                let local = match proxy::local(&env) {
                    Ok(Some(local)) => local,
                    Ok(None) => return (400,json!({"detail":{"code":"erro_cliproxy_ausente","msg":"CLIProxyAPI local sem config ou sem api-keys","params":{}}})),
                    Err(e) => return (400,json!({"detail":e})),
                };
                if store::normalize_base(body.get("base_url").and_then(Value::as_str).unwrap_or("").trim()) != local.base_url {
                    return (400,json!({"detail":{"code":"erro_cliproxy_endereco","msg":"endereço diferente do CLIProxyAPI local","params":{}}}));
                }
                body.insert("api_key".into(),json!(local.api_key));
            }
            return match store::put(&env, name, body) {
                Ok(()) => (200, json!({"motores":visible(&accounts)})),
                Err(e) => (400, json!({"detail":e})),
            };
        }
        if method == Method::DELETE && !name.is_empty() {
            return match store::remove(&env, name) {
                Ok(true) => {
                    let result=mirrors::remove(&env,name,&["pi".into(),"kimi".into(),"codex".into()]);
                    tracing::debug!(%result,"Espelhos do motor removido conferidos");
                    (200,json!({"ok":true}))
                },
                Ok(false) => (404, json!({"detail":{"code":"erro_motor_nao_encontrado","msg":"motor nao encontrado","params":{}}})),
                Err(e) => (400, json!({"detail":e})),
            };
        }
        (405, json!({"detail":"Method Not Allowed"}))
    }).await;
    match result {
        Ok((status, body)) => json_response(StatusCode::from_u16(status).unwrap(), body),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"detail":"falha ao ler motores"}),
        ),
    }
}

const ROUTES: &[(&str, &str)] = &[
    ("/api/providers", "GET"),
    ("/api/engines", "GET"),
    ("/api/engines/{nome}", "PUT"),
    ("/api/engines/{nome}", "DELETE"),
    ("/api/engines/modelos", "POST"),
    ("/api/engines/cliproxy", "GET"),
    ("/api/engines/cliproxy/management-key", "PUT"),
    ("/api/credenciais/apelido", "PUT"),
    ("/api/credenciais/sincronizar", "POST"),
    ("/api/credenciais/kimi/{nome}", "DELETE"),
    ("/api/model-options", "GET"),
    ("/api/sessions/{name}/model/options", "GET"),
    ("/api/sessions/{name}/engine/model", "POST"),
];

/// CORS e tamanho antecedem autenticação; resolução de método não executa dependências do handler.
pub async fn cors_layer(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<AppState>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let path = percent_encoding::percent_decode_str(request.uri().path())
        .decode_utf8_lossy()
        .into_owned();
    let owned = matches(request.method(), path.trim_end_matches('/'))
        || path.trim_end_matches('/') == "/api/model-options"
        || (path.starts_with("/api/sessions/")
            && (path.trim_end_matches('/').ends_with("/model/options")
                || path.trim_end_matches('/').ends_with("/engine/model")));
    if !owned {
        return next.run(request).await;
    }
    let headers = request.headers().clone();
    let method = request.method().clone();
    if let Some(response) = crate::http_cors::preflight(&method, &headers) {
        return response;
    }
    let decision = crate::http_contract::resolve(ROUTES, method.as_str(), &path);
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|peer| peer.0.ip())
        .unwrap_or(state.cfg.listen.ip());
    let https = state.cfg.trusted.resolve(peer, &headers).1;
    let mut response = if let Some(response) = crate::http_body_limit::precheck(&headers) {
        response
    } else if let Some(response) = crate::http_route_response::response(
        &decision,
        &method,
        request.uri(),
        &headers,
        https,
        &state.cfg.listen.to_string(),
    ) {
        response
    } else {
        next.run(request).await
    };
    crate::http_route_response::suppress_head(&method, &mut response);
    crate::http_cors::apply(&headers, &mut response);
    response
}
