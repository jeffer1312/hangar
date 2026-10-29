//! Contrato de configuração compartilhada sem backend Python disponível.
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use std::sync::Arc;

#[test]
fn config_sync_aliases_match_python_and_serialize_writers() {
    use hangar_server::accounts::aliases;
    use serde_json::Value;
    let contract: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/aliases.json"
    ))
    .unwrap();
    for case in contract["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let env = AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    home.path().to_string_lossy().into_owned(),
                ),
            ]
            .into(),
        );
        let path = aliases::path(&env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if let Some(raw) = case["initial"].as_str() {
            std::fs::write(&path, raw).unwrap();
        }
        assert_eq!(Value::Object(aliases::read(&env)), case["before"]);
        assert_eq!(
            Value::Object(aliases::set(&env, "a", case["value"].as_str()).unwrap()),
            case["result"]
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            case["raw"].as_str().unwrap()
        );
    }
    let home = tempfile::tempdir().unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                home.path().to_string_lossy().into_owned(),
            ),
        ]
        .into(),
    );
    std::thread::scope(|scope| {
        for i in 0..32 {
            let env = &env;
            scope.spawn(move || aliases::set(env, &format!("id-{i}"), Some("Nome")).unwrap());
        }
    });
    assert_eq!(aliases::read(&env).len(), 32);
}

#[tokio::test]
async fn config_sync_translation_matches_python_batches_cache_and_failures() {
    use hangar_server::config_sync::translate::Translator;
    use serde_json::{Value, json};
    let contract: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/translation.json"
    ))
    .unwrap();
    for case in contract["cases"].as_array().unwrap() {
        let base = tempfile::tempdir().unwrap();
        let path = base.path().join("traducoes.json");
        let mut translator = Translator::default();
        assert_eq!(translator.retry_waits.map(|d| d.as_secs()), [20, 30]);
        translator.retry_waits = [std::time::Duration::ZERO; 2];
        translator.save_error = case["disk_error"]
            .as_bool()
            .unwrap()
            .then(|| "disco cheio".into());
        let texts: Vec<String> = serde_json::from_value(case["texts"].clone()).unwrap();
        let lang = case["lang"].as_str().unwrap();
        let mut answers = case["answers"].as_array().unwrap().iter();
        let mut calls = Vec::new();
        let result = translator
            .translate(&path, &texts, lang, |system, prompt| {
                let batch: Vec<String> = serde_json::from_str(&prompt).unwrap();
                calls.push(json!({"system":system,"prompt":prompt,"temperature":0.1,"timeout":60}));
                let answer = answers
                    .next()
                    .expect("chamada extra ao provedor")
                    .as_str()
                    .unwrap();
                std::future::ready(match answer {
                    "429" => Err("provedor 429: rate limit".into()),
                    "erro" => Err("falha simulada".into()),
                    "prefix" => Ok(serde_json::to_string(
                        &batch
                            .iter()
                            .map(|s| format!("{lang}:{s}"))
                            .collect::<Vec<_>>(),
                    )
                    .unwrap()),
                    raw => Ok(raw.to_owned()),
                })
            })
            .await;
        assert_eq!(
            result,
            json!({"texts":case["output"],"error":case["error"]}),
            "{}",
            case["name"]
        );
        assert_eq!(json!(calls), case["calls"], "chamadas {}", case["name"]);
        let cache: Value = std::fs::read(&path)
            .ok()
            .map(|b| serde_json::from_slice(&b).unwrap())
            .unwrap_or(Value::Null);
        assert_eq!(cache, case["cache"], "cache {}", case["name"]);
        if !case["cached"].is_null() {
            let again = translator
                .translate(&path, &texts, lang, |_, _| {
                    std::future::ready(Err("não deveria consultar o provedor".into()))
                })
                .await;
            assert_eq!(again, result);
        }
    }
}

#[test]
fn config_sync_preferences_match_executed_python() {
    let contract: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/preferences.json"
    ))
    .unwrap();
    for case in contract["cases"].as_array().unwrap() {
        let result = hangar_server::config_sync::preferences::coerce(
            case["key"].as_str().unwrap(),
            &case["input"],
        );
        if let Some(error) = case["error"].as_str() {
            assert_eq!(result.unwrap_err(), error, "{case}");
        } else {
            assert_eq!(result.unwrap(), case["output"], "{case}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn config_sync_bundles_match_python_over_http() {
    use base64::Engine;
    use hangar_server::config_sync::{bundle::Bundle, export::Context, paths::Roots};
    use serde_json::{Value, json};
    use std::os::unix::fs::{PermissionsExt, symlink};
    let contract: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/bundles.json"
    ))
    .unwrap();
    for case in contract["cases"].as_array().unwrap() {
        let base = tempfile::tempdir().unwrap();
        let expand = |s: &str| s.replace("⟦FIXTURE⟧", &base.path().to_string_lossy());
        for (name, entry) in case["initial"].as_object().unwrap() {
            let path = base.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if let Some(link) = entry["link"].as_str() {
                symlink(expand(link), &path).unwrap();
            } else {
                let bytes = if let Some(text) = entry["text"].as_str() {
                    expand(text).into_bytes()
                } else {
                    base64::engine::general_purpose::STANDARD
                        .decode(entry["base64"].as_str().unwrap())
                        .unwrap()
                };
                std::fs::write(&path, bytes).unwrap();
                std::fs::set_permissions(
                    path,
                    std::fs::Permissions::from_mode(entry["mode"].as_u64().unwrap() as u32),
                )
                .unwrap();
            }
        }
        let source: Roots = serde_json::from_value(hangar_server::config_sync::paths::map_strings(
            &case["source"],
            &expand,
        ))
        .unwrap();
        let mut context = Context::new(source.clone());
        let (progress, mut events) = tokio::sync::mpsc::unbounded_channel();
        context.progress = Some(progress);
        context.preferences = std::path::Path::new(&source.home)
            .parent()
            .unwrap()
            .join("prefs.json");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address,
            upstream: crate::refused_address(),
            internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.config_sync_context = Some(context);
        state.accounts = AccountService::new(AccountEnvironment::from_map(
            [
                ("HOME".into(), source.home.clone()),
                ("USERPROFILE".into(), source.home),
            ]
            .into(),
        ));
        let router = hangar_server::routes::router(Arc::new(state));
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let items = case["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(",");
        let response = reqwest::Client::new()
            .get(format!(
                "http://{address}/api/config-sync/bundle?items={items}"
            ))
            .bearer_auth("fixture-owner")
            .send()
            .await
            .unwrap();
        let status = response.status();
        let raw = response.bytes().await.unwrap();
        task.abort();
        let mut received = Vec::new();
        while let Ok(event) = events.try_recv() {
            received.push(event);
        }
        assert_eq!(json!(received), case["progress"], "progresso {items}");
        assert_eq!(status, 200, "{items}: {}", String::from_utf8_lossy(&raw));
        let actual = Bundle::unpack(&raw).unwrap();
        assert_eq!(
            actual.items,
            case["bundle"]["items"].as_object().unwrap().clone(),
            "itens {items}"
        );
        assert_eq!(
            actual.warnings,
            case["bundle"]["warnings"].as_object().unwrap().clone(),
            "avisos {items}"
        );
        let files:serde_json::Map<_,_>=actual.files.into_iter().map(|(name,blob)|(name,json!({"data":base64::engine::general_purpose::STANDARD.encode(blob.data),"mode":blob.mode}))).collect();
        assert_eq!(
            Value::Object(files),
            case["bundle"]["files"],
            "arquivos {items}"
        );
        let target: Roots = serde_json::from_value(hangar_server::config_sync::paths::map_strings(
            &case["target"],
            &expand,
        ))
        .unwrap();
        let mut target_context = Context::new(target.clone());
        target_context.preferences = std::path::Path::new(&target.home)
            .parent()
            .unwrap()
            .join("prefs.json");
        let mut apply = hangar_server::config_sync::apply::Apply::new(
            target_context,
            Bundle::unpack(&raw).unwrap(),
            "20000101-000000-012345",
            Default::default(),
        );
        for item in case["items"].as_array().unwrap() {
            let item = item.as_str().unwrap();
            let result = apply.apply_item(item);
            apply.finish_item(item, result);
        }
        assert_eq!(
            apply.report(),
            hangar_server::config_sync::paths::map_strings(&case["report"], &expand),
            "aplicação {items}"
        );
    }
}

#[test]
fn config_sync_helpers_match_executed_python() {
    use hangar_server::config_sync::{describe, paths};
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/helpers.json"
    ))
    .unwrap();
    for case in cases["paths"].as_array().unwrap() {
        let roots: paths::Roots = serde_json::from_value(case["roots"].clone()).unwrap();
        let input = case["input"].as_str().unwrap();
        assert_eq!(
            paths::canonicalize(input, &roots),
            case["canonical"].as_str().unwrap(),
            "canonicalize {input:?}"
        );
        assert_eq!(
            paths::resolve(input, &roots),
            case["resolved"].as_str().unwrap(),
            "resolve {input:?}"
        );
        assert_eq!(
            serde_json::json!(paths::marked_paths(input)),
            case["marked"],
            "marked_paths {input:?}"
        );
    }
    for case in cases["programs"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        let (output, missing) = paths::fix_programs(
            input,
            |n| case["available"][n].as_str().map(str::to_owned),
            |p| p.starts_with("/exists/"),
        );
        assert_eq!(
            output,
            case["output"].as_str().unwrap(),
            "fix_programs {input:?}"
        );
        assert_eq!(
            serde_json::json!(missing),
            case["missing"],
            "missing {input:?}"
        );
    }
    let folder = tempfile::tempdir().unwrap();
    for case in cases["descriptions"].as_array().unwrap() {
        let path = folder.path().join(case["name"].as_str().unwrap());
        std::fs::write(&path, case["content"].as_str().unwrap()).unwrap();
        assert_eq!(
            describe::describe(&path),
            case["output"].as_str().unwrap(),
            "describe {case}"
        );
    }
}

#[tokio::test]
async fn config_sync_manifest_is_native_without_python() {
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                home.path().to_string_lossy().into_owned(),
            ),
        ]
        .into(),
    ));
    let router = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let response = reqwest::Client::new()
        .get(format!("http://{address}/api/config-sync/manifest"))
        .bearer_auth("fixture-owner")
        .send()
        .await
        .unwrap();
    let status = response.status();
    let translation = reqwest::Client::new()
        .post(format!("http://{address}/api/config-sync/translate"))
        .bearer_auth("fixture-owner")
        .header("content-type", "application/json")
        .body(r#"{"lang":"pt","texts":["Lê o arquivo quando o hook roda."]}"#)
        .send()
        .await
        .unwrap();
    let validation: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/validation.json"
    ))
    .unwrap();
    for case in validation["cases"].as_array().unwrap() {
        let response = reqwest::Client::new()
            .post(format!("http://{address}/api/config-sync/translate"))
            .bearer_auth("fixture-owner")
            .header("content-type", "application/json")
            .body(case["raw"].as_str().unwrap().to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            case["status"].as_u64().unwrap() as u16,
            "{}",
            case["raw"]
        );
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap(),
            case["body"],
            "validação {}",
            case["raw"]
        );
    }
    let progress = reqwest::Client::new()
        .get(format!(
            "http://{address}/api/config-sync/manifest?stream=1"
        ))
        .bearer_auth("fixture-owner")
        .send()
        .await
        .unwrap();
    assert!(
        progress.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/x-ndjson")
    );
    let lines: Vec<serde_json::Value> = progress
        .text()
        .await
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.last().unwrap()["type"], "done");
    assert_eq!(
        lines
            .iter()
            .filter(|e| e["type"] == "progress" && e["entry"].is_null())
            .count(),
        13
    );
    task.abort();
    assert_eq!(
        status, 200,
        "o manifesto precisa funcionar sem consultar o Python"
    );
    assert_eq!(
        translation.status(),
        200,
        "a tradução precisa funcionar sem consultar o Python"
    );
    assert_eq!(
        translation.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({"texts":["Lê o arquivo quando o hook roda."],"error":""})
    );
}
