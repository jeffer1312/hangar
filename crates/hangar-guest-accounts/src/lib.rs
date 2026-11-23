//! Contas de convidado que esta máquina guarda: `~/.hangar/guest-accounts.json` (0600).
//!
//! A máquina deriva as chaves da senha com o esquema do cofre de sincronização dos clientes e guarda a
//! chave do cofre de cada convidado, para recifrar a lista de máquinas dele sem pedir a senha de novo.
//! O arquivo tem os tokens de convidado de cada máquina: não sai daqui (configuração compartilhada,
//! diagnóstico).
#![deny(unsafe_code)]

mod crypto;

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::watch;

pub use crypto::{Blob, ITERATIONS, Keys, decrypt, derive_keys, user_hash, verifier};
/// O nome como o arquivo e o serviço o guardam: a mesma regra dos clientes, que mandam o nome ao serviço.
pub use hangar_connect_client::normalize_name as normalize;

const MAX_SERVERS: usize = 64;
const MAX_PASSWORD: usize = 1024;

pub fn path(home: &Path) -> PathBuf {
    home.join(".hangar").join("guest-accounts.json")
}

/// O `.` ou `@` dos clientes ([`hangar_connect_client::valid_guest_user`]), que conta do Connect não aceita: com um
/// deles, o nome nunca colide com uma conta. O arquivo guarda ainda só ASCII minúsculo, de 3 a 64.
pub fn valid_user(user: &str) -> bool {
    (3..=64).contains(&user.len())
        && user.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._@+-".contains(&b))
        && hangar_connect_client::valid_guest_user(user)
}

#[derive(Debug)]
pub enum Error {
    /// Campo do pedido fora da forma; o nome do campo vai na resposta.
    Invalid(&'static str),
    PasswordRequired,
    NotFound,
    Unreadable,
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Save {
    pub user: String,
    /// Obrigatória ao criar; vazia ou ausente ao alterar mantém a senha.
    #[serde(default)]
    pub password: Option<String>,
    pub sees_owner: bool,
    pub owner_sees: bool,
    pub servers: Vec<ServerInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerInput {
    pub server_id: String,
    pub label: String,
    /// Ausente numa máquina que a conta já tinha: fica o gravado.
    #[serde(default)]
    pub base_url: Option<String>,
    pub guest_id: String,
    /// A listagem não devolve tokens: ausente numa máquina já gravada, com o mesmo `guest_id`, fica o gravado.
    #[serde(default)]
    pub token: Option<String>,
    pub root: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub user: String,
    pub sees_owner: bool,
    pub owner_sees: bool,
    pub servers: Vec<ServerSummary>,
    /// O serviço do Connect recusou o nome: ele já é de outra conta.
    pub name_taken: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServerSummary {
    pub server_id: String,
    pub label: String,
    pub base_url: String,
    pub guest_id: String,
    pub root: String,
}

/// O que confere a senha de uma conta. Quem chama faz a derivação, uma só por tentativa, exista o nome ou não.
pub struct Credential {
    pub user: String,
    pub verifier_salt: [u8; 16],
    /// `PBKDF2-SHA256(auth_hash, verifier_salt, 200 mil)` em base64, como no `sync-vault.json`.
    pub auth_verifier: String,
}

impl Credential {
    pub fn generation(&self) -> String {
        generation(&self.verifier_salt)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Doc {
    /// Base64 de 32 bytes: deriva o sal falso de nome inexistente.
    secret: String,
    #[serde(default)]
    accounts: BTreeMap<String, Account>,
    #[serde(default)]
    taken: BTreeSet<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Account {
    salt: String,
    verifier_salt: String,
    auth_verifier: String,
    enc_key: String,
    enc_blob: Blob,
    sees_owner: bool,
    owner_sees: bool,
    servers: Vec<Server>,
    rev: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Server {
    server_id: String,
    label: String,
    base_url: String,
    guest_id: String,
    token: String,
    root: String,
}

/// Data de modificação e tamanho: o arquivo lido continua o mesmo enquanto os dois não mudam.
type Stamp = (SystemTime, u64);

pub struct Store {
    path: PathBuf,
    lock: Mutex<()>,
    /// A última leitura, para um pedido que consulta o arquivo várias vezes não o ler a cada uma.
    cache: Mutex<Option<(Stamp, Arc<Doc>)>>,
    /// Conta gravada ou apagada; quem publica o diretório no serviço segue este sinal.
    changed: watch::Sender<u64>,
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self { path, lock: Mutex::new(()), cache: Mutex::new(None), changed: watch::channel(0).0 }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Muda a cada conta gravada ou apagada por este processo.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn cached(&self) -> MutexGuard<'_, Option<(Stamp, Arc<Doc>)>> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `None` quando o sistema não diz: aí o arquivo é lido de novo, sem guardar.
    fn stamp(&self) -> Option<Stamp> {
        let meta = std::fs::metadata(&self.path).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    /// Ausente é `None`; ilegível é erro, nunca arquivo vazio: a próxima gravação apagaria as contas.
    /// A data é lida antes do conteúdo: troca no meio deixa a data velha guardada, e a próxima leitura relê.
    fn load(&self) -> Result<Option<Arc<Doc>>, Error> {
        let stamp = self.stamp();
        if let Some((seen, doc)) = &*self.cached() && Some(*seen) == stamp {
            return Ok(Some(doc.clone()));
        }
        let doc = match hangar_core::fs::read_replaced(&self.path) {
            Ok(raw) => serde_json::from_slice::<Doc>(&raw).ok().filter(|doc| crypto::decode::<32>(&doc.secret).is_some())
                .map(Arc::new).ok_or(Error::Unreadable)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Error::Io(error)),
        };
        if let Some(stamp) = stamp {
            *self.cached() = Some((stamp, doc.clone()));
        }
        Ok(Some(doc))
    }

    /// O que vai ser alterado: cópia da leitura, nunca a guardada.
    fn load_owned(&self) -> Result<Option<Doc>, Error> {
        Ok(self.load()?.map(Arc::unwrap_or_clone))
    }

    fn write(&self, doc: Doc) -> Result<(), Error> {
        let parent = self.path.parent().ok_or_else(|| io::Error::other("arquivo sem pasta"))?;
        std::fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(&doc).map_err(io::Error::other)?;
        *self.cached() = None;
        hangar_core::fs::atomic_write(&self.path, &bytes)?;
        // `0600` não protege no Windows: o arquivo fica só para o usuário.
        #[cfg(windows)]
        hangar_core::private_acl::restrict_path(&self.path, "")?;
        if let Some(stamp) = self.stamp() {
            *self.cached() = Some((stamp, Arc::new(doc)));
        }
        Ok(())
    }

    fn announce(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Há conta de convidado: o login dela vale sem a Sincronização ligada.
    pub fn any(&self) -> Result<bool, Error> {
        Ok(self.load()?.is_some_and(|doc| !doc.accounts.is_empty()))
    }

    pub fn contains(&self, user: &str) -> Result<bool, Error> {
        Ok(self.load()?.is_some_and(|doc| doc.accounts.contains_key(&normalize(user))))
    }

    pub fn list(&self) -> Result<Vec<Summary>, Error> {
        let Some(doc) = self.load()? else { return Ok(Vec::new()) };
        Ok(doc.accounts.iter().map(|(user, account)| summary(user, account, &doc.taken)).collect())
    }

    pub fn save(&self, input: Save) -> Result<Summary, Error> {
        let user = normalize(&input.user);
        if !valid_user(&user) {
            return Err(Error::Invalid("user"));
        }
        let password = input.password.filter(|p| !p.is_empty());
        if password.as_ref().is_some_and(|p| p.len() > MAX_PASSWORD) {
            return Err(Error::Invalid("password"));
        }
        if input.servers.len() > MAX_SERVERS {
            return Err(Error::Invalid("servers"));
        }
        // Fora da trava: as 800 mil voltas da senha nova não seguram as outras gravações.
        let fresh = match password {
            Some(password) => {
                let salt = crypto::random::<16>()?;
                let keys = derive_keys(&password, &salt, ITERATIONS);
                // Sal do verificador novo: a geração dos cookies muda e as sessões da senha antiga caem.
                let verifier_salt = crypto::random::<16>()?;
                Some((crypto::encode(&salt), crypto::encode(&verifier_salt), verifier(&keys.auth_hash, &verifier_salt), keys.enc_key))
            }
            None => None,
        };
        let _guard = self.guard();
        let mut doc = match self.load_owned()? {
            Some(doc) => doc,
            None => Doc { secret: crypto::encode(&crypto::random::<32>()?), accounts: BTreeMap::new(), taken: BTreeSet::new() },
        };
        let previous = doc.accounts.get(&user);
        let servers = merge_servers(input.servers, previous)?;
        let (salt, verifier_salt, auth_verifier, enc_key) = match (fresh, previous) {
            (Some(fresh), _) => fresh,
            (None, Some(previous)) => {
                let key = crypto::decode::<32>(&previous.enc_key).ok_or(Error::Unreadable)?;
                (previous.salt.clone(), previous.verifier_salt.clone(), previous.auth_verifier.clone(), key)
            }
            (None, None) => return Err(Error::PasswordRequired),
        };
        let list: Vec<_> = servers.iter()
            .map(|s| json!({"id": s.server_id, "label": s.label, "baseUrl": s.base_url, "token": s.token})).collect();
        let enc_blob = crypto::encrypt(&enc_key, &serde_json::to_vec(&list).map_err(io::Error::other)?)?;
        let rev = previous.map_or(1, |p| p.rev.saturating_add(1));
        let account = Account { salt, verifier_salt, auth_verifier, enc_key: crypto::encode(&enc_key), enc_blob,
            sees_owner: input.sees_owner, owner_sees: input.owner_sees, servers, rev };
        let result = summary(&user, &account, &doc.taken);
        doc.accounts.insert(user, account);
        self.write(doc)?;
        self.announce();
        Ok(result)
    }

    pub fn remove(&self, user: &str) -> Result<(), Error> {
        let user = normalize(user);
        let _guard = self.guard();
        let mut doc = self.load_owned()?.ok_or(Error::NotFound)?;
        doc.accounts.remove(&user).ok_or(Error::NotFound)?;
        doc.taken.remove(&user);
        // O arquivo fica, mesmo vazio: o segredo dele mantém o sal falso igual.
        self.write(doc)?;
        self.announce();
        Ok(())
    }

    /// Os hashes que o serviço guarda. `None` = nunca houve conta aqui: nada a publicar.
    pub fn user_hashes(&self) -> Result<Option<Vec<String>>, Error> {
        Ok(self.load()?.map(|doc| doc.accounts.keys().map(|user| user_hash(user)).collect()))
    }

    /// Guarda os nomes que o serviço recusou, pela resposta de `PUT /v1/guests`.
    pub fn mark_taken(&self, hashes: &[String]) -> Result<(), Error> {
        let _guard = self.guard();
        let Some(mut doc) = self.load_owned()? else { return Ok(()) };
        let taken: BTreeSet<String> = doc.accounts.keys().filter(|user| hashes.contains(&user_hash(user))).cloned().collect();
        if taken == doc.taken {
            return Ok(());
        }
        doc.taken = taken;
        self.write(doc)
    }

    /// `(sal, conta existe)`. Nome inexistente recebe um sal falso estável, do segredo do arquivo.
    pub fn prelogin(&self, user: &str) -> Result<Option<(String, bool)>, Error> {
        let user = normalize(user);
        let Some(doc) = self.load()? else { return Ok(None) };
        if let Some(account) = doc.accounts.get(&user) {
            return Ok(Some((account.salt.clone(), true)));
        }
        let secret = crypto::decode::<32>(&doc.secret).ok_or(Error::Unreadable)?;
        Ok(Some((crypto::fake_salt(&secret, &user), false)))
    }

    pub fn credential(&self, user: &str) -> Result<Option<Credential>, Error> {
        let user = normalize(user);
        let Some(doc) = self.load()? else { return Ok(None) };
        let Some(account) = doc.accounts.get(&user) else { return Ok(None) };
        let verifier_salt = crypto::decode::<16>(&account.verifier_salt).ok_or(Error::Unreadable)?;
        Ok(Some(Credential { verifier_salt, auth_verifier: account.auth_verifier.clone(), user }))
    }

    /// A geração do cookie: muda quando a senha muda.
    pub fn generation(&self, user: &str) -> Result<Option<String>, Error> {
        Ok(self.credential(user)?.map(|credential| credential.generation()))
    }

    pub fn vault(&self, user: &str) -> Result<Option<(Blob, u64)>, Error> {
        let Some(doc) = self.load()? else { return Ok(None) };
        Ok(doc.accounts.get(&normalize(user)).map(|account| (account.enc_blob.clone(), account.rev)))
    }
}

/// A geração do cookie, a mesma no cofre antigo: os quatro primeiros bytes do sal do verificador.
pub fn generation(verifier_salt: &[u8]) -> String {
    verifier_salt.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn summary(user: &str, account: &Account, taken: &BTreeSet<String>) -> Summary {
    Summary {
        user: user.to_owned(),
        sees_owner: account.sees_owner,
        owner_sees: account.owner_sees,
        servers: account.servers.iter().map(|s| ServerSummary { server_id: s.server_id.clone(), label: s.label.clone(),
            base_url: s.base_url.clone(), guest_id: s.guest_id.clone(), root: s.root.clone() }).collect(),
        name_taken: taken.contains(user),
    }
}

fn text(value: String, field: &'static str, min: usize, max: usize) -> Result<String, Error> {
    let value = value.trim().to_owned();
    if value.chars().count() < min || value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(Error::Invalid(field));
    }
    Ok(value)
}

fn merge_servers(input: Vec<ServerInput>, previous: Option<&Account>) -> Result<Vec<Server>, Error> {
    let mut seen = BTreeSet::new();
    input.into_iter().map(|server| {
        let server_id = text(server.server_id, "server_id", 1, 200)?;
        if !seen.insert(server_id.clone()) {
            return Err(Error::Invalid("servers"));
        }
        let guest_id = text(server.guest_id, "guest_id", 1, 200)?;
        let old = previous.and_then(|p| p.servers.iter().find(|o| o.server_id == server_id && o.guest_id == guest_id));
        let token = match server.token.filter(|t| !t.trim().is_empty()) {
            Some(token) => text(token, "token", 1, 512)?,
            None => old.map(|o| o.token.clone()).ok_or(Error::Invalid("token"))?,
        };
        let base_url = match server.base_url.filter(|u| !u.trim().is_empty()) {
            Some(url) => text(url, "base_url", 1, 2048)?,
            None => old.map(|o| o.base_url.clone()).ok_or(Error::Invalid("base_url"))?,
        };
        if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
            return Err(Error::Invalid("base_url"));
        }
        Ok(Server { server_id, label: text(server.label, "label", 1, 200)?, base_url, guest_id, token,
            root: text(server.root, "root", 1, 4096)? })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(path(dir.path()));
        (dir, store)
    }

    fn server(token: Option<&str>) -> ServerInput {
        ServerInput { server_id: "srv-casa".into(), label: "Casa".into(), base_url: Some("https://casa.ana.hangar.dev.br".into()),
            guest_id: "g1".into(), token: token.map(Into::into), root: "/home/ana/projetos".into() }
    }

    fn save(user: &str, password: Option<&str>, servers: Vec<ServerInput>) -> Save {
        Save { user: user.into(), password: password.map(Into::into), sees_owner: false, owner_sees: true, servers }
    }

    fn open(store: &Store, user: &str, password: &str) -> serde_json::Value {
        let (salt, known) = store.prelogin(user).unwrap().unwrap();
        assert!(known);
        let salt = crypto::decode::<16>(&salt).unwrap();
        let keys = derive_keys(password, &salt, ITERATIONS);
        let credential = store.credential(user).unwrap().unwrap();
        assert_eq!(crypto::verifier(&keys.auth_hash, &credential.verifier_salt), credential.auth_verifier, "senha certa recusada");
        let (blob, _) = store.vault(user).unwrap().unwrap();
        serde_json::from_slice(&decrypt(&keys.enc_key, &blob).unwrap()).unwrap()
    }

    #[test]
    fn created_account_opens_with_the_guest_password_and_lists_without_secrets() {
        let (_dir, store) = store();
        let saved = store.save(save("Lhais87.Oliveira", Some("senha-dela"), vec![server(Some("tok-1"))])).unwrap();
        assert_eq!(saved.user, "lhais87.oliveira", "nome guardado em minúsculas");
        let list = open(&store, "lhais87.oliveira", "senha-dela");
        assert_eq!(list, json!([{"id":"srv-casa","label":"Casa","baseUrl":"https://casa.ana.hangar.dev.br","token":"tok-1"}]));
        let listed = serde_json::to_string(&store.list().unwrap()).unwrap();
        assert!(!listed.contains("tok-1") && !listed.contains("enc_key"), "listagem sem token nem chave");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(store.path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn edit_without_password_keeps_password_and_stored_token() {
        let (_dir, store) = store();
        store.save(save("ana.silva", Some("primeira"), vec![server(Some("tok-1"))])).unwrap();
        let generation = store.generation("ana.silva").unwrap();
        let mut edited = server(None);
        edited.root = "/home/ana/outra".into();
        edited.base_url = None;
        store.save(save("ana.silva", None, vec![edited])).unwrap();
        assert_eq!(open(&store, "ana.silva", "primeira")[0]["token"], "tok-1");
        assert_eq!(store.generation("ana.silva").unwrap(), generation, "sessões continuam valendo");
        store.save(save("ana.silva", Some("segunda"), vec![server(Some("tok-2"))])).unwrap();
        assert_ne!(store.generation("ana.silva").unwrap(), generation, "senha nova derruba as sessões antigas");
        assert_eq!(open(&store, "ana.silva", "segunda")[0]["token"], "tok-2");
    }

    #[test]
    fn creation_requires_password_and_a_name_with_dot_or_at() {
        let (_dir, store) = store();
        assert!(matches!(store.save(save("ana.silva", None, vec![])), Err(Error::PasswordRequired)));
        assert!(matches!(store.save(save("ana.silva", Some(""), vec![])), Err(Error::PasswordRequired)));
        for bad in ["anasilva", "ana silva.x", "a.", "ana/x.y", "ána.silva"] {
            assert!(matches!(store.save(save(bad, Some("x"), vec![])), Err(Error::Invalid("user"))), "{bad}");
        }
        assert!(matches!(store.save(save("ana@casa", Some("x"), vec![server(None)])), Err(Error::Invalid("token"))));
    }

    #[test]
    fn a_server_label_needs_a_visible_character() {
        let (_dir, store) = store();
        for blank in ["", "   ", "\t"] {
            let row = ServerInput { label: blank.into(), ..server(Some("tok-1")) };
            assert!(matches!(store.save(save("ana.silva", Some("x"), vec![row])), Err(Error::Invalid("label"))), "{blank:?}");
        }
        let row = ServerInput { label: "  Casa ".into(), ..server(Some("tok-1")) };
        store.save(save("ana.silva", Some("x"), vec![row])).unwrap();
        assert_eq!(open(&store, "ana.silva", "x")[0]["label"], "Casa");
    }

    #[test]
    fn unknown_name_gets_a_stable_fake_salt() {
        let (_dir, store) = store();
        assert!(store.prelogin("ninguem.aqui").unwrap().is_none(), "sem arquivo quem decide é o cofre antigo");
        store.save(save("ana.silva", Some("x"), vec![])).unwrap();
        let (first, known) = store.prelogin("Ninguem.Aqui").unwrap().unwrap();
        assert!(!known);
        assert_eq!(store.prelogin("ninguem.aqui").unwrap().unwrap().0, first);
        assert_ne!(store.prelogin("outro.nome").unwrap().unwrap().0, first);
        assert_eq!(crypto::decode::<16>(&first).map(|b| b.len()), Some(16));
        assert!(store.credential("ninguem.aqui").unwrap().is_none());
    }

    #[test]
    fn removal_keeps_the_secret_and_taken_follows_the_service_answer() {
        let (_dir, store) = store();
        store.save(save("ana.silva", Some("x"), vec![])).unwrap();
        store.save(save("bia@casa", Some("y"), vec![])).unwrap();
        store.mark_taken(&[user_hash("bia@casa")]).unwrap();
        let taken: Vec<_> = store.list().unwrap().into_iter().map(|s| (s.user, s.name_taken)).collect();
        assert_eq!(taken, vec![("ana.silva".into(), false), ("bia@casa".into(), true)]);
        let fake = store.prelogin("zz.zz").unwrap().unwrap().0;
        store.remove("BIA@casa").unwrap();
        store.remove("ana.silva").unwrap();
        assert!(matches!(store.remove("ana.silva"), Err(Error::NotFound)));
        assert!(!store.any().unwrap());
        assert_eq!(store.user_hashes().unwrap(), Some(vec![]), "lista vazia ainda é publicada");
        assert_eq!(store.prelogin("zz.zz").unwrap().unwrap().0, fake);
    }

    #[test]
    fn writes_are_announced_and_a_file_changed_elsewhere_is_read_again() {
        let (_dir, store) = store();
        let changes = store.changes();
        store.save(save("ana.silva", Some("x"), vec![])).unwrap();
        assert!(changes.has_changed().unwrap(), "conta gravada avisa");
        assert_eq!(store.list().unwrap().len(), 1);
        std::fs::write(store.path(), b"{nao-e-json").unwrap();
        assert!(matches!(store.list(), Err(Error::Unreadable)), "a leitura guardada não esconde o arquivo trocado");
    }

    #[test]
    fn unreadable_file_is_an_error_not_an_empty_list() {
        let (_dir, store) = store();
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), b"{nao-e-json").unwrap();
        assert!(matches!(store.list(), Err(Error::Unreadable)));
        assert!(matches!(store.save(save("ana.silva", Some("x"), vec![])), Err(Error::Unreadable)));
    }
}
