use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Schedule;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Target {
    New {
        launch: Value,
        worktree: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    Session {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_key: Option<String>,
        /// A conversa da sessão quando a tarefa foi vinculada: o `/clear` troca a conversa e conserva a chave.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskInput {
    pub name: String,
    pub prompt: String,
    pub enabled: bool,
    pub schedule: Schedule,
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub revision: u64,
    #[serde(flatten)]
    pub input: TaskInput,
    pub next_run_at: Option<String>,
    pub last_run_at: Option<String>,
    pub last_status: Status,
    pub last_error: Option<String>,
    pub last_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) attempt: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Never,
    Dispatching,
    Sent,
    Failed,
    Uncertain,
}

#[derive(Clone, Debug)]
pub struct Attempt {
    pub task: Task,
    pub id: String,
    pub session_name: String,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub status: Status,
    pub error: Option<String>,
    pub session: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for Error {}
impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: "scheduled_task_invalid",
            message: message.into(),
        }
    }
    pub fn request_id() -> Self {
        Self {
            status: 400,
            code: "scheduled_task_invalid",
            message: "O request_id deve ter de 1 a 64 letras, números, _ ou -.".into(),
        }
    }
    pub fn unavailable() -> Self {
        Self {
            status: 503,
            code: "scheduled_tasks_unavailable",
            message: "O armazenamento das tarefas está indisponível.".into(),
        }
    }
    pub(crate) fn missing() -> Self {
        Self {
            status: 404,
            code: "scheduled_task_not_found",
            message: "A tarefa não existe mais.".into(),
        }
    }
    pub(crate) fn conflict() -> Self {
        Self {
            status: 409,
            code: "scheduled_revision_conflict",
            message: "A tarefa mudou. Atualize a lista antes de tentar novamente.".into(),
        }
    }
}
