// crates/hangar-server/tests/contract_pyjson.rs
mod common;

use common::golden;
use hangar_server::transcript::{decode_line, pyjson, ts_of_iso};

#[test]
fn dumps_matches_python() {
    for case in golden("pyjson.json").as_array().expect("lista") {
        let raw = case["raw"].as_str().unwrap();
        let v = pyjson::loads_lossless(raw).unwrap_or_else(|| panic!("não leu {raw}"));
        assert_eq!(pyjson::dumps(&v, true), case["sorted"].as_str().unwrap(), "sort_keys: {raw}");
        assert_eq!(pyjson::dumps(&v, false), case["unsorted"].as_str().unwrap(), "ordem original: {raw}");
        assert_eq!(pyjson::dumps_unicode(&v, false), case["unicode"].as_str().unwrap(), "ensure_ascii=False: {raw}");
    }
}

#[test]
fn decode_line_scrubs_like_python() {
    for case in golden("pyjson.json").as_array().unwrap() {
        let raw = case["raw"].as_str().unwrap();
        let v = decode_line(format!("  {raw}\n").as_bytes()).unwrap_or_else(|| panic!("não leu {raw}"));
        assert_eq!(pyjson::dumps(&v, true), case["scrubbed"].as_str().unwrap(), "{raw}");
    }
    assert!(decode_line(b"   \n").is_none());
    assert!(decode_line(br#"{"type": "user", "message":"#).is_none());
}

#[test]
fn iso_clock_matches_python() {
    for case in golden("isotime.json").as_array().unwrap() {
        let s = case[0].as_str().unwrap();
        assert_eq!(ts_of_iso(s), case[1].as_f64(), "{s:?}");
    }
}
