//! `external_pairs.json`: o lado de saída do par externo (endereço e token que o OUTRO usuário
//! entregou). O lado de entrada é um convite de `kind: "pair"` na loja de convites; fora do
//! `peers.json`, que guarda o token de dono das máquinas do próprio usuário.
use crate::groups::peers::{self, PeerError};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};
use unicode_normalization::UnicodeNormalization;

pub const MAX_TEXT: usize = 16000;
pub const FUNNEL_PORT: u16 = 8443;
/// Colchete que abre um cabeçalho forjado (o de largura total o NFKC já vira `[`).
const OPENERS: [char; 3] = ['[', '\u{ff3b}', '\u{3010}'];

static HOST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+ts\.net$").unwrap());
static SLUG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9._-]+").unwrap());

/// Os 8 campos são exigidos pelo leitor da lista (`list/links.rs`) e pelo `groups/context_cmd.rs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalPair {
    pub share_id: String,
    pub local_session: String,
    pub alias: String,
    pub peer_owner: String,
    pub peer_session: String,
    pub peer_address: String,
    pub peer_token: String,
    pub created_at: f64,
}

impl ExternalPair {
    pub fn address(&self) -> String { format!("{}::{}", self.alias, self.peer_session) }
}

pub struct PairStore { path: PathBuf, state: Mutex<Vec<ExternalPair>> }

impl PairStore {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let state = Mutex::new(load(&path));
        Self { path, state }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<ExternalPair>> { self.state.lock().unwrap_or_else(|e| e.into_inner()) }

    fn save(&self, state: &[ExternalPair]) -> io::Result<()> {
        if let Some(parent) = self.path.parent() { std::fs::create_dir_all(parent)?; }
        let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
        crate::runtime::queue::atomic_write(&self.path, &bytes)
    }

    /// Gravação que falha desfaz o registro: senão a memória diria pareado e o disco não.
    pub fn add(&self, rec: ExternalPair) -> io::Result<()> {
        let mut state = self.lock();
        state.push(rec);
        let saved = self.save(&state);
        if saved.is_err() { state.pop(); }
        saved
    }

    pub fn remove(&self, share_id: &str) -> io::Result<Option<ExternalPair>> {
        let mut state = self.lock();
        let Some(i) = state.iter().position(|r| r.share_id == share_id) else { return Ok(None) };
        let rec = state.remove(i);
        self.save(&state)?;
        Ok(Some(rec))
    }

    pub fn rename_local(&self, old: &str, new: &str) -> io::Result<()> {
        let mut state = self.lock();
        let moved: Vec<usize> = state.iter().enumerate().filter(|(_, r)| r.local_session == old).map(|(i, _)| i).collect();
        if moved.is_empty() { return Ok(()); }
        for &i in &moved { state[i].local_session = new.to_owned(); }
        // Falha fica com o nome novo na memória: a gravação pode ter chegado ao disco, e quem chama
        // desfaz com o rename de volta, que regrava os dois.
        self.save(&state)
    }

    pub fn all(&self) -> Vec<ExternalPair> { self.lock().clone() }

    pub fn by_share(&self, share_id: &str) -> Option<ExternalPair> { self.lock().iter().find(|r| r.share_id == share_id).cloned() }

    pub fn by_address(&self, addr: &str) -> Option<ExternalPair> { self.lock().iter().find(|r| r.address() == addr).cloned() }

    pub fn by_local(&self, session: &str) -> Vec<ExternalPair> { self.lock().iter().filter(|r| r.local_session == session).cloned().collect() }

    /// Os membros de grupo que são de fora: separa par externo de máquina própria (`srv::`).
    pub fn addresses(&self) -> HashSet<String> { self.lock().iter().map(ExternalPair::address).collect() }

    /// Apelido do dono de fora que não colide com peer, com esta máquina nem com outro par. A mesma
    /// máquina (endereço) mantém o apelido que já tem, mesmo com o dono renomeado.
    pub fn free_alias(&self, owner: &str, address: &str, peers: &HashSet<String>, own_id: &str) -> String {
        let state = self.lock();
        if let Some(r) = state.iter().find(|r| r.peer_address == address) { return r.alias.clone(); }
        let base = slug(owner);
        let taken = |a: &str| peers.contains(a) || a == own_id || state.iter().any(|r| r.alias == a);
        let (mut alias, mut n) = (base.clone(), 2);
        while taken(&alias) {
            alias = format!("{base}-{n}");
            n += 1;
        }
        alias
    }

    pub fn paired_with(&self, address: &str, peer_session: &str) -> bool {
        self.lock().iter().any(|r| r.peer_address == address && r.peer_session == peer_session)
    }

    /// Apelido que virou peer depois do par: `alias::sessao` não diz mais quem é quem.
    pub fn ambiguous(&self, alias: &str, peers: &HashSet<String>) -> bool {
        peers.contains(alias) && self.lock().iter().any(|r| r.alias == alias)
    }
}

/// Arquivo torto vai para o lado: a próxima gravação apagaria tokens que ainda dá para recuperar.
fn load(path: &Path) -> Vec<ExternalPair> {
    let error = match std::fs::read(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => e.to_string(),
        Ok(raw) => match serde_json::from_slice(&raw) {
            Ok(state) => return state,
            Err(e) => e.to_string(),
        },
    };
    tracing::warn!(code = "external_pairs_unreadable", error, "par externo: external_pairs.json ilegível, começando vazio");
    let epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut bad = path.as_os_str().to_owned();
    bad.push(format!(".bad-{epoch}"));
    if let Err(e) = std::fs::rename(path, &bad) {
        tracing::warn!(code = "external_pairs_quarantine_failed", error = %e, "par externo: não consegui pôr o arquivo ilegível de lado");
    }
    Vec::new()
}

pub fn slug(owner: &str) -> String {
    let ascii: String = owner.nfkd().filter(char::is_ascii).collect::<String>().to_ascii_lowercase();
    let s = SLUG_RE.replace_all(&ascii, "-").trim_matches('-').to_owned();
    if s.is_empty() { "par".into() } else { s }
}

fn charset(s: &str, len: std::ops::RangeInclusive<usize>, extra: &[u8]) -> bool {
    len.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

pub fn valid_owner(owner: &str) -> bool { charset(owner, 1..=40, b"._-") }

/// Vai para o endereço, para o cabeçalho e para um comando que o modelo copia: só o alfabeto dos
/// nomes do Hangar.
pub fn valid_session(name: &str) -> bool { charset(name, 1..=64, b"._-") }

pub fn valid_token(token: &str) -> bool { charset(token, 20..=200, b"_-") }

/// `str.strip()` do Python: inclui os separadores 0x1C-0x1F, que o `trim` do Rust não tira.
fn py_strip(s: &str) -> &str { s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)) }

struct SplitUrl { scheme: String, netloc: String, path: String, query: String, fragment: String }

/// `urllib.parse.urlsplit`, só no que as regras abaixo leem.
fn urlsplit(raw: &str) -> SplitUrl {
    let url: String = raw.trim_start_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut rest = url.as_str();
    let mut scheme = String::new();
    if let Some((s, after)) = rest.split_once(':')
        && s.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b)) {
        scheme = s.to_ascii_lowercase();
        rest = after;
    }
    let mut netloc = "";
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        (netloc, rest) = after.split_at(end);
    }
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    SplitUrl { scheme, netloc: netloc.into(), path: path.into(), query: query.into(), fragment: fragment.into() }
}

/// Só o Funnel de outra máquina: `https://<nó>.ts.net:8443`, sem usuário, caminho, query nem âncora.
pub fn normalize_address(addr: &str) -> Option<String> {
    let u = urlsplit(py_strip(addr));
    // Colchete é IPv6, que nunca é `ts.net`; o Python recusa ou falha no parse.
    if u.netloc.contains(['[', ']']) { return None; }
    let (userinfo, hostinfo) = u.netloc.rsplit_once('@').map_or(("", u.netloc.as_str()), |(a, b)| (a, b));
    let (host, port) = hostinfo.split_once(':').unwrap_or((hostinfo, ""));
    let host = host.to_lowercase();
    let port_ok = !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && port.trim_start_matches('0') == "8443";
    let ok = u.scheme == "https" && port_ok && HOST_RE.is_match(&host) && userinfo.split(':').all(str::is_empty)
        && u.query.is_empty() && u.fragment.is_empty() && matches!(u.path.as_str(), "" | "/");
    ok.then(|| format!("https://{host}:{FUNNEL_PORT}"))
}

/// Link de convite de par: `<funnel>/par/<código>` → (endereço, código).
pub fn parse_pair_link(link: &str) -> Option<(String, String)> {
    let u = urlsplit(py_strip(link));
    let parts: Vec<&str> = u.path.split('/').filter(|p| !p.is_empty()).collect();
    let [first, code] = parts[..] else { return None };
    if first != "par" { return None; }
    Some((normalize_address(&format!("{}://{}", u.scheme, u.netloc))?, code.to_owned()))
}

/// `unicodedata.category` em Cf, Zs ou Cc (Unicode 16, o do Python 3.14), mais U+180E,
/// U+2061-2064 e as tags. Sem crate de categoria no workspace: faixas tiradas do Python.
const INVISIBLE: [(u32, u32); 24] = [
    (0x0, 0x20), (0x7F, 0xA0), (0xAD, 0xAD), (0x600, 0x605), (0x61C, 0x61C), (0x6DD, 0x6DD), (0x70F, 0x70F),
    (0x890, 0x891), (0x8E2, 0x8E2), (0x1680, 0x1680), (0x180E, 0x180E), (0x2000, 0x200F), (0x202A, 0x202F),
    (0x205F, 0x2064), (0x2066, 0x206F), (0x3000, 0x3000), (0xFEFF, 0xFEFF), (0xFFF9, 0xFFFB), (0x110BD, 0x110BD),
    (0x110CD, 0x110CD), (0x13430, 0x1343F), (0x1BCA0, 0x1BCA3), (0x1D173, 0x1D17A), (0xE0000, 0xE007F),
];

/// Espaço, controle e formatação (bidi, largura zero, tags) fazem um cabeçalho forjado parecer
/// começo de linha comum.
pub fn is_invisible(c: char) -> bool {
    let c = c as u32;
    INVISIBLE.iter().any(|&(a, b)| (a..=b).contains(&c))
}

/// `str.splitlines(keepends=True)`.
fn split_lines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            let mut end = i + c.len_utf8();
            if c == '\r' && it.peek().is_some_and(|&(_, n)| n == '\n') {
                it.next();
                end += 1;
            }
            out.push(&s[start..end]);
            start = end;
        }
    }
    if start < s.len() { out.push(&s[start..]); }
    out
}

/// Recado de fora não pode se passar por aviso do app nem por outra sessão: linha que começa
/// com colchete (depois de invisíveis) perde o colchete.
pub fn sanitize_message(text: &str) -> String {
    let normalized: String = text.nfkc().collect();
    let mut out = String::with_capacity(normalized.len());
    for line in split_lines(&normalized) {
        let n = line.find(|c| !is_invisible(c)).unwrap_or(line.len());
        match line[n..].chars().next() {
            Some(c) if OPENERS.contains(&c) => {
                out.push_str(&line[..n]);
                out.push('(');
                out.push_str(&line[n + c.len_utf8()..]);
            }
            _ => out.push_str(line),
        }
    }
    out.chars().take(MAX_TEXT).collect()
}

static HTTP: OnceLock<reqwest::Client> = OnceLock::new();

/// Chamada ao Funnel de outra pessoa (`peers.call_url` sem redirect): o 3xx vira recusa com o
/// status, porque seguir levaria o token a um destino que ninguém conferiu. Sem token, sem
/// `Authorization`.
pub async fn call(address: &str, token: Option<&str>, method: reqwest::Method, path: &str, body: Option<&Value>)
    -> Result<(u16, Option<Value>), PeerError> {
    let http = match HTTP.get() {
        Some(http) => http,
        None => {
            let built = peers::client(crate::http_client().redirect(reqwest::redirect::Policy::none()))
                .map_err(|e| peers::transport(address, &e))?;
            HTTP.get_or_init(|| built)
        }
    };
    peers::request(http, address, &format!("{address}{path}"), token, method, body).await
}
