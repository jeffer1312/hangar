//! O Gerenciador de Credenciais de verdade, em itens próprios de cada teste que somem no fim.
use super::key::{self, CredentialManager, KeyError, KeyStore};
use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
use zeroize::Zeroizing;

struct Items(String);

impl Items {
    fn new(test: &str) -> Self { Self(format!("hangar-test-{}-{test}", std::process::id())) }
    fn store(&self) -> CredentialManager { CredentialManager::with_prefix(self.0.clone()) }
}

impl Drop for Items {
    fn drop(&mut self) {
        for kind in ["vault-key", "vault-backup-key"] {
            let name: Vec<u16> = format!("{}/{kind}", self.0).encode_utf16().chain(Some(0)).collect();
            // SAFETY: `name` termina em 0 e vive até o fim da chamada.
            unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
        }
    }
}

#[tokio::test]
async fn new_vault_key_is_read_back() {
    let items = Items::new("create");
    let store = items.store();
    assert_eq!(key::open(&store, false).await.unwrap_err(), KeyError::Absent);
    let created = key::create(&store).await.unwrap();
    assert_eq!(*key::open(&store, true).await.unwrap(), *created);
}

#[tokio::test]
async fn write_replaces_the_key() {
    let items = Items::new("replace");
    let store = items.store();
    store.write(Zeroizing::new([7u8; key::KEY_LEN])).await.unwrap();
    store.write(Zeroizing::new([9u8; key::KEY_LEN])).await.unwrap();
    assert_eq!(*store.read().await.unwrap().unwrap(), [9u8; key::KEY_LEN]);
}

#[tokio::test]
async fn recovery_item_round_trips() {
    let items = Items::new("recovery");
    let store = items.store();
    assert!(store.read_recovery().await.unwrap().is_none());
    store.write_recovery(Zeroizing::new(vec![1, 2, 3])).await.unwrap();
    assert_eq!(store.read_recovery().await.unwrap().unwrap().as_slice(), &[1, 2, 3]);
}
