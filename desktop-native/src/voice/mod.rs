//! Conversa por voz: o servidor conectado conduz a chamada (organizador, regras, sessões). Aqui ficam o áudio, as
//! ferramentas de tela e o estado que ele publica.
pub mod audio;
pub mod rtc;

use crate::{api::Api, ws::{self, TextEvent, TextSocket}};
use serde_json::{Value, json};
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tokio::{runtime::Handle, sync::{Notify, mpsc}};

pub enum Phase { Connecting, Live, Closed }

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Mode { #[default] Direct, Plan }

pub enum VoiceEvent {
    Phase(Phase), Levels(f32, f32),
    /// Retrato que o servidor publica (`phase`, `mode`, `plan`, bastidores…).
    State(Value),
    /// Ferramenta de tela; a resposta volta por `Voice::reply`.
    Tool { call: u64, name: String, args: Value },
    /// Outro aparelho assumiu a chamada.
    Taken,
    /// Código da falha que encerrou a chamada.
    Failed(String),
}

enum Command { Screen(Option<(String, String)>), Reply(u64, bool, String), Mode(Mode) }

pub struct Voice { commands: mpsc::UnboundedSender<Command>, muted: Arc<AtomicBool>, stop: Arc<Notify> }

/// O servidor derruba o aparelho calado por 30 s.
const PING_EVERY: Duration = Duration::from_secs(10);
const PONG_WITHIN: Duration = Duration::from_secs(30);
const LEVEL_EVERY: Duration = Duration::from_millis(100);
/// Quanto o `stop` espera o servidor fechar antes de largar o socket.
const STOP_WAIT: Duration = Duration::from_secs(2);

const CAPS: [&str; 5] = ["switch_session", "hangar_actions", "hangar_action", "read_screen", "click_screen"];

impl Voice {
    /// `token`: credencial da conexão de `api`. `screen`: (chave da máquina, nome) da sessão na tela. `actions`: catálogo
    /// das ações de tela que o Jev do servidor oferece.
    pub fn start(runtime: &Handle, api: &Api, token: String, screen: Option<(String, String)>, actions: Value,
        events: async_channel::Sender<VoiceEvent>) -> Voice {
        let (commands, inbox) = mpsc::unbounded_channel();
        let (muted, stop) = (Arc::new(AtomicBool::new(false)), Arc::new(Notify::new()));
        let setup = Setup { api: api.clone(), token, screen, actions };
        runtime.spawn(call(setup, events, inbox, muted.clone(), stop.clone()));
        Voice { commands, muted, stop }
    }
    pub fn set_muted(&self, muted: bool) { self.muted.store(muted, Ordering::Relaxed); }
    pub fn screen(&self, screen: Option<(String, String)>) { let _ = self.commands.send(Command::Screen(screen)); }
    pub fn reply(&self, call: u64, ok: bool, text: String) { let _ = self.commands.send(Command::Reply(call, ok, text)); }
    pub fn set_mode(&self, mode: Mode) { let _ = self.commands.send(Command::Mode(mode)); }
    // notify_one guarda a licença mesmo sem ninguém esperando ainda.
    pub fn stop(&mut self) { self.stop.notify_one(); }
}

impl Drop for Voice { fn drop(&mut self) { self.stop(); } }

/// Diário da voz. Nunca recebe fala, transcrição, texto de pedido nem argumentos de ferramenta.
pub(crate) fn log(text: impl AsRef<str>) { crate::log_line(&format!("voice: {}", text.as_ref())); }

struct Setup { api: Api, token: String, screen: Option<(String, String)>, actions: Value }

pub(crate) fn host_of(address: &str) -> Option<String> { url::Url::parse(address.trim()).ok()?.host_str().map(str::to_ascii_lowercase) }

/// Como o servidor da voz chama a máquina `key`: `""` ele mesmo, o id do peer de mesmo host, ou `None` (ele não a conhece).
fn screen_server(key: &str, own: &str, peers: &[(String, String)]) -> Option<String> {
    let host = host_of(key)?;
    if host_of(own).as_deref() == Some(host.as_str()) { return Some(String::new()); }
    peers.iter().find(|(_, base)| host_of(base).as_deref() == Some(host.as_str())).map(|(id, _)| id.clone())
}

fn screen_json(screen: Option<&(String, String)>, own: &str, peers: &[(String, String)]) -> Value {
    screen.and_then(|(key, name)| screen_server(key, own, peers).map(|server| json!({"server": server, "name": name}))).unwrap_or(Value::Null)
}

/// `(id, base_url)` dos peers do servidor da voz; falha deixa a sessão de outra máquina fora da tela, nunca a chamada.
async fn read_peers(api: &Api) -> Vec<(String, String)> {
    match api.server_read(&["peers"], &[], 5).await {
        Ok(list) => list.as_array().into_iter().flatten()
            .filter_map(|p| Some((p["id"].as_str()?.to_owned(), p["base_url"].as_str()?.to_owned()))).collect(),
        Err(error) => { log(format!("peers read failed status={:?}", error.status)); Vec::new() }
    }
}

fn socket_code(error: &ws::Error) -> &'static str {
    match error { ws::Error::Http(_) | ws::Error::Handshake => "refused", _ => "connection_lost" }
}

fn rtc_code(error: rtc::RtcError) -> &'static str {
    // Mídia parada é microfone trocado/desconectado ou codec, não rede: o texto de rede mandava olhar o firewall.
    match error { rtc::RtcError::Microphone => "microphone", rtc::RtcError::Speaker => "speaker", rtc::RtcError::Media => "audio_stopped", _ => "network" }
}

fn send(socket: &TextSocket, msg: Value) -> Result<(), String> {
    socket.send(&msg.to_string()).map_err(|error| { log(format!("socket send failed error={error:?}")); socket_code(&error).to_owned() })
}

async fn call(setup: Setup, events: async_channel::Sender<VoiceEvent>, mut inbox: mpsc::UnboundedReceiver<Command>, muted: Arc<AtomicBool>,
    stop: Arc<Notify>) {
    log("call start");
    let _ = events.send(VoiceEvent::Phase(Phase::Connecting)).await;
    let stopped = Arc::new(AtomicBool::new(false));
    let (mut socket, mut server_done, mut peer) = (None, false, None);
    let outcome = tokio::select! {
        outcome = run_call(setup, &events, &mut inbox, &muted, &stopped, &mut socket, &mut server_done, &mut peer) => outcome,
        _ = stop.notified() => { log("stop requested"); Ok(()) },
    };
    // Fim do lado de cá: o servidor encerra já, em vez de esperar o prazo do aparelho sumido.
    if let Some(socket) = socket.as_mut() {
        if !server_done && send(socket, json!({"type": "stop"})).is_ok() {
            let incoming = socket.events();
            let _ = tokio::time::timeout(STOP_WAIT, async {
                while let Ok(Ok(TextEvent::Text(_) | TextEvent::Connected)) = incoming.recv().await {}
            }).await;
        }
        socket.close();
    }
    stopped.store(true, Ordering::Relaxed);
    if let Some(peer) = peer && !matches!(tokio::task::spawn_blocking(move || peer.join()).await, Ok(Ok(()))) { log("rtc thread join failed (panic)"); }
    match &outcome { Ok(()) => log("call end ok"), Err(code) => log(format!("call end failure={code}")) }
    if let Err(code) = outcome { let _ = events.send(VoiceEvent::Failed(code)).await; }
    let _ = events.send(VoiceEvent::Phase(Phase::Closed)).await;
}

#[allow(clippy::too_many_arguments)]
async fn run_call(setup: Setup, events: &async_channel::Sender<VoiceEvent>, inbox: &mut mpsc::UnboundedReceiver<Command>, muted: &Arc<AtomicBool>,
    stopped: &Arc<AtomicBool>, slot: &mut Option<TextSocket>, server_done: &mut bool, peer: &mut Option<std::thread::JoinHandle<()>>) -> Result<(), String> {
    let Setup { api, token, screen, actions } = setup;
    let own = api.identity();
    let peers = read_peers(&api).await;
    let socket = slot.insert(TextSocket::open(&Handle::current(), &api, "/api/voice", token));
    let incoming = socket.events();
    match incoming.recv().await {
        Ok(Ok(TextEvent::Connected)) => log("socket connected"),
        Ok(Err(error)) => { log(format!("socket open failed error={error:?}")); return Err(socket_code(&error).to_owned()); }
        _ => return Err("connection_lost".to_owned()),
    }
    send(socket, json!({"type": "hello", "client": "native", "screen": screen_json(screen.as_ref(), &own, &peers), "caps": CAPS, "actions": actions}))?;
    let offer = tokio::task::spawn_blocking(rtc::offer).await.map_err(|_| "network".to_owned())?.map_err(|error| rtc_code(error).to_owned())?;
    send(socket, json!({"type": "offer", "sdp": offer.sdp.clone()}))?;
    log(format!("offer sent bytes={}", offer.sdp.len()));
    let mut offer = Some(offer);
    let (rtc_tx, rtc_rx) = async_channel::unbounded();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    let mut last_pong = Instant::now();
    let mut last_level: Option<Instant> = None;
    loop {
        tokio::select! {
            message = incoming.recv() => match message {
                Ok(Ok(TextEvent::Text(text))) => {
                    let Ok(message) = serde_json::from_str::<Value>(&text) else { log("server message unreadable"); continue };
                    match message["type"].as_str().unwrap_or_default() {
                        "answer" => match offer.take() {
                            Some(offer) => {
                                let answer = message["sdp"].as_str().unwrap_or_default().to_owned();
                                log(format!("answer received bytes={}", answer.len()));
                                *peer = Some(rtc::run(offer, answer, muted.clone(), rtc_tx.clone(), stopped.clone()));
                            }
                            None => log("answer repeated ignored"),
                        },
                        "state" => { let _ = events.send(VoiceEvent::State(message["state"].clone())).await; }
                        "tool" => {
                            let (Some(call), Some(name)) = (message["call"].as_u64(), message["name"].as_str()) else { log("tool unreadable"); continue };
                            log(format!("tool {name} call={call}"));
                            let _ = events.send(VoiceEvent::Tool { call, name: name.to_owned(), args: message["args"].clone() }).await;
                        }
                        "pong" => last_pong = Instant::now(),
                        "taken" => { log("taken by another device"); *server_done = true; let _ = events.send(VoiceEvent::Taken).await; return Ok(()); }
                        "error" => { *server_done = true; return Err(message["code"].as_str().unwrap_or("failed").to_owned()); }
                        "closed" => { log("closed by server"); *server_done = true; return Ok(()); }
                        other => log(format!("server message unknown type={other}")),
                    }
                }
                Ok(Ok(TextEvent::Connected)) => {}
                Ok(Ok(TextEvent::Closed)) | Err(_) => { *server_done = true; return Err("connection_lost".to_owned()); }
                Ok(Err(error)) => { log(format!("socket failed error={error:?}")); *server_done = true; return Err(socket_code(&error).to_owned()); }
            },
            event = rtc_rx.recv() => match event {
                Ok(rtc::RtcEvent::Connected) => {
                    log("phase Live");
                    send(socket, json!({"type": "live"}))?;
                    let _ = events.send(VoiceEvent::Phase(Phase::Live)).await;
                }
                Ok(rtc::RtcEvent::Levels(input, output)) => {
                    let _ = events.try_send(VoiceEvent::Levels(input, output));
                    if last_level.is_none_or(|at| at.elapsed() >= LEVEL_EVERY) {
                        last_level = Some(Instant::now());
                        send(socket, json!({"type": "level", "input": input}))?;
                    }
                }
                Ok(rtc::RtcEvent::Failed(error)) => { log(format!("rtc failed error={error:?}")); return Err(rtc_code(error).to_owned()); }
                Ok(rtc::RtcEvent::Closed) => { log("rtc closed"); return Ok(()); }
                // A thread sempre manda o último evento antes de sair; canal fechado sem ele é thread morta.
                Err(_) => return Err("network".to_owned()),
            },
            command = inbox.recv() => match command {
                Some(Command::Screen(screen)) => send(socket, json!({"type": "screen", "screen": screen_json(screen.as_ref(), &own, &peers)}))?,
                Some(Command::Reply(call, ok, text)) => send(socket, json!({"type": "tool_result", "call": call, "ok": ok, "text": text}))?,
                Some(Command::Mode(mode)) => send(socket, json!({"type": "mode", "mode": match mode { Mode::Direct => "direct", Mode::Plan => "plan" }}))?,
                None => return Ok(()),
            },
            _ = ping.tick() => {
                if last_pong.elapsed() >= PONG_WITHIN { log("pong missing"); return Err("timeout".to_owned()); }
                send(socket, json!({"type": "ping"}))?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn screen_server_names_the_voice_server_by_its_own_words() {
        let peers = [("pc".to_owned(), "https://jefferson-felizardo.tailnet.ts.net".to_owned())];
        let own = "http://127.0.0.1:8765/";
        assert_eq!(screen_server("http://127.0.0.1:8765", own, &peers), Some(String::new()), "a própria máquina é vazio");
        assert_eq!(screen_server("https://jefferson-felizardo.tailnet.ts.net:8443", own, &peers), Some("pc".into()), "casa pelo host");
        assert_eq!(screen_server("https://vps.example.com", own, &peers), None, "máquina que o servidor não conhece");
        assert_eq!(screen_json(None, own, &peers), Value::Null);
        assert_eq!(screen_json(Some(&("https://vps.example.com".into(), "x".into())), own, &peers), Value::Null);
        assert_eq!(screen_json(Some(&("http://127.0.0.1:8765".into(), "hangar".into())), own, &peers), json!({"server": "", "name": "hangar"}));
    }
}
