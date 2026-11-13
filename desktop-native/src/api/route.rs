//! Rota até cada servidor, como o `rota.ts` do web: o endereço da rede local quando ele responde como a MESMA máquina,
//! senão o salvo, e o Connect só quando os dois falham. O salvo continua sendo a identidade (`Api::identity`); só o
//! caminho da rede muda.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};
use reqwest::Client;
use serde_json::{Value, json};
use url::Url;
use super::Api;

// Rede local responde em poucos ms; fora dela o IP não existe ou é outra máquina.
const LAN_TIMEOUT: Duration = Duration::from_millis(800);
const SAVED_TIMEOUT: Duration = Duration::from_secs(4);
const ENROLL_TIMEOUT: Duration = Duration::from_secs(15);

/// O endereço da rede local que o servidor informou e o nome com que ele se apresentou. `url` vazio: sem acesso local.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lan { pub url: String, pub id: String }

/// O endereço do Connect da máquina e a credencial de aparelho que ela emitiu para este app. A credencial só vai a
/// pedidos para esse endereço; os outros caminhos seguem com a senha do dono.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectRoute { pub url: String, pub id: String, pub device_id: String, pub token: String }

/// O que a decisão aprendeu e quem guarda a entrada precisa gravar.
pub enum Learned {
    Lan(Lan),
    Connect(ConnectRoute),
    /// 401 pelo Connect: o dono revogou a credencial, que não é pedida de novo sozinha.
    ConnectRefused,
}

#[derive(Default)]
struct Known {
    invite: bool,
    lan: Option<Lan>,
    route: Option<Url>,
    deciding: Arc<tokio::sync::Mutex<()>>,
    connect: Option<(ConnectRoute, Client)>,
    /// Todo endereço do Connect que esta máquina já teve: a senha do dono nunca vai a eles, nem depois da recusa.
    connect_origins: Vec<String>,
    refused: bool,
    enrolling: bool,
}

/// Quem leva a credencial a uma URL da máquina.
pub(super) enum Credential {
    Owner,
    /// Cliente e token da credencial de aparelho.
    Device(Client, String),
    /// Endereço do Connect sem credencial de aparelho: o pedido falha no app, sem sair.
    Blocked,
}

/// Escolha da credencial para `url` na máquina `identity`, a mesma para HTTP (`Authed`) e socket (`socket_token`).
pub(super) fn credential(identity: &str, url: &Url) -> Credential {
    with(|t| {
        let Some(known) = t.get(identity) else { return Credential::Owner };
        match &known.connect {
            Some((c, client)) if same_origin(&c.url, url) => Credential::Device(client.clone(), c.token.clone()),
            _ if known.connect_origins.iter().any(|origin| same_origin(origin, url)) => Credential::Blocked,
            _ => Credential::Owner,
        }
    })
}

/// URL sem host: o reqwest recusa antes de abrir conexão, então nada sai do app.
pub(crate) fn blocked_url() -> Url { Url::parse("hangar-blocked:connect-refused").expect("URL fixa") }

static TABLE: LazyLock<Mutex<HashMap<String, Known>>> = LazyLock::new(Default::default);
type Notify = Box<dyn Fn(String, Learned) + Send + Sync>;
static LEARNED: Mutex<Option<Notify>> = Mutex::new(None);
static DEVICE_NAME: OnceLock<String> = OnceLock::new();

fn with<R>(f: impl FnOnce(&mut HashMap<String, Known>) -> R) -> R {
    f(&mut TABLE.lock().unwrap_or_else(|e| e.into_inner()))
}

fn notify(key: String, learned: Learned) {
    if let Some(notify) = LEARNED.lock().unwrap_or_else(|e| e.into_inner()).as_ref() { notify(key, learned); }
}

/// Mesma normalização do `Api::new`, para a chave casar com `Api::identity`.
fn key(address: &str) -> Option<String> {
    let mut url = Url::parse(address.trim()).ok()?;
    if !url.path().ends_with('/') { url.set_path(&format!("{}/", url.path())); }
    Some(url.into())
}

fn same_origin(stored: &str, url: &Url) -> bool { Url::parse(stored).is_ok_and(|s| s.origin() == url.origin()) }

/// Troca a credencial do Connect. A rota que passava pelo endereço antigo é decidida de novo.
fn set_connect(known: &mut Known, connect: Option<(ConnectRoute, Client)>) {
    let stale = matches!((&known.connect, &known.route), (Some((old, _)), Some(route)) if same_origin(&old.url, route));
    if stale { known.route = None; }
    if let Some((c, _)) = &connect && !known.connect_origins.contains(&c.url) { known.connect_origins.push(c.url.clone()); }
    known.connect = connect;
}

/// O que a entrada guardada sabe. A rede local aprendida nesta execução vale mais que a do arquivo, e a recusa
/// aprendida aqui não é desfeita pelo arquivo que ainda não a gravou.
pub fn seed(address: &str, invite: bool, lan: Option<&Lan>, connect: Option<&ConnectRoute>, refused: bool) {
    let Some(key) = key(address) else { return };
    let changed = connect.filter(|c| with(|t| t.get(&key).and_then(|k| k.connect.as_ref()).map(|(known, _)| known) != Some(*c)));
    let client = changed.and_then(|c| super::bearer_client(&c.token).ok());
    with(|t| {
        let known = t.entry(key).or_default();
        known.invite = invite;
        if known.lan.is_none() { known.lan = lan.cloned(); }
        known.refused |= refused;
        if known.refused { set_connect(known, None); }
        else if let (Some(connect), Some(client)) = (changed, client) { set_connect(known, Some((connect.clone(), client))); }
    });
}

/// Quem guarda a entrada ouve aqui cada coisa aprendida (identidade, o quê). `device_name` é o nome com que este app
/// aparece na lista de aparelhos da máquina.
pub fn on_learned(device_name: String, f: impl Fn(String, Learned) + Send + Sync + 'static) {
    let _ = DEVICE_NAME.set(device_name);
    *LEARNED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
}

/// Conexão caiu: a rede pode ter mudado, a próxima conexão decide de novo.
pub fn forget(api: &Api) {
    with(|t| if let Some(known) = t.get_mut(api.base.as_str()) { known.route = None; });
}

/// O dono trocou o endereço ou o token da máquina: a recusa do Connect deixa de valer e a credencial pode ser pedida
/// de novo na próxima decisão.
pub fn reset_refused(address: &str) {
    let Some(key) = key(address) else { return };
    with(|t| if let Some(known) = t.get_mut(&key) { known.refused = false; known.enrolling = false; });
}

/// 401 vindo do endereço do Connect: a credencial de aparelho foi revogada.
pub(crate) fn refused_at(url: &Url) {
    // As rotas do próprio Connect respondem 401 na porta dele mesmo com credencial válida.
    if url.path().contains("/api/connect/") { return; }
    let keys: Vec<String> = with(|t| t.iter_mut()
        .filter(|(_, k)| k.connect.as_ref().is_some_and(|(c, _)| same_origin(&c.url, url)))
        .map(|(key, k)| { set_connect(k, None); k.refused = true; key.clone() }).collect());
    for key in keys {
        crate::log_line(&format!("connect: credencial de aparelho recusada (401) para {key}; o Connect sai dos caminhos dela"));
        notify(key, Learned::ConnectRefused);
    }
}

impl Api {
    /// Base dos pedidos: a rota decidida ou, sem decisão, o endereço salvo.
    pub(crate) fn route(&self) -> Url {
        with(|t| t.get(self.base.as_str()).and_then(|known| known.route.clone())).unwrap_or_else(|| self.base.clone())
    }

    /// Credencial que vai na URL de um socket para `url`, pela mesma escolha do cliente HTTP (`Authed`). `None`: endereço
    /// do Connect sem credencial de aparelho, onde a senha do dono nunca vai.
    pub(crate) fn socket_token(&self, url: &Url, owner: &str) -> Option<String> {
        match credential(self.base.as_str(), url) {
            Credential::Owner => Some(owner.trim().to_owned()),
            Credential::Device(_, token) => Some(token),
            Credential::Blocked => None,
        }
    }
}

/// Só para os testes de quem monta URL pela rota (sockets).
#[cfg(test)]
pub(crate) fn route_to(api: &Api, route: Url) { set_route(&api.identity(), route); }

fn set_route(key: &str, route: Url) { with(|t| t.entry(key.to_owned()).or_default().route = Some(route)); }

/// Decide a rota antes de conectar. Nunca falha: sem resposta, fica o endereço salvo.
pub async fn ensure(api: &Api) {
    let key = api.identity();
    let Some((skip, gate)) = with(|t| {
        let known = t.entry(key.clone()).or_default();
        known.route.is_none().then(|| (known.invite || api.is_loopback(), known.deciding.clone()))
    }) else { return };
    // Convite e esta própria máquina: o endereço salvo já é o caminho.
    if skip { return set_route(&key, api.base.clone()); }
    // Lista e conversas da mesma máquina reconectam juntas: uma decide, as outras aproveitam.
    let _turn = gate.lock().await;
    let Some((saved, connect)) = with(|t| t.get(&key).filter(|k| k.route.is_none())
        .map(|k| (k.lan.clone(), k.connect.as_ref().map(|(c, _)| c.clone())))) else { return };
    let route = decide(api, saved, connect).await;
    set_route(&key, route);
    // Máquina com Connect ganha a credencial de aparelho; endereço novo do Connect fica guardado.
    tokio::spawn(refresh_connect(api.clone()));
}

/// Rede local se provar antes de o salvo responder; senão o salvo; o Connect só se nenhum dos dois responder.
async fn decide(api: &Api, saved: Option<Lan>, connect: Option<ConnectRoute>) -> Url {
    let connect = connect.and_then(|c| lan_url(&Lan { url: c.url.clone(), id: c.id.clone() }).filter(|url| *url != api.base).map(|url| (url, c)));
    // Prova em paralelo para não somar ao prazo, mas só vale depois de os outros falharem.
    let connect_proof = connect.as_ref().map(|(url, c)| {
        let (api, url, c) = (api.clone(), url.clone(), c.clone());
        tokio::spawn(async move { same_machine(&api, &url, &c.id, Some(&c), SAVED_TIMEOUT).await })
    });
    // O local ganha só se provar a identidade ANTES de o salvo responder: o mesmo IP alcançado por
    // VPN de outra rede responde, mas mais devagar que o Tailscale.
    let saved_answered = Arc::new(AtomicBool::new(false));
    let (lan, saved_took, saved_task) = match saved {
        // Primeira vez: pergunta pelo salvo antes de conectar, pra já nascer na rota certa.
        None => {
            let start = Instant::now();
            let lan = learn(api).await;
            let took = lan.as_ref().map(|_| start.elapsed());
            (lan, took, None)
        }
        // A mesma pergunta mede o salvo e atualiza o IP local (DHCP, bind aberto depois).
        Some(lan) => {
            let (api, answered) = (api.clone(), saved_answered.clone());
            let task = tokio::spawn(async move {
                let ok = learn(&api).await.is_some();
                if ok { answered.store(true, Ordering::SeqCst); }
                ok
            });
            (Some(lan), None, Some(task))
        }
    };
    let lan_start = Instant::now();
    let proven = match lan.as_ref().and_then(|lan| lan_url(lan).map(|route| (route, lan.id.clone()))) {
        Some((route, id)) if same_machine(api, &route, &id, None, LAN_TIMEOUT).await => Some(route),
        _ => None,
    };
    let lan_took = lan_start.elapsed();
    let wins = proven.filter(|_| match saved_took {
        Some(took) => lan_took <= took,
        None => !saved_answered.load(Ordering::SeqCst),
    });
    if let Some(route) = wins { return route; }
    let (Some((url, _)), Some(proof)) = (connect, connect_proof) else { return api.base.clone() };
    let saved_ok = match saved_task { Some(task) => task.await.unwrap_or(false), None => saved_took.is_some() };
    if !saved_ok && proof.await.unwrap_or(false) { url } else { api.base.clone() }
}

fn lan_url(lan: &Lan) -> Option<Url> {
    if lan.url.is_empty() || lan.id.is_empty() { return None; }
    key(&lan.url).and_then(|url| Url::parse(&url).ok()).filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

async fn ask(api: &Api, mut url: Url, timeout: Duration) -> Option<Value> {
    url.path_segments_mut().ok()?.pop_if_empty().extend(["api", "peers", "identificador"]);
    let r = api.client.get(url).timeout(timeout).send().await.ok()?;
    if !r.status().is_success() { return None; }
    r.json().await.ok()
}

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

// Mesmo IP em outra rede pode ser outra máquina: o token só vai pro endereço depois de ele
// provar que conhece o token (HMAC do desafio) e dizer o mesmo nome. Por isso o cliente sem token.
// Com `device`, a prova é a do aparelho: a máquina só guarda o hash do token dele.
async fn same_machine(api: &Api, route: &Url, id: &str, device: Option<&ConnectRoute>, timeout: Duration) -> bool {
    let mut raw = [0u8; 16];
    if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut raw).is_err() { return false; }
    let challenge = hex(&raw);
    let mut url = route.clone();
    let Ok(mut segments) = url.path_segments_mut() else { return false };
    segments.pop_if_empty().extend(["api", "peers", "prova"]);
    drop(segments);
    url.query_pairs_mut().append_pair("desafio", &challenge);
    if let Some(device) = device { url.query_pairs_mut().append_pair("device", &device.device_id); }
    let Ok(r) = api.plain.get(url).timeout(timeout).send().await else { return false };
    let Ok(v) = r.error_for_status().map(|r| r.json::<Value>()) else { return false };
    let Ok(v) = v.await else { return false };
    let secret = match device {
        Some(device) => hex(ring::digest::digest(&ring::digest::SHA256, device.token.as_bytes()).as_ref()),
        None => api.token.clone(),
    };
    v.get("identificador").and_then(Value::as_str) == Some(id)
        && v.get("prova").and_then(Value::as_str) == Some(proof(&secret, &challenge, id, device.map(|_| route)).as_str())
}

/// HMAC do desafio. `bound`: o endereço chamado, que só a prova de aparelho assina.
fn proof(secret: &str, challenge: &str, id: &str, bound: Option<&Url>) -> String {
    let signed = match bound {
        // A prova de aparelho é pública: sem o host, uma rede hostil a repassaria do endereço do Connect.
        Some(url) => {
            let host = url.host_str().unwrap_or_default();
            match url.port() { Some(port) => format!("{challenge}|{id}|{host}:{port}"), None => format!("{challenge}|{id}|{host}") }
        }
        None => format!("{challenge}|{id}"),
    };
    hex(ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes()), signed.as_bytes()).as_ref())
}

async fn learn(api: &Api) -> Option<Lan> {
    let answer = ask(api, api.base.clone(), SAVED_TIMEOUT).await?;
    let id = answer.get("identificador").and_then(Value::as_str).filter(|id| !id.is_empty())?.to_owned();
    // Backend antigo não manda `lan_url`: fica como sem acesso local.
    let url = answer.get("lan_url").and_then(Value::as_str).unwrap_or_default().trim_end_matches('/').to_owned();
    let lan = Lan { url, id };
    let key = api.identity();
    let changed = with(|t| {
        let known = t.entry(key.clone()).or_default();
        let changed = known.lan.as_ref() != Some(&lan);
        known.lan = Some(lan.clone());
        changed
    });
    if changed { notify(key, Learned::Lan(lan.clone())); }
    Some(lan)
}

/// Endereço que a máquina anuncia para o Connect (`/api/alcance`); `None` sem Connect, em máquina antiga ou fora do ar.
async fn announced(api: &Api) -> Option<String> {
    let mut url = api.route();
    url.path_segments_mut().ok()?.pop_if_empty().extend(["api", "alcance"]);
    let r = api.client.get(url).timeout(SAVED_TIMEOUT).send().await.ok()?;
    if !r.status().is_success() { return None; }
    connect_announced(r.json::<Value>().await.ok()?.get("connect")?.as_str()?)
}

fn connect_announced(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https" && url.host_str().is_some() && url.username().is_empty() && url.password().is_none()
        && url.query().is_none() && url.fragment().is_none()).then(|| url.as_str().trim_end_matches('/').to_owned())
}

/// Depois de decidir: máquina que anuncia o Connect e ainda não deu credencial a este app emite uma; a que já deu e
/// mudou de endereço tem o novo guardado (o `decide` prova antes de usar).
async fn refresh_connect(api: Api) {
    let identity = api.identity();
    let Some((id, current)) = with(|t| t.get(&identity).and_then(|k| {
        let id = k.lan.as_ref().map(|l| l.id.clone()).filter(|id| !id.is_empty())?;
        let current = k.connect.as_ref().map(|(c, _)| c.clone());
        (current.is_some() || !(k.refused || k.enrolling)).then_some((id, current))
    })) else { return };
    let Some(url) = announced(&api).await else { return };
    // Endereço salvo que já é o do Connect: não há outro caminho para guardar.
    if key(&url).as_deref() == Some(identity.as_str()) { return; }
    match current {
        Some(c) if c.url == url && c.id == id => {}
        Some(c) => store(&identity, ConnectRoute { url, id, ..c }),
        None => enroll(&api, &identity, url, id).await,
    }
}

fn store(identity: &str, connect: ConnectRoute) {
    let Ok(client) = super::bearer_client(&connect.token) else {
        crate::log_line(&format!("connect: credencial de aparelho de {identity} com formato inválido, não guardada"));
        return;
    };
    let stored = with(|t| {
        let known = t.entry(identity.to_owned()).or_default();
        known.enrolling = false;
        // Pedido que terminou depois do 401 não desfaz a recusa.
        if known.refused { return false; }
        set_connect(known, Some((connect.clone(), client)));
        true
    });
    if stored { notify(identity.to_owned(), Learned::Connect(connect)); }
    else { crate::log_line(&format!("connect: credencial de aparelho de {identity} chegou depois da recusa e foi descartada")); }
}

/// `POST /api/connect/devices/self` com a senha do dono, como o `upgradeToDevice` do Expo. Recusa não é tentada de novo
/// nesta execução; queda de rede é, na próxima decisão.
async fn enroll(api: &Api, identity: &str, url: String, id: String) {
    let go = with(|t| {
        let known = t.entry(identity.to_owned()).or_default();
        let go = !(known.enrolling || known.refused || known.connect.is_some());
        if go { known.enrolling = true; }
        go
    });
    if !go { return; }
    let failed = |retry: bool, why: &str| {
        if retry { with(|t| if let Some(known) = t.get_mut(identity) { known.enrolling = false; }); }
        crate::log_line(&format!("connect: credencial de aparelho não emitida por {identity}: {why}"));
    };
    let mut target = api.route();
    if let Ok(mut parts) = target.path_segments_mut() { parts.pop_if_empty().extend(["api", "connect", "devices", "self"]); }
    let name = DEVICE_NAME.get().cloned().unwrap_or_else(|| "Hangar desktop".into());
    let response = match api.client.post(target).json(&json!({"device_name": name})).timeout(ENROLL_TIMEOUT).send().await {
        Ok(response) => response,
        Err(_) => return failed(true, "rede"),
    };
    let status = response.status().as_u16();
    let body = response.json::<Value>().await.ok();
    if !(200..300).contains(&status) {
        let code = super::envelope_code(body.as_ref()).unwrap_or_default();
        return failed(false, &format!("HTTP {status} {code}"));
    }
    let field = |name: &str| body.as_ref().and_then(|b| b.get(name)).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned);
    let (Some(device_id), Some(token)) = (field("device_id"), field("token")) else { return failed(false, "resposta sem device_id ou token") };
    crate::log_line(&format!("connect: credencial de aparelho {device_id} emitida por {identity}"));
    store(identity, ConnectRoute { url, id, device_id, token });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    struct Req { method: String, target: String, headers: HashMap<String, String>, body: String }

    impl Req {
        fn query(&self, name: &str) -> Option<String> {
            Url::parse(&format!("http://h{}", self.target)).ok()?.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
        }
        fn path(&self) -> &str { self.target.split('?').next().unwrap_or_default() }
        fn bearer(&self) -> Option<&str> { self.headers.get("authorization").and_then(|v| v.strip_prefix("Bearer ")) }
    }

    /// Servidor HTTP mínimo: `handler` devolve status, corpo e atraso em ms.
    async fn serve(handler: impl Fn(&Req) -> (u16, Value, u64) + Send + Sync + 'static) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut stream = BufReader::new(stream);
                    let mut line = String::new();
                    if stream.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                    let mut parts = line.split_whitespace();
                    let (method, target) = (parts.next().unwrap_or_default().to_owned(), parts.next().unwrap_or_default().to_owned());
                    let mut headers = HashMap::new();
                    loop {
                        let mut header = String::new();
                        if stream.read_line(&mut header).await.unwrap_or(0) == 0 || header == "\r\n" { break; }
                        if let Some((k, v)) = header.split_once(':') { headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned()); }
                    }
                    let length = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                    let mut body = vec![0; length];
                    if stream.read_exact(&mut body).await.is_err() { return; }
                    let req = Req { method, target, headers, body: String::from_utf8_lossy(&body).into_owned() };
                    let (status, reply, delay) = (*handler)(&req);
                    if delay > 0 { tokio::time::sleep(Duration::from_millis(delay)).await; }
                    let reply = reply.to_string();
                    let _ = stream.get_mut().write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes()).await;
                });
            }
        });
        (address, task)
    }

    /// Porta sem ninguém ouvindo: conexão recusada na hora.
    async fn closed() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        address
    }

    fn sign(secret: &str, signed: &str) -> String {
        hex(ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes()), signed.as_bytes()).as_ref())
    }

    /// A prova como o servidor monta (`machine_peers`): com `device`, chave = sha256 do token e o `Host` recebido.
    fn prova(req: &Req, id: &str, owner: &str, device: Option<(&str, &str)>) -> (u16, Value, u64) {
        let challenge = req.query("desafio").unwrap_or_default();
        let signed = match (device, req.query("device")) {
            (Some((device_id, token)), Some(asked)) if asked == device_id => {
                let host = req.headers.get("host").cloned().unwrap_or_default().to_ascii_lowercase();
                sign(&hex(ring::digest::digest(&ring::digest::SHA256, token.as_bytes()).as_ref()), &format!("{challenge}|{id}|{host}"))
            }
            (_, Some(_)) => return (404, json!({"detail": "Not Found"}), 0),
            _ => sign(owner, &format!("{challenge}|{id}")),
        };
        (200, json!({"identificador": id, "prova": signed}), 0)
    }

    fn connect_route(url: &str) -> ConnectRoute {
        ConnectRoute { url: url.into(), id: "casa".into(), device_id: "dev-1".into(), token: "device-secret".into() }
    }

    fn route_of(url: &str) -> Url { Url::parse(&key(url).unwrap()).unwrap() }

    #[tokio::test]
    async fn connect_wins_only_when_lan_and_saved_fail() {
        let (connect, server) = serve(|req| match req.path() {
            "/api/peers/prova" => prova(req, "casa", "owner", Some(("dev-1", "device-secret"))),
            _ => (404, json!({}), 0),
        }).await;
        let api = Api::new(&closed().await, "owner").unwrap();
        let lan = Lan { url: closed().await, id: "casa".into() };
        let chosen = decide(&api, Some(lan), Some(connect_route(&connect))).await;
        server.abort();
        assert_eq!(chosen, route_of(&connect));
    }

    #[tokio::test]
    async fn saved_that_answers_beats_a_proven_connect() {
        let (saved, saved_server) = serve(|req| match req.path() {
            "/api/peers/identificador" => (200, json!({"identificador": "casa", "lan_url": ""}), 0),
            _ => (404, json!({}), 0),
        }).await;
        let (connect, connect_server) = serve(|req| prova(req, "casa", "owner", Some(("dev-1", "device-secret")))).await;
        let api = Api::new(&saved, "owner").unwrap();
        let lan = Lan { url: closed().await, id: "casa".into() };
        let chosen = decide(&api, Some(lan), Some(connect_route(&connect))).await;
        saved_server.abort();
        connect_server.abort();
        assert_eq!(chosen, api.base);
    }

    #[tokio::test]
    async fn lan_that_proves_first_beats_saved_and_connect() {
        let (saved, saved_server) = serve(|req| match req.path() {
            "/api/peers/identificador" => (200, json!({"identificador": "casa", "lan_url": ""}), 300),
            _ => (404, json!({}), 0),
        }).await;
        let (lan, lan_server) = serve(|req| prova(req, "casa", "owner", None)).await;
        let (connect, connect_server) = serve(|req| prova(req, "casa", "owner", Some(("dev-1", "device-secret")))).await;
        let api = Api::new(&saved, "owner").unwrap();
        let chosen = decide(&api, Some(Lan { url: lan.clone(), id: "casa".into() }), Some(connect_route(&connect))).await;
        for task in [saved_server, lan_server, connect_server] { task.abort(); }
        assert_eq!(chosen, route_of(&lan));
    }

    #[tokio::test]
    async fn connect_with_wrong_proof_is_never_chosen() {
        let (connect, server) = serve(|_| (200, json!({"identificador": "casa", "prova": "00"}), 0)).await;
        let api = Api::new(&closed().await, "owner").unwrap();
        let chosen = decide(&api, None, Some(connect_route(&connect))).await;
        server.abort();
        assert_eq!(chosen, api.base);
    }

    #[tokio::test]
    async fn connect_route_carries_device_credential_and_its_401_drops_it() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let (connect, server) = serve(move |req| {
            log.lock().unwrap().push(req.bearer().unwrap_or_default().to_owned());
            (401, json!({"detail": {"code": "erro_nao_autorizado", "msg": "unauthorized"}}), 0)
        }).await;
        let api = Api::new(&closed().await, "owner").unwrap();
        let identity = api.identity();
        seed(&identity, false, None, Some(&connect_route(&connect)), false);
        set_route(&identity, route_of(&connect));
        let error = api.sessions().await.unwrap_err();
        server.abort();
        assert_eq!(error.status, Some(401));
        assert_eq!(*seen.lock().unwrap(), vec!["device-secret".to_owned()], "a senha do dono nunca vai ao Connect");
        let (connect_left, refused, route) = with(|t| t.get(&identity).map(|k| (k.connect.is_some(), k.refused, k.route.clone()))).unwrap();
        assert!(!connect_left && refused && route.is_none());
        // O arquivo ainda sem a recusa não a desfaz.
        seed(&identity, false, None, Some(&connect_route(&connect)), false);
        assert!(with(|t| t.get(&identity).is_some_and(|k| k.connect.is_none() && k.refused)));
    }

    #[tokio::test]
    async fn connect_own_routes_401_keeps_the_credential() {
        let connect = closed().await;
        let api = Api::new(&closed().await, "owner").unwrap();
        let identity = api.identity();
        seed(&identity, false, None, Some(&connect_route(&connect)), false);
        refused_at(&Url::parse(&format!("{connect}/api/connect/state")).unwrap());
        assert!(with(|t| t.get(&identity).is_some_and(|k| k.connect.is_some() && !k.refused)));
    }

    #[tokio::test]
    async fn other_routes_keep_the_owner_secret() {
        let api = Api::new(&closed().await, "owner").unwrap();
        let identity = api.identity();
        seed(&identity, false, None, Some(&connect_route(&closed().await)), false);
        assert!(matches!(credential(&identity, &api.base), Credential::Owner));
        assert_eq!(api.socket_token(&api.base, " owner ").as_deref(), Some("owner"));
    }

    #[tokio::test]
    async fn owner_secret_never_reaches_a_refused_connect_origin() {
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        let (connect, server) = serve(move |_| { counted.fetch_add(1, Ordering::SeqCst); (200, json!([]), 0) }).await;
        let api = Api::new(&closed().await, "owner").unwrap();
        let identity = api.identity();
        seed(&identity, false, None, Some(&connect_route(&connect)), false);
        // URL calculada antes da recusa, como a de um SSE ou socket já aberto.
        let late = Url::parse(&format!("{connect}/api/sessions")).unwrap();
        refused_at(&late);
        assert!(matches!(credential(&identity, &late), Credential::Blocked));
        assert_eq!(api.socket_token(&late, "owner"), None);
        assert!(api.client.get(late.clone()).send().await.is_err());
        assert!(api.client.request(reqwest::Method::POST, late).send().await.is_err());
        server.abort();
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nada sai para o Connect sem credencial de aparelho");
    }

    #[tokio::test]
    async fn enroll_that_finishes_after_the_refusal_is_discarded() {
        let (saved, server) = serve(|req| match (req.method.as_str(), req.path()) {
            ("GET", "/api/alcance") => (200, json!({"connect": "https://late.jeff.hangar.dev.br"}), 0),
            ("POST", "/api/connect/devices/self") => (200, json!({"device_id": "dev-late", "token": "late"}), 300),
            _ => (404, json!({}), 0),
        }).await;
        let api = Api::new(&saved, "owner").unwrap();
        let identity = api.identity();
        seed(&identity, false, Some(&Lan { url: String::new(), id: "casa".into() }), None, false);
        let pending = tokio::spawn(refresh_connect(api.clone()));
        tokio::time::sleep(Duration::from_millis(100)).await;
        // O 401 chega enquanto a emissão ainda espera a resposta.
        seed(&identity, false, None, None, true);
        pending.await.unwrap();
        server.abort();
        assert!(with(|t| t.get(&identity).is_some_and(|k| k.connect.is_none() && k.refused)));
    }

    #[tokio::test]
    async fn owner_edit_lifts_the_refusal() {
        let api = Api::new(&closed().await, "owner").unwrap();
        let identity = api.identity();
        seed(&identity, false, None, None, true);
        reset_refused(&identity);
        assert!(with(|t| t.get(&identity).is_some_and(|k| !k.refused && !k.enrolling)));
    }

    #[tokio::test]
    async fn machine_with_connect_issues_a_device_credential_with_the_owner_secret() {
        let posts = Arc::new(AtomicUsize::new(0));
        let counted = posts.clone();
        let (saved, server) = serve(move |req| match (req.method.as_str(), req.path()) {
            ("GET", "/api/alcance") => (200, json!({"connect": "https://casa.jeff.hangar.dev.br/"}), 0),
            ("POST", "/api/connect/devices/self") => {
                counted.fetch_add(1, Ordering::SeqCst);
                let named = serde_json::from_str::<Value>(&req.body).ok()
                    .and_then(|b| b.get("device_name").and_then(Value::as_str).map(|n| !n.is_empty())).unwrap_or(false);
                if req.bearer() != Some("owner") || !named { return (400, json!({}), 0); }
                (200, json!({"device_id": "dev-9", "token": "issued"}), 0)
            }
            _ => (404, json!({}), 0),
        }).await;
        let api = Api::new(&saved, "owner").unwrap();
        let identity = api.identity();
        seed(&identity, false, Some(&Lan { url: String::new(), id: "casa".into() }), None, false);
        refresh_connect(api.clone()).await;
        // Já tem credencial: a próxima decisão não pede outra.
        refresh_connect(api.clone()).await;
        server.abort();
        assert_eq!(posts.load(Ordering::SeqCst), 1);
        let stored = with(|t| t.get(&identity).and_then(|k| k.connect.as_ref().map(|(c, _)| c.clone())));
        assert_eq!(stored, Some(ConnectRoute { url: "https://casa.jeff.hangar.dev.br".into(), id: "casa".into(), device_id: "dev-9".into(), token: "issued".into() }));
    }

    #[tokio::test]
    async fn revoked_machine_is_not_enrolled_again() {
        let posts = Arc::new(AtomicUsize::new(0));
        let counted = posts.clone();
        let (saved, server) = serve(move |req| match req.path() {
            "/api/alcance" => (200, json!({"connect": "https://casa.jeff.hangar.dev.br"}), 0),
            _ => { counted.fetch_add(1, Ordering::SeqCst); (200, json!({"device_id": "x", "token": "y"}), 0) }
        }).await;
        let api = Api::new(&saved, "owner").unwrap();
        let identity = api.identity();
        seed(&identity, false, Some(&Lan { url: String::new(), id: "casa".into() }), None, true);
        refresh_connect(api).await;
        server.abort();
        assert_eq!(posts.load(Ordering::SeqCst), 0);
        assert!(with(|t| t.get(&identity).is_some_and(|k| k.connect.is_none())));
    }

    #[test]
    fn only_clean_https_is_a_connect_address() {
        assert_eq!(connect_announced("https://casa.jeff.hangar.dev.br/").as_deref(), Some("https://casa.jeff.hangar.dev.br"));
        for raw in ["http://casa.example", "https://u:p@casa.example", "https://casa.example/?x=1", "https://casa.example/#a", "nada"] {
            assert_eq!(connect_announced(raw), None, "{raw}");
        }
    }
}
