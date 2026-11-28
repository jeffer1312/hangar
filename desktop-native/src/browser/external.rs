//! Os verbos do hangar-preview numa página FORA do app (`hangar-preview --cdp http://127.0.0.1:9222`): um WebView2
//! de outro programa ou um Chrome com porta de depuração, alcançado por túnel. O controlador é o mesmo do navegador
//! da sessão; só a página muda, um WebSocket CDP no lugar do motor embutido.
use std::{cell::{Cell, RefCell}, collections::HashMap, future::Future, rc::{Rc, Weak}, sync::OnceLock, time::Duration};

use futures::channel::oneshot;
use serde_json::{Value, json};

use super::control::{Controller, Page, Reply};
use crate::ws::{CdpEvent, CdpSocket};

/// Endereço do CDP de fora, só loopback: o túnel traz a página até aqui, e nada sai da máquina por este caminho.
pub fn endpoint(raw: &str) -> Result<url::Url, String> {
    let raw = raw.trim();
    let refuse = || format!("erro: --cdp so aceita http://127.0.0.1:<porta> ou http://localhost:<porta> (recebi {raw:?})");
    let mut url = url::Url::parse(&if raw.contains("://") { raw.to_owned() } else { format!("http://{raw}") }).map_err(|_| refuse())?;
    // O `webSocketDebuggerUrl` colado direto também vale: lista e WebSocket saem do mesmo endereço.
    if url.scheme() == "ws" { url.set_scheme("http").map_err(|()| refuse())?; }
    let loopback = match url.host() {
        Some(url::Host::Domain(name)) => name == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.scheme() != "http" || !loopback { return Err(refuse()); }
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

/// A página entre as de `/json/list`: a única, ou a que tem `alvo` na URL. Devolve o caminho do WebSocket dela.
pub fn pick_target(list: &Value, alvo: Option<&str>) -> Result<String, String> {
    let pages: Vec<&Value> = list.as_array().into_iter().flatten().filter(|t| t["type"] == "page").collect();
    let listed = |ts: &[&Value]| ts.iter().map(|t| format!("\n  {}  {}", t["url"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""))).collect::<String>();
    let alvo = alvo.filter(|a| !a.is_empty());
    let found: Vec<&Value> = pages.iter().copied().filter(|t| alvo.is_none_or(|a| t["url"].as_str().is_some_and(|u| u.contains(a)))).collect();
    match found.as_slice() {
        [one] => {
            // Sem o endereço o Chrome diz que outro cliente DevTools já está preso à página.
            let ws = one["webSocketDebuggerUrl"].as_str().ok_or_else(|| format!("erro: --cdp: a pagina {} ja tem um cliente DevTools conectado (feche o DevTools dela)", one["url"].as_str().unwrap_or("")))?;
            let url = url::Url::parse(ws).map_err(|_| format!("erro: --cdp: endereco de WebSocket invalido na lista: {ws}"))?;
            Ok(url[url::Position::BeforePath..].to_owned())
        }
        [] if pages.is_empty() => Err("erro: --cdp: nenhuma pagina aberta nesse endereco (a tela web do app esta aberta?)".into()),
        [] => Err(format!("erro: --cdp: nenhuma pagina com {:?} na URL. Paginas:{}", alvo.unwrap_or(""), listed(&pages))),
        many => Err(format!("erro: --cdp: {} paginas; escolha uma com --alvo <trecho da url>:{}", many.len(), listed(many))),
    }
}

struct Job { cdp: url::Url, alvo: Option<String>, verb: String, args: Vec<String>, reply: oneshot::Sender<Reply> }

/// Endereço conferido aqui, antes da fila: o recusado nunca chega a abrir conexão.
pub fn submit(cdp: &str, alvo: Option<String>, verb: String, args: Vec<String>) -> Result<oneshot::Receiver<Reply>, String> {
    let cdp = endpoint(cdp)?;
    let (reply, rx) = oneshot::channel();
    jobs().try_send(Job { cdp, alvo, verb, args, reply }).map_err(|_| "erro: --cdp: a fila das paginas de fora parou (veja o log do app)".to_string())?;
    Ok(rx)
}

fn jobs() -> &'static async_channel::Sender<Job> {
    static JOBS: OnceLock<async_channel::Sender<Job>> = OnceLock::new();
    JOBS.get_or_init(|| {
        let (tx, rx) = async_channel::unbounded();
        // O controlador não é Send: as páginas de fora vivem numa thread só delas, com runtime local.
        let spawned = std::thread::Builder::new().name("nav-cdp-fora".into()).spawn(move || {
            match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => tokio::task::LocalSet::new().block_on(&runtime, serve(rx)),
                Err(e) => eprintln!("[nav] runtime do --cdp nao subiu: {e}"),
            }
        });
        if let Err(e) = spawned { eprintln!("[nav] thread do --cdp nao subiu: {e}"); }
        tx
    })
}

#[derive(Default)]
struct Pages {
    open: RefCell<HashMap<String, Rc<Controller<WsPage>>>>,
    /// Uma conexão por vez: o `objetivo` pede `snapshot` e `url` juntos, e dois controladores dividiriam as refs.
    connecting: futures::lock::Mutex<()>,
}

async fn serve(jobs: async_channel::Receiver<Job>) {
    let pages = Rc::new(Pages::default());
    while let Ok(job) = jobs.recv().await { tokio::task::spawn_local(handle(job, pages.clone())); }
}

async fn handle(job: Job, pages: Rc<Pages>) {
    let Job { cdp, alvo, verb, args, reply } = job;
    // A página escolhida fica presa à chave: o `objetivo` navega e a URL deixa de ter o `--alvo`.
    let key = format!("{cdp}|{}", alvo.as_deref().unwrap_or(""));
    if verb == "close" {
        let closed = pages.open.borrow_mut().remove(&key).is_some();
        let _ = reply.send(Reply::Text(if closed { "ok: close".into() } else { "erro: --cdp: nenhuma conexao aberta com essa pagina".into() }));
        return;
    }
    let ctl = {
        let _one = pages.connecting.lock().await;
        let existing = pages.open.borrow().get(&key).cloned();
        match existing {
            Some(ctl) => ctl,
            None => match connect(&cdp, alvo.as_deref(), &pages, key).await {
                Ok(ctl) => ctl,
                Err(e) => { let _ = reply.send(Reply::Text(e)); return; }
            },
        }
    };
    let _ = reply.send(ctl.run(&verb, &args).await);
}

async fn connect(cdp: &url::Url, alvo: Option<&str>, pages: &Rc<Pages>, key: String) -> Result<Rc<Controller<WsPage>>, String> {
    let down = |e: &dyn std::fmt::Display| format!("erro: --cdp: {cdp} nao respondeu ({e}); a tela web do app de fora esta aberta e o tunel de pe?");
    let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(5))
        .build().map_err(|e| down(&e))?;
    let list: Value = client.get(cdp.join("json/list").map_err(|e| down(&e))?).send().await.and_then(|r| r.error_for_status())
        .map_err(|e| down(&e))?.json().await.map_err(|e| down(&e))?;
    // O WebSocket sai do loopback do --cdp, nunca do host que a lista anuncia.
    let ws = cdp.join(&pick_target(&list, alvo)?).map_err(|e| down(&e))?;
    // `//outro.host/x` na lista viraria outro host no `join`: só segue se continuar no endereço do --cdp.
    if ws.host_str() != cdp.host_str() || ws.port_or_known_default() != cdp.port_or_known_default() {
        return Err(format!("erro: --cdp: a pagina anunciou um WebSocket fora de {cdp}; recusado"));
    }
    let socket = CdpSocket::open_url(&tokio::runtime::Handle::current(), ws);
    let events = socket.events();
    match tokio::time::timeout(Duration::from_secs(12), events.recv()).await {
        Ok(Ok(Ok(CdpEvent::Connected))) => {}
        Ok(Ok(Err(e))) => return Err(format!("erro: --cdp: o WebSocket da pagina recusou ({e:?})")),
        _ => return Err("erro: --cdp: o WebSocket da pagina nao conectou".into()),
    }
    let wire = Rc::new(Wire { socket, next: Cell::new(0), pending: RefCell::default() });
    // Escondida nunca: o realce do alvo aparece na tela do app de fora. E sem `start`: o tamanho é do app dono dela.
    let ctl = Rc::new(Controller::new(WsPage(wire.clone()), false));
    let answer = Rc::downgrade(&wire);
    ctl.on_dialog_answer(move |accept, text, done| {
        let mut params = json!({"accept": accept});
        if let Some(text) = text { params["promptText"] = text.into(); }
        let Some(wire) = answer.upgrade() else { let _ = done.send(Err("a conexao com a pagina caiu".into())); return };
        let call = wire.call("Page.handleJavaScriptDialog", params);
        tokio::task::spawn_local(async move { let _ = done.send(call.await.map(|_| ())); });
    });
    tokio::task::spawn_local(listen(events, Rc::downgrade(&wire), Rc::downgrade(&ctl), pages.clone(), key.clone()));
    pages.open.borrow_mut().insert(key, ctl.clone());
    Ok(ctl)
}

/// Respostas por id para quem pediu; eventos para o controlador. Só referências fracas: fechar a página solta o socket.
async fn listen(events: async_channel::Receiver<Result<CdpEvent, crate::ws::Error>>, wire: Weak<Wire>, ctl: Weak<Controller<WsPage>>, pages: Rc<Pages>, key: String) {
    while let Ok(Ok(CdpEvent::Text(text))) = events.recv().await {
        let Ok(message) = serde_json::from_str::<Value>(&text) else { continue };
        if let Some(id) = message["id"].as_u64() {
            let Some(wire) = wire.upgrade() else { break };
            if let Some(tx) = wire.pending.borrow_mut().remove(&id) {
                let _ = tx.send(match message.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("erro do CDP").to_owned()),
                    None => Ok(message["result"].clone()),
                });
            }
            continue;
        }
        let Some(ctl) = ctl.upgrade() else { break };
        let (method, params) = (message["method"].as_str().unwrap_or(""), &message["params"]);
        let text = |k: &str| params[k].as_str().unwrap_or("").to_owned();
        match method {
            "Page.javascriptDialogOpening" => ctl.set_dialog(Some((text("type"), text("message"), text("defaultPrompt")))),
            "Page.javascriptDialogClosed" => ctl.set_dialog(None),
            _ => {}
        }
        ctl.on_event(method, params);
    }
    // Página fechada ou túnel caído: quem esperava resposta recebe erro agora, e a próxima chamada reconecta.
    if let Some(wire) = wire.upgrade() { wire.pending.borrow_mut().clear(); }
    let mut open = pages.open.borrow_mut();
    if open.get(&key).is_some_and(|c| std::ptr::eq(Rc::as_ptr(c), ctl.as_ptr())) { open.remove(&key); }
}

struct Wire {
    socket: CdpSocket,
    next: Cell<u64>,
    pending: RefCell<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
}

impl Wire {
    fn call(&self, method: &str, params: Value) -> impl Future<Output = Result<Value, String>> + 'static {
        let id = self.next.get() + 1;
        self.next.set(id);
        let (tx, rx) = oneshot::channel();
        self.pending.borrow_mut().insert(id, tx);
        let sent = self.socket.send(&json!({"id": id, "method": method, "params": params}).to_string());
        if sent.is_err() { self.pending.borrow_mut().remove(&id); }
        async move {
            sent.map_err(|e| format!("cdp: o pedido nao saiu ({e:?})"))?;
            rx.await.map_err(|_| "cdp: a conexao com a pagina de fora caiu".to_string())?
        }
    }
}

struct WsPage(Rc<Wire>);

impl Page for WsPage {
    fn call(&self, method: &str, params: Value) -> impl Future<Output = Result<Value, String>> { self.0.call(method, params) }
    fn sleep(&self, ms: u64) -> impl Future<Output = ()> { tokio::time::sleep(Duration::from_millis(ms)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn only_loopback_endpoints_are_accepted() {
        for ok in ["http://127.0.0.1:9222", "127.0.0.1:9222", "http://localhost:9222/", "ws://127.0.0.1:9222"] {
            assert_eq!(endpoint(ok).map(|u| u.port()), Ok(Some(9222)), "{ok}");
        }
        for bad in ["http://192.168.77.142:9222", "http://example.com:9222", "http://127.0.0.1.example.com:9222", "https://localhost:9222", "file:///x", ""] {
            assert!(endpoint(bad).unwrap_err().starts_with("erro: --cdp"), "{bad}");
        }
    }

    fn page(id: &str, url: &str) -> Value {
        json!({"id": id, "type": "page", "title": "t", "url": url, "webSocketDebuggerUrl": format!("ws://127.0.0.1:9222/devtools/page/{id}")})
    }

    #[test]
    fn the_single_page_is_the_default_and_alvo_picks_by_url() {
        let worker = json!({"id": "W", "type": "service_worker", "url": "http://app.local/sw.js", "webSocketDebuggerUrl": "ws://x/devtools/page/W"});
        let one = json!([page("A", "http://app.local/agenda"), worker]);
        assert_eq!(pick_target(&one, None), Ok("/devtools/page/A".into()));
        let two = json!([page("A", "http://app.local/agenda"), page("B", "http://app.local/login")]);
        assert_eq!(pick_target(&two, Some("/login")), Ok("/devtools/page/B".into()));
        let ambiguous = pick_target(&two, None).unwrap_err();
        assert!(ambiguous.starts_with("erro: --cdp: 2 paginas") && ambiguous.contains("http://app.local/agenda") && ambiguous.contains("http://app.local/login"), "{ambiguous}");
        let none = pick_target(&two, Some("/financeiro")).unwrap_err();
        assert!(none.contains("nenhuma pagina") && none.contains("http://app.local/login"), "{none}");
        let busy = json!([{"id": "A", "type": "page", "url": "http://app.local/"}]);
        assert!(pick_target(&busy, None).unwrap_err().contains("DevTools"));
    }

    /// CDP falso de verdade (HTTP + WebSocket): `/json/list` anuncia o WebSocket noutro host, para provar que a
    /// conexão fica no loopback do `--cdp`; a árvore passa de 1 MiB, o teto do terminal.
    async fn fake_cdp() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") { head.push(socket.read_u8().await.unwrap()); }
                    let head = String::from_utf8(head).unwrap();
                    assert!(!head.to_ascii_lowercase().contains("authorization"), "nenhum token vai ao CDP de fora");
                    if head.starts_with("GET /json/list ") {
                        let body = json!([page("A", "http://app.local/agenda")]).to_string().replace("127.0.0.1:9222", "evil.example:9");
                        let _ = socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
                        return;
                    }
                    assert!(head.starts_with("GET /devtools/page/A "), "{head}");
                    let key = head.lines().find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case("sec-websocket-key")).map(|(_, v)| v.trim().to_owned())).unwrap();
                    socket.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", crate::ws::accept_key(&key)).as_bytes()).await.unwrap();
                    while let Ok(frame) = crate::ws::read_client_frame(&mut socket).await {
                        let call: Value = serde_json::from_slice(&frame.data).unwrap();
                        let result = match call["method"].as_str().unwrap() {
                            "Accessibility.getFullAXTree" => json!({"pad": "x".repeat(2 << 20), "nodes": [
                                {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "x"}, "childIds": ["2"]},
                                {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "Ok"}, "backendDOMNodeId": 7, "childIds": []}]}),
                            "Page.getNavigationHistory" => json!({"currentIndex": 0, "entries": [{"url": "http://app.local/agenda"}]}),
                            _ => json!({}),
                        };
                        let reply = json!({"id": call["id"], "result": result}).to_string();
                        crate::ws::write_server_frame(&mut socket, 1, reply.as_bytes()).await.unwrap();
                    }
                });
            }
        });
        addr
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn verbs_run_on_the_external_page_with_the_session_formats() {
        let addr = fake_cdp().await;
        let run = |verb: &str| {
            let rx = submit(&format!("http://{addr}"), None, verb.into(), vec![]).unwrap();
            async move { match tokio::time::timeout(Duration::from_secs(10), rx).await.unwrap().unwrap() { Reply::Text(t) | Reply::Error(_, t) => t, Reply::Png(_) => "png".into() } }
        };
        assert_eq!(run("snapshot").await, "- RootWebArea \"x\"\n  - button \"Ok\" [ref=@e1]");
        assert_eq!(run("url").await, "ok: \"http://app.local/agenda\"");
        assert_eq!(run("close").await, "ok: close");
        assert!(submit("http://10.0.0.1:9222", None, "url".into(), vec![]).unwrap_err().starts_with("erro: --cdp"));
    }
}
