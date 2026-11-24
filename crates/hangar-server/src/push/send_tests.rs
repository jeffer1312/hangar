//! Casos de envio e concorrência do push.
use super::*;
use crate::orchestration::codec;

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

/// O aviso do rascunho de WhatsApp é a frase fixa: nada da conversa, do número nem do texto.
#[test]
fn the_whatsapp_draft_push_says_only_that_a_draft_waits() {
    let entry = json!({"serverId": "casa", "locale": "pt"});
    let p = Outgoing::whatsapp_draft("s0", "casa".into(), "d1".into()).payload(&entry).unwrap();
    assert_eq!(p["body"], json!("Rascunho de WhatsApp aguardando aprovação"));
    assert_eq!(p["title"], json!("claude · WhatsApp"));
    assert_eq!(p["url"], json!("/"));
    assert_eq!((&p["server_id"], &p["whatsapp_draft_id"]), (&json!("casa"), &json!("d1")), "o app abre a folha pelo aviso");
    let en = Outgoing::whatsapp_draft("s0", "casa".into(), "d1".into()).payload(&json!({"locale": "en"})).unwrap();
    assert_eq!(en["body"], json!("WhatsApp draft awaiting approval"));
}

/// O cofre abre a folha pela `url` e pelo `request_id`, com o id que a inscrição conhece para esta máquina.
#[test]
fn the_vault_push_opens_its_sheet_by_the_subscription_server() {
    let out = || Outgoing::vault_request("s0", "casa".into(), "r1".into(), "s0".into(), "GitHub".into());
    let p = out().payload(&json!({"serverId": "notebook"})).unwrap();
    assert_eq!(p["url"], json!("/?server=notebook&vault_request=r1"));
    assert_eq!((&p["server_id"], &p["request_id"]), (&json!("notebook"), &json!("r1")));
    assert!(p.get("whatsapp_draft_id").is_none());
    let unnamed = out().payload(&json!({})).unwrap();
    assert_eq!((&unnamed["server_id"], &unnamed["url"]), (&json!("casa"), &json!("/?server=casa&vault_request=r1")),
        "sem id na inscrição, o da máquina");
}
