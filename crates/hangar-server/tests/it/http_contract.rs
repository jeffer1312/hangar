//! Tabela e respostas capturadas do roteador FastAPI, sem dependências de convidados.
use hangar_server::http_contract::{Decision,resolve};
use serde_json::Value;
use proptest::prelude::*;

#[test]
fn login_guests_http_contract_resolution_matches_python() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/methods.json")).unwrap();
    let table:Vec<_>=fixture["routes"].as_array().unwrap().iter().map(|r|(r[0].as_str().unwrap(),r[1].as_str().unwrap())).collect();
    for row in fixture["rows"].as_array().unwrap().iter().filter(|r|r["token"]=="owner") {
        let expected=if row["matched"]==true { Decision::Matched }
            else if row["partial"]==true { Decision::MethodNotAllowed(row["allow"].as_str().unwrap().into()) }
            else if row["status"]==307 { Decision::Redirect(row["location"].as_str().unwrap().strip_prefix("⟦BASE⟧").unwrap().split('?').next().unwrap().into()) }
            else { Decision::NotFound };
        assert_eq!(resolve(&table,row["method"].as_str().unwrap(),row["decoded_path"].as_str().unwrap()),expected,"{} {}",row["method"],row["path"]);
    }
}

proptest! {
    #[test]
    fn login_guests_http_contract_external_path_never_panics(path in ".{0,2000}", method in ".{0,30}") {
        let _=resolve(&[("/api/item/{id}","GET"),("/api/item/{id}/delete","POST")],&method,&path);
    }
}
