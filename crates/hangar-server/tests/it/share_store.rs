use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hangar_server::share::store::{Clock, Kind, PeekError, ShareStore, CODE_TTL, KEEP_EXPIRED, KEEP_REVOKED};

struct Fixture {
    dir: tempfile::TempDir,
    now: Arc<Mutex<f64>>,
    store: ShareStore,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(Mutex::new(1000.0));
        let store = ShareStore::open(dir.path().join("shares.json"), clock(&now));
        Self { dir, now, store }
    }
    fn at(&self, t: f64) -> &ShareStore {
        *self.now.lock().unwrap() = t;
        &self.store
    }
    fn file(&self) -> PathBuf {
        self.dir.path().join("shares.json")
    }
    fn reopen(&self) -> ShareStore {
        ShareStore::open(self.file(), clock(&self.now))
    }
}

fn clock(now: &Arc<Mutex<f64>>) -> Clock {
    let now = now.clone();
    Arc::new(move || *now.lock().unwrap())
}

fn set(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn life(v: &str) -> Option<String> {
    Some(v.into())
}

fn bad_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).unwrap().flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("shares.json.bad-"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect()
}

#[test]
fn create_writes_only_hashes_and_file_0600() {
    let f = Fixture::new();
    let (s, code) = f.at(1000.0).create("proj", life("t:100"), Kind::Share, false).unwrap();
    assert_eq!(code.len(), 26);
    assert!(code.chars().all(|c| c.is_ascii_alphanumeric()) && code.to_uppercase() == code);
    let raw = std::fs::read_to_string(f.file()).unwrap();
    assert!(!raw.contains(&code));
    assert!(raw.contains(&s.code_hash));
    assert_eq!(s.code_expires_at, 1000.0 + CODE_TTL);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(f.file()).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn redeem_is_single_use() {
    let f = Fixture::new();
    let (_, code) = f.at(1000.0).create("proj", life("t:100"), Kind::Share, false).unwrap();
    let (s, token) = f.at(1001.0).redeem(&code, "notebook do fulano", None, Kind::Share).unwrap();
    assert_eq!(s.device.as_deref(), Some("notebook do fulano"));
    assert_eq!(s.redeemed_at, Some(1001.0));
    assert_eq!(f.store.lookup_token(&token).unwrap().share_for("proj").unwrap().id, s.id);
    assert_eq!(f.at(1002.0).redeem(&code, "outro", None, Kind::Share).unwrap_err(), PeekError::Used);
}

#[test]
fn code_accepts_lowercase_and_spaces() {
    let f = Fixture::new();
    let (_, code) = f.at(1000.0).create("proj", life("t:100"), Kind::Share, false).unwrap();
    let s = f.store.peek(&format!("  {} ", code.to_lowercase()), Kind::Share).unwrap();
    assert_eq!(s.session, "proj");
}

#[test]
fn expired_and_unknown_code() {
    let f = Fixture::new();
    let (_, code) = f.at(1000.0).create("proj", life("t:100"), Kind::Share, false).unwrap();
    assert_eq!(f.at(1000.0 + CODE_TTL + 1.0).peek(&code, Kind::Share).unwrap_err(), PeekError::Expired);
    assert_eq!(f.at(1000.0).peek("AAAAAAAAAAAAAAAAAAAAAAAAAA", Kind::Share).unwrap_err(), PeekError::Unknown);
}

#[test]
fn revoke_drops_token_but_lookup_still_sees() {
    let f = Fixture::new();
    let (s, code) = f.at(1000.0).create("proj", life("t:100"), Kind::Share, false).unwrap();
    let (_, token) = f.at(1001.0).redeem(&code, "x", None, Kind::Share).unwrap();
    assert!(f.store.revoke(&s.id).unwrap());
    assert!(!f.store.revoke(&s.id).unwrap());
    let guest = f.store.lookup_token(&token).unwrap();
    assert!(!guest.sees("proj"));
    assert!(guest.share_for("proj").unwrap().revoked_at.is_some());
    assert_eq!(f.at(1002.0).peek(&code, Kind::Share).unwrap_err(), PeekError::Revoked);
}

#[test]
fn revoke_session_rename_and_active() {
    let f = Fixture::new();
    let st = f.at(1000.0);
    st.create("a", life("t:1"), Kind::Share, false).unwrap();
    st.create("a", life("t:1"), Kind::Share, false).unwrap();
    st.create("b", life("t:2"), Kind::Share, false).unwrap();
    assert_eq!(st.active_sessions(), set(&["a", "b"]));
    st.rename("b", "c").unwrap();
    assert_eq!(st.active_sessions(), set(&["a", "c"]));
    assert_eq!(st.revoke_session("a", None).unwrap(), 2);
    assert!(st.list_for("a").is_empty());
    assert!(st.has_active(false));
}

#[test]
fn expired_unused_code_does_not_count_as_active() {
    let f = Fixture::new();
    f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    assert!(!f.at(1000.0 + CODE_TTL + 1.0).has_active(false));
}

#[test]
fn sweep_revokes_session_whose_life_changed() {
    let f = Fixture::new();
    let (_, code) = f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    let (_, token) = f.at(1001.0).redeem(&code, "x", None, Kind::Share).unwrap();
    f.at(1000.0).create("b", life("t:2"), Kind::Share, false).unwrap();
    assert_eq!(f.at(1002.0).sweep(|session, _| session == "b").unwrap(), 1);
    assert!(!f.store.lookup_token(&token).unwrap().sees("a"));
    assert_eq!(f.store.active_sessions(), set(&["b"]));
}

#[test]
fn sweep_deletes_old_expired_code() {
    let f = Fixture::new();
    f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    f.at(1000.0 + CODE_TTL + KEEP_EXPIRED + 1.0).sweep(|_, _| true).unwrap();
    assert!(f.store.all().is_empty());
}

#[test]
fn persists_across_reloads() {
    let f = Fixture::new();
    let (s, code) = f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    let (_, token) = f.at(1001.0).redeem(&code, "x", None, Kind::Share).unwrap();
    let again = f.reopen();
    assert_eq!(again.lookup_token(&token).unwrap().share_for("a").unwrap().id, s.id);
}

#[test]
fn corrupt_file_does_not_crash() {
    let f = Fixture::new();
    std::fs::write(f.file(), "{nao e json").unwrap();
    let st = f.reopen();
    assert!(!st.has_active(false));
    // Vai pro lado, senão a próxima gravação o apagaria.
    st.create("a", life("t:1"), Kind::Share, false).unwrap();
    assert_eq!(bad_files(f.dir.path()), vec!["{nao e json".to_string()]);
}

#[test]
fn set_life() {
    let f = Fixture::new();
    let (s, _) = f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    f.store.set_life("a", life("k:abc")).unwrap();
    let got = f.store.all().into_iter().find(|x| x.id == s.id).unwrap();
    assert_eq!(got.life.as_deref(), Some("k:abc"));
}

#[test]
fn sweep_asks_alive_without_holding_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let now = Arc::new(Mutex::new(1000.0));
    let store = Arc::new(ShareStore::open(dir.path().join("shares.json"), clock(&now)));
    store.create("a", life("t:1"), Kind::Share, false).unwrap();
    let free = Mutex::new(Vec::new());
    *now.lock().unwrap() = 1001.0;
    store.sweep(|_, _| {
        // Outra thread prova que o lock está solto; presa, ela estoura o prazo.
        let (tx, rx) = std::sync::mpsc::channel();
        let other = store.clone();
        std::thread::spawn(move || { let _ = tx.send(other.has_any(false)); });
        free.lock().unwrap().push(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        true
    }).unwrap();
    assert_eq!(*free.lock().unwrap(), vec![true]);
}

#[test]
fn sweep_does_not_revoke_share_changed_during_query() {
    let f = Fixture::new();
    let (s, _) = f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    let n = f.at(1001.0).sweep(|_, _| {
        // Troca de modo terminou no meio da consulta.
        f.store.set_life("a", life("t:2")).unwrap();
        false
    }).unwrap();
    assert_eq!(n, 0);
    assert!(f.store.all().into_iter().find(|x| x.id == s.id).unwrap().revoked_at.is_none());
}

#[test]
fn has_any() {
    let f = Fixture::new();
    assert!(!f.store.has_any(false));
    f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    assert!(f.store.has_any(false));
}

#[test]
fn recently_ended_only_inside_window() {
    let f = Fixture::new();
    let (s, _) = f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    assert!(!f.store.recently_ended(120.0, false));
    f.at(5000.0).revoke(&s.id).unwrap();
    let revoked = f.store.all()[0].revoked_at.unwrap();
    assert_eq!(revoked, 5000.0);
    assert!(f.at(revoked + 119.0).recently_ended(120.0, false));
    assert!(!f.at(revoked + 121.0).recently_ended(120.0, false));
}

#[test]
fn redeem_with_existing_token_reuses_the_token() {
    let f = Fixture::new();
    let (_, ca) = f.at(1000.0).create("proj-a", life("t:1"), Kind::Share, false).unwrap();
    let (_, cb) = f.store.create("proj-b", life("t:2"), Kind::Share, false).unwrap();
    let (_, tok) = f.at(1001.0).redeem(&ca, "Pixel", None, Kind::Share).unwrap();
    let (_, tok2) = f.at(1002.0).redeem(&cb, "Pixel", Some(&tok), Kind::Share).unwrap();
    assert_eq!(tok2, tok);
    assert_eq!(f.store.lookup_token(&tok).unwrap().sessions(), set(&["proj-a", "proj-b"]));
}

#[test]
fn redeem_with_revoked_token_makes_new_token() {
    let f = Fixture::new();
    let (a, ca) = f.at(1000.0).create("proj-a", life("t:1"), Kind::Share, false).unwrap();
    let (_, cb) = f.store.create("proj-b", life("t:2"), Kind::Share, false).unwrap();
    let (_, tok) = f.at(1001.0).redeem(&ca, "Pixel", None, Kind::Share).unwrap();
    f.store.revoke(&a.id).unwrap();
    let (_, tok2) = f.at(1002.0).redeem(&cb, "Pixel", Some(&tok), Kind::Share).unwrap();
    assert_ne!(tok2, tok);
    assert_eq!(f.store.lookup_token(&tok2).unwrap().sessions(), set(&["proj-b"]));
}

#[test]
fn revoking_one_share_keeps_the_other_session_of_the_token() {
    let f = Fixture::new();
    let (a, ca) = f.at(1000.0).create("proj-a", life("t:1"), Kind::Share, false).unwrap();
    let (_, cb) = f.store.create("proj-b", life("t:2"), Kind::Share, false).unwrap();
    let (_, tok) = f.at(1001.0).redeem(&ca, "Pixel", None, Kind::Share).unwrap();
    f.at(1002.0).redeem(&cb, "Pixel", Some(&tok), Kind::Share).unwrap();
    f.store.revoke(&a.id).unwrap();
    let guest = f.store.lookup_token(&tok).unwrap();
    assert_eq!(guest.sessions(), set(&["proj-b"]));
    assert!(guest.share_for("proj-a").unwrap().revoked_at.is_some());
}

#[test]
fn same_session_invited_twice_survives_one_revoke() {
    let f = Fixture::new();
    let (a, ca) = f.at(1000.0).create("proj", life("t:1"), Kind::Share, false).unwrap();
    let (_, cb) = f.at(1000.5).create("proj", life("t:1"), Kind::Share, false).unwrap();
    let (_, tok) = f.at(1001.0).redeem(&ca, "Pixel", None, Kind::Share).unwrap();
    f.at(1002.0).redeem(&cb, "Pixel", Some(&tok), Kind::Share).unwrap();
    f.store.revoke(&a.id).unwrap();
    assert!(f.store.lookup_token(&tok).unwrap().sees("proj"));
}

#[test]
fn pair_code_does_not_redeem_as_share_and_vice_versa() {
    let f = Fixture::new();
    let (_, cp) = f.at(1000.0).create("proj", life("t:1"), Kind::Pair, false).unwrap();
    let (_, cs) = f.store.create("proj", life("t:1"), Kind::Share, false).unwrap();
    assert_eq!(f.at(1001.0).peek(&cp, Kind::Share).unwrap_err(), PeekError::Unknown);
    assert!(f.store.peek(&cs, Kind::Pair).is_err());
}

#[test]
fn attach_links_other_token_sessions_and_revoking_parent_revokes_child() {
    let f = Fixture::new();
    let (parent, tok_pair) = f.at(1000.0).create_redeemed("proj-y", life("t:9"), Kind::Pair).unwrap();
    let (_, c) = f.store.create("proj-z", life("t:3"), Kind::Share, false).unwrap();
    let (_, tok) = f.at(1001.0).redeem(&c, "Pixel", None, Kind::Share).unwrap();
    assert_eq!(f.at(1002.0).attach(&tok, &tok_pair).unwrap(), 1);
    let guest = f.store.lookup_token(&tok).unwrap();
    assert_eq!(guest.sessions(), set(&["proj-y", "proj-z"]));
    assert_eq!(guest.kind_of("proj-y"), Some(Kind::Pair));
    assert_eq!(guest.kind_of("proj-z"), Some(Kind::Share));
    assert!(guest.pair_shares().is_empty()); // filho ligado não fala pelo par
    f.store.revoke(&parent.id).unwrap();
    assert_eq!(f.store.lookup_token(&tok).unwrap().sessions(), set(&["proj-z"]));
}

#[test]
fn attach_requires_both_tokens_valid() {
    let f = Fixture::new();
    let (_, c) = f.at(1000.0).create("proj-z", life("t:3"), Kind::Share, false).unwrap();
    let (_, tok) = f.at(1001.0).redeem(&c, "Pixel", None, Kind::Share).unwrap();
    assert_eq!(f.at(1002.0).attach(&tok, "nao-existe").unwrap(), 0);
    assert_eq!(f.store.attach("nao-existe", &tok).unwrap(), 0);
}

#[test]
fn dialog_list_and_shared_mark_ignore_pair() {
    let f = Fixture::new();
    f.at(1000.0).create("proj", life("t:1"), Kind::Pair, false).unwrap();
    assert!(f.at(1001.0).list_for("proj").is_empty());
    assert!(f.store.active_sessions().is_empty());
}

#[test]
fn chained_attach_points_to_root_and_revoking_root_drops_all() {
    let f = Fixture::new();
    let (parent, tok_p) = f.at(1000.0).create_redeemed("proj-y", life("t:9"), Kind::Pair).unwrap();
    let (_, c1) = f.store.create("proj-z", life("t:3"), Kind::Share, false).unwrap();
    let (_, tok_t) = f.at(1001.0).redeem(&c1, "Pixel", None, Kind::Share).unwrap();
    let (_, c2) = f.at(1000.0).create("proj-w", life("t:4"), Kind::Share, false).unwrap();
    let (_, tok_t2) = f.at(1001.0).redeem(&c2, "PC", None, Kind::Share).unwrap();
    assert_eq!(f.at(1002.0).attach(&tok_t, &tok_p).unwrap(), 1);
    assert_eq!(f.at(1003.0).attach(&tok_t2, &tok_t).unwrap(), 2);
    assert_eq!(f.at(1004.0).attach(&tok_t2, &tok_t).unwrap(), 0); // sem duplicar
    let copy = f.store.lookup_token(&tok_t2).unwrap().share_for("proj-y").unwrap().clone();
    assert_eq!(copy.parent_id.as_deref(), Some(parent.id.as_str()));
    f.store.revoke(&parent.id).unwrap();
    assert_eq!(f.store.lookup_token(&tok_t2).unwrap().sessions(), set(&["proj-w", "proj-z"]));
    assert_eq!(f.store.lookup_token(&tok_t).unwrap().sessions(), set(&["proj-z"]));
}

#[test]
fn revoke_session_filters_by_kind() {
    let f = Fixture::new();
    f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    let (_, token) = f.store.create_redeemed("a", life("t:1"), Kind::Pair).unwrap();
    assert_eq!(f.store.revoke_session("a", Some(Kind::Share)).unwrap(), 1);
    assert!(f.store.lookup_token(&token).unwrap().share_for("a").unwrap().revoked_at.is_none());
    assert_eq!(f.store.revoke_session("a", None).unwrap(), 1);
    assert!(f.store.lookup_token(&token).unwrap().share_for("a").unwrap().revoked_at.is_some());
}

#[test]
fn session_shared_and_paired_under_same_token_resolves_to_share() {
    let f = Fixture::new();
    let (_, c) = f.at(1000.0).create("proj", life("t:1"), Kind::Share, false).unwrap();
    let (_, tok) = f.at(1001.0).redeem(&c, "Pixel", None, Kind::Share).unwrap();
    let (_, tok_pair) = f.at(1002.0).create_redeemed("proj", life("t:1"), Kind::Pair).unwrap();
    assert_eq!(f.at(1003.0).attach(&tok, &tok_pair).unwrap(), 1);
    // A cópia do par é mais nova, mas o convite de verdade vence.
    assert_eq!(f.store.lookup_token(&tok).unwrap().kind_of("proj"), Some(Kind::Share));
}

// Comportamentos sem teste no Python.

#[test]
fn redeem_trims_device_to_80_chars_and_empty_becomes_none() {
    let f = Fixture::new();
    let (_, c1) = f.at(1000.0).create("a", life("t:1"), Kind::Share, false).unwrap();
    let (_, c2) = f.store.create("b", life("t:1"), Kind::Share, false).unwrap();
    let long = format!("  {}  ", "é".repeat(100));
    let (s, _) = f.store.redeem(&c1, &long, None, Kind::Share).unwrap();
    assert_eq!(s.device, Some("é".repeat(80)));
    let (s, _) = f.store.redeem(&c2, "   ", None, Kind::Share).unwrap();
    assert_eq!(s.device, None);
}

#[test]
fn sweep_purges_revoked_after_30_days_only() {
    let f = Fixture::new();
    let (a, _) = f.at(1000.0).create_redeemed("a", life("t:1"), Kind::Pair).unwrap();
    let (b, _) = f.store.create_redeemed("b", life("t:1"), Kind::Pair).unwrap();
    f.at(2000.0).revoke(&a.id).unwrap();
    f.at(3000.0).revoke(&b.id).unwrap();
    assert_eq!(f.at(2000.0 + KEEP_REVOKED + 1.0).sweep(|_, _| true).unwrap(), 0);
    let left: Vec<String> = f.store.all().into_iter().map(|s| s.id).collect();
    assert_eq!(left, vec![b.id.clone()]);
    assert_eq!(f.reopen().all().len(), 1);
}

#[test]
fn redeemed_pair_has_no_code_and_pair_shares_lists_every_live_root() {
    let f = Fixture::new();
    let (p1, tok) = f.at(1000.0).create_redeemed("x", life("t:1"), Kind::Pair).unwrap();
    assert_eq!((p1.code_hash.as_str(), p1.code_expires_at, p1.redeemed_at), ("", 1000.0, Some(1000.0)));
    let (_, other) = f.store.create_redeemed("y", life("t:2"), Kind::Pair).unwrap();
    // Duas raízes de par ligadas ao token: as duas falam pelo par, não só a primeira.
    let (_, c) = f.store.create("w", life("t:4"), Kind::Pair, false).unwrap();
    f.store.redeem(&c, "", Some(&tok), Kind::Pair).unwrap();
    f.store.attach(&tok, &other).unwrap();
    let guest = f.store.lookup_token(&tok).unwrap();
    let mut roots: Vec<&str> = guest.pair_shares().iter().map(|s| s.session.as_str()).collect();
    roots.sort();
    assert_eq!(roots, vec!["w", "x"]);
    assert!(f.store.lookup_token("").is_none());
}

#[test]
fn reads_and_rewrites_python_file_without_losing_fields() {
    let f = Fixture::new();
    // Gravado por `share_store.create` + `redeem` do Python (indent 2, float, null explícito).
    let python = r#"[
  {
    "id": "3f1c2a9b7d6e5f40",
    "session": "proj",
    "life": "t:1790624703",
    "created_at": 1791234567.123456,
    "code_expires_at": 1791320967.123456,
    "code_hash": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
    "token_hash": "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae",
    "device": "Pixel",
    "redeemed_at": 1791234570.5,
    "revoked_at": null,
    "local": false,
    "kind": "share",
    "parent_id": null
  },
  {
    "id": "0011223344556677",
    "session": "proj-par",
    "life": "k:abc",
    "created_at": 1791234500.0,
    "code_expires_at": 1791234500.0,
    "code_hash": "",
    "token_hash": "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae",
    "device": null,
    "redeemed_at": 1791234500.0,
    "revoked_at": null,
    "local": true,
    "kind": "pair",
    "parent_id": "aaaaaaaaaaaaaaaa"
  }
]"#;
    std::fs::write(f.file(), python).unwrap();
    let st = f.reopen();
    // sha256("foo") é o token_hash da fixture.
    let guest = st.lookup_token("foo").unwrap();
    assert_eq!(guest.sessions(), set(&["proj", "proj-par"]));
    *f.now.lock().unwrap() = 1791234600.0;
    st.rename("nada", "x").unwrap();
    st.set_life("proj", life("t:1790624703")).unwrap();
    let rewritten = std::fs::read_to_string(f.file()).unwrap();
    assert_eq!(rewritten, python);
}

// A gravação sai do lock do estado: com escritas concorrentes, o disco termina no último estado.
#[test]
fn concurrent_writes_end_with_the_latest_state_on_disk() {
    let f = Fixture::new();
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| for _ in 0..20 { f.store.create("proj", life("t:1"), Kind::Share, false).unwrap(); });
        }
    });
    let ids = |st: &ShareStore| st.all().into_iter().map(|s| s.id).collect::<HashSet<_>>();
    assert_eq!(ids(&f.store).len(), 160);
    assert_eq!(ids(&f.reopen()), ids(&f.store));
}

// Registros filhos do par: um por sessão do grupo, sob a chave do par, com o nível dela.

fn pair_root(f: &Fixture) -> (hangar_server::share::store::Share, String) {
    f.at(1000.0).create_redeemed("dono", life("t:1"), Kind::Pair).unwrap()
}

#[test]
fn grant_gives_the_pair_token_the_session_with_its_level() {
    let f = Fixture::new();
    let (root, tok) = pair_root(&f);
    f.at(1001.0).grant(&root.id, "membro", life("t:m"), Kind::Share).unwrap();
    let guest = f.store.lookup_token(&tok).unwrap();
    assert_eq!(guest.kind_of("membro"), Some(Kind::Share));
    let child = guest.share_for("membro").unwrap();
    assert_eq!(child.parent_id.as_deref(), Some(root.id.as_str()));
    // A vida é a da sessão do filho: o porteiro e a varredura comparam com ela.
    assert_eq!(child.life, life("t:m"));
    // Filho não fala pelo par: só a raiz.
    assert_eq!(guest.pair_shares().len(), 1);
    f.at(1002.0).grant(&root.id, "membro", life("t:m"), Kind::Pair).unwrap();
    assert_eq!(f.store.lookup_token(&tok).unwrap().kind_of("membro"), Some(Kind::Pair));
    assert_eq!(f.store.children(&root.id).len(), 1, "trocar o nível não cria outro filho");
    assert!(f.store.ungrant(&root.id, "membro").unwrap());
    assert!(!f.store.lookup_token(&tok).unwrap().sees("membro"));
    assert!(!f.store.ungrant(&root.id, "membro").unwrap());
    assert!(f.store.children(&root.id).is_empty());
}

#[test]
fn grant_on_unknown_or_revoked_root_fails() {
    let f = Fixture::new();
    assert!(f.at(1000.0).grant("nao-existe", "membro", life("t:m"), Kind::Share).is_err());
    let (root, _) = pair_root(&f);
    f.store.revoke(&root.id).unwrap();
    assert!(f.store.grant(&root.id, "membro", life("t:m"), Kind::Share).is_err());
}

#[test]
fn revoking_the_root_drops_every_child() {
    let f = Fixture::new();
    let (root, tok) = pair_root(&f);
    f.at(1001.0).grant(&root.id, "a", life("t:a"), Kind::Share).unwrap();
    f.store.grant(&root.id, "b", life("t:b"), Kind::Pair).unwrap();
    assert_eq!(f.store.lookup_token(&tok).unwrap().sessions(), set(&["dono", "a", "b"]));
    f.store.revoke(&root.id).unwrap();
    assert!(f.store.lookup_token(&tok).unwrap().sessions().is_empty());
    assert!(f.store.children(&root.id).is_empty());
}

#[test]
fn attach_copies_every_child_of_the_same_root() {
    let f = Fixture::new();
    let (root, tok_pair) = pair_root(&f);
    f.at(1001.0).grant(&root.id, "a", life("t:a"), Kind::Share).unwrap();
    f.store.grant(&root.id, "b", life("t:b"), Kind::Pair).unwrap();
    let (_, c) = f.store.create("z", life("t:3"), Kind::Share, false).unwrap();
    let (_, tok) = f.at(1002.0).redeem(&c, "Pixel", None, Kind::Share).unwrap();
    assert_eq!(f.at(1003.0).attach(&tok, &tok_pair).unwrap(), 3);
    assert_eq!(f.store.lookup_token(&tok).unwrap().sessions(), set(&["z", "dono", "a", "b"]));
    assert_eq!(f.at(1004.0).attach(&tok, &tok_pair).unwrap(), 0);
    // A cópia não é filho do par: `children` só devolve os da chave do par.
    assert_eq!(f.store.children(&root.id).len(), 2);
    // Tirar a sessão do par tira também da cópia.
    f.store.ungrant(&root.id, "a").unwrap();
    assert_eq!(f.store.lookup_token(&tok).unwrap().sessions(), set(&["z", "dono", "b"]));
}

#[test]
fn pair_children_stay_out_of_owner_list_shared_mark_and_end_all() {
    let f = Fixture::new();
    let (root, tok) = pair_root(&f);
    f.at(1001.0).grant(&root.id, "membro", life("t:m"), Kind::Share).unwrap();
    assert!(f.store.list_for("membro").is_empty());
    assert!(f.store.active_sessions().is_empty());
    assert_eq!(f.store.revoke_session("membro", Some(Kind::Share)).unwrap(), 0);
    assert!(f.store.lookup_token(&tok).unwrap().sees("membro"));
    // Sessão que morreu leva tudo.
    assert_eq!(f.store.revoke_session("membro", None).unwrap(), 1);
}

#[test]
fn grant_many_reaches_attached_tokens_and_full_sessions_reads_the_pair_token() {
    let f = Fixture::new();
    let (root, tok) = pair_root(&f);
    let (_, c) = f.store.create("outra", life("t:o"), Kind::Share, false).unwrap();
    let (_, other) = f.at(1001.0).redeem(&c, "Pixel", None, Kind::Share).unwrap();
    f.store.attach(&other, &tok).unwrap();
    let sessions = [("a".to_owned(), life("t:a")), ("b".to_owned(), life("t:b"))];
    f.at(1002.0).grant_many(&root.id, &sessions, Kind::Share).unwrap();
    for t in [&tok, &other] {
        let guest = f.store.lookup_token(t).unwrap();
        assert_eq!(guest.kind_of("a"), Some(Kind::Share));
        assert_eq!(guest.kind_of("b"), Some(Kind::Share));
    }
    f.store.grant(&root.id, "b", life("t:b"), Kind::Pair).unwrap();
    assert_eq!(f.store.lookup_token(&other).unwrap().kind_of("b"), Some(Kind::Pair));
    assert_eq!(f.store.full_sessions().get(&root.id), Some(&set(&["a"])));
    assert!(f.store.grant_many("nao-existe", &sessions, Kind::Pair).is_err());
}

#[test]
fn children_survive_reload_without_new_fields_on_disk() {
    let f = Fixture::new();
    let (root, tok) = pair_root(&f);
    f.at(1001.0).grant(&root.id, "membro", life("t:m"), Kind::Share).unwrap();
    // Mesmas chaves do formato do Python: um Hangar antigo lê o arquivo sem quarentena.
    let raw: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_str(&std::fs::read_to_string(f.file()).unwrap()).unwrap();
    let keys = ["id", "session", "life", "created_at", "code_expires_at", "code_hash", "token_hash",
        "device", "redeemed_at", "revoked_at", "local", "kind", "parent_id"];
    for record in &raw {
        let mut got: Vec<&str> = record.keys().map(String::as_str).collect();
        got.sort();
        let mut want = keys.to_vec();
        want.sort();
        assert_eq!(got, want);
    }
    let st = f.reopen();
    assert_eq!(st.lookup_token(&tok).unwrap().kind_of("membro"), Some(Kind::Share));
    assert_eq!(st.children(&root.id).len(), 1);
    assert!(st.list_for("membro").is_empty());
}

#[test]
fn grant_and_ungrant_reach_copies_held_by_other_tokens() {
    let f = Fixture::new();
    let (root, tok_pair) = pair_root(&f);
    f.at(1001.0).grant(&root.id, "s", life("t:s"), Kind::Share).unwrap();
    let (_, c) = f.store.create("z", life("t:3"), Kind::Share, false).unwrap();
    let (_, tok_b) = f.at(1002.0).redeem(&c, "Pixel", None, Kind::Share).unwrap();
    assert_eq!(f.at(1003.0).attach(&tok_b, &tok_pair).unwrap(), 2);
    assert_eq!(f.store.lookup_token(&tok_b).unwrap().kind_of("s"), Some(Kind::Share));
    // Baixar o nível no par baixa também na cópia: senão o outro token seguiria escrevendo.
    f.store.grant(&root.id, "s", life("t:s"), Kind::Pair).unwrap();
    assert_eq!(f.store.lookup_token(&tok_b).unwrap().kind_of("s"), Some(Kind::Pair));
    assert!(f.store.ungrant(&root.id, "s").unwrap());
    assert_eq!(f.store.lookup_token(&tok_b).unwrap().sessions(), set(&["z", "dono"]));
}

#[test]
fn member_who_left_and_came_back_is_copied_again_by_attach() {
    let f = Fixture::new();
    let (root, tok_pair) = pair_root(&f);
    f.at(1001.0).grant(&root.id, "s", life("t:s"), Kind::Share).unwrap();
    let (_, c) = f.store.create("z", life("t:3"), Kind::Share, false).unwrap();
    let (_, tok_b) = f.at(1002.0).redeem(&c, "Pixel", None, Kind::Share).unwrap();
    f.at(1003.0).attach(&tok_b, &tok_pair).unwrap();
    f.store.ungrant(&root.id, "s").unwrap();
    f.at(1004.0).grant(&root.id, "s", life("t:s"), Kind::Share).unwrap();
    f.at(1005.0).attach(&tok_b, &tok_pair).unwrap();
    assert_eq!(f.store.lookup_token(&tok_b).unwrap().sessions(), set(&["z", "dono", "s"]));
}

#[test]
fn grant_after_attach_reaches_the_token_that_attached_the_root() {
    let f = Fixture::new();
    let (root, tok_pair) = pair_root(&f);
    let (_, c) = f.store.create("z", life("t:3"), Kind::Share, false).unwrap();
    let (_, tok_b) = f.at(1002.0).redeem(&c, "Pixel", None, Kind::Share).unwrap();
    f.at(1003.0).attach(&tok_b, &tok_pair).unwrap();
    f.at(1004.0).grant(&root.id, "novo", life("t:n"), Kind::Pair).unwrap();
    let guest = f.store.lookup_token(&tok_b).unwrap();
    assert_eq!(guest.sessions(), set(&["z", "dono", "novo"]));
    assert_eq!(guest.kind_of("novo"), Some(Kind::Pair));
    assert_eq!(f.store.children(&root.id).len(), 1, "a cópia não conta como filho do par");
    f.at(1005.0).grant(&root.id, "novo", life("t:n"), Kind::Share).unwrap();
    assert_eq!(f.store.lookup_token(&tok_b).unwrap().records.iter().filter(|r| r.session == "novo").count(), 1, "trocar o nível não duplica a cópia");
}

#[test]
fn pair_invite_level_survives_a_restart() {
    let f = Fixture::new();
    let (_, code) = f.store.create_pair_invite("s0", life("t:1"), Kind::Share).unwrap();
    let (plain, _) = f.store.create("s1", life("t:1"), Kind::Pair, false).unwrap();
    let reopened = f.reopen();
    assert_eq!(reopened.peek(&code, Kind::Pair).unwrap().level, Some(Kind::Share));
    assert_eq!(reopened.all().into_iter().find(|s| s.id == plain.id).unwrap().level, None);
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(f.file()).unwrap()).unwrap();
    let without = raw.as_array().unwrap().iter().filter(|r| r.get("level").is_none()).count();
    assert_eq!(without, 1, "registro sem nível não ganha a chave");
}

#[test]
fn corrupt_file_marks_unreadable_until_persisted() {
    let f = Fixture::new();
    assert!(!f.store.unreadable(), "arquivo ausente não é ilegível");
    std::fs::write(f.file(), "[]").unwrap();
    assert!(!f.reopen().unreadable());
    std::fs::write(f.file(), "{corrompido").unwrap();
    let reopened = f.reopen();
    assert!(reopened.unreadable(), "shares.json ilegível deve sinalizar unreadable()");
    assert_eq!(bad_files(f.dir.path()), ["{corrompido"]);
    reopened.create("s0", life("t:1"), Kind::Share, false).unwrap();
    assert!(!reopened.unreadable(), "gravação bem-sucedida limpa unreadable()");
}
