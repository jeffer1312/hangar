use chrono::{DateTime, Utc};
use hangar_scheduled_tasks::{Outcome, Schedule, Status, Store, Target, TaskInput};
fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}
fn input() -> TaskInput {
    TaskInput {
        name: "Relatório".into(),
        prompt: "Revise as mudanças.".into(),
        enabled: true,
        schedule: Schedule::Interval { minutes: 5 },
        target: Target::New {
            launch: serde_json::json!({"cwd":"/project"}),
            worktree: false,
            base: None,
        },
    }
}
#[test]
fn storage_reserves_once_before_effect_and_restart_marks_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite");
    let now = at("2026-11-12T12:00:00Z");
    let later = at("2026-11-12T15:00:00Z");
    let store = Store::open(&path).unwrap();
    let task = store.create(input(), now, &Utc).unwrap();
    let attempt = store.reserve(&task.id, later, &Utc, false).unwrap();
    assert_eq!(store.list().unwrap()[0].last_status, Status::Dispatching);
    assert_eq!(
        store.list().unwrap()[0].next_run_at.as_deref(),
        Some("2026-11-12T15:05:00+00:00")
    );
    assert_eq!(
        store
            .reserve(&task.id, later, &Utc, true)
            .unwrap_err()
            .status,
        409
    );
    drop(store);
    let store = Store::open(&path).unwrap();
    store.recover().unwrap();
    assert_eq!(store.list().unwrap()[0].last_status, Status::Uncertain);
    assert!(store.reserve(&task.id, later, &Utc, false).is_err());
    assert!(
        !store
            .finish(
                &attempt,
                Outcome {
                    status: Status::Sent,
                    error: None,
                    session: Some("created".into())
                }
            )
            .unwrap()
    );
}
#[test]
fn storage_revision_pause_delete_and_failed_attempt_do_not_replay() {
    let dir = tempfile::tempdir().unwrap();
    let now = at("2026-11-12T12:00:00Z");
    let store = Store::open(&dir.path().join("tasks.sqlite")).unwrap();
    let task = store.create(input(), now, &Utc).unwrap();
    let paused = store
        .set_enabled(&task.id, false, task.revision, now, &Utc)
        .unwrap();
    assert_eq!(
        store
            .set_enabled(&task.id, true, task.revision, now, &Utc)
            .unwrap_err()
            .status,
        409
    );
    assert!(store.reserve(&task.id, now, &Utc, false).is_err());
    let attempt = store.reserve(&task.id, now, &Utc, true).unwrap();
    store.delete(&task.id, paused.revision + 1).unwrap();
    assert!(
        !store
            .finish(
                &attempt,
                Outcome {
                    status: Status::Failed,
                    error: Some("falhou".into()),
                    session: None
                }
            )
            .unwrap()
    );
    assert!(store.list().unwrap().is_empty());
}
#[test]
fn storage_independent_connections_cannot_reserve_the_same_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite");
    let now = at("2026-11-12T12:00:00Z");
    let first = Store::open(&path).unwrap();
    let task = first.create(input(), now, &Utc).unwrap();
    let second = Store::open(&path).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let one = {
        let barrier = barrier.clone();
        let id = task.id.clone();
        std::thread::spawn(move || {
            barrier.wait();
            first.reserve(&id, now, &Utc, true)
        })
    };
    barrier.wait();
    let two = second.reserve(&task.id, now, &Utc, true);
    assert_ne!(one.join().unwrap().is_ok(), two.is_ok());
}
#[test]
fn storage_once_due_during_shutdown_is_reserved_on_return_and_never_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite");
    let now = at("2026-11-12T12:00:00Z");
    let store = Store::open(&path).unwrap();
    let mut request = input();
    request.schedule = Schedule::Once {
        at: "2026-11-12T13:00:00Z".into(),
    };
    let task = store.create(request, now, &Utc).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    store.recover().unwrap();
    let later = at("2026-11-13T12:00:00Z");
    let attempt = store.reserve(&task.id, later, &Utc, false).unwrap();
    assert_eq!(store.list().unwrap()[0].next_run_at, None);
    store
        .finish(
            &attempt,
            Outcome {
                status: Status::Failed,
                error: Some("Recusada".into()),
                session: None,
            },
        )
        .unwrap();
    assert!(store.reserve(&task.id, later, &Utc, false).is_err());
}
#[test]
fn storage_internal_launch_fields_are_never_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite")).unwrap();
    for key in [
        "creator",
        "link",
        "prewarm",
        "take_reserved_name",
        "remember_provider",
        "initial_prompt",
        "api_key",
    ] {
        let mut request = input();
        if let Target::New { launch, .. } = &mut request.target {
            launch[key] = serde_json::json!("forbidden");
        }
        assert_eq!(
            store
                .create(request, at("2026-11-12T12:00:00Z"), &Utc)
                .unwrap_err()
                .status,
            400
        );
    }
    assert!(store.list().unwrap().is_empty());
}
