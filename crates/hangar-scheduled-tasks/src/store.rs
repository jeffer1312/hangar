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
        let mut connection = Connection::open(path).map_err(|_| Error::unavailable())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| Error::unavailable())?;
        connection.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS scheduled_tasks (id TEXT PRIMARY KEY, body TEXT NOT NULL);")
            .map_err(|_| Error::unavailable())?;
        // Arquivo criado antes do request_id ganha a coluna; a trava evita duas aberturas somando a mesma.
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Error::unavailable())?;
        let has_request_id = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('scheduled_tasks') WHERE name = 'request_id'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| Error::unavailable())?
            > 0;
        if !has_request_id {
            tx.execute_batch("ALTER TABLE scheduled_tasks ADD COLUMN request_id TEXT;")
                .map_err(|_| Error::unavailable())?;
        }
        tx.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS scheduled_tasks_request_id ON scheduled_tasks(request_id) WHERE request_id IS NOT NULL;")
            .map_err(|_| Error::unavailable())?;
        tx.commit().map_err(|_| Error::unavailable())?;
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
        self.create_with_request(input, None, now, zone)
    }

    /// A tarefa já criada com este `request_id`; a resposta passa por `repeated`.
    pub fn find_request(&self, request_id: &str) -> Result<Option<Task>, Error> {
        validate_request_id(request_id)?;
        let connection = self.connection.lock().map_err(|_| Error::unavailable())?;
        read_request(&connection, request_id)
    }

    /// Resposta perdida não duplica: o mesmo `request_id` devolve a tarefa que ele já criou.
    pub fn create_with_request<T: TimeZone>(
        &self,
        input: TaskInput,
        request_id: Option<&str>,
        now: DateTime<Utc>,
        zone: &T,
    ) -> Result<Task, Error> {
        if let Some(request_id) = request_id {
            validate_request_id(request_id)?;
        }
        self.transaction(|tx| {
            if let Some(request_id) = request_id
                && let Some(task) = read_request(tx, request_id)?
            {
                return repeated(task, &input);
            }
            validate(&input)?;
            require_future_once(&input, now)?;
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
            let count: i64 = tx
                .query_row("SELECT COUNT(*) FROM scheduled_tasks", [], |row| row.get(0))
                .map_err(|_| Error::unavailable())?;
            if count >= 256 {
                return Err(Error::invalid("O servidor já tem 256 tarefas."));
            }
            write(tx, &task)?;
            if let Some(request_id) = request_id {
                tx.execute(
                    "UPDATE scheduled_tasks SET request_id=?2 WHERE id=?1",
                    params![task.id, request_id],
                )
                .map_err(|_| Error::unavailable())?;
            }
            Ok(task)
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
            // Tarefa única já passada continua editável enquanto a data não muda.
            let rescheduled = !same_schedule(&task.input.schedule, &input.schedule);
            if rescheduled {
                require_future_once(&input, now)?;
            }
            if rescheduled || task.input.enabled != input.enabled {
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
                // "Executar agora" antes da data não cancela a execução marcada.
                Schedule::Once { .. } if manual => task
                    .next_run_at
                    .take()
                    .filter(|at| parse_instant(at).is_ok_and(|at| at > now)),
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
            // Resultado sem sessão conserva a que a reserva gravou, salvo a conversa nova que comprovadamente não abriu.
            if outcome.session.is_some() {
                task.last_session = outcome.session;
            } else if outcome.status == Status::Failed
                && matches!(task.input.target, Target::New { .. })
            {
                task.last_session = None;
            }
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
/// Os clientes escrevem o mesmo instante com fusos e frações diferentes: compara o instante, não o texto.
fn same_schedule(a: &Schedule, b: &Schedule) -> bool {
    match (a, b) {
        (Schedule::Once { at: a }, Schedule::Once { at: b }) => {
            a == b || matches!((parse_instant(a), parse_instant(b)), (Ok(a), Ok(b)) if a == b)
        }
        _ => a == b,
    }
}
fn require_future_once(input: &TaskInput, now: DateTime<Utc>) -> Result<(), Error> {
    if let Schedule::Once { at } = &input.schedule
        && parse_instant(at)? <= now
    {
        return Err(Error::invalid(
            "Escolha uma data futura para executar uma vez.",
        ));
    }
    Ok(())
}
fn validate_request_id(request_id: &str) -> Result<(), Error> {
    if request_id.is_empty()
        || request_id.len() > 64
        || !request_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::request_id());
    }
    Ok(())
}
fn read_request(connection: &Connection, request_id: &str) -> Result<Option<Task>, Error> {
    let body: Option<String> = connection
        .query_row(
            "SELECT body FROM scheduled_tasks WHERE request_id=?1",
            [request_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| Error::unavailable())?;
    body.map(|body| serde_json::from_str(&body).map_err(|_| Error::unavailable()))
        .transpose()
}
fn check_revision(task: &Task, revision: u64) -> Result<(), Error> {
    if task.revision == revision {
        Ok(())
    } else {
        Err(Error::conflict())
    }
}
/// A repetição só devolve a tarefa do mesmo pedido; a sessão preparada ganhou chave e conversa que o cliente não manda.
pub(crate) fn repeated(task: Task, input: &TaskInput) -> Result<Task, Error> {
    let stored = &task.input;
    let target = match (&stored.target, &input.target) {
        (
            Target::Session {
                name: a,
                session_key: key_a,
                ..
            },
            Target::Session {
                name: b,
                session_key: key_b,
                ..
            },
        ) => a == b && (key_b.is_none() || key_a == key_b),
        (a, b) => a == b,
    };
    if target
        && stored.name == input.name
        && stored.prompt == input.prompt
        && stored.enabled == input.enabled
        && same_schedule(&stored.schedule, &input.schedule)
    {
        Ok(task)
    } else {
        Err(Error::request_conflict())
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
        Target::Session {
            name,
            session_key,
            conversation,
        } => {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || session_key
                    .as_ref()
                    .is_none_or(|key| key.is_empty() || key.len() > 128)
                || conversation
                    .as_ref()
                    .is_some_and(|id| id.is_empty() || id.len() > 256 || id.contains('\0'))
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
