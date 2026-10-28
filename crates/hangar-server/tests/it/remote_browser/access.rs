use std::time::{Duration, Instant};

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use hangar_server::auth::Auth;
use hangar_server::remote_browser::access::{self, AccessError};
use proptest::prelude::*;
use super::support::contract;

const OWNER: &str = "remote-browser-contract-owner";
const IP: &str = "198.51.100.2";
const GOOD: Option<&str> = Some("token=remote-browser-contract-owner");

fn status(result: Result<(), AccessError>) -> u16 {
    match result { Ok(()) => 200, Err(AccessError::Unauthorized) => 401, Err(AccessError::Blocked) => 429 }
}

fn bad(auth: &Auth, ip: &str, count: usize, now: Instant) {
    for _ in 0..count {
        assert_eq!(access::websocket(auth, ip, Some("token=wrong"), false, now), Err(AccessError::Unauthorized));
    }
}

// Quebra se o GET deixar de conferir precedência/cookies ou contar os oito palpites.
#[test]
fn http_matches_frozen_credentials_and_rate_limit() {
    let corpus = contract("url-commands");
    for case in corpus["cases"].as_array().unwrap().iter().filter(|c|
        c["name"].as_str().unwrap().starts_with("get_auth_") || c["name"] == "get_native_top_level_url") {
        let input = &case["inputs"];
        let auth = Auth::new("contract-owner-token");
        let mut headers = HeaderMap::new();
        for (name, value) in input["headers"].as_object().unwrap() {
            headers.insert(HeaderName::from_bytes(name.as_bytes()).unwrap(), HeaderValue::from_str(value.as_str().unwrap()).unwrap());
        }
        let query = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(input["query"].as_object().unwrap().iter().map(|(k,v)| (k.as_str(),v.as_str().unwrap()))).finish();
        let now = Instant::now();
        let got: Vec<_> = (0..input["count"].as_u64().unwrap()).map(|_| status(access::http(
            &auth, input["client_host"].as_str().unwrap(), &headers, Some(&query), input["scheme"] == "https", now))).collect();
        let want: Vec<_> = case["expected"]["responses"].as_array().unwrap().iter().map(|r| r["status"].as_u64().unwrap() as u16).collect();
        assert_eq!(got, want, "{}", case["name"]);
    }
}

// Quebra se WS usar credencial ambiente, aceitar palpite durante bloqueio ou limpar no acerto.
#[test]
fn websocket_matches_frozen_auth_sequences() {
    let corpus = contract("transport");
    for case in corpus["cases"].as_array().unwrap().iter().filter(|c| c["name"].as_str().unwrap().starts_with("auth_")) {
        let input = &case["inputs"];
        let auth = Auth::new(OWNER);
        let now = Instant::now();
        let got: Vec<_> = input["token_sequence"].as_array().unwrap().iter().map(|token| {
            let query = token.as_str().map(|token| form_urlencoded::Serializer::new(String::new()).append_pair("token", token).finish());
            access::websocket(&auth, input["client_ip"].as_str().unwrap(), query.as_deref(), false, now).is_ok()
        }).collect();
        // O Python só chega à recusa de sessão inexistente depois de autorizar.
        let want: Vec<_> = case["expected"]["closes"].as_array().unwrap().iter().map(|c| c["reason"] == "sessao nao existe").collect();
        assert_eq!(got, want, "{}", case["name"]);
    }
}

#[test]
fn websocket_success_keeps_failures() {
    let auth = Auth::new(OWNER);
    let now = Instant::now();
    bad(&auth, IP, 7, now);
    assert_eq!(access::websocket(&auth, IP, GOOD, false, now), Ok(()));
    bad(&auth, IP, 1, now);
    assert_eq!(access::websocket(&auth, IP, GOOD, false, now), Err(AccessError::Blocked));
}

#[test]
fn http_success_clears_failures_and_shares_the_websocket_counter() {
    let auth = Auth::new(OWNER);
    let now = Instant::now();
    bad(&auth, IP, 7, now);
    assert_eq!(access::http(&auth, IP, &HeaderMap::new(), GOOD, false, now), Ok(()));
    bad(&auth, IP, 1, now);
    assert_eq!(access::websocket(&auth, IP, GOOD, false, now), Ok(()));
}

#[test]
fn blocked_precedes_guest_and_token_until_the_exact_window_end() {
    let auth = Auth::new(OWNER);
    let now = Instant::now();
    bad(&auth, IP, 8, now);
    assert_eq!(access::websocket(&auth, IP, GOOD, true, now + Duration::from_millis(29_999)), Err(AccessError::Blocked));
    assert_eq!(access::websocket(&auth, IP, None, true, now + Duration::from_secs(30)), Ok(()));
    assert_eq!(access::websocket(&auth, IP, GOOD, false, now + Duration::from_secs(30)), Ok(()));
}

#[test]
fn websocket_missing_token_counts_but_loopback_does_not() {
    let now = Instant::now();
    for ip in [IP, "127.0.0.1", "::1", "localhost"] {
        let auth = Auth::new(OWNER);
        for _ in 0..8 {
            assert_eq!(access::websocket(&auth, ip, None, false, now), Err(AccessError::Unauthorized));
        }
        let want = if ip == IP { Err(AccessError::Blocked) } else { Ok(()) };
        assert_eq!(access::websocket(&auth, ip, GOOD, false, now), want, "{ip}");
    }
}

#[test]
fn connect_http_missing_token_does_not_count_and_success_does_not_clear() {
    let auth = Auth::new(OWNER);
    let now = Instant::now();
    let headers = HeaderMap::new();
    for _ in 0..12 {
        assert_eq!(access::http(&auth, "192.0.2.1", &headers, None, true, now), Err(AccessError::Unauthorized));
    }
    bad(&auth, "192.0.2.1", 7, now);
    assert_eq!(access::http(&auth, "192.0.2.1", &headers, GOOD, true, now), Ok(()));
    bad(&auth, "192.0.2.1", 1, now);
    assert_eq!(access::http(&auth, "192.0.2.1", &headers, GOOD, true, now), Err(AccessError::Blocked));
}

#[test]
fn origin_cap_evicts_the_oldest_recent_failure() {
    let auth = Auth::new(OWNER);
    let now = Instant::now();
    bad(&auth, IP, 8, now);
    assert_eq!(access::websocket(&auth, IP, GOOD, false, now), Err(AccessError::Blocked));
    for i in 0..512 {
        bad(&auth, &format!("203.0.{}.{}", i / 256, i % 256), 1, now + Duration::from_millis(i + 1));
    }
    assert_eq!(access::websocket(&auth, IP, GOOD, false, now + Duration::from_secs(1)), Ok(()));
}

#[test]
fn legacy_owner_success_still_clears_and_failures_still_require_recording() {
    let auth = Auth::new(OWNER);
    for _ in 0..12 { assert!(!auth.is_owner(IP, Some(b"wrong"))); }
    assert!(auth.is_owner(IP, Some(OWNER.as_bytes())));
    for _ in 0..7 { auth.record_fail(IP); }
    assert!(auth.is_owner(IP, Some(OWNER.as_bytes())));
    auth.record_fail(IP);
    assert!(auth.is_owner(IP, Some(OWNER.as_bytes())));
    auth.mark_blocked(IP);
    assert!(!auth.is_owner(IP, Some(OWNER.as_bytes())));
}

#[test]
fn empty_server_token_never_authorizes_a_websocket() {
    let auth = Auth::new("");
    assert_eq!(access::websocket(&auth, IP, Some("token="), false, Instant::now()), Err(AccessError::Unauthorized));
}

#[cfg(unix)]
#[tokio::test]
async fn real_discovery_owner_guest_and_external_resources_close() {
    use super::support::{self, Scene, RefusingServer};
    use hangar_server::share::store::Kind;

    let test = crate::exact_name(module_path!(), "real_discovery_owner_guest_and_external_resources_close");
    if std::env::var_os("REMOTE_BROWSER_AMBIENT_PROBE").is_some() {
        if support::isolated(&test, |root| assert!(!root.join("git.calls").exists(), "atividade Git depois da descoberta")).await { return; }
    } else if std::env::var_os("HANGAR_REMOTE_BROWSER_TEST").is_none() {
        let ambient = tempfile::tempdir().unwrap();
        let home = ambient.path().join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let shares = ambient.path().join("shares.json");
        std::fs::write(&shares, "ambient-invalid-shares").unwrap();
        let bin = support::git_probe(ambient.path());
        let dirs = serde_json::json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
            "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")});
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", &test, "--nocapture"]).env_clear().current_dir(ambient.path())
            .kill_on_drop(true)
            .env("HOME", &home).env("USERPROFILE", &home).env("XDG_CACHE_HOME", home.join("cache"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HANGAR_LIST_DIRS", dirs.to_string()).env("HANGAR_SHARES_FILE", &shares)
            .env("CP_CLAUDE_CONFIG_DIRS", home.join(".claude")).env("REMOTE_BROWSER_AMBIENT_PROBE", "1");
        let output = tokio::time::timeout(Duration::from_secs(30), command.output()).await
            .expect("sonda do ambiente não encerrou").unwrap();
        crate::assert_ran_one(&output.stdout);
        let untouched = std::fs::read_to_string(&shares).ok().as_deref() == Some("ambient-invalid-shares");
        let no_git = !ambient.path().join("git.calls").exists();
        assert_eq!((untouched, no_git), (true, true), "estado alheio preservado / Git ausente; filho: {} {}",
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "filho falhou: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let scene = Scene::new().await;
    std::fs::create_dir_all(scene.root.path().join(".git")).unwrap();
    std::fs::create_dir_all(scene.root.path().join("w0/.git")).unwrap();
    assert_eq!(scene.state.origins_home, scene.home);
    let (row, _) = scene.state.list.discovered_session("headless").await.unwrap().unwrap();
    let info = hangar_server::locate::info_of(&row, &scene.home.join(".claude"));
    assert_eq!(info.session_key, "headless-transcript");
    assert_eq!(info.provider, "claude-headless");
    assert!(scene.state.list.discovered_session("s0").await.unwrap().is_some());
    assert!(scene.state.list.discovered_session("absent").await.unwrap().is_none());
    let client = reqwest::Client::new();
    let owner = client.get(format!("http://{}/api/session-defaults?provider=claude", scene.owner.address)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(owner.status(), 200);
    assert_eq!(owner.json::<serde_json::Value>().await.unwrap()["configured"], false);
    let token = scene.guest_token("headless", Kind::Share).await;
    let guest = client.get(format!("http://{}/api/session-defaults?provider=claude", scene.guest.address)).bearer_auth(token).send().await.unwrap();
    assert_eq!(guest.status(), 403);
    scene.upstream.assert_no_browser_or_discovery_calls();

    let desktop = RefusingServer::new().await;
    let response = client.post(format!("http://{}/cmd", desktop.server.address)).body("layout").send().await.unwrap();
    assert_eq!(response.status(), 503);
    let refused = tokio_tungstenite::connect_async(format!("ws://{}/cdp", desktop.server.address)).await;
    assert!(matches!(refused, Err(tokio_tungstenite::tungstenite::Error::Http(ref r)) if r.status() == 503));
    assert_eq!(desktop.paths(), ["/cmd", "/cdp"]);
    let jpeg = super::support::frozen_jpeg();
    assert!(jpeg.starts_with(&[0xff, 0xd8]) && jpeg.ends_with(&[0xff, 0xd9]));
    // Nem conexão HTTP ociosa nem tarefa do servidor sobrevivem ao ensaio.
    drop(client);
    desktop.server.close().await;
    scene.close().await;
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn arbitrary_query_never_panics_or_accepts_a_different_token(token in ".{0,256}", raw in ".{0,512}") {
        let query = form_urlencoded::Serializer::new(String::new()).append_pair("token", &token).finish();
        let result = access::websocket(&Auth::new(OWNER), IP, Some(&query), false, Instant::now());
        prop_assert_eq!(result.is_ok(), token == OWNER);
        let _ = access::websocket(&Auth::new(OWNER), IP, Some(&raw), false, Instant::now());
        let _ = access::http(&Auth::new(OWNER), IP, &HeaderMap::new(), Some(&raw), true, Instant::now());
    }
}
