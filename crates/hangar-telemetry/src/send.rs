//! A tarefa de fundo: grava de tempos em tempos, fecha o dia e manda os dias fechados ao receptor.
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use hangar_core::ids::random_hex;

use crate::aggregate::{Telemetry, day_of, lock};
use crate::queue;
use crate::wire::{Batch, SCHEMA};

pub const DEFAULT_URL: &str = "https://telemetry.hangar.dev.br/v1/batch";
/// Os mesmos valores do relatório automático: máquinas que falham juntas se espalham nas esperas.
const RETRY_WAITS: [u64; 4] = [0, 60, 180, 420];
const SAVE_EVERY: Duration = Duration::from_secs(15 * 60);
/// Espalha o envio das máquinas que fecham o dia no mesmo instante.
const MAX_JITTER_SECS: u64 = 60 * 60;
const TIMEOUT: Duration = Duration::from_secs(30);

/// Status HTTP da resposta; `None` quando não houve resposta (rede).
pub type PostFuture = Pin<Box<dyn Future<Output = Option<u16>> + Send>>;
/// Quem leva o corpo do pacote ao receptor; os testes injetam um falso, sem socket.
pub type Post = Arc<dyn Fn(Vec<u8>) -> PostFuture + Send + Sync>;

/// O que a tarefa de fundo devolve a quem a chamou, para o diário.
#[derive(Debug)]
pub enum Failure {
    /// 400 ou 413: o pacote foi apagado.
    Refused(u16),
    /// Nenhuma tentativa deu 2xx (rede, DNS, TLS, redirecionamento, 401/403/404/410, 5xx): o dia fica na
    /// fila. `status` é o da última tentativa, `None` sem resposta. No máximo um por dia (UTC).
    Unreachable { status: Option<u16> },
    /// Dias que saíram da fila pelo teto, sem ter sido enviados.
    Pruned(usize),
    Disk(io::Error),
}

impl Telemetry {
    /// `POST` ao `HANGAR_TELEMETRY_URL` do ambiente ou ao endereço padrão.
    pub fn http_post(&self) -> reqwest::Result<Post> {
        // Redirecionamento não é seguido: vira resposta não 2xx e o dia espera a próxima tentativa.
        let client = hangar_core::net::http_client().timeout(TIMEOUT).redirect(reqwest::redirect::Policy::none()).build()?;
        let url = (self.lookup)("HANGAR_TELEMETRY_URL").filter(|url| !url.trim().is_empty()).unwrap_or_else(|| DEFAULT_URL.to_owned());
        let stamp = format!("hangar-server/{}", self.batch_env.version);
        Ok(Arc::new(move |body: Vec<u8>| -> PostFuture {
            let request = client.post(&url).header("content-type", "application/json").header("x-hangar-stamp", &stamp).body(body);
            Box::pin(async move { request.send().await.ok().map(|response| response.status().as_u16()) })
        }))
    }

    /// Roda até o processo acabar. O desligamento do servidor chama `save` à parte. A gravação e o
    /// envio são laços separados: receptor lento ou fora do ar não atrasa a gravação.
    pub async fn run(self: Arc<Self>, post: Post, report: impl Fn(Failure) + Send + 'static) {
        // A trava só divide o `report` entre os dois laços sem exigir `Sync` de quem chama.
        let report = std::sync::Mutex::new(report);
        let tell = |failure: Failure| {
            if let Ok(report) = report.lock() {
                (*report)(failure);
            }
        };
        let saving = async {
            loop {
                if let Err(e) = self.tick() {
                    tell(Failure::Disk(e));
                }
                for failure in self.take_notes() {
                    tell(failure);
                }
                tokio::time::sleep(SAVE_EVERY).await;
            }
        };
        let sending = async {
            loop {
                match self.closed_days() {
                    Ok(closed) if !closed.is_empty() => {
                        tokio::time::sleep(jitter()).await;
                        for failure in self.send_closed(&post).await {
                            tell(failure);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tell(Failure::Disk(e)),
                }
                tokio::time::sleep(SAVE_EVERY).await;
            }
        };
        tokio::join!(saving, sending);
    }

    /// Os dias da fila que já podem sair: anteriores ao corrente e já gravados. Um dia que virou
    /// e ainda espera a gravação fica de fora: mandado agora, voltaria sem `batch_id` e sairia duas vezes.
    fn closed_days(&self) -> io::Result<Vec<String>> {
        if !self.enabled() {
            return Ok(Vec::new());
        }
        let (today, pending) = self.pending_days()?;
        Ok(queue::days(&self.home)?.into_iter().filter(|day| *day < today && !pending.contains(day)).collect())
    }

    /// Manda cada dia fechado, do mais antigo ao mais novo.
    pub(crate) async fn send_closed(&self, post: &Post) -> Vec<Failure> {
        let mut failures = Vec::new();
        let days = match self.closed_days() {
            Ok(days) => days,
            Err(e) => {
                failures.push(Failure::Disk(e));
                return failures;
            }
        };
        for day in days {
            if !self.enabled() {
                break;
            }
            match self.send_day(&day, post).await {
                Ok(Some(Failure::Unreachable { status })) => {
                    if self.first_unreachable_today() {
                        failures.push(Failure::Unreachable { status });
                    }
                }
                Ok(Some(failure)) => failures.push(failure),
                Ok(None) => {}
                Err(e) => failures.push(Failure::Disk(e)),
            }
        }
        failures
    }

    /// Receptor fora do ar falha em toda rodada: o diário recebe um aviso por dia (UTC).
    fn first_unreachable_today(&self) -> bool {
        let today = day_of((self.clock)());
        let Ok(mut last) = self.unreachable_day.lock() else { return false };
        if last.as_deref() == Some(today.as_str()) {
            return false;
        }
        *last = Some(today);
        true
    }

    /// `Refused` quando o receptor recusou o pacote e ele foi apagado; `Unreachable` quando nenhuma
    /// tentativa deu 2xx (sem resposta, 429, 5xx ou outro status), e o dia fica na fila para a próxima rodada.
    async fn send_day(&self, day: &str, post: &Post) -> io::Result<Option<Failure>> {
        let mut last = None;
        for wait in RETRY_WAITS {
            tokio::time::sleep(Duration::from_secs(wait)).await;
            // Relido a cada tentativa: revogar na espera apaga o dia da fila, e consentir de novo não o traz.
            let Some(body) = self.prepare(day)? else { return Ok(None) };
            match post(body).await {
                Some(200..=299) => return self.drop_day(day).map(|()| None),
                Some(status @ (400 | 413)) => return self.drop_day(day).map(|()| Some(Failure::Refused(status))),
                status => last = status,
            }
        }
        Ok(Some(Failure::Unreachable { status: last }))
    }

    /// O corpo do pacote do dia; o `batch_id` é gravado antes do primeiro envio.
    fn prepare(&self, day: &str) -> io::Result<Option<Vec<u8>>> {
        let _io = lock(&self.io)?;
        if !self.enabled() {
            return Ok(None);
        }
        let mut file = match queue::read_day(&self.home, day) {
            Ok(Some(file)) => file,
            Ok(None) => return Ok(None),
            // Arquivo que não se lê nunca sairia e travaria a fila.
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                queue::remove_day(&self.home, day)?;
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        let batch_id = match &file.batch_id {
            Some(id) => id.clone(),
            None => {
                let id = random_hex(16);
                file.batch_id = Some(id.clone());
                queue::write_day(&self.home, &file)?;
                id
            }
        };
        let state = lock(&self.settings)?.state.clone();
        let batch = Batch {
            schema: SCHEMA,
            batch_id,
            machine_id: state.machine_id,
            owner_id: state.owner_id,
            day: day.to_owned(),
            env: self.batch_env.clone(),
            counters: file.counters,
            logs: file.logs,
        };
        // Fora da forma o receptor recusaria sempre: vale como arquivo ilegível.
        if let Err(field) = batch.validate() {
            queue::remove_day(&self.home, day)?;
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("telemetria: pacote de {day} fora da forma ({field})")));
        }
        Ok(Some(serde_json::to_vec(&batch)?))
    }

    fn drop_day(&self, day: &str) -> io::Result<()> {
        let _io = lock(&self.io)?;
        queue::remove_day(&self.home, day)
    }
}

fn jitter() -> Duration {
    Duration::from_secs(u64::from_str_radix(&random_hex(4), 16).unwrap_or(0) % MAX_JITTER_SECS)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::aggregate::tests::{Fixture, message};
    use crate::events::Event;

    fn receiver(status: Option<u16>) -> (Post, Arc<Mutex<Vec<Batch>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let post: Post = Arc::new(move |body: Vec<u8>| -> PostFuture {
            log.lock().unwrap().push(serde_json::from_slice::<Batch>(&body).unwrap());
            Box::pin(async move { status })
        });
        (post, seen)
    }

    #[tokio::test(start_paused = true)]
    async fn day_revoked_between_retries_is_not_sent_even_after_consenting_again() {
        let fx = Fixture::new();
        let telemetry = Arc::new(fx.start());
        telemetry.count(Event::MessageSent, message(), 4);
        fx.next_day();
        telemetry.save().unwrap();

        let tries = Arc::new(Mutex::new(0));
        let (seen, revoking) = (tries.clone(), telemetry.clone());
        let post: Post = Arc::new(move |_body: Vec<u8>| -> PostFuture {
            *seen.lock().unwrap() += 1;
            revoking.set_consent(false).unwrap();
            revoking.set_consent(true).unwrap();
            Box::pin(async { Some(503) })
        });
        assert!(telemetry.send_closed(&post).await.is_empty());
        assert_eq!(*tries.lock().unwrap(), 1, "o corpo de antes da revogação não sai");
        assert!(!fx.queue().join("2026-11-26.json").exists());
    }

    #[tokio::test(start_paused = true)]
    async fn closed_day_is_one_batch_and_retries_reuse_its_id() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 4);
        fx.next_day();
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();

        let (down, tried) = receiver(Some(503));
        assert!(matches!(telemetry.send_closed(&down).await.as_slice(), [Failure::Unreachable { status: Some(503) }]));
        let tried = tried.lock().unwrap().clone();
        assert_eq!(tried.len(), RETRY_WAITS.len());
        assert!(tried.iter().all(|batch| batch.batch_id == tried[0].batch_id && batch.day == "2026-11-26"));
        assert_eq!(tried[0].counters[0].count, 4);
        assert!(tried[0].validate().is_ok());
        assert!(fx.queue().join("2026-11-26.json").exists());

        let (up, sent) = receiver(Some(200));
        assert!(telemetry.send_closed(&up).await.is_empty());
        let sent = sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].batch_id, tried[0].batch_id);
        assert!(!fx.queue().join("2026-11-26.json").exists());
        assert!(fx.queue().join("2026-11-27.json").exists(), "o dia corrente fica");

        let (again, resent) = receiver(Some(200));
        assert!(telemetry.send_closed(&again).await.is_empty());
        assert!(resent.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn unreachable_receiver_is_reported_once_a_day() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        fx.next_day();
        telemetry.save().unwrap();

        let (silent, tried) = receiver(None);
        assert!(matches!(telemetry.send_closed(&silent).await.as_slice(), [Failure::Unreachable { status: None }]));
        assert!(telemetry.send_closed(&silent).await.is_empty(), "o mesmo dia não avisa de novo");
        assert_eq!(tried.lock().unwrap().len(), 2 * RETRY_WAITS.len());
        assert!(fx.queue().join("2026-11-26.json").exists(), "o dia fica na fila");

        fx.next_day();
        let (moved, _) = receiver(Some(301));
        assert!(matches!(telemetry.send_closed(&moved).await.as_slice(), [Failure::Unreachable { status: Some(301) }]));
    }

    #[tokio::test]
    async fn closed_day_carries_its_log_lines() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.log(crate::events::LogLevel::Error, "hangar_server::list", "falhou");
        fx.next_day();
        telemetry.save().unwrap();
        let (up, seen) = receiver(Some(200));
        assert!(telemetry.send_closed(&up).await.is_empty());
        let sent = seen.lock().unwrap().clone();
        assert_eq!(sent[0].logs, vec![crate::wire::LogLine {
            level: "error".into(), target: "hangar_server::list".into(), message: "falhou".into(), count: 1,
        }]);
        assert!(sent[0].validate().is_ok());
    }

    #[tokio::test]
    async fn refused_batch_is_dropped_and_reported() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        fx.next_day();
        telemetry.save().unwrap();

        let (refusing, seen) = receiver(Some(400));
        let failures = telemetry.send_closed(&refusing).await;
        assert!(matches!(failures.as_slice(), [Failure::Refused(400)]));
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(!fx.queue().join("2026-11-26.json").exists());
    }

    #[tokio::test]
    async fn malformed_day_is_dropped_and_reported() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        let bad = queue::DayFile {
            day: "2026-11-25".into(),
            batch_id: None,
            counters: vec![crate::wire::Counter { event: "Bad".into(), dims: Default::default(), bucket: None, count: 1 }],
            logs: Vec::new(),
        };
        queue::write_day(fx.home.path(), &bad).unwrap();

        let (up, seen) = receiver(Some(200));
        let failures = telemetry.send_closed(&up).await;
        assert!(matches!(failures.as_slice(), [Failure::Disk(e)] if e.kind() == io::ErrorKind::InvalidData));
        assert!(seen.lock().unwrap().is_empty());
        assert!(!fx.queue().join("2026-11-25.json").exists());
    }

    #[tokio::test(start_paused = true)]
    async fn slow_receiver_does_not_stop_the_periodic_save() {
        let fx = Fixture::new();
        let telemetry = Arc::new(fx.start());
        telemetry.count(Event::MessageSent, message(), 1);
        fx.next_day();

        let calls = Arc::new(Mutex::new(0usize));
        let seen = calls.clone();
        let post: Post = Arc::new(move |_: Vec<u8>| -> PostFuture {
            *seen.lock().unwrap() += 1;
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                Some(503)
            })
        });
        tokio::spawn(telemetry.clone().run(post, |_| {}));
        tokio::time::sleep(Duration::from_secs(1)).await;
        for n in 1..=8u64 {
            telemetry.count(Event::MessageSent, message(), 1);
            tokio::time::sleep(SAVE_EVERY).await;
            let file = queue::read_day(fx.home.path(), "2026-11-27").unwrap().unwrap();
            assert_eq!(file.counters[0].count, n, "gravação de {n} × 15 min");
        }
        assert!(*calls.lock().unwrap() >= 1, "o envio estava em curso");
        assert!(fx.queue().join("2026-11-26.json").exists());
    }

    #[tokio::test]
    async fn day_waiting_for_its_save_is_not_sent() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        telemetry.count(Event::MessageSent, message(), 1);
        fx.next_day();
        // A contagem do dia seguinte vira o dia em memória sem gravar o anterior.
        telemetry.count(Event::MessageSent, message(), 1);

        let (up, seen) = receiver(Some(200));
        assert!(telemetry.send_closed(&up).await.is_empty());
        assert!(seen.lock().unwrap().is_empty());
        telemetry.save().unwrap();
        assert!(telemetry.send_closed(&up).await.is_empty());
        assert_eq!(seen.lock().unwrap()[0].counters[0].count, 2);
    }
}
