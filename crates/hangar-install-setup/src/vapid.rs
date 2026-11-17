//! `hangar-server vapid-setup`: gera o par VAPID do web push quando o `.env` não tem um.
//!
//! Sem as chaves o push para o celular é descartado calado e o celular nem consegue se inscrever.
//! Chave que já existe nunca é trocada: as inscrições dos aparelhos são assinadas por ela.
//!
//! Qual `.env`: `--env` vence. Senão a regra do `app/user_data.py`: `~/.hangar/config/.env` quando o
//! arquivo está lá, quando a máquina migrou (`~/.hangar/current`) ou quando a raiz da cópia tem
//! `manifest.json` (pacote); senão `<raiz>/backend/.env`. A raiz é `--root` (o checkout, no lugar do
//! antigo cwd `backend/`); sem ele, a primeira pasta acima do binário que tem `manifest.json` (o
//! `bin/` do pacote fica dentro da versão) ou uma `backend/` do Hangar (`.env` ou `app/`). Sem raiz,
//! vale o lugar novo. A pasta nova nunca é criada aqui: fora do pacote ela não existe de propósito.
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};

pub const KEYS: [&str; 2] = ["CP_VAPID_PUBLIC", "CP_VAPID_PRIVATE"];
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Uma chamada: a casa, o `.env` explícito, a raiz da cópia e o binário que roda.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub home: PathBuf,
    pub env: Option<PathBuf>,
    pub root: Option<PathBuf>,
    pub exe: Option<PathBuf>,
}

/// (pública, privada) cruas: o ponto P-256 não comprimido (65 bytes) que o navegador pede e o
/// escalar de 32 bytes que o `push/webpush.rs` assina.
pub fn new_pair() -> Result<(Vec<u8>, Vec<u8>), &'static str> {
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).map_err(|_| "gerador de chaves falhou")?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
        .map_err(|_| "chave gerada ilegível")?;
    let public = pair.public_key().as_ref().to_vec();
    let private = scalar(pkcs8.as_ref(), &public).ok_or("formato da chave gerada inesperado")?;
    // O `ring` recusa par em que a privada não gera a pública: a mesma conferência do leitor.
    EcdsaKeyPair::from_private_key_and_public_key(&ECDSA_P256_SHA256_FIXED_SIGNING, &private, &public, &rng)
        .map_err(|_| "par gerado inconsistente")?;
    Ok((public, private))
}

/// O escalar do PKCS#8 v1 que o `ring` gera para P-256 (138 bytes: privada em 36..68, pública no fim).
fn scalar(der: &[u8], public: &[u8]) -> Option<Vec<u8>> {
    (der.len() == 138 && der[34..36] == [0x04, 0x20] && der[68..73] == [0xa1, 0x44, 0x03, 0x42, 0x00] && &der[73..] == public)
        .then(|| der[36..68].to_vec())
}

/// O par em base64url sem `=`, como grava no `.env`.
pub fn new_pair_b64() -> Result<(String, String), &'static str> {
    let (public, private) = new_pair()?;
    Ok((B64.encode(public), B64.encode(private)))
}

/// Uma linha do arquivo com o fim dela (`""` só na última sem quebra).
struct Line<'a> {
    body: &'a str,
    end: &'a str,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    text.split_inclusive('\n').map(|chunk| match chunk.strip_suffix("\r\n") {
        Some(body) => Line { body, end: "\r\n" },
        None => match chunk.strip_suffix('\n') {
            Some(body) => Line { body, end: "\n" },
            None => Line { body: chunk, end: "" },
        },
    }).collect()
}

/// `CHAVE=valor` fora de comentário, como o Python lia: a chave sem espaço e sem `export `, e se há valor.
struct Entry {
    key: String,
    filled: bool,
    export: bool,
}

fn entry(body: &str) -> Option<Entry> {
    let (key, value) = body.split_once('=')?;
    if body.trim_start().starts_with('#') { return None; }
    let key = key.trim();
    let (key, export) = match key.strip_prefix("export ") {
        Some(rest) => (rest.trim(), true),
        None => (key, false),
    };
    Some(Entry { key: key.to_owned(), filled: !value.trim().is_empty(), export })
}

/// O que fazer com o `.env`.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// As duas chaves já estão lá: nada muda.
    Kept,
    /// As duas estão lá, mas alguma com `export`, que o servidor não lê: o texto sem ele e quais.
    Unexported(String, Vec<&'static str>),
    /// O texto com o par novo.
    Generated(String),
}

/// O `.env` com as duas chaves. Só uma delas é erro: não adivinha a outra. A linha da chave sem valor
/// recebe o valor no lugar (senão a chave apareceria duas vezes); a que falta vai para o fim, no fim de
/// linha que o arquivo já usa.
pub fn with_keys(text: &str, pair: impl FnOnce() -> Result<(String, String), &'static str>) -> Result<Outcome, String> {
    let lines = lines(text);
    let entries: Vec<Option<Entry>> = lines.iter().map(|line| entry(line.body)).collect();
    let present = |key: &str| entries.iter().flatten().any(|e| e.key == key && e.filled);
    let found: Vec<&'static str> = KEYS.into_iter().filter(|key| present(key)).collect();
    let eol = lines.first().map(|line| line.end).filter(|end| !end.is_empty()).unwrap_or("\n");
    if found.len() == KEYS.len() {
        let mut unexported = Vec::new();
        let mut out = String::with_capacity(text.len());
        for (line, entry) in lines.iter().zip(&entries) {
            match entry.as_ref().and_then(|e| KEYS.into_iter().find(|key| e.export && e.filled && e.key == *key)) {
                Some(key) => {
                    let indent = &line.body[..line.body.len() - line.body.trim_start().len()];
                    let value = line.body.split_once('=').map_or("", |(_, value)| value);
                    out.push_str(&format!("{indent}{key}={value}{}", line.end));
                    if !unexported.contains(&key) { unexported.push(key); }
                }
                None => { out.push_str(line.body); out.push_str(line.end); }
            }
        }
        return Ok(if unexported.is_empty() { Outcome::Kept } else { Outcome::Unexported(out, unexported) });
    }
    if let Some(only) = found.first() {
        return Err(format!("tem só {only}; apague a linha ou complete o par à mão"));
    }
    let (public, private) = pair().map_err(str::to_owned)?;
    let mut bodies: Vec<String> = lines.iter().map(|line| line.body.to_owned()).collect();
    let mut appended = String::new();
    for (key, value) in KEYS.into_iter().zip([public, private]) {
        // A última linha vazia da chave: é a que o leitor (a última vale) usaria.
        match entries.iter().rposition(|e| e.as_ref().is_some_and(|e| e.key == key)) {
            Some(at) => bodies[at] = format!("{key}={value}"),
            None => appended.push_str(&format!("{key}={value}{eol}")),
        }
    }
    let mut out: String = bodies.iter().zip(&lines).map(|(body, line)| format!("{body}{}", line.end)).collect();
    if !appended.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') { out.push_str(eol); }
        out.push_str(&appended);
    }
    Ok(Outcome::Generated(out))
}

/// A primeira pasta acima do binário que é raiz de uma cópia do Hangar.
pub fn version_root(exe: &Path) -> Option<PathBuf> {
    exe.parent()?.ancestors().find(|dir| {
        let backend = dir.join("backend");
        dir.join("manifest.json").is_file() || backend.join(".env").is_file() || backend.join("app").is_dir()
    }).map(Path::to_path_buf)
}

/// O `.env` onde as chaves moram (ver o topo do módulo).
pub fn env_path(request: &Request) -> io::Result<PathBuf> {
    if let Some(env) = &request.env { return Ok(env.clone()); }
    let root = request.root.clone().or_else(|| request.exe.as_deref().and_then(version_root));
    let new = request.home.join(".hangar").join("config").join(".env");
    let Some(root) = root else { return Ok(new) };
    let migrated = new.try_exists()? || hangar_core::paths::current(&request.home).try_exists()?
        || root.join("manifest.json").try_exists()?;
    Ok(if migrated { new } else { root.join("backend").join(".env") })
}

/// Onde gravar: o alvo do link, para o `.env` que é link continuar link.
fn write_target(env: &Path) -> PathBuf {
    match std::fs::symlink_metadata(env) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(env).unwrap_or_else(|_| {
            // Link quebrado: grava onde ele aponta, como o leitor o seguiria.
            match std::fs::read_link(env) {
                Ok(target) if target.is_absolute() => target,
                Ok(target) => env.parent().map_or(target.clone(), |dir| dir.join(target)),
                Err(_) => env.to_path_buf(),
            }
        }),
        _ => env.to_path_buf(),
    }
}

/// Temporário na mesma pasta, no modo do arquivo de antes (novo: 600), e troca atômica. Falhou: o
/// temporário sai e o erro sobe.
fn write(target: &Path, text: &str) -> io::Result<()> {
    let mode = match std::fs::metadata(target) {
        Ok(_) => hangar_core::fs::file_mode(target).ok(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(0o600),
        Err(error) => return Err(error),
    };
    let dir = target.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let tick = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let temp = dir.join(format!(".vapid-{}-{tick}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if let Some(mode) = mode { hangar_core::fs::chmod(&temp, mode)?; }
        hangar_core::fs::replace(&temp, target)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temp); }
    result
}

/// O comando: frases e códigos do `python -m app.vapid_setup`, com as correções da decisão 16.
pub fn run(request: &Request, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let env = match env_path(request) {
        Ok(env) => env,
        Err(error) => {
            let _ = writeln!(err, "erro: não consegui conferir onde mora o .env: {error}");
            return 1;
        }
    };
    let text = match std::fs::read(&env) {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(text) => text.to_owned(),
            Err(error) => {
                let _ = writeln!(err, "erro: {}", crate::utf8_error(&bytes, &error));
                return 1;
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            let _ = writeln!(err, "erro: {}", crate::os_error(&error, &env));
            return 1;
        }
    };
    let shown = env.display();
    let (new, done) = match with_keys(&text, new_pair_b64) {
        Ok(Outcome::Kept) => {
            let _ = writeln!(out, "ok: {shown} já tem as chaves VAPID (mantidas)");
            return 0;
        }
        Ok(Outcome::Unexported(new, keys)) => {
            let message = format!("ok: {shown} já tem as chaves VAPID (mantidas)");
            let warning = format!("aviso: {shown}: `export` tirado de {}; o servidor ignora a linha com ele (reinicie o backend para valer)", keys.join(", "));
            (new, Some((message, warning)))
        }
        Ok(Outcome::Generated(new)) => (new, None),
        Err(message) => {
            let _ = writeln!(err, "erro: {shown} {message}");
            return 1;
        }
    };
    if let Err(error) = write(&write_target(&env), &new) {
        let (message, number) = hangar_core::fs::os_error_parts(&error);
        let detail = number.map_or(message.clone(), |n| format!("[Errno {n}] {message}"));
        let _ = writeln!(err, "erro: não consegui gravar {shown}: {detail}");
        return 1;
    }
    match done {
        Some((message, warning)) => {
            let _ = writeln!(err, "{warning}");
            let _ = writeln!(out, "{message}");
        }
        None => { let _ = writeln!(out, "ok: chaves VAPID geradas em {shown} (reinicie o backend para valer)"); }
    }
    0
}

/// `vapid-setup [--env <arquivo>] [--root <raiz da cópia>]`.
pub fn parse_args(args: &[String]) -> Result<(Option<PathBuf>, Option<PathBuf>), String> {
    let (mut env, mut root) = (None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        let slot = match flag {
            "--env" => &mut env,
            "--root" => &mut root,
            other => return Err(format!("argumento inesperado '{other}'")),
        };
        let value = inline.or_else(|| rest.next().cloned()).filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{flag} pede um caminho"))?;
        *slot = Some(PathBuf::from(value));
    }
    Ok((env, root))
}

pub fn cli(args: &[String]) -> ExitCode {
    let mut err = io::stderr().lock();
    let (env, root) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = writeln!(err, "uso: hangar-server vapid-setup [--env <arquivo>] [--root <raiz da cópia>] ({message})");
            return ExitCode::from(2);
        }
    };
    let request = Request {
        home: hangar_core::account_env::AccountEnvironment::capture().home,
        env,
        root,
        exe: std::env::current_exe().ok(),
    };
    let mut out = io::stdout().lock();
    let code = run(&request, &mut out, &mut err);
    let _ = out.flush();
    ExitCode::from(code)
}
