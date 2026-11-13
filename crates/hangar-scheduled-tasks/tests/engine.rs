use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use hangar_scheduled_tasks::{
    Attempt, Clock, Engine, Error, Host, HostFuture, Outcome, Schedule, Status, Store, Target,
    TaskInput,
};

struct FixedClock(Mutex<DateTime<Utc>>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}
struct RecordingHost {
    calls: Mutex<Vec<String>>,
}
impl Host for RecordingHost {
    fn prepare_target(&self, target: Target) -> HostFuture<'_, Result<Target, Error>> {
        Box::pin(async move { Ok(target) })
    }
    fn dispatch(&self, attempt: Attempt) -> HostFuture<'_, Outcome> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(attempt.id);
            Outcome {
                status: Status::Uncertain,
                error: Some("A resposta do transporte não chegou.".into()),
                session: Some(attempt.session_name),
            }
        })
    }
}
#[tokio::test]
async fn engine_transport_uncertainty_is_not_resent_and_new_occurrences_collapse() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&directory.path().join("tasks.sqlite")).unwrap());
    let now = DateTime::parse_from_rfc3339("2026-11-12T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let clock = Arc::new(FixedClock(Mutex::new(now)));
    let host = Arc::new(RecordingHost {
        calls: Mutex::new(vec![]),
    });
    let engine = Engine::new(
        store,
        chrono_tz::America::Sao_Paulo,
        host.clone(),
        clock.clone(),
    );
    let task = engine
        .create(TaskInput {
            name: "Revisão".into(),
            prompt: "Revise o projeto.".into(),
            enabled: true,
            schedule: Schedule::Interval { minutes: 5 },
            target: Target::New {
                launch: serde_json::json!({"cwd":"/project"}),
                worktree: false,
                base: None,
            },
        })
        .await
        .unwrap();
    *clock.0.lock().unwrap() = now + chrono::Duration::hours(8);
    let attempt = engine.reserve(&task.id, false).unwrap();
    engine.dispatch(attempt).await.unwrap();
    assert_eq!(host.calls.lock().unwrap().len(), 1);
    assert_eq!(
        engine.store.list().unwrap()[0].last_status,
        Status::Uncertain
    );
    assert!(engine.reserve(&task.id, false).is_err());
    *clock.0.lock().unwrap() = now + chrono::Duration::hours(9);
    let attempt = engine.reserve(&task.id, false).unwrap();
    engine.dispatch(attempt).await.unwrap();
    assert_eq!(host.calls.lock().unwrap().len(), 2);
    assert_eq!(
        engine.store.list().unwrap()[0].next_run_at.as_deref(),
        Some("2026-11-12T21:05:00+00:00")
    );
}
struct PanickingHost;
impl Host for PanickingHost {
    fn prepare_target(&self, target: Target) -> HostFuture<'_, Result<Target, Error>> {
        Box::pin(async move { Ok(target) })
    }
    fn dispatch(&self, attempt: Attempt) -> HostFuture<'_, Outcome> {
        Box::pin(async move {
            assert!(attempt.id.is_empty(), "host interrompido");
            Outcome {
                status: Status::Sent,
                error: None,
                session: None,
            }
        })
    }
}
#[tokio::test]
async fn dispatcher_panicking_host_releases_the_reservation_as_uncertain() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&directory.path().join("tasks.sqlite")).unwrap());
    let now = DateTime::parse_from_rfc3339("2026-11-12T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let engine = Arc::new(Engine::new(
        store,
        chrono_tz::UTC,
        Arc::new(PanickingHost),
        Arc::new(FixedClock(Mutex::new(now))),
    ));
    let task = engine
        .create(TaskInput {
            name: "Revisão".into(),
            prompt: "Revise o projeto.".into(),
            enabled: true,
            schedule: Schedule::Interval { minutes: 5 },
            target: Target::New {
                launch: serde_json::json!({"cwd":"/project"}),
                worktree: false,
                base: None,
            },
        })
        .await
        .unwrap();
    let (handle, dispatcher) = hangar_scheduled_tasks::Dispatcher::new(engine.clone(), false);
    let worker = tokio::spawn(dispatcher.run());
    handle.run_now(&task.id).unwrap();
    let mut row = engine.store.list().unwrap().remove(0);
    for _ in 0..500 {
        if row.last_status != Status::Dispatching {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        row = engine.store.list().unwrap().remove(0);
    }
    assert_eq!(row.last_status, Status::Uncertain);
    assert!(row.last_session.is_some_and(|name| name.starts_with("task-")));
    // Sem reserva presa: a próxima execução manual é aceita.
    handle.run_now(&task.id).unwrap();
    worker.abort();
}
