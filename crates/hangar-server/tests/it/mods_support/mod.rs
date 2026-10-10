//! Apoio dos testes da interface dos mods: as conversas gravadas pela sonda, já limpas.
#![allow(dead_code)]

pub mod pane;

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

use hangar_server::mods::model::{ModsCall, ModsError, SurfaceEffect};
use hangar_server::mods::state::{CallFuture, Mods, ShownFuture, SurfaceLink, TerminalProbe};
use hangar_server::mods::surface::Surface;
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, RequestId, RuntimeTarget};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mods")
}

/// (direção, mensagem) de cada linha: `out` é o que a sonda mandou, `in` o que o Claude Code mandou.
pub fn fixture(name: &str) -> Vec<(String, Value)> {
    let raw = std::fs::read_to_string(fixtures().join(format!("{name}.jsonl"))).unwrap();
    raw.lines().map(|line| {
        let entry: Value = serde_json::from_str(line).unwrap();
        (entry["dir"].as_str().unwrap().to_owned(), entry["msg"].clone())
    }).collect()
}

/// A árvore da primeira resposta gravada ao desenho de `instance`.
pub fn first_render(name: &str, instance: &str) -> Value {
    let lines = fixture(name);
    let id = lines.iter().find(|(dir, msg)| dir == "out" && msg["request"]["subtype"] == "ui_render"
        && msg["request"]["instance_id"] == instance).map(|(_, msg)| msg["request_id"].clone()).unwrap();
    lines.iter().find(|(dir, msg)| dir == "in" && msg["type"] == "control_response" && msg["response"]["request_id"] == id)
        .map(|(_, msg)| msg["response"]["response"]["tree"].clone()).unwrap()
}

/// A árvore da `n`-ésima resposta (contando de 0) ao desenho de `instance`.
pub fn nth_render(name: &str, instance: &str, n: usize) -> Value {
    let lines = fixture(name);
    let ids: Vec<Value> = lines.iter().filter(|(dir, msg)| dir == "out" && msg["request"]["subtype"] == "ui_render"
        && msg["request"]["instance_id"] == instance).map(|(_, msg)| msg["request_id"].clone()).collect();
    let answers: Vec<Value> = ids.iter().filter_map(|id| lines.iter().find(|(dir, msg)| dir == "in"
        && msg["type"] == "control_response" && msg["response"]["request_id"] == *id)
        .map(|(_, msg)| msg["response"]["response"]["tree"].clone())).collect();
    answers.get(n).cloned()
        .unwrap_or_else(|| panic!("{name}: só há {} respostas ao desenho de {instance}, faltou a {n}", answers.len()))
}

/// Todo o texto dos filhos de uma árvore, na ordem do documento.
pub fn texts(node: &Value) -> String {
    match node {
        Value::String(text) => text.clone(),
        Value::Object(map) => map.get("children").and_then(Value::as_array)
            .map(|children| children.iter().map(texts).collect::<Vec<_>>().join("")).unwrap_or_default(),
        _ => String::new(),
    }
}

/// O primeiro quadro de um cano novo, sem processo saído nem pendência: o que os canos de teste mandam.
pub fn cano_snapshot_json() -> Value {
    serde_json::json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
        "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}})
}

/// Superfície que nunca responde: para os testes que só mexem no registro.
pub struct NoLink;
impl hangar_server::mods::state::SurfaceLink for NoLink {
    fn call(&self, _: hangar_server::mods::model::ModsCall, _: std::time::Instant) -> hangar_server::mods::state::CallFuture {
        Box::pin(async { Err(hangar_server::mods::model::missing()) })
    }
}

/// Elo de sessão com terminal de mentira: o painel que a tela mostraria, quantas leituras houve e se o
/// vigia parou. Os pedidos dos apps vão ao `NoLink`.
#[derive(Default)]
pub struct Probe { pub shown: Mutex<Option<String>>, pub reads: AtomicUsize, pub stopped: AtomicBool, pub anchor: Mutex<Option<String>> }
impl SurfaceLink for Probe {
    fn call(&self, call: ModsCall, deadline: std::time::Instant) -> CallFuture { SurfaceLink::call(&NoLink, call, deadline) }
}
impl TerminalProbe for Probe {
    fn read_shown(&self) -> ShownFuture {
        self.reads.fetch_add(1, SeqCst);
        let shown = self.shown.lock().unwrap().clone();
        Box::pin(async move { shown })
    }
    fn stop(&self) { self.stopped.store(true, SeqCst); }
    fn anchor(&self, anchor: Option<String>) { *self.anchor.lock().unwrap() = anchor; }
}

/// Pedidos que mudam o estado do mod; desenho e rol só leem o estado.
const ACTIONS: [&str; 7] = ["ui_attach", "ui_press", "ui_input", "ui_select", "ui_close", "ui_pane_show", "ui_pane_focus"];

/// Claude Code falso que responde com o que foi gravado. Uma ação casa com o primeiro pedido gravado
/// igual ainda não usado (de preferência depois da última ação) e devolve, na ordem gravada, os
/// avisos, o `ui_copy` e a resposta até o próximo pedido da sonda. Desenho e rol devolvem o estado
/// depois da última ação: a última resposta gravada antes da ação seguinte.
pub struct FakeClaude {
    lines: Vec<(String, Value)>,
    used: HashSet<usize>,
    cursor: usize,
}

impl FakeClaude {
    pub fn new(name: &str) -> Self {
        Self { lines: fixture(name), used: HashSet::new(), cursor: 0 }
    }

    fn is_request(&self, index: usize) -> bool {
        let (dir, msg) = &self.lines[index];
        dir == "out" && msg["type"] == "control_request"
    }

    fn same(recorded: &Value, wanted: &Value) -> bool {
        let (recorded, wanted) = (&recorded["request"], &wanted["request"]);
        if recorded["subtype"] != wanted["subtype"] { return false; }
        match wanted["subtype"].as_str().unwrap_or("") {
            "ui_render" => recorded["component"] == wanted["component"] && recorded["instance_id"] == wanted["instance_id"],
            "ui_press" => recorded["key"] == wanted["key"],
            "ui_select" => recorded["key"] == wanted["key"] && recorded["value"] == wanted["value"],
            "ui_input" => recorded["key"] == wanted["key"] && recorded["kind"] == wanted["kind"] && recorded["value"] == wanted["value"],
            "ui_close" | "ui_pane_show" | "ui_pane_focus" => recorded["id"] == wanted["id"],
            _ => true,
        }
    }

    fn next_action(&self) -> usize {
        (self.cursor + 1..self.lines.len()).find(|&i| self.is_request(i)
            && ACTIONS.contains(&self.lines[i].1["request"]["subtype"].as_str().unwrap_or(""))).unwrap_or(self.lines.len())
    }

    fn response(&self, index: usize, frame: &Value) -> Option<Value> {
        let recorded = &self.lines[index].1["request_id"];
        let (_, msg) = self.lines[index + 1..].iter().find(|(dir, msg)| dir == "in" && msg["type"] == "control_response"
            && msg["response"]["request_id"] == *recorded)?;
        let mut msg = msg.clone();
        msg["response"]["request_id"] = frame["request_id"].clone();
        Some(msg)
    }

    pub fn answer(&mut self, frame: &Value) -> Vec<Value> {
        let subtype = frame["request"]["subtype"].as_str().unwrap_or("").to_owned();
        if ACTIONS.contains(&subtype.as_str()) { return self.act(frame); }
        if subtype == "ui_panes" { return vec![self.roster(frame)]; }
        let end = self.next_action();
        let matching: Vec<usize> = (0..self.lines.len()).filter(|&i| self.is_request(i) && Self::same(&self.lines[i].1, frame)).collect();
        let chosen = matching.iter().rev().copied().find(|&i| i < end).or_else(|| matching.first().copied());
        chosen.and_then(|i| self.response(i, frame)).into_iter().collect()
    }

    fn act(&mut self, frame: &Value) -> Vec<Value> {
        let Some(index) = (0..self.lines.len()).filter(|&i| self.is_request(i) && !self.used.contains(&i)
            && Self::same(&self.lines[i].1, frame)).min_by_key(|&i| (i < self.cursor, i)) else { return Vec::new() };
        self.used.insert(index);
        self.cursor = index;
        let recorded = self.lines[index].1["request_id"].clone();
        let mut out = Vec::new();
        for (dir, msg) in &self.lines[index + 1..] {
            if dir == "out" && msg["type"] == "control_request" { break; }
            if dir != "in" { continue; }
            if msg["type"] == "control_response" {
                if msg["response"]["request_id"] == recorded {
                    let mut msg = msg.clone();
                    msg["response"]["request_id"] = frame["request_id"].clone();
                    out.push(msg);
                }
            } else {
                out.push(msg.clone());
            }
        }
        out
    }

    /// O rol na hora: o último `ui_panes` gravado (aviso ou resposta) antes da próxima ação.
    fn roster(&self, frame: &Value) -> Value {
        let end = self.next_action();
        let last = self.lines[..end].iter().rev().find_map(|(dir, msg)| {
            if dir != "in" { return None; }
            if msg["type"] == "system" && msg["subtype"] == "ui_panes" { return Some(msg.clone()); }
            if msg["type"] == "control_response" && msg["response"]["response"].get("panes").is_some() { return Some(msg["response"]["response"].clone()); }
            None
        }).unwrap_or_else(|| json!({"panes": []}));
        let body = json!({"panes": if last["panes"].is_array() { last["panes"].clone() } else { json!([]) },
            "shown_id": last["shown_id"], "focused_id": last["focused_id"], "focus_requested_id": last["focus_requested_id"]});
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": frame["request_id"], "response": body}})
    }
}

/// O último `plugin_ui` publicado entre os efeitos.
pub fn published(out: &[SurfaceEffect]) -> Option<Value> {
    out.iter().rev().find_map(|effect| match effect { SurfaceEffect::Publish { data } => Some(data.clone()), _ => None })
}

/// A resposta ao pedido `token` de um app, se já saiu.
pub fn reply_of(out: &[SurfaceEffect], token: u64) -> Option<Result<Value, ModsError>> {
    out.iter().find_map(|effect| match effect { SurfaceEffect::Reply { token: t, result } if *t == token => Some(result.clone()), _ => None })
}

/// Liga uma `Surface` ao Claude Code falso e guarda tudo o que ela emitiu.
pub struct Drive {
    pub surface: Surface,
    pub fake: FakeClaude,
    pub now: f64,
    pub out: Vec<SurfaceEffect>,
}

impl Drive {
    pub fn start(name: &str) -> Self {
        let mut drive = Self { surface: Surface::new("ui:t".into()), fake: FakeClaude::new(name), now: 10.0, out: Vec::new() };
        let out = drive.surface.start(drive.now);
        drive.feed(out);
        drive
    }

    pub fn feed(&mut self, effects: Vec<SurfaceEffect>) {
        let mut queue: VecDeque<SurfaceEffect> = effects.into();
        while let Some(effect) = queue.pop_front() {
            if let SurfaceEffect::Write { frame, .. } = &effect && frame["type"] == "control_request" {
                for line in self.fake.answer(frame) {
                    let next = self.line(&line);
                    queue.extend(next);
                }
            }
            self.out.push(effect);
        }
    }

    fn line(&mut self, line: &Value) -> Vec<SurfaceEffect> {
        match line["type"].as_str() {
            Some("control_response") => {
                let id: RequestId = serde_json::from_value(line["response"]["request_id"].clone()).unwrap();
                self.surface.on_response(&id, &line["response"], self.now)
            }
            Some("system") => self.surface.on_notice(line, self.now),
            Some("control_request") => self.surface.on_copy(&line["request_id"], &line["request"]),
            _ => Vec::new(),
        }
    }

    pub fn advance(&mut self, seconds: f64) {
        self.now += seconds;
        let out = self.surface.tick(self.now);
        self.feed(out);
    }

    pub fn call(&mut self, token: u64, call: ModsCall) {
        let out = self.surface.call(token, call, self.now, self.now + 7.0);
        self.feed(out);
    }

    pub fn view(&self) -> Value {
        published(&self.out).expect("a superfície publicou algo")
    }

    pub fn reply(&self, token: u64) -> Option<Result<Value, ModsError>> {
        reply_of(&self.out, token)
    }

    pub fn pane_text(&self, id: &str) -> String {
        let view = self.view();
        view["panes"].as_array().unwrap().iter().find(|pane| pane["id"] == id).map(|pane| texts(&pane["tree"])).unwrap_or_default()
    }
}

/// O prazo que a rota daria a um pedido que acabou de entrar.
pub fn budget() -> std::time::Instant { std::time::Instant::now() + std::time::Duration::from_millis(7500) }

/// Alvo de sessão Claude sem terminal. `initialized: false` deixa a superfície sem ligar: ela espera o
/// `initialize` dizer se o processo a aceita (A13).
pub fn claude_target(dir:&std::path::Path,escuta:String,initialized:bool) -> RuntimeTarget {
    RuntimeTarget { key:"key".into(),generation:1,name:"session".into(),provider:"claude".into(),
        metadata:json!({"name":"session","headless":true,"session_id":"sid-1","initialized":initialized}),
        binding:CanoBinding { pid:42,escuta,token:"secret-test".into(),versao:2 },
        lease_path:dir.join("key.lock"),state_path:dir.join("key.queue-state.json"),projection_dir:dir.join("projection"),
        transcript:dir.join("chat.jsonl"),created:0.0 }
}

/// (subtipo, id) de cada pedido `ui_*` que chegou ao cano.
pub type Seen = Arc<Mutex<Vec<(String,String)>>>;

/// Cano que responde os `ui_*` pela vitrine gravada, aceita várias conexões (reabertura) e anota
/// subtipo e id dos pedidos. `swallow` lista subtipos que ficam sem resposta.
pub async fn vitrine_cano(swallow:&'static [&'static str]) -> (String,Seen,tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let ids = Arc::new(Mutex::new(Vec::new()));
    let seen = ids.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream,_)) = listener.accept().await else { return };
            let seen = seen.clone();
            tokio::spawn(async move {
                let (read,mut write) = tokio::io::split(stream);
                let mut reader = BufReader::new(read);
                let mut raw = String::new();
                if reader.read_line(&mut raw).await.unwrap_or(0) == 0 || raw != "secret-test\n" { return; }
                let snapshot = cano_snapshot_json();
                write.write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
                let mut fake = FakeClaude::new("vitrine");
                loop {
                    raw.clear();
                    if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { return; }
                    let envelope:serde_json::Value = serde_json::from_str(&raw).unwrap();
                    let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"written"});
                    if write.write_all(format!("{ack}\n").as_bytes()).await.is_err() { return; }
                    let frame:serde_json::Value = serde_json::from_str(envelope["frame"].as_str().unwrap()).unwrap();
                    let subtype = frame["request"]["subtype"].as_str().unwrap_or("").to_owned();
                    if frame["type"] != "control_request" || !subtype.starts_with("ui_") { continue; }
                    seen.lock().unwrap().push((subtype.clone(),frame["request_id"].as_str().unwrap_or("").to_owned()));
                    if swallow.contains(&subtype.as_str()) { continue; }
                    for line in fake.answer(&frame) {
                        let out = json!({"type":"cano_output","frame":line.to_string()});
                        if write.write_all(format!("{out}\n").as_bytes()).await.is_err() { return; }
                    }
                }
            });
        }
    });
    (format!("tcp:{address}"),ids,task)
}

pub async fn wait_ui(mods:&Mods,check:impl Fn(&serde_json::Value)->bool) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            if let Some(ui) = mods.replay("session").into_iter().find(|(event,_)|*event == "plugin_ui")
                .map(|(_,data)|serde_json::from_str::<serde_json::Value>(&data).unwrap()).filter(|ui|check(ui)) { return ui; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("plugin_ui esperado")
}

/// Espera, com prazo, o cano receber um pedido do subtipo.
pub async fn wait_request(seen:&Seen,subtype:&str) {
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        while !seen.lock().unwrap().iter().any(|(kind,_)|kind == subtype) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap_or_else(|_|panic!("{subtype} esperado no cano"))
}

pub fn registry(mods:&Mods) -> RuntimeRegistry {
    // Política num endereço sem ninguém: só serviços cosméticos a usam aqui.
    RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into()).with_mods(mods.clone())
}
