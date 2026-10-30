//! Casos de envio e concorrência capturados do push original.
use super::*;
use crate::orchestration::codec;

#[test]
fn guest_push_persisted_payload_matches_original() {
    let fixture = codec::loads(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/push-send.json"
    ))
    .unwrap();
    for case in fixture["rows"].as_array().unwrap() {
        let out = if case["aggregate"] == true {
            Outgoing {
                session: "sessão".into(),
                body: Box::new(|locale| msg(locale, "aguardando").into()),
                title: Some(Box::new(|_| "2 sessões".into())),
                tag: Some("aggregate"),
                url: Some("/".into()),
            }
        } else {
            Outgoing::single("sessão", Notice::Awaiting(None))
        };
        if case.get("error").is_some() {
            assert!(out.payload(&case["entry"]).is_err(), "{case:?}");
            continue;
        }
        let expected = codec::loads(case["calls"][0]["data"].as_str().unwrap()).unwrap();
        assert_eq!(
            out.payload(&case["entry"]).unwrap(),
            expected,
            "GUE-082: {case:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guest_push_subscribe_waits_for_expired_delivery_and_survives_prune() {
    use crate::{
        accounts::{AccountService, environment::AccountEnvironment},
        auth::TrustedHosts,
        config::Config,
        routes::AppState,
    };
    use axum::{
        Router,
        body::Body,
        extract::ConnectInfo,
        http::{Request, StatusCode},
        routing::post,
    };
    use base64::Engine;
    use ring::{
        agreement,
        rand::SystemRandom,
        signature::{self, KeyPair},
    };
    use tower::ServiceExt;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/push-lock.json"
    ))
    .unwrap();
    for case in fixture["rows"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let rng = SystemRandom::new();
        let pkcs8 = signature::EcdsaKeyPair::generate_pkcs8(
            &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &rng,
        )
        .unwrap();
        let pair = signature::EcdsaKeyPair::from_pkcs8(
            &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8.as_ref(),
            &rng,
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let public = b64.encode(pair.public_key().as_ref());
        let private = b64.encode(&pkcs8.as_ref()[36..68]);
        let ua = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let key = b64.encode(ua.compute_public_key().unwrap().as_ref());
        let (started, entered) = tokio::sync::oneshot::channel();
        let started = Arc::new(Mutex::new(Some(started)));
        let release = Arc::new(tokio::sync::Notify::new());
        let wait = release.clone();
        let app = Router::new().route(
            "/e",
            post(move || {
                let started = started.clone();
                let wait = wait.clone();
                async move {
                    started.lock().unwrap().take().unwrap().send(()).unwrap();
                    wait.notified().await;
                    StatusCode::GONE
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/e", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut st = AppState::new(Config {
            listen: "127.0.0.1:1".parse().unwrap(),
            upstream: "127.0.0.1:2".parse().unwrap(),
            internal_secret: "fixture".into(),
            auth_token: "owner".into(),
            log_path: None,
            trusted: TrustedHosts::parse(""),
        });
        st.accounts = AccountService::new(AccountEnvironment::from_pairs([
            ("HOME", home.path().to_str().unwrap()),
            ("USERPROFILE", home.path().to_str().unwrap()),
            ("CP_VAPID_PUBLIC", public.as_str()),
            ("CP_VAPID_PRIVATE", private.as_str()),
        ]));
        st.share = None;
        let st = Arc::new(st);
        let sender = Sender::from_state(st.clone()).unwrap();
        let dir = sender.dir();
        std::fs::create_dir_all(&dir).unwrap();
        let old = json!({"endpoint":endpoint,"keys":{"p256dh":key,"auth":b64.encode([7u8;16])}});
        let new = if case["changed"] == true {
            json!({"endpoint":endpoint,"keys":{"p256dh":key,"auth":b64.encode([8u8;16])}})
        } else {
            old.clone()
        };
        std::fs::write(
            dir.join("subs.json"),
            serde_json::to_vec(
                &json!([{"subscription":old,"label":"Servidor","serverId":"s","locale":"pt"}]),
            )
            .unwrap(),
        )
        .unwrap();
        let delivery = tokio::spawn(async move {
            sender
                .broadcast(Outgoing::single("s", Notice::Awaiting(None)))
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        let probe = store::lock_probe(&dir);
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/push/subscribe")
            .header("authorization", "Bearer owner")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(
                    &json!({"subscription":new,"label":"Servidor","serverId":"s","locale":"pt"}),
                )
                .unwrap(),
            ))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "192.0.2.1:1234".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let writing =
            tokio::spawn(async move { crate::routes::router(st).oneshot(request).await.unwrap() });
        let blocked = tokio::time::timeout(Duration::from_secs(5), probe)
            .await
            .unwrap()
            .unwrap();
        release.notify_one();
        delivery.await.unwrap();
        let response = writing.await.unwrap();
        server.abort();
        assert_eq!(
            blocked,
            case["blocked"].as_bool().unwrap(),
            "GUE-082: reinscrição precisa aguardar o envio antigo"
        );
        assert_eq!(response.status(), StatusCode::OK);
        let saved = store::subs(&dir).unwrap();
        assert_eq!(saved.len(), case["final"].as_array().unwrap().len());
        assert_eq!(saved[0]["subscription"], new);
    }
}

#[tokio::test]
async fn guest_push_gate_uses_shared_last_good_owner_and_current_life() {
    use crate::{
        accounts::{AccountService, environment::AccountEnvironment},
        auth::TrustedHosts,
        config::Config,
        login_guests::{Service, Settings},
        routes::AppState,
        share::life::{LifeProbe, Lives},
    };
    struct Probe(Mutex<String>);
    impl LifeProbe for Probe {
        fn birth<'a>(&'a self, _: &'a str) -> futures_util::future::BoxFuture<'a, Option<u64>> {
            Box::pin(async { None })
        }
        fn has_session<'a>(
            &'a self,
            _: &'a str,
        ) -> futures_util::future::BoxFuture<'a, Option<bool>> {
            Box::pin(async { Some(true) })
        }
        fn sidecar(&self, _: &str) -> Option<(String, bool)> {
            Some((self.0.lock().unwrap().clone(), false))
        }
        fn sidecar_exists(&self, _: &str) -> bool {
            true
        }
    }
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("guests.json");
    let service = Service::new(Settings {
        guests: file.clone(),
        vault: home.path().join("vault.json"),
        runtime_config: home.path().join("runtime-config.json"),
        sync_enabled: false,
        bootstrap: String::new(),
        secret: b"fixture".to_vec(),
        secure: false,
        rate_max: 10.into(),
        rate_window: 300.into(),
    });
    service
        .store
        .create(
            "Ana",
            home.path().to_str().unwrap(),
            false,
            false,
            "ana".into(),
            "guest".into(),
        )
        .unwrap();
    service.store.claim("s", "ana", None).unwrap();
    let probe = Arc::new(Probe(Mutex::new("first-life".into())));
    let lives = Arc::new(Lives::new(probe.clone(), Arc::default()));
    assert!(service.lives.set(lives.clone()).is_ok());
    let mut state = AppState::new(Config {
        listen: "127.0.0.1:1".parse().unwrap(),
        upstream: "127.0.0.1:2".parse().unwrap(),
        internal_secret: "fixture".into(),
        auth_token: "owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse(""),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_pairs([
        ("HOME", home.path().to_str().unwrap()),
        ("USERPROFILE", home.path().to_str().unwrap()),
    ]));
    state.share = None;
    state.login_guests = Arc::new(service);
    let state = Arc::new(state);
    let sender = Sender::from_state(state).unwrap();
    assert!(
        sender.gate("s").await.hidden_guest,
        "GUE-022: o produtor usa o cadastro compartilhado"
    );
    std::fs::write(&file, b"{").unwrap();
    assert!(
        sender.gate("s").await.hidden_guest,
        "D02: leitura inválida conserva a última posse válida"
    );
    *probe.0.lock().unwrap() = "second-life".into();
    lives.life("s", true).await;
    assert!(
        !sender.gate("s").await.hidden_guest,
        "GUE-022: outra vida não herda o silêncio do convidado"
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"{");
}

mod external {
    use super::*;
    use proptest::prelude::*;
    fn document() -> impl Strategy<Value = Value> {
        prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| json!(n)),
            ".{0,80}".prop_map(Value::String)
        ]
        .prop_recursive(4, 48, 8, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
                proptest::collection::vec(
                    (
                        prop_oneof![
                            Just("endpoint".to_owned()),
                            Just("subscription".to_owned()),
                            Just("muted".to_owned()),
                            Just("quiet_hours".to_owned()),
                            Just("label".to_owned()),
                            Just("locale".to_owned()),
                            ".{0,16}"
                        ],
                        inner
                    ),
                    0..8
                )
                .prop_map(|fields| Value::Object(fields.into_iter().collect()))
            ]
        })
    }
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn guest_push_external_documents_and_sidecars_never_panic(value in document(),raw in proptest::collection::vec(any::<u8>(),0..256)) {
            let dir=tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("subs.json"),&raw).unwrap();
            let _=store::add(dir.path(),codec::clone_value(&value),"fixture","s","pt");
            std::fs::write(dir.path().join("push_prefs.json"),codec::dumps(&value,false)).unwrap();
            if let Ok(prefs)=store::prefs(dir.path()) {let _=store::is_muted(&prefs,"s");let _=store::in_quiet_hours(&prefs,chrono::NaiveTime::MIN);codec::discard(prefs);}
            let _=store::set_muted(dir.path(),"s",true);
            let _=Outgoing::single("s",Notice::Awaiting(None)).payload(&value);
            for schema in ["PushSubscribeBody","PushMuteBody","PushQuietHoursBody"] {
                match crate::login_guests::schema::validate(schema,&value) {Ok(normalized)=>codec::discard(normalized),Err(failure)=>codec::discard(failure.body)}
            }
            codec::discard(value);
        }
    }
}

#[tokio::test]
async fn guest_push_transport_status_preserves_original_pruning() {
    use axum::{Router, http::StatusCode, routing::post};
    use base64::Engine;
    use ring::{
        agreement,
        rand::SystemRandom,
        signature::{self, KeyPair},
    };
    let cases: Value = codec::loads(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/push-send.json"
    ))
    .unwrap();
    let rng = SystemRandom::new();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let pkcs8 =
        signature::EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .unwrap();
    let pair = signature::EcdsaKeyPair::from_pkcs8(
        &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8.as_ref(),
        &rng,
    )
    .unwrap();
    let env = BTreeMap::from([
        (
            "CP_VAPID_PUBLIC".into(),
            b64.encode(pair.public_key().as_ref()),
        ),
        (
            "CP_VAPID_PRIVATE".into(),
            b64.encode(&pkcs8.as_ref()[36..68]),
        ),
    ]);
    let vapid = webpush::vapid(&env).unwrap();
    let ua = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
    let key = b64.encode(ua.compute_public_key().unwrap().as_ref());
    let status = Arc::new(std::sync::atomic::AtomicU16::new(200));
    let answer = status.clone();
    let app = Router::new().route(
        "/e",
        post(move || {
            let code = answer.load(std::sync::atomic::Ordering::SeqCst);
            async move { StatusCode::from_u16(code).unwrap() }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/e", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = crate::http_client()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    for case in cases["statuses"].as_array().unwrap() {
        status.store(
            case["status"].as_u64().unwrap_or(200) as u16,
            std::sync::atomic::Ordering::SeqCst,
        );
        let endpoint = if case["status"].is_null() {
            "http://127.0.0.1:0/e"
        } else {
            &endpoint
        };
        let subscription =
            json!({"endpoint":endpoint,"keys":{"p256dh":key,"auth":b64.encode([7u8;16])}});
        let sent = webpush::send(&client, &vapid, &subscription, b"{}").await;
        assert_eq!(
            !matches!(sent, webpush::Sent::Gone),
            case["alive"].as_bool().unwrap(),
            "GUE-082: {case:?}"
        );
    }
    task.abort();
}
