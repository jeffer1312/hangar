use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::{Id, JoinSet};

use crate::{Attempt, Engine, Error, Outcome, Status};

#[derive(Clone)]
pub struct DispatchHandle {
    engine: Arc<Engine>,
    sender: mpsc::Sender<Attempt>,
}
impl DispatchHandle {
    pub fn run_now(&self, id: &str) -> Result<(), Error> {
        let permit = self
            .sender
            .try_reserve()
            .map_err(|_| Error::unavailable())?;
        let attempt = self.engine.reserve(id, true)?;
        permit.send(attempt);
        Ok(())
    }
}

pub struct Dispatcher {
    engine: Arc<Engine>,
    automatic: bool,
    receiver: mpsc::Receiver<Attempt>,
}
impl Dispatcher {
    pub fn new(engine: Arc<Engine>, automatic: bool) -> (DispatchHandle, Self) {
        let (sender, receiver) = mpsc::channel(256);
        (
            DispatchHandle {
                engine: engine.clone(),
                sender,
            },
            Self {
                engine,
                automatic,
                receiver,
            },
        )
    }
    pub async fn run(mut self) {
        let mut children = JoinSet::new();
        let mut attempts = HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                attempt=self.receiver.recv() => match attempt {Some(attempt)=>self.spawn(&mut children,&mut attempts,attempt),None=>break},
                _=tick.tick(), if self.automatic => {
                    match self.engine.store.list() {
                        Ok(tasks)=>for task in tasks {
                            let due=task.next_run_at.as_deref().and_then(|at|chrono::DateTime::parse_from_rfc3339(at).ok()).is_some_and(|at|at.with_timezone(&chrono::Utc)<=self.engine.now());
                            if task.input.enabled && due {
                                match self.engine.reserve(&task.id,false) {
                                    Ok(attempt)=>self.spawn(&mut children,&mut attempts,attempt),
                                    Err(error) if error.status==409 || error.status==404=>{},
                                    Err(error)=>tracing::warn!(code=error.code,"reserva da tarefa agendada indisponível"),
                                }
                            }
                        },
                        Err(error)=>tracing::warn!(code=error.code,"leitura das tarefas agendadas indisponível"),
                    }
                }
                result=children.join_next_with_id(), if !children.is_empty() => match result {
                    Some(Ok((id,())))=>{ attempts.remove(&id); }
                    Some(Err(error))=>{
                        tracing::warn!(code="scheduled_dispatch_interrupted","envio agendado interrompido; tentativa marcada como incerta");
                        if let Some(attempt)=attempts.remove(&error.id()) { self.interrupted(&attempt); }
                    }
                    None=>{}
                },
            }
        }
    }
    fn spawn(&self, children: &mut JoinSet<()>, attempts: &mut HashMap<Id, Attempt>, attempt: Attempt) {
        let engine = self.engine.clone();
        let child = attempt.clone();
        let id = children
            .spawn(async move {
                engine.dispatch(child).await;
            })
            .id();
        attempts.insert(id, attempt);
    }
    /// O envio parou no meio (pânico do host): sem prova de entrega, a reserva sai como incerta.
    fn interrupted(&self, attempt: &Attempt) {
        let outcome = Outcome {
            status: Status::Uncertain,
            error: Some("O envio foi interrompido. Confira a conversa antes de repetir.".into()),
            session: None,
        };
        let engine = self.engine.clone();
        let attempt = attempt.clone();
        tokio::spawn(async move {
            engine.finish(&attempt, outcome).await;
        });
    }
}
