//! O `agy` com terminal ignora o `GEMINI_HOME`: só a conta padrão.

mod common;

use common::{FakeServer, Home, Route, plain_host};

/// `--conta` com o Antigravity para antes de qualquer pedido, sem preparar uma conta Claude no lugar.
#[test]
fn account_with_antigravity_is_refused_before_any_request() {
    let server = FakeServer::start(vec![Route::new("*", "", 200, "{}")]);
    let home = Home::new(server.port, "");
    let done = home.run(&["--new", "agy", "/tmp", "--provider", "antigravity", "--conta", "trabalho"], &[], &plain_host());
    assert_eq!(done.code, 2, "{}", done.stderr);
    assert_eq!(done.stderr, "erro: --conta não vale para o Antigravity: com terminal ele só usa a conta padrão (a do ~/.gemini)\n");
    assert!(server.requests().is_empty());
}
