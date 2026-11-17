//! Os casos capturados do Python da base (`tests/contract/*.json`, copiados da captura R4 em
//! `contract/pacote/`) rodados contra `vapid-setup` e `tailscale-dns-name`: código, stdout, stderr e a
//! árvore de arquivos (tipo, modo, bytes, alvo do link) depois da rodada. O que a migração mudou de
//! propósito está em `adjust`, cada caso com a decisão que o mudou.
#![cfg(unix)]
use std::collections::BTreeMap;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use base64::Engine;
use hangar_install_setup::{tailscale, vapid};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use serde_json::Value;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Dir(u32),
    File(u32, Vec<u8>),
    Link(String),
}

fn captures(prefix: &str) -> Vec<(String, Value)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/contract");
    let mut found: Vec<(String, Value)> = std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with(prefix))
        .map(|path| {
            let data: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            (data["case"].as_str().unwrap().to_owned(), data)
        }).collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!found.is_empty(), "nenhuma captura {prefix}");
    found
}

fn mode_of(value: &Value) -> u32 {
    u32::from_str_radix(value["mode"].as_str().unwrap().trim_start_matches("0o"), 8).unwrap()
}

fn content(value: &Value) -> Vec<u8> {
    let content = &value["content"];
    match content["text"].as_str() {
        Some(text) => text.as_bytes().to_vec(),
        None => base64::engine::general_purpose::STANDARD.decode(content["b64"].as_str().unwrap()).unwrap(),
    }
}

fn node(value: &Value, t: &str) -> Node {
    match value["type"].as_str().unwrap() {
        "dir" => Node::Dir(mode_of(value)),
        "file" => Node::File(mode_of(value), content(value)),
        "symlink" => Node::Link(value["target"].as_str().unwrap().replace("$T", t)),
        other => panic!("tipo inesperado {other}"),
    }
}

fn snapshot(root: &Path) -> BTreeMap<String, Node> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Node>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let key = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            if meta.file_type().is_symlink() {
                out.insert(key, Node::Link(std::fs::read_link(&path).unwrap().to_string_lossy().into_owned()));
            } else if meta.is_dir() {
                out.insert(key, Node::Dir(mode));
                walk(root, &path, out);
            } else {
                out.insert(key, Node::File(mode, std::fs::read(&path).unwrap_or_default()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// A árvore de antes do caso: pastas, arquivos e links; os modos das pastas por último (uma pasta sem
/// escrita não deixaria criar o que mora nela).
fn setup(t: &Path, before: &serde_json::Map<String, Value>) {
    std::fs::create_dir_all(t.join("home")).unwrap();
    let ts = t.to_string_lossy();
    let mut dirs = Vec::new();
    for (key, value) in before {
        let path = t.join(key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        match value["type"].as_str().unwrap() {
            "dir" => { std::fs::create_dir_all(&path).unwrap(); dirs.push((path, mode_of(value))); }
            "file" => {
                std::fs::write(&path, content(value)).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode_of(value))).unwrap();
            }
            "symlink" => symlink(value["target"].as_str().unwrap().replace("$T", &ts), &path).unwrap(),
            // `vc("env-unreadable", env_text="x\n", mode=0o000)` da captura.
            "unreadable" => {
                std::fs::write(&path, "x\n").unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            }
            other => panic!("tipo inesperado {other}"),
        }
    }
    dirs.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (path, mode) in dirs {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
}

/// Devolve escrita às pastas, para o `TempDir` conseguir apagar.
fn unlock(dir: &Path) {
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) { unlock(&path); }
    }
}

fn root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// O par gerado que ficou no arquivo: as duas chaves, base64url sem `=`, pública de 65 bytes com 0x04,
/// privada de 32, e privada × G = pública (o `ring` recusa par inconsistente).
fn generated_pair(text: &str) -> (String, String) {
    let last = |key: &str| text.lines().filter_map(|line| line.trim_end_matches('\r').strip_prefix(&format!("{key}=")))
        .last().unwrap_or_else(|| panic!("sem {key} em {text:?}")).to_owned();
    let (public, private) = (last("CP_VAPID_PUBLIC"), last("CP_VAPID_PRIVATE"));
    assert!(!public.contains('=') && !private.contains('='));
    let (public_raw, private_raw) = (B64.decode(&public).unwrap(), B64.decode(&private).unwrap());
    assert_eq!((public_raw.len(), public_raw[0], private_raw.len()), (65, 0x04, 32));
    EcdsaKeyPair::from_private_key_and_public_key(&ECDSA_P256_SHA256_FIXED_SIGNING, &private_raw, &public_raw,
        &ring::rand::SystemRandom::new()).expect("privada × G = pública");
    (public, private)
}

/// O esperado do Python, ou o que a migração mudou de propósito.
struct Expected {
    code: u8,
    stdout: String,
    stderr: String,
    /// Caminho relativo → nó depois (`None` = apagado). Só o que muda.
    diff: BTreeMap<String, Option<Value>>,
    generated: bool,
}

fn expected(data: &Value) -> Expected {
    let output = &data["output"];
    let mut diff = BTreeMap::new();
    for (key, value) in output["files"]["created"].as_object().unwrap() { diff.insert(key.clone(), Some(value.clone())); }
    for (key, value) in output["files"]["changed"].as_object().unwrap() { diff.insert(key.clone(), Some(value["after"].clone())); }
    for key in output["files"]["removed"].as_array().unwrap() { diff.insert(key.as_str().unwrap().to_owned(), None); }
    Expected {
        code: output["code"].as_u64().unwrap() as u8,
        stdout: output["stdout"]["text"].as_str().unwrap().to_owned(),
        stderr: output["stderr"]["text"].as_str().unwrap().to_owned(),
        diff,
        generated: !data["post"]["generated_pair"].is_null(),
    }
}

fn file_value(mode: &str, text: &str) -> Value {
    serde_json::json!({"type": "file", "mode": mode, "content": {"text": text}})
}

fn adjust(case: &str, mut e: Expected) -> Expected {
    let not_written = |env: &str, errno: &str| format!("erro: não consegui gravar {env}: {errno}\n");
    match case {
        // Decisão 16 (surpresa 10): o `mkstemp` que falha vira frase com código, sem traceback.
        "vapid/dir-not-writable" => e.stderr = not_written("$T/inst/backend/.env", "[Errno 13] Permission denied"),
        "vapid/current-without-config-dir" | "vapid/manifest-without-config-dir" =>
            e.stderr = not_written("$T/home/.hangar/config/.env", "[Errno 2] No such file or directory"),
        // Decisão 13.3: `export CP_VAPID_*=` é regravado sem `export`, com aviso.
        "vapid/export-both" => {
            e.stderr = "aviso: $T/inst/backend/.env: `export` tirado de CP_VAPID_PUBLIC, CP_VAPID_PRIVATE; o servidor ignora a linha com ele (reinicie o backend para valer)\n".to_owned();
            e.diff.insert("inst/backend/.env".to_owned(), Some(file_value("0o644", "CP_VAPID_PUBLIC=a\nCP_VAPID_PRIVATE=b\n")));
        }
        // Decisão 16 (surpresa 9): o fim de linha do arquivo fica.
        "vapid/crlf" => {
            e.diff.insert("inst/backend/.env".to_owned(), Some(file_value("0o644",
                "CP_AUTH_TOKEN=x\r\nCP_PORT=1\r\nCP_VAPID_PUBLIC=$VAPID_PUBLIC\r\nCP_VAPID_PRIVATE=$VAPID_PRIVATE\r\n")));
        }
        // Decisão 16 (surpresa 7): o link continua link; grava no alvo, no modo dele.
        "vapid/env-symlink" => {
            let after = e.diff.remove("inst/backend/.env").unwrap().unwrap();
            e.diff.insert("work/real.env".to_owned(), Some(after));
        }
        // Decisão 16 (surpresa 10): a linha da chave vazia recebe o valor; a chave não aparece duas vezes.
        "vapid/empty-values" => {
            e.diff.insert("inst/backend/.env".to_owned(), Some(file_value("0o644",
                "CP_VAPID_PUBLIC=$VAPID_PUBLIC\nCP_VAPID_PRIVATE=$VAPID_PRIVATE\n")));
        }
        _ => {}
    }
    e
}

#[test]
fn vapid_setup_matches_the_python_captures() {
    for (case, data) in captures("vapid__") {
        let permissions = matches!(case.as_str(), "vapid/dir-not-writable" | "vapid/env-unreadable");
        if permissions && root() { continue; }
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().to_path_buf();
        let ts = t.to_string_lossy().into_owned();
        setup(&t, data["input"]["files_before"].as_object().unwrap());
        let before = snapshot(&t);
        let request = vapid::Request { home: t.join("home"), env: None, root: Some(t.join("inst")), exe: None };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = vapid::run(&request, &mut out, &mut err);
        let after = snapshot(&t);
        unlock(&t);
        let e = adjust(&case, expected(&data));
        let normalize = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap().replace(&ts, "$T");
        assert_eq!(code, e.code, "{case}: código");
        assert_eq!(normalize(out), e.stdout, "{case}: stdout");
        assert_eq!(normalize(err), e.stderr, "{case}: stderr");
        let changed: BTreeMap<String, Option<Node>> = before.keys().chain(after.keys())
            .filter(|key| before.get(*key) != after.get(*key))
            .map(|key| (key.clone(), after.get(key).cloned())).collect();
        let pair = e.generated.then(|| {
            let text = changed.values().flatten().find_map(|node| match node {
                Node::File(_, bytes) => Some(String::from_utf8(bytes.clone()).unwrap()),
                _ => None,
            }).unwrap_or_else(|| panic!("{case}: par gerado sem arquivo gravado"));
            generated_pair(&text)
        });
        let want: BTreeMap<String, Option<Node>> = e.diff.iter().map(|(key, value)| {
            let wanted = value.as_ref().map(|value| {
                let mut value = value.clone();
                if let (Some(text), Some((public, private))) = (value["content"]["text"].as_str().map(str::to_owned), &pair) {
                    value["content"]["text"] = Value::from(text.replace("$VAPID_PUBLIC", public).replace("$VAPID_PRIVATE", private));
                }
                node(&value, &ts)
            });
            (key.clone(), wanted)
        }).collect();
        assert_eq!(changed, want, "{case}: arquivos");
    }
}

#[test]
fn tailscale_dns_name_matches_the_python_captures() {
    for (case, data) in captures("tailscale__") {
        let status = &data["harness"]["stub_config"]["tailscale"]["status"];
        // Sem `tailscale` instalado, o pipe entrega a entrada vazia.
        let stdin = status["stdout"].as_str().unwrap_or("");
        let printed = data["output"]["stdout"]["text"].as_str().unwrap();
        let name = printed.lines().find_map(|line| line.strip_prefix("nome=[")?.strip_suffix(']'));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = tailscale::run(&[], stdin.as_bytes(), &mut out, &mut err);
        match name {
            Some(name) => {
                assert_eq!((code, String::from_utf8(out).unwrap()), (0, format!("{name}\n")), "{case}");
            }
            None => {
                assert!(printed.contains("Tailscale sem login"), "{case}: {printed}");
                assert_eq!((code, out.len()), (1, 0), "{case}");
            }
        }
        assert!(err.is_empty(), "{case}: stderr");
    }
}
