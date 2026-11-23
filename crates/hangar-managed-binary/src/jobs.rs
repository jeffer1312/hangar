//! Uma instalação, atualização ou remoção por vez, com o andamento em memória para o `GET` da tela.
//!
//! A tarefa roda em segundo plano e sai no JSON `{kind,step,done,error,outcome}`; uma operação que a rota
//! faz na hora ([`Jobs::inline`]) também ocupa a vez.
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use serde_json::{Value, json};

/// Etapa da tarefa, com o nome que vai em `step`.
pub trait Step: Copy + Send + 'static {
    fn name(self) -> &'static str;
}

/// Sucesso da tarefa, com o nome que vai em `outcome`.
pub trait Outcome: Send + 'static {
    fn name(&self) -> &'static str;
}

/// Falha da tarefa, que vai em `error` como `{code,detail}`.
pub trait Failure: Send + 'static {
    fn code(&self) -> &str;
    fn detail(&self) -> &str;
    /// A tarefa caiu no meio: sem um resultado, toda tentativa seguinte veria "ocupado".
    fn crashed(kind: &'static str, error: &tokio::task::JoinError) -> Self;
}

/// Outra tarefa ou operação em curso.
#[derive(Debug, PartialEq, Eq)]
pub struct Busy;

pub struct Jobs<S, O, F> {
    slot: Arc<Mutex<Slot<S, O, F>>>,
}

/// Enquanto viver, nenhuma tarefa começa.
pub struct InlineGuard<'a, S, O, F>(&'a Jobs<S, O, F>);

/// A vez guardada para uma tarefa que ainda vai começar ([`Reservation::start`]); solta sem começar, libera.
pub struct Reservation<S, O, F> {
    slot: Arc<Mutex<Slot<S, O, F>>>,
    kind: &'static str,
    first: S,
    started: bool,
}

/// Fica pronto quando o resultado da tarefa está gravado. Soltar sem esperar não cancela nada.
pub struct Done(tokio::sync::oneshot::Receiver<()>);

/// O andamento que a tarefa informa, no formato que as funções de instalação recebem.
pub type Progress<S> = Arc<dyn Fn(S) + Send + Sync>;

struct Slot<S, O, F> {
    job: Option<Job<S, O, F>>,
    /// Uma operação síncrona (remover, apagar com o serviço parado) em curso.
    inline: bool,
    /// Uma [`Reservation`] viva que ainda não começou.
    reserved: bool,
    /// Identidade da última tarefa: o andamento de uma tarefa velha não mexe na seguinte.
    last_id: u64,
}

struct Job<S, O, F> {
    id: u64,
    kind: &'static str,
    step: S,
    result: Option<Result<O, F>>,
}

impl<S: Step, O: Outcome, F: Failure> Jobs<S, O, F> {
    /// Começa a tarefa `kind` na etapa `first`: [`Jobs::reserve`] seguido de [`Reservation::start`].
    ///
    /// # Errors
    ///
    /// [`Busy`] com outra tarefa ou operação em curso.
    pub fn start<W, Fut>(&self, kind: &'static str, first: S, work: W) -> Result<Done, Busy>
    where
        W: FnOnce(Progress<S>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<O, F>> + Send + 'static,
    {
        Ok(self.reserve(kind, first)?.start(work))
    }

    /// Guarda a vez para a tarefa `kind`, para quem grava algo antes de ela começar: com a vez guardada,
    /// nada mais começa até a tarefa começar ou a reserva ser solta.
    ///
    /// # Errors
    ///
    /// [`Busy`] com outra tarefa ou operação em curso.
    pub fn reserve(&self, kind: &'static str, first: S) -> Result<Reservation<S, O, F>, Busy> {
        let mut slot = lock(&self.slot);
        if slot.busy() {
            return Err(Busy);
        }
        slot.reserved = true;
        Ok(Reservation { slot: self.slot.clone(), kind, first, started: false })
    }

    /// Reserva a vez para uma operação que a rota faz na hora.
    ///
    /// # Errors
    ///
    /// [`Busy`] com outra tarefa ou operação em curso.
    pub fn inline(&self) -> Result<InlineGuard<'_, S, O, F>, Busy> {
        let mut slot = lock(&self.slot);
        if slot.busy() {
            return Err(Busy);
        }
        slot.inline = true;
        Ok(InlineGuard(self))
    }

    /// Há tarefa, reserva ou operação em curso agora.
    pub fn busy(&self) -> bool {
        lock(&self.slot).busy()
    }

    /// Esquece a última tarefa terminada: depois de remover, o resultado da instalação não vale mais.
    pub fn forget(&self) {
        let mut slot = lock(&self.slot);
        if slot.job.as_ref().is_some_and(|j| j.result.is_some()) {
            slot.job = None;
        }
    }

    /// `{kind,step,done,error,outcome}` ou `null`.
    pub fn job(&self) -> Value {
        let slot = lock(&self.slot);
        let Some(job) = slot.job.as_ref() else { return Value::Null };
        let (done, error, outcome) = match &job.result {
            None => (false, Value::Null, Value::Null),
            Some(Ok(outcome)) => (true, Value::Null, json!(outcome.name())),
            Some(Err(f)) => (true, json!({"code": f.code(), "detail": f.detail()}), Value::Null),
        };
        json!({"kind": job.kind, "step": job.step.name(), "done": done, "error": error, "outcome": outcome})
    }
}

impl<S: Step, O: Outcome, F: Failure> Reservation<S, O, F> {
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// Começa a tarefa na vez guardada. `work` roda dentro da tarefa vigiada: pânico nele, ou no que ele
    /// devolve, vira [`Failure::crashed`] e libera a vez.
    pub fn start<W, Fut>(mut self, work: W) -> Done
    where
        W: FnOnce(Progress<S>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<O, F>> + Send + 'static,
    {
        self.started = true;
        let kind = self.kind;
        let id = {
            let mut slot = lock(&self.slot);
            slot.reserved = false;
            slot.last_id += 1;
            let id = slot.last_id;
            slot.job = Some(Job { id, kind, step: self.first, result: None });
            id
        };
        let progress_slot = self.slot.clone();
        let progress: Progress<S> = Arc::new(move |step| {
            if let Some(job) = lock(&progress_slot).job.as_mut().filter(|j| j.id == id && j.result.is_none()) {
                job.step = step;
            }
        });
        let work = tokio::spawn(async move { work(progress).await });
        let slot = self.slot.clone();
        let (finished, done) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = work.await.unwrap_or_else(|error| Err(F::crashed(kind, &error)));
            if let Some(job) = lock(&slot).job.as_mut().filter(|j| j.id == id) {
                job.result = Some(result);
            }
            let _ = finished.send(());
        });
        Done(done)
    }
}

impl Future for Done {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        Pin::new(&mut self.0).poll(cx).map(drop)
    }
}

impl<S, O, F> Slot<S, O, F> {
    fn busy(&self) -> bool {
        self.inline || self.reserved || self.job.as_ref().is_some_and(|j| j.result.is_none())
    }
}

impl<S, O, F> Default for Jobs<S, O, F> {
    fn default() -> Self {
        Self { slot: Arc::new(Mutex::new(Slot { job: None, inline: false, reserved: false, last_id: 0 })) }
    }
}

impl<S, O, F> Drop for InlineGuard<'_, S, O, F> {
    fn drop(&mut self) {
        lock(&self.0.slot).inline = false;
    }
}

impl<S, O, F> Drop for Reservation<S, O, F> {
    fn drop(&mut self) {
        if !self.started {
            lock(&self.slot).reserved = false;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    enum Stage {
        Download,
        Service,
    }

    struct Done;

    #[derive(Debug)]
    struct Fail(&'static str, String);

    type TestJobs = Jobs<Stage, Done, Fail>;

    impl Step for Stage {
        fn name(self) -> &'static str {
            match self {
                Self::Download => "download",
                Self::Service => "service",
            }
        }
    }

    impl Outcome for Done {
        fn name(&self) -> &'static str {
            "installed"
        }
    }

    impl Failure for Fail {
        fn code(&self) -> &str {
            self.0
        }

        fn detail(&self) -> &str {
            &self.1
        }

        fn crashed(_kind: &'static str, error: &tokio::task::JoinError) -> Self {
            Fail("x_job_crashed", format!("a tarefa parou no meio: {error}"))
        }
    }

    #[tokio::test]
    async fn one_job_or_inline_operation_at_a_time() {
        let jobs = TestJobs::default();
        assert!(!jobs.busy());
        let guard = jobs.inline().unwrap();
        assert_eq!(jobs.inline().err(), Some(Busy));
        assert!(jobs.start("install", Stage::Download, |_| async { Ok(Done) }).is_err(), "operação na hora ocupa a vez");
        assert!(jobs.busy());
        drop(guard);

        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let (published, service) = tokio::sync::oneshot::channel::<()>();
        let done = jobs.start("install", Stage::Download, |progress| async move {
            progress(Stage::Service);
            let _ = published.send(());
            let _ = wait.await;
            Err(Fail("x_hash", "x".into()))
        }).unwrap();
        service.await.unwrap();
        assert_eq!(jobs.inline().err(), Some(Busy));
        assert!(jobs.start("update", Stage::Download, |_| async { Ok(Done) }).is_err());
        assert_eq!(jobs.job(), json!({"kind": "install", "step": "service", "done": false, "error": null, "outcome": null}));
        release.send(()).unwrap();
        done.await;
        assert_eq!(jobs.job(), json!({"kind": "install", "step": "service", "done": true,
            "error": {"code": "x_hash", "detail": "x"}, "outcome": null}));
        assert!(jobs.inline().is_ok(), "terminada, a vez está livre");
        jobs.forget();
        assert_eq!(jobs.job(), Value::Null);

        jobs.start("update", Stage::Download, |_| async { Ok(Done) }).unwrap().await;
        assert_eq!(jobs.job(), json!({"kind": "update", "step": "download", "done": true, "error": null, "outcome": "installed"}));
    }

    #[tokio::test]
    async fn a_job_that_crashes_still_finishes() {
        let jobs = TestJobs::default();
        jobs.start("install", Stage::Download, |_| async {
            if true {
                panic!("caiu");
            }
            Ok(Done)
        }).unwrap().await;
        let job = jobs.job();
        assert_eq!((&job["done"], &job["error"]["code"]), (&json!(true), &json!("x_job_crashed")), "{job}");
        assert!(!jobs.busy());
    }

    /// O pânico de quem monta o trabalho, antes de haver o que esperar, também termina a tarefa.
    #[tokio::test]
    async fn a_work_factory_that_panics_still_finishes() {
        let jobs = TestJobs::default();
        jobs.start("install", Stage::Download, |_| {
            if true {
                panic!("fábrica");
            }
            async { Ok(Done) }
        }).unwrap().await;
        assert_eq!(jobs.job()["error"]["code"], "x_job_crashed");
        assert!(!jobs.busy());
    }

    /// Quem guardou a vez grava antes de começar; soltar a reserva sem começar libera a vez sem tarefa.
    #[tokio::test]
    async fn a_reservation_holds_the_slot_until_it_starts_or_is_dropped() {
        let jobs = TestJobs::default();
        let reservation = jobs.reserve("install", Stage::Download).unwrap();
        assert_eq!(reservation.kind(), "install");
        assert!(jobs.busy());
        assert!(jobs.inline().is_err(), "a reserva ocupa a vez");
        assert!(jobs.reserve("update", Stage::Download).is_err());
        assert_eq!(jobs.job(), Value::Null, "reservar não publica tarefa");
        drop(reservation);
        assert!(!jobs.busy(), "solta sem começar, libera");

        jobs.reserve("install", Stage::Download).unwrap().start(|_| async { Ok(Done) }).await;
        assert_eq!(jobs.job()["outcome"], "installed");
        assert!(!jobs.busy());
    }

    /// O andamento informado por uma tarefa que já terminou não muda a etapa da seguinte.
    #[tokio::test]
    async fn progress_from_an_older_job_does_not_touch_the_next_one() {
        let jobs = TestJobs::default();
        let kept: Arc<Mutex<Option<Progress<Stage>>>> = Arc::default();
        let keep = kept.clone();
        jobs.start("install", Stage::Download, move |progress| async move {
            *lock(&keep) = Some(progress);
            Ok(Done)
        }).unwrap().await;
        let old = lock(&kept).take().unwrap();
        old(Stage::Service);
        assert_eq!(jobs.job()["step"], "download", "terminada, a etapa não muda");

        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let done = jobs.start("update", Stage::Download, |_| async move {
            let _ = wait.await;
            Ok(Done)
        }).unwrap();
        old(Stage::Service);
        assert_eq!(jobs.job()["step"], "download", "o andamento velho não é da tarefa nova");
        release.send(()).unwrap();
        done.await;
    }
}
