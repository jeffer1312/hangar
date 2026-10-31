//! Resolução de rotas do contrato HTTP, sem dependências de convidados.
use hangar_server::http_contract::resolve;
use proptest::prelude::*;

proptest! {
    #[test]
    fn login_guests_http_contract_external_path_never_panics(path in ".{0,2000}", method in ".{0,30}") {
        let _=resolve(&[("/api/item/{id}","GET"),("/api/item/{id}/delete","POST")],&method,&path);
    }
}
