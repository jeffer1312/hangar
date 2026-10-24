//! Um Chromium por processo do app, compartilhado pelos navegadores das sessões, e as sessões CDP sobre o pipe dele.
//! Respostas voltam direto da thread de leitura; eventos são entregues na thread da interface, como no WebView2.
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    future::Future,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::Child,
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use futures::future::{FutureExt, LocalBoxFuture, Shared};
use gpui_kit::{BackgroundExecutor, ForegroundExecutor, Task};
use serde_json::{Value, json};

use super::launch;

type Reply = Result<Value, String>;
type Handler = Rc<RefCell<dyn FnMut(Value)>>;
/// Consome um evento na própria thread de leitura (o quadro do painel, entregue sem passar pela interface).
pub type Sink = Box<dyn Fn(Value) + Send>;
/// Confirma ao Chromium o quadro de screencast com este número, de qualquer thread.
pub type Acker = Box<dyn Fn(Value) + Send + Sync>;
type Ready = Shared<LocalBoxFuture<'static, Result<(), String>>>;
const LAUNCH_WAIT: Duration = Duration::from_secs(10);

/// O lado que qualquer thread usa: escrever e casar respostas.
struct Wire {
    writer: Mutex<std::io::PipeWriter>,
    next: AtomicU64,
    /// id → (sessão, quem espera).
    pending: Mutex<HashMap<u64, (Option<String>, Box<dyn FnOnce(Reply) + Send>)>>,
    /// Sessões cuja página caiu: o Chromium aceita o comando e nunca responde, então ele falha já.
    /// ponytail: só cresce; uma entrada por página caída.
    fallen: Mutex<HashSet<String>>,
    /// Quadro de screencast por sessão, tratado antes de chegar à interface.
    sinks: Mutex<HashMap<String, Sink>>,
    /// Clientes CDP de fora (`/cdp/browser`): cada um recebe os eventos das sessões dele, e só deles.
    taps: Mutex<HashMap<u64, Tapped>>,
    /// O processo; quem o fecha (o `Drop`, o `retire`) ou a thread de leitura, quando ele sai sozinho, o recolhe.
    child: Mutex<Option<Child>>,
    /// Por que este Chromium não serve mais. Gravado pela thread de leitura assim que o pipe fecha (sem esperar a
    /// interface): quem chama depois recebe isto na hora, e o próximo pedido sobe outro processo.
    gone: OnceLock<String>,
    /// O app pediu o fim (fechar, refazer): a saída não é queda e não vai ao diário.
    asked: AtomicBool,
}

struct Tapped {
    /// Sessões que o cliente abriu e as filhas delas (iframe, worker), que o auto-attach dele traz.
    scope: HashSet<String>,
    out: async_channel::Sender<String>,
}

impl Wire {
    fn new(writer: std::io::PipeWriter, child: Option<Child>) -> Arc<Wire> {
        Arc::new(Wire { writer: Mutex::new(writer), next: AtomicU64::new(1), pending: Mutex::default(), fallen: Mutex::default(), sinks: Mutex::default(),
            taps: Mutex::default(), child: Mutex::new(child), gone: OnceLock::new(), asked: AtomicBool::new(false) })
    }

    fn alive(&self) -> bool { self.gone.get().is_none() }

    /// Marca o fim pedido pelo app; `false` quando o processo já tinha saído sozinho, e aí a queda segue ao diário.
    fn retire(&self, reason: &str) -> bool {
        if self.gone.set(format!("{RETIRED}: {reason}")).is_err() { return false; }
        self.asked.store(true, Ordering::SeqCst);
        true
    }

    fn send(&self, session: Option<&str>, method: &str, params: Value, done: Option<Box<dyn FnOnce(Reply) + Send>>) {
        // Processo que já saiu: a escrita poderia até passar (pipe ainda aberto por um instante) e ninguém responderia.
        if let Some(gone) = self.gone.get() {
            if let Some(done) = done { done(Err(gone.clone())); }
            return;
        }
        if session.is_some_and(|s| lock(&self.fallen).contains(s)) {
            if let Some(done) = done { done(Err("a pagina fechou".into())); }
            return;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session { message["sessionId"] = session.into(); }
        if let Some(done) = done { lock(&self.pending).insert(id, (session.map(str::to_owned), done)); }
        let mut bytes = message.to_string().into_bytes();
        bytes.push(0);
        if let Err(e) = lock(&self.writer).write_all(&bytes) {
            // Sem o pipe não haverá resposta: quem espera recebe o erro agora.
            if let Some((_, done)) = lock(&self.pending).remove(&id) { done(Err(format!("{EXITED}: o Chromium fechou: {e}"))); }
        }
    }

    /// Sessão que caiu não responde o que já estava em curso: quem espera por ela recebe o erro agora.
    fn fail(&self, session: &str) {
        let gone: Vec<_> = lock(&self.pending).extract_if(|_, (s, _)| s.as_deref() == Some(session)).collect();
        for (_, (_, done)) in gone { done(Err("a pagina fechou".into())); }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(PoisonError::into_inner) }

/// Texto do erro do CDP, o mesmo que o WebView2 devolve: o controlador reconhece "No node", "SyntaxError" etc.
fn reply_of(message: &Value) -> Reply {
    match message.get("error") {
        Some(error) => Err(error["message"].as_str().map_or_else(|| error.to_string(), str::to_owned)),
        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
    }
}

pub struct Browser {
    wire: Arc<Wire>,
    /// Escala do processo (`--force-device-scale-factor`): a de todo quadro do screencast.
    scale: u32,
    /// (sessão, evento) → ouvintes.
    handlers: RefCell<HashMap<(String, String), Vec<Handler>>>,
    /// Evento do navegador (`Target.*`, `CLOSED`) → (sessão dona, ouvinte); sai junto com a sessão dona.
    watchers: RefCell<HashMap<String, Vec<(String, Handler)>>>,
    /// Abas que o Chromium abre sozinho ao nascer; o primeiro navegador as fecha.
    initial: RefCell<Vec<String>>,
    /// Páginas que os navegadores do app abriram; qualquer outra é janela que a página abriu.
    owned: RefCell<HashSet<String>>,
    /// Páginas que caíram. Sem renderer a página cai antes do `createTarget` responder, quando ninguém a ouve ainda.
    /// ponytail: só cresce; queda é rara e o processo doente é trocado.
    crashed: RefCell<HashSet<String>>,
    /// Aperto de mão do processo que nasceu sem travar a interface; `None` quando já respondeu.
    ready: RefCell<Option<Ready>>,
    _drain: RefCell<Option<Task<()>>>,
}

/// Qual Chromium compartilhado: o do painel e dos sites (perfil com cookies e login) ou o das páginas que o agente
/// publica na conversa, que só abrem contextos próprios.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Kind { Panel, Pages }

impl Kind {
    /// O painel fica em 1 e emula a escala da janela por alvo. As páginas da conversa saem no dobro, reduzidas pela GPU:
    /// em 1x o texto delas, sobre fundo transparente, só é suavizado em cinza e destoa do texto do app.
    fn scale(self) -> u32 { match self { Kind::Panel => 1, Kind::Pages => 2 } }

    fn folder(self) -> &'static str { match self { Kind::Panel => "chromium", Kind::Pages => "chromium-pages" } }
}

thread_local! {
    static SHARED: RefCell<[Weak<Browser>; 2]> = RefCell::new([Weak::new(), Weak::new()]);
}

impl Browser {
    /// O Chromium em uso, ou um novo, sem esperar o processo responder: o aperto de mão corre sozinho e `ready` o
    /// espera. Para a interface não congelar enquanto o Chromium nasce.
    pub fn shared_async(executor: &ForegroundExecutor, background: &BackgroundExecutor, kind: Kind) -> Result<Rc<Browser>, String> {
        if let Some(browser) = Self::current(kind) { return Ok(browser); }
        let bin = launch::find()?;
        let profile = Self::profile(kind)?;
        // Cookies trazidos do Chrome são do perfil do painel; o das páginas nem guarda login.
        let carry = (kind == Kind::Panel).then(|| super::carry::prepare(&profile)).flatten();
        let browser = Self::spawn(bin, &profile, kind.scale(), executor)?;
        let handshake = browser.handshake(bin, background).boxed_local().shared();
        if let Some(copy) = carry {
            let (ready, weak, background) = (handshake.clone(), Rc::downgrade(&browser), background.clone());
            executor.spawn(async move {
                if ready.await.is_err() { return; }
                if let Some(browser) = weak.upgrade() { super::carry::run(browser, profile, copy, background).await; }
            }).detach();
        }
        // Corre mesmo sem ninguém esperar: quem só aquece o processo não fica olhando.
        executor.spawn(handshake.clone()).detach();
        *browser.ready.borrow_mut() = Some(handshake);
        SHARED.with(|s| s.borrow_mut()[kind as usize] = Rc::downgrade(&browser));
        Ok(browser)
    }

    /// Resolve quando o processo já respondeu ao aperto de mão (na hora, se nasceu bloqueando).
    pub fn ready(&self) -> impl Future<Output = Result<(), String>> + use<> {
        let pending = self.ready.borrow().clone();
        async move { match pending { Some(ready) => ready.await, None => Ok(()) } }
    }

    fn current(kind: Kind) -> Option<Rc<Browser>> { SHARED.with(|s| s.borrow()[kind as usize].upgrade()).filter(|b| b.alive()) }

    fn profile(kind: Kind) -> Result<std::path::PathBuf, String> {
        crate::appearance::dir().map(|d| d.join(kind.folder())).ok_or_else(|| "sem pasta de configuracao do app".into())
    }

    /// As chamadas saem já, nesta ordem: o `getTargets` chega antes de qualquer `createTarget` de quem vier depois, e
    /// as abas iniciais continuam sendo só as do nascimento.
    fn handshake(self: &Rc<Self>, bin: &Path, background: &BackgroundExecutor) -> impl Future<Output = Result<(), String>> + use<> {
        let version = self.call(None, "Browser.getVersion", json!({}));
        let discover = self.call(None, "Target.setDiscoverTargets", json!({"discover": true}));
        let targets = self.call(None, "Target.getTargets", json!({}));
        // Sem a negação o download cai na pasta do usuário sem ninguém pedir: faz parte do aperto de mão, com o mesmo prazo.
        let deny = self.call(None, "Browser.setDownloadBehavior", json!({"behavior": "deny"}));
        let (weak, timer, bin) = (Rc::downgrade(self), background.timer(LAUNCH_WAIT), bin.display().to_string());
        async move {
            let answers = async { Ok::<_, String>((version.await?, discover.await?, targets.await?, deny.await?)) };
            let answers = match futures::future::select(std::pin::pin!(answers), std::pin::pin!(timer)).await {
                futures::future::Either::Left((answers, _)) => answers,
                futures::future::Either::Right(_) => Err("tempo esgotado".into()),
            };
            let Some(browser) = weak.upgrade() else { return Err("o Chromium fechou".into()) };
            match answers {
                Ok((_, _, targets, _)) => {
                    *browser.initial.borrow_mut() = targets["targetInfos"].as_array().into_iter().flatten()
                        .filter(|t| t["type"] == "page").filter_map(|t| t["targetId"].as_str().map(str::to_owned)).collect();
                    Ok(())
                }
                Err(e) => {
                    // Processo mudo não fica como o compartilhado: o próximo pedido sobe outro.
                    let e = format!("o Chromium ({bin}) nao respondeu: {e}");
                    browser.retire(&e);
                    Err(e)
                }
            }
        }
    }

    /// Processo, thread de leitura e entrega dos eventos; nenhuma espera pelo CDP.
    fn spawn(bin: &Path, profile: &Path, scale: u32, executor: &ForegroundExecutor) -> Result<Rc<Browser>, String> {
        let launched = launch::spawn(bin, profile, scale).map_err(|e| format!("o Chromium ({}) nao abriu: {e}", bin.display()))?;
        let wire = Wire::new(launched.writer, Some(launched.child));
        let (events, received) = async_channel::unbounded::<Value>();
        let reading = wire.clone();
        std::thread::Builder::new().name("chromium-pipe".into())
            .spawn(move || read(launched.reader, &reading, &events))
            .map_err(|e| e.to_string())?;
        let browser = Rc::new(Browser {
            wire, scale, handlers: RefCell::default(), watchers: RefCell::default(),
            initial: RefCell::default(), owned: RefCell::default(), crashed: RefCell::default(), ready: RefCell::new(None),
            _drain: RefCell::new(None),
        });
        let weak = Rc::downgrade(&browser);
        *browser._drain.borrow_mut() = Some(executor.spawn(async move {
            while let Ok(message) = received.recv().await {
                let Some(browser) = weak.upgrade() else { break };
                browser.dispatch(message);
            }
            if let Some(browser) = weak.upgrade() { browser.closed(); }
        }));
        Ok(browser)
    }

    pub fn alive(&self) -> bool { self.wire.alive() }

    pub fn scale(&self) -> u32 { self.scale }

    /// Este Chromium não serve mais (morreu no meio de uma abertura, não consegue criar página): sai já de cena, para o
    /// próximo pedido subir outro no mesmo perfil sem dois processos nele.
    /// ponytail: SIGKILL sem `Browser.close`; o que o processo doente ainda não gravou do perfil se perde.
    pub fn retire(&self, reason: &str) {
        if !self.wire.retire(reason) { return; }
        // Descarte também derruba as páginas de todas as sessões: vai ao diário, como a queda.
        let line = format!("[nav] Chromium descartado: {reason}");
        eprintln!("{line}");
        crate::log_line(&line);
        if let Some(mut child) = lock(&self.wire.child).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    #[cfg(test)]
    pub(super) fn pid(&self) -> Option<u32> { lock(&self.wire.child).as_ref().map(Child::id) }

    pub fn port(&self) -> Port { Port(self.wire.clone()) }

    pub fn own(&self, target: &str, owned: bool) {
        if owned { self.owned.borrow_mut().insert(target.to_owned()); } else { self.owned.borrow_mut().remove(target); }
    }

    pub fn owns(&self, target: &str) -> bool { self.owned.borrow().contains(target) }

    pub fn crashed(&self, target: &str) -> bool { self.crashed.borrow().contains(target) }

    /// Fecha as abas que nasceram com o processo, depois que já existe outra (sem nenhuma o Chromium sairia).
    pub fn close_initial(&self) {
        for target in self.initial.take() { drop(self.call(None, "Target.closeTarget", json!({"targetId": target}))); }
    }

    fn closed(&self) { self.notify(CLOSED, Value::Null); }

    fn notify(&self, method: &str, params: Value) {
        // Clonados antes de chamar: o ouvinte pode registrar outro sem o mapa estar emprestado.
        let watchers: Vec<Handler> = self.watchers.borrow().get(method).into_iter().flatten().map(|(_, h)| h.clone()).collect();
        for handler in watchers { (handler.borrow_mut())(params.clone()); }
    }

    fn dispatch(&self, message: Value) {
        let Some(method) = message["method"].as_str() else { return };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if method == "Target.targetCrashed" && let Some(id) = params["targetId"].as_str() { self.crashed.borrow_mut().insert(id.to_owned()); }
        // Alvo fechado pelo app segue dele até sumir: um popup largado ainda manda `targetInfoChanged` e, sem dono,
        // seria aberto na página que o abriu.
        if method == "Target.targetDestroyed" && let Some(id) = params["targetId"].as_str() { self.owned.borrow_mut().remove(id); }
        let Some(session) = message["sessionId"].as_str() else { return self.notify(method, params) };
        let handlers = self.handlers.borrow().get(&(session.to_owned(), method.to_owned())).cloned().unwrap_or_default();
        for handler in handlers { (handler.borrow_mut())(params.clone()); }
    }

    /// Envia já; o futuro só espera a resposta. Descartar o futuro não cancela o comando.
    pub fn call(&self, session: Option<&str>, method: &str, params: Value) -> impl Future<Output = Reply> + use<> {
        let (tx, rx) = futures::channel::oneshot::channel();
        self.wire.send(session, method, params, Some(Box::new(move |reply| { let _ = tx.send(reply); })));
        async move { rx.await.map_err(|_| "o Chromium fechou".to_string())? }
    }

    pub fn on(&self, session: &str, event: &str, f: impl FnMut(Value) + 'static) {
        let handler: Handler = Rc::new(RefCell::new(f));
        self.handlers.borrow_mut().entry((session.to_owned(), event.to_owned())).or_default().push(handler);
    }

    /// Evento do navegador, ouvido enquanto a sessão `owner` existir.
    pub fn watch(&self, owner: &str, event: &str, f: impl FnMut(Value) + 'static) {
        let handler: Handler = Rc::new(RefCell::new(f));
        self.watchers.borrow_mut().entry(event.to_owned()).or_default().push((owner.to_owned(), handler));
    }

    /// Quadros desta sessão vão ao `sink`, sem confirmação: quem os desenha confirma com o `acker`.
    pub fn sink(&self, session: &str, sink: Sink) { lock(&self.wire.sinks).insert(session.to_owned(), sink); }

    pub fn acker(&self, session: &str) -> Acker {
        let (wire, session) = (self.wire.clone(), session.to_owned());
        Box::new(move |id| wire.send(Some(&session), "Page.screencastFrameAck", json!({"sessionId": id}), None))
    }

    fn forget(&self, session: &str) {
        self.handlers.borrow_mut().retain(|(s, _), _| s != session);
        for list in self.watchers.borrow_mut().values_mut() { list.retain(|(owner, _)| owner != session); }
        lock(&self.wire.sinks).remove(session);
    }
}

/// Evento sintético entregue a quem ouve o navegador quando o pipe fecha.
pub const CLOSED: &str = "Hangar.closed";
/// Início do erro de toda chamada a um Chromium que saiu sozinho (queda) ou que o app descartou.
pub const EXITED: &str = "chromium_exited";
pub const RETIRED: &str = "chromium_retired";

impl Drop for Browser {
    fn drop(&mut self) {
        // Saída limpa primeiro: o perfil é persistente, e o SIGKILL perde cookies e localStorage ainda não gravados.
        self.wire.asked.store(true, Ordering::SeqCst);
        self.wire.send(None, "Browser.close", json!({}), None);
        let Some(mut child) = lock(&self.wire.child).take() else { return };
        // A espera (até 2 s) fora da thread da tela, que fechava o painel congelada.
        let waited = std::thread::Builder::new().name("chromium-exit".into()).spawn(move || {
            for _ in 0..40 {
                if !matches!(child.try_wait(), Ok(None)) { return; }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        });
        if let Err(e) = waited { eprintln!("[nav] sem thread para fechar o Chromium: {e}"); }
    }
}

/// Lê mensagens terminadas em NUL. Respostas resolvem quem espera; quadros com dono vão ao dono; o resto vai à interface.
fn read(reader: std::io::PipeReader, wire: &Wire, events: &async_channel::Sender<Value>) {
    let mut reader = BufReader::with_capacity(1 << 20, reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(0, &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buffer.last() == Some(&0) { buffer.pop(); }
        let Ok(message) = serde_json::from_slice::<Value>(&buffer) else { continue };
        route(message, wire, events);
    }
    // Antes de soltar quem espera: o erro deles já diz como o processo saiu.
    let _ = wire.gone.set(exited(wire));
    let gone = wire.gone.get().cloned().unwrap_or_default();
    for (_, (_, done)) in lock(&wire.pending).drain() { done(Err(gone.clone())); }
    // O socket de cada cliente de fora fecha depois de entregar o que já estava na fila.
    for (_, tap) in lock(&wire.taps).drain() { tap.out.close(); }
    events.close();
}

/// Recolhe o processo que fechou o pipe e diz como ele saiu; queda que o app não pediu vai ao diário com o fim do
/// `chromium.log`.
fn exited(wire: &Wire) -> String {
    let child = lock(&wire.child).take();
    let status = child.and_then(|mut child| {
        // O pipe fecha na saída do processo: a espera é de instantes. Pipe fechado com o processo ainda de pé (travado)
        // não segura a thread; ele é morto.
        for _ in 0..40 {
            if let Ok(Some(status)) = child.try_wait() { return Some(status); }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        child.wait().ok()
    });
    let how = status.map_or_else(|| "saida desconhecida".to_owned(), |s| {
        use std::os::unix::process::ExitStatusExt;
        s.signal().map_or_else(|| format!("codigo {}", s.code().unwrap_or(-1)), |n| format!("sinal {n}"))
    });
    let reason = format!("{EXITED}: o Chromium fechou ({how})");
    if !wire.asked.load(Ordering::SeqCst) {
        let tail = std::fs::read_to_string(launch::log_path().unwrap_or_default()).map(|log| crash_tail(&log)).unwrap_or_default();
        let line = format!("[nav] {reason}; fim do chromium.log: {tail}");
        eprintln!("{line}");
        crate::log_line(&line);
    }
    reason
}

/// As últimas linhas de texto do `chromium.log`, sem os quadros da pilha e os registradores que o Chromium despeja na
/// queda: o que sobra diz o sinal e o que vinha acontecendo.
fn crash_tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with(' ')).collect();
    lines[lines.len().saturating_sub(6)..].iter().map(|l| l.chars().take(200).collect::<String>()).collect::<Vec<_>>().join(" | ")
}

fn route(mut message: Value, wire: &Wire, events: &async_channel::Sender<Value>) {
    if let Some(id) = message["id"].as_u64() {
        if let Some((_, done)) = lock(&wire.pending).remove(&id) { done(reply_of(&message)); }
        return;
    }
    if tapped(&message, wire) { return; }
    if message["method"] == "Page.screencastFrame"
        && let Some(session) = message["sessionId"].as_str().map(str::to_owned)
        && lock(&wire.sinks).contains_key(&session)
    {
        // Os parâmetros vão inteiros ao dono, sem copiar o quadro; ele confirma depois de desenhar.
        if let Some(sink) = lock(&wire.sinks).get(&session) { sink(message["params"].take()); }
        return;
    }
    let _ = events.try_send(message);
}

/// Evento de sessão de um cliente de fora vai só a ele. O fim de uma sessão dele no nível do navegador
/// (`Target.detachedFromTarget`) vai a ele e segue também para a interface, que confere se era dela.
fn tapped(message: &Value, wire: &Wire) -> bool {
    let mut taps = lock(&wire.taps);
    if taps.is_empty() { return false; }
    let (method, child) = (message["method"].as_str().unwrap_or(""), message["params"]["sessionId"].as_str());
    let Some(session) = message["sessionId"].as_str() else {
        if method == "Target.detachedFromTarget" && let Some(child) = child
            && let Some(tap) = taps.values_mut().find(|t| t.scope.contains(child)) {
            tap.scope.remove(child);
            let _ = tap.out.try_send(message.to_string());
            drop(taps);
            wire.fail(child);
        }
        return false;
    };
    let Some(tap) = taps.values_mut().find(|t| t.scope.contains(session)) else { return false };
    let _ = tap.out.try_send(message.to_string());
    match (method, child) {
        ("Target.attachedToTarget", Some(child)) => { tap.scope.insert(child.to_owned()); }
        ("Target.detachedFromTarget", Some(child)) => {
            tap.scope.remove(child);
            drop(taps);
            // O que o cliente ainda esperava dessa sessão não vai ser respondido.
            wire.fail(child);
        }
        _ => {}
    }
    true
}

/// O pipe visto de qualquer thread, para o cliente CDP de fora: cada comando ganha um id novo do `Wire` (nunca
/// colide com os do app) e os eventos chegam filtrados pelas sessões do cliente.
#[derive(Clone)]
pub struct Port(Arc<Wire>);

impl Port {
    pub fn call(&self, session: Option<&str>, method: &str, params: Value) -> impl Future<Output = Reply> + Send + use<> {
        let (tx, rx) = futures::channel::oneshot::channel();
        self.0.send(session, method, params, Some(Box::new(move |reply| { let _ = tx.send(reply); })));
        async move { rx.await.map_err(|_| "o Chromium fechou".to_string())? }
    }

    /// Registra um cliente; `out` é fechado (para todos os remetentes) quando o Chromium cai.
    pub fn tap(&self, out: async_channel::Sender<String>) -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        lock(&self.0.taps).insert(id, Tapped { scope: HashSet::new(), out });
        id
    }

    pub fn adopt(&self, tap: u64, session: &str) {
        if let Some(t) = lock(&self.0.taps).get_mut(&tap) { t.scope.insert(session.to_owned()); }
    }

    pub fn owns(&self, tap: u64, session: &str) -> bool { lock(&self.0.taps).get(&tap).is_some_and(|t| t.scope.contains(session)) }

    /// Tira o cliente e devolve as sessões que ainda eram dele, para quem sai soltá-las.
    pub fn untap(&self, tap: u64) -> Vec<String> { lock(&self.0.taps).remove(&tap).map(|t| t.scope.into_iter().collect()).unwrap_or_default() }

    #[cfg(test)]
    pub(in crate::browser) fn test() -> (Port, std::io::PipeReader) {
        let (wire, reader) = tests::wire();
        (Port(wire), reader)
    }

    /// Chromium de verdade num perfil descartável, sem a interface: os eventos dela são descartados.
    #[cfg(test)]
    pub(in crate::browser) fn launch(profile: &Path) -> (Port, Child) {
        let launched = launch::spawn(launch::find().unwrap(), profile, Kind::Panel.scale()).unwrap();
        let wire = Wire::new(launched.writer, None);
        // O processo fica com o teste; o fim dele não vai ao diário de quem roda.
        wire.asked.store(true, Ordering::SeqCst);
        let (events, received) = async_channel::unbounded();
        std::mem::forget(received);
        let reading = wire.clone();
        std::thread::spawn(move || read(launched.reader, &reading, &events));
        (Port(wire), launched.child)
    }

    /// Mensagem como se viesse do Chromium; devolve o que seguiria para a interface.
    #[cfg(test)]
    pub(in crate::browser) fn inject(&self, message: Value) -> Option<Value> {
        let (events, received) = async_channel::unbounded();
        route(message, &self.0, &events);
        received.try_recv().ok()
    }
}

/// Uma sessão CDP num alvo, com a mesma forma do `cdp::Cdp` do WebView2. Cair a sessão a desliga do alvo.
pub struct Session {
    browser: Rc<Browser>,
    id: String,
    /// O motor do painel e o controlador do hangar-preview dividem esta sessão: aqui um sabe o tamanho que o outro pôs.
    pub viewport: Viewport,
}

/// Um dono por vez para o tamanho da página: o painel, ou o layout pedido pelo agente enquanto ele valer.
#[derive(Default)]
pub struct Viewport {
    /// Largura, altura e densidade que o painel aplicou por último.
    pub panel: Cell<Option<(u32, u32, f32)>>,
    /// Layout pedido (`mobile`, `<l> <a>`) em vigor: o painel não aplica o dele.
    pub held: Cell<bool>,
}

impl Session {
    /// Liga uma sessão CDP ao alvo, sem travar a interface.
    pub fn attach_async(browser: &Rc<Browser>, target: &str) -> impl Future<Output = Result<Session, String>> + use<> {
        let (call, browser) = (browser.call(None, "Target.attachToTarget", json!({"targetId": target, "flatten": true})), browser.clone());
        async move {
            let id = call.await?["sessionId"].as_str().ok_or("attachToTarget sem sessionId")?.to_owned();
            Ok(Session { browser, id, viewport: Viewport::default() })
        }
    }

    pub fn id(&self) -> &str { &self.id }
    pub fn browser(&self) -> &Rc<Browser> { &self.browser }

    /// O alvo caiu ou a sessão se soltou: as chamadas em curso e as seguintes recebem erro em vez de esperar para sempre.
    pub fn fail_pending(&self) {
        lock(&self.browser.wire.fallen).insert(self.id.clone());
        self.browser.wire.fail(&self.id);
    }

    pub fn call(&self, method: &str, params: Value) -> impl Future<Output = Reply> + use<> {
        self.browser.call(Some(&self.id), method, params)
    }

    pub fn on(&self, event: &str, f: impl FnMut(Value) + 'static) -> Result<(), String> {
        self.browser.on(&self.id, event, f);
        Ok(())
    }

    /// Evento do navegador inteiro, ouvido enquanto esta sessão existir.
    pub fn watch(&self, event: &str, f: impl FnMut(Value) + 'static) { self.browser.watch(&self.id, event, f); }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.browser.forget(&self.id);
        drop(self.browser.call(None, "Target.detachFromTarget", json!({"sessionId": self.id})));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    pub(super) fn wire() -> (Arc<Wire>, std::io::PipeReader) {
        let (reader, writer) = std::io::pipe().unwrap();
        (Wire::new(writer, None), reader)
    }

    #[test]
    fn retire_after_a_real_death_leaves_it_for_the_diary() {
        let (dead, _r) = wire();
        let _ = dead.gone.set("morreu sozinho".into());
        assert!(!dead.retire("doente"));
        assert!(!dead.asked.load(Ordering::SeqCst), "a queda de verdade ainda vai ao diário");
        let (alive, _r) = wire();
        assert!(alive.retire("doente"));
        assert!(alive.asked.load(Ordering::SeqCst));
        assert!(alive.gone.get().unwrap().ends_with("doente"));
    }

    #[test]
    fn messages_are_nul_terminated_and_carry_the_session() {
        let (wire, reader) = wire();
        wire.send(Some("S1"), "Page.enable", json!({}), None);
        wire.send(None, "Browser.getVersion", json!({}), None);
        drop(wire);
        let mut all = Vec::new();
        std::io::Read::read_to_end(&mut { reader }, &mut all).unwrap();
        let parts: Vec<Value> = all.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| serde_json::from_slice(p).unwrap()).collect();
        assert_eq!(parts[0], json!({"id": 1, "method": "Page.enable", "params": {}, "sessionId": "S1"}));
        assert_eq!(parts[1], json!({"id": 2, "method": "Browser.getVersion", "params": {}}));
    }

    #[test]
    fn replies_resolve_by_id_and_errors_keep_the_cdp_text() {
        let (wire, _reader) = wire();
        let (events, received) = async_channel::unbounded();
        let (tx, rx) = mpsc::channel();
        let ok = tx.clone();
        wire.send(None, "A", json!({}), Some(Box::new(move |r| ok.send(r).unwrap())));
        wire.send(None, "B", json!({}), Some(Box::new(move |r| tx.send(r).unwrap())));
        route(json!({"id": 2, "error": {"code": -32000, "message": "No node with given id found"}}), &wire, &events);
        route(json!({"id": 1, "result": {"v": 1}}), &wire, &events);
        assert_eq!(rx.recv().unwrap(), Err("No node with given id found".into()));
        assert_eq!(rx.recv().unwrap(), Ok(json!({"v": 1})));
        route(json!({"method": "Page.frameNavigated", "sessionId": "S", "params": {}}), &wire, &events);
        assert_eq!(received.try_recv().unwrap()["method"], "Page.frameNavigated");
    }

    #[test]
    fn a_dropped_session_fails_only_its_own_calls() {
        let (wire, _reader) = wire();
        let (tx, rx) = mpsc::channel();
        for s in [Some("S"), Some("T"), None] {
            let tx = tx.clone();
            wire.send(s, "A", json!({}), Some(Box::new(move |r| tx.send((s, r)).unwrap())));
        }
        wire.fail("S");
        assert_eq!(rx.try_recv().unwrap(), (Some("S"), Err("a pagina fechou".into())));
        assert!(rx.try_recv().is_err());
        assert_eq!(lock(&wire.pending).len(), 2);
    }

    #[test]
    fn a_dead_chromium_fails_the_waiters_and_every_later_call_with_how_it_exited() {
        let child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        let (_theirs, writer) = std::io::pipe().unwrap();
        let wire = Wire::new(writer, Some(child));
        // Sem ir ao diário de verdade de quem roda o teste.
        wire.asked.store(true, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        let waiting = tx.clone();
        wire.send(None, "Runtime.evaluate", json!({}), Some(Box::new(move |r| waiting.send(r).unwrap())));
        // O fd 4 do "Chromium": fecha quando ele morre.
        let (from_chromium, chromium_end) = std::io::pipe().unwrap();
        let (reading, (events, _received)) = (wire.clone(), async_channel::unbounded());
        let reader = std::thread::spawn(move || read(from_chromium, &reading, &events));
        assert!(wire.alive());
        // SAFETY: sinal para o filho que este teste criou.
        unsafe { libc::kill(pid, libc::SIGKILL); }
        drop(chromium_end);
        let reply = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err();
        assert_eq!(reply, format!("{EXITED}: o Chromium fechou (sinal 9)"));
        reader.join().unwrap();
        assert!(!wire.alive());
        assert!(lock(&wire.child).is_none(), "recolhido, sem zumbi");
        let started = std::time::Instant::now();
        wire.send(None, "Browser.getVersion", json!({}), Some(Box::new(move |r| tx.send(r).unwrap())));
        assert_eq!(rx.try_recv().unwrap(), Err(reply), "na hora, sem escrever no pipe");
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(lock(&wire.pending).is_empty());
    }

    #[test]
    fn the_crash_tail_keeps_the_signal_line_and_drops_frames_and_registers() {
        let log = "[1013/152052:INFO:CONSOLE:782] \"[HMR] connected\"\nReceived signal 11 SEGV_MAPERR 000000000000\n#0 0x55 (x+0x4)\n#1 0x56 (x+0x5)\n  r8: 00 r9: 40\n[end of stack trace]\n";
        assert_eq!(crash_tail(log), "[1013/152052:INFO:CONSOLE:782] \"[HMR] connected\" | Received signal 11 SEGV_MAPERR 000000000000 | [end of stack trace]");
    }

    /// Prova do modo página: fundo transparente chega ao quadro PNG do screencast com alfa 0.
    #[test]
    #[ignore = "sobe o Chromium da máquina"]
    fn page_mode_png_frames_keep_the_transparent_background() {
        let bin = launch::find().unwrap();
        let profile = std::env::temp_dir().join(format!("hangar-page-proof-{}", std::process::id()));
        let launched = launch::spawn(bin, &profile, Kind::Pages.scale()).unwrap();
        let wire = Wire::new(launched.writer, None);
        wire.asked.store(true, Ordering::SeqCst);
        let (events, _received) = async_channel::unbounded();
        let reading = wire.clone();
        std::thread::spawn(move || read(launched.reader, &reading, &events));
        let call = |session: Option<&str>, method: &str, params: Value| -> Value {
            let (tx, rx) = mpsc::channel();
            wire.send(session, method, params, Some(Box::new(move |r| { let _ = tx.send(r); })));
            rx.recv_timeout(Duration::from_secs(15)).unwrap().unwrap_or_else(|e| panic!("{method}: {e}"))
        };
        let context = call(None, "Target.createBrowserContext", json!({"disposeOnDetach": true}))["browserContextId"].as_str().unwrap().to_owned();
        let target = call(None, "Target.createTarget", json!({"url": "about:blank", "newWindow": true, "browserContextId": context}))["targetId"]
            .as_str().unwrap().to_owned();
        let session = call(None, "Target.attachToTarget", json!({"targetId": target, "flatten": true}))["sessionId"].as_str().unwrap().to_owned();
        let s = Some(session.as_str());
        let window = call(None, "Browser.getWindowForTarget", json!({"targetId": target}))["windowId"].as_i64().unwrap();
        call(s, "Page.enable", json!({}));
        call(s, "Emulation.setDefaultBackgroundColorOverride", json!({"color": {"r": 0, "g": 0, "b": 0, "a": 0}}));
        call(None, "Browser.setWindowBounds", json!({"windowId": window, "bounds": {"width": 400, "height": 300}}));
        let frame = call(s, "Page.getFrameTree", json!({}))["frameTree"]["frame"]["id"].as_str().unwrap().to_owned();
        let html = r#"<style>html,body{margin:0;background:transparent}</style>
            <div style="position:absolute;left:50px;top:50px;width:100px;height:100px;background:rgb(255,0,0)"></div>"#;
        call(s, "Page.setDocumentContent", json!({"frameId": frame, "html": html}));
        let decode = |data: &str| {
            let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).unwrap();
            image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap().into_rgba8()
        };
        let shot = decode(call(s, "Page.captureScreenshot", json!({"format": "png"}))["data"].as_str().unwrap());
        eprintln!("captureScreenshot {:?}: canto {:?}, caixa {:?}", shot.dimensions(), shot.get_pixel(5, 5), shot.get_pixel(100, 100));
        let (tx, rx) = mpsc::channel();
        lock(&wire.sinks).insert(session.clone(), Box::new(move |p: Value| { let _ = tx.send(p["data"].as_str().unwrap_or("").to_owned()); }));
        call(s, "Page.startScreencast", json!({"format": "png", "maxWidth": 400, "maxHeight": 300, "everyNthFrame": 1}));
        let pixels = decode(&rx.recv_timeout(Duration::from_secs(15)).unwrap());
        eprintln!("screencast {:?}: canto {:?}, caixa {:?}", pixels.dimensions(), pixels.get_pixel(5, 5), pixels.get_pixel(100, 100));
        drop(call(None, "Target.disposeBrowserContext", json!({"browserContextId": context})));
        wire.send(None, "Browser.close", json!({}), None);
        let _ = std::fs::remove_dir_all(&profile);
        assert_eq!(pixels.get_pixel(5, 5)[3], 0, "fundo deveria ser transparente");
        assert_eq!(pixels.get_pixel(100, 100).0, [255, 0, 0, 255], "caixa deveria ser opaca");
    }

    #[test]
    fn frames_with_an_owner_skip_the_interface_and_wait_for_the_owner_to_ack() {
        let (wire, reader) = wire();
        let (events, received) = async_channel::unbounded();
        let (tx, rx) = mpsc::channel();
        lock(&wire.sinks).insert("P".into(), Box::new(move |p: Value| tx.send(p["data"].clone()).unwrap()));
        route(json!({"method": "Page.screencastFrame", "sessionId": "P", "params": {"data": "QQ==", "sessionId": 9}}), &wire, &events);
        route(json!({"method": "Page.screencastFrame", "sessionId": "V", "params": {"data": "Qg==", "sessionId": 3}}), &wire, &events);
        assert_eq!(rx.recv().unwrap(), "QQ==");
        // O quadro de outra sessão (o espectador do celular) segue para a interface, sem ack daqui.
        assert_eq!(received.try_recv().unwrap()["sessionId"], "V");
        drop(wire);
        let mut all = Vec::new();
        std::io::Read::read_to_end(&mut { reader }, &mut all).unwrap();
        assert!(all.is_empty(), "a confirmação é de quem desenha, não do pipe");
    }
}
