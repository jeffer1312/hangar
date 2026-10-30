//! Contrato de configuração compartilhada sem backend Python disponível.
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use std::sync::Arc;


#[test]
fn config_sync_runtime_jev_publication_matches_python() {
    use hangar_server::config_sync::runtime::RuntimeConfig;
    use serde_json::{Value,json};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/jev-sync.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();
        let service=RuntimeConfig::new(AccountEnvironment::from_map([("HOME".into(),home.path().to_string_lossy().into_owned()),("USERPROFILE".into(),home.path().to_string_lossy().into_owned())].into())).unwrap();
        for (path,bytes) in case["files"].as_object().unwrap() {
            let path=home.path().join(path);std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path,bytes.as_str().unwrap().replace("⟦HOME⟧",&home.path().to_string_lossy())).unwrap();
        }
        for step in case["steps"].as_array().unwrap() {
            let body=step["body"].as_object().unwrap();
            let changes=body.iter().filter(|(_,v)|!v.is_null()).map(|(k,v)|(k.clone(),v.clone())).collect();
            let remove=body.iter().filter(|(_,v)|v.is_null()).map(|(k,_)|k.clone()).collect();
            let result=match service.apply(&changes,&remove) {Ok(value)=>json!({"ok":value}),Err(error)=>json!({"error":error.replace(home.path().to_str().unwrap(),"⟦HOME⟧"),"kind":"ValueError"})};
            assert_eq!(result,step["result"],"{}",case["name"]);
            fn collect(root:&std::path::Path,path:&std::path::Path,out:&mut serde_json::Map<String,Value>) {
                for entry in std::fs::read_dir(path).unwrap() {
                    let path=entry.unwrap().path();
                    if path.is_dir(){collect(root,&path,out);}else {
                        out.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\',"/"),json!(std::fs::read_to_string(&path).unwrap().replace(root.to_str().unwrap(),"⟦HOME⟧")));
                    }
                }
            }
            let mut files=serde_json::Map::new();collect(home.path(),home.path(),&mut files);
            assert_eq!(json!(files),step["files"],"arquivos {}",case["name"]);
        }
    }
}

#[test]
fn config_sync_runtime_legacy_choice_matches_python() {
    use hangar_server::config_sync::runtime::RuntimeConfig;
    use serde_json::{Value,json};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/legacy.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();
        let service=RuntimeConfig::new(AccountEnvironment::from_map([("HOME".into(),home.path().to_string_lossy().into_owned()),("USERPROFILE".into(),home.path().to_string_lossy().into_owned())].into())).unwrap();
        std::fs::create_dir_all(service.path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(home.path().join(".hangar")).unwrap();
        std::fs::write(&service.path,case["runtime"].to_string()).unwrap();
        if !case["entry"].is_null() {
            let (path,data)=if case["parked"]==true {(home.path().join(".hangar/computer-control.json"),case["entry"].clone())}
            else {(home.path().join(".claude.json"),json!({"mcpServers":{"hangar-computer-control":case["entry"]}}))};
            std::fs::write(path,data.to_string()).unwrap();
        }
        for (key,value) in case["effective"].as_object().unwrap() {
            assert_eq!(&service.get(key),value,"{case}: {key}");
        }
    }
}

#[test]
fn config_sync_runtime_state_and_writes_match_python() {
    use hangar_server::config_sync::runtime::RuntimeConfig;
    use serde_json::{Value,json};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/runtime.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();
        let env=AccountEnvironment::from_map([("HOME".into(),home.path().to_string_lossy().into_owned()),("USERPROFILE".into(),home.path().to_string_lossy().into_owned()),("CP_SCAN_ROOTS".into(),String::new())].into());
        let service=RuntimeConfig::new(env).unwrap();
        std::fs::create_dir_all(service.path.parent().unwrap()).unwrap();
        let thinking=home.path().join(".claude/settings.json");
        if let Some(raw)=case["initial"].as_str(){std::fs::write(&service.path,raw).unwrap();}
        if let Some(raw)=case["settings"].as_str(){std::fs::write(&thinking,raw).unwrap();}
        if let Some(raw)=case["proxy"].as_str(){
            let path=home.path().join(".cli-proxy-api/config.yaml");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path,raw).unwrap();
        }
        for step in case["steps"].as_array().unwrap() {
            assert_eq!(service.state().unwrap(),step["before"],"antes {}",case["name"]);
            let body=step["body"].as_object().unwrap();
            let changes=body.iter().filter(|(_,v)|!v.is_null()).map(|(k,v)|(k.clone(),v.clone())).collect();
            let remove=body.iter().filter(|(_,v)|v.is_null()).map(|(k,_)|k.clone()).collect();
            let result=service.apply(&changes,&remove);
            if matches!(case["name"].as_str(),Some("bad_runtime"|"root_runtime")) {
                assert_eq!(result.unwrap_err(),"runtime-config.json ilegível; arquivo preservado");
                assert_eq!(std::fs::read_to_string(&service.path).unwrap(),case["initial"].as_str().unwrap());
                continue;
            }
            let actual=match result {Ok(value)=>json!({"ok":value}),Err(error)=>json!({"error":error,"kind":"ValueError"})};
            let mut expected=step["result"].clone();
            if matches!(case["name"].as_str(),Some("thinking_invalid"|"thinking_root")) {
                expected["error"]=json!("mostrar_pensamento: settings.json ilegível — não mexi nele");
            }
            assert_eq!(actual,expected,"resultado {}",case["name"]);
            assert_eq!(service.state().unwrap(),step["after"],"depois {}",case["name"]);
            assert_eq!(json!(std::fs::read_to_string(&service.path).ok()),step["runtime"],"arquivo {}",case["name"]);
            assert_eq!(json!(std::fs::read_to_string(&thinking).ok()),step["settings"],"pensamento {}",case["name"]);
        }
    }
}

#[tokio::test]
async fn config_sync_runtime_settings_are_native() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/runtime.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".hangar/current");
    std::fs::create_dir_all(&root).unwrap();
    hangar_server::boot_version::publish(&root, hangar_server::boot_version::Snapshot {
        describe:"fixture-running".into(),readable:None,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {listen:address,upstream:crate::refused_address(),internal_secret:"fixture-internal".into(),auth_token:"fixture-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts = AccountService::new(AccountEnvironment::from_map([
        ("HOME".into(),home.path().to_string_lossy().into_owned()),
        ("USERPROFILE".into(),home.path().to_string_lossy().into_owned()),
        ("CP_SCAN_ROOTS".into(),String::new()),
    ].into()));
    let router = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move {axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
    let response = reqwest::Client::new().get(format!("http://{address}/api/config")).bearer_auth("fixture-owner").send().await.unwrap();
    assert_eq!(response.status(),200,"configuração precisa responder sem Python");
    assert_eq!(response.json::<serde_json::Value>().await.unwrap()["campos"],fixture["cases"][0]["steps"][0]["before"]);
    task.abort();
}

#[tokio::test]
#[cfg(unix)]
async fn config_sync_command_capture_preserves_streams_and_limits() {
    use hangar_server::config_sync::command;
    use serde_json::{Value,json};
    use std::time::Duration;
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/commands.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let env = AccountEnvironment::from_map([
        ("HOME".into(),home.path().to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR".into(),"/fixture/secondary".into()),
        ("CP_AUTH_TOKEN".into(),"fixture-private".into()),
    ].into());
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let script = match name {
            "stdout" => "printf 'resultado\\n'".to_owned(),
            "stderr" => "printf 'saída\\n'; printf 'erro\\n' >&2; exit 3".into(),
            "empty" => ":".into(),
            "unicode_tail" => format!("printf '{}\\n'","á".repeat(350)),
            "newlines" => "printf 'first\\rsecond\\r\\nthird\\n'".into(),
            "decode" => "printf 'bad\\377' >&2".into(),
            "config_env" => "printf '%s\\n' \"${CLAUDE_CONFIG_DIR-ausente}\"".into(),
            "timeout" => "/bin/sleep 10".into(),
            "missing" | "permission" => String::new(),
            _ => panic!("caso desconhecido"),
        };
        let mut args = vec!["/bin/sh".into(),"-c".into(),script];
        if name == "missing" {args=vec![home.path().join("missing").to_string_lossy().into_owned()];}
        if name == "permission" {
            let file=home.path().join("no-exec"); std::fs::write(&file,"fixture").unwrap(); args=vec![file.to_string_lossy().into_owned()];
        }
        let budget = if name == "timeout" {Duration::from_millis(40)} else {Duration::from_secs(5)};
        assert_eq!(json!(command::run(args,env.clone(),budget).await),case["result"],"{name}");
    }
    assert_eq!(command::run(vec!["/bin/sh".into(),"-c".into(),"printf '%s' \"${CP_AUTH_TOKEN-ausente}\"".into()],env,Duration::from_secs(5)).await,(true,"ausente".into()));
}

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

#[tokio::test]
async fn config_sync_plugin_install_and_numeric_hash_match_python() {
    use hangar_server::config_sync::{apply::Apply, bundle::{Bundle, hash, python_json}, export::Context, paths::Roots};
    use serde_json::{Value, json};
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/plugins.json")).unwrap();
    for case in fixture["hashes"].as_array().unwrap() {
        assert_eq!(python_json(&case["value"], true), case["serialized"].as_str().unwrap());
        assert_eq!(hash(&case["value"]), case["hash"].as_str().unwrap());
    }
    for case in fixture["cases"].as_array().unwrap() {
        let folder = tempfile::tempdir().unwrap();
        let home = folder.path().to_string_lossy().into_owned();
        let roots = Roots {claude:format!("{home}/.claude"),codex:format!("{home}/.codex"),hangar:format!("{home}/hangar"),home};
        let plugins = std::path::Path::new(&roots.claude).join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::write(plugins.join("known_marketplaces.json"), case["known"].to_string()).unwrap();
        std::fs::write(plugins.join("installed_plugins.json"), json!({"plugins":case["installed"]}).to_string()).unwrap();
        let bundle = Bundle {items:[("claude_plugins".into(),case["data"].clone())].into_iter().collect(), ..Default::default()};
        let mut context = Context::new(roots.clone());
        let (progress, mut events) = tokio::sync::mpsc::unbounded_channel();
        context.progress = Some(progress);
        context.emit(json!({"phase":"apply","item":"claude_plugins","entry":null,"index":0,"total":1}));
        let mut apply = Apply::new(context, bundle,"fixture",[("claude".into(),"/fixture/claude".into())].into());
        let result = apply.apply_item("claude_plugins");
        result.unwrap();
        let mut calls = Vec::new();
        let result = apply.install_plugins_with(|args| {
            calls.push(args);
            std::future::ready(if case["mode"] == "failed" {(false,"fatal: repo not found".into())} else {(true,String::new())})
        }).await;
        apply.finish_item("claude_plugins",result);
        assert_eq!(apply.report(),case["report"],"{}",case["mode"]);
        let expand = |s:&str| s.replace("⟦HOME⟧",&roots.home);
        assert_eq!(json!(calls),hangar_server::config_sync::paths::map_strings(&case["calls"], &expand));
        let mut received = Vec::new();
        while let Ok(event) = events.try_recv() { received.push(event); }
        assert_eq!(json!(received),case["progress"]);
        let settings: Value = serde_json::from_slice(&std::fs::read(std::path::Path::new(&roots.claude).join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings,case["settings"]);
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
#[cfg(unix)]
async fn config_sync_codex_applies_through_native_writer() {
    use hangar_server::config_sync::{apply::Apply, bundle::Bundle, export::Context, paths::Roots};
    use serde_json::{Value, json};
    use std::os::unix::fs::PermissionsExt;
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/codex.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().to_string_lossy().into_owned();
        let roots = Roots {claude:format!("{home}/.claude"),codex:format!("{home}/.codex"),hangar:format!("{home}/hangar"),home};
        std::fs::create_dir_all(&roots.codex).unwrap();
        let path = std::path::Path::new(&roots.codex).join("config.toml");
        std::fs::write(&path,case["current"].as_str().unwrap()).unwrap();
        let bin = base.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let script = bin.join("codex");
        std::fs::write(&script,r##"#!/bin/sh
while IFS= read -r message; do
 id=$(printf '%s' "$message" | /usr/bin/sed -n 's/.*"id":\([0-9]*\).*/\1/p')
 case "$message" in
 *'"method":"config/batchWrite"'*)
   printf '%s\n' "$message" >> "$FIXTURE_CALLS"
   printf 'model = "nativo"\n' > "$CODEX_HOME/config.toml"
   printf '{"id":%s,"result":{}}\n' "$id";;
 *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id";;
 esac
done
"##).unwrap();
        std::fs::set_permissions(&script,std::fs::Permissions::from_mode(0o700)).unwrap();
        let calls = base.path().join("calls.jsonl");
        let env = AccountEnvironment::from_map([
            ("HOME".into(),roots.home.clone()),("PATH".into(),bin.to_string_lossy().into_owned()),
            ("FIXTURE_CALLS".into(),calls.to_string_lossy().into_owned()),
        ].into());
        let bundle = Bundle {items:[("codex".into(),json!({"config":case["incoming"]}))].into_iter().collect(),..Default::default()};
        let programs = serde_json::from_value(case["programs"].clone()).unwrap();
        let mut apply = Apply::new(Context::new(roots.clone()),bundle,"20000101-000000-012345",programs);
        apply.accounts = AccountService::new(env);
        let result = apply.apply_item("codex");
        result.unwrap();
        let result = apply.apply_codex_config().await;
        apply.finish_item("codex",result);
        assert_eq!(std::fs::read_to_string(&path).unwrap(),case["final"].as_str().unwrap(),"{}",case["mode"]);
        let expand = |s:&str|s.replace("⟦HOME⟧",&roots.home);
        let mut expected=hangar_server::config_sync::paths::map_strings(&case["report"],&expand);
        if case["mode"]=="invalid" {
            expected["items"]["codex"]["warnings"][0]["params"]["error"]=json!("config.toml inválido");
        }
        assert_eq!(apply.report(),expected);
        let actual:Vec<Value> = std::fs::read_to_string(calls).unwrap_or_default().lines().map(|line|{
            let value:Value = serde_json::from_str(line).unwrap();
            json!({"method":value["method"],"params":value["params"]})
        }).collect();
        assert_eq!(json!(actual),case["calls"]);
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
