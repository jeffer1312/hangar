//! Convites de compartilhamento (`shares.json`): um registro por link gerado.
//!
//! Só hashes vão ao disco; código e token crus existem só na resposta que os entrega. Este
//! processo é o único escritor, então o estado em memória é a verdade e o disco é a cópia.
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub const CODE_TTL: f64 = 24.0 * 3600.0;
pub const KEEP_EXPIRED: f64 = 24.0 * 3600.0;
pub const KEEP_REVOKED: f64 = 30.0 * 24.0 * 3600.0;

pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Share,
    Pair,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Share {
    pub id: String,
    pub session: String,
    pub life: Option<String>,
    pub created_at: f64,
    pub code_expires_at: f64,
    pub code_hash: String,
    #[serde(default)]
    pub token_hash: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub redeemed_at: Option<f64>,
    #[serde(default)]
    pub revoked_at: Option<f64>,
    #[serde(default)]
    pub local: bool,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Nível que o convite de par dará no resgate. No arquivo: reiniciar não pode rebaixá-lo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Kind>,
}


impl Share {
    pub fn active(&self, now: f64) -> bool {
        self.revoked_at.is_none() && (self.redeemed_at.is_some() || now <= self.code_expires_at)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeekError {
    Unknown,
    Revoked,
    Used,
    Expired,
    /// Só do `redeem`: o resgate não chegou ao disco.
    Io(String),
}

/// Tudo o que um token de convidado alcança: um registro por sessão.
#[derive(Debug, Clone)]
pub struct Guest {
    pub token_hash: String,
    pub records: Vec<Share>,
    /// Tem convite de compartilhamento vivo (não acesso vindo de par): só ele gasta o crédito do dono.
    pub invite: bool,
}

impl Guest {
    pub fn share_for(&self, session: &str) -> Option<&Share> {
        let cand: Vec<&Share> = self.records.iter().filter(|s| s.session == session).collect();
        let live: Vec<&Share> = cand.iter().copied().filter(|s| s.revoked_at.is_none()).collect();
        // Só revogado: devolve mesmo assim, para o porteiro dizer "encerrado" e não "fora do convite".
        let pool = if live.is_empty() { cand } else { live };
        // Compartilhada E pareada sob o mesmo token: o convite vence a cópia do par.
        let key = |s: &Share| (s.kind == Kind::Share, s.created_at);
        pool.into_iter().fold(None, |best: Option<&Share>, s| match best {
            Some(b) if key(s).partial_cmp(&key(b)) != Some(std::cmp::Ordering::Greater) => Some(b),
            _ => Some(s),
        })
    }
    pub fn sessions(&self) -> HashSet<String> {
        self.records.iter().filter(|s| s.revoked_at.is_none()).map(|s| s.session.clone()).collect()
    }
    pub fn sees(&self, name: &str) -> bool {
        self.records.iter().any(|s| s.revoked_at.is_none() && s.session == name)
    }
    pub fn kind_of(&self, name: &str) -> Option<Kind> {
        self.share_for(name).map(|s| s.kind)
    }
    pub fn pair_shares(&self) -> Vec<&Share> {
        self.records.iter()
            .filter(|s| s.kind == Kind::Pair && s.parent_id.is_none() && s.revoked_at.is_none())
            .collect()
    }
}

fn hash(value: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, value.as_bytes()).as_ref()
        .iter().map(|b| format!("{b:02x}")).collect()
}

fn norm_code(code: &str) -> String {
    code.split_whitespace().collect::<String>().to_uppercase()
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
        .expect("gerador do sistema indisponível");
    bytes
}

fn new_id() -> String {
    crate::mods::state::random_hex(8)
}

fn new_token() -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random::<32>())
}

/// base32 RFC 4648 sem `=`, igual ao `b32encode(...).rstrip("=")` do Python.
fn new_code() -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut out, mut acc, mut bits) = (String::new(), 0u32, 0u32);
    for byte in random::<16>() {
        acc = (acc << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

type State = IndexMap<String, Share>;

pub struct ShareStore {
    path: PathBuf,
    clock: Clock,
    state: Mutex<State>,
    /// Número da última fotografia tirada, contado sob `state`.
    taken: AtomicU64,
    /// Número da fotografia que está no disco; a gravação acontece sob este lock, fora do `state`.
    written: Mutex<u64>,
}

impl ShareStore {
    pub fn open(path: PathBuf, clock: Clock) -> Self {
        let state = load(&path, (clock)());
        Self { path, clock, state: Mutex::new(state), taken: AtomicU64::new(0), written: Mutex::new(0) }
    }

    /// O relógio da loja (o injetado nos testes).
    pub fn now(&self) -> f64 {
        (self.clock)()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Fotografa sob o lock do estado e grava depois de soltá-lo: quem só lê não espera o fsync.
    fn persist(&self, state: MutexGuard<'_, State>) -> io::Result<()> {
        let records: Vec<&Share> = state.values().collect();
        let bytes = serde_json::to_vec_pretty(&records).map_err(io::Error::other)?;
        let seq = self.taken.fetch_add(1, Ordering::Relaxed) + 1;
        drop(state);
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        // Uma fotografia mais nova já está no disco: gravar esta a desfaria.
        if *written > seq {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::runtime::queue::atomic_write(&self.path, &bytes)?;
        *written = seq;
        Ok(())
    }

    fn insert(&self, share: Share) -> io::Result<()> {
        let mut state = self.lock();
        state.insert(share.id.clone(), share);
        self.persist(state)
    }

    pub fn create(&self, session: &str, life: Option<String>, kind: Kind, local: bool) -> io::Result<(Share, String)> {
        self.create_with(session, life, kind, local, None)
    }

    /// Convite de par com o nível que o resgate dará.
    pub fn create_pair_invite(&self, session: &str, life: Option<String>, level: Kind) -> io::Result<(Share, String)> {
        self.create_with(session, life, Kind::Pair, false, Some(level))
    }

    fn create_with(&self, session: &str, life: Option<String>, kind: Kind, local: bool, level: Option<Kind>) -> io::Result<(Share, String)> {
        let now = self.now();
        let code = new_code();
        let share = Share {
            id: new_id(), session: session.into(), life, created_at: now,
            code_expires_at: now + CODE_TTL, code_hash: hash(&code), token_hash: None, device: None,
            redeemed_at: None, revoked_at: None, local, kind, parent_id: None, level,
        };
        self.insert(share.clone())?;
        Ok((share, code))
    }

    pub fn peek(&self, code: &str, kind: Kind) -> Result<Share, PeekError> {
        let now = self.now();
        let state = self.lock();
        peek_in(&state, code, kind, now).cloned()
    }

    pub fn redeem(&self, code: &str, device: &str, token: Option<&str>, kind: Kind) -> Result<(Share, String), PeekError> {
        let now = self.now();
        let mut state = self.lock();
        let id = peek_in(&state, code, kind, now)?.id.clone();
        // Token que já vale aqui leva a sessão nova junto: o nativo guarda um token por máquina.
        let token = match token.filter(|t| !t.is_empty()) {
            Some(t) if state.values().any(|x| x.token_hash.as_deref() == Some(hash(t).as_str()) && x.revoked_at.is_none()) => t.to_string(),
            _ => new_token(),
        };
        let device: String = device.trim().chars().take(80).collect();
        let share = state.get_mut(&id).expect("registro do peek");
        share.token_hash = Some(hash(&token));
        share.device = (!device.is_empty()).then_some(device);
        share.redeemed_at = Some(now);
        let share = share.clone();
        self.persist(state).map_err(|e| PeekError::Io(e.to_string()))?;
        Ok((share, token))
    }

    /// Registro já resgatado, sem código: o token sai direto para quem o criou.
    pub fn create_redeemed(&self, session: &str, life: Option<String>, kind: Kind) -> io::Result<(Share, String)> {
        let now = self.now();
        let token = new_token();
        let share = Share {
            id: new_id(), session: session.into(), life, created_at: now, code_expires_at: now,
            code_hash: String::new(), token_hash: Some(hash(&token)), device: None,
            redeemed_at: Some(now), revoked_at: None, local: false, kind, parent_id: None, level: None,
        };
        self.insert(share.clone())?;
        Ok((share, token))
    }

    pub fn lookup_token(&self, token: &str) -> Option<Guest> {
        if token.is_empty() {
            return None;
        }
        self.lookup_hash(&hash(token))
    }

    pub fn lookup_hash(&self, h: &str) -> Option<Guest> {
        let state = self.lock();
        let records: Vec<Share> = state.values()
            .filter(|x| x.token_hash.as_deref() == Some(h)).cloned().collect();
        let invite = records.iter().any(|x| x.kind == Kind::Share && x.revoked_at.is_none() && !under_pair(&state, x));
        (!records.is_empty()).then(|| Guest { token_hash: h.to_owned(), records, invite })
    }

    /// Liga ao token `holder` as sessões vivas do token `other`; devolve quantas ligou.
    pub fn attach(&self, holder: &str, other: &str) -> io::Result<usize> {
        let now = self.now();
        let (Some(h), Some(o)) = (self.lookup_token(holder), self.lookup_token(other)) else { return Ok(0) };
        if h.sessions().is_empty() || o.sessions().is_empty() {
            return Ok(0);
        }
        let mut state = self.lock();
        // A cópia aponta sempre para a RAIZ: revogar o registro original derruba toda a cadeia.
        // Uma raiz de par dá várias sessões: a chave é (raiz, sessão), não só a raiz.
        let mut done: HashSet<(String, String)> = state.values()
            .filter(|x| x.token_hash.as_deref() == Some(h.token_hash.as_str()) && x.revoked_at.is_none())
            .filter_map(|x| x.parent_id.clone().map(|p| (p, x.session.clone()))).collect();
        let mut added = Vec::new();
        for s in &o.records {
            let root = s.parent_id.clone().unwrap_or_else(|| s.id.clone());
            if s.revoked_at.is_some() || !done.insert((root.clone(), s.session.clone())) {
                continue;
            }
            added.push(Share {
                id: new_id(), session: s.session.clone(), life: s.life.clone(), created_at: now,
                code_expires_at: now, code_hash: String::new(), token_hash: Some(h.token_hash.clone()),
                device: None, redeemed_at: Some(now), revoked_at: None, local: false, kind: s.kind,
                parent_id: Some(root), level: None,
            });
        }
        let count = added.len();
        for share in added {
            state.insert(share.id.clone(), share);
        }
        if count > 0 {
            self.persist(state)?;
        }
        Ok(count)
    }

    /// Dá ao par a sessão com o nível `kind`, ou troca o nível de quem já a tinha pelo par.
    pub fn grant(&self, root_id: &str, session: &str, life: Option<String>, kind: Kind) -> io::Result<()> {
        self.grant_many(root_id, &[(session.to_owned(), life)], kind)
    }

    /// O `grant` de várias sessões `(sessão, vida)` sob a mesma raiz, gravando o arquivo uma vez.
    pub fn grant_many(&self, root_id: &str, sessions: &[(String, Option<String>)], kind: Kind) -> io::Result<()> {
        if sessions.is_empty() { return Ok(()) }
        let now = self.now();
        let mut state = self.lock();
        let token = state.get(root_id)
            .filter(|r| r.kind == Kind::Pair && r.parent_id.is_none() && r.revoked_at.is_none())
            .and_then(|r| r.token_hash.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "par desconhecido ou encerrado"))?;
        // Quem já ligou a raiz pelo `attach` recebe também a sessão nova, senão o membro que
        // entrou depois só aparece no token do par.
        let mut all_holders: HashSet<String> = state.values()
            .filter(|x| x.parent_id.as_deref() == Some(root_id) && x.revoked_at.is_none())
            .filter_map(|x| x.token_hash.clone()).collect();
        all_holders.insert(token);
        for (session, life) in sessions {
            let mut holders = all_holders.clone();
            // As cópias que o `attach` deu a outros tokens seguem o nível: senão escreveriam depois do "ver".
            for x in state.values_mut().filter(|x| x.parent_id.as_deref() == Some(root_id) && x.session == *session && x.revoked_at.is_none()) {
                if let Some(t) = &x.token_hash { holders.remove(t); }
                x.kind = kind;
                x.life = life.clone();
            }
            for t in holders {
                let child = Share {
                    id: new_id(), session: session.clone(), life: life.clone(), created_at: now, code_expires_at: now,
                    code_hash: String::new(), token_hash: Some(t), device: None, redeemed_at: Some(now),
                    revoked_at: None, local: false, kind, parent_id: Some(root_id.into()), level: None,
                };
                state.insert(child.id.clone(), child);
            }
        }
        self.persist(state)
    }

    /// Tira a sessão do par, cópias de outros tokens incluídas; `false` se ele não a dava.
    pub fn ungrant(&self, root_id: &str, session: &str) -> io::Result<bool> {
        let now = self.now();
        let mut state = self.lock();
        let mut found = false;
        for x in state.values_mut().filter(|x| x.parent_id.as_deref() == Some(root_id) && x.session == session && x.revoked_at.is_none()) {
            x.revoked_at = Some(now);
            found = true;
        }
        if found { self.persist(state)?; }
        Ok(found)
    }

    /// Filhos vivos sob a chave do próprio par (as cópias de outros tokens ficam de fora).
    pub fn children(&self, root_id: &str) -> Vec<Share> {
        let state = self.lock();
        let Some(token) = state.get(root_id).and_then(|r| r.token_hash.clone()) else { return Vec::new() };
        state.values()
            .filter(|x| x.parent_id.as_deref() == Some(root_id) && x.revoked_at.is_none() && x.token_hash.as_deref() == Some(token.as_str()))
            .cloned().collect()
    }

    /// Por raiz de par: as sessões que o próprio token dela vê com acesso completo (filho `Share`
    /// vivo de `children`). Uma volta na loja para todos os pares.
    pub fn full_sessions(&self) -> HashMap<String, HashSet<String>> {
        let state = self.lock();
        let mut out: HashMap<String, HashSet<String>> = HashMap::new();
        for x in state.values().filter(|x| x.kind == Kind::Share && x.revoked_at.is_none()) {
            let Some(root) = x.parent_id.as_ref().and_then(|p| state.get(p)) else { continue };
            if root.token_hash.is_some() && x.token_hash == root.token_hash {
                out.entry(root.id.clone()).or_default().insert(x.session.clone());
            }
        }
        out
    }

    pub fn list_for(&self, session: &str) -> Vec<Share> {
        let now = self.now();
        let state = self.lock();
        let mut out: Vec<Share> = state.values()
            .filter(|x| x.session == session && x.kind == Kind::Share && x.active(now) && !under_pair(&state, x))
            .cloned().collect();
        out.sort_by(|a, b| a.created_at.total_cmp(&b.created_at));
        out
    }

    pub fn all(&self) -> Vec<Share> {
        self.lock().values().cloned().collect()
    }

    pub fn revoke(&self, id: &str) -> io::Result<bool> {
        let now = self.now();
        let mut state = self.lock();
        if state.get(id).is_none_or(|s| s.revoked_at.is_some()) {
            return Ok(false);
        }
        for x in state.values_mut() {
            if (x.id == id || x.parent_id.as_deref() == Some(id)) && x.revoked_at.is_none() {
                x.revoked_at = Some(now);
            }
        }
        self.persist(state)?;
        Ok(true)
    }

    pub fn revoke_session(&self, session: &str, kind: Option<Kind>) -> io::Result<usize> {
        let now = self.now();
        let mut state = self.lock();
        // Com tipo é o "encerrar" de um recurso (convite ou par); o acesso dado pelo par só sai pelo par.
        let paired: HashSet<String> = state.values().filter(|x| under_pair(&state, x)).map(|x| x.id.clone()).collect();
        let mut count = 0;
        for x in state.values_mut() {
            if x.session == session && x.revoked_at.is_none() && kind.is_none_or(|k| x.kind == k && !paired.contains(&x.id)) {
                x.revoked_at = Some(now);
                count += 1;
            }
        }
        if count > 0 {
            self.persist(state)?;
        }
        Ok(count)
    }

    pub fn rename(&self, old: &str, new: &str) -> io::Result<()> {
        let mut state = self.lock();
        let mut moved = false;
        for x in state.values_mut().filter(|x| x.session == old) {
            x.session = new.into();
            moved = true;
        }
        if !moved { return Ok(()); }
        // Falha fica com o nome novo na memória: a gravação pode ter chegado ao disco, e quem chama
        // desfaz com o rename de volta, que regrava os dois.
        self.persist(state)
    }

    pub fn set_life(&self, session: &str, life: Option<String>) -> io::Result<()> {
        let Some(life) = life else { return Ok(()) };
        let mut state = self.lock();
        let mut changed = false;
        for x in state.values_mut().filter(|x| x.session == session && x.revoked_at.is_none()) {
            x.life = Some(life.clone());
            changed = true;
        }
        if changed { self.persist(state) } else { Ok(()) }
    }

    // Revogado fica guardado por dias, então "tem registro" cobre o funnel a desligar depois do
    // último revoke; quem nunca compartilhou não tem nenhum.
    pub fn has_any(&self, internet_only: bool) -> bool {
        self.lock().values().any(|x| !(internet_only && x.local))
    }

    pub fn has_active(&self, internet_only: bool) -> bool {
        let now = self.now();
        self.lock().values().any(|x| x.active(now) && !(internet_only && x.local))
    }

    pub fn recently_ended(&self, window_s: f64, internet_only: bool) -> bool {
        let now = self.now();
        self.lock().values().any(|x| {
            x.revoked_at.is_some_and(|r| now - r <= window_s) && !(internet_only && x.local)
        })
    }

    pub fn active_sessions(&self) -> HashSet<String> {
        let now = self.now();
        let state = self.lock();
        state.values().filter(|x| x.kind == Kind::Share && x.active(now) && !under_pair(&state, x))
            .map(|x| x.session.clone()).collect()
    }

    /// Revoga convites de sessão que acabou ou renasceu com o mesmo nome; apaga os velhos.
    pub fn sweep(&self, alive: impl Fn(&str, Option<&str>) -> bool) -> io::Result<usize> {
        let now = self.now();
        // `alive` consulta o tmux (segundos por convite): fora do lock.
        let candidates: Vec<(String, String, Option<String>)> = self.lock().values()
            .filter(|x| x.revoked_at.is_none() && x.active(now))
            .map(|x| (x.id.clone(), x.session.clone(), x.life.clone())).collect();
        let dead: Vec<_> = candidates.into_iter()
            .filter(|(_, session, life)| !alive(session, life.as_deref())).collect();
        let mut state = self.lock();
        let mut revoked = 0;
        for (id, session, life) in dead {
            // Mudou no meio (trocou de modo, foi revogado): a leitura de fora do lock não vale.
            if let Some(x) = state.get_mut(&id)
                && x.revoked_at.is_none() && x.session == session && x.life == life
            {
                x.revoked_at = Some(now);
                revoked += 1;
            }
        }
        let before = state.len();
        state.retain(|_, x| {
            let old_revoked = x.revoked_at.is_some_and(|r| now - r > KEEP_REVOKED);
            let old_unused = x.redeemed_at.is_none() && x.revoked_at.is_none() && now > x.code_expires_at + KEEP_EXPIRED;
            !(old_revoked || old_unused)
        });
        if revoked > 0 || state.len() != before {
            self.persist(state)?;
        }
        Ok(revoked)
    }
}

/// Acesso que veio de um par (filho da chave do par ou cópia dele): não é convite do dono.
fn under_pair(state: &State, x: &Share) -> bool {
    x.parent_id.as_ref().and_then(|p| state.get(p)).is_some_and(|r| r.kind == Kind::Pair)
}

fn peek_in<'a>(state: &'a State, code: &str, kind: Kind, now: f64) -> Result<&'a Share, PeekError> {
    let h = hash(&norm_code(code));
    // Código de par não abre como convite (e vice-versa): quem o recebe nem fica sabendo que existe.
    let share = state.values().find(|x| x.code_hash == h).filter(|x| x.kind == kind).ok_or(PeekError::Unknown)?;
    if share.revoked_at.is_some() {
        return Err(PeekError::Revoked);
    }
    if share.redeemed_at.is_some() {
        return Err(PeekError::Used);
    }
    if now > share.code_expires_at {
        return Err(PeekError::Expired);
    }
    Ok(share)
}

fn load(path: &std::path::Path, now: f64) -> State {
    let parsed = std::fs::read(path).map_err(|e| e.to_string()).and_then(|bytes| {
        serde_json::from_slice::<Vec<Share>>(&bytes).map_err(|e| e.to_string())
    });
    match parsed {
        Ok(records) => records.into_iter().map(|s| (s.id.clone(), s)).collect(),
        Err(_) if !path.exists() => State::new(),
        Err(error) => {
            // Ilegível vira "nenhum convite": perde acessos, mas não derruba o servidor. Vai pro
            // lado, senão a próxima gravação o apagaria de vez.
            tracing::warn!("[share] {} ilegivel, comecando vazio: {error}", path.display());
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let bad = path.with_file_name(format!("{name}.bad-{}", now as i64));
            if let Err(e) = std::fs::rename(path, &bad) {
                tracing::warn!("[share] nao consegui guardar {name} ilegivel: {e}");
            }
            State::new()
        }
    }
}
