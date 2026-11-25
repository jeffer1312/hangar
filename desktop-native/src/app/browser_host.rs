//! Canal anfitrião (`GET /api/browser-host`): o agente de uma sessão de outra máquina dirige o navegador da sessão
//! neste app. Um canal por servidor remoto, aberto na primeira vez que uma sessão dele aparece na tela; os verbos
//! passam pelo mesmo executor do `/cmd` local (`run_verb`). Só servidor do dono tem canal: convite e par externo nunca.
use std::{io, sync::{Arc, Mutex}, time::{Duration, Instant}};
#[cfg(not(target_os = "linux"))]
use std::net::IpAddr;

use tokio::{io::{AsyncWrite, AsyncWriteExt}, time::timeout};

use super::*;
use super::ports_panel::owner_server;
use crate::browser::control::{Reply, Secret};
use crate::{api::route::blocked_url, ws};

const PATH: &str = "/api/browser-host";
/// O servidor pinga a cada 25 s: sem quadro nenhum nesse tempo, o canal morreu no caminho.
const IDLE: Duration = Duration::from_secs(60);
/// Os pedidos do servidor são o corpo do `/cmd`, que ele limita a 1 MiB.
const MAX_IN: usize = 1024 * 1024;
/// Um print de 32 MiB numa rota lenta.
const SEND_WITHIN: Duration = Duration::from_secs(60);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// O mesmo prazo do CLI quando o pedido não diz o dele.
const DEFAULT_DEADLINE: Duration = Duration::from_secs(45);
const READY_POLL: Duration = Duration::from_millis(150);
const SEEN_EVERY: Duration = Duration::from_secs(10);
#[cfg(not(target_os = "linux"))]
const LOCAL_REFUSED: &str = "endereço local deste computador recusado";

/// O que o app manda ao servidor pelo canal.
enum Out { Text(String), Png(u64, Vec<u8>) }

/// O que a tarefa do canal lê na hora de (re)conectar: conexão, credencial e a sessão em foco. `route` é a rota da
/// criação do canal, só para o quadro perceber que ela mudou.
struct Conn { api: Api, route: String, token: String, focus: Option<String> }

struct Link {
    out: async_channel::Sender<Out>,
    conn: Arc<Mutex<Conn>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Link { fn drop(&mut self) { self.task.abort(); } }

#[derive(Default)]
pub(super) struct Hosts {
    /// Pela chave normalizada do servidor.
    links: HashMap<String, Link>,
    /// Pedidos dos canais para a janela; a ponta de leitura é drenada por uma tarefa da janela.
    incoming: Option<async_channel::Sender<(String, Value)>>,
    /// Última sessão (`servidor::sessão`) dada como usada aqui, e quando: o aviso se repete no máximo a cada `SEEN_EVERY`.
    seen: Option<(String, Instant)>,
    /// Sessão à vista na janela ativa no quadro anterior.
    pub(super) shown: Option<String>,
}

/// O que o quadro faz com o canal que já existe; `[identidade, rota, token]` de antes e de agora.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LinkChange { Keep, Focus, Reconnect }

pub(super) fn link_change(have: [&str; 3], have_focus: Option<&str>, want: [&str; 3], want_focus: &str) -> LinkChange {
    if have != want { LinkChange::Reconnect } else if have_focus != Some(want_focus) { LinkChange::Focus } else { LinkChange::Keep }
}

fn lock(conn: &Mutex<Conn>) -> std::sync::MutexGuard<'_, Conn> { conn.lock().unwrap_or_else(|e| e.into_inner()) }

/// `caps`: o que este app sabe fazer além dos verbos de sempre. Sem `fill_secret`, o servidor não manda o valor do cofre.
fn focus_text(session: &str) -> String { json!({"type": "focus", "session": session, "caps": ["fill_secret"]}).to_string() }

impl Hosts {
    fn out(&self, server: &str) -> Option<async_channel::Sender<Out>> { self.links.get(server).map(|l| l.out.clone()) }
}

// --- regras puras: endereço pedido pelo agente remoto e as portas nas respostas ---

/// O endereço de `open`/`goto` como o `goto` do controlador o completa (sem esquema = `http://`); `None` é
/// `about:blank`. Só web: `file:`, `chrome:`, `javascript:`... leriam este computador.
pub(super) fn web_url(raw: &str) -> Result<Option<url::Url>, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() { return Err("goto precisa de uma url"); }
    if raw == "about:blank" { return Ok(None); }
    let full = if raw.contains("://") { raw.to_owned() } else { format!("http://{raw}") };
    let url = url::Url::parse(&full).map_err(|_| "url invalida")?;
    if !matches!(url.scheme(), "http" | "https") { return Err("só endereços http e https"); }
    Ok(Some(url))
}

/// Para onde a navegação do agente remoto pode ir. No Linux quem decide é o SOCKS do agente, a cada pedido.
#[cfg(not(target_os = "linux"))]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// Endereço de fora; `lookup` = nome a conferir no DNS antes de abrir.
    Public { url: String, lookup: Option<(String, u16)> },
    /// `localhost:<port>` da máquina da sessão: vai pelo túnel, e a porta do `url` vira a local.
    Tunnel { url: url::Url, port: u16 },
}

/// Rede deste computador que o agente de outra máquina não alcança: privada, de enlace, CGNAT (a do Tailscale) e
/// loopback (que só vale pelo túnel, decidido antes).
#[cfg(not(target_os = "linux"))]
pub(super) fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast()
                || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() { return private_ip(IpAddr::V4(v4)); }
            let first = v6.segments()[0];
            v6.is_loopback() || v6.is_unspecified() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn target(raw: &str) -> Result<Target, &'static str> {
    let Some(url) = web_url(raw)? else { return Ok(Target::Public { url: "about:blank".into(), lookup: None }) };
    let port = url.port_or_known_default().unwrap_or(80);
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            if d == "localhost" || d.ends_with(".localhost") { true }
            else if d.ends_with(".local") { return Err(LOCAL_REFUSED) }
            else { return Ok(Target::Public { url: url.to_string(), lookup: Some((d, port)) }) }
        }
        Some(url::Host::Ipv4(ip)) if ip.is_loopback() => true,
        Some(url::Host::Ipv6(ip)) if ip.is_loopback() => true,
        Some(url::Host::Ipv4(ip)) if private_ip(IpAddr::V4(ip)) => return Err(LOCAL_REFUSED),
        Some(url::Host::Ipv6(ip)) if private_ip(IpAddr::V6(ip)) => return Err(LOCAL_REFUSED),
        Some(_) => false,
        None => return Err("url invalida"),
    };
    Ok(if loopback { Target::Tunnel { url, port } } else { Target::Public { url: url.to_string(), lookup: None } })
}

/// A URL do túnel com a porta local deste computador.
#[cfg(not(target_os = "linux"))]
pub(super) fn with_local_port(mut url: url::Url, local: u16) -> String {
    let _ = url.set_port(Some(local));
    url.to_string()
}

/// A porta local volta a ser a remota em tudo que o agente lê (`(local, remota)`), numa passada só: a troca de uma
/// porta nunca é trocada de novo por outro par.
pub(super) fn to_remote(text: &str, pairs: &[(u16, u16)]) -> String {
    const HOSTS: [&str; 3] = ["localhost:", "127.0.0.1:", "[::1]:"];
    if pairs.iter().all(|(l, r)| l == r) { return text.to_owned(); }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    'scan: while i < text.len() {
        for host in HOSTS {
            if !text[i..].starts_with(host) { continue; }
            let start = i + host.len();
            let end = start + text[start..].bytes().take_while(u8::is_ascii_digit).count();
            if let Some((_, remote)) = text[start..end].parse::<u16>().ok().and_then(|p| pairs.iter().find(|(l, _)| *l == p)) {
                out.push_str(host);
                out.push_str(&remote.to_string());
                i = end;
                continue 'scan;
            }
        }
        let Some(ch) = text[i..].chars().next() else { break };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

// --- o canal ---

async fn send(writer: &mut (impl AsyncWrite + Unpin), opcode: u8, data: &[u8]) -> io::Result<()> {
    let frame = ws::client_frame(opcode, data).map_err(|_| io::Error::other("rng"))?;
    timeout(SEND_WITHIN, async { writer.write_all(&frame).await?; writer.flush().await }).await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

/// Reconecta para sempre, com espera crescente, pela rota em uso a cada tentativa; só a aba fechando a encerra.
async fn run(server: String, conn: Arc<Mutex<Conn>>, out: async_channel::Receiver<Out>, incoming: async_channel::Sender<(String, Value)>) {
    let mut wait = Duration::from_secs(1);
    let mut failing = false;
    loop {
        let (api, token) = { let c = lock(&conn); (c.api.clone(), c.token.clone()) };
        // Connect sem credencial de aparelho: a senha do dono não sai, e a tentativa espera a rota mudar.
        let url = ws::text_url(&api, PATH, &token);
        let result = if url == blocked_url() { Err(ws::Error::Handshake) } else { ws::handshake(url, Some(api.identity())).await };
        match result {
            Ok(socket) => {
                wait = Duration::from_secs(1);
                failing = false;
                // O que ficou na fila era do canal anterior: o servidor não conhece mais aqueles pedidos.
                while out.try_recv().is_ok() {}
                let focus = lock(&conn).focus.clone();
                pump(socket, focus, &server, &out, &incoming).await;
                crate::log_line(&format!("nav: browser_host_dropped: canal anfitrião de {server} caiu; reconectando"));
            }
            Err(error) => if !std::mem::replace(&mut failing, true) {
                crate::log_line(&format!("nav: browser_host_connect_failed: canal anfitrião de {server} não abriu ({error:?}); tentando de novo"));
            },
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(BACKOFF_MAX);
    }
}

async fn pump(socket: reqwest::Upgraded, focus: Option<String>, server: &str, out: &async_channel::Receiver<Out>,
    incoming: &async_channel::Sender<(String, Value)>) {
    let (mut reader, mut writer) = tokio::io::split(socket);
    if let Some(session) = focus && send(&mut writer, 1, focus_text(&session).as_bytes()).await.is_err() { return; }
    let mut message = ws::Message::default();
    let mut kind = 0;
    loop {
        // A leitura fica de pé entre as escritas: cancelada no meio, o quadro seguinte sairia torto.
        let frame = {
            let next = timeout(IDLE, ws::read_frame_as(&mut reader, false, MAX_IN));
            tokio::pin!(next);
            loop {
                tokio::select! {
                    frame = &mut next => match frame { Ok(Ok(frame)) => break frame, _ => return },
                    item = out.recv() => {
                        let sent = match item {
                            Ok(Out::Text(text)) => send(&mut writer, 1, text.as_bytes()).await,
                            Ok(Out::Png(id, png)) => match send(&mut writer, 1, json!({"id": id, "bin": "png"}).to_string().as_bytes()).await {
                                Ok(()) => send(&mut writer, 2, &png).await,
                                error => error,
                            },
                            Err(_) => return,
                        };
                        if sent.is_err() { return; }
                    }
                }
            }
        };
        match frame.opcode {
            8 => { let _ = send(&mut writer, 8, &frame.data).await; return; }
            9 => if send(&mut writer, 10, &frame.data).await.is_err() { return },
            10 => {}
            opcode => {
                if opcode != 0 { kind = opcode; }
                match message.push(frame) {
                    Ok(Some(bytes)) if kind == 1 => {
                        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { continue };
                        if incoming.send((server.to_owned(), value)).await.is_err() { return; }
                    }
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
        }
    }
}

fn refusal(id: u64, error: &str) -> Out { Out::Text(json!({"id": id, "ok": false, "error": error}).to_string()) }

fn reply_out(id: u64, reply: Reply, pairs: &[(u16, u16)]) -> Out {
    match reply {
        Reply::Text(text) => Out::Text(json!({"id": id, "ok": true, "text": to_remote(&text, pairs)}).to_string()),
        Reply::Png(png) => Out::Png(id, png),
    }
}

// --- na janela ---

impl Hangar {
    /// A cada quadro: sessão de outra máquina na tela dá foco no canal dela, que nasce na primeira vez.
    pub(super) fn browser_host_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser_host_prune();
        let Some(api) = self.session_api().filter(|api| !api.is_loopback()) else { return };
        let (Some(server), Some(name)) = (self.session_server().map(|s| servers::norm(&s)), self.selected.as_ref().map(|s| s.name.clone()))
            else { return };
        // Convite e par externo não são do dono: o canal é só do dono do servidor.
        if !owner_server(&self.servers, &server) { return; }
        let (identity, route, token) = (api.identity(), api.route().to_string(), self.server_token(&server));
        if let Some(link) = self.browser_hosts.links.get(&server) {
            let change = {
                let c = lock(&link.conn);
                link_change([&c.api.identity(), &c.route, &c.token], c.focus.as_deref(), [&identity, &route, &token], &name)
            };
            match change {
                LinkChange::Keep => return,
                LinkChange::Focus => {
                    lock(&link.conn).focus = Some(name.clone());
                    let _ = link.out.try_send(Out::Text(focus_text(&name)));
                    return;
                }
                // Senha ou rota nova: o canal de pé seguiria com a antiga; derrubar a tarefa reconecta com a de agora.
                LinkChange::Reconnect => { self.browser_hosts.links.remove(&server); }
            }
        }
        let incoming = match &self.browser_hosts.incoming {
            Some(incoming) => incoming.clone(),
            None => {
                let (tx, rx) = async_channel::unbounded::<(String, Value)>();
                cx.spawn_in(window, async move |this, cx| {
                    while let Ok((server, request)) = rx.recv().await {
                        if this.update_in(cx, |this, window, cx| this.host_execute(server, request, window, cx)).is_err() { break; }
                    }
                }).detach();
                self.browser_hosts.incoming = Some(tx.clone());
                tx
            }
        };
        let conn = Arc::new(Mutex::new(Conn { api, route, token, focus: Some(name) }));
        let (out, out_rx) = async_channel::unbounded();
        let task = self.runtime.spawn(run(server.clone(), conn.clone(), out_rx, incoming));
        self.browser_hosts.links.insert(server, Link { out, conn, task });
    }

    /// Clique ou tecla na janela: a pessoa está usando a sessão aqui, e o navegador do agente vem para este app. Sessão
    /// desta máquina grava a hora em `_focus.json`; de outra, avisa o servidor dela pelo canal. No máximo a cada 10 s.
    pub(super) fn note_session_seen(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(name)) = (self.session_api(), self.selected.as_ref().map(|s| s.name.clone())) else { return };
        let Some(key) = self.side_key() else { return };
        if self.browser_hosts.seen.as_ref().is_some_and(|(k, at)| *k == key && at.elapsed() < SEEN_EVERY) { return; }
        self.browser_hosts.seen = Some((key, Instant::now()));
        if api.is_loopback() {
            cx.background_spawn(async move { crate::browser::server::record_focus(&name) }).detach();
        } else if let Some(link) = self.session_server().and_then(|s| self.browser_hosts.links.get(&servers::norm(&s))) {
            let _ = link.out.try_send(Out::Text(json!({"type": "seen", "session": name}).to_string()));
        }
    }

    /// A cada quadro, o canal de servidor que saiu do app ou virou convite fecha (o `Drop` do `Link` derruba o socket).
    fn browser_host_prune(&mut self) {
        let servers = &self.servers;
        self.browser_hosts.links.retain(|key, _| owner_server(servers, key));
    }

    /// Pares (porta local, porta remota) dos encaminhamentos da máquina `server`.
    fn host_pairs(&self, server: &str) -> Vec<(u16, u16)> {
        // No Linux a página do agente não passa pelo encaminhamento: um "3001 → 3000" que a pessoa abriu à mão não é
        // a porta que ele vê.
        if cfg!(target_os = "linux") { return Vec::new(); }
        let Some(api) = self.machine_api(server) else { return Vec::new() };
        self.ports.forwards.list(&api.identity()).into_iter().map(|f| (f.local, f.port)).collect()
    }

    fn host_execute(&mut self, server: String, mut request: Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = request["id"].as_u64() else { return };
        // O valor do cofre sai do pedido antes de tudo: nada adiante (args, log, resposta) o vê.
        let value = match request.as_object_mut().and_then(|o| o.remove("valor")) { Some(Value::String(v)) => Some(Secret::new(v)), _ => None };
        // Pedido em voo de um servidor que acabou de virar convite ou sair não passa.
        if !owner_server(&self.servers, &server) { return; }
        let Some(out) = self.browser_hosts.out(&server) else { return };
        let session = request["session"].as_str().unwrap_or_default().to_owned();
        if session.is_empty() { let _ = out.try_send(refusal(id, "pedido sem sessao")); return; }
        let verb = request["verbo"].as_str().unwrap_or_default().to_owned();
        let args: Vec<String> = request["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_owned)).collect();
        let key = format!("{server}::{session}");
        // Só o `fill-secret` leva valor, e só com o domínio aprovado: o controlador confere a página contra ele.
        let secret = match (verb.as_str(), value, request["expected_host"].as_str().filter(|h| !h.is_empty())) {
            ("fill-secret", Some(value), Some(host)) => Some(value.for_host(host.to_owned())),
            ("fill-secret", ..) => { let _ = out.try_send(refusal(id, "fill-secret sem o valor ou o dominio aprovado")); return; }
            _ => None,
        };
        if matches!(verb.as_str(), "open" | "goto") {
            let deadline = Instant::now() + request["deadline_ms"].as_u64().map_or(DEFAULT_DEADLINE, Duration::from_millis);
            return self.host_navigate(server, id, key, session, verb, args.first().cloned().unwrap_or_default(), deadline, window, cx);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        self.host_run_verb(key, verb, args, request["aba"].as_i64(), request["deadline_ms"].as_u64(), secret, tx, cx);
        let pairs = self.host_pairs(&server);
        cx.spawn(async move |_, _| {
            let reply = rx.await.unwrap_or_else(|_| Reply::Text("erro: o navegador fechou no meio do comando".into()));
            let _ = out.send(reply_out(id, reply, &pairs)).await;
        }).detach();
    }

    /// `open`/`goto` do agente remoto: só endereço público, ou `localhost:N` da máquina dele pelo túnel. No Linux o
    /// endereço fica o original, e o SOCKS do contexto do agente confere cada pedido.
    #[allow(clippy::too_many_arguments)]
    fn host_navigate(&mut self, server: String, id: u64, key: String, session: String, verb: String, raw: String, deadline: Instant,
        window: &mut Window, cx: &mut Context<Self>) {
        let Some(out) = self.browser_hosts.out(&server) else { return };
        #[cfg(target_os = "linux")]
        let url = match web_url(&raw) {
            Ok(url) => url.map_or_else(|| "about:blank".to_owned(), String::from),
            Err(why) => { let _ = out.try_send(refusal(id, why)); return; }
        };
        #[cfg(target_os = "linux")]
        let remote_url = url.clone();
        #[cfg(not(target_os = "linux"))]
        let target = match target(&raw) {
            Ok(target) => target,
            Err(why) => { let _ = out.try_send(refusal(id, why)); return; }
        };
        #[cfg(not(target_os = "linux"))]
        let (url, remote_url, lookup, tunnel) = match target {
            Target::Public { url, lookup } => (url.clone(), url, lookup, None),
            Target::Tunnel { url, port } => {
                let Some(api) = self.machine_api(&server) else { let _ = out.try_send(refusal(id, "sem conexão com a máquina da sessão")); return };
                let token = self.server_token(&server);
                // O túnel do agente aparece em Portas da sessão dele, com Parar.
                self.note_forward_origin(api.identity(), port, key.clone());
                let opening = self.ports.forwards.open(self.runtime.handle(), &api, &token, port);
                (String::new(), url.to_string(), None, Some((opening, url, port)))
            }
        };
        #[cfg(not(target_os = "linux"))]
        let runtime = self.runtime.handle().clone();
        cx.spawn_in(window, async move |this, cx| {
            // O encaminhamento abre no runtime: a sonda e a busca de porta não seguram a tela.
            #[cfg(not(target_os = "linux"))]
            let url = match tunnel {
                None => url,
                Some((opening, tunnel, port)) => match opening.wait().await {
                    Some(Ok(local)) => with_local_port(tunnel, local),
                    failed => {
                        let why = failed.and_then(Result::err).map_or_else(|| "o encaminhamento foi parado".to_owned(), |e| e.to_string());
                        let _ = out.send(refusal(id, &format!("a porta {port} não foi encaminhada a este computador: {why}"))).await;
                        return;
                    }
                },
            };
            // Nome que resolve para a rede deste computador vale como o endereço dela.
            #[cfg(not(target_os = "linux"))]
            if let Some((host, port)) = lookup {
                let resolved = runtime.spawn(async move { tokio::net::lookup_host((host.as_str(), port)).await.map(|a| a.collect::<Vec<_>>()) }).await;
                // Endereço que não se conferiu não abre: poderia ser a rede deste computador.
                let Ok(Ok(addrs)) = resolved else {
                    let _ = out.send(refusal(id, "não foi possível verificar o endereço")).await;
                    return;
                };
                if addrs.iter().any(|a| private_ip(a.ip())) {
                    let _ = out.send(refusal(id, LOCAL_REFUSED)).await;
                    return;
                }
            }
            let Ok(ready) = this.update_in(cx, |this, window, cx| this.host_open_panel(&key, &url, window, cx)) else { return };
            let run = |this: &WeakEntity<Hangar>, cx: &mut AsyncWindowContext, verb: &str, args: Vec<String>| {
                let (tx, rx) = futures::channel::oneshot::channel();
                let (key, verb) = (key.clone(), verb.to_owned());
                // O que sobra do prazo do `open`/`goto`: o verbo não segura a vez depois de o servidor desistir.
                let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u64;
                let sent = this.update(cx, |this, cx| this.host_run_verb(key, verb, args, None, Some(left), None, tx, cx)).is_ok();
                async move { if sent { rx.await.ok() } else { None } }
            };
            let first = if ready { run(&this, cx, "goto", vec![url.clone()]).await } else {
                // Motor nascendo com o endereço: espera os verbos responderem, como o `open` do CLI espera o sidecar.
                loop {
                    if Instant::now() > deadline { break Some(Reply::Text("erro: o navegador nao ficou pronto no prazo".into())); }
                    cx.background_executor().timer(READY_POLL).await;
                    let ready = this.update(cx, |this, cx| this.side.browsers.get(&key).is_some_and(|b| agent_ready(b.read(cx))));
                    match ready {
                        Ok(true) => break run(&this, cx, "wait", vec!["--idle".into()]).await,
                        Ok(false) => {}
                        Err(_) => return,
                    }
                }
            };
            let failed = matches!(&first, Some(Reply::Text(t)) if t.starts_with("erro:")) || first.is_none();
            let reply = if verb == "open" && !failed { run(&this, cx, "url", Vec::new()).await } else { first };
            let reply = reply.unwrap_or_else(|| Reply::Text("erro: o navegador fechou no meio do comando".into()));
            let ok = matches!(&reply, Reply::Text(t) if !t.starts_with("erro:"));
            let pairs = this.update(cx, |this, _| this.host_pairs(&server)).unwrap_or_default();
            let _ = out.send(reply_out(id, reply, &pairs)).await;
            if ok { let _ = out.send(Out::Text(json!({"type": "state", "session": session, "url": remote_url}).to_string())).await; }
        }).detach();
    }

    /// O navegador da sessão no app, à vista como o `nav` local o deixa. Devolve se os verbos já respondem; senão o
    /// motor nasce aqui com `url`.
    fn host_open_panel(&mut self, key: &str, url: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // A aba Navegador da sessão remota mostrava a tela do app de lá; agora a página é a deste app.
        if let Some(remote) = self.side.remote_browsers.remove(key) { remote.update(cx, |view, cx| view.set_shown(false, cx)); }
        let browser = self.browser_for(key.to_owned(), window, cx);
        #[cfg(target_os = "linux")]
        self.arm_agent(&browser, key, cx);
        // Página aberta pela pessoa não está pronta para o agente: o `go` dele troca o motor pelo do contexto dele.
        let ready = agent_ready(browser.read(cx));
        if !ready { browser.update(cx, |panel, cx| panel.go(url.to_owned(), super::browser::By::Agent, window, cx)); }
        if self.browser_key().as_deref() == Some(key) { self.show_browser_tab(window, cx); } else {
            self.side.browser_open.insert(key.to_owned());
            self.side.tabs.insert(key.to_owned(), crate::appearance::SideTab::Browser);
        }
        ready
    }

    /// Verbo do agente remoto. No Linux só na página aberta no contexto dele: a que a pessoa abriu no painel ele não
    /// lê nem dirige.
    #[allow(clippy::too_many_arguments)]
    fn host_run_verb(&mut self, key: String, verb: String, args: Vec<String>, tab: Option<i64>, deadline: Option<u64>,
        secret: Option<Secret>, reply: futures::channel::oneshot::Sender<Reply>, cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        if let Some(browser) = self.side.browsers.get(&key).cloned() {
            self.arm_agent(&browser, &key, cx);
            if !browser.read(cx).engine_in(super::browser::By::Agent) {
                let _ = reply.send(Reply::Text(format!("erro: o navegador da sessao {key} esta com uma pagina aberta pela pessoa; abra a sua com open <url>")));
                return;
            }
        }
        self.run_verb(key, verb, args, tab, secret, deadline, reply, cx);
    }

    /// O contexto do agente remoto no painel da sessão `key`: o SOCKS em modo agente do servidor dela, relido a cada
    /// pedido. Sem conexão (ou servidor desta máquina) fica o erro, e o motor dele não nasce: nunca o perfil do painel.
    #[cfg(target_os = "linux")]
    pub(super) fn arm_agent(&mut self, browser: &Entity<super::browser::BrowserPanel>, key: &str, cx: &mut Context<Self>) {
        let server = key.rsplit_once("::").map_or("", |(server, _)| server);
        let proxy = match self.machine_api(server).filter(|api| !api.is_loopback()) {
            Some(api) => { let token = self.server_token(server); self.ports.socks.ensure_agent(self.runtime.handle(), &api, &token) }
            None => Err("sem conexão com a máquina da sessão".to_owned()),
        };
        browser.update(cx, |panel, _| panel.agent_proxy = Some(proxy));
    }
}

/// Os verbos respondem, e (no Linux) no contexto do agente.
fn agent_ready(panel: &super::browser::BrowserPanel) -> bool {
    #[cfg(target_os = "linux")]
    if !panel.engine_in(super::browser::By::Agent) { return false; }
    panel.verbs_ready()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    /// O servidor só manda o `fill-secret` (com o valor do cofre) ao app que anuncia saber fazê-lo.
    #[test]
    fn the_focus_announces_fill_secret() {
        let focus: Value = serde_json::from_str(&focus_text("s")).unwrap();
        assert_eq!(focus, json!({"type": "focus", "session": "s", "caps": ["fill_secret"]}));
    }

    #[test]
    fn only_web_addresses_are_opened_and_a_bare_host_is_http() {
        assert_eq!(web_url("localhost:3000/app").unwrap().map(String::from).as_deref(), Some("http://localhost:3000/app"));
        assert_eq!(web_url("http://0.0.0.0:5173").unwrap().map(String::from).as_deref(), Some("http://0.0.0.0:5173/"));
        assert_eq!(web_url(" about:blank "), Ok(None));
        for raw in ["", "file:///etc/passwd", "javascript:alert(1)", "chrome://settings"] {
            assert!(web_url(raw).is_err(), "{raw}");
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn tunnel(raw: &str) -> (String, u16) {
        match target(raw) { Ok(Target::Tunnel { url, port }) => (url.to_string(), port), other => panic!("{raw}: {other:?}") }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn localhost_goes_through_the_tunnel_with_the_local_port() {
        assert_eq!(tunnel("localhost:3000/app?x=1"), ("http://localhost:3000/app?x=1".into(), 3000));
        assert_eq!(tunnel("http://127.0.0.1:5173").1, 5173);
        assert_eq!(tunnel("https://[::1]/").1, 443);
        assert_eq!(tunnel("http://api.localhost:8080").1, 8080);
        let Ok(Target::Tunnel { url, .. }) = target("localhost:3000/app") else { panic!() };
        assert_eq!(with_local_port(url, 3001), "http://localhost:3001/app");
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn the_network_of_this_computer_is_refused() {
        for raw in ["http://192.168.0.1", "10.0.0.5:80", "172.16.1.1", "172.31.255.1", "169.254.169.254", "100.64.0.3",
            "http://[fd7a:115c:a1e0::1]/", "http://[fe80::1]", "http://[::ffff:192.168.1.1]", "impressora.local", "0.0.0.0:3000"] {
            assert_eq!(target(raw), Err(LOCAL_REFUSED), "{raw}");
        }
        for raw in ["file:///etc/passwd", "javascript:alert(1)", "chrome://settings"] {
            assert!(target(raw).is_err(), "{raw}");
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn public_addresses_pass_and_names_are_checked_in_dns() {
        assert_eq!(target("https://example.com/x"), Ok(Target::Public { url: "https://example.com/x".into(), lookup: Some(("example.com".into(), 443)) }));
        assert_eq!(target("http://8.8.8.8"), Ok(Target::Public { url: "http://8.8.8.8/".into(), lookup: None }));
        assert_eq!(target("172.32.0.1"), Ok(Target::Public { url: "http://172.32.0.1/".into(), lookup: None }));
        assert_eq!(target("about:blank"), Ok(Target::Public { url: "about:blank".into(), lookup: None }));
    }

    #[test]
    fn replies_show_the_remote_port_and_never_swap_twice() {
        let pairs = [(3001, 3000), (3000, 2999)];
        assert_eq!(to_remote("ok: \"http://localhost:3001/a\" e 127.0.0.1:3000", &pairs), "ok: \"http://localhost:3000/a\" e 127.0.0.1:2999");
        assert_eq!(to_remote("localhost:30011 [::1]:3001 ação", &pairs), "localhost:30011 [::1]:3000 ação");
        assert_eq!(to_remote("localhost:3001", &[(3001, 3001)]), "localhost:3001");
    }

    #[test]
    fn only_the_owner_entry_keeps_the_channel() {
        let entry = |address: &str, invite: bool, ephemeral: bool| ServerEntry { address: address.into(), invite, ephemeral, ..Default::default() };
        let servers = [entry("https://casa.ts.net:8765", false, false), entry("https://convite.ts.net:8765", true, false),
            entry("https://par.ts.net:8765", false, true)];
        let key = |address: &str| servers::norm(address);
        assert!(owner_server(&servers, &key("https://casa.ts.net:8765")));
        assert!(!owner_server(&servers, &key("https://convite.ts.net:8765")));
        assert!(!owner_server(&servers, &key("https://par.ts.net:8765")));
        assert!(!owner_server(&servers, &key("https://removido.ts.net:8765")));
    }

    #[test]
    fn a_new_password_or_route_reconnects_and_a_new_session_only_refocuses() {
        let have = ["https://casa/", "http://192.168.0.2:8765/", "senha"];
        assert_eq!(link_change(have, Some("a"), have, "a"), LinkChange::Keep);
        assert_eq!(link_change(have, Some("a"), have, "b"), LinkChange::Focus);
        assert_eq!(link_change(have, Some("a"), ["https://casa/", "http://192.168.0.2:8765/", "outra"], "a"), LinkChange::Reconnect);
        assert_eq!(link_change(have, Some("a"), ["https://casa/", "https://casa/", "senha"], "b"), LinkChange::Reconnect);
        assert_eq!(link_change(have, Some("a"), ["https://outra/", "http://192.168.0.2:8765/", "senha"], "a"), LinkChange::Reconnect);
    }
}
