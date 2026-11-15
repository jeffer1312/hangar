use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use hangar_dream::config::Config as DreamConfig;
use serde_json::{json, Value};

use crate::accounts::{environment::AccountEnvironment, AccountService};
use crate::auth::TrustedHosts;
use crate::config::Config;
use crate::routes::AppState;

static TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn state(home: &std::path::Path) -> Arc<AppState> {
    let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: address,
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [("HOME".into(), home.to_string_lossy().into_owned()), ("USERPROFILE".into(), home.to_string_lossy().into_owned())].into(),
    ));
    state.hangar_dir = home.join(".hangar");
    Arc::new(state)
}

async fn call(st: &Arc<AppState>, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer fixture-owner")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = super::routes::route(st.clone(), "127.0.0.1:1".parse().unwrap(), req).await;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn dreamer_is(st: &AppState, machine: &str) {
    DreamConfig { machine: machine.into(), ..DreamConfig::default() }.save(&super::Paths::of(st).config()).unwrap();
}

#[tokio::test]
async fn only_the_registered_dreamer_writes_memory_and_only_over_what_it_read() {
    let _t = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let st = state(home.path());
    dreamer_is(&st, "vps");
    let empty = hangar_dream::memory::digest(&Default::default());
    let root = home.path().join("proj").to_string_lossy().into_owned();
    let put = |from: &str, digest: &str| json!({"root": root,"files": {"MEMORY.md": "- [a](a.md) — x\n", "a.md": "fato"}, "expect_digest": digest, "from_machine": from});

    let (status, body) = call(&st, "PUT", "/api/dream/memory", put("notebook", &empty)).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["detail"]["code"], "dream_not_dreamer", "{body}");

    let (status, body) = call(&st, "PUT", "/api/dream/memory", put("vps", "velho")).await;
    assert_eq!(status, 409, "{body}");

    let (status, body) = call(&st, "PUT", "/api/dream/memory", put("vps", &empty)).await;
    assert_eq!(status, 200, "{body}");
    let dir = hangar_dream::project::memory_dir(&home.path().join(".claude"), std::path::Path::new(&root));
    assert_eq!(std::fs::read_to_string(dir.join("a.md")).unwrap(), "fato");

    let query: String = form_urlencoded::byte_serialize(root.as_bytes()).collect();
    let (status, body) = call(&st, "GET", &format!("/api/dream/memory?root={query}"), Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(body["files"]["a.md"], "fato");
}

#[tokio::test]
async fn config_masks_the_key_and_keeps_it_when_the_mask_comes_back() {
    let _t = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let st = state(home.path());
    let (status, body) = call(&st, "PUT", "/api/dream/config", json!({"api_key": "chave-de-teste-segredo-1234", "enabled": true})).await;
    assert_eq!(status, 200, "{body}");
    let masked = body["api_key"].as_str().unwrap().to_string();
    assert!(!masked.contains("segredo"));
    let (status, _) = call(&st, "PUT", "/api/dream/config", json!({"api_key": masked, "hour": "02:30"})).await;
    assert_eq!(status, 200);
    let saved = DreamConfig::load(&super::Paths::of(&st).config()).unwrap();
    assert_eq!(saved.api_key, "chave-de-teste-segredo-1234");
    assert_eq!(saved.hour, "02:30");
    let (status, body) = call(&st, "PUT", "/api/dream/config", json!({"hour": "25:00"})).await;
    assert_eq!(status, 422, "{body}");
}

#[test]
fn undo_of_the_user_file_tells_no_pass_from_unreadable_record() {
    use super::round::{USER_AFTER, USER_BEFORE};
    use super::routes::undo_user_local;
    let tmp = tempfile::tempdir().unwrap();
    let record = tmp.path().join("run");
    let target = tmp.path().join("rules").join("memoria.md");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();

    // Rodada sem a passada do usuário: nada a desfazer.
    assert_eq!(undo_user_local(&record, &target), Ok(None));

    // Registro que existe mas não se lê é falha, não "nada a desfazer".
    std::fs::create_dir_all(record.join(USER_BEFORE)).unwrap();
    std::fs::write(record.join(USER_AFTER), "depois").unwrap();
    assert_eq!(undo_user_local(&record, &target), Err("dream_io".into()));
    std::fs::remove_dir(record.join(USER_BEFORE)).unwrap();

    std::fs::write(record.join(USER_BEFORE), "antes").unwrap();
    std::fs::write(&target, "mexido depois").unwrap();
    assert_eq!(undo_user_local(&record, &target), Err("dream_memory_changed".into()));

    std::fs::write(&target, "depois").unwrap();
    assert_eq!(undo_user_local(&record, &target), Ok(Some(("antes".into(), "depois".into()))));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "antes");

    // O arquivo atual ilegível não vira vazio.
    std::fs::remove_file(&target).unwrap();
    std::fs::create_dir(&target).unwrap();
    assert_eq!(undo_user_local(&record, &target), Err("dream_io".into()));
}

#[test]
fn user_file_write_checks_what_it_expects_and_empty_removes() {
    use super::round::write_user_memory;
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("rules").join("memoria.md");

    // Ausente conta como vazio; a pasta nasce junto.
    assert_eq!(write_user_memory(&target, "nova", Some("")), Ok(()));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "nova");

    assert_eq!(write_user_memory(&target, "outra", Some("velha")), Err("dream_memory_changed".into()));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "nova");

    assert_eq!(write_user_memory(&target, "", Some("nova")), Ok(()));
    assert!(!target.exists());
    assert_eq!(write_user_memory(&target, "", None), Ok(()));
}

#[tokio::test]
async fn peer_refuses_the_user_file_undo_when_it_changed() {
    let _t = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let st = state(home.path());
    dreamer_is(&st, "vps");
    let target = home.path().join(".claude").join(super::round::USER_MEMORY_PATH);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "mexido aqui").unwrap();

    let (status, body) = call(&st, "PUT", "/api/dream/user-memory", json!({"content": "antes", "expect": "depois", "from_machine": "vps"})).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["detail"]["code"], "dream_memory_changed", "{body}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "mexido aqui");

    let (status, body) = call(&st, "PUT", "/api/dream/user-memory", json!({"content": "", "expect": "mexido aqui", "from_machine": "vps"})).await;
    assert_eq!(status, 200, "{body}");
    assert!(!target.exists());
}

#[tokio::test]
async fn state_writes_wait_for_the_round() {
    let _t = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let st = state(home.path());
    *super::RUNNING.lock().unwrap() = Some("20261114-010000-abcd".into());
    let (status, body) = call(&st, "PUT", "/api/dream/project?id=git%3Aa%2Fb", json!({"enabled": false})).await;
    *super::RUNNING.lock().unwrap() = None;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["detail"]["code"], "dream_running", "{body}");
    assert!(!super::Paths::of(&st).state().exists());
}

#[test]
fn records_of_projects_with_the_same_clean_name_do_not_overwrite_each_other() {
    use super::round::{load_records, save_record, RunRecord};
    let tmp = tempfile::tempdir().unwrap();
    let paths = super::Paths { dir: tmp.path().to_path_buf() };
    let run_id = "20261114-010000-abcd";
    for project in ["git:a/b-c", "git:a/b/c"] {
        let record = RunRecord {
            run_id: run_id.into(),
            project: project.into(),
            at: chrono::Utc::now(),
            cost_usd: 0.0,
            places: Vec::new(),
            cursors_before: Default::default(),
            last_dream_before: None,
        };
        save_record(&paths, &record).unwrap();
    }
    let mut projects: Vec<String> = load_records(&paths, run_id).unwrap().into_iter().map(|r| r.project).collect();
    projects.sort();
    assert_eq!(projects, ["git:a/b-c", "git:a/b/c"]);
}

#[tokio::test]
async fn turning_the_dream_on_waits_for_the_next_hour() {
    let _t = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let st = state(home.path());
    let Some(zone) = iana_time_zone::get_timezone().ok().and_then(|z| z.parse::<chrono_tz::Tz>().ok()) else { return };
    // O horário de um minuto atrás: contar de ontem sonharia agora.
    let hour = (chrono::Utc::now() - chrono::Duration::minutes(1)).with_timezone(&zone).format("%H:%M").to_string();
    DreamConfig { enabled: true, hour, machine: super::this_machine(&st), ..DreamConfig::default() }.save(&super::Paths::of(&st).config()).unwrap();
    let wait = super::next_wait(&st).expect("esta máquina sonha");
    assert!(wait > std::time::Duration::from_secs(22 * 3600), "{wait:?}");
}

#[test]
fn a_place_left_unwritten_holds_the_cursors_and_names_code_and_machines() {
    use super::round::{place_failure, PlaceChange};
    let place = |machine: &str, error: Option<&str>| PlaceChange {
        machine: machine.into(),
        root: "/srv/proj".into(),
        dir: None,
        before: Default::default(),
        after: Default::default(),
        written: error.is_none(),
        error: error.map(Into::into),
    };
    assert_eq!(place_failure(&[place("notebook", None), place("vps", None)]), None);
    assert_eq!(
        place_failure(&[place("notebook", None), place("vps", Some("dream_locked")), place("vps", Some("dream_io")), place("pc", Some("dream_peer_failed"))]),
        Some("dream_locked: pc, vps".into())
    );
    // Peer sem leitura da memória segura os cursores como um que não gravou.
    assert_eq!(place_failure(&[place("notebook", None), super::round::unread_place("pc", "/srv/proj")]), Some("dream_peer_failed: pc".into()));
}

#[tokio::test]
async fn run_now_is_refused_off_the_dreamer() {
    let _t = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let st = state(home.path());
    dreamer_is(&st, "vps");
    let (status, body) = call(&st, "POST", "/api/dream/run", json!({})).await;
    assert_eq!(status, 409, "{body}");
}
