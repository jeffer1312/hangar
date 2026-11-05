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
    /// Item da senha de recuperação (`backup::RECOVERY_LEN` bytes: sal || chave derivada); `Ok(None)` = não definida.
    fn read_recovery(&self) -> BoxFuture<'_, Result<Option<Zeroizing<Vec<u8>>>, KeyError>> { Box::pin(async { Err(KeyError::Unsupported) }) }
    fn write_recovery(&self, _item: Zeroizing<Vec<u8>>) -> BoxFuture<'_, Result<(), KeyError>> { Box::pin(async { Err(KeyError::Unsupported) }) }
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
/// arquivo depende de outra chave. Item que sobrou sem 32 bytes é trocado por uma chave nova
/// (`write` substitui o item de mesmos atributos); senão o cofre nunca nasceria.
pub async fn create(store: &dyn KeyStore) -> Result<Key, KeyError> {
    match store.read().await {
        Ok(Some(key)) => return Ok(key),
        Ok(None) | Err(KeyError::Lost) => {}
        Err(error) => return Err(error),
    }
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
            // Barramento travado não segura o estado do cofre (e a trava da conexão) para sempre.
            let conn = tokio::time::timeout(LIMIT, zbus::Connection::session()).await.map_err(|_| KeyError::Bus)?.map_err(|_| KeyError::Bus)?;
            *slot = Some(conn.clone());
            Ok(conn)
        }

        /// Só conexão caída é solta. Prazo estourado ou recusa mantêm a conexão: soltá-la com um
        /// pedido no ar faria do Hangar o cliente que sai do barramento no meio do pedido.
        async fn settle<T>(&self, result: Result<T, Fail>) -> Result<T, KeyError> {
            match result {
                Ok(value) => Ok(value),
                Err(Fail { error, dead }) => {
                    if dead {
                        tracing::warn!(code = "vault_key_bus_lost", "a conexão com o Secret Service caiu; a próxima abre outra");
                        *self.conn.lock().await = None;
                    } else if error == KeyError::Bus {
                        tracing::warn!(code = "vault_key_bus", "o Secret Service não respondeu à chave do cofre");
                    }
                    Err(error)
                }
            }
        }

        async fn open(&self) -> Result<(zbus::Connection, OwnedObjectPath), KeyError> {
            let conn = self.connection().await?;
            match tokio::time::timeout(LIMIT, open_session(&conn)).await {
                Ok(Ok(session)) => Ok((conn, session)),
                Ok(Err(fail)) => self.settle(Err(fail)).await,
                Err(_) => self.settle(Err(KeyError::Bus.into())).await,
            }
        }

        /// A sessão fecha mesmo quando a operação estourou o prazo.
        async fn finish<T>(&self, conn: &zbus::Connection, session: &OwnedObjectPath, result: Result<T, Fail>) -> Result<T, KeyError> {
            let _ = tokio::time::timeout(LIMIT, close_session(conn, session)).await;
            self.settle(result).await
        }
    }

    impl KeyStore for SecretService {
        fn read(&self) -> BoxFuture<'_, Result<Option<Key>, KeyError>> {
            Box::pin(async move {
                let Some(value) = self.read_item(VAULT_KEY).await? else { return Ok(None) };
                if value.len() != KEY_LEN { return Err(KeyError::Lost); }
                let mut key = Zeroizing::new([0u8; KEY_LEN]);
                key.copy_from_slice(&value);
                Ok(Some(key))
            })
        }

        fn write(&self, key: Key) -> BoxFuture<'_, Result<(), KeyError>> {
            Box::pin(async move { self.write_item(VAULT_KEY, "Hangar: chave do cofre de senhas", &key[..]).await })
        }

        fn read_recovery(&self) -> BoxFuture<'_, Result<Option<Zeroizing<Vec<u8>>>, KeyError>> {
            Box::pin(async move { self.read_item(RECOVERY_KEY).await })
        }

        fn write_recovery(&self, item: Zeroizing<Vec<u8>>) -> BoxFuture<'_, Result<(), KeyError>> {
            Box::pin(async move { self.write_item(RECOVERY_KEY, "Hangar: chave da cópia do cofre de senhas", &item).await })
        }
    }

    impl SecretService {
        async fn read_item(&self, kind: &'static str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyError> {
            let (conn, session) = self.open().await?;
            let found = tokio::time::timeout(LIMIT, read_secret(&conn, &session, kind)).await.unwrap_or(Err(KeyError::Bus.into()));
            self.finish(&conn, &session, found).await
        }

        async fn write_item(&self, kind: &'static str, label: &'static str, value: &[u8]) -> Result<(), KeyError> {
            let (conn, session) = self.open().await?;
            let written = tokio::time::timeout(LIMIT, write_secret(&conn, &session, kind, label, value)).await
                .unwrap_or(Err(KeyError::Bus.into()));
            self.finish(&conn, &session, written).await
        }
    }

    /// `dead`: a conexão caiu (E/S), e só então se solta.
    struct Fail { error: KeyError, dead: bool }

    impl From<KeyError> for Fail {
        fn from(error: KeyError) -> Self { Self { error, dead: false } }
    }

    fn fail(error: zbus::Error) -> Fail {
        let dead = matches!(error, zbus::Error::InputOutput(_));
        // Só o nome: o texto e a mensagem do erro não entram no log de quem lida com a chave.
        let error = match &error {
            zbus::Error::MethodError(name, ..) if name.to_string() == "org.freedesktop.Secret.Error.IsLocked" => KeyError::Locked,
            zbus::Error::MethodError(name, ..) => {
                tracing::warn!(code = "vault_key_dbus", name = %name, "o Secret Service recusou");
                KeyError::Bus
            }
            zbus::Error::InputOutput(cause) => {
                tracing::warn!(code = "vault_key_dbus", kind = ?cause.kind(), "a conexão com o Secret Service falhou");
                KeyError::Bus
            }
            _ => {
                tracing::warn!(code = "vault_key_dbus", "resposta inesperada do Secret Service");
                KeyError::Bus
            }
        };
        Fail { error, dead }
    }

    async fn proxy<'a>(conn: &zbus::Connection, path: &'a str, interface: &'a str) -> Result<zbus::Proxy<'a>, Fail> {
        build_proxy(conn, path, interface).await.map_err(fail)
    }

    async fn build_proxy<'a>(conn: &zbus::Connection, path: &'a str, interface: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
        zbus::proxy::Builder::<zbus::Proxy>::new(conn).destination(SECRETS)?.path(path)?.interface(interface)?
            .cache_properties(zbus::proxy::CacheProperties::No).build().await
    }

    /// Sem dono do nome não pergunta: perguntar subiria um daemon com o `login` trancado.
    async fn open_session(conn: &zbus::Connection) -> Result<OwnedObjectPath, Fail> {
        let bus = zbus::fdo::DBusProxy::new(conn).await.map_err(fail)?;
        let name = zbus::names::BusName::try_from(SECRETS).map_err(|_| Fail::from(KeyError::Bus))?;
        if !bus.name_has_owner(name).await.map_err(|e| fail(e.into()))? { return Err(KeyError::Bus.into()); }
        let service = proxy(conn, SERVICE, "org.freedesktop.Secret.Service").await?;
        let reply = service.call_with_flags::<_, _, (OwnedValue, OwnedObjectPath)>("OpenSession",
            zbus::proxy::MethodFlags::NoAutoStart.into(), &("plain", Value::from(""))).await.map_err(fail)?;
        Ok(reply.map(|(_, session)| session).ok_or(KeyError::Bus)?)
    }

    async fn close_session(conn: &zbus::Connection, session: &OwnedObjectPath) {
        if let Ok(proxy) = proxy(conn, session.as_str(), "org.freedesktop.Secret.Session").await {
            let _ = proxy.call_with_flags::<_, _, ()>("Close", zbus::proxy::MethodFlags::NoAutoStart.into(), &()).await;
        }
    }

    const VAULT_KEY: &str = "vault-key";
    const RECOVERY_KEY: &str = "vault-backup-key";

    fn attributes(kind: &'static str) -> HashMap<&'static str, &'static str> {
        HashMap::from([("application", "hangar"), ("kind", kind)])
    }

    async fn read_secret(conn: &zbus::Connection, session: &OwnedObjectPath, kind: &'static str) -> Result<Option<Zeroizing<Vec<u8>>>, Fail> {
        let service = proxy(conn, SERVICE, "org.freedesktop.Secret.Service").await?;
        let flags = zbus::proxy::MethodFlags::NoAutoStart.into();
        let (unlocked, locked) = service.call_with_flags::<_, _, (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>(
            "SearchItems", flags, &(attributes(kind),)).await.map_err(fail)?.ok_or(KeyError::Bus)?;
        let in_login = |items: Vec<OwnedObjectPath>| -> Vec<OwnedObjectPath> {
            let mut items: Vec<_> = items.into_iter().filter(|p| p.as_str().starts_with(&format!("{LOGIN}/"))).collect();
            items.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            items
        };
        let (unlocked, locked) = (in_login(unlocked), in_login(locked));
        let Some(item) = unlocked.into_iter().next() else {
            return if locked.is_empty() { Ok(None) } else { Err(KeyError::Locked.into()) };
        };
        let secrets = service.call_with_flags::<_, _, HashMap<OwnedObjectPath, Secret>>(
            "GetSecrets", flags, &(vec![item.clone()], session)).await.map_err(fail)?.ok_or(KeyError::Bus)?;
        let mut found = None;
        for (path, (_, _, value, _)) in secrets {
            let value = Zeroizing::new(value);
            if path == item { found = Some(value); }
        }
        Ok(Some(found.ok_or(KeyError::Bus)?))
    }

    /// `replace`: um item que sobrou com os mesmos atributos (sem 32 bytes) dá lugar à chave nova.
    async fn write_secret(conn: &zbus::Connection, session: &OwnedObjectPath, kind: &'static str, label: &'static str, value: &[u8])
        -> Result<(), Fail> {
        let collection = proxy(conn, LOGIN, "org.freedesktop.Secret.Collection").await?;
        let properties: HashMap<&str, Value> = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from(label)),
            ("org.freedesktop.Secret.Item.Attributes", Value::from(attributes(kind))),
        ]);
        let value = Zeroizing::new(value.to_vec());
        let secret = (session, Vec::<u8>::new(), &*value, "application/octet-stream");
        let (_, prompt) = collection.call_with_flags::<_, _, (OwnedObjectPath, OwnedObjectPath)>("CreateItem",
            zbus::proxy::MethodFlags::NoAutoStart.into(), &(properties, secret, true)).await
            .map_err(fail)?.ok_or(KeyError::Bus)?;
        // Prompt = coleção trancada; sem display ele não abre, e quem destranca é o aviso do chaveiro.
        if prompt.as_str() != "/" { return Err(KeyError::Locked.into()); }
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
