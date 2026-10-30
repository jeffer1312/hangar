//! Cópia diária e restauração: a cifra com a senha de recuperação, a retenção de quem guarda, o envio
//! só com revisão nova e a restauração só em cofre vazio, contra um servidor de verdade guardando.
use super::*;
use crate::{auth::TrustedHosts, config::Config, groups::peers::{PeerBook, PeerClient}, routes::{AppState, router}};
use backup::{CopyError, RestoreError, Sent};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::{net::SocketAddr, sync::Mutex};

const PASSWORD: &str = "senha-guardada-Hq27";
const RECOVERY: &str = "recuperacao-longa-Wm4";
const TOKEN: &str = "tok-guarda";
const OWNER_ID: &str = "dono";

/// Chaveiro falso com a chave do cofre e o item da recuperação.
#[derive(Default)]
struct Keys { key: Mutex<Option<[u8; 32]>>, recovery: Mutex<Option<Vec<u8>>> }

impl KeyStore for Keys {
    fn read(&self) -> BoxFuture<'_, Result<Option<Key>, KeyError>> {
        let found = self.key.lock().unwrap().map(Zeroizing::new);
        Box::pin(async move { Ok(found) })
    }
    fn write(&self, key: Key) -> BoxFuture<'_, Result<(), KeyError>> {
        *self.key.lock().unwrap() = Some(*key);
        Box::pin(async { Ok(()) })
    }
    fn read_recovery(&self) -> BoxFuture<'_, Result<Option<Zeroizing<Vec<u8>>>, KeyError>> {
        let found = self.recovery.lock().unwrap().clone().map(Zeroizing::new);
        Box::pin(async move { Ok(found) })
    }
    fn write_recovery(&self, item: Zeroizing<Vec<u8>>) -> BoxFuture<'_, Result<(), KeyError>> {
        *self.recovery.lock().unwrap() = Some(item.to_vec());
        Box::pin(async { Ok(()) })
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool { haystack.windows(needle.len()).any(|w| w == needle.as_bytes()) }

fn new_entry(name: &str) -> NewEntry {
    NewEntry { name: name.into(), username: None, password: Zeroizing::new(PASSWORD.into()), url: None, notes: None, policy: Policy::Ask }
}

fn doc_with(name: &str) -> store::Doc {
    let entry = Entry { id: "e1".into(), name: name.into(), username: None, password: Zeroizing::new(PASSWORD.into()), url: None,
        notes: None, policy: Policy::Ask, created_at: 1.0, updated_at: 1.0 };
    store::Doc { entries: vec![entry], revision: 4 }
}

/// Uma máquina que guarda cópias (`server_id` "lab"), com o roteador de verdade.
async fn keeper() -> (SocketAddr, tempfile::TempDir) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config { listen: addr, upstream: "127.0.0.1:9".parse().unwrap(), auth_token: TOKEN.into(),
        internal_secret: "privado".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
    state.hangar_dir = dir.path().to_owned();
    state.server_id = "lab".into();
    state.vault = Arc::new(Vault::new(Arc::new(Keys::default())));
    let app = router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, dir)
}

fn peers_for(dir: &std::path::Path, addr: SocketAddr) -> PeerClient {
    let file = dir.join("peers.json");
    std::fs::write(&file, json!({"lab": {"base_url": format!("http://{addr}"), "token": TOKEN}}).to_string()).unwrap();
    PeerClient::new(PeerBook::new(Some(file)))
}

fn copies(keeper_dir: &std::path::Path, id: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(keeper_dir.join("vault-backups").join(id)).map(|r| {
        r.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect()
    }).unwrap_or_default();
    names.sort();
    names
}

/// A dona com uma senha, `backup_peer = lab` e a senha de recuperação definida.
async fn owner_vault(dir: &std::path::Path) -> (Vault, Arc<Keys>) {
    let keys = Arc::new(Keys::default());
    let vault = Vault::new(keys.clone());
    vault.create(dir).await.unwrap();
    vault.add(dir, new_entry("banco")).await.unwrap();
    config::save(dir, &config::Config { owner: None, backup_peer: Some("lab".into()) }).unwrap();
    backup::set_recovery(&vault, dir, Zeroizing::new(RECOVERY.into())).await.unwrap();
    (vault, keys)
}

#[test]
fn the_copy_round_trips_with_the_recovery_password_and_carries_nothing_in_clear() {
    let recovery = backup::Recovery::new(RECOVERY).unwrap();
    let sealed = backup::seal_copy(&recovery, &doc_with("banco")).unwrap();
    assert_eq!(sealed[0], backup::FORMAT);
    assert_eq!(&sealed[1..1 + backup::SALT_LEN], &recovery.salt[..], "o sal vai em claro, autenticado");
    assert!(!contains(&sealed, PASSWORD) && !contains(&sealed, RECOVERY) && !contains(&sealed, "banco"));
    let (doc, again) = backup::open_copy(RECOVERY, &sealed).unwrap();
    assert_eq!((doc.entries[0].name.as_str(), doc.entries[0].password.as_str(), doc.revision), ("banco", PASSWORD, 4));
    assert_eq!(again.key[..], recovery.key[..], "a mesma senha e o mesmo sal dão a mesma chave");
    assert_eq!(backup::Recovery::from_item(&recovery.to_item()).unwrap().salt, recovery.salt);
}

#[test]
fn a_wrong_password_or_a_tampered_copy_does_not_open() {
    let recovery = backup::Recovery::new(RECOVERY).unwrap();
    let sealed = backup::seal_copy(&recovery, &doc_with("banco")).unwrap();
    assert_eq!(backup::open_copy("outra-senha", &sealed).err(), Some(CopyError::WrongPassword));
    let mut salt = sealed.clone();
    salt[3] ^= 1;
    assert_eq!(backup::open_copy(RECOVERY, &salt).err(), Some(CopyError::WrongPassword), "sal trocado não passa");
    assert_eq!(backup::open_copy(RECOVERY, &sealed[..20]).err(), Some(CopyError::Unreadable));
    let mut version = sealed;
    version[0] = 9;
    assert_eq!(backup::open_copy(RECOVERY, &version).err(), Some(CopyError::Unreadable));
}

#[test]
fn retention_keeps_seven_and_never_drops_the_newest_non_empty_copy() {
    let mut names: Vec<String> = vec!["01.bin".into()];
    names.extend((2..=10).map(|i| format!("{i:02}.empty.bin")));
    assert_eq!(backup::prune(&names), vec!["02.empty.bin", "03.empty.bin"], "a 01 é a única com senhas e fica");
    let full: Vec<String> = (1..=9).map(|i| format!("{i:02}.bin")).collect();
    assert_eq!(backup::prune(&full), vec!["01.bin", "02.bin"]);
    assert!(backup::prune(&full[..7]).is_empty());
}

#[test]
fn the_keeper_prunes_on_disk_and_serves_the_newest() {
    let dir = tempfile::tempdir().unwrap();
    let copy = |tag: u8| { let mut b = vec![backup::FORMAT; 60]; b[59] = tag; b };
    backup::keep_copy(dir.path(), OWNER_ID, &copy(1), 3).unwrap();
    for tag in 2..=9 { backup::keep_copy(dir.path(), OWNER_ID, &copy(tag), 0).unwrap(); }
    let names = copies(dir.path(), OWNER_ID);
    assert_eq!(names.len(), 8, "{names:?}");
    assert!(!names[0].ends_with(".empty.bin"), "a cópia com senhas sobreviveu: {names:?}");
    assert_eq!(backup::newest_copy(dir.path(), OWNER_ID).unwrap(), Some(copy(9)));
    assert_eq!(backup::newest_copy(dir.path(), "outro").unwrap(), None);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: std::path::PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.path().join("vault-backups").join(OWNER_ID)), 0o700);
        assert_eq!(mode(dir.path().join("vault-backups").join(OWNER_ID).join(&names[0])), 0o600);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_copy_goes_only_when_the_revision_changed() {
    let (addr, keeper_dir) = keeper().await;
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), addr);
    let (vault, _) = owner_vault(dir.path()).await;
    assert_eq!(backup::send_once(&vault, &peers, dir.path(), OWNER_ID).await, Sent::Sent);
    assert_eq!(backup::send_once(&vault, &peers, dir.path(), OWNER_ID).await, Sent::Skipped("sem mudança"));
    assert_eq!(copies(keeper_dir.path(), OWNER_ID).len(), 1);
    vault.add(dir.path(), new_entry("email")).await.unwrap();
    assert_eq!(backup::send_once(&vault, &peers, dir.path(), OWNER_ID).await, Sent::Sent);
    let names = copies(keeper_dir.path(), OWNER_ID);
    assert_eq!(names.len(), 2);
    let stored = std::fs::read(keeper_dir.path().join("vault-backups").join(OWNER_ID).join(&names[1])).unwrap();
    assert!(!contains(&stored, PASSWORD), "quem guarda não vê a senha");
    let status = backup::status(dir.path(), "lab");
    assert!(status["last_ok_at"].is_number() && status.get("last_error").is_none(), "{status}");
    // Senha de recuperação nova: a cópia de lá abre com a velha, então vai de novo.
    backup::set_recovery(&vault, dir.path(), Zeroizing::new("outra-recuperacao".into())).await.unwrap();
    assert_eq!(backup::send_once(&vault, &peers, dir.path(), OWNER_ID).await, Sent::Sent);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_recovery_password_or_a_peer_nothing_is_sent() {
    let (addr, keeper_dir) = keeper().await;
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), addr);
    let vault = Vault::new(Arc::new(Keys::default()));
    assert_eq!(backup::send_once(&vault, &peers, dir.path(), OWNER_ID).await, Sent::Skipped("sem cofre nesta máquina"));
    vault.create(dir.path()).await.unwrap();
    assert_eq!(backup::send_once(&vault, &peers, dir.path(), OWNER_ID).await, Sent::Skipped("sem backup_peer"));
    config::save(dir.path(), &config::Config { owner: None, backup_peer: Some("lab".into()) }).unwrap();
    assert_eq!(backup::send_once(&vault, &peers, dir.path(), OWNER_ID).await, Sent::Skipped("sem senha de recuperação"));
    assert!(copies(keeper_dir.path(), OWNER_ID).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_needs_an_empty_vault_and_the_right_password() {
    let (addr, _keeper_dir) = keeper().await;
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), addr);
    let (vault, _) = owner_vault(dir.path()).await;
    assert_eq!(backup::send_once(&vault, &peers, dir.path(), OWNER_ID).await, Sent::Sent);

    // Cofre com senhas: recusado antes de buscar, arquivo intacto.
    let before = std::fs::read(store::path(dir.path())).unwrap();
    let refused = backup::restore(&vault, &peers, dir.path(), "lab", OWNER_ID, Zeroizing::new(RECOVERY.into())).await;
    assert_eq!(refused.err(), Some(RestoreError::NotEmpty));
    assert_eq!(std::fs::read(store::path(dir.path())).unwrap(), before);

    // Máquina nova: senha errada não cria nada.
    let fresh = tempfile::tempdir().unwrap();
    let keys = Arc::new(Keys::default());
    let restored = Vault::new(keys.clone());
    let wrong = backup::restore(&restored, &peers, fresh.path(), "lab", OWNER_ID, Zeroizing::new("errada".into())).await;
    assert_eq!(wrong.err(), Some(RestoreError::WrongPassword));
    assert!(!store::exists(&store::path(fresh.path())) && keys.recovery.lock().unwrap().is_none());
    let missing = backup::restore(&restored, &peers, fresh.path(), "lab", "outro", Zeroizing::new(RECOVERY.into())).await;
    assert_eq!(missing.err(), Some(RestoreError::NoCopy));

    backup::restore(&restored, &peers, fresh.path(), "lab", OWNER_ID, Zeroizing::new(RECOVERY.into())).await.unwrap();
    let list = restored.list(fresh.path()).await.unwrap();
    assert_eq!(list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["banco"]);
    assert_eq!(restored.password(fresh.path(), &list[0].id).await.unwrap().as_str(), PASSWORD);
    assert_eq!(keys.recovery.lock().unwrap().as_ref().map(Vec::len), Some(backup::RECOVERY_LEN), "a recuperação continua definida");
    assert!(keys.key.lock().unwrap().is_some(), "chave do cofre própria desta máquina");
}

/// O desastre que a cópia cobre: o `vault.bin` ficou e a chave sumiu do chaveiro. O arquivo vai para o
/// lado, nunca apagado, e o cofre volta da cópia com chave nova.
#[tokio::test(flavor = "multi_thread")]
async fn restore_covers_a_vault_whose_key_is_gone_and_keeps_the_old_file() {
    let (addr, _keeper_dir) = keeper().await;
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), addr);
    let (vault, _) = owner_vault(dir.path()).await;
    assert_eq!(backup::send_once(&vault, &peers, dir.path(), OWNER_ID).await, Sent::Sent);
    let orphan = std::fs::read(store::path(dir.path())).unwrap();

    // Servidor reiniciado com o item da chave apagado: o cofre de pé, sem chave.
    let keys = Arc::new(Keys::default());
    let after = Vault::new(keys.clone());
    assert!(matches!(after.list(dir.path()).await, Err(VaultError::Key(KeyError::Lost))));
    backup::restore(&after, &peers, dir.path(), "lab", OWNER_ID, Zeroizing::new(RECOVERY.into())).await.unwrap();
    assert_eq!(after.list(dir.path()).await.unwrap().iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["banco"]);
    let aside: Vec<_> = std::fs::read_dir(store::path(dir.path()).parent().unwrap()).unwrap().flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("vault.bin.perdido-")).collect();
    assert_eq!(aside.len(), 1, "o arquivo sem chave fica guardado ao lado");
    assert_eq!(std::fs::read(aside[0].path()).unwrap(), orphan);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_that_uses_another_owner_does_not_restore_a_vault_of_its_own() {
    let (addr, _keeper_dir) = keeper().await;
    let dir = tempfile::tempdir().unwrap();
    let peers = peers_for(dir.path(), addr);
    config::save(dir.path(), &config::Config { owner: Some("outra".into()), backup_peer: None }).unwrap();
    let vault = Vault::new(Arc::new(Keys::default()));
    let refused = backup::restore(&vault, &peers, dir.path(), "lab", OWNER_ID, Zeroizing::new(RECOVERY.into())).await;
    assert_eq!(refused.err(), Some(RestoreError::RemoteOwner));
    assert!(!store::exists(&store::path(dir.path())));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_keeper_route_limits_the_body_and_validates_the_request() {
    let (addr, _dir) = keeper().await;
    let http = reqwest::Client::new();
    let put = |path: &str, body: Vec<u8>| http.put(format!("http://{addr}{path}")).bearer_auth(TOKEN).body(body).send();
    let small = vec![backup::FORMAT; 64];
    let big = vec![backup::FORMAT; backup::MAX_COPY + 1];
    let resp = put("/api/vault/backup/dono?entries=1", big).await.unwrap();
    assert_eq!(resp.status().as_u16(), 413);
    assert_eq!(resp.json::<serde_json::Value>().await.unwrap()["detail"]["code"], "erro_cofre_backup_grande");
    assert_eq!(put("/api/vault/backup/dono", small.clone()).await.unwrap().status().as_u16(), 422, "sem entries");
    assert_eq!(put("/api/vault/backup/dono?entries=1", b"curto".to_vec()).await.unwrap().status().as_u16(), 422);
    assert_eq!(put("/api/vault/backup/x%2Fy?entries=1", small.clone()).await.unwrap().status().as_u16(), 422);
    let empty = http.get(format!("http://{addr}/api/vault/backup/dono")).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(empty.status().as_u16(), 404);
    let ok = put("/api/vault/backup/dono?entries=1", small.clone()).await.unwrap();
    assert_eq!(ok.json::<serde_json::Value>().await.unwrap()["server_id"], "lab");
    let got = http.get(format!("http://{addr}/api/vault/backup/dono")).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(got.bytes().await.unwrap().to_vec(), small);
    let stranger = http.get(format!("http://{addr}/api/vault/backup/dono")).send().await.unwrap();
    assert_eq!(stranger.status().as_u16(), 401);
}

#[test]
fn the_migration_table_knows_the_backup_routes() {
    use axum::http::Method;
    assert!(http::matches(&Method::POST, "/api/vault/recovery") && http::matches(&Method::POST, "/api/vault/restore"));
    assert!(http::matches(&Method::PUT, "/api/vault/backup/dono") && http::matches(&Method::GET, "/api/vault/backup/dono"));
    assert!(!http::matches(&Method::DELETE, "/api/vault/backup/dono") && !http::matches(&Method::PUT, "/api/vault/backup/"));
}
