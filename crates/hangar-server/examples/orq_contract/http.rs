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
        "http.request" => request(input),
        _ => return None,
    })
}

fn request(input: &Value) -> Result<Value, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        use axum::{body::{Body, to_bytes}, extract::Request, response::IntoResponse, routing::any};
        use hangar_server::list::{bridge::{ListBridge, ListEnv, parse_dirs}, facts::FactsClient, mux::Mux, procs::SystemProcs};
        let deliveries = Arc::new(Mutex::new(Vec::<Value>::new()));
        let saved = deliveries.clone();
        let upstream = axum::Router::new().fallback(any(move |req: Request| {
            let saved = saved.clone();
            async move {
                let path = req.uri().path().to_owned();
                let method = req.method().to_string();
                let body = to_bytes(req.into_body(), 1024 * 1024).await.unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                saved.lock().unwrap().push(json!({"method":method,"path":path,"body":body}));
                axum::http::Response::builder().header("content-type","application/json").body(Body::from(json!({"ok":true,"delivered":true}).to_string())).unwrap().into_response()
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
        let request = Request::builder().method(input["method"].as_str().unwrap_or("GET"))
            .uri(input["path"].as_str().ok_or("caminho da prova ausente")?)
            .header("content-type", "application/json")
            .body(Body::from(input.get("body").map(Value::to_string).unwrap_or_default())).map_err(|e| e.to_string())?;
        let response = orq::serve(&Arc::new(state), request).await;
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024).await.map_err(|e| e.to_string())?;
        let body: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        server.abort();
        let deliveries = deliveries.lock().unwrap().clone();
        Ok(json!({"status":status,"body":body,"deliveries":deliveries}))
    })
}
