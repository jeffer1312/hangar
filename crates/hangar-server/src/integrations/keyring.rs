//! Chaveiro isolado das integrações MCP (Secret Service no Linux, CredMan no Windows).
//!
//! Totalmente isolado do chaveiro e do armazenamento do Cofre de senhas.

use std::collections::HashMap;
use std::sync::Arc;

use hangar_integrations::secrets::SecretKeyStore;

const INTEGRATIONS_KIND: &str = "integrations-key";
const LINUX_LABEL: &str = "Hangar — Integrações MCP";
#[cfg(windows)]
const WINDOWS_TARGET_PREFIX: &str = "hangar";

/// Atributos Secret Service exclusivos das integrações MCP no Linux.
pub(crate) fn linux_attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([("application", "hangar"), ("kind", INTEGRATIONS_KIND)])
}

/// Rótulo visível no chaveiro do Linux para a chave das integrações MCP.
pub(crate) fn linux_label() -> &'static str {
    LINUX_LABEL
}

/// Nome alvo no Gerenciador de Credenciais do Windows.
pub(crate) fn windows_target() -> &'static str {
    "hangar/integrations-key"
}

/// Chaveiro padrão desta plataforma para as integrações MCP.
pub fn system() -> Arc<dyn SecretKeyStore> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::SecretService::default())
    }
    #[cfg(windows)]
    {
        Arc::new(credman::CredentialManager::default())
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Arc::new(NoKeyring)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::time::Duration;

    use futures_util::future::BoxFuture;
    use hangar_integrations::secrets::{SECRET_KEY_LEN, SecretKey, SecretKeyError, SecretKeyStore};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    use zeroize::Zeroizing;

    use super::{linux_attributes, linux_label};

    const SECRETS: &str = "org.freedesktop.secrets";
    const SERVICE: &str = "/org/freedesktop/secrets";
    const LOGIN: &str = "/org/freedesktop/secrets/collection/login";
    const LIMIT: Duration = Duration::from_secs(10);

    type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

    /// Conexão D-Bus reutilizável com o Secret Service (descartada somente em falha de E/S).
    #[derive(Default)]
    pub(super) struct SecretService {
        conn: tokio::sync::Mutex<Option<zbus::Connection>>,
    }

    impl SecretService {
        async fn connection(&self) -> Result<zbus::Connection, SecretKeyError> {
            let mut slot = self.conn.lock().await;
            if let Some(conn) = slot.as_ref() {
                return Ok(conn.clone());
            }
            let conn = tokio::time::timeout(LIMIT, zbus::Connection::session())
                .await
                .map_err(|_| SecretKeyError::Bus)?
                .map_err(|_| SecretKeyError::Bus)?;
            *slot = Some(conn.clone());
            Ok(conn)
        }

        async fn settle<T>(&self, result: Result<T, Fail>) -> Result<T, SecretKeyError> {
            match result {
                Ok(value) => Ok(value),
                Err(Fail { error, dead }) => {
                    if dead {
                        tracing::warn!(
                            code = "integrations_key_bus_lost",
                            "a conexão com o Secret Service caiu; a próxima abre outra"
                        );
                        *self.conn.lock().await = None;
                    } else if error == SecretKeyError::Bus {
                        tracing::warn!(
                            code = "integrations_key_bus",
                            "o Secret Service não respondeu à chave das integrações"
                        );
                    }
                    Err(error)
                }
            }
        }

        async fn open(&self) -> Result<(zbus::Connection, OwnedObjectPath), SecretKeyError> {
            let conn = self.connection().await?;
            match tokio::time::timeout(LIMIT, open_session(&conn)).await {
                Ok(Ok(session)) => Ok((conn, session)),
                Ok(Err(fail)) => self.settle(Err(fail)).await,
                Err(_) => self.settle(Err(SecretKeyError::Bus.into())).await,
            }
        }

        async fn finish<T>(
            &self,
            conn: &zbus::Connection,
            session: &OwnedObjectPath,
            result: Result<T, Fail>,
        ) -> Result<T, SecretKeyError> {
            let _ = tokio::time::timeout(LIMIT, close_session(conn, session)).await;
            self.settle(result).await
        }

        async fn read_key(&self) -> Result<Option<SecretKey>, SecretKeyError> {
            let (conn, session) = self.open().await?;
            let found = tokio::time::timeout(LIMIT, read_secret(&conn, &session))
                .await
                .unwrap_or(Err(SecretKeyError::Bus.into()));
            let Some(value) = self.finish(&conn, &session, found).await? else {
                return Ok(None);
            };
            if value.len() != SECRET_KEY_LEN {
                return Err(SecretKeyError::Lost);
            }
            let mut key = Zeroizing::new([0u8; SECRET_KEY_LEN]);
            key.copy_from_slice(&value);
            Ok(Some(key))
        }

        async fn write_key(&self, key: SecretKey) -> Result<(), SecretKeyError> {
            let (conn, session) = self.open().await?;
            let written = tokio::time::timeout(LIMIT, write_secret(&conn, &session, &key[..]))
                .await
                .unwrap_or(Err(SecretKeyError::Bus.into()));
            self.finish(&conn, &session, written).await
        }
    }

    impl SecretKeyStore for SecretService {
        fn read(&self) -> BoxFuture<'_, Result<Option<SecretKey>, SecretKeyError>> {
            Box::pin(async move { self.read_key().await })
        }

        fn write(&self, key: SecretKey) -> BoxFuture<'_, Result<(), SecretKeyError>> {
            Box::pin(async move { self.write_key(key).await })
        }
    }

    struct Fail {
        error: SecretKeyError,
        dead: bool,
    }

    impl From<SecretKeyError> for Fail {
        fn from(error: SecretKeyError) -> Self {
            Self { error, dead: false }
        }
    }

    fn fail(error: zbus::Error) -> Fail {
        let dead = matches!(error, zbus::Error::InputOutput(_));
        let mapped = match &error {
            zbus::Error::MethodError(name, ..)
                if name.to_string() == "org.freedesktop.Secret.Error.IsLocked" =>
            {
                SecretKeyError::Locked
            }
            zbus::Error::MethodError(name, ..) => {
                tracing::warn!(
                    code = "integrations_key_dbus",
                    error = %name,
                    "o Secret Service recusou a chave das integrações"
                );
                SecretKeyError::Bus
            }
            zbus::Error::InputOutput(cause) => {
                tracing::warn!(
                    code = "integrations_key_dbus",
                    kind = ?cause.kind(),
                    "a conexão com o Secret Service falhou"
                );
                SecretKeyError::Bus
            }
            _ => {
                tracing::warn!(
                    code = "integrations_key_dbus",
                    "resposta inesperada do Secret Service"
                );
                SecretKeyError::Bus
            }
        };
        Fail {
            error: mapped,
            dead,
        }
    }

    async fn proxy<'a>(
        conn: &zbus::Connection,
        path: &'a str,
        interface: &'a str,
    ) -> Result<zbus::Proxy<'a>, Fail> {
        build_proxy(conn, path, interface).await.map_err(fail)
    }

    async fn build_proxy<'a>(
        conn: &zbus::Connection,
        path: &'a str,
        interface: &'a str,
    ) -> zbus::Result<zbus::Proxy<'a>> {
        zbus::proxy::Builder::<zbus::Proxy>::new(conn)
            .destination(SECRETS)?
            .path(path)?
            .interface(interface)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
    }

    async fn open_session(conn: &zbus::Connection) -> Result<OwnedObjectPath, Fail> {
        let bus = zbus::fdo::DBusProxy::new(conn).await.map_err(fail)?;
        let name = zbus::names::BusName::try_from(SECRETS)
            .map_err(|_| Fail::from(SecretKeyError::Bus))?;
        if !bus.name_has_owner(name).await.map_err(|e| fail(e.into()))? {
            return Err(SecretKeyError::Bus.into());
        }
        let service = proxy(conn, SERVICE, "org.freedesktop.Secret.Service").await?;
        let reply = service
            .call_with_flags::<_, _, (OwnedValue, OwnedObjectPath)>(
                "OpenSession",
                zbus::proxy::MethodFlags::NoAutoStart.into(),
                &("plain", Value::from("")),
            )
            .await
            .map_err(fail)?;
        Ok(reply.map(|(_, session)| session).ok_or(SecretKeyError::Bus)?)
    }

    async fn close_session(conn: &zbus::Connection, session: &OwnedObjectPath) {
        if let Ok(proxy) = proxy(conn, session.as_str(), "org.freedesktop.Secret.Session").await {
            let _ = proxy
                .call_with_flags::<_, _, ()>(
                    "Close",
                    zbus::proxy::MethodFlags::NoAutoStart.into(),
                    &(),
                )
                .await;
        }
    }

    async fn collections(conn: &zbus::Connection) -> Result<Vec<String>, Fail> {
        let service = proxy(conn, SERVICE, "org.freedesktop.Secret.Service").await?;
        let all: Vec<OwnedObjectPath> = service.get_property("Collections").await.map_err(fail)?;
        let mut found = Vec::new();
        if all.iter().any(|path| path.as_str() == LOGIN) {
            found.push(LOGIN.to_owned());
        }
        let alias = service
            .call_with_flags::<_, _, OwnedObjectPath>(
                "ReadAlias",
                zbus::proxy::MethodFlags::NoAutoStart.into(),
                &("default",),
            )
            .await
            .map_err(fail)?
            .ok_or(SecretKeyError::Bus)?;
        let session = format!("{SERVICE}/collection/session");
        if !["/", LOGIN, session.as_str()].contains(&alias.as_str()) {
            found.push(alias.as_str().to_owned());
        }
        if found.is_empty() {
            tracing::warn!(
                code = "integrations_key_no_collection",
                "o chaveiro não tem a coleção login nem uma padrão em disco"
            );
            return Err(SecretKeyError::Bus.into());
        }
        Ok(found)
    }

    async fn read_secret(
        conn: &zbus::Connection,
        session: &OwnedObjectPath,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, Fail> {
        let places = collections(conn).await?;
        let service = proxy(conn, SERVICE, "org.freedesktop.Secret.Service").await?;
        let flags = zbus::proxy::MethodFlags::NoAutoStart.into();
        let (unlocked, locked) = service
            .call_with_flags::<_, _, (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>(
                "SearchItems",
                flags,
                &(linux_attributes(),),
            )
            .await
            .map_err(fail)?
            .ok_or(SecretKeyError::Bus)?;
        let inside = |items: Vec<OwnedObjectPath>| -> Vec<OwnedObjectPath> {
            let place = |p: &OwnedObjectPath| {
                places
                    .iter()
                    .position(|c| p.as_str().starts_with(&format!("{c}/")))
            };
            let mut items: Vec<_> = items
                .into_iter()
                .filter_map(|p| Some((place(&p)?, p)))
                .collect();
            items.sort_by(|a, b| (a.0, a.1.as_str()).cmp(&(b.0, b.1.as_str())));
            items.into_iter().map(|(_, p)| p).collect()
        };
        let (unlocked, locked) = (inside(unlocked), inside(locked));
        let Some(item) = unlocked.into_iter().next() else {
            return if locked.is_empty() {
                Ok(None)
            } else {
                Err(SecretKeyError::Locked.into())
            };
        };
        let secrets = service
            .call_with_flags::<_, _, HashMap<OwnedObjectPath, Secret>>(
                "GetSecrets",
                flags,
                &(vec![item.clone()], session),
            )
            .await
            .map_err(fail)?
            .ok_or(SecretKeyError::Bus)?;
        let mut found = None;
        for (path, (_, _, value, _)) in secrets {
            let value = Zeroizing::new(value);
            if path == item {
                found = Some(value);
            }
        }
        Ok(Some(found.ok_or(SecretKeyError::Bus)?))
    }

    async fn write_secret(
        conn: &zbus::Connection,
        session: &OwnedObjectPath,
        value: &[u8],
    ) -> Result<(), Fail> {
        let target = collections(conn).await?.swap_remove(0);
        let collection = proxy(conn, &target, "org.freedesktop.Secret.Collection").await?;
        let properties: HashMap<&str, Value> = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from(linux_label())),
            (
                "org.freedesktop.Secret.Item.Attributes",
                Value::from(linux_attributes()),
            ),
        ]);
        let value = Zeroizing::new(value.to_vec());
        let secret = (
            session,
            Vec::<u8>::new(),
            &*value,
            "application/octet-stream",
        );
        let (_, prompt) = collection
            .call_with_flags::<_, _, (OwnedObjectPath, OwnedObjectPath)>(
                "CreateItem",
                zbus::proxy::MethodFlags::NoAutoStart.into(),
                &(properties, secret, true),
            )
            .await
            .map_err(fail)?
            .ok_or(SecretKeyError::Bus)?;
        if prompt.as_str() != "/" {
            return Err(SecretKeyError::Locked.into());
        }
        Ok(())
    }
}

#[cfg(windows)]
mod credman {
    use futures_util::future::BoxFuture;
    use hangar_integrations::secrets::{SECRET_KEY_LEN, SecretKey, SecretKeyError, SecretKeyStore};
    use windows_sys::Win32::Foundation::{
        ERROR_NO_SUCH_LOGON_SESSION, ERROR_NOT_FOUND, GetLastError,
    };
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
        CredWriteW,
    };
    use zeroize::{Zeroize, Zeroizing};

    use super::{INTEGRATIONS_KIND, WINDOWS_TARGET_PREFIX};

    const LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

    pub(super) struct CredentialManager {
        prefix: String,
    }

    impl Default for CredentialManager {
        fn default() -> Self {
            Self {
                prefix: WINDOWS_TARGET_PREFIX.to_owned(),
            }
        }
    }

    impl CredentialManager {
        fn target(&self) -> Vec<u16> {
            wide(&format!("{}/{}", self.prefix, INTEGRATIONS_KIND))
        }
    }

    type Job = Box<dyn FnOnce() + Send>;

    async fn blocking<T: Send + 'static>(
        job: impl FnOnce() -> Result<T, SecretKeyError> + Send + 'static,
    ) -> Result<T, SecretKeyError> {
        static QUEUE: std::sync::OnceLock<std::sync::mpsc::Sender<Job>> =
            std::sync::OnceLock::new();
        let queue = QUEUE.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            let worker = move || {
                for job in rx {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                }
            };
            if let Err(error) = std::thread::Builder::new()
                .name("hangar-integrations-credman".into())
                .spawn(worker)
            {
                tracing::warn!(
                    code = "integrations_key_credman_thread",
                    kind = ?error.kind(),
                    "a fila do Gerenciador de Credenciais das integrações não subiu"
                );
            }
            tx
        });
        let (done, answer) = tokio::sync::oneshot::channel();
        let sent = queue.send(Box::new(move || {
            let _ = done.send(job());
        }));
        let answer = match sent {
            Ok(()) => tokio::time::timeout(LIMIT, answer).await.ok().and_then(Result::ok),
            Err(_) => None,
        };
        match answer {
            Some(result) => result,
            None => {
                tracing::warn!(
                    code = "integrations_key_credman",
                    "o Gerenciador de Credenciais não respondeu à chave das integrações"
                );
                Err(SecretKeyError::Bus)
            }
        }
    }

    impl SecretKeyStore for CredentialManager {
        fn read(&self) -> BoxFuture<'_, Result<Option<SecretKey>, SecretKeyError>> {
            let target = self.target();
            Box::pin(async move {
                let Some(value) = blocking(move || read_item(&target)).await? else {
                    return Ok(None);
                };
                if value.len() != SECRET_KEY_LEN {
                    return Err(SecretKeyError::Lost);
                }
                let mut key = Zeroizing::new([0u8; SECRET_KEY_LEN]);
                key.copy_from_slice(&value);
                Ok(Some(key))
            })
        }

        fn write(&self, key: SecretKey) -> BoxFuture<'_, Result<(), SecretKeyError>> {
            let target = self.target();
            Box::pin(blocking(move || {
                write_item(&target, super::LINUX_LABEL, &key[..])
            }))
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn refused(call: &'static str, error: u32) -> SecretKeyError {
        if error == ERROR_NO_SUCH_LOGON_SESSION {
            tracing::warn!(
                code = "integrations_key_no_logon_session",
                call,
                "este logon do Windows não tem Gerenciador de Credenciais"
            );
            return SecretKeyError::Unsupported;
        }
        tracing::warn!(
            code = "integrations_key_credman",
            call,
            error,
            "o Gerenciador de Credenciais recusou"
        );
        SecretKeyError::Bus
    }

    fn read_item(target: &[u16]) -> Result<Option<Zeroizing<Vec<u8>>>, SecretKeyError> {
        let mut found: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target` termina em NUL; `found` só é lido quando `CredReadW` retorna diferente de zero.
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) } == 0 {
            let error = unsafe { GetLastError() };
            return if error == ERROR_NOT_FOUND {
                Ok(None)
            } else {
                Err(refused("CredReadW", error))
            };
        }
        // SAFETY: `found` é um `CREDENTIALW` válido até `CredFree`; o blob é copiado e zerado antes de liberar.
        unsafe {
            let credential = &*found;
            let size = credential.CredentialBlobSize as usize;
            let mut value = Zeroizing::new(Vec::new());
            if size > 0 && !credential.CredentialBlob.is_null() {
                let blob = std::slice::from_raw_parts_mut(credential.CredentialBlob, size);
                value.extend_from_slice(blob);
                blob.zeroize();
            }
            CredFree(found.cast());
            Ok(Some(value))
        }
    }

    fn write_item(target: &[u16], label: &str, value: &[u8]) -> Result<(), SecretKeyError> {
        let mut target = target.to_vec();
        let mut comment = wide(label);
        let mut user = wide("hangar");
        let mut blob = Zeroizing::new(value.to_vec());
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: comment.as_mut_ptr(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: user.as_mut_ptr(),
            ..Default::default()
        };
        // SAFETY: todos os ponteiros apontam para buffers vivos até o fim da chamada.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            // SAFETY: lê o último erro da thread imediatamente após a falha de `CredWriteW`.
            return Err(refused("CredWriteW", unsafe { GetLastError() }));
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
struct NoKeyring;

#[cfg(not(any(target_os = "linux", windows)))]
impl SecretKeyStore for NoKeyring {
    fn read(
        &self,
    ) -> futures_util::future::BoxFuture<
        '_,
        Result<
            Option<hangar_integrations::secrets::SecretKey>,
            hangar_integrations::secrets::SecretKeyError,
        >,
    > {
        Box::pin(async { Err(hangar_integrations::secrets::SecretKeyError::Unsupported) })
    }

    fn write(
        &self,
        _key: hangar_integrations::secrets::SecretKey,
    ) -> futures_util::future::BoxFuture<
        '_,
        Result<(), hangar_integrations::secrets::SecretKeyError>,
    > {
        Box::pin(async { Err(hangar_integrations::secrets::SecretKeyError::Unsupported) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrations_keyring_metadata_is_isolated_from_vault() {
        let linux_attrs = linux_attributes();
        assert_eq!(linux_attrs.get("application").copied(), Some("hangar"));
        assert_eq!(linux_attrs.get("kind").copied(), Some("integrations-key"));
        assert_eq!(linux_label(), "Hangar — Integrações MCP");
        assert_eq!(windows_target(), "hangar/integrations-key");
        assert!(!windows_target().contains("vault"));
        assert_ne!(linux_attrs.get("kind").copied(), Some("vault-key"));
        let source = include_str!("keyring.rs");
        let prod_only = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(!prod_only.contains("vault-key"));
        assert!(!prod_only.contains("vault-backup-key"));
        assert!(!prod_only.contains("vault.bin"));
    }
}
