use chrono::{DateTime, FixedOffset, Utc};
use hangar_scheduled_tasks::{Schedule, next_run};
fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}
#[test]
fn scheduling_once_interval_and_weekly_use_instants_and_server_weekdays() {
    let zone = FixedOffset::west_opt(3 * 3600).unwrap();
    let now = at("2026-11-12T10:00:00-03:00");
    assert_eq!(
        next_run(
            &Schedule::Once {
                at: "2026-11-12T11:00:00-03:00".into()
            },
            now,
            &zone
        )
        .unwrap(),
        Some(at("2026-11-12T14:00:00Z"))
    );
    assert_eq!(
        next_run(&Schedule::Interval { minutes: 90 }, now, &zone).unwrap(),
        Some(at("2026-11-12T14:30:00Z"))
    );
    assert_eq!(
        next_run(
            &Schedule::Weekly {
                time: "09:00".into(),
                weekdays: vec![1]
            },
            now,
            &zone
        )
        .unwrap(),
        Some(at("2026-11-16T12:00:00Z"))
    );
    assert_eq!(
        next_run(
            &Schedule::Once {
                at: "2026-11-12T09:00:00-03:00".into()
            },
            now,
            &zone
        )
        .unwrap(),
        None
    );
}
#[test]
fn scheduling_dst_skips_missing_wall_time_and_repeated_time_runs_only_once() {
    let zone = chrono_tz::America::New_York;
    let gap = Schedule::Weekly {
        time: "02:30".into(),
        weekdays: vec![0],
    };
    assert_eq!(
        next_run(&gap, at("2026-03-08T00:00:00-05:00"), &zone).unwrap(),
        Some(at("2026-03-15T02:30:00-04:00"))
    );
    let repeated = Schedule::Weekly {
        time: "01:30".into(),
        weekdays: vec![0],
    };
    assert_eq!(
        next_run(&repeated, at("2026-11-01T00:00:00-04:00"), &zone).unwrap(),
        Some(at("2026-11-01T01:30:00-04:00"))
    );
    assert_eq!(
        next_run(&repeated, at("2026-11-01T01:45:00-04:00"), &zone).unwrap(),
        Some(at("2026-11-08T01:30:00-05:00"))
    );
    assert_eq!(
        next_run(
            &Schedule::Interval { minutes: 60 },
            at("2026-03-08T01:30:00-05:00"),
            &zone
        )
        .unwrap(),
        Some(at("2026-03-08T03:30:00-04:00"))
    );
}
#[test]
fn scheduling_invalid_dates_days_and_intervals_are_rejected() {
    for schedule in [
        Schedule::Once {
            at: "2026-02-30T10:00:00-03:00".into(),
        },
        Schedule::Once {
            at: "2026-11-12T10:00:00".into(),
        },
        Schedule::Weekly {
            time: "25:00".into(),
            weekdays: vec![1],
        },
        Schedule::Weekly {
            time: "10:00".into(),
            weekdays: vec![],
        },
        Schedule::Interval { minutes: 0 },
    ] {
        assert!(
            next_run(&schedule, at("2026-11-12T10:00:00Z"), &Utc).is_err(),
            "{schedule:?}"
        );
    }
}
