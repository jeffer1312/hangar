use std::path::Path;

use serde_json::{Value, json};

use hangar_update_news::{known_version, mark_seen, record, unseen, valid_device};

fn item(id: &str, title: &str) -> Value { json!({ "id": id, "title": title, "kind": "new", "platforms": "all" }) }

fn file(home: &Path) -> std::path::PathBuf { home.join(".hangar").join("update-news.json") }

fn stored(home: &Path) -> Value { serde_json::from_slice(&std::fs::read(file(home)).unwrap()).unwrap() }

fn ids(news: &[Value]) -> Vec<&str> { news.iter().map(|item| item["id"].as_str().unwrap()).collect() }

#[test]
fn a_new_device_gets_nothing_and_starts_at_the_current_seq() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(record(home.path(), "1", "2", vec![item("a", "A")]).unwrap(), Some(1));
    let first = unseen(home.path(), "phone").unwrap();
    assert!(first.news.is_empty());
    assert_eq!(first.through, 1);
    assert_eq!(stored(home.path())["devices"]["phone"]["cursor"], 1);
    // Ainda sem nada novo: continua vazio.
    assert!(unseen(home.path(), "phone").unwrap().news.is_empty());
}

#[test]
fn an_arrival_after_the_cursor_shows_and_mark_seen_hides_it() {
    let home = tempfile::tempdir().unwrap();
    unseen(home.path(), "phone").unwrap();
    assert_eq!(record(home.path(), "1", "2", vec![item("a", "A"), item("b", "B")]).unwrap(), Some(1));
    let got = unseen(home.path(), "phone").unwrap();
    assert_eq!(ids(&got.news), ["a", "b"]);
    assert_eq!(got.through, 1);
    let arrival = &stored(home.path())["arrivals"][0];
    assert_eq!((arrival["from"].as_str(), arrival["to"].as_str()), (Some("1"), Some("2")));
    assert!(chrono::DateTime::parse_from_rfc3339(arrival["at"].as_str().unwrap()).is_ok());
    assert_eq!(mark_seen(home.path(), "phone", got.through).unwrap(), 1);
    let after = unseen(home.path(), "phone").unwrap();
    assert!(after.news.is_empty());
    assert_eq!(after.through, 1);
}

#[test]
fn mark_seen_never_goes_back_and_is_capped_at_seq() {
    let home = tempfile::tempdir().unwrap();
    unseen(home.path(), "phone").unwrap();
    record(home.path(), "1", "2", vec![item("a", "A")]).unwrap();
    record(home.path(), "2", "3", vec![item("b", "B")]).unwrap();
    assert_eq!(mark_seen(home.path(), "phone", 99).unwrap(), 2);
    assert_eq!(mark_seen(home.path(), "phone", 1).unwrap(), 2);
    assert_eq!(stored(home.path())["devices"]["phone"]["cursor"], 2);
}

#[test]
fn two_devices_are_independent() {
    let home = tempfile::tempdir().unwrap();
    unseen(home.path(), "phone").unwrap();
    unseen(home.path(), "desk").unwrap();
    record(home.path(), "1", "2", vec![item("a", "A")]).unwrap();
    let phone = unseen(home.path(), "phone").unwrap();
    mark_seen(home.path(), "phone", phone.through).unwrap();
    assert!(unseen(home.path(), "phone").unwrap().news.is_empty());
    assert_eq!(ids(&unseen(home.path(), "desk").unwrap().news), ["a"]);
}

#[test]
fn a_repeated_id_shows_once_newest_arrival_first() {
    let home = tempfile::tempdir().unwrap();
    unseen(home.path(), "phone").unwrap();
    record(home.path(), "1", "2", vec![item("a", "old A"), item("b", "B")]).unwrap();
    record(home.path(), "2", "3", vec![item("c", "C"), item("a", "new A")]).unwrap();
    let got = unseen(home.path(), "phone").unwrap();
    assert_eq!(ids(&got.news), ["c", "a", "b"]);
    assert_eq!(got.news[1]["title"], "new A");
    assert_eq!(got.through, 2);
}

#[test]
fn record_without_news_only_keeps_the_version() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(known_version(home.path()).unwrap(), None);
    assert_eq!(record(home.path(), "1", "2", vec![]).unwrap(), None);
    assert_eq!(known_version(home.path()).unwrap().as_deref(), Some("2"));
    let data = stored(home.path());
    assert_eq!((data["seq"].as_u64(), data["arrivals"].as_array().map(Vec::len)), (Some(0), Some(0)));
    assert_eq!(record(home.path(), "2", "3", vec![item("a", "A")]).unwrap(), Some(1));
    assert_eq!(known_version(home.path()).unwrap().as_deref(), Some("3"));
}

#[test]
fn a_file_without_version_reads_as_none() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".hangar")).unwrap();
    std::fs::write(file(home.path()), br#"{"seq":0,"arrivals":[],"devices":{}}"#).unwrap();
    assert_eq!(known_version(home.path()).unwrap(), None);
}

#[test]
fn only_the_last_50_arrivals_are_kept() {
    let home = tempfile::tempdir().unwrap();
    for n in 1..=51 { record(home.path(), "x", "y", vec![item(&format!("n{n}"), "N")]).unwrap(); }
    let data = stored(home.path());
    assert_eq!(data["seq"], 51);
    let arrivals = data["arrivals"].as_array().unwrap();
    assert_eq!(arrivals.len(), 50);
    assert_eq!((arrivals[0]["seq"].as_u64(), arrivals[49]["seq"].as_u64()), (Some(2), Some(51)));
}

#[test]
fn only_the_50_most_recently_seen_devices_are_kept() {
    let home = tempfile::tempdir().unwrap();
    let devices: serde_json::Map<String, Value> = (0..50)
        .map(|n| (format!("d{n}"), json!({ "cursor": 0, "at": format!("2026-01-01T00:00:{n:02}Z") })))
        .collect();
    std::fs::create_dir_all(home.path().join(".hangar")).unwrap();
    std::fs::write(file(home.path()), json!({ "seq": 0, "arrivals": [], "devices": devices }).to_string()).unwrap();
    unseen(home.path(), "fresh").unwrap();
    let data = stored(home.path());
    let kept = data["devices"].as_object().unwrap();
    assert_eq!(kept.len(), 50);
    assert!(kept.contains_key("fresh"));
    assert!(!kept.contains_key("d0"), "o visto há mais tempo sai");
    assert!(kept.contains_key("d1"));
}

#[test]
fn a_corrupt_file_is_an_error_not_an_empty_list() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".hangar")).unwrap();
    std::fs::write(file(home.path()), b"{ not json").unwrap();
    assert!(unseen(home.path(), "phone").is_err());
    assert!(mark_seen(home.path(), "phone", 1).is_err());
    assert!(record(home.path(), "1", "2", vec![item("a", "A")]).is_err());
    assert!(record(home.path(), "1", "2", vec![]).is_err());
    assert!(known_version(home.path()).is_err());
    assert_eq!(std::fs::read(file(home.path())).unwrap(), b"{ not json");
}

#[test]
fn device_ids_are_1_to_64_safe_characters() {
    assert!(valid_device("a"));
    assert!(valid_device("Phone_01-x"));
    assert!(valid_device(&"x".repeat(64)));
    assert!(!valid_device(""));
    assert!(!valid_device(&"x".repeat(65)));
    assert!(!valid_device("a/b"));
    assert!(!valid_device("a b"));
    assert!(!valid_device("ação"));
}
