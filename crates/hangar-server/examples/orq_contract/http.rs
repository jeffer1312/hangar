//! Operações HTTP da prova de migração.
use hangar_server::{orchestration::{self as orq, plan, runs}, routes::AppState};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::{Arc, Mutex}, time::Duration};

pub fn evaluate(action: &str, input: &Value) -> Option<Result<Value, String>> {
    let text = |key: &str| input[key].as_str().unwrap_or("");
    Some(match action {
        "conductor.watchdog" => Ok(runs::watchdog(Path::new(text("directory")), input.get("units").filter(|v| !v.is_null()), input["now"].as_f64().unwrap())),
        "conductor.units" => Ok(runs::units().unwrap_or(Value::Null)),
        "conductor.veto" => Ok(json!(hangar_server::orq_conductor::JEV_VETO_P)),
        "conductor.exec-dir" => Ok(json!(runs::exec_dir(Path::new(text("home")), text("id")))),
        "plan.readiness" => Ok(plan::readiness(input["cwd"].as_str(), input["grouped"] == true, input["roles"] == true)),
        "plan.kickoff" => Ok(json!(plan::kickoff(&input["readiness"], text("contract")))),
        "plan.find" => Ok(plan::find(Path::new(text("repo")))),
        "plan.sha" => Ok(json!(plan::sha(text("text")))),
        "runs.activity" => Ok(json!(runs::activity(Path::new(text("timeline")), input["now"].as_f64().unwrap_or_else(runs::now)))),
        "runs.timeline-path" => Ok(json!(runs::timeline_path(Path::new(text("directory"))))),
        "timeline.history" => {
            use hangar_server::transcript::{HistoryRequest, Provider, TAIL_WINDOW, merged_history};
            merged_history(&HistoryRequest { provider: Provider::Orq, jsonl: text("path").into(), queue: None, limit: None, tail_window: TAIL_WINDOW })
                .map(|events|json!(events)).map_err(|e|e.to_string())
        }
        "http.request" => request(input, false),
        "http.public" => request(input, true),
        _ => return None,
    })
}

fn request(input: &Value, public: bool) -> Result<Value, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        use axum::{body::{Body, to_bytes}, extract::Request, response::IntoResponse, routing::any};
        use hangar_server::list::{bridge::{ListBridge, ListEnv, parse_dirs}, facts::FactsClient, mux::Mux, procs::SystemProcs};
        let deliveries = Arc::new(Mutex::new(Vec::<Value>::new()));
        let saved = deliveries.clone();
        let upstream_response = input.get("upstream_response").cloned().unwrap_or(json!({"status":200,"body":{"ok":true,"delivered":true}}));
        let upstream = axum::Router::new().fallback(any(move |req: Request| {
            let saved = saved.clone();
            let upstream_response = upstream_response.clone();
            async move {
                let path = req.uri().path().to_owned();
                let method = req.method().to_string();
                let body = to_bytes(req.into_body(), 1024 * 1024).await.unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                saved.lock().unwrap().push(json!({"method":method,"path":path,"body":body}));
                axum::http::Response::builder().status(upstream_response["status"].as_u64().unwrap_or(200) as u16)
                    .header("content-type","application/json").body(Body::from(upstream_response["body"].to_string())).unwrap().into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
        let upstream_addr = listener.local_addr().map_err(|e| e.to_string())?;
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await });
        let mut state = AppState::new(hangar_server::config::Config {
            listen: "127.0.0.1:0".parse().unwrap(), upstream: upstream_addr,
            internal_secret: "fixture-internal".into(), auth_token: "fixture-owner".into(),
            log_path: None, trusted: hangar_server::auth::TrustedHosts::parse("127.0.0.1"),
        });
        let env: BTreeMap<String,String> = serde_json::from_value(input["env"].clone()).map_err(|e| e.to_string())?;
        state.accounts.env = hangar_server::accounts::environment::AccountEnvironment::from_map(env);
        state.groups = None;
        let mux = input["mux"].as_str().ok_or("multiplexador da prova ausente")?;
        let list_env = ListEnv {
            mux: Mux::with_program(mux, Duration::from_secs(2)), capture_program: mux.into(),
            procs: Arc::new(SystemProcs::default()), dirs: parse_dirs(&input["dirs"].to_string()),
        };
        state.list = Arc::new(ListBridge::new(list_env, FactsClient::new(upstream_addr, "fixture-internal".into())));
        state.side.locate = Arc::new(hangar_server::locate::ListLocate { list: state.list.clone() });
        let mut request = Request::builder().method(input["method"].as_str().unwrap_or("GET"))
            .uri(input["path"].as_str().ok_or("caminho da prova ausente")?)
            .header("content-type", "application/json")
            .body(Body::from(input.get("body").map(Value::to_string).unwrap_or_default())).map_err(|e| e.to_string())?;
        let state = Arc::new(state);
        let response = if public {
            use tower_service::Service;
            request.extensions_mut().insert(axum::extract::ConnectInfo("127.0.0.1:5000".parse::<std::net::SocketAddr>().unwrap()));
            request.headers_mut().insert("x-forwarded-for", "198.51.100.80".parse().unwrap());
            if input["authenticated"] != false { request.headers_mut().insert("authorization", "Bearer fixture-owner".parse().unwrap()); }
            if input["forwarded"] == true {
                let path = request.uri().path().to_owned();
                let query = request.uri().query().map(|q|format!("?{q}")).unwrap_or_default();
                request.headers_mut().remove("x-forwarded-for");
                request.headers_mut().insert("x-hangar-path", path.parse().unwrap());
                request.headers_mut().insert("x-hangar-internal", "fixture-internal".parse().unwrap());
                *request.uri_mut() = format!("/__hangar_server/forwarded{query}").parse().unwrap();
            }
            let mut router = if input["forwarded"] == true {
                hangar_server::routes::terminal_router(state.clone())
            } else {
                hangar_server::routes::router(state.clone())
            };
            router.call(request).await.map_err(|e|e.to_string())?
        } else { orq::serve(&state, request).await };
        let status = response.status().as_u16();
        let body: Value = if input["state_only"] == true && status == 200 {
            use futures_util::StreamExt;
            let mut stream = response.into_body().into_data_stream();
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut frames = String::new();
                while let Some(chunk) = stream.next().await {
                    frames.push_str(&String::from_utf8_lossy(&chunk.map_err(|e| e.to_string())?));
                    let normalized = frames.replace("\r\n", "\n");
                    for frame in normalized.split("\n\n") {
                        if frame.starts_with("event: state\n") {
                            if let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) {
                                return serde_json::from_str(data).map_err(|e| e.to_string());
                            }
                        }
                    }
                }
                Err("o SSE terminou sem publicar estado".into())
            }).await.map_err(|_| "o SSE não publicou estado no prazo".to_owned())??
        } else {
            let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024).await.map_err(|e| e.to_string())?;
            if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).map_err(|e| e.to_string())? }
        };
        let body = if input["poll_panel"] == true && status == 200 {
            let first = body;
            let second = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let request = Request::builder().uri(input["path"].as_str().unwrap()).body(Body::empty()).unwrap();
                    let response = orq::serve(&state, request).await;
                    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024).await.map_err(|e|e.to_string())?;
                    let value: Value = serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
                    if !value["consumption"].is_null() { return Ok::<_,String>(value); }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.map_err(|_|"o painel não publicou consumo no prazo".to_owned())??;
            json!({"first":first,"second":second})
        } else { body };
        server.abort();
        let deliveries = deliveries.lock().unwrap().clone();
        Ok(json!({"status":status,"body":body,"deliveries":deliveries}))
    })
}
