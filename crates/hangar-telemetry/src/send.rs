//! A tarefa de fundo: grava de tempos em tempos, fecha o dia e manda os dias fechados ao receptor.
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};

use hangar_core::ids::random_hex;

use crate::aggregate::{Telemetry, day_of, lock};
use crate::diary::DayDetail;
use crate::events::{Code, Dims, Event};
use crate::queue::{self, DayFile};
use crate::state::Collection;
use crate::wire::{Batch, Counter, MAX_COUNT, SCHEMA};

pub const DEFAULT_URL: &str = "https://telemetry.hangar.dev.br/v1/batch";
/// Os mesmos valores do relatório automático: máquinas que falham juntas se espalham nas esperas.
const RETRY_WAITS: [u64; 4] = [0, 60, 180, 420];
const SAVE_EVERY: Duration = Duration::from_secs(15 * 60);
/// O retrato do dia ainda aberto: o receptor troca o anterior do mesmo `batch_id`.
const PARTIAL_EVERY: Duration = Duration::from_secs(60 * 60);
/// Espalha o envio das máquinas que fecham o dia no mesmo instante.
const MAX_JITTER_SECS: u64 = 60 * 60;
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 2 * 1024 * 1024;

/// Status HTTP da resposta; `None` quando não houve resposta (rede).
pub type PostFuture = Pin<Box<dyn Future<Output = Option<u16>> + Send>>;
/// Quem leva o corpo ao receptor. A construção e cada poll não podem bloquear nem chamar o portão de volta.
pub type Post = Arc<dyn Fn(Vec<u8>) -> PostFuture + Send + Sync>;

/// O que a tarefa de fundo devolve a quem a chamou, para o diário.
#[derive(Debug)]
pub enum Failure {
    /// O receptor recusou o lote: 400 dá baixa; 413 mantém na fila para diagnóstico e reenvio.
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
                if let Err(e) = self.save_async().await { tell(Failure::Disk(e)); }
                for failure in self.take_notes() {
                    tell(failure);
                }
                tokio::time::sleep(SAVE_EVERY).await;
            }
        };
        let sending = async {
            loop {
                match self.closed_days_async().await {
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
        let partial = async {
            loop {
                tokio::time::sleep(PARTIAL_EVERY).await;
                match self.save_async().await {
                    Ok(()) => if let Some(failure) = self.send_partial(&post).await { tell(failure); },
                    Err(e) => tell(Failure::Disk(e)),
                }
            }
        };
        tokio::join!(saving, sending, partial);
    }

    async fn save_async(self: &Arc<Self>) -> io::Result<()> {
        let telemetry = self.clone();
        tokio::task::spawn_blocking(move || telemetry.save()).await.map_err(io::Error::other)?
    }

    /// Guarda o parcial atual e congela os dias fechados mesmo sem rede. A marca vem depois do arquivo.
    pub(crate) fn checkpoint_held(&self, period: &Collection, now: DateTime<Utc>) -> io::Result<()> {
        let today = now.date_naive();
        let earliest = period.since.date_naive().max(today - TimeDelta::days(queue::MAX_DAYS as i64 - 1));
        let first = earliest.format("%Y-%m-%d").to_string();
        for day in queue::days(&self.home)?.into_iter().take_while(|day| day < &first) {
            self.capture_held(&day, period, now, true, false)?;
        }
        let mut day = earliest;
        while day <= today {
            let name = day.format("%Y-%m-%d").to_string();
            if period.sealed_through.as_ref().is_none_or(|sealed| name > *sealed) {
                let sealed = day < today;
                self.capture_held(&name, period, now, sealed, true)?;
                if sealed { self.seal_through_held(name)?; }
            }
            day = day.succ_opt().ok_or_else(|| io::Error::other("dia da telemetria fora do intervalo"))?;
        }
        let pruned = queue::cap(&self.home)?;
        if pruned > 0 { self.note(Failure::Pruned(pruned)); }
        Ok(())
    }

    fn capture_held(&self, day: &str, period: &Collection, now: DateTime<Utc>, sealed: bool, read_diary: bool) -> io::Result<()> {
        let mut file = match queue::read_day(&self.home, day) {
            Ok(file) => file.filter(|file| file.collection == period.id && file.day == day)
                .unwrap_or_else(|| DayFile { day: day.into(), collection: period.id.clone(), ..DayFile::default() }),
            // Recomeça pelo diário: retornar aqui deixaria o dia ser selado sem a reconstrução.
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                queue::remove_day(&self.home, day)?;
                self.note(Failure::Disk(e));
                DayFile { day: day.into(), collection: period.id.clone(), ..DayFile::default() }
            }
            Err(e) => return Err(e),
        };
        if file.sealed { return Ok(()); }
        if read_diary && let Some(source) = self.detail.get() {
            let detail = source(day, period, now)?;
            // Se o diário já rodou a poda, o último retrato gravado ainda pode fechar o lote.
            if !detail.is_empty() { file.detail = detail; }
        }
        if file.counters.is_empty() && file.detail.is_empty() { return Ok(()); }
        file.batch_id.get_or_insert_with(|| random_hex(16));
        file.sealed = sealed;
        let mut batch = self.batch(&file)?;
        encode_fit(&mut batch, &mut file.detail, MAX_BODY)?;
        queue::write_day(&self.home, &file)
    }

    async fn closed_days_async(self: &Arc<Self>) -> io::Result<Vec<String>> {
        let telemetry = self.clone();
        tokio::task::spawn_blocking(move || telemetry.closed_days()).await.map_err(io::Error::other)?
    }

    fn closed_days(&self) -> io::Result<Vec<String>> {
        if !self.enabled() { return Ok(Vec::new()); }
        let (today, pending) = self.pending_days()?;
        Ok(queue::days(&self.home)?.into_iter().filter(|day| *day < today && !pending.contains(day)).collect())
    }

    /// Manda cada dia fechado, do mais antigo ao mais novo.
    pub(crate) async fn send_closed(&self, post: &Post) -> Vec<Failure> {
        let mut failures = Vec::new();
        let Some(period) = self.collection() else { return failures };
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
            let outcome = self.send_day(&day, &period.id, post).await;
            failures.extend(self.once_a_day(outcome));
        }
        failures
    }

    /// O dia corrente até agora, com `partial`: uma tentativa só, porque a hora seguinte manda de
    /// novo. O dia fica na fila com o mesmo `batch_id`; o envio do dia fechado é o final.
    pub(crate) async fn send_partial(&self, post: &Post) -> Option<Failure> {
        let period = self.collection()?;
        let day = self.today();
        let outcome = match self.attempt(&day, &period.id, true, None, post).await {
            Ok(Some((_, status))) => match status {
                Some(200..=299) => Ok(None),
                Some(status @ (400 | 413)) => Ok(Some(Failure::Refused(status))),
                status => Ok(Some(Failure::Unreachable { status })),
            },
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        };
        self.once_a_day(outcome)
    }

    /// Receptor fora do ar avisa uma vez por dia, também entre o retrato da hora e o dia fechado.
    fn once_a_day(&self, outcome: io::Result<Option<Failure>>) -> Option<Failure> {
        match outcome {
            Ok(Some(Failure::Unreachable { status })) => self.first_unreachable_today().then_some(Failure::Unreachable { status }),
            Ok(failure) => failure,
            Err(e) => Some(Failure::Disk(e)),
        }
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

    async fn send_day(&self, day: &str, collection: &str, post: &Post) -> io::Result<Option<Failure>> {
        let mut last = None;
        let mut expected: Option<String> = None;
        for wait in RETRY_WAITS {
            tokio::time::sleep(Duration::from_secs(wait)).await;
            let Some((id, status)) = self.attempt(day, collection, false, expected.as_deref(), post).await? else { return Ok(None) };
            match status {
                Some(200..=299) => return self.drop_day(day, collection, &id).map(|()| None),
                Some(400) => return self.drop_day(day, collection, &id).map(|()| Some(Failure::Refused(400))),
                Some(413) => return Ok(Some(Failure::Refused(413))),
                status => last = status,
            }
            expected = Some(id);
        }
        Ok(Some(Failure::Unreachable { status: last }))
    }

    /// Prepara e dispara uma tentativa. Cada poll do pedido passa pelo portão sob a trava, e a revogação
    /// fecha o portão sob a mesma trava: revogado, o próximo poll descarta o pedido (cancela a tentativa) e a
    /// resposta não dá baixa. O cliente HTTP só entrega o pedido à conexão dentro de um poll, então revogação
    /// anterior a essa entrega impede a saída. Depois dela, a conexão do `reqwest` escreve o corpo na própria
    /// tarefa: o que já foi entregue ao transporte não volta. `None`: nada a enviar ou portão fechado.
    async fn attempt(&self, day: &str, collection: &str, partial: bool, expected: Option<&str>, post: &Post)
        -> io::Result<Option<(String, Option<u16>)>> {
        let Some((id, body)) = self.prepare(day, collection, partial, expected)? else { return Ok(None) };
        let mut request = post(body);
        let status = poll_fn(|cx| match lock(&self.post_gate) {
            Err(e) => Poll::Ready(Err(e)),
            Ok(gate) if gate.as_deref() != Some(collection) => Poll::Ready(Ok(None)),
            Ok(_open) => request.as_mut().poll(cx).map(|status| Ok(Some(status))),
        }).await?;
        Ok(status.map(|status| (id, status)))
    }

    fn prepare(&self, day: &str, collection: &str, partial: bool, expected: Option<&str>) -> io::Result<Option<(String, Vec<u8>)>> {
        let _io = lock(&self.io)?;
        self.refresh_held()?;
        if self.collection().is_none_or(|current| current.id != collection) || (partial && self.today() != day) { return Ok(None); }
        let Some(mut file) = queue::read_day(&self.home, day)?.filter(|file| {
            file.day == day && file.collection == collection && file.sealed != partial && file.batch_id.is_some()
                && expected.is_none_or(|expected| file.batch_id.as_deref() == Some(expected))
        }) else { return Ok(None) };
        let mut batch = self.batch(&file)?;
        let body = encode_fit(&mut batch, &mut file.detail, MAX_BODY)?;
        // A serialização pode demorar: reconfere o portão depois dela, imediatamente antes do POST.
        self.refresh_held()?;
        if self.collection().is_none_or(|current| current.id != collection) { return Ok(None); }
        Ok(Some((batch.batch_id, body)))
    }

    fn batch(&self, file: &DayFile) -> io::Result<Batch> {
        let state = lock(&self.settings)?;
        Ok(Batch {
            schema: SCHEMA,
            batch_id: file.batch_id.clone().ok_or_else(|| io::Error::other("lote sem ID persistido"))?,
            machine_id: state.state.machine_id.clone(),
            owner_id: state.state.owner_id.clone(),
            day: file.day.clone(),
            env: self.batch_env.clone(),
            counters: file.counters.clone(),
            logs: Vec::new(),
            partial: !file.sealed,
            records: Vec::new(),
            trail: Vec::new(),
        })
    }

    fn drop_day(&self, day: &str, collection: &str, id: &str) -> io::Result<()> {
        let _io = lock(&self.io)?;
        self.refresh_held()?;
        if self.collection().is_some_and(|current| current.id == collection)
            && queue::read_day(&self.home, day)?.is_some_and(|file| {
                file.day == day && file.sealed && file.collection == collection && file.batch_id.as_deref() == Some(id)
            }) {
            queue::remove_day(&self.home, day)?;
        }
        Ok(())
    }
}

/// Mede o JSON que de fato vai sair, incluindo bytes de UTF-8, escapes e contadores de cortes.
fn encode_fit(batch: &mut Batch, detail: &mut DayDetail, limit: usize) -> io::Result<Vec<u8>> {
    let counters = batch.counters.len();
    let records = detail.records.len();
    let steps = detail.trail.len();
    let mut encode = |keep: usize| -> io::Result<Vec<u8>> {
        let keep_records = keep.min(records);
        let keep_steps = keep.saturating_sub(records);
        batch.records = detail.records[..keep_records].to_vec();
        batch.trail = detail.trail[steps - keep_steps..].to_vec();
        batch.counters.truncate(counters);
        for (part, dropped) in [("records", detail.dropped_records + (records - keep_records) as u64),
                                ("trail", detail.dropped_steps + (steps - keep_steps) as u64)] {
            if dropped > 0 {
                let dims = Dims::new().with("part", Code::fixed(part));
                batch.counters.push(Counter::new(Event::DiaryDropped, dims, None, dropped.min(MAX_COUNT)));
            }
        }
        batch.validate().map_err(|field| io::Error::new(io::ErrorKind::InvalidData, format!("telemetria: pacote inválido ({field})")))?;
        Ok(serde_json::to_vec(batch)?)
    };
    let body = encode(records + steps)?;
    if body.len() <= limit { return Ok(body); }
    let mut body = encode(0)?;
    if body.len() > limit { return Err(io::Error::other("os contadores do dia excedem o teto HTTP da telemetria")); }
    let (mut low, mut high) = (0, records + steps);
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        let candidate = encode(mid)?;
        if candidate.len() <= limit { low = mid; body = candidate; } else { high = mid; }
    }
    let keep_records = low.min(records);
    let keep_steps = low.saturating_sub(records);
    detail.records.truncate(keep_records);
    detail.trail.drain(..steps - keep_steps);
    detail.dropped_records += (records - keep_records) as u64;
    detail.dropped_steps += (steps - keep_steps) as u64;
    Ok(body)
}

fn jitter() -> Duration {
    Duration::from_secs(u64::from_str_radix(&random_hex(4), 16).unwrap_or(0) % MAX_JITTER_SECS)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

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
    async fn telemetry_kill_switch_before_retry_invalidates_the_period_across_restart() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        let period = telemetry.collection().unwrap();
        telemetry.count(Event::MessageSent, message(), 1);
        fx.next_day();
        telemetry.save().unwrap();
        let config = fx.home.path().join(".hangar/config");
        std::fs::create_dir_all(&config).unwrap();
        let dotenv = config.join(".env");
        let stop = dotenv.clone();
        let tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = tries.clone();
        let post: Post = Arc::new(move |_| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::fs::write(&stop, "HANGAR_TELEMETRY=0\n").unwrap();
            Box::pin(async { Some(503) })
        });
        assert!(telemetry.send_closed(&post).await.is_empty());
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!telemetry.enabled());
        drop(telemetry);
        std::fs::remove_file(dotenv).unwrap();
        let telemetry = fx.start_as_is();
        assert_ne!(telemetry.collection().unwrap().id, period.id);
        let (post, seen) = receiver(Some(200));
        assert!(telemetry.send_closed(&post).await.is_empty());
        assert!(seen.lock().unwrap().is_empty());
    }

    /// POST que fica pendente até `release`; `delivered` marca quando o corpo teria chegado ao transporte.
    fn held_receiver(status: u16) -> (Post, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>, Arc<AtomicBool>) {
        let (polled, release, delivered) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()), Arc::new(AtomicBool::new(false)));
        let (seen, gate, sent) = (polled.clone(), release.clone(), delivered.clone());
        let post: Post = Arc::new(move |_body| {
            seen.notify_one();
            let (gate, sent) = (gate.clone(), sent.clone());
            Box::pin(async move {
                gate.notified().await;
                sent.store(true, Ordering::SeqCst);
                Some(status)
            })
        });
        (post, polled, release, delivered)
    }

    #[tokio::test(start_paused = true)]
    async fn revocation_while_the_post_is_pending_cancels_it_before_the_body_goes_out() {
        let fx = Fixture::new();
        let telemetry = Arc::new(fx.start());
        telemetry.count(Event::MessageSent, message(), 1);
        fx.next_day();
        telemetry.save().unwrap();
        let (post, polled, release, delivered) = held_receiver(200);
        let sending = tokio::spawn({
            let (telemetry, post) = (telemetry.clone(), post.clone());
            async move { telemetry.send_closed(&post).await }
        });
        polled.notified().await;
        telemetry.set_consent(false).unwrap();
        release.notify_one();
        assert!(sending.await.unwrap().is_empty());
        assert!(!delivered.load(Ordering::SeqCst), "o pedido pendente é cancelado na revogação");
    }

    #[tokio::test(start_paused = true)]
    async fn telemetry_old_success_cannot_ack_the_new_acceptance_for_the_same_day() {
        let fx = Fixture::new();
        let telemetry = Arc::new(fx.start());
        telemetry.count(Event::MessageSent, message(), 1);
        fx.next_day();
        telemetry.save().unwrap();
        let day = "2026-11-26";
        let old_period = telemetry.collection().unwrap().id;
        let old = queue::read_day(fx.home.path(), day).unwrap().unwrap().batch_id.unwrap();
        let (post, polled, release, _) = held_receiver(200);
        let sending = tokio::spawn({
            let (telemetry, post) = (telemetry.clone(), post.clone());
            async move { telemetry.send_closed(&post).await }
        });
        // O primeiro poll já passou pelo portão; a troca de coleção vem com o POST antigo pendente.
        polled.notified().await;
        telemetry.set_consent(false).unwrap();
        telemetry.set_consent(true).unwrap();
        let replacement = "a".repeat(32);
        let period = telemetry.collection().unwrap();
        queue::write_day(fx.home.path(), &DayFile { day: day.into(), collection: period.id, batch_id: Some(replacement.clone()),
            sealed: true, ..DayFile::default() }).unwrap();
        release.notify_one();
        assert!(sending.await.unwrap().is_empty());
        assert_eq!(queue::read_day(fx.home.path(), day).unwrap().unwrap().batch_id, Some(replacement.clone()));
        // A baixa tardia do lote antigo, se chegasse, também não apaga o dia do aceite novo.
        telemetry.drop_day(day, &old_period, &old).unwrap();
        assert_eq!(queue::read_day(fx.home.path(), day).unwrap().unwrap().batch_id, Some(replacement));
    }

    #[tokio::test(start_paused = true)]
    async fn unreadable_closed_day_is_rebuilt_from_the_diary_before_sealing() {
        use crate::wire::Step;
        let fx = Fixture::new();
        let day = "2026-11-26";
        let source: crate::diary::DetailSource = Arc::new(|day: &str, _: &Collection, _: DateTime<Utc>| -> io::Result<DayDetail> {
            Ok(if day == "2026-11-26" { DayDetail { trail: vec![Step { t: "10:01:00".into(), event: "app.abriu".into(),
                level: "ok".into(), screen: None, count: 1 }], ..DayDetail::default() } } else { DayDetail::default() })
        });
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        drop(telemetry);
        std::fs::write(fx.queue().join(format!("{day}.json")), "{").unwrap();
        fx.next_day();
        let telemetry = fx.start_as_is();
        telemetry.set_detail(source);
        telemetry.save().unwrap();
        let file = queue::read_day(fx.home.path(), day).unwrap().expect("o dia volta pelo diário");
        assert!(file.sealed && file.batch_id.is_some());
        assert_eq!(file.detail.trail.len(), 1);
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

    #[tokio::test(start_paused = true)]
    async fn telemetry_detail_only_partial_and_final_keep_the_id_across_restart_and_prune() {
        use crate::wire::Step;
        let fx = Fixture::new();
        let day = "2026-11-26";
        let telemetry = fx.start();
        let count = Arc::new(Mutex::new(1));
        let source = {
            let count = count.clone();
            Arc::new(move |day: &str, _: &Collection, _: DateTime<Utc>| -> io::Result<DayDetail> {
                if day != "2026-11-26" { return Ok(DayDetail::default()); }
                Ok(DayDetail { trail: vec![Step { t: "10:01:00".into(), event: "app.abriu".into(),
                    level: "ok".into(), screen: None, count: *count.lock().unwrap() }], ..DayDetail::default() })
            })
        };
        telemetry.set_detail(source.clone());
        let (post, seen) = receiver(Some(200));
        telemetry.save().unwrap();
        assert!(telemetry.send_partial(&post).await.is_none());
        let id = queue::read_day(fx.home.path(), day).unwrap().unwrap().batch_id.unwrap();
        drop(telemetry);
        let telemetry = fx.start_as_is();
        telemetry.set_detail(source);
        *count.lock().unwrap() = 2;
        telemetry.save().unwrap();
        assert!(telemetry.send_partial(&post).await.is_none());
        *count.lock().unwrap() = 3;
        fx.next_day();
        telemetry.save().unwrap();
        drop(telemetry);
        let telemetry = fx.start_as_is();
        telemetry.set_detail(Arc::new(|_, _, _| panic!("o envio relê o diário depois de congelar")));
        assert!(telemetry.send_closed(&post).await.is_empty());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.iter().map(|b| (b.partial, b.trail[0].count)).collect::<Vec<_>>(), [(true, 1), (true, 2), (false, 3)]);
        assert!(seen.iter().all(|b| b.batch_id == id && b.day == day && b.logs.is_empty() && b.counters.is_empty()));
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

    #[tokio::test(start_paused = true)]
    async fn telemetry_legacy_queue_is_never_imported_and_413_keeps_the_new_batch() {
        use serde_json::json;
        let fx = Fixture::new();
        let legacy = fx.home.path().join(".hangar/telemetry/queue/2026-11-26.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, json!({"day": "2026-11-26", "logs": [{"message": "private_fixture"}]}).to_string()).unwrap();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        fx.next_day();
        telemetry.save().unwrap();
        let (post, seen) = receiver(Some(413));
        assert!(matches!(telemetry.send_closed(&post).await.as_slice(), [Failure::Refused(413)]));
        let sent = seen.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].logs.is_empty());
        let current = queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap();
        assert_eq!(current.batch_id.as_deref(), Some(sent[0].batch_id.as_str()));
        assert!(legacy.exists());
    }

    #[test]
    fn telemetry_body_cap_counts_encoded_bytes_and_preserves_counters() {
        use crate::wire::{MAX_RECORDS, Record};
        let fx = Fixture::new();
        let telemetry = fx.start();
        let counter = Counter::new(Event::MessageSent, message(), None, 7);
        let file = DayFile { day: telemetry.today(), batch_id: Some("f".repeat(32)), counters: vec![counter.clone()], ..DayFile::default() };
        let record = Record { level: "error".into(), event: "http.action".into(), count: 1, first: "10:00:00".into(), last: "10:00:00".into(),
            fields: (0..22).map(|n| (format!("field{n}"), "é\"\\".repeat(150))).collect(), trail: Vec::new() };
        let mut detail = DayDetail { records: vec![record; MAX_RECORDS], ..DayDetail::default() };
        let mut batch = telemetry.batch(&file).unwrap();
        let body = encode_fit(&mut batch, &mut detail, MAX_BODY).unwrap();
        assert!(body.len() <= MAX_BODY);
        assert!(detail.dropped_records > 0);
        let decoded: Batch = serde_json::from_slice(&body).unwrap();
        assert_eq!(decoded.counters[0], counter);
        assert_eq!(decoded.counters[1].count as usize + decoded.records.len(), MAX_RECORDS);
        assert_eq!(decoded.counters[1].event, "diary_dropped");
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
