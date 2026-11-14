//! Chaves de cliente do `~/.cli-proxy-api/config.yaml` (JEV-01, HIS-40/44/48/49). Os dois leitores do
//! Rust: `jev_aux::cliproxy_keys` (o `cliproxy` do `GET /api/computer-control`, conferido pelo
//! `key_is_cliproxy` de cada chave) e `cliproxy_config::client_key` (a primeira, que o
//! `jev_aux_cliproxy` da página Jev grava). Os textos e as chaves esperadas vêm de
//! `test_cliproxy_keys_new_and_old_format` e `test_cliproxy_keys_quoted_keep_hash_and_spaces`
//! (ponta 7ef9011ba); o `_cliproxy_keys` do Python descarta chave vazia e devolve `[]` com o arquivo
//! ilegível (OSError/UnicodeDecodeError), com aviso no log.
use std::path::Path;

use hangar_server::accounts::environment::AccountEnvironment;
use hangar_server::cliproxy_config::client_key;

const CONFIG: &str = ".cli-proxy-api/config.yaml";

/// Os três parâmetros de `test_cliproxy_keys_new_and_old_format`: sempre `["k1", "k2"]`.
const NEW_AND_OLD: [&str; 3] = [
    "port: 8317\naccess:\n  api-keys:\n    - \"k1\"\n    - k2\napi-keys:\n  gemini:\n    - name: x\n",
    "port: 8317\napi-keys:\n- 'k1'\n- k2\nother: 1\n",
    "api-keys:\n  - name: provider\n    base-url: x\naccess:\n  api-keys:\n    - k1\n    - k2  # comment\n",
];
const QUOTED: &str = "access:\n  api-keys:\n    - \"ab#cd\"\n    - 'x y'\n";
const EMPTY_FIRST: &str = "access:\n  api-keys:\n    - \"\"\n    - k\n";
const ONLY_EMPTY: &str = "access:\n  api-keys:\n    - \"\"\n";
const NOT_UTF8: &[u8] = b"access:\n  api-keys:\n    - k\xff\n";

fn home_with(config: Option<&[u8]>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    if let Some(bytes) = config {
        let path = dir.path().join(CONFIG);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    dir
}

fn first_key(home: &Path) -> Option<String> {
    let home = home.to_string_lossy().into_owned();
    client_key(&AccountEnvironment::from_pairs([("HOME", home.clone()), ("USERPROFILE", home)]))
}

/// JEV-01, JEV-02, HIS-40, HIS-44, HIS-48, HIS-49: a chave que a página grava é a primeira da lista.
#[test]
fn client_key_is_the_first_key_of_the_list() {
    for text in NEW_AND_OLD {
        assert_eq!(first_key(home_with(Some(text.as_bytes())).path()).as_deref(), Some("k1"), "{text}");
    }
    assert_eq!(first_key(home_with(Some(QUOTED.as_bytes())).path()).as_deref(), Some("ab#cd"));
    assert_eq!(first_key(home_with(Some(b"access:\n  api-keys:\n    - 'x y'\n")).path()).as_deref(), Some("x y"));
    assert_eq!(first_key(home_with(Some(EMPTY_FIRST.as_bytes())).path()).as_deref(), Some("k"), "chave vazia é descartada");
    assert_eq!(first_key(home_with(Some(ONLY_EMPTY.as_bytes())).path()), None);
    assert_eq!(first_key(home_with(None).path()), None, "arquivo ausente");
    assert_eq!(first_key(home_with(Some(NOT_UTF8)).path()), None, "arquivo ilegível");
    let dir = home_with(None);
    std::fs::create_dir_all(dir.path().join(CONFIG)).unwrap();
    assert_eq!(first_key(dir.path()), None, "o caminho é uma pasta");
}

#[cfg(unix)]
mod route {
    use std::collections::{BTreeMap, BTreeSet};
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use hangar_server::accounts::AccountService;
    use hangar_server::routes::{AppState, router};

    use super::*;
    use crate::fake::OWNER;

    struct Scene {
        _root: tempfile::TempDir,
        home: PathBuf,
        app: axum::Router,
    }

    impl Scene {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("home");
            let bin = root.path().join("bin");
            std::fs::create_dir_all(home.join(".claude")).unwrap();
            std::fs::create_dir_all(&bin).unwrap();
            let mut state = AppState::new(crate::fake::config(crate::refused_address(), "127.0.0.1"));
            state.accounts = AccountService::new(AccountEnvironment::from_map(BTreeMap::from([
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
                ("PATH".into(), bin.to_string_lossy().into_owned()),
            ])));
            let app = router(Arc::new(state));
            Self { _root: root, home, app }
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let path = self.home.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }

        /// O bloco `cliproxy` do GET com `key` como chave do auxiliar da página Jev.
        async fn cliproxy(&self, key: &str) -> Value {
            self.write(".claude/runtime-config.json", json!({"jev_aux_api_key": key}).to_string().as_bytes());
            let mut request = Request::builder()
                .method("GET")
                .uri("/api/computer-control")
                .header("authorization", format!("Bearer {OWNER}"))
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(ConnectInfo::<SocketAddr>("127.0.0.1:1234".parse().unwrap()));
            let response = self.app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status().as_u16(), 200, "o GET não cai com config.yaml ruim");
            let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
            body["cliproxy"].clone()
        }

        /// Quais dos candidatos o GET reconhece como chave do CLIProxy, e o `has_keys`.
        async fn keys(&self, config: Option<&[u8]>, candidates: &[&str]) -> (BTreeSet<String>, bool) {
            if let Some(bytes) = config {
                self.write(CONFIG, bytes);
            }
            let mut found = BTreeSet::new();
            let mut has_keys = None;
            for candidate in candidates {
                let cliproxy = self.cliproxy(candidate).await;
                if cliproxy["key_is_cliproxy"] == true {
                    found.insert((*candidate).to_owned());
                }
                let has = cliproxy["has_keys"].as_bool().unwrap();
                assert!(has_keys.is_none_or(|before| before == has));
                has_keys = Some(has);
            }
            (found, has_keys.unwrap())
        }
    }

    /// Chaves e quase-chaves: aspas, comentário, provedor e item da raiz.
    const CANDIDATES: [&str; 13] =
        ["k1", "k2", "k3", "'k1'", "\"k1\"", "k2  # comment", "k2 # comment", "name: provider", "provider", "x", "gemini", "ab#cd", "x y"];

    fn set(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|k| (*k).to_owned()).collect()
    }

    /// JEV-01, HIS-40, HIS-44, HIS-49, HIS-51: formato novo e antigo, aspas simples na raiz sem recuo,
    /// comentário depois do item, `#` e espaço dentro de aspas.
    #[tokio::test]
    async fn get_reads_the_same_keys_as_the_python() {
        for text in NEW_AND_OLD {
            assert_eq!(Scene::new().keys(Some(text.as_bytes()), &CANDIDATES).await, (set(&["k1", "k2"]), true), "{text}");
        }
        assert_eq!(Scene::new().keys(Some(QUOTED.as_bytes()), &CANDIDATES).await, (set(&["ab#cd", "x y"]), true));
    }

    /// HIS-49: chave vazia é descartada; só vazias = sem chave.
    #[tokio::test]
    async fn get_drops_empty_keys() {
        assert_eq!(Scene::new().keys(Some(EMPTY_FIRST.as_bytes()), &["k", "x"]).await, (set(&["k"]), true));
        assert_eq!(Scene::new().keys(Some(ONLY_EMPTY.as_bytes()), &["k"]).await, (set(&[]), false));
    }

    /// HIS-48: ausente, não UTF-8 ou pasta no lugar do arquivo = sem chaves, e o GET responde.
    #[tokio::test]
    async fn get_without_a_readable_config_has_no_keys() {
        assert_eq!(Scene::new().keys(None, &["k"]).await, (set(&[]), false));
        assert_eq!(Scene::new().keys(Some(NOT_UTF8), &["k", "k\u{fffd}"]).await, (set(&[]), false));
        let scene = Scene::new();
        std::fs::create_dir_all(scene.home.join(CONFIG)).unwrap();
        assert_eq!(scene.keys(None, &["k"]).await, (set(&[]), false));
    }
}
