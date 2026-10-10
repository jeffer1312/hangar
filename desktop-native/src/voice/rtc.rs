//! Chamada WebRTC direto com a OpenAI: o app-server só troca o SDP; o áudio não passa por ele.
use crate::voice::{log, audio::{Audio, AudioError, FRAME}};
use std::{net::{IpAddr, SocketAddr, UdpSocket}, sync::{Arc, Once, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig,
    change::{SdpAnswer, SdpPendingOffer}, format::Codec, media::{Direction, Frequency, MediaKind, MediaTime, Mid}, net::{Protocol, Receive}};

// UDP bloqueado não dá erro: só nunca conecta. Sem prazo a tela fica em "conectando" para sempre.
const CONNECT_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy)]
pub enum RtcError { Network, Answer, Media, Microphone, Speaker }

pub enum RtcEvent { Connected, Closed, Levels(f32, f32), Failed(RtcError) }

pub struct Offer { pub sdp: String, rtc: Rtc, socket: UdpSocket, local: SocketAddr, mid: Mid, pending: SdpPendingOffer }

static CRYPTO: Once = Once::new();

/// O IP que sai para a internet; o socket escuta em 0.0.0.0, mas o candidato precisa do endereço real.
fn route_ip() -> Option<IpAddr> {
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("8.8.8.8:80").ok()?;
    Some(probe.local_addr().ok()?.ip())
}

pub fn offer() -> Result<Offer, RtcError> {
    CRYPTO.call_once(|| str0m::crypto::from_feature_flags().install_process_default());
    let mut rtc = RtcConfig::new().build(Instant::now());
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|_| RtcError::Network)?;
    let ip = route_ip().ok_or(RtcError::Network)?;
    let local = SocketAddr::new(ip, socket.local_addr().map_err(|_| RtcError::Network)?.port());
    rtc.add_local_candidate(Candidate::host(local, "udp").map_err(|_| RtcError::Network)?).ok_or(RtcError::Network)?;
    let mut api = rtc.sdp_api();
    let mid = api.add_media(MediaKind::Audio, Direction::SendRecv, None, None, None);
    // O nome é o que o Realtime espera; sem o canal a OpenAI não abre a sessão.
    api.add_channel("oai-events".into());
    let (offer, pending) = api.apply().ok_or(RtcError::Media)?;
    Ok(Offer { sdp: offer.to_sdp_string(), rtc, socket, local, mid, pending })
}

// O canal de eventos precisa ser ilimitado: com send_blocking num canal cheio o laço de mídia travaria.
pub fn run(offer: Offer, answer: String, muted: Arc<AtomicBool>, events: async_channel::Sender<RtcEvent>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let outcome = drive(offer, answer, muted, &events, &stop);
        log(format!("rtc thread end outcome={outcome:?}"));
        let _ = events.send_blocking(match outcome { Ok(()) => RtcEvent::Closed, Err(error) => RtcEvent::Failed(error) });
    })
}

fn audio_error(error: AudioError) -> RtcError {
    match error { AudioError::Microphone => RtcError::Microphone, AudioError::Speaker => RtcError::Speaker }
}

/// Contadores da janela de 5 s do diário: dizem se o microfone sai e se o áudio da OpenAI chega.
#[derive(Default)]
struct Window { written: u32, write_errors: u32, encode_errors: u32, no_writer: u32, no_opus: u32,
    received: u32, decode_errors: u32, max_in: f32, max_out: f32,
    // RTP que chegou ao socket antes do str0m: separa "a OpenAI não mandou" de "o str0m descartou".
    rtp_raw: u32,
    // Maior intervalo entre pacotes (ms) no socket e na entrega do str0m: rajada da rede ou do laço.
    gap_raw_ms: u32, gap_media_ms: u32,
    // Volta mais longa do laço (ms): se bate com o intervalo no socket, quem segura é o app.
    loop_max_ms: u32,
    // Pela numeração e pelo relógio do RTP, o que o intervalo bruto mistura: pacotes que faltaram (perda na rede),
    // atraso da chegada além do tempo de áudio (oscilação da rede) e áudio que o servidor nem mandou (silêncio).
    lost: u32, late_max_ms: u32, skipped_max_ms: u32 }

/// Chegada de cada pacote comparada ao anterior: numeração, relógio do áudio e relógio da parede.
#[derive(Default)]
struct Arrival { last: Option<(u64, u64, Instant)> }

impl Arrival {
    /// `seq`: primeiro e último número do pacote; `media_us`: relógio do áudio dele.
    fn observe(&mut self, seq: (u64, u64), media_us: u64, now: Instant, window: &mut Window) {
        if let Some((prev_seq, prev_us, prev_at)) = self.last {
            window.lost += seq.0.saturating_sub(prev_seq + 1) as u32;
            let audio_ms = media_us.saturating_sub(prev_us) / 1000;
            let wall_ms = now.duration_since(prev_at).as_millis() as u64;
            window.late_max_ms = window.late_max_ms.max(wall_ms.saturating_sub(audio_ms) as u32);
            // Pacote seguido na numeração com salto no relógio: o servidor pulou áudio (silêncio), não a rede.
            if seq.0 == prev_seq + 1 { window.skipped_max_ms = window.skipped_max_ms.max(audio_ms.saturating_sub(20) as u32); }
        }
        // Pacote atrasado que chega depois de um mais novo não volta o relógio.
        if self.last.is_none_or(|(prev, ..)| seq.1 > prev) { self.last = Some((seq.1, media_us, now)); }
    }
}

/// RTP (não RTCP) pelo cabeçalho, que o SRTP deixa em claro.
fn is_rtp(datagram: &[u8]) -> bool {
    datagram.len() >= 12 && (128..=191).contains(&datagram[0]) && !(192..=223).contains(&datagram[1])
}

fn rtp_seq(datagram: &[u8]) -> u16 { u16::from_be_bytes([datagram[2], datagram[3]]) }

/// A OpenAI manda um pacote solto e reinicia o fluxo com o mesmo SSRC e sequência menor; o str0m
/// toma o solto como referência e descarta o resto como duplicado. Só libera após dois em sequência.
#[derive(Default)]
struct RtpStart { open: bool, held: Option<(u16, Vec<u8>, SocketAddr)> }

enum Admit { Pass, Hold, Release(Vec<u8>, SocketAddr) }

impl RtpStart {
    fn admit(&mut self, seq: u16, datagram: &[u8], source: SocketAddr) -> Admit {
        if self.open { return Admit::Pass; }
        match self.held.take() {
            Some((prev, bytes, from)) if seq == prev.wrapping_add(1) => { self.open = true; Admit::Release(bytes, from) }
            _ => { self.held = Some((seq, datagram.to_vec(), source)); Admit::Hold }
        }
    }
}

const SUMMARY_EVERY: Duration = Duration::from_secs(5);
const WRITE_DEADLINE: Duration = Duration::from_secs(5);

/// Detecção de fim de fala pedida ao Realtime ao abrir o canal: o padrão decidia cedo demais que a pessoa terminou.
const TURN_DETECTION: &str = r#"{"type":"semantic_vad","eagerness":"low"}"#;
/// Desligado: o Realtime v3 recusa `session.audio.input` e o erro dele encerra a chamada inteira.
const SEND_TURN_DETECTION: bool = false;

/// `session.update` parcial no formato v3 (`audio.input`, como o `audio.output.voice` que o Codex manda).
fn turn_detection_update() -> String {
    let detection: serde_json::Value = serde_json::from_str(TURN_DETECTION).unwrap_or_default();
    serde_json::json!({"type": "session.update", "session": {"audio": {"input": {"turn_detection": detection}}}}).to_string()
}

/// Só o campo `type` dos eventos do canal; o resto pode trazer fala transcrita.
fn event_type(data: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(data).ok()
        .and_then(|v| v["type"].as_str().map(str::to_owned)).unwrap_or_else(|| "?".into())
}

fn drive(offer: Offer, answer: String, muted: Arc<AtomicBool>, events: &async_channel::Sender<RtcEvent>, stop: &AtomicBool) -> Result<(), RtcError> {
    let Offer { mut rtc, socket, local, mid, pending, .. } = offer;
    let answer = SdpAnswer::from_sdp_string(&answer).map_err(|_| RtcError::Answer)?;
    rtc.sdp_api().accept_answer(pending, answer).map_err(|_| RtcError::Answer)?;
    log("rtc answer accepted");
    // Abrir o som aqui: o fluxo do cpal fica na thread que o usa, e a tela não espera o WASAPI.
    let mut audio = Audio::start(muted).map_err(audio_error)?;
    let (mut window, mut last_summary) = (Window::default(), Instant::now());
    let (mut last_type, mut repeats, mut usage_logged) = (String::new(), 0u32, 0u32);
    let (mut heard_user, mut delegated) = (false, false);
    let mut encoder = opus_rs::OpusEncoder::new(48_000, 1, opus_rs::Application::Voip).map_err(|_| RtcError::Media)?;
    let mut decoder = opus_rs::OpusDecoder::new(48_000, 1).map_err(|_| RtcError::Media)?;
    let (mut connected, mut timestamp, mut buffer) = (false, 0u64, vec![0u8; 2000]);
    let mut rtp_start = RtpStart::default();
    let (mut last_raw, mut last_media): (Option<Instant>, Option<Instant>) = (None, None);
    let mut arrival = Arrival::default();
    let (mut write_errors, mut last_written) = (0u32, Instant::now());
    let (mut decoded, mut packet) = (vec![0f32; FRAME * 2], vec![0u8; 1500]);
    let (started, mut last_levels) = (Instant::now(), Instant::now());
    let mut loop_top = Instant::now();
    // Enviado o `session.update`, o próximo `session.updated` ou erro diz se o servidor aceitou; recusa não derruba a chamada.
    let mut update_pending = false;
    let result = loop {
        let now = Instant::now();
        window.loop_max_ms = window.loop_max_ms.max(now.duration_since(loop_top).as_millis() as u32);
        loop_top = now;
        if stop.load(Ordering::Relaxed) { break Ok(()); }
        // Ninguém ouve mais: sem isto o microfone ficaria aberto.
        if events.is_closed() { break Err(RtcError::Network); }
        if !connected && started.elapsed() > CONNECT_DEADLINE { break Err(RtcError::Network); }
        if let Some(error) = audio.failed() { break Err(audio_error(error)); }
        let (input, output) = audio.levels();
        (window.max_in, window.max_out) = (window.max_in.max(input), window.max_out.max(output));
        if last_summary.elapsed() >= SUMMARY_EVERY {
            let w = std::mem::take(&mut window);
            let flow = audio.take_flow();
            log(format!("rtc summary connected={connected} written={} write_errors={} encode_errors={} no_writer={} no_opus={} received={} rtp_raw={} decode_errors={} max_in={:.4} raw_in_peak={:.4} max_out={:.4} capture_queue={} playback_queue={} underruns={} speech_underruns={} flow_in={} flow_played={} flow_dropped={} out_frames={} gap_raw_ms={} gap_media_ms={} loop_max_ms={} lost={} late_max_ms={} skipped_max_ms={}",
                w.written, w.write_errors, w.encode_errors, w.no_writer, w.no_opus, w.received, w.rtp_raw, w.decode_errors,
                w.max_in, audio.take_raw_peak(), w.max_out, audio.capture_len(), audio.playback_len(), audio.take_underruns(), audio.take_speech_underruns(),
                flow[0], flow[1], flow[2], flow[3], w.gap_raw_ms, w.gap_media_ms, w.loop_max_ms, w.lost, w.late_max_ms, w.skipped_max_ms));
            last_summary = Instant::now();
        }
        let timeout = match rtc.poll_output() {
            Err(_) => break Err(RtcError::Network),
            Ok(Output::Transmit(t)) => { let _ = socket.send_to(&t.contents, t.destination); continue; }
            Ok(Output::Event(event)) => {
                match event {
                    Event::Connected => { log("rtc connected"); connected = true; last_written = Instant::now(); audio.reset(); let _ = events.send_blocking(RtcEvent::Connected); }
                    Event::IceConnectionStateChange(state) => {
                        log(format!("rtc ice {state:?}"));
                        if state == IceConnectionState::Disconnected { break Err(RtcError::Network); }
                    }
                    Event::ChannelOpen(id, label) => {
                        log(format!("rtc channel open id={id:?} label={label}"));
                        if SEND_TURN_DETECTION && label == "oai-events" && let Some(mut channel) = rtc.channel(id) {
                            let update = turn_detection_update();
                            match channel.write(false, update.as_bytes()) {
                                Ok(sent) => { update_pending = sent; log(format!("rtc turn detection update sent={sent} bytes={}", update.len())); }
                                Err(error) => log(format!("rtc turn detection update write failed: {error:?}")),
                            }
                        }
                    }
                    Event::ChannelClose(id) => log(format!("rtc channel close id={id:?}")),
                    Event::ChannelData(data) => {
                        // Deltas chegam aos montes: repetição do mesmo tipo vira uma contagem.
                        let kind = event_type(&data.data);
                        // Formato desconhecido: só os números e o caminho das chaves vão ao diário, nunca texto.
                        if kind == "session.usage.updated" && usage_logged < 3 {
                            usage_logged += 1;
                            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data.data) {
                                let leaves: Vec<String> = numeric_leaves(&value).into_iter().map(|(path, n)| format!("{path}={n}")).collect();
                                log(format!("rtc usage {}", leaves.join(" ")));
                            }
                        }
                        // Pedido de ação que a voz respondeu sozinha some sem rastro: o turno sem delegação fica no diário.
                        if update_pending && (kind == "session.updated" || kind == "error" || kind.ends_with(".failed")) {
                            update_pending = false;
                            log(format!("rtc turn detection outcome={kind}"));
                        }
                        match kind.as_str() {
                            "input_transcript.added" => heard_user = true,
                            "delegation.created" => delegated = true,
                            "turn.done" => {
                                if heard_user && !delegated { log("realtime answered without delegation"); }
                                (heard_user, delegated) = (false, false);
                            }
                            _ => {}
                        }
                        if kind == last_type { repeats += 1; } else {
                            if repeats > 0 { log(format!("rtc channel event type={last_type} repeated={repeats}")); }
                            log(format!("rtc channel event type={kind} bytes={}", data.data.len()));
                            (last_type, repeats) = (kind, 0);
                        }
                    }
                    Event::MediaData(media) => {
                        window.received += 1;
                        let now = Instant::now();
                        if let Some(prev) = last_media { window.gap_media_ms = window.gap_media_ms.max(now.duration_since(prev).as_millis() as u32); }
                        last_media = Some(now);
                        arrival.observe((**media.seq_range.start(), **media.seq_range.end()), media.time.as_micros(), media.network_time, &mut window);
                        match decoder.decode(&media.data, FRAME, &mut decoded) {
                            Ok(n) => audio.play(&decoded[..n]),
                            Err(_) => window.decode_errors += 1,
                        }
                    }
                    _ => {}
                }
                continue;
            }
            Ok(Output::Timeout(t)) => t,
        };
        // Mídia escrita antes do Connected é descartada pelo str0m.
        if connected {
            let mut return_media_failure = false;
            while let Some(frame) = audio.next_frame() {
                let Ok(len) = encoder.encode(&frame, FRAME, &mut packet) else { window.encode_errors += 1; continue };
                let Some(writer) = rtc.writer(mid) else { window.no_writer += 1; break };
                let Some(pt) = writer.payload_params().find(|p| p.spec().codec == Codec::Opus).map(|p| p.pt()) else { window.no_opus += 1; break };
                // Falha persistente de escrita deixaria a chamada conectada com o microfone mudo para a OpenAI.
                match writer.write(pt, Instant::now(), MediaTime::new(timestamp, Frequency::FORTY_EIGHT_KHZ), packet[..len].to_vec()) {
                    Ok(_) => { write_errors = 0; window.written += 1; last_written = Instant::now(); }
                    Err(_) => { write_errors += 1; window.write_errors += 1; if write_errors >= 50 { return_media_failure = true; break; } }
                }
                timestamp += FRAME as u64;
            }
            if return_media_failure { break Err(RtcError::Media); }
            // Mudo também escreve quadros (silêncio): sem escrita por 5 s o microfone, o codec ou o writer pararam.
            if last_written.elapsed() > WRITE_DEADLINE {
                log(format!("rtc media stalled: no frame written for 5s encode_errors={} no_writer={} no_opus={}", window.encode_errors, window.no_writer, window.no_opus));
                break Err(RtcError::Media);
            }
            if last_levels.elapsed() >= Duration::from_millis(60) {
                let (input, output) = audio.levels();
                let _ = events.try_send(RtcEvent::Levels(input, output));
                last_levels = Instant::now();
            }
        }
        // Relógio vencido não pode pular a leitura: enviando áudio a cada volta, o socket nunca era lido
        // e a voz da OpenAI se perdia no buffer. Acorda no mínimo a cada 10 ms para drenar o microfone.
        let now = Instant::now();
        if timeout <= now && rtc.handle_input(Input::Timeout(now)).is_err() { break Err(RtcError::Network); }
        let wait = timeout.saturating_duration_since(now).clamp(Duration::from_millis(1), Duration::from_millis(10));
        let _ = socket.set_read_timeout(Some(wait));
        match socket.recv_from(&mut buffer) {
            Ok((n, source)) => {
                if is_rtp(&buffer[..n]) {
                    window.rtp_raw += 1;
                    let now = Instant::now();
                    if let Some(prev) = last_raw { window.gap_raw_ms = window.gap_raw_ms.max(now.duration_since(prev).as_millis() as u32); }
                    last_raw = Some(now);
                    match rtp_start.admit(rtp_seq(&buffer[..n]), &buffer[..n], source) {
                        Admit::Hold => continue,
                        Admit::Pass => {}
                        Admit::Release(held, from) => {
                            log("rtc rtp stream open");
                            let Ok(contents) = held.as_slice().try_into() else { continue };
                            if rtc.handle_input(Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source: from, destination: local, contents })).is_err() { break Err(RtcError::Network); }
                        }
                    }
                }
                let Ok(contents) = buffer[..n].try_into() else { continue };
                if rtc.handle_input(Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: local, contents })).is_err() { break Err(RtcError::Network); }
            }
            Err(_) => { if rtc.handle_input(Input::Timeout(Instant::now())).is_err() { break Err(RtcError::Network); } }
        }
    };
    rtc.disconnect();
    result
}

/// Só números, com o caminho da chave (`response.usage.input_tokens`); nenhum texto sai daqui.
fn numeric_leaves(value: &serde_json::Value) -> Vec<(String, String)> {
    use serde_json::Value;
    fn walk(value: &Value, path: &str, out: &mut Vec<(String, String)>) {
        if out.len() >= 80 { return; }
        match value {
            Value::Number(n) => out.push((path.to_owned(), n.to_string())),
            Value::Object(map) => for (key, v) in map { walk(v, &if path.is_empty() { key.clone() } else { format!("{path}.{key}") }, out); },
            Value::Array(items) => for (i, v) in items.iter().enumerate() { walk(v, &format!("{path}[{i}]"), out); },
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn arrival_separates_loss_lateness_and_skipped_silence() {
        let (mut arrival, mut window, t0) = (Arrival::default(), Window::default(), Instant::now());
        let ms = |n: u64| t0 + Duration::from_millis(n);
        arrival.observe((1, 1), 0, ms(0), &mut window);
        arrival.observe((2, 2), 20_000, ms(20), &mut window);
        assert_eq!((window.lost, window.late_max_ms, window.skipped_max_ms), (0, 0, 0), "no ritmo");
        arrival.observe((3, 3), 40_000, ms(200), &mut window);
        assert_eq!(window.late_max_ms, 160, "20 ms de áudio levaram 180 ms: atraso da rede");
        arrival.observe((6, 6), 100_000, ms(260), &mut window);
        assert_eq!(window.lost, 2, "4 e 5 faltaram");
        arrival.observe((7, 7), 500_000, ms(660), &mut window);
        assert_eq!(window.skipped_max_ms, 380, "numeração seguida, relógio pulou: silêncio que o servidor não mandou");
        assert_eq!(window.late_max_ms, 160, "o salto chegou no tempo dele");
        arrival.observe((5, 5), 80_000, ms(670), &mut window);
        assert_eq!(window.lost, 2, "atrasado fora de ordem não volta o relógio nem conta perda de novo");
    }

    #[test]
    fn stray_first_packet_is_dropped_and_stream_opens_on_consecutive_pair() {
        let from: SocketAddr = "1.2.3.4:5".parse().unwrap();
        let mut start = RtpStart::default();
        // Medido: pacote solto seq 29604, depois o fluxo recomeça em 18768.
        assert!(matches!(start.admit(29604, b"stray", from), Admit::Hold));
        assert!(matches!(start.admit(18768, b"first", from), Admit::Hold));
        assert!(matches!(start.admit(18769, b"second", from), Admit::Release(held, _) if held == b"first"));
        assert!(matches!(start.admit(18770, b"third", from), Admit::Pass));
        let mut wrap = RtpStart::default();
        assert!(matches!(wrap.admit(u16::MAX, b"a", from), Admit::Hold));
        assert!(matches!(wrap.admit(0, b"b", from), Admit::Release(..)));
    }

    #[test]
    fn offer_has_opus_audio_and_events_channel() {
        // Máquina sem rota de rede não monta a oferta.
        let Ok(offer) = offer() else { return };
        assert!(offer.sdp.starts_with("v=0"));
        assert!(offer.sdp.contains("m=audio"));
        assert!(offer.sdp.to_lowercase().contains("opus/48000"));
        assert!(offer.sdp.contains("webrtc-datachannel"));
    }

    #[test]
    fn turn_detection_update_is_v3_partial_session_update() {
        let update: serde_json::Value = serde_json::from_str(&turn_detection_update()).unwrap();
        assert_eq!(update["type"], "session.update");
        assert_eq!(update["session"]["audio"]["input"]["turn_detection"], serde_json::json!({"type": "semantic_vad", "eagerness": "low"}));
        assert_eq!(update["session"].as_object().unwrap().len(), 1, "parcial: não mexe em instruções nem voz");
    }

    #[test]
    fn numeric_leaves_keep_paths_and_drop_strings() {
        let event = serde_json::json!({"type": "session.usage.updated", "response": {"usage": {"input_tokens": 123, "details": [{"cached": 4.5}], "model": "gpt"}}, "ok": true});
        let mut leaves = numeric_leaves(&event);
        leaves.sort();
        assert_eq!(leaves, vec![("response.usage.details[0].cached".to_owned(), "4.5".to_owned()), ("response.usage.input_tokens".to_owned(), "123".to_owned())]);
    }

    #[test]
    fn event_type_reads_only_the_type() {
        assert_eq!(event_type(br#"{"type":"session.created","transcript":"oi"}"#), "session.created");
        assert_eq!(event_type(b"not json"), "?");
    }
}
