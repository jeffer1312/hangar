//! Canal Expo Push: o app registra o token pelo dono, o Python manda avisar pela rota privada.
use axum::{Router, body::Bytes, extract::State, routing::post};
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// O que o servidor Expo falso recebeu, e o recibo que ele devolve.
#[derive(Default)]
struct Expo {
    calls: Mutex<Vec<(String, Value)>>,
    receipts: Mutex<Value>,
    // Ticket `DeviceNotRegistered` no `/send`, em vez de ok.
    unregistered: std::sync::atomic::AtomicBool,
    // O `/send` só responde depois deste sinal: o teste age entre a leitura e a gravação do envio.
    hold: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

async fn expo_send(State(expo): State<Arc<Expo>>, raw: Bytes) -> String {
    let body: Value = serde_json::from_slice(&raw).unwrap();
    let dead = expo.unregistered.load(std::sync::atomic::Ordering::SeqCst);
    let tickets: Vec<Value> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            if dead {
                json!({"status": "error", "details": {"error": "DeviceNotRegistered"}})
            } else {
                json!({"status": "ok", "id": format!("ticket-{}", m["to"].as_str().unwrap())})
            }
        })
        .collect();
    expo.calls.lock().unwrap().push(("send".into(), body));
    let hold = expo.hold.lock().unwrap().take();
    if let Some(hold) = hold {
        let _ = hold.await;
    }
    json!({"data": tickets}).to_string()
}

async fn expo_receipts(State(expo): State<Arc<Expo>>, raw: Bytes) -> String {
    let body: Value = serde_json::from_slice(&raw).unwrap();
    expo.calls.lock().unwrap().push(("receipts".into(), body));
    json!({"data": expo.receipts.lock().unwrap().clone()}).to_string()
}

struct Fixture {
    home: tempfile::TempDir,
    public: String,
    private: String,
    state: Arc<AppState>,
    expo: Arc<Expo>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://{address}"), task)
}

impl Fixture {
    async fn start() -> Self {
        let expo = Arc::new(Expo::default());
        let (expo_base, expo_task) = serve(
            Router::new()
                .route("/--/api/v2/push/send", post(expo_send))
                .route("/--/api/v2/push/getReceipts", post(expo_receipts))
                .with_state(expo.clone()),
        )
        .await;
        let home = tempfile::tempdir().unwrap();
        let mut state = AppState::new(Config {
            listen: "127.0.0.1:0".parse().unwrap(),
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
                ("HANGAR_EXPO_PUSH_URL".into(), expo_base),
            ]
            .into(),
        ));
        let state = Arc::new(state);
        let (public, public_task) = serve(hangar_server::routes::router(state.clone())).await;
        let (private, private_task) =
            serve(hangar_server::routes::terminal_router(state.clone())).await;
        Self {
            home,
            public,
            private,
            state,
            expo,
            tasks: vec![expo_task, public_task, private_task],
        }
    }

    async fn subscribe(
        &self,
        token: &str,
        label: &str,
        locale: &str,
        owner: bool,
    ) -> reqwest::Response {
        self.subscribe_via(SUBSCRIBE, "casa", token, label, locale, owner)
            .await
    }

    /// `FORWARDED` entra pela porta privada, como o dono pelo Connect.
    async fn subscribe_via(
        &self,
        path: &str,
        server_id: &str,
        token: &str,
        label: &str,
        locale: &str,
        owner: bool,
    ) -> reqwest::Response {
        let client = reqwest::Client::new();
        let mut request = if path == FORWARDED {
            client
                .post(format!("{}{path}", self.private))
                .header("x-hangar-internal", "fixture-internal")
                .header("x-hangar-path", SUBSCRIBE)
        } else {
            client.post(format!("{}{path}", self.public))
        };
        if owner {
            request = request.bearer_auth("fixture-owner");
        }
        request
            .header("content-type", "application/json")
            .body(
                json!({"token": token, "server_id": server_id, "label": label, "locale": locale})
                    .to_string(),
            )
            .send()
            .await
            .unwrap()
    }

    async fn notify(&self, list: bool) {
        use hangar_server::push::send::{Outgoing, Sender};
        let sender = Sender::from_state(self.state.clone()).unwrap();
        sender
            .broadcast(Outgoing {
                session: "s1".into(),
                body: Box::new(|locale| {
                    if locale == "en" {
                        "done".into()
                    } else {
                        "pronto".into()
                    }
                }),
                title: Some(Box::new(|locale| {
                    if locale == "en" {
                        "s1 en".into()
                    } else {
                        "s1".into()
                    }
                })),
                tag: None,
                url: list.then(|| "/".into()),
            })
            .await;
    }

    fn registry(&self) -> Value {
        serde_json::from_slice(
            &std::fs::read(self.home.path().join(".hangar/expo-push-tokens.json")).unwrap(),
        )
        .unwrap()
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.expo.calls.lock().unwrap().clone()
    }
}

async fn wait_until<T: std::fmt::Debug>(read: impl Fn() -> T, done: impl Fn(&T) -> bool) -> T {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let value = read();
        if done(&value) {
            return value;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "não chegou ao esperado: {value:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn body(response: reqwest::Response) -> Value {
    serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
}

const TOKEN: &str = "ExponentPushToken[aaa]";
const SUBSCRIBE: &str = "/api/push/expo/subscribe";
const FORWARDED: &str = "/__hangar_server/forwarded";

#[tokio::test]
async fn resubscribing_the_same_token_keeps_one_entry() {
    let fixture = Fixture::start().await;
    assert_eq!(
        fixture.subscribe(TOKEN, "Velho", "pt", true).await.status(),
        200
    );
    let response = fixture.subscribe(TOKEN, "Casa", "en", true).await;
    assert_eq!(response.status(), 200);
    assert_eq!(body(response).await, json!({"ok": true}));
    let tokens = fixture.registry()["tokens"].clone();
    assert_eq!(tokens.as_array().unwrap().len(), 1);
    assert_eq!(tokens[0]["label"], "Casa");
    assert_eq!(tokens[0]["locale"], "en");
}

#[tokio::test]
async fn invalid_token_is_refused() {
    let fixture = Fixture::start().await;
    let response = fixture.subscribe("abc", "Casa", "pt", true).await;
    assert_eq!(response.status(), 400);
    assert_eq!(body(response).await["detail"]["code"], "push_expo_invalid");
}

#[tokio::test]
async fn guest_cannot_subscribe() {
    let fixture = Fixture::start().await;
    let response = fixture.subscribe(TOKEN, "Casa", "pt", false).await;
    assert_eq!(response.status(), 403);
    assert_eq!(
        body(response).await["detail"]["code"],
        "push_expo_owner_only"
    );
}

#[tokio::test]
async fn session_notice_uses_label_locale_and_session_data() {
    let fixture = Fixture::start().await;
    fixture.subscribe(TOKEN, "Casa", "pt", true).await;
    fixture.notify(false).await;
    let calls = wait_until(|| fixture.calls(), |c| c.len() >= 1).await;
    assert_eq!(calls[0].0, "send");
    let message = &calls[0].1[0];
    assert_eq!(message["to"], TOKEN);
    assert_eq!(message["title"], "Casa · s1");
    assert_eq!(message["body"], "pronto");
    assert_eq!(message["data"], json!({"server": "casa", "session": "s1"}));
    assert_eq!(message["channelId"], "default");
    assert_eq!(message["priority"], "high");
}

#[tokio::test]
async fn list_notice_only_carries_the_server() {
    let fixture = Fixture::start().await;
    fixture.subscribe(TOKEN, "Casa", "en", true).await;
    fixture.notify(true).await;
    let calls = wait_until(|| fixture.calls(), |c| c.len() >= 1).await;
    assert_eq!(calls[0].1[0]["title"], "Casa · s1 en");
    assert_eq!(calls[0].1[0]["data"], json!({"server": "casa"}));
}

#[tokio::test]
async fn unregistered_device_receipt_removes_the_token_on_the_next_send() {
    let fixture = Fixture::start().await;
    fixture.subscribe(TOKEN, "Casa", "pt", true).await;
    fixture.notify(false).await;
    wait_until(|| fixture.calls(), |c| c.len() >= 1).await;
    wait_until(
        || fixture.registry(),
        |r| r["pending"].as_array().is_some_and(|p| !p.is_empty()),
    )
    .await;
    *fixture.expo.receipts.lock().unwrap() = json!({
        format!("ticket-{TOKEN}"): {"status": "error", "details": {"error": "DeviceNotRegistered"}}
    });
    fixture.notify(false).await;
    let calls = wait_until(|| fixture.calls(), |c| c.len() >= 2).await;
    assert_eq!(calls[1].0, "receipts");
    assert_eq!(calls[1].1, json!({"ids": [format!("ticket-{TOKEN}")]}));
    let registry = wait_until(
        || fixture.registry(),
        |r| r["tokens"].as_array().is_some_and(Vec::is_empty),
    )
    .await;
    assert_eq!(registry["pending"], json!([]));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.calls().len(), 2, "sem token vivo não há envio");
}

#[tokio::test]
async fn no_tokens_means_no_call() {
    let fixture = Fixture::start().await;
    fixture.notify(false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(fixture.calls().is_empty());
}

#[tokio::test]
async fn two_tokens_go_in_one_send() {
    let fixture = Fixture::start().await;
    fixture.subscribe(TOKEN, "Casa", "pt", true).await;
    fixture
        .subscribe("ExponentPushToken[bbb]", "Casa", "pt", true)
        .await;
    fixture.notify(false).await;
    let calls = wait_until(|| fixture.calls(), |c| c.len() >= 1).await;
    assert_eq!(calls[0].1.as_array().unwrap().len(), 2);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.calls().len(), 1);
}

#[tokio::test]
async fn more_than_a_hundred_tokens_go_in_batches() {
    let fixture = Fixture::start().await;
    for i in 0..101 {
        fixture
            .subscribe(&format!("ExponentPushToken[t{i}]"), "Casa", "pt", true)
            .await;
    }
    fixture.notify(false).await;
    let calls = wait_until(|| fixture.calls(), |c| c.len() >= 2).await;
    let sizes: Vec<usize> = calls
        .iter()
        .map(|c| c.1.as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, [100, 1]);
}

#[tokio::test]
async fn subscribe_during_a_send_survives_its_write() {
    let fixture = Fixture::start().await;
    fixture.subscribe(TOKEN, "Casa", "pt", true).await;
    let (release, hold) = tokio::sync::oneshot::channel();
    *fixture.expo.hold.lock().unwrap() = Some(hold);
    fixture.notify(false).await;
    wait_until(|| fixture.calls(), |c| c.len() >= 1).await;
    assert_eq!(
        fixture
            .subscribe("ExponentPushToken[novo]", "Casa", "pt", true)
            .await
            .status(),
        200
    );
    release.send(()).unwrap();
    let registry = wait_until(
        || fixture.registry(),
        |r| r["pending"].as_array().is_some_and(|p| !p.is_empty()),
    )
    .await;
    let tokens: Vec<&str> = registry["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["token"].as_str().unwrap())
        .collect();
    assert_eq!(tokens, [TOKEN, "ExponentPushToken[novo]"]);
}

#[tokio::test]
async fn unregistered_ticket_prunes_right_away() {
    let fixture = Fixture::start().await;
    fixture.subscribe(TOKEN, "Casa", "pt", true).await;
    fixture
        .expo
        .unregistered
        .store(true, std::sync::atomic::Ordering::SeqCst);
    fixture.notify(false).await;
    let registry = wait_until(
        || fixture.registry(),
        |r| r["tokens"].as_array().is_some_and(Vec::is_empty),
    )
    .await;
    assert_eq!(registry["pending"], json!([]));
    assert_eq!(fixture.calls().len(), 1);
}

#[tokio::test]
async fn retired_python_push_bridge_is_absent_even_with_internal_secret() {
    let fixture = Fixture::start().await;
    for (method, path) in [
        (reqwest::Method::POST, "/__hangar_server/push/expo"),
        (reqwest::Method::GET, "/__hangar_server/presence"),
    ] {
        let response = reqwest::Client::new()
            .request(method, format!("{}{path}", fixture.private))
            .header("x-hangar-internal", "fixture-internal")
            .header("content-type", "application/json")
            .body(
                json!({"session":"s1","title":{"pt":"a","en":"a"},"body":{"pt":"b","en":"b"}})
                    .to_string(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "ponte retirada: {path}");
    }
}

#[tokio::test]
async fn empty_server_or_label_is_refused() {
    let fixture = Fixture::start().await;
    assert_eq!(
        fixture.subscribe(TOKEN, " ", "pt", true).await.status(),
        400
    );
    let response = fixture
        .subscribe_via(SUBSCRIBE, "", TOKEN, "Casa", "pt", true)
        .await;
    assert_eq!(response.status(), 400);
    assert_eq!(body(response).await["detail"]["code"], "push_expo_invalid");
}

#[tokio::test]
async fn connect_owner_subscribes_through_the_forwarder() {
    let fixture = Fixture::start().await;
    let response = fixture
        .subscribe_via(FORWARDED, "casa", TOKEN, "Casa", "pt", false)
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(fixture.registry()["tokens"][0]["token"], TOKEN);
}
