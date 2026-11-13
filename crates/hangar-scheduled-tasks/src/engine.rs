use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::{Attempt, Error, Outcome, Store, Target, Task, TaskInput};

pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Host: Send + Sync {
    fn prepare_target(&self, target: Target) -> HostFuture<'_, Result<Target, Error>>;
    fn dispatch(&self, attempt: Attempt) -> HostFuture<'_, Outcome>;
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub struct Engine {
    pub store: Arc<Store>,
    pub timezone: Tz,
    host: Arc<dyn Host>,
    clock: Arc<dyn Clock>,
}

impl Engine {
    pub fn new(
        store: Arc<Store>,
        timezone: Tz,
        host: Arc<dyn Host>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            timezone,
            host,
            clock,
        }
    }
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }
    pub async fn create(&self, input: TaskInput) -> Result<Task, Error> {
        self.create_with_request(input, None).await
    }
    /// A repetição responde antes de conferir o destino, que pode ter mudado desde a primeira criação.
    pub async fn create_with_request(
        &self,
        mut input: TaskInput,
        request_id: Option<&str>,
    ) -> Result<Task, Error> {
        if let Some(request_id) = request_id
            && let Some(task) = self.store.find_request(request_id)?
        {
            return Ok(task);
        }
        input.target = self.host.prepare_target(input.target).await?;
        self.store
            .create_with_request(input, request_id, self.now(), &self.timezone)
    }
    pub async fn update(
        &self,
        id: &str,
        mut input: TaskInput,
        revision: u64,
    ) -> Result<Task, Error> {
        input.target = self.host.prepare_target(input.target).await?;
        self.store
            .update(id, input, revision, self.now(), &self.timezone)
    }
    pub fn reserve(&self, id: &str, manual: bool) -> Result<Attempt, Error> {
        self.store.reserve(id, self.now(), &self.timezone, manual)
    }
    pub async fn dispatch(&self, attempt: Attempt) -> Result<bool, Error> {
        let outcome = self.host.dispatch(attempt.clone()).await;
        self.store.finish(&attempt, outcome)
    }
}
