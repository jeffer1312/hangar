//! Uma instalação, atualização ou remoção por vez, com o andamento em memória para o `GET` da tela.
//!
//! A tarefa roda em segundo plano e sai no JSON `{kind,step,done,error,outcome}`; uma operação que a rota
//! faz na hora ([`Jobs::inline`]) também ocupa a vez.
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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

/// O andamento que a tarefa informa, no formato que as funções de instalação recebem.
pub type Progress<S> = Arc<dyn Fn(S) + Send + Sync>;

struct Slot<S, O, F> {
    job: Option<Job<S, O, F>>,
    /// Uma operação síncrona (remover, apagar com o serviço parado) em curso.
    inline: bool,
}

struct Job<S, O, F> {
    kind: &'static str,
    step: S,
    result: Option<Result<O, F>>,
}

impl<S: Step, O: Outcome, F: Failure> Jobs<S, O, F> {
    /// Começa a tarefa `kind` na etapa `first`. O que volta termina depois de o resultado ficar gravado.
    ///
    /// # Errors
    ///
    /// [`Busy`] com outra tarefa ou operação em curso.
    pub fn start<W, Fut>(&self, kind: &'static str, first: S, work: W) -> Result<tokio::task::JoinHandle<()>, Busy>
    where
        W: FnOnce(Progress<S>) -> Fut,
        Fut: Future<Output = Result<O, F>> + Send + 'static,
    {
        {
            let mut slot = lock(&self.slot);
            if slot.busy() {
                return Err(Busy);
            }
            slot.job = Some(Job { kind, step: first, result: None });
        }
        let progress_slot = self.slot.clone();
        let progress: Progress<S> = Arc::new(move |step| {
            if let Some(job) = lock(&progress_slot).job.as_mut() {
                job.step = step;
            }
        });
        let work = tokio::spawn(work(progress));
        let slot = self.slot.clone();
        Ok(tokio::spawn(async move {
            let result = work.await.unwrap_or_else(|error| Err(F::crashed(kind, &error)));
            if let Some(job) = lock(&slot).job.as_mut() {
                job.result = Some(result);
            }
        }))
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

    /// Há tarefa ou operação em curso agora; quem grava algo antes de [`Jobs::start`] pergunta aqui.
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

impl<S, O, F> Slot<S, O, F> {
    fn busy(&self) -> bool {
        self.inline || self.job.as_ref().is_some_and(|j| j.result.is_none())
    }
}

impl<S, O, F> Default for Jobs<S, O, F> {
    fn default() -> Self {
        Self { slot: Arc::new(Mutex::new(Slot { job: None, inline: false })) }
    }
}

impl<S, O, F> Drop for InlineGuard<'_, S, O, F> {
    fn drop(&mut self) {
        lock(&self.0.slot).inline = false;
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
        let done = jobs.start("install", Stage::Download, |progress| async move {
            progress(Stage::Service);
            let _ = wait.await;
            Err(Fail("x_hash", "x".into()))
        }).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(jobs.inline().err(), Some(Busy));
        assert!(jobs.start("update", Stage::Download, |_| async { Ok(Done) }).is_err());
        assert_eq!(jobs.job(), json!({"kind": "install", "step": "service", "done": false, "error": null, "outcome": null}));
        release.send(()).unwrap();
        done.await.unwrap();
        assert_eq!(jobs.job(), json!({"kind": "install", "step": "service", "done": true,
            "error": {"code": "x_hash", "detail": "x"}, "outcome": null}));
        assert!(jobs.inline().is_ok(), "terminada, a vez está livre");
        jobs.forget();
        assert_eq!(jobs.job(), Value::Null);

        jobs.start("update", Stage::Download, |_| async { Ok(Done) }).unwrap().await.unwrap();
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
        }).unwrap().await.unwrap();
        let job = jobs.job();
        assert_eq!((&job["done"], &job["error"]["code"]), (&json!(true), &json!("x_job_crashed")), "{job}");
        assert!(!jobs.busy());
    }
}
