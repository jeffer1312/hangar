use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use hangar_server::share::life::{sweep, verdict, LifeFacts, LifeProbe, Lives, MuxProbe, Verdict};
use hangar_server::share::store::{Guest, Kind, Share, ShareStore};

/// Multiplexador e sidecars falsos: cada campo é o que o tmux/disco responderia agora.
#[derive(Default)]
struct Fake {
    keys: Mutex<HashMap<String, String>>,
    births: Mutex<HashMap<String, u64>>,
    has: Mutex<Option<bool>>,
    sidecar: Mutex<bool>,
    transfer: Mutex<bool>,
    /// Roda no meio da consulta ao multiplexador: o que o Python faz enquanto o Rust pergunta.
    on_has: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Fake {
    fn key(&self, name: &str, key: &str) {
        self.keys.lock().unwrap().insert(name.into(), key.into());
    }
    fn birth_of(&self, name: &str, birth: u64) {
        self.births.lock().unwrap().insert(name.into(), birth);
    }
    fn mux_says(&self, has: Option<bool>) {
        *self.has.lock().unwrap() = has;
    }
}

impl LifeProbe for Fake {
    fn birth<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<u64>> {
        let b = self.births.lock().unwrap().get(name).copied();
        Box::pin(async move { b })
    }
    fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> {
        if let Some(hook) = self.on_has.lock().unwrap().take() {
            hook();
        }
        let h = *self.has.lock().unwrap();
        Box::pin(async move { h })
    }
    fn sidecar(&self, name: &str) -> Option<(String, bool)> {
        let key = self.keys.lock().unwrap().get(name).cloned()?;
        Some((key, *self.transfer.lock().unwrap()))
    }
    fn sidecar_exists(&self, _name: &str) -> bool {
        *self.sidecar.lock().unwrap()
    }
}

fn setup(snapshot: bool) -> (Arc<Fake>, Arc<LifeFacts>, Lives) {
    let fake = Arc::new(Fake::default());
    let facts = Arc::new(LifeFacts::default());
    if snapshot {
        facts.apply_transfer_snapshot(HashMap::new(), HashSet::new());
    }
    let lives = Lives::new(fake.clone(), facts.clone());
    (fake, facts, lives)
}

fn share(session: &str, life: &str) -> Share {
    Share {
        id: format!("id-{session}"), session: session.into(), life: Some(life.into()), created_at: 0.0,
        code_expires_at: 0.0, code_hash: "c".into(), token_hash: Some("t".into()), device: None,
        redeemed_at: Some(1.0), revoked_at: None, local: false, kind: Kind::Share, parent_id: None, level: None,
    }
}

fn guest(records: Vec<Share>) -> Guest {
    Guest { token_hash: "t".into(), records, invite: false }
}

fn cc() -> Guest {
    guest(vec![share("cc", "k:L1")])
}

#[tokio::test]
async fn same_life_is_ok_and_revoked_is_ended() {
    let (fake, _, lives) = setup(true);
    fake.key("cc", "L1");
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ok);
    let mut revoked = share("cc", "k:L1");
    revoked.revoked_at = Some(2.0);
    assert_eq!(verdict(&guest(vec![revoked]), Some("cc"), &lives).await, Verdict::Ended);
    assert_eq!(verdict(&cc(), Some("outra"), &lives).await, Verdict::Ended, "sem registro da sessão");
}

#[tokio::test]
async fn recreated_session_with_same_name_is_ended() {
    let (fake, _, lives) = setup(true);
    fake.key("cc", "OUTRA-VIDA");
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ended);
}

#[tokio::test]
async fn cached_life_from_before_mode_change_is_not_ended() {
    let (fake, _, lives) = setup(true);
    fake.key("cc", "L0");
    assert_eq!(lives.life("cc", false).await.as_deref(), Some("k:L0"));
    fake.key("cc", "L1");
    assert_eq!(lives.life("cc", false).await.as_deref(), Some("k:L0"), "cache de 2 s");
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ok);
}

#[tokio::test]
async fn really_different_life_still_ends() {
    let (fake, _, lives) = setup(true);
    fake.key("cc", "L0");
    lives.life("cc", false).await;
    fake.key("cc", "OUTRA-VIDA");
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ended);
}

#[tokio::test]
async fn mode_change_is_unsure_not_ended() {
    let (fake, facts, lives) = setup(true);
    fake.key("cc", "OUTRA-VIDA");
    facts.begin_mode_change("cc");
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Unsure);
    facts.end_mode_change("cc");
    fake.key("cc", "L1");
    assert_eq!(lives.life("cc", true).await.as_deref(), Some("k:L1"));
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ok);
}

#[tokio::test]
async fn mux_without_answer_is_unsure_and_confirmed_absence_ends() {
    let (fake, _, lives) = setup(true);
    fake.mux_says(None);
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Unsure);
    assert!(!lives.confirmed_absent("cc").await);
    fake.mux_says(Some(false));
    assert!(lives.confirmed_absent("cc").await);
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ended);
    *fake.sidecar.lock().unwrap() = true;
    assert!(!lives.confirmed_absent("cc").await, "sidecar que sobrou não é ausência");
}

#[tokio::test]
async fn transfer_life_pushed_by_python_wins() {
    let (fake, facts, lives) = setup(true);
    // Transferência em curso: terminal e sidecar sumiram, mas a vida de origem continua valendo.
    fake.mux_says(Some(false));
    facts.set_life("cc", Some("k:L1".into()));
    assert_eq!(lives.life("cc", false).await.as_deref(), Some("k:L1"));
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ok);
    facts.set_life("cc", None);
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ended);
}

#[tokio::test]
async fn without_snapshot_nothing_but_revoke_ends() {
    let (fake, _, lives) = setup(false);
    fake.key("cc", "OUTRA-VIDA");
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Unsure);
    fake.keys.lock().unwrap().clear();
    fake.mux_says(Some(false));
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Unsure);
}

#[tokio::test]
async fn sessionless_route_ok_if_any_record_ok() {
    let (fake, _, lives) = setup(true);
    fake.key("cc", "L1");
    fake.key("dd", "OUTRA");
    fake.mux_says(Some(true));
    let g = guest(vec![share("dd", "k:L9"), share("cc", "k:L1")]);
    assert_eq!(verdict(&g, None, &lives).await, Verdict::Ok);
    let g = guest(vec![share("dd", "k:L9"), share("ee", "k:L1")]);
    assert_eq!(verdict(&g, None, &lives).await, Verdict::Unsure, "ee: tmux vivo sem vida legível");
    assert_eq!(verdict(&guest(vec![share("dd", "k:L9")]), None, &lives).await, Verdict::Ended);
}

#[tokio::test]
async fn stale_transfer_life_loses_to_a_new_tmux_birth() {
    // Rust reiniciado com transferência incompleta na fotografia; a sessão renasceu com o mesmo nome.
    let (fake, facts, lives) = setup(false);
    facts.apply_transfer_snapshot(HashMap::from([("cc".to_string(), "k:L1".to_string())]), HashSet::new());
    fake.birth_of("cc", 99);
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ended);
}

#[tokio::test]
async fn transfer_life_wins_only_over_the_sidecar_of_the_transfer() {
    let (fake, facts, lives) = setup(true);
    facts.set_life("cc", Some("k:L1".into()));
    fake.key("cc", "DESTINO");
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ended, "sidecar sem transfer_id é outra vida");
    *fake.transfer.lock().unwrap() = true;
    assert_eq!(lives.life("cc", true).await.as_deref(), Some("k:L1"));
    assert_eq!(verdict(&cc(), Some("cc"), &lives).await, Verdict::Ok);
}

#[tokio::test]
async fn verdict_rereads_the_record_before_ending() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ShareStore::open(dir.path().join("shares.json"), Arc::new(|| 1000.0)));
    let (_, token) = store.create_redeemed("cc", Some("k:L1".into()), Kind::Share).unwrap();
    let fake = Arc::new(Fake::default());
    let facts = Arc::new(LifeFacts::default());
    facts.apply_transfer_snapshot(HashMap::new(), HashSet::new());
    let reread = store.clone();
    let lives = Lives::new(fake.clone(), facts).rereading(move |hash| reread.lookup_hash(hash));
    let stale = store.lookup_token(&token).unwrap();
    // A troca de modo move o convite entre a leitura do registro e a decisão.
    store.set_life("cc", Some("k:L2".into())).unwrap();
    fake.key("cc", "L2");
    assert_eq!(verdict(&stale, Some("cc"), &lives).await, Verdict::Ok);
    fake.key("cc", "OUTRA");
    assert_eq!(verdict(&stale, Some("cc"), &lives).await, Verdict::Ended);
}

// --- varredura (`share_api._alive`) ---

fn store() -> (tempfile::TempDir, ShareStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ShareStore::open(dir.path().join("shares.json"), Arc::new(|| 1000.0));
    store.create("proj", Some("t:1".into()), Kind::Share, false).unwrap();
    (dir, store)
}

#[tokio::test]
async fn sweep_revokes_reborn_session() {
    let (_d, store) = store();
    let (fake, _, lives) = setup(true);
    fake.birth_of("proj", 2);
    assert_eq!(sweep(&store, &lives).await.unwrap(), 1);
    assert!(!store.has_active(false));
}

#[tokio::test]
async fn sweep_unconfirmed_absence_does_not_revoke() {
    let (_d, store) = store();
    let (fake, _, lives) = setup(true);
    fake.mux_says(None);
    assert_eq!(sweep(&store, &lives).await.unwrap(), 0);
    assert!(store.has_active(false));
}

#[tokio::test]
async fn sweep_live_mux_with_unreadable_life_does_not_revoke() {
    let (_d, store) = store();
    let (fake, _, lives) = setup(true);
    fake.mux_says(Some(true));
    assert_eq!(sweep(&store, &lives).await.unwrap(), 0);
}

#[tokio::test]
async fn sweep_leftover_sidecar_does_not_revoke() {
    let (_d, store) = store();
    let (fake, _, lives) = setup(true);
    fake.mux_says(Some(false));
    *fake.sidecar.lock().unwrap() = true;
    assert_eq!(sweep(&store, &lives).await.unwrap(), 0);
}

#[tokio::test]
async fn sweep_confirmed_absence_revokes() {
    let (_d, store) = store();
    let (fake, _, lives) = setup(true);
    fake.mux_says(Some(false));
    assert_eq!(sweep(&store, &lives).await.unwrap(), 1);
    assert!(!store.has_active(false));
}

#[tokio::test]
async fn sweep_skips_session_changing_mode() {
    let (_d, store) = store();
    let (fake, facts, lives) = setup(true);
    fake.key("proj", "novo");
    facts.begin_mode_change("proj");
    assert_eq!(sweep(&store, &lives).await.unwrap(), 0);
    assert!(store.has_active(false));
    facts.end_mode_change("proj");
    assert_eq!(sweep(&store, &lives).await.unwrap(), 1);
}

#[tokio::test]
async fn sweep_mode_change_finishing_mid_probe_does_not_revoke() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ShareStore::open(dir.path().join("shares.json"), Arc::new(|| 1000.0)));
    store.create("proj", Some("t:1".into()), Kind::Share, false).unwrap();
    let (fake, facts, lives) = setup(true);
    facts.begin_mode_change("proj");
    fake.mux_says(Some(false));
    let (s, f) = (store.clone(), facts.clone());
    *fake.on_has.lock().unwrap() = Some(Box::new(move || {
        s.set_life("proj", Some("k:novo".into())).unwrap();
        f.end_mode_change("proj");
    }));
    assert_eq!(sweep(&store, &lives).await.unwrap(), 0);
    assert!(store.has_active(false));
}

#[tokio::test]
async fn sweep_without_snapshot_revokes_nothing() {
    let (_d, store) = store();
    let (fake, _, lives) = setup(false);
    fake.birth_of("proj", 2);
    assert_eq!(sweep(&store, &lives).await.unwrap(), 0);
    fake.births.lock().unwrap().clear();
    fake.mux_says(Some(false));
    assert_eq!(sweep(&store, &lives).await.unwrap(), 0);
    assert!(store.has_active(false));
}

// --- vida lida do tmux e dos sidecars (`share_life.session_life`) ---

#[cfg(unix)]
fn mux_lives(body: &str) -> (tempfile::TempDir, Lives) {
    let dir = tempfile::tempdir().unwrap();
    let tmux = dir.path().join("tmux");
    crate::write_executable(&tmux, format!("#!/bin/sh\necho \"$@\" >> '{}'\n{body}\n", dir.path().join("log").display()));
    let probe = MuxProbe::new(tmux.into_os_string(), dir.path().to_path_buf());
    (dir, Lives::new(Arc::new(probe), Arc::new(LifeFacts::default())))
}

#[cfg(unix)]
#[tokio::test]
async fn session_life_prefers_sidecar_key() {
    let (dir, lives) = mux_lives("[ \"$4\" = '=t:' ] && echo 1700000000 && exit 0\nexit 1");
    let headless = dir.path().join(".hangar/claude-headless");
    let codex = dir.path().join(".hangar/codex-sessions");
    std::fs::create_dir_all(&headless).unwrap();
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(headless.join("h.json"), r#"{"name":"h","key":"abc"}"#).unwrap();
    std::fs::write(codex.join("c.json"), r#"{"name":"c","key":"cdx"}"#).unwrap();
    std::fs::write(codex.join("t.json"), r#"{"name":"t"}"#).unwrap();
    assert_eq!(lives.life("h", false).await.as_deref(), Some("k:abc"));
    assert_eq!(lives.life("c", false).await.as_deref(), Some("k:cdx"));
    assert_eq!(lives.life("t", false).await.as_deref(), Some("t:1700000000"));
    assert_eq!(lives.life("nada", false).await, None);
    // tmux respondeu que não há a sessão, mas o sidecar do Codex segura a ausência.
    assert!(!lives.confirmed_absent("t").await);
    assert!(lives.confirmed_absent("nada").await);
}

#[cfg(unix)]
#[tokio::test]
async fn tmux_birth_uses_target_with_colon() {
    let (dir, lives) = mux_lives("echo 1790624703");
    assert_eq!(lives.life("proj", false).await.as_deref(), Some("t:1790624703"));
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    assert_eq!(log.trim(), "display-message -p -t =proj: #{session_created}");
}

#[cfg(unix)]
#[tokio::test]
async fn tmux_birth_empty_error_or_garbage_is_none() {
    for body in ["echo; exit 0", "exit 0", "echo 1790624703; exit 1", "echo abc; exit 0"] {
        let (_dir, lives) = mux_lives(body);
        assert_eq!(lives.life("proj", false).await, None, "{body}");
    }
}

#[test]
fn session_life_from_batch_birth() {
    use hangar_server::list::links::session_life;
    assert_eq!(session_life(None, Some(100)).as_deref(), Some("t:100"));
    assert_eq!(session_life(None, None), None);
    assert_eq!(session_life(Some("preserved"), Some(100)).as_deref(), Some("k:preserved"));
}
