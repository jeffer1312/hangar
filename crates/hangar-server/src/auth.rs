//! Credenciais e limite por origem compartilhados pelas rotas do dono.
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, Method, header};
use subtle::ConstantTimeEq;

pub use hangar_auth::{AuthAttempt, AuthDecision, AuthError, Profile, ReceiptContext};
#[cfg(test)]
const MAX_FAILS: usize = 8;
const COOKIE: &str = "cp_token";
const COOKIE_HOST: &str = "__Host-cp_token";

pub fn is_loopback(ip: &str) -> bool {
    matches!(ip, "127.0.0.1" | "::1" | "localhost")
}

#[derive(Clone)]
pub struct Auth {
    token: Vec<u8>,
    backend: Backend,
    clock: (Instant, f64),
}
#[derive(Clone)]
enum Backend {
    Local(Arc<Mutex<hangar_auth::Origins>>),
    Shared(Arc<hangar_auth::SharedLimiter>),
    Broken(AuthError),
}

impl Auth {
    pub fn new(token: &str) -> Auth {
        Auth { token: token.as_bytes().to_vec(), backend: Backend::Local(Arc::default()),
            clock: (Instant::now(), SystemTime::now().duration_since(UNIX_EPOCH).expect("relógio Unix").as_secs_f64()) }
    }

    pub fn configured(token: &str) -> Self {
        let mut auth = Self::new(token);
        if let Some(raw) = std::env::var_os("HANGAR_AUTH_CONTEXT") {
            auth.backend = match raw.to_str().and_then(|s| serde_json::from_str::<hangar_auth::AuthConfig>(s).ok()) {
                Some(config) if config.mode == "attach" => match hangar_auth::SharedLimiter::open(config) {
                    Ok(shared) => Backend::Shared(Arc::new(shared)), Err(e) => Backend::Broken(e),
                },
                _ => Backend::Broken(AuthError::InvalidInput),
            };
        }
        auth
    }

    pub fn shared(&self) -> bool { !matches!(self.backend, Backend::Local(_)) }
    fn wall(&self, now: Instant) -> f64 {
        if now >= self.clock.0 { self.clock.1 + now.duration_since(self.clock.0).as_secs_f64() }
        else { self.clock.1 - self.clock.0.duration_since(now).as_secs_f64() }
    }
    fn operation_time(&self) -> Result<f64, AuthError> {
        if self.shared() {
            SystemTime::now().duration_since(UNIX_EPOCH).map(|t| t.as_secs_f64()).map_err(|_| AuthError::StoreUnavailable)
        } else { Ok(self.wall(Instant::now())) }
    }
    fn transaction<T>(&self, op: impl FnOnce(&mut hangar_auth::Origins) -> T) -> Result<T, AuthError> {
        match &self.backend {
            Backend::Local(origins) => {
                let mut origins = origins.lock().map_err(|_| AuthError::Panic)?;
                Ok(op(&mut origins))
            },
            Backend::Shared(shared) => shared.transaction(op),
            Backend::Broken(error) => Err(*error),
        }
    }

    pub async fn evaluate(&self, attempt: AuthAttempt) -> Result<AuthDecision, AuthError> {
        let auth = self.clone();
        let origin = attempt.origin.clone();
        let decision = tokio::task::spawn_blocking(move || match &auth.backend {
            Backend::Shared(shared) => shared.evaluate(&attempt),
            Backend::Broken(error) => Err(*error),
            Backend::Local(_) => auth.transaction(|origins| origins.evaluate(&auth.token, &attempt, auth.wall(Instant::now()))),
        }).await.map_err(|_| AuthError::Panic)??;
        if decision.became_blocked != 0 { tracing::warn!(ip=%origin, "token errado 8 vezes em 30 s; origem bloqueada"); }
        Ok(decision)
    }
    pub fn evaluate_at_local(&self, attempt: AuthAttempt, now: Instant) -> AuthDecision {
        assert!(!self.shared(), "autenticação compartilhada exige a operação assíncrona");
        self.transaction(|origins| origins.evaluate(&self.token, &attempt, self.wall(now))).expect("contador local")
    }
    pub async fn owner(&self, ip: &str, token: Option<&[u8]>) -> Result<bool, AuthError> {
        self.evaluate(AuthAttempt { profile: Profile::LegacyOwnerProbe, origin: ip.into(),
            presented_token: token.map(Vec::from), verified_guest: false, login_guest: false }).await.map(|d| d.status == 0)
    }
    pub async fn record_failure(&self, ip: &str) -> Result<(), AuthError> {
        let auth = self.clone(); let ip = ip.to_owned();
        let origin = ip.clone();
        let decision = tokio::task::spawn_blocking(move || auth.transaction(|origins| {
            Ok::<_, AuthError>(origins.record_failure(&ip, auth.operation_time()?))
        })?).await.map_err(|_| AuthError::Panic)??;
        if decision.became_blocked != 0 { tracing::warn!(ip=%origin, "token errado 8 vezes em 30 s; origem bloqueada"); }
        Ok(())
    }
    pub async fn mark_blocked_async(&self, ip: &str) -> Result<(), AuthError> {
        let auth = self.clone(); let ip = ip.to_owned();
        tokio::task::spawn_blocking(move || auth.transaction(|origins| {
            let now = auth.operation_time()?;
            origins.is_blocked(&ip, now);
            for _ in 0..hangar_auth::MAX_FAILS { origins.record_failure(&ip, now); }
            Ok::<_, AuthError>(())
        })?).await.map_err(|_| AuthError::Panic)?
    }
    pub async fn receipt(&self, context: &ReceiptContext) -> Result<Option<String>, AuthError> {
        let auth = self.clone();
        let context = context.clone();
        tokio::task::spawn_blocking(move || match &auth.backend {
            Backend::Shared(shared) => shared.receipt(&context).map(Some),
            Backend::Local(_) => Ok(None), Backend::Broken(error) => Err(*error),
        }).await.map_err(|_| AuthError::Panic)?
    }

    // Compatibilidade dos ensaios locais; chamadas de produção usam a operação assíncrona.
    pub fn is_owner(&self, ip: &str, token: Option<&[u8]>) -> bool {
        assert!(!self.shared(), "autenticação compartilhada exige a operação assíncrona");
        self.transaction(|origins| origins.evaluate(&self.token, &AuthAttempt { profile: Profile::LegacyOwnerProbe,
            origin: ip.into(), presented_token: token.map(Vec::from), verified_guest: false, login_guest: false },
            self.wall(Instant::now())).status == 0).expect("contador local")
    }

    /// Comparação sem efeito na contagem: o WebSocket conserva falhas mesmo no acerto.
    pub fn token_matches(&self, token: Option<&[u8]>) -> bool {
        // Fatias de tamanho diferente saem na hora, como o compare_digest.
        token.is_some_and(|t| bool::from(t.ct_eq(self.token.as_slice())))
    }

    pub fn clear_failures(&self, ip: &str) {
        assert!(!self.shared(), "autenticação compartilhada exige a operação assíncrona");
        self.transaction(|o| o.clear_origin(ip)).expect("contador local");
    }

    pub fn is_blocked_at(&self, ip: &str, now: Instant) -> bool {
        assert!(!self.shared(), "autenticação compartilhada exige a operação assíncrona");
        self.transaction(|o| o.is_blocked(ip, self.wall(now))).expect("contador local")
    }

    pub fn record_fail(&self, ip: &str) {
        self.record_fail_at(ip, Instant::now());
    }

    pub fn record_fail_at(&self, ip: &str, now: Instant) {
        assert!(!self.shared(), "autenticação compartilhada exige a operação assíncrona");
        self.transaction(|o| { o.record_failure(ip, self.wall(now)); }).expect("contador local");
    }

    pub fn mark_blocked(&self, ip: &str) {
        for _ in 0..hangar_auth::MAX_FAILS { self.record_fail(ip); }
    }
}

pub fn unavailable(error: AuthError) -> axum::response::Response {
    crate::session_write::json_response(axum::http::StatusCode::SERVICE_UNAVAILABLE,
        serde_json::json!({"detail": {"error_code": error.to_string(), "message": "autenticação indisponível"}}))
}

/// Token apresentado, na ordem do require_auth: Bearer, ?token=, cookie (só GET/HEAD).
pub fn presented_token(headers: &HeaderMap, query: Option<&str>, method: &Method, https: bool) -> Option<Vec<u8>> {
    if let Some(v) = headers.get(header::AUTHORIZATION) {
        if let Some(t) = v.as_bytes().strip_prefix(b"Bearer ") {
            return Some(t.to_vec());
        }
    }
    if let Some(q) = query_param(query, "token").filter(|q| !q.is_empty()) {
        return Some(q.into_bytes());
    }
    // O cookie vai junto em pedido de outra página do mesmo site: só serve para ler.
    if method != Method::GET && method != Method::HEAD {
        return None;
    }
    let jar = cookies(headers);
    // `__Host-` só nasce numa página https deste host; o `cp_token` sem prefixo outra máquina do
    // mesmo site consegue gravar, e em https ele não vale.
    jar.get(COOKIE_HOST)
        .filter(|v| !v.is_empty())
        .or_else(|| if https { None } else { jar.get(COOKIE).filter(|v| !v.is_empty()) })
        .map(|v| v.as_bytes().to_vec())
}

/// Valor de um parâmetro da query; repetido, vale o último (QueryParams do Starlette).
pub fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    form_urlencoded::parse(query?.as_bytes())
        .filter(|(k, _)| k == key)
        .last()
        .map(|(_, v)| v.into_owned())
}

/// cookie_parser do Starlette: `;` separa, `=` só no primeiro, aspas em volta saem.
fn cookies(headers: &HeaderMap) -> HashMap<String, String> {
    let mut jar = HashMap::new();
    let Some(raw) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) else { return jar };
    for chunk in raw.split(';') {
        let (k, v) = chunk.split_once('=').unwrap_or(("", chunk));
        let (k, v) = (k.trim(), v.trim());
        if !k.is_empty() || !v.is_empty() {
            let v = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
            jar.insert(k.to_string(), v.to_string());
        }
    }
    jar
}

/// `forwarded_allow_ips` do uvicorn: IPs, redes CIDR, `*` e nomes literais.
#[derive(Clone, Debug, Default)]
pub struct TrustedHosts {
    all: bool,
    hosts: Vec<IpAddr>,
    nets: Vec<(IpAddr, u8)>,
    literals: Vec<String>,
}

impl TrustedHosts {
    pub fn parse(raw: &str) -> TrustedHosts {
        let raw = raw.trim();
        if raw == "*" {
            return TrustedHosts { all: true, ..TrustedHosts::default() };
        }
        let mut t = TrustedHosts::default();
        for item in raw.split(',').map(str::trim).filter(|i| !i.is_empty()) {
            if let Some((addr, bits)) = item.split_once('/') {
                match (addr.parse::<IpAddr>(), bits.parse::<u8>()) {
                    (Ok(a), Ok(b)) if b <= if a.is_ipv4() { 32 } else { 128 } => t.nets.push((a, b)),
                    _ => t.literals.push(item.to_string()),
                }
            } else {
                match item.parse::<IpAddr>() {
                    Ok(a) => t.hosts.push(a),
                    Err(_) => t.literals.push(item.to_string()),
                }
            }
        }
        t
    }

    pub fn contains(&self, host: &str) -> bool {
        if self.all {
            return true;
        }
        if host.is_empty() {
            return false;
        }
        match host.parse::<IpAddr>() {
            Ok(ip) => self.hosts.contains(&ip) || self.nets.iter().any(|(n, b)| in_net(ip, *n, *b)),
            Err(_) => self.literals.iter().any(|l| l == host),
        }
    }

    /// Primeiro endereço não confiável da direita para a esquerda; todos confiáveis = o primeiro.
    pub fn client_from_xff(&self, xff: &str) -> String {
        let hosts: Vec<&str> = xff.split(',').map(str::trim).collect();
        if self.all {
            return host_of(hosts[0]).to_string();
        }
        for hp in hosts.iter().rev() {
            let h = host_of(hp);
            if !self.contains(h) {
                return h.to_string();
            }
        }
        host_of(hosts[0]).to_string()
    }

    /// (ip do cliente, https?) como o uvicorn resolve com proxy_headers: só um vizinho confiável
    /// reescreve o cliente e o esquema. Cabeçalho ilegível ou que não dá um IP volta ao vizinho
    /// TCP: nunca vira um endereço presumido (127.0.0.1 escaparia do bloqueio por origem).
    pub fn resolve(&self, peer: IpAddr, headers: &HeaderMap) -> (String, bool) {
        let peer = peer.to_canonical().to_string();
        if !self.contains(&peer) {
            return (peer, false);
        }
        let proto = headers
            .get_all("x-forwarded-proto")
            .iter()
            .last()
            .and_then(|v| v.to_str().ok())
            .map(str::trim);
        let https = matches!(proto, Some("https" | "wss"));
        let mut xff: Vec<&str> = Vec::new();
        for v in headers.get_all("x-forwarded-for") {
            match v.to_str() {
                Ok(s) => xff.push(s),
                Err(_) => return (peer, https),
            }
        }
        if xff.is_empty() {
            return (peer, https);
        }
        let host = self.client_from_xff(&xff.join(", "));
        match host.parse::<IpAddr>() {
            Ok(ip) => (ip.to_canonical().to_string(), https),
            Err(_) => (peer, https),
        }
    }
}

fn in_net(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let m = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(a) & m == u32::from(n) & m
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let m = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(a) & m == u128::from(n) & m
        }
        _ => false,
    }
}

/// `_parse_host_port` do uvicorn, só a parte do host.
fn host_of(value: &str) -> &str {
    if let Some(rest) = value.strip_prefix('[') {
        return match rest.find(']') {
            None => value,
            Some(end) => {
                let after = &rest[end + 1..];
                if after.is_empty() || after.starts_with(':') { &rest[..end] } else { value }
            }
        };
    }
    if value.matches(':').count() == 1 {
        let (h, p) = value.rsplit_once(':').expect("um ':' contado acima");
        if p.trim().parse::<i64>().is_ok() {
            return h;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn h(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        m
    }

    #[test]
    fn bearer_wins_over_query_and_cookie() {
        let hs = h(&[("authorization", "Bearer convidado"), ("cookie", "cp_token=dono")]);
        assert_eq!(presented_token(&hs, Some("token=dono"), &Method::GET, false), Some(b"convidado".to_vec()));
    }

    #[test]
    fn query_before_cookie_and_last_value_wins() {
        let hs = h(&[("cookie", "cp_token=c")]);
        assert_eq!(presented_token(&hs, Some("token=a&token=b"), &Method::GET, false), Some(b"b".to_vec()));
        assert_eq!(presented_token(&hs, Some("token="), &Method::GET, false), Some(b"c".to_vec()));
    }

    #[test]
    fn cookie_only_on_get_and_head() {
        let hs = h(&[("cookie", "cp_token=dono")]);
        assert_eq!(presented_token(&hs, None, &Method::POST, false), None);
        assert_eq!(presented_token(&hs, None, &Method::HEAD, false), Some(b"dono".to_vec()));
    }

    #[test]
    fn https_prefers_host_cookie_and_ignores_plain_one() {
        let both = h(&[("cookie", "cp_token=x; __Host-cp_token=y")]);
        assert_eq!(presented_token(&both, None, &Method::GET, true), Some(b"y".to_vec()));
        let plain = h(&[("cookie", "cp_token=x")]);
        assert_eq!(presented_token(&plain, None, &Method::GET, true), None);
        assert_eq!(presented_token(&plain, None, &Method::GET, false), Some(b"x".to_vec()));
    }

    #[test]
    fn owner_compare_block_and_loopback() {
        let a = Auth::new("tok");
        assert!(a.is_owner("192.0.2.5", Some(b"tok")));
        assert!(!a.is_owner("192.0.2.5", Some(b"tok-maior")));
        assert!(!a.is_owner("192.0.2.5", None));
        for _ in 0..MAX_FAILS {
            a.record_fail("192.0.2.5");
        }
        assert!(!a.is_owner("192.0.2.5", Some(b"tok")), "bloqueado não avalia o token");
        for _ in 0..MAX_FAILS {
            a.record_fail("127.0.0.1");
        }
        assert!(a.is_owner("127.0.0.1", Some(b"tok")), "loopback é isento");
    }

    #[test]
    fn owner_success_clears_the_origin() {
        let a = Auth::new("tok");
        for _ in 0..MAX_FAILS - 1 {
            a.record_fail("192.0.2.6");
        }
        assert!(a.is_owner("192.0.2.6", Some(b"tok")));
        a.record_fail("192.0.2.6");
        assert!(a.is_owner("192.0.2.6", Some(b"tok")));
    }

    #[test]
    fn trusted_hosts_walk_xff_like_uvicorn() {
        let t = TrustedHosts::parse("127.0.0.1, 10.0.0.0/8");
        assert!(t.contains("10.2.3.4") && t.contains("127.0.0.1") && !t.contains("192.0.2.1"));
        assert_eq!(t.client_from_xff("203.0.113.9, 10.0.0.2"), "203.0.113.9");
        assert_eq!(t.client_from_xff("10.0.0.3, 10.0.0.2"), "10.0.0.3");
        assert_eq!(t.client_from_xff("[2001:db8::1]:443"), "2001:db8::1");
        assert_eq!(t.client_from_xff("198.51.100.4:5000"), "198.51.100.4");
        let all = TrustedHosts::parse("*");
        assert_eq!(all.client_from_xff("198.51.100.1, 198.51.100.2"), "198.51.100.1");
    }

    #[test]
    fn resolve_only_trusts_a_trusted_peer() {
        let t = TrustedHosts::parse("127.0.0.1");
        let hs = h(&[("x-forwarded-for", "198.51.100.7"), ("x-forwarded-proto", "https")]);
        assert_eq!(t.resolve("127.0.0.1".parse().unwrap(), &hs), ("198.51.100.7".to_string(), true));
        assert_eq!(t.resolve("192.0.2.9".parse().unwrap(), &hs), ("192.0.2.9".to_string(), false));
        assert_eq!(
            t.resolve("::ffff:127.0.0.1".parse().unwrap(), &HeaderMap::new()),
            ("127.0.0.1".to_string(), false)
        );
    }

    #[test]
    fn resolve_falls_back_to_the_tcp_peer_on_unusable_xff() {
        let t = TrustedHosts::parse("127.0.0.1");
        let peer: IpAddr = "127.0.0.1".parse().unwrap();
        let mut non_ascii = HeaderMap::new();
        non_ascii.insert("x-forwarded-for", HeaderValue::from_bytes(b"\xff\xfe").unwrap());
        assert_eq!(t.resolve(peer, &non_ascii).0, "127.0.0.1");
        let garbage = h(&[("x-forwarded-for", "nao-e-ip")]);
        assert_eq!(t.resolve(peer, &garbage).0, "127.0.0.1");
        // Um vizinho de rede (não loopback) confiável também cai nele, nunca em 127.0.0.1.
        let net = TrustedHosts::parse("10.0.0.0/8");
        assert_eq!(net.resolve("10.1.2.3".parse().unwrap(), &garbage).0, "10.1.2.3");
    }
}
