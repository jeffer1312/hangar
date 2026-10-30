//! Chave do cofre: 32 bytes num item da coleção `login` do Secret Service, a mesma que
//! `keyring.rs` vigia e destranca. Chave nova só quando o `vault.bin` não existe: com o arquivo de
//! pé e o item sumido, a chave está perdida, e uma nova esconderia isso cifrando outro cofre vazio.
use futures_util::future::BoxFuture;
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;
pub type Key = Zeroizing<[u8; KEY_LEN]>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// A coleção `login` está trancada: abre pelo aviso do chaveiro.
    Locked,
    /// Sem item e sem `vault.bin`: ainda não há cofre.
    Absent,
    /// `vault.bin` existe e o item não (ou o item não tem 32 bytes).
    Lost,
    /// O Secret Service não respondeu ou recusou por outro motivo.
    Bus,
    /// Sistema sem chaveiro para o cofre (fora do Linux).
    Unsupported,
}

/// Onde a chave mora; o teste troca por um falso.
pub trait KeyStore: Send + Sync {
    /// `Ok(None)` = nenhum item da chave.
    fn read(&self) -> BoxFuture<'_, Result<Option<Key>, KeyError>>;
    fn write(&self, key: Key) -> BoxFuture<'_, Result<(), KeyError>>;
}

/// A chave de um cofre que pode existir: sem item, `Lost` se o `vault.bin` existe.
pub async fn open(store: &dyn KeyStore, vault_exists: bool) -> Result<Key, KeyError> {
    match store.read().await? {
        Some(key) => Ok(key),
        None if vault_exists => Err(KeyError::Lost),
        None => Err(KeyError::Absent),
    }
}

/// Só para o cofre que ainda não existe. Item que sobrou de um cofre apagado é reaproveitado: nenhum
/// arquivo depende de outra chave.
pub async fn create(store: &dyn KeyStore) -> Result<Key, KeyError> {
    if let Some(key) = store.read().await? { return Ok(key); }
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut key[..]).map_err(|_| KeyError::Bus)?;
    store.write(key.clone()).await?;
    Ok(key)
}

#[cfg(target_os = "linux")]
pub use linux::SecretService;

#[cfg(target_os = "linux")]
mod linux {
    use super::{Key, KeyError, KeyStore, KEY_LEN};
    use futures_util::future::BoxFuture;
    use std::{collections::HashMap, time::Duration};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    use zeroize::Zeroizing;

    const SECRETS: &str = "org.freedesktop.secrets";
    const SERVICE: &str = "/org/freedesktop/secrets";
    const LOGIN: &str = "/org/freedesktop/secrets/collection/login";
    const LIMIT: Duration = Duration::from_secs(10);

    type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

    /// Uma conexão que vive com o servidor. Cliente que sai do barramento no meio de um pedido é o
    /// que `keyring.rs` diz derrubar o gnome-keyring-daemon; com a conexão de pé isso não acontece.
    #[derive(Default)]
    pub struct SecretService { conn: tokio::sync::Mutex<Option<zbus::Connection>> }

    impl SecretService {
        async fn connection(&self) -> Result<zbus::Connection, KeyError> {
            let mut slot = self.conn.lock().await;
            if let Some(conn) = slot.as_ref() { return Ok(conn.clone()); }
            let conn = zbus::Connection::session().await.map_err(|_| KeyError::Bus)?;
            *slot = Some(conn.clone());
            Ok(conn)
        }

        /// Erro de barramento solta a conexão: a próxima chamada abre outra.
        async fn settle<T>(&self, result: Result<T, KeyError>) -> Result<T, KeyError> {
            if result.as_ref().err() == Some(&KeyError::Bus) {
                tracing::warn!(code = "vault_key_bus", "o Secret Service não respondeu à chave do cofre");
                *self.conn.lock().await = None;
            }
            result
        }
    }

    impl KeyStore for SecretService {
        fn read(&self) -> BoxFuture<'_, Result<Option<Key>, KeyError>> {
            Box::pin(async move {
                let conn = self.connection().await?;
                let found = tokio::time::timeout(LIMIT, async {
                    let session = open_session(&conn).await?;
                    let found = read_key(&conn, &session).await;
                    close_session(&conn, &session).await;
                    found
                }).await.unwrap_or(Err(KeyError::Bus));
                self.settle(found).await
            })
        }

        fn write(&self, key: Key) -> BoxFuture<'_, Result<(), KeyError>> {
            Box::pin(async move {
                let conn = self.connection().await?;
                let written = tokio::time::timeout(LIMIT, async {
                    let session = open_session(&conn).await?;
                    let written = write_key(&conn, &session, &key).await;
                    close_session(&conn, &session).await;
                    written
                }).await.unwrap_or(Err(KeyError::Bus));
                self.settle(written).await
            })
        }
    }

    fn classify(error: &zbus::Error) -> KeyError {
        match error {
            zbus::Error::MethodError(name, ..) if name.to_string() == "org.freedesktop.Secret.Error.IsLocked" => KeyError::Locked,
            _ => KeyError::Bus,
        }
    }

    async fn proxy<'a>(conn: &zbus::Connection, path: &'a str, interface: &'a str) -> Result<zbus::Proxy<'a>, KeyError> {
        build_proxy(conn, path, interface).await.map_err(|_| KeyError::Bus)
    }

    async fn build_proxy<'a>(conn: &zbus::Connection, path: &'a str, interface: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
        zbus::proxy::Builder::<zbus::Proxy>::new(conn).destination(SECRETS)?.path(path)?.interface(interface)?
            .cache_properties(zbus::proxy::CacheProperties::No).build().await
    }

    /// Sem dono do nome não pergunta: perguntar subiria um daemon com o `login` trancado.
    async fn open_session(conn: &zbus::Connection) -> Result<OwnedObjectPath, KeyError> {
        let bus = zbus::fdo::DBusProxy::new(conn).await.map_err(|_| KeyError::Bus)?;
        let name = zbus::names::BusName::try_from(SECRETS).map_err(|_| KeyError::Bus)?;
        if !bus.name_has_owner(name).await.map_err(|_| KeyError::Bus)? { return Err(KeyError::Bus); }
        let service = proxy(conn, SERVICE, "org.freedesktop.Secret.Service").await?;
        let reply = service.call_with_flags::<_, _, (OwnedValue, OwnedObjectPath)>("OpenSession",
            zbus::proxy::MethodFlags::NoAutoStart.into(), &("plain", Value::from(""))).await.map_err(|e| classify(&e))?;
        reply.map(|(_, session)| session).ok_or(KeyError::Bus)
    }

    async fn close_session(conn: &zbus::Connection, session: &OwnedObjectPath) {
        if let Ok(proxy) = proxy(conn, session.as_str(), "org.freedesktop.Secret.Session").await {
            let _ = proxy.call_with_flags::<_, _, ()>("Close", zbus::proxy::MethodFlags::NoAutoStart.into(), &()).await;
        }
    }

    fn attributes() -> HashMap<&'static str, &'static str> {
        HashMap::from([("application", "hangar"), ("kind", "vault-key")])
    }

    async fn read_key(conn: &zbus::Connection, session: &OwnedObjectPath) -> Result<Option<Key>, KeyError> {
        let service = proxy(conn, SERVICE, "org.freedesktop.Secret.Service").await?;
        let flags = zbus::proxy::MethodFlags::NoAutoStart.into();
        let (unlocked, locked) = service.call_with_flags::<_, _, (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>(
            "SearchItems", flags, &(attributes(),)).await.map_err(|e| classify(&e))?.ok_or(KeyError::Bus)?;
        let in_login = |items: Vec<OwnedObjectPath>| -> Vec<OwnedObjectPath> {
            let mut items: Vec<_> = items.into_iter().filter(|p| p.as_str().starts_with(&format!("{LOGIN}/"))).collect();
            items.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            items
        };
        let (unlocked, locked) = (in_login(unlocked), in_login(locked));
        let Some(item) = unlocked.into_iter().next() else {
            return if locked.is_empty() { Ok(None) } else { Err(KeyError::Locked) };
        };
        let secrets = service.call_with_flags::<_, _, HashMap<OwnedObjectPath, Secret>>(
            "GetSecrets", flags, &(vec![item.clone()], session)).await.map_err(|e| classify(&e))?.ok_or(KeyError::Bus)?;
        let mut found = None;
        for (path, (_, _, value, _)) in secrets {
            let value = Zeroizing::new(value);
            if path == item { found = Some(value); }
        }
        let value = found.ok_or(KeyError::Bus)?;
        if value.len() != KEY_LEN { return Err(KeyError::Lost); }
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        key.copy_from_slice(&value);
        Ok(Some(key))
    }

    async fn write_key(conn: &zbus::Connection, session: &OwnedObjectPath, key: &Key) -> Result<(), KeyError> {
        let collection = proxy(conn, LOGIN, "org.freedesktop.Secret.Collection").await?;
        let properties: HashMap<&str, Value> = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from("Hangar: chave do cofre de senhas")),
            ("org.freedesktop.Secret.Item.Attributes", Value::from(attributes())),
        ]);
        let value = Zeroizing::new(key.to_vec());
        let secret = (session, Vec::<u8>::new(), &*value, "application/octet-stream");
        let (_, prompt) = collection.call_with_flags::<_, _, (OwnedObjectPath, OwnedObjectPath)>("CreateItem",
            zbus::proxy::MethodFlags::NoAutoStart.into(), &(properties, secret, false)).await
            .map_err(|e| classify(&e))?.ok_or(KeyError::Bus)?;
        // Prompt = coleção trancada; sem display ele não abre, e quem destranca é o aviso do chaveiro.
        if prompt.as_str() != "/" { return Err(KeyError::Locked); }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Default)]
pub struct SecretService;

#[cfg(not(target_os = "linux"))]
impl KeyStore for SecretService {
    fn read(&self) -> BoxFuture<'_, Result<Option<Key>, KeyError>> { Box::pin(async { Err(KeyError::Unsupported) }) }
    fn write(&self, _key: Key) -> BoxFuture<'_, Result<(), KeyError>> { Box::pin(async { Err(KeyError::Unsupported) }) }
}
