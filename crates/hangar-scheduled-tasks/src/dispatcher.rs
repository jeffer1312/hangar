use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::{Attempt, Engine, Error};

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
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                attempt=self.receiver.recv() => match attempt {Some(attempt)=>self.spawn(&mut children,attempt),None=>break},
                _=tick.tick(), if self.automatic => {
                    match self.engine.store.list() {
                        Ok(tasks)=>for task in tasks {
                            let due=task.next_run_at.as_deref().and_then(|at|chrono::DateTime::parse_from_rfc3339(at).ok()).is_some_and(|at|at.with_timezone(&chrono::Utc)<=self.engine.now());
                            if task.input.enabled && due {
                                match self.engine.reserve(&task.id,false) {
                                    Ok(attempt)=>self.spawn(&mut children,attempt),
                                    Err(error) if error.status==409 || error.status==404=>{},
                                    Err(error)=>tracing::warn!(code=error.code,"reserva da tarefa agendada indisponível"),
                                }
                            }
                        },
                        Err(error)=>tracing::warn!(code=error.code,"leitura das tarefas agendadas indisponível"),
                    }
                }
                result=children.join_next(), if !children.is_empty() => {
                    if let Some(Err(_))=result { tracing::warn!(code="scheduled_dispatch_interrupted","envio agendado interrompido; reserva conservada"); }
                }
            }
        }
    }
    fn spawn(&self, children: &mut JoinSet<()>, attempt: Attempt) {
        let engine = self.engine.clone();
        children.spawn(async move {
            if let Err(error) = engine.dispatch(attempt).await {
                tracing::warn!(
                    code = error.code,
                    "resultado da tarefa agendada não gravado; reserva conservada"
                );
            }
        });
    }
}
