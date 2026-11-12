use std::fs::OpenOptions;
use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::schedule::{parse_instant, validate_schedule};
use crate::{Attempt, Error, Outcome, Schedule, Status, Target, Task, TaskInput, next_run};

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let parent = path.parent().ok_or_else(Error::unavailable)?;
        std::fs::create_dir_all(parent).map_err(|_| Error::unavailable())?;
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path).map_err(|_| Error::unavailable())?;
        let connection = Connection::open(path).map_err(|_| Error::unavailable())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| Error::unavailable())?;
        connection.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS scheduled_tasks (id TEXT PRIMARY KEY, body TEXT NOT NULL);")
            .map_err(|_| Error::unavailable())?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn list(&self) -> Result<Vec<Task>, Error> {
        let connection = self.connection.lock().map_err(|_| Error::unavailable())?;
        read_all(&connection)
    }

    pub fn create<T: TimeZone>(
        &self,
        input: TaskInput,
        now: DateTime<Utc>,
        zone: &T,
    ) -> Result<Task, Error> {
        validate(&input)?;
        if let Schedule::Once { at } = &input.schedule
            && parse_instant(at)? <= now
        {
            return Err(Error::invalid(
                "Escolha uma data futura para executar uma vez.",
            ));
        }
        let task = Task {
            id: hangar_core::ids::random_hex(16),
            revision: 1,
            next_run_at: scheduled(&input, now, zone)?,
            input,
            last_run_at: None,
            last_status: Status::Never,
            last_error: None,
            last_session: None,
            attempt: None,
        };
        self.transaction(|tx| {
            let count: i64 = tx
                .query_row("SELECT COUNT(*) FROM scheduled_tasks", [], |row| row.get(0))
                .map_err(|_| Error::unavailable())?;
            if count >= 256 {
                return Err(Error::invalid("O servidor já tem 256 tarefas."));
            }
            write(tx, &task)?;
            Ok(task.clone())
        })
    }

    pub fn update<T: TimeZone>(
        &self,
        id: &str,
        input: TaskInput,
        revision: u64,
        now: DateTime<Utc>,
        zone: &T,
    ) -> Result<Task, Error> {
        validate(&input)?;
        self.transaction(|tx| {
            let mut task = read(tx, id)?;
            check_revision(&task, revision)?;
            if task.input.schedule != input.schedule || task.input.enabled != input.enabled {
                task.next_run_at = scheduled(&input, now, zone)?;
            }
            task.input = input;
            task.revision += 1;
            write(tx, &task)?;
            Ok(task)
        })
    }

    pub fn set_enabled<T: TimeZone>(
        &self,
        id: &str,
        enabled: bool,
        revision: u64,
        now: DateTime<Utc>,
        zone: &T,
    ) -> Result<Task, Error> {
        self.transaction(|tx| {
            let mut task = read(tx, id)?;
            check_revision(&task, revision)?;
            if task.input.enabled != enabled {
                task.input.enabled = enabled;
                task.next_run_at = scheduled(&task.input, now, zone)?;
            }
            task.revision += 1;
            write(tx, &task)?;
            Ok(task)
        })
    }

    pub fn delete(&self, id: &str, revision: u64) -> Result<(), Error> {
        self.transaction(|tx| {
            check_revision(&read(tx, id)?, revision)?;
            tx.execute("DELETE FROM scheduled_tasks WHERE id=?1", [id])
                .map_err(|_| Error::unavailable())?;
            Ok(())
        })
    }

    pub fn reserve<T: TimeZone>(
        &self,
        id: &str,
        now: DateTime<Utc>,
        zone: &T,
        manual: bool,
    ) -> Result<Attempt, Error> {
        self.transaction(|tx| {
            let mut task = read(tx, id)?;
            if task.last_status == Status::Dispatching {
                return Err(Error {
                    status: 409,
                    code: "scheduled_task_busy",
                    message: "Esta tarefa já está sendo enviada.".into(),
                });
            }
            if !manual
                && (!task.input.enabled
                    || !task
                        .next_run_at
                        .as_ref()
                        .is_some_and(|at| parse_instant(at).is_ok_and(|at| at <= now)))
            {
                return Err(Error {
                    status: 409,
                    code: "scheduled_task_not_due",
                    message: "Esta tarefa ainda não tem uma ocorrência pendente.".into(),
                });
            }
            let id = hangar_core::ids::random_hex(16);
            task.attempt = Some(id.clone());
            task.last_run_at = Some(now.to_rfc3339());
            task.last_status = Status::Dispatching;
            task.last_error = None;
            task.revision += 1;
            // A próxima ocorrência parte da volta do servidor; intervalos perdidos não formam uma fila.
            task.next_run_at = match task.input.schedule {
                Schedule::Once { .. } => None,
                _ => scheduled(&task.input, now, zone)?,
            };
            let session_name = format!("task-{}-{}", &task.id[..8], &id[..12]);
            task.last_session = Some(match &task.input.target {
                Target::Session { name, .. } => name.clone(),
                Target::New { .. } => session_name.clone(),
            });
            write(tx, &task)?;
            Ok(Attempt {
                task,
                id,
                session_name,
            })
        })
    }

    pub fn finish(&self, attempt: &Attempt, outcome: Outcome) -> Result<bool, Error> {
        if !matches!(
            outcome.status,
            Status::Sent | Status::Failed | Status::Uncertain
        ) {
            return Err(Error::invalid("Resultado de envio inválido."));
        }
        self.transaction(|tx| {
            let mut task = match read(tx, &attempt.task.id) {
                Ok(task) => task,
                Err(error) if error.status == 404 => return Ok(false),
                Err(error) => return Err(error),
            };
            if task.last_status != Status::Dispatching
                || task.attempt.as_deref() != Some(&attempt.id)
            {
                return Ok(false);
            }
            task.last_status = outcome.status;
            task.last_error = outcome.error.map(|s| s.chars().take(512).collect());
            task.last_session = outcome.session;
            task.attempt = None;
            task.revision += 1;
            write(tx, &task)?;
            Ok(true)
        })
    }

    pub fn recover(&self) -> Result<(), Error> {
        self.transaction(|tx| {
            for mut task in read_all(tx)? {
                if task.last_status == Status::Dispatching {
                    task.last_status = Status::Uncertain;
                    task.last_error = Some(
                        "O servidor parou durante o envio. Confira a conversa antes de repetir."
                            .into(),
                    );
                    task.attempt = None;
                    task.revision += 1;
                    write(tx, &task)?;
                }
            }
            Ok(())
        })
    }

    fn transaction<R>(
        &self,
        run: impl FnOnce(&Transaction<'_>) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let mut connection = self.connection.lock().map_err(|_| Error::unavailable())?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Error::unavailable())?;
        let result = run(&tx)?;
        tx.commit().map_err(|_| Error::unavailable())?;
        Ok(result)
    }
}

fn scheduled<T: TimeZone>(
    input: &TaskInput,
    now: DateTime<Utc>,
    zone: &T,
) -> Result<Option<String>, Error> {
    if input.enabled {
        Ok(next_run(&input.schedule, now, zone)?.map(|at| at.to_rfc3339()))
    } else {
        Ok(None)
    }
}
fn check_revision(task: &Task, revision: u64) -> Result<(), Error> {
    if task.revision == revision {
        Ok(())
    } else {
        Err(Error::conflict())
    }
}
fn read(connection: &Connection, id: &str) -> Result<Task, Error> {
    let body: Option<String> = connection
        .query_row(
            "SELECT body FROM scheduled_tasks WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| Error::unavailable())?;
    serde_json::from_str(&body.ok_or_else(Error::missing)?).map_err(|_| Error::unavailable())
}
fn read_all(connection: &Connection) -> Result<Vec<Task>, Error> {
    let mut statement = connection
        .prepare("SELECT body FROM scheduled_tasks ORDER BY rowid")
        .map_err(|_| Error::unavailable())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| Error::unavailable())?;
    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(
            serde_json::from_str(&row.map_err(|_| Error::unavailable())?)
                .map_err(|_| Error::unavailable())?,
        );
    }
    Ok(tasks)
}
fn write(tx: &Transaction<'_>, task: &Task) -> Result<(), Error> {
    let body = serde_json::to_string(task).map_err(|_| Error::unavailable())?;
    tx.execute("INSERT INTO scheduled_tasks(id,body) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET body=excluded.body",params![task.id,body]).map_err(|_| Error::unavailable())?;
    Ok(())
}
fn validate(input: &TaskInput) -> Result<(), Error> {
    if input.name.trim().is_empty()
        || input.name.chars().count() > 128
        || input.name.chars().any(char::is_control)
    {
        return Err(Error::invalid("Informe um nome com até 128 caracteres."));
    }
    if input.prompt.trim().is_empty()
        || input.prompt.len() > 128 * 1024
        || input.prompt.contains('\0')
    {
        return Err(Error::invalid(
            "Informe um pedido com até 128 KiB, sem caractere nulo.",
        ));
    }
    validate_schedule(&input.schedule)?;
    match &input.target {
        Target::Session { name, session_key } => {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || session_key
                    .as_ref()
                    .is_none_or(|key| key.is_empty() || key.len() > 128)
            {
                return Err(Error::invalid(
                    "O destino precisa de uma sessão viva e sua identidade.",
                ));
            }
        }
        Target::New { launch, base, .. } => {
            let allowed = [
                "cwd",
                "provider",
                "config_dir",
                "codex_account",
                "engine",
                "engine_account",
                "model",
                "effort",
                "service_tier",
                "permission_mode",
                "subagent_model",
                "claude_customizations",
                "jev",
                "omp_profile",
                "read_only",
                "headless",
            ];
            let Some(object) = launch.as_object() else {
                return Err(Error::invalid(
                    "As opções da conversa precisam ser um objeto.",
                ));
            };
            if object.keys().any(|key| !allowed.contains(&key.as_str()))
                || serde_json::to_vec(launch)
                    .map_err(|_| Error::invalid("Opções inválidas."))?
                    .len()
                    > 32 * 1024
                || !launch["cwd"].as_str().is_some_and(|cwd| {
                    !cwd.trim().is_empty() && cwd.len() <= 4096 && !cwd.contains('\0')
                })
                || base.as_ref().is_some_and(|base| {
                    base.len() > 256 || base.trim().is_empty() || base.contains('\0')
                })
            {
                return Err(Error::invalid(
                    "As opções contêm campos internos ou inválidos.",
                ));
            }
        }
    }
    Ok(())
}
