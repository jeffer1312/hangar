//! Agendamentos duráveis do dono; o host liga a abertura e a entrada das sessões.
#![forbid(unsafe_code)]

mod dispatcher;
mod engine;
mod model;
mod schedule;
mod store;

pub use dispatcher::{DispatchHandle, Dispatcher};
pub use engine::{Clock, Engine, Host, HostFuture, SystemClock};
pub use model::{Attempt, Error, Outcome, Status, Target, Task, TaskInput};
pub use schedule::{Schedule, next_run};
pub use store::Store;
