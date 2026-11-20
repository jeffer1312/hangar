//! Memória e CPU do próprio processo do servidor: uma leitura por minuto num anel de uma hora, de
//! onde saem a curva e a média do bloco Servidor.
use std::collections::VecDeque;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use hangar_api::status::{Point, ServerBlock};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Uma hora, um ponto por minuto.
pub const RING_LEN: usize = 60;
/// Intervalo entre os `tick()` que o servidor agenda.
pub const TICK: Duration = Duration::from_secs(60);

pub type Clock = Box<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    ts: DateTime<Utc>,
    rss: u64,
    cpu: Option<f64>,
}

pub struct ProcessSampler {
    /// O mesmo `System` entre leituras: a CPU do processo é a diferença desde a leitura anterior.
    system: System,
    pid: Pid,
    clock: Clock,
    ring: VecDeque<Sample>,
    /// A primeira leitura de CPU não tem base de comparação e sai 0, que não é um valor real.
    cpu_primed: bool,
    started_at: Option<DateTime<Utc>>,
}

impl Default for ProcessSampler {
    fn default() -> Self { Self::new() }
}

impl ProcessSampler {
    pub fn new() -> Self { Self::with_clock(Box::new(Utc::now)) }

    pub fn with_clock(clock: Clock) -> Self {
        Self {
            system: System::new(),
            pid: Pid::from_u32(std::process::id()),
            clock,
            ring: VecDeque::with_capacity(RING_LEN),
            cpu_primed: false,
            started_at: None,
        }
    }

    pub fn pid(&self) -> u32 { self.pid.as_u32() }

    /// Lê o processo e guarda um ponto; o mais velho sai quando o anel passa de uma hora. Devolve
    /// `false` quando o sistema não entregou o processo (o anel fica como estava).
    pub fn tick(&mut self) -> bool {
        let kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
        self.system.refresh_processes_specifics(ProcessesToUpdate::Some(&[self.pid]), true, kind);
        let Some(process) = self.system.process(self.pid) else { return false };
        let cpu = self.cpu_primed.then(|| f64::from(process.cpu_usage()));
        self.cpu_primed = true;
        if self.started_at.is_none() {
            self.started_at = i64::try_from(process.start_time()).ok()
                .filter(|secs| *secs > 0)
                .and_then(|secs| Utc.timestamp_opt(secs, 0).single());
        }
        let sample = Sample { ts: (self.clock)(), rss: process.memory(), cpu };
        if self.ring.len() == RING_LEN {
            self.ring.pop_front();
        }
        self.ring.push_back(sample);
        true
    }

    /// O bloco Servidor com a última leitura; `None` antes do primeiro `tick()` que deu certo.
    /// Versão e commit vêm de quem monta o retrato.
    pub fn block(&self, version: String, commit: Option<String>) -> Option<ServerBlock> {
        let last = self.ring.back()?;
        let started_at = self.started_at.or_else(|| self.ring.front().map(|s| s.ts))?;
        Some(ServerBlock {
            version,
            commit,
            pid: self.pid(),
            started_at: rfc3339(started_at),
            rss_bytes: last.rss,
            cpu_pct: last.cpu,
            rss_series: self.ring.iter().map(|s| Point { ts: rfc3339(s.ts), v: s.rss as f64 }).collect(),
            cpu_series: self.ring.iter().filter_map(|s| Some(Point { ts: rfc3339(s.ts), v: s.cpu? })).collect(),
            rss_avg_bytes: self.rss_avg(),
            cpu_avg_pct: self.cpu_avg(),
        })
    }

    fn rss_avg(&self) -> Option<u64> {
        let n = self.ring.len() as u128;
        (n > 0).then(|| (self.ring.iter().map(|s| u128::from(s.rss)).sum::<u128>() / n) as u64)
    }

    fn cpu_avg(&self) -> Option<f64> {
        let values: Vec<f64> = self.ring.iter().filter_map(|s| s.cpu).collect();
        (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
    }
}

pub(crate) fn rfc3339(ts: DateTime<Utc>) -> String { ts.to_rfc3339_opts(SecondsFormat::Secs, true) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    /// Relógio que anda um minuto a cada leitura, a partir de um instante fixo.
    fn minute_clock() -> Clock {
        let start = Utc.with_ymd_and_hms(2026, 11, 17, 13, 0, 0).unwrap();
        let minutes = Arc::new(AtomicI64::new(0));
        Box::new(move || start + chrono::Duration::minutes(minutes.fetch_add(1, Ordering::SeqCst)))
    }

    #[test]
    fn ring_keeps_the_last_hour_and_first_cpu_is_none() {
        let mut sampler = ProcessSampler::with_clock(minute_clock());
        assert_eq!(sampler.block("1".into(), None), None, "sem leitura, sem bloco");

        assert!(sampler.tick());
        let first = sampler.block("1".into(), None).unwrap();
        assert_eq!(first.cpu_pct, None, "primeira CPU não tem base");
        assert!(first.cpu_series.is_empty());
        assert_eq!(first.cpu_avg_pct, None);
        assert!(first.rss_bytes > 0);
        assert_eq!(first.pid, std::process::id());

        for _ in 0..60 {
            assert!(sampler.tick());
        }
        let block = sampler.block("1".into(), Some("abc".into())).unwrap();
        assert_eq!(block.rss_series.len(), RING_LEN, "61 leituras guardam 60 pontos");
        assert_eq!(block.rss_series[0].ts, "2026-11-17T13:01:00Z", "o ponto mais velho saiu");
        assert_eq!(block.rss_series[59].ts, "2026-11-17T14:00:00Z");
        assert_eq!(block.cpu_series.len(), RING_LEN, "o ponto sem CPU foi o que saiu");
        assert!(block.cpu_pct.is_some());
        assert!(block.cpu_avg_pct.is_some_and(|avg| avg >= 0.0));
        assert_eq!(block.commit.as_deref(), Some("abc"));
        assert!(!block.started_at.is_empty());
    }

    #[test]
    fn averages_are_over_the_ring() {
        let mut sampler = ProcessSampler::with_clock(minute_clock());
        let ts = Utc.with_ymd_and_hms(2026, 11, 17, 13, 0, 0).unwrap();
        sampler.ring.extend([
            Sample { ts, rss: 100, cpu: None },
            Sample { ts, rss: 200, cpu: Some(10.0) },
            Sample { ts, rss: 300, cpu: Some(30.0) },
        ]);
        assert_eq!(sampler.rss_avg(), Some(200));
        assert_eq!(sampler.cpu_avg(), Some(20.0), "a leitura sem CPU fica fora da média");
    }
}
