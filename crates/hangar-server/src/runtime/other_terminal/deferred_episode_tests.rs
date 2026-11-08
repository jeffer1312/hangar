use super::deferred_episode::{DeferredEpisode, Notice};
use crate::terminal_input::DeliveryStage;

#[test]
fn other_deferred_episode_matches_python_thresholds_and_recovery() {
    let mut episode = DeferredEpisode::default();
    let notices: Vec<_> = (1..=15)
        .filter_map(|_| episode.observe(DeliveryStage::Composer, "composer_busy"))
        .collect();
    assert_eq!(
        notices,
        vec![
            Notice {
                code: "composer_busy",
                count: 1
            },
            Notice {
                code: "composer_busy",
                count: 6
            },
            Notice {
                code: "composer_busy",
                count: 10
            },
            Notice {
                code: "composer_busy",
                count: 15
            },
        ]
    );
    assert_eq!(episode.observe(DeliveryStage::Write, "submitted"), None);
    assert_eq!(
        episode.observe(DeliveryStage::Composer, "composer_busy"),
        Some(Notice {
            code: "composer_busy",
            count: 1
        })
    );
}

#[test]
fn other_deferred_episode_keeps_composer_count_through_unavailability() {
    let mut episode = DeferredEpisode::default();
    for _ in 0..5 {
        episode.observe(DeliveryStage::Composer, "composer_busy");
    }
    assert_eq!(
        episode.observe(DeliveryStage::Ready, "question_open"),
        Some(Notice {
            code: "input_unavailable",
            count: 1
        })
    );
    for _ in 0..4 {
        assert_eq!(episode.observe(DeliveryStage::Ready, "not_ready"), None);
    }
    assert_eq!(
        episode.observe(DeliveryStage::Ready, "input_unavailable"),
        Some(Notice {
            code: "input_unavailable",
            count: 6
        })
    );
    assert_eq!(
        episode.observe(DeliveryStage::Composer, "composer_busy"),
        Some(Notice {
            code: "composer_busy",
            count: 6
        })
    );
    assert_eq!(
        episode.observe(DeliveryStage::Ready, "question_open"),
        Some(Notice {
            code: "input_unavailable",
            count: 1
        })
    );
    let mut new_life = DeferredEpisode::default();
    assert_eq!(
        new_life.observe(DeliveryStage::Composer, "composer_busy"),
        Some(Notice {
            code: "composer_busy",
            count: 1
        })
    );
}

#[test]
fn other_deferred_episode_identity_failure_does_not_prove_composer_recovery() {
    let mut episode = DeferredEpisode::default();
    for _ in 0..5 {
        episode.observe(DeliveryStage::Composer, "composer_busy");
    }
    assert_eq!(
        episode.observe(DeliveryStage::Identity, "life_changed"),
        None
    );
    assert_eq!(
        episode.observe(DeliveryStage::Composer, "composer_busy"),
        Some(Notice {
            code: "composer_busy",
            count: 6
        })
    );
}

#[tokio::test]
async fn other_deferred_episode_diary_keeps_escalation_and_new_episode() {
    let reports = tempfile::tempdir().unwrap();
    let diag = crate::diag::DiagClient::new("127.0.0.1:9".parse().unwrap(), String::new())
        .with_diary(Some(reports.path().to_owned()));
    let mut episode = DeferredEpisode::default();
    for _ in 0..10 {
        if let Some(notice) = episode.observe(DeliveryStage::Composer, "composer_busy") {
            notice.report(&diag, "episode-a");
        }
    }
    episode.observe(DeliveryStage::Write, "submitted");
    episode
        .observe(DeliveryStage::Composer, "composer_busy")
        .unwrap()
        .report(&diag, "episode-a");
    diag.flush().await;
    let rows = crate::diag::test_support::rows(reports.path());
    let rows: Vec<_> = rows
        .iter()
        .filter(|row| row["codigo"] == "composer_busy")
        .collect();
    assert_eq!(
        rows.iter()
            .map(|row| row["count"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 6, 10, 1]
    );
    assert_eq!(
        rows.iter()
            .map(|row| row["nivel"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["aviso", "erro", "erro", "aviso"]
    );
}
