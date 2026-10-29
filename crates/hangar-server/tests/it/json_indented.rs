use hangar_server::orchestration::codec;
use serde_json::Value;
#[test]
fn json_indented_preserves_python_utf8_bytes_and_refuses_surrogates() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/peers/indented.json")).unwrap();
    let mut failures=vec![];
    for row in fixture["cases"].as_array().unwrap(){
        let input=codec::loads(row["input"].as_str().unwrap()).unwrap();
        let result=codec::dumps_indented_utf8(&input,row["indent"].as_u64().unwrap() as usize);
        if row["result"]["error"].is_string(){assert!(result.is_err(),"{row}");}
        else {
            let actual=result.unwrap().bytes().map(|byte|format!("{byte:02x}")).collect::<String>();
            if actual!=row["result"]["hex"].as_str().unwrap(){failures.push(format!("{} indent {}",row["input"],row["indent"]));}
        }
        codec::discard(input);
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}
