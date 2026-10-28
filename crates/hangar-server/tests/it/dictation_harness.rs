use axum::{Router, routing::get};
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    list::{
        bridge::{ListBridge, ListEnv, parse_dirs},
        facts::FactsClient,
        mux::Mux,
    },
    routes::AppState,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

async fn attempt(provider: &str, oauth: bool, model: Option<&str>, generation: &str) -> Value {
    attempt_with_context(provider, oauth, model, generation, false, None, false).await
}
async fn attempt_with_context(
    provider: &str,
    oauth: bool,
    model: Option<&str>,
    generation: &str,
    include: bool,
    snapshot: Option<Value>,
    locked: bool,
) -> Value {
    attempt_with_budget(
        provider, oauth, model, generation, include, snapshot, locked, None, None, false,
    )
    .await
    .0
}
/// Resultado e o registro do que o CLI de fixture recebeu (argv, ambiente, prompt de sistema).
async fn attempt_with_budget(
    provider: &str,
    oauth: bool,
    model: Option<&str>,
    generation: &str,
    include: bool,
    snapshot: Option<Value>,
    locked: bool,
    budget: Option<Duration>,
    account_snapshot: Option<&str>,
    catalog_deadline: bool,
) -> (Value, Value) {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_BIN_EXE_hangar-dictation-fixture"));
    for name in ["claude", "codex", "tmux"] {
        std::fs::copy(
            &fixture,
            bin.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            }),
        )
        .unwrap();
    }
    let home = root.path().join("home");
    let account = home.join(format!("{provider}-a"));
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&account).unwrap();
    if provider == "claude" {
        std::fs::write(
            account.join("settings.json"),
            if oauth {"{}".to_owned()} else {json!({"env":{"ANTHROPIC_API_KEY":"fixture-api-key"},"fixture_catalog_stall":budget.is_some() || catalog_deadline}).to_string()},
        )
        .unwrap();
        if oauth {
            std::fs::write(
                account.join(".credentials.json"),
                r#"{"claudeAiOauth":{"accessToken":"fixture-token"}}"#,
            )
            .unwrap();
        }
    }
    let folder = home.join(".hangar").join(if provider == "claude" {
        "claude-headless"
    } else {
        "codex-sessions"
    });
    std::fs::create_dir_all(&folder).unwrap();
    let metadata = if provider == "claude" {
        json!({"name":"destination","session_id":"00000000-0000-0000-0000-000000000001","cwd":project,"config_dir":account,"key":"original","model":"conversation-model"})
    } else {
        json!({"name":"destination","cwd":project,"codex_home":account,"key":"original","headless":true,"model":"conversation-model"})
    };
    std::fs::write(folder.join("destination.json"), metadata.to_string()).unwrap();
    let transcripts = account
        .join("projects")
        .join(hangar_workspace::worktrees::sanitize_cwd(
            &project.to_string_lossy(),
        ));
    std::fs::create_dir_all(&transcripts).unwrap();
    let transcript = transcripts.join("00000000-0000-0000-0000-000000000001.jsonl");
    let lines = [
        json!({"type":"user","uuid":"old","message":{"role":"user","content":"Mensagem anterior excluída"}}),
        json!({"type":"user","uuid":"a","message":{"role":"user","content":"O projeto usa PostgreSQL"}}),
        json!({"type":"assistant","uuid":"b","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Segredo do pensamento"},{"type":"text","text":"O serviço se chama Hangar"},{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"segredo"}}]}}),
        json!({"type":"user","uuid":"tool","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"Segredo da ferramenta"}]}}),
        json!({"type":"user","uuid":"c","message":{"role":"user","content":"á".repeat(2100)}}),
    ];
    std::fs::write(
        &transcript,
        lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let selected = model
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("model-{provider}-a"));
    let config = json!({"organization":{"dictation_organization_mode":"harness","dictation_claude_model":selected,"dictation_codex_model":selected,"ditado_estilo":"limpar"}});
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = upstream_listener.local_addr().unwrap();
    let app = Router::new().route(
        "/internal/transcription/config",
        get(move || {
            let config = config.clone();
            async move { ([("content-type", "application/json")], config.to_string()) }
        }),
    );
    let python = tokio::spawn(async move {
        axum::serve(upstream_listener, app).await.unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream,
        internal_secret: "harness-secret".into(),
        auth_token: "owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    let environment = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("PATH".into(), bin.to_string_lossy().into_owned()),
            ("CP_AUTH_TOKEN".into(), "fixture-authority".into()),
        ]
        .into(),
    );
    state.accounts = AccountService::new(environment);
    let key = hangar_server::accounts::AccountKey::new(
        if provider == "claude" {
            hangar_server::accounts::Provider::Claude
        } else {
            hangar_server::accounts::Provider::Codex
        },
        &account,
    )
    .unwrap();
    let _guard = locked.then(|| {
        state
            .accounts
            .locks
            .try_acquire(&key, hangar_server::accounts::GuardMode::Exclusive)
            .unwrap()
    });
    let mux = bin.join(if cfg!(windows) { "tmux.exe" } else { "tmux" });
    let dirs=parse_dirs(&json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string());
    state.list = Arc::new(ListBridge::new(
        ListEnv {
            mux: Mux::with_program(&mux, Duration::from_secs(2)),
            capture_program: mux.into_os_string(),
            procs: Arc::new(hangar_server::list::procs::SystemProcs::default()),
            dirs,
        },
        FactsClient::new(upstream, "harness-secret".into()),
    ));
    let state = Arc::new(state);
    if catalog_deadline {
        let app = hangar_server::routes::router(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let requests = (0..4)
            .map(|_| {
                tokio::spawn(async move {
                    reqwest::Client::new()
                        .get(format!(
                            "http://{address}/api/dictation/models?session=destination&fresh=true"
                        ))
                        .bearer_auth("owner")
                        .send()
                        .await
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !account.join("fixture-catalog-stalled").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            state.dictation_slots.available_permits(),
            3,
            "só a consulta sob o mutex deve ocupar vaga"
        );
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        let responses = tokio::time::timeout(
            Duration::from_secs(2),
            futures_util::future::join_all(requests),
        )
        .await;
        tokio::time::resume();
        server.abort();
        python.abort();
        let responses = responses.expect("fresh concorrentes ultrapassaram o teto público de60s");
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../backend/tests/fixtures/contract/dictation_public/edges.json"
        ))
        .unwrap();
        let expected = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "catalog_timeout")
            .unwrap();
        let mut timed_out = 0;
        for response in responses {
            let response = response.unwrap();
            if response.status() == 503 {
                assert_eq!(
                    response.json::<Value>().await.unwrap(),
                    expected["response"]
                );
                timed_out += 1;
            }
        }
        assert!(timed_out > 0, "ninguém testou a espera pelo mutex");
        assert_eq!(state.dictation_slots.available_permits(), 4);
        return (Value::Null, Value::Null);
    }
    if let Some(budget) = budget {
        use hangar_server::dictation::{
            harness,
            model::{
                DictationStyle, OrganizationConfig, OrganizationMode, OrganizationRequest,
                OrganizationResult,
            },
        };
        let request = OrganizationRequest {
            raw: "  Hoje vamos conferir o ditado completo.\n".into(),
            session: Some("destination".into()),
            generation: Some(generation.into()),
            ..Default::default()
        };
        let cfg = OrganizationConfig {
            dictation_claude_model: "model-claude-a".into(),
            ..Default::default()
        };
        let original = OrganizationResult::original(request.raw.clone(), OrganizationMode::Harness);
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            harness::organize(
                &state,
                &request,
                &cfg,
                DictationStyle::Clean,
                original,
                tokio::time::Instant::now() + budget,
            ),
        )
        .await
        .expect("a espera pelo catálogo ignorou o prazo único da tentativa");
        assert!(
            !account.join("fixture-executed").exists(),
            "a organização não pode começar depois do prazo"
        );
        python.abort();
        return (serde_json::to_value(result).unwrap(), Value::Null);
    }
    let app = hangar_server::routes::router(state);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let response=reqwest::Client::new().post(format!("http://{address}/api/ditado/relimpar"))
        .bearer_auth("owner").header("content-type","application/json")
        .body(json!({"session":"destination","generation":generation,"organization_account":account_snapshot,"texto":"  Hoje vamos conferir o ditado completo.\n","estilo":"limpar","organization_model":if model==Some(""){json!("")}else{Value::Null},"include_recent_messages":include,"recent_messages":snapshot}).to_string()).send().await.unwrap();
    let status = response.status();
    let data = response.bytes().await.unwrap();
    server.abort();
    python.abort();
    assert_eq!(status, 200);
    let record = std::fs::read(account.join("fixture-record.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    (serde_json::from_slice::<Value>(&data).unwrap(), record)
}

#[tokio::test]
async fn dictation_harness_uses_destination_account_and_its_separate_model() {
    for provider in ["claude", "codex"] {
        let result = attempt(provider, false, None, "k:original").await;
        assert_eq!(
            result["text"], "Hoje vamos conferir o ditado completo.",
            "harness recusou a conta: {result}"
        );
        assert!(result["aviso"].is_null());
        assert_eq!(result["estilo_aplicado"], "limpar");
    }
}
#[tokio::test]
async fn dictation_harness_oauth_claude_uses_safe_mode_and_destination_subscription() {
    let (result, record) = attempt_with_budget(
        "claude",
        true,
        None,
        "k:original",
        false,
        None,
        false,
        None,
        None,
        false,
    )
    .await;
    assert_eq!(result["text"], "Hoje vamos conferir o ditado completo.");
    assert!(result["organization_code"].is_null(), "{result}");
    assert_eq!(
        record["anthropic_api_key"], false,
        "chave de API trocaria a assinatura: {record}"
    );
    assert!(
        record["config_dir"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("claude-a")),
        "pasta da conta de destino ausente: {record}"
    );
    assert_eq!(record["credentials_present"], true, "{record}");
    let args = record["args"].as_array().unwrap();
    assert!(args.iter().any(|a| a == "--safe-mode"));
    assert!(!args.iter().any(|a| a == "--system-prompt"));
}
#[tokio::test]
async fn dictation_harness_refuses_recreated_destination_and_unavailable_model() {
    let result = attempt("claude", false, None, "k:old-conversation").await;
    assert_eq!(result["organization_code"], "dictation_target_changed");
    let result = attempt("claude", false, Some("conversation-model"), "k:original").await;
    assert_eq!(result["organization_code"], "dictation_model_unavailable");
}

#[tokio::test]
async fn dictation_recent_messages_are_opt_in_bounded_and_frozen_for_revision() {
    let off = attempt_with_context("claude", false, None, "k:original", false, None, false).await;
    assert!(
        off["recent_messages"].is_null(),
        "sem opção não há histórico"
    );
    let (on, record) = attempt_with_budget(
        "claude",
        false,
        None,
        "k:original",
        true,
        None,
        false,
        None,
        None,
        false,
    )
    .await;
    let prompt = record["system_prompt"].as_str().unwrap_or_default();
    assert!(
        prompt.contains("O projeto usa PostgreSQL") && prompt.contains("O serviço se chama Hangar"),
        "as referências não chegaram ao prompt de sistema do CLI: {record}"
    );
    assert!(
        !record["args"].to_string().contains("PostgreSQL"),
        "referências da conversa no argv"
    );
    let references = on["recent_messages"]
        .as_array()
        .expect("referências solicitadas ausentes");
    assert_eq!(
        references.len(),
        3,
        "só as três últimas mensagens humanas/assistente"
    );
    assert_eq!(references[0]["text"], "O projeto usa PostgreSQL");
    assert_eq!(references[1]["text"], "O serviço se chama Hangar");
    assert_eq!(
        references[2]["text"].as_str().unwrap().chars().count(),
        2000
    );
    assert!(!on["recent_messages"].to_string().contains("Segredo"));
    let frozen = json!([{"role":"user","text":"Referência da primeira tentativa"}]);
    let revised = attempt_with_context(
        "claude",
        false,
        None,
        "k:original",
        true,
        Some(frozen.clone()),
        false,
    )
    .await;
    assert_eq!(
        revised["recent_messages"], frozen,
        "revisão reutiliza o snapshot, sem reler conversa"
    );
}

#[tokio::test]
async fn dictation_harness_does_not_read_or_use_an_account_during_login_or_removal() {
    let result = attempt_with_context("claude", false, None, "k:original", false, None, true).await;
    assert_eq!(
        result["organization_code"], "dictation_account_busy",
        "não pode organizar enquanto a conta muda: {result}"
    );
    assert_eq!(result["text"], "  Hoje vamos conferir o ditado completo.\n");
}

#[tokio::test]
async fn dictation_harness_empty_model_snapshot_does_not_use_a_later_saved_model() {
    let result = attempt("claude", false, Some(""), "k:original").await;
    assert_eq!(
        result["organization_code"], "dictation_model_unavailable",
        "modelo não escolhido no snapshot: {result}"
    );
    assert_eq!(result["text"], "  Hoje vamos conferir o ditado completo.\n");
}

#[tokio::test]
async fn dictation_deadline_includes_waiting_for_the_catalog_and_preserves_raw() {
    let result = attempt_with_budget(
        "claude",
        false,
        None,
        "k:original",
        false,
        None,
        false,
        Some(Duration::from_millis(500)),
        None,
        false,
    )
    .await
    .0;
    assert_eq!(
        result["organization_code"],
        "dictation_organization_timeout"
    );
    assert_eq!(result["text"], "  Hoje vamos conferir o ditado completo.\n");
}

#[tokio::test]
async fn dictation_public_harness_refuses_changed_account_snapshot() {
    let (result, record) = attempt_with_budget(
        "claude",
        false,
        None,
        "k:original",
        false,
        None,
        false,
        None,
        Some("other-account"),
        false,
    )
    .await;
    assert_eq!(result["organization_code"], "dictation_target_changed");
    assert_eq!(result["raw"], "  Hoje vamos conferir o ditado completo.\n");
    assert!(record.is_null(), "a conta divergente executou o CLI");
}

#[tokio::test]
async fn dictation_public_catalog_fresh_queue_keeps_sixty_second_deadline() {
    attempt_with_budget(
        "claude",
        false,
        None,
        "k:original",
        false,
        None,
        false,
        None,
        None,
        true,
    )
    .await;
}
