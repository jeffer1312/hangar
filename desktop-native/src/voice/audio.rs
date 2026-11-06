//! Microfone e alto-falante da voz: `cpal` nas pontas, `sonora` (AEC3) no meio, 48 kHz mono para o Opus.
use cpal::Sample as _;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sonora::{AudioProcessing, Config, StreamConfig, config::{AdaptiveDigital, EchoCanceller, GainController2}};
use std::{collections::VecDeque, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering}}};

pub const RATE: u32 = 48_000;
pub const FRAME: usize = 960;
// ponytail: atraso fixo entre o que toca e o que o microfone ouve; o AEC3 refina sozinho. Ajustar se sobrar eco.
const STREAM_DELAY_MS: i32 = 60;
// Dois segundos de folga: mais que isso é atraso acumulado, e tocar atrasado é pior que pular.
const MAX_QUEUE: usize = RATE as usize * 2;
// Teto da fala guardada na pausa (cerca de 115 MB em f32). Só fala conta: o servidor não manda o silêncio entre as falas.
const HOLD_MAX: usize = RATE as usize * 600;
// A fila de reprodução tem teto: a fala guardada desce aos poucos para não ser descartada por ele.
const REFILL_BELOW: usize = RATE as usize;

#[derive(Debug, Clone, Copy)]
pub enum AudioError { Microphone, Speaker }

pub(crate) fn downmix(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 { return data.to_vec(); }
    data.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect()
}

pub(crate) fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() { return 0.0; }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

pub(crate) fn apply_mute(frame: &mut [f32], muted: bool) { if muted { frame.fill(0.0); } }

/// Pausa da fala da assistente. O modelo segue gerando em tempo real e não há como pará-lo, então o que chega fica guardado
/// aqui e toca depois, atrás do que faltava: a demora só some quando a voz fica calada, porque silêncio não é enviado.
#[derive(Default)]
pub struct SpeechPause { paused: AtomicBool, held: Mutex<VecDeque<f32>>, dropped: AtomicU32, alert: AtomicBool }

impl SpeechPause {
    pub fn set(&self, paused: bool) { self.paused.store(paused, Ordering::Relaxed); }
    /// Pede o som de travamento; quem toca é o laço da chamada, dono do áudio.
    pub fn request_alert(&self) { self.alert.store(true, Ordering::Relaxed); }
    fn is_paused(&self) -> bool { self.paused.load(Ordering::Relaxed) }

    /// `true` se ficou guardado: com fala guardada, a nova não pode passar na frente da antiga.
    fn hold(&self, samples: &[f32]) -> bool {
        let mut held = self.held.lock().unwrap();
        if !self.is_paused() && held.is_empty() { return false; }
        let room = HOLD_MAX.saturating_sub(held.len()).min(samples.len());
        self.dropped.fetch_add((samples.len() - room) as u32, Ordering::Relaxed);
        held.extend(&samples[..room]);
        true
    }

    fn release(&self, queued: usize) -> Vec<f32> {
        if self.is_paused() || queued >= REFILL_BELOW { return Vec::new(); }
        let mut held = self.held.lock().unwrap();
        let take = held.len().min(REFILL_BELOW - queued);
        held.drain(..take).collect()
    }

    /// Volta para a frente o que estava na fila de reprodução, para a fala sobreviver à troca da conexão de áudio.
    fn keep_front(&self, samples: VecDeque<f32>) {
        let mut held = self.held.lock().unwrap();
        for sample in samples.into_iter().rev() { held.push_front(sample); }
    }

    fn held_len(&self) -> usize { self.held.lock().unwrap().len() }
    fn take_dropped(&self) -> u32 { self.dropped.swap(0, Ordering::Relaxed) }
}

/// Com a fala pausada o microfone também fecha: outra pessoa falando perto interromperia a resposta no servidor.
fn mic_closed(muted: &AtomicBool, pause: &SpeechPause) -> bool { muted.load(Ordering::Relaxed) || pause.is_paused() }

/// Sinal de que a chamada conectou: duas notas subindo.
pub(crate) fn connected_tone() -> Vec<f32> { two_notes(660.0, 880.0) }

/// Sinal de que a resposta travou: as mesmas notas descendo.
pub(crate) fn stalled_tone() -> Vec<f32> { two_notes(880.0, 660.0) }

/// Passa da reserva inicial, que só toca o que a alcança, e termina em silêncio.
fn two_notes(first: f32, second: f32) -> Vec<f32> {
    const NOTE: usize = RATE as usize * 110 / 1000;
    const GAP: usize = RATE as usize * 40 / 1000;
    const TOTAL: usize = RATE as usize * 320 / 1000;
    const FADE: usize = RATE as usize * 8 / 1000;
    let note = |hz: f32| (0..NOTE).map(move |i| {
        let fade = (i.min(NOTE - 1 - i) as f32 / FADE as f32).min(1.0);
        0.18 * fade * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin()
    });
    let mut tone: Vec<f32> = note(first).collect();
    tone.resize(NOTE + GAP, 0.0);
    tone.extend(note(second));
    tone.resize(TOTAL, 0.0);
    tone
}

pub(crate) struct Resampler { step: f64, position: f64, last: Option<f32> }

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self { Self { step: from as f64 / to as f64, position: 0.0, last: None } }
    /// Interpolação linear; `position` e `last` atravessam as chamadas para não estalar na emenda dos blocos.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() { return; }
        if (self.step - 1.0).abs() < f64::EPSILON { out.extend_from_slice(input); return; }
        // Primeiro bloco: sem amostra anterior, repete a primeira em vez de interpolar a partir do silêncio.
        let last = self.last.unwrap_or(input[0]);
        let at = |i: isize| if i < 0 { last } else { input[i as usize] };
        while self.position < input.len() as f64 {
            let index = self.position.floor() as isize - 1;
            let fraction = (self.position - self.position.floor()) as f32;
            let (a, b) = (at(index), at(index + 1));
            out.push(a + (b - a) * fraction);
            self.position += self.step;
        }
        self.position -= input.len() as f64;
        self.last = input.last().copied();
    }
}

fn bounded_extend(queue: &mut VecDeque<f32>, samples: impl IntoIterator<Item = f32>, cap: usize) {
    queue.extend(samples);
    let excess = queue.len().saturating_sub(cap);
    queue.drain(..excess);
}

/// Só aparelho que sumiu ou fluxo invalidado derruba a chamada; estouro de buffer e troca de aparelho o fluxo supera.
fn is_fatal(error: &cpal::Error) -> bool {
    matches!(error.kind(), cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated)
}

struct Shared {
    // Microfone parado não pode virar áudio velho depois: 200 ms na taxa do aparelho.
    capture_cap: usize,
    capture: Mutex<VecDeque<f32>>,
    render: Mutex<VecDeque<f32>>,
    playback: Mutex<VecDeque<f32>>,
    input_level: AtomicU32,
    output_level: AtomicU32,
    // Pico do microfone antes do AEC: separa "o microfone não capta" de "o AEC apagou a voz".
    raw_peak: AtomicU32,
    // 0 = ok, 1 = microfone caiu, 2 = saída caiu
    failed: AtomicU8,
    // Vezes que a saída esvaziou no meio da fala: mede o "travando e pulando".
    underruns: AtomicU32,
    // Das esvaziadas, as que cortaram áudio audível (o bloco anterior tinha voz); as outras são o fim de uma fala.
    speech_underruns: AtomicU32,
    // Reserva atual em amostras; cresce com os cortes audíveis (`grown_prime`).
    prime: AtomicU32,
    // Fluxo da reprodução no diário: amostras que entraram, que tocaram, descartadas pelo teto e quadros pedidos pela saída.
    flow: [AtomicU32; 4],
    pause: Arc<SpeechPause>,
}

impl Shared {
    fn new(capture_cap: usize, pause: Arc<SpeechPause>) -> Self {
        Self { capture_cap, capture: Default::default(), render: Default::default(), playback: Default::default(),
            input_level: AtomicU32::new(0), output_level: AtomicU32::new(0), raw_peak: AtomicU32::new(0), failed: AtomicU8::new(0), underruns: AtomicU32::new(0),
            speech_underruns: AtomicU32::new(0), prime: AtomicU32::new(PRIME_SAMPLES as u32), flow: Default::default(), pause }
    }

    fn enqueue(&self, samples: &[f32]) {
        let mut queue = self.playback.lock().unwrap();
        let dropped = (queue.len() + samples.len()).saturating_sub(MAX_QUEUE);
        self.flow[0].fetch_add(samples.len() as u32, Ordering::Relaxed);
        self.flow[2].fetch_add(dropped as u32, Ordering::Relaxed);
        bounded_extend(&mut queue, samples.iter().copied(), MAX_QUEUE);
    }

    fn play(&self, samples: &[f32]) { if !self.pause.hold(samples) { self.enqueue(samples); } }

    fn refill(&self) {
        if self.pause.alert.swap(false, Ordering::Relaxed) { self.enqueue(&stalled_tone()); }
        let queued = self.playback.lock().unwrap().len();
        let samples = self.pause.release(queued);
        if !samples.is_empty() { self.enqueue(&samples); }
    }

    fn keep_unplayed(&self) {
        if self.pause.is_paused() { self.pause.keep_front(std::mem::take(&mut *self.playback.lock().unwrap())); }
    }
}

// Reserva antes de tocar: o áudio chega em rajadas pela rede, e tocar no ato esvazia a fila a cada atraso.
const PRIME_SAMPLES: usize = RATE as usize * 240 / 1000;
// Rede que atrasa mais que a reserva corta a fala: cada corte audível aumenta a reserva até o teto, pelo resto da chamada.
// Quem tem rede boa fica nos 240 ms; só quem engasgou paga a espera maior.
const PRIME_STEP: usize = RATE as usize * 80 / 1000;
const PRIME_MAX: usize = RATE as usize * 400 / 1000;

fn grown_prime(prime: usize) -> usize { (prime + PRIME_STEP).min(PRIME_MAX) }
// RMS do bloco anterior acima do qual a esvaziada cortou voz audível, não o fim de uma fala.
const SPEECH_LEVEL: f32 = 0.01;

/// Estado do callback de saída: o que já foi reamostrado e se está juntando reserva.
struct OutputState { resampler: Resampler, carry: VecDeque<f32>, priming: bool }

pub struct Audio {
    shared: Arc<Shared>,
    muted: Arc<AtomicBool>,
    apm: AudioProcessing,
    capture_config: StreamConfig,
    out_config: StreamConfig,
    capture_chunk: usize,
    render_chunk: usize,
    pending: Vec<f32>,
    _input: cpal::Stream,
    _output: cpal::Stream,
}

/// O padrão do aparelho; só a taxa muda para 48 kHz, no mesmo formato e canais (a lista vem com U8/I8 primeiro).
fn output_config(device: &cpal::Device) -> Option<cpal::SupportedStreamConfig> {
    let default = device.default_output_config().ok()?;
    if default.sample_rate() == RATE { return Some(default); }
    let same_shape = device.supported_output_configs().ok().and_then(|mut ranges| ranges.find(|range|
        range.sample_format() == default.sample_format() && range.channels() == default.channels()
            && range.min_sample_rate() <= RATE && RATE <= range.max_sample_rate()));
    Some(same_shape.map_or(default, |range| range.with_sample_rate(RATE)))
}

impl Audio {
    pub fn start(muted: Arc<AtomicBool>, pause: Arc<SpeechPause>) -> Result<Audio, AudioError> {
        let host = cpal::default_host();
        let input = host.default_input_device().ok_or(AudioError::Microphone)?;
        let output = host.default_output_device().ok_or(AudioError::Speaker)?;
        let in_config = input.default_input_config().map_err(|_| AudioError::Microphone)?;
        let out_config = output_config(&output).ok_or(AudioError::Speaker)?;
        let (in_rate, in_channels) = (in_config.sample_rate(), in_config.channels().max(1) as usize);
        let (out_rate, out_channels) = (out_config.sample_rate(), out_config.channels().max(1) as usize);
        let shared = Arc::new(Shared::new(in_rate as usize / 5, pause));
        crate::voice::log(format!("audio in rate={in_rate} ch={in_channels} fmt={:?} out rate={out_rate} ch={out_channels} fmt={:?}",
            in_config.sample_format(), out_config.sample_format()));
        let input_stream = build_input(&input, &in_config, in_channels, shared.clone()).ok_or(AudioError::Microphone)?;
        let output_stream = build_output(&output, &out_config, out_rate, out_channels, shared.clone()).ok_or(AudioError::Speaker)?;
        input_stream.play().map_err(|_| AudioError::Microphone)?;
        output_stream.play().map_err(|_| AudioError::Speaker)?;
        let capture_config = StreamConfig::new(in_rate, 1);
        let apm = AudioProcessing::builder()
            // Ganho automático como o do navegador: microfone USB baixo chegava à OpenAI quase mudo.
            .config(Config { echo_canceller: Some(EchoCanceller::default()),
                gain_controller2: Some(GainController2 { adaptive_digital: Some(AdaptiveDigital::default()), ..Default::default() }),
                ..Default::default() })
            .capture_config(capture_config).render_config(StreamConfig::new(out_rate, 1)).build();
        Ok(Audio { shared, muted, apm, capture_config, out_config: StreamConfig::new(RATE, 1),
            capture_chunk: in_rate as usize / 100, render_chunk: out_rate as usize / 100,
            pending: Vec::with_capacity(FRAME * 2), _input: input_stream, _output: output_stream })
    }

    pub fn next_frame(&mut self) -> Option<[f32; FRAME]> {
        loop {
            if self.pending.len() >= FRAME {
                let mut frame = [0f32; FRAME];
                frame.copy_from_slice(&self.pending[..FRAME]);
                self.pending.drain(..FRAME);
                apply_mute(&mut frame, mic_closed(&self.muted, &self.shared.pause));
                self.shared.input_level.store(rms(&frame).to_bits(), Ordering::Relaxed);
                return Some(frame);
            }
            let capture: Vec<f32> = {
                let mut queue = self.shared.capture.lock().unwrap();
                if queue.len() < self.capture_chunk { return None; }
                queue.drain(..self.capture_chunk).collect()
            };
            // O AEC precisa ver o que tocou antes de limpar o microfone.
            loop {
                let render: Vec<f32> = {
                    let mut queue = self.shared.render.lock().unwrap();
                    if queue.len() < self.render_chunk { break; }
                    queue.drain(..self.render_chunk).collect()
                };
                let mut scratch = vec![0f32; self.render_chunk];
                let _ = self.apm.process_render_f32(&[&render], &mut [&mut scratch]);
            }
            let _ = self.apm.set_stream_delay_ms(STREAM_DELAY_MS);
            let mut clean = vec![0f32; RATE as usize / 100];
            match self.apm.process_capture_f32_with_config(&[&capture], &self.capture_config, &self.out_config, &mut [&mut clean]) {
                Ok(()) => self.pending.extend_from_slice(&clean),
                // Sem isto a chamada ficaria muda sem ninguém saber por quê.
                Err(_) => self.shared.failed.store(1, Ordering::Relaxed),
            }
        }
    }

    pub fn play(&self, samples: &[f32]) { self.shared.play(samples); }

    /// Desce a fala guardada para a reprodução depois da pausa.
    pub fn refill(&self) { self.shared.refill(); }

    pub fn keep_unplayed(&self) { self.shared.keep_unplayed(); }

    pub fn announce_connected(&self) { self.shared.enqueue(&connected_tone()); }

    /// (ms de fala guardada, amostras perdidas pelo teto) para o diário.
    pub fn take_hold(&self) -> (u32, u32) {
        ((self.shared.pause.held_len() as u64 * 1000 / RATE as u64) as u32, self.shared.pause.take_dropped())
    }

    /// (entrou, tocou, descartado, quadros pedidos pela saída) desde a última leitura.
    pub fn take_flow(&self) -> [u32; 4] { std::array::from_fn(|i| self.shared.flow[i].swap(0, Ordering::Relaxed)) }

    pub fn reset(&mut self) {
        self.shared.capture.lock().unwrap().clear();
        self.shared.render.lock().unwrap().clear();
        self.pending.clear();
    }

    pub fn levels(&self) -> (f32, f32) {
        (f32::from_bits(self.shared.input_level.load(Ordering::Relaxed)),
         f32::from_bits(self.shared.output_level.load(Ordering::Relaxed)))
    }

    /// Pico do microfone cru desde a última leitura.
    pub fn take_raw_peak(&self) -> f32 { f32::from_bits(self.shared.raw_peak.swap(0, Ordering::Relaxed)) }

    pub fn capture_len(&self) -> usize { self.shared.capture.lock().unwrap().len() }

    pub fn playback_len(&self) -> usize { self.shared.playback.lock().unwrap().len() }

    pub fn take_underruns(&self) -> u32 { self.shared.underruns.swap(0, Ordering::Relaxed) }
    pub fn take_speech_underruns(&self) -> u32 { self.shared.speech_underruns.swap(0, Ordering::Relaxed) }
    pub fn prime_ms(&self) -> u32 { self.shared.prime.load(Ordering::Relaxed) * 1000 / RATE }

    pub fn failed(&self) -> Option<AudioError> {
        match self.shared.failed.load(Ordering::Relaxed) { 1 => Some(AudioError::Microphone), 2 => Some(AudioError::Speaker), _ => None }
    }
}

fn on_input<T: cpal::SizedSample>(data: &[T], channels: usize, shared: &Shared) where f32: cpal::FromSample<T> {
    let floats: Vec<f32> = data.iter().map(|v| f32::from_sample(*v)).collect();
    let mono = downmix(&floats, channels);
    // Float positivo ordena igual aos bits: fetch_max nos bits é o máximo do valor.
    shared.raw_peak.fetch_max(rms(&mono).to_bits(), Ordering::Relaxed);
    bounded_extend(&mut shared.capture.lock().unwrap(), mono, shared.capture_cap);
}

fn on_output<T: cpal::SizedSample + cpal::FromSample<f32>>(data: &mut [T], channels: usize, shared: &Shared, state: &mut OutputState) {
    let frames = data.len() / channels;
    shared.flow[3].fetch_add(frames as u32, Ordering::Relaxed);
    if shared.pause.is_paused() {
        // Fila e resto reamostrado ficam como estão: a retomada sai do ponto exato.
        for slot in data.iter_mut() { *slot = T::from_sample(0.0f32); }
        shared.output_level.store(0f32.to_bits(), Ordering::Relaxed);
        bounded_extend(&mut shared.render.lock().unwrap(), std::iter::repeat_n(0.0, frames), MAX_QUEUE);
        return;
    }
    let OutputState { resampler, carry, priming } = state;
    if *priming {
        let queued = shared.playback.lock().unwrap().len();
        if queued >= shared.prime.load(Ordering::Relaxed) as usize { *priming = false; }
    }
    while !*priming && carry.len() < frames {
        let chunk: Vec<f32> = {
            let mut queue = shared.playback.lock().unwrap();
            let take = queue.len().min(FRAME);
            shared.flow[1].fetch_add(take as u32, Ordering::Relaxed);
            queue.drain(..take).collect()
        };
        if chunk.is_empty() { break; }
        let mut out = Vec::new();
        resampler.push(&chunk, &mut out);
        carry.extend(out);
    }
    // Esvaziou com fala tocando: volta a juntar reserva em vez de picotar amostra a amostra.
    if !*priming && carry.len() < frames {
        *priming = true;
        shared.underruns.fetch_add(1, Ordering::Relaxed);
        if f32::from_bits(shared.output_level.load(Ordering::Relaxed)) >= SPEECH_LEVEL {
            shared.speech_underruns.fetch_add(1, Ordering::Relaxed);
            let prime = shared.prime.load(Ordering::Relaxed) as usize;
            shared.prime.store(grown_prime(prime) as u32, Ordering::Relaxed);
        }
    }
    let mono: Vec<f32> = (0..frames).map(|_| carry.pop_front().unwrap_or(0.0)).collect();
    shared.output_level.store(rms(&mono).to_bits(), Ordering::Relaxed);
    for (frame, sample) in data.chunks_mut(channels).zip(&mono) {
        for slot in frame { *slot = T::from_sample(*sample); }
    }
    bounded_extend(&mut shared.render.lock().unwrap(), mono, MAX_QUEUE);
}

fn build_input(device: &cpal::Device, config: &cpal::SupportedStreamConfig, channels: usize, shared: Arc<Shared>) -> Option<cpal::Stream> {
    macro_rules! input {
        ($t:ty) => {{
            let (data, error) = (shared.clone(), shared.clone());
            device.build_input_stream::<$t, _, _>(config.config(), move |samples: &[$t], _| on_input(samples, channels, &data),
                move |e| if is_fatal(&e) { error.failed.store(1, Ordering::Relaxed) }, None).ok()
        }};
    }
    match config.sample_format() {
        cpal::SampleFormat::F32 => input!(f32), cpal::SampleFormat::I16 => input!(i16), cpal::SampleFormat::I32 => input!(i32),
        cpal::SampleFormat::U16 => input!(u16), cpal::SampleFormat::U8 => input!(u8), cpal::SampleFormat::I8 => input!(i8),
        cpal::SampleFormat::F64 => input!(f64), _ => None,
    }
}

fn build_output(device: &cpal::Device, config: &cpal::SupportedStreamConfig, rate: u32, channels: usize, shared: Arc<Shared>) -> Option<cpal::Stream> {
    macro_rules! output {
        ($t:ty) => {{
            let (data, error) = (shared.clone(), shared.clone());
            let mut state = OutputState { resampler: Resampler::new(RATE, rate), carry: VecDeque::new(), priming: true };
            device.build_output_stream::<$t, _, _>(config.config(),
                move |samples: &mut [$t], _| on_output(samples, channels, &data, &mut state),
                move |e| if is_fatal(&e) { error.failed.store(2, Ordering::Relaxed) }, None).ok()
        }};
    }
    match config.sample_format() {
        cpal::SampleFormat::F32 => output!(f32), cpal::SampleFormat::I16 => output!(i16), cpal::SampleFormat::I32 => output!(i32),
        cpal::SampleFormat::U16 => output!(u16), cpal::SampleFormat::U8 => output!(u8), cpal::SampleFormat::I8 => output!(i8),
        cpal::SampleFormat::F64 => output!(f64), _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn resampler_keeps_duration_across_calls() {
        let mut r = Resampler::new(48_000, 44_100);
        let mut out = Vec::new();
        for _ in 0..50 { r.push(&[0.5; 960], &mut out); } // 1 s em blocos de 20 ms
        assert!((out.len() as i64 - 44_100).abs() <= 2, "{}", out.len());
        assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn resampler_same_rate_is_identity() {
        let mut r = Resampler::new(48_000, 48_000);
        let mut out = Vec::new();
        let input: Vec<f32> = (0..960).map(|i| i as f32 / 960.).collect();
        r.push(&input, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.25, 0.75], 1), vec![0.25, 0.75]);
    }

    #[test]
    fn muted_frames_are_silence() {
        let mut frame = [0.7f32; FRAME];
        apply_mute(&mut frame, true);
        assert!(frame.iter().all(|s| *s == 0.0));
        let mut live = [0.7f32; FRAME];
        apply_mute(&mut live, false);
        assert!(live.iter().all(|s| *s == 0.7));
    }

    fn paused_shared() -> (Arc<SpeechPause>, Shared) {
        let pause = Arc::new(SpeechPause::default());
        (pause.clone(), Shared::new(1, pause))
    }

    fn ramp(from: usize, len: usize) -> Vec<f32> { (from..from + len).map(|i| i as f32).collect() }

    fn output_state() -> OutputState { OutputState { resampler: Resampler::new(RATE, RATE), carry: VecDeque::new(), priming: false } }

    /// Toca até a fila e a fala guardada acabarem, como o laço da chamada e o callback da saída.
    fn drain(shared: &Shared, state: &mut OutputState) -> Vec<f32> {
        let mut heard = Vec::new();
        loop {
            shared.refill();
            let mut block = vec![0f32; FRAME];
            on_output(&mut block, 1, shared, state);
            let done = shared.playback.lock().unwrap().is_empty() && state.carry.is_empty() && shared.pause.held_len() == 0;
            heard.extend(block);
            if done { return heard; }
        }
    }

    #[test]
    fn pausing_keeps_the_speech_and_resuming_continues_from_the_same_point() {
        let (pause, shared) = paused_shared();
        let mut state = output_state();
        shared.play(&ramp(1, FRAME * 3));
        let mut block = vec![0f32; FRAME];
        on_output(&mut block, 1, &shared, &mut state);
        assert_eq!(block, ramp(1, FRAME), "antes da pausa toca normal");
        pause.set(true);
        on_output(&mut block, 1, &shared, &mut state);
        assert!(block.iter().all(|s| *s == 0.0), "pausa cala na hora");
        assert_eq!(f32::from_bits(shared.output_level.load(Ordering::Relaxed)), 0.0);
        shared.play(&ramp(1 + FRAME * 3, FRAME * 2));
        assert_eq!(shared.playback.lock().unwrap().len(), FRAME * 2, "o que faltava tocar fica parado");
        assert_eq!(pause.held_len(), FRAME * 2, "o que segue chegando fica guardado");
        shared.refill();
        assert_eq!(pause.held_len(), FRAME * 2, "pausada, nada desce");
        pause.set(false);
        shared.play(&ramp(1 + FRAME * 5, FRAME));
        assert_eq!(pause.held_len(), FRAME * 3, "fala nova vai atrás da guardada");
        let heard: Vec<f32> = drain(&shared, &mut state).into_iter().filter(|s| *s != 0.0).collect();
        assert_eq!(heard, ramp(1 + FRAME, FRAME * 5), "nenhuma amostra perdida nem fora de ordem");
    }

    #[test]
    fn a_long_pause_goes_back_without_hitting_the_playback_ceiling() {
        let (pause, shared) = paused_shared();
        let mut state = output_state();
        pause.set(true);
        let total = RATE as usize * 5;
        for at in (0..total).step_by(FRAME) { shared.play(&ramp(1 + at, FRAME)); }
        pause.set(false);
        let heard: Vec<f32> = drain(&shared, &mut state).into_iter().filter(|s| *s != 0.0).collect();
        assert_eq!(heard.len(), total, "cinco segundos passam do teto de dois da reprodução");
        assert_eq!(shared.flow[2].load(Ordering::Relaxed), 0, "nada descartado pelo teto");
    }

    #[test]
    fn with_no_pause_speech_goes_straight_to_playback() {
        let (pause, shared) = paused_shared();
        shared.play(&[0.1; 10]);
        assert_eq!((pause.held_len(), shared.playback.lock().unwrap().len()), (0, 10));
    }

    #[test]
    fn the_hold_ceiling_counts_what_it_could_not_keep() {
        let pause = SpeechPause::default();
        pause.set(true);
        pause.held.lock().unwrap().resize(HOLD_MAX - 2, 0.0);
        assert!(pause.hold(&[0.1; 5]));
        assert_eq!((pause.held_len(), pause.take_dropped()), (HOLD_MAX, 3));
    }

    #[test]
    fn a_new_audio_connection_keeps_what_the_old_one_had_queued_while_paused() {
        let (pause, shared) = paused_shared();
        shared.play(&ramp(1, 4));
        pause.set(true);
        shared.play(&ramp(5, 2));
        shared.keep_unplayed();
        assert!(shared.playback.lock().unwrap().is_empty());
        assert_eq!(pause.held.lock().unwrap().iter().copied().collect::<Vec<_>>(), ramp(1, 6), "o da fila vai na frente");
    }

    #[test]
    fn a_paused_call_sends_silence_even_with_the_microphone_on() {
        let (muted, pause) = (AtomicBool::new(false), SpeechPause::default());
        assert!(!mic_closed(&muted, &pause));
        pause.set(true);
        assert!(mic_closed(&muted, &pause));
        pause.set(false);
        muted.store(true, Ordering::Relaxed);
        assert!(mic_closed(&muted, &pause), "retomar a fala não abre o microfone silenciado");
    }

    #[test]
    fn the_stall_alert_plays_once() {
        let (pause, shared) = paused_shared();
        pause.request_alert();
        shared.refill();
        assert_eq!(shared.playback.lock().unwrap().len(), stalled_tone().len());
        shared.refill();
        assert_eq!(shared.playback.lock().unwrap().len(), stalled_tone().len(), "um pedido, um som");
    }

    #[test]
    fn the_connected_tone_passes_the_reserve_and_ends_in_silence() {
        let tone = connected_tone();
        assert!(tone.len() > PRIME_SAMPLES, "abaixo da reserva o som nunca tocaria");
        assert!(tone.iter().all(|s| s.abs() <= 0.18));
        assert!(tone[tone.len() - FRAME..].iter().all(|s| *s == 0.0), "fim sem voz não conta como corte");
    }

    #[test]
    fn reserve_grows_80_ms_per_cut_up_to_400_ms() {
        let ms = |samples: usize| samples * 1000 / RATE as usize;
        assert_eq!(ms(PRIME_SAMPLES), 240);
        let steps: Vec<usize> = std::iter::successors(Some(PRIME_SAMPLES), |p| Some(grown_prime(*p))).take(5).map(ms).collect();
        assert_eq!(steps, [240, 320, 400, 400, 400]);
    }

    #[test]
    fn rms_of_full_scale_square_is_one() {
        assert!((rms(&[1.0, -1.0, 1.0, -1.0]) - 1.0).abs() < 1e-6);
        assert_eq!(rms(&[]), 0.0);
    }
}
