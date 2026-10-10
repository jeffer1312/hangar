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
    // Fluxo da reprodução no diário: amostras que entraram, que tocaram, descartadas pelo teto e quadros pedidos pela saída.
    flow: [AtomicU32; 4],
}

// Reserva antes de tocar: o áudio chega em rajadas pela rede, e tocar no ato esvazia a fila a cada atraso.
const PRIME_SAMPLES: usize = RATE as usize * 240 / 1000;
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
    pub fn start(muted: Arc<AtomicBool>) -> Result<Audio, AudioError> {
        let host = cpal::default_host();
        let input = host.default_input_device().ok_or(AudioError::Microphone)?;
        let output = host.default_output_device().ok_or(AudioError::Speaker)?;
        let in_config = input.default_input_config().map_err(|_| AudioError::Microphone)?;
        let out_config = output_config(&output).ok_or(AudioError::Speaker)?;
        let (in_rate, in_channels) = (in_config.sample_rate(), in_config.channels().max(1) as usize);
        let (out_rate, out_channels) = (out_config.sample_rate(), out_config.channels().max(1) as usize);
        let shared = Arc::new(Shared {
            capture_cap: in_rate as usize / 5,
            capture: Default::default(), render: Default::default(), playback: Default::default(),
            input_level: AtomicU32::new(0), output_level: AtomicU32::new(0), raw_peak: AtomicU32::new(0), failed: AtomicU8::new(0), underruns: AtomicU32::new(0), speech_underruns: AtomicU32::new(0), flow: Default::default(),
        });
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
                apply_mute(&mut frame, self.muted.load(Ordering::Relaxed));
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

    pub fn play(&self, samples: &[f32]) {
        let mut queue = self.shared.playback.lock().unwrap();
        let dropped = (queue.len() + samples.len()).saturating_sub(MAX_QUEUE);
        self.shared.flow[0].fetch_add(samples.len() as u32, Ordering::Relaxed);
        self.shared.flow[2].fetch_add(dropped as u32, Ordering::Relaxed);
        bounded_extend(&mut queue, samples.iter().copied(), MAX_QUEUE);
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
    let OutputState { resampler, carry, priming } = state;
    if *priming {
        let queued = shared.playback.lock().unwrap().len();
        if queued >= PRIME_SAMPLES { *priming = false; }
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
        if f32::from_bits(shared.output_level.load(Ordering::Relaxed)) >= SPEECH_LEVEL { shared.speech_underruns.fetch_add(1, Ordering::Relaxed); }
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

    #[test]
    fn rms_of_full_scale_square_is_one() {
        assert!((rms(&[1.0, -1.0, 1.0, -1.0]) - 1.0).abs() < 1e-6);
        assert_eq!(rms(&[]), 0.0);
    }
}
