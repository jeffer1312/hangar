use hangar_server::orchestration::{body, codec};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/json-deep-output.json"
    )).unwrap()
}

#[test]
fn json_deep_serialization_matches_every_python_byte_without_aborting() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "json_deep_output::json_deep_output_child", "--ignored"])
            .env("HANGAR_JSON_OUTPUT_CASE", index.to_string()).output().unwrap();
        assert!(output.status.success(), "{row}: {}", String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
#[ignore = "serialização profunda executada em subprocesso"]
fn json_deep_output_child() {
    let fixture = fixture();
    let index: usize = std::env::var("HANGAR_JSON_OUTPUT_CASE").unwrap().parse().unwrap();
    let row = &fixture["rows"][index];
    let depth = row["depth"].as_u64().unwrap_or(0) as usize;
    let (opening, closing) = match row["shape"].as_str().unwrap_or("") {
        "array" => ("[", "]"),
        "ordered" => ("{\"z\":0,\"x\":", "}"),
        _ => ("{\"x\":", "}"),
    };
    let input = row["input"].as_str().map(str::to_owned)
        .unwrap_or_else(||format!("{}0{}", opening.repeat(depth), closing.repeat(depth)));
    let value = body::parse(input.as_bytes()).unwrap();
    let output = if row["compact"] == true {
        hangar_server::transcript::pyjson::dumps_compact_unicode(&value)
    } else if let Some(indent) = row["indent"].as_u64() {
        hangar_server::transcript::pyjson::dumps_indent(&value, indent as usize, false)
    } else { codec::dumps(&value, row["sort_keys"].as_bool().unwrap()) };
    codec::discard(value);
    assert_eq!(output.len() as u64, row["bytes"].as_u64().unwrap());
    let hash: String = ring::digest::digest(&ring::digest::SHA256, output.as_bytes())
        .as_ref().iter().map(|byte|format!("{byte:02x}")).collect();
    assert_eq!(hash, row["sha256"].as_str().unwrap());
}
