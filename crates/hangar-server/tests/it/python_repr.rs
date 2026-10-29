use hangar_server::orchestration::codec;
use serde_json::{Value,json};
fn fixture()->Value {serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/python-repr.json")).unwrap()}
#[test]
fn python_string_scalars_match_captured_representation() {
    for row in fixture()["cases"].as_array().unwrap().iter().filter(|row|row["kind"]=="scalar") {
        let value=codec::loads(row["raw"].as_str().unwrap()).unwrap();
        let result=codec::python_string(&value);
        assert_eq!(codec::dumps(&Value::String(result),false),row["output_json"].as_str().unwrap(),"{}",row["raw"]);
        codec::discard(value);
    }
}
#[test]
fn python_string_deep_representation_matches_every_captured_byte() {
    let mut failures=vec![];
    for (index,row) in fixture()["cases"].as_array().unwrap().iter().enumerate().filter(|(_,row)|row["kind"]=="deep") {
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","python_repr::python_repr_deep_child","--ignored"])
            .env("HANGAR_PY_REPR_CASE",index.to_string()).output().unwrap();
        if !output.status.success(){failures.push(format!("{row}: {}",String::from_utf8_lossy(&output.stderr)));}
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}
#[test]
#[ignore="representação profunda executada em subprocesso"]
fn python_repr_deep_child() {
    let fixture=fixture();let index:usize=std::env::var("HANGAR_PY_REPR_CASE").unwrap().parse().unwrap();let row=&fixture["cases"][index];
    let depth=row["depth"].as_u64().unwrap() as usize;
    let (opening,closing)=match row["shape"].as_str().unwrap(){"array"=>("[","]"),"ordered"=>("{\"z\":0,\"x\":","}"),_=>("{\"x\":","}")};
    let value=codec::loads(&format!("{}0{}",opening.repeat(depth),closing.repeat(depth))).unwrap();
    let result=codec::python_string(&value);
    assert_eq!(result.len() as u64,row["bytes"].as_u64().unwrap());
    let hash:String=ring::digest::digest(&ring::digest::SHA256,result.as_bytes()).as_ref().iter().map(|byte|format!("{byte:02x}")).collect();
    assert_eq!(hash,row["sha256"].as_str().unwrap());codec::discard(value);
}
proptest::proptest! {
    #[test]
    fn container_repr_does_not_emit_raw_control_characters(items in proptest::collection::vec(proptest::arbitrary::any::<String>(),0..64)) {
        let value=json!(items);let output=codec::python_string(&value);
        proptest::prop_assert!(!output.chars().any(char::is_control));
    }
}
