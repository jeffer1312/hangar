//! Testes de integração do hangar-server num executável só: um módulo por assunto, o suporte na raiz.
//! Um executável por arquivo eram dezenas de links de ~180 MB cada; os que mexem em estado do processo
//! inteiro (PATH, HOME, TZ, logger global, gancho de pânico) continuam à parte (ver o Cargo.toml).
//! O limitador de avisos do tracing (`warn_limit`) também é do processo: teste que conta avisos vive à
//! parte. O do diário é por servidor (o Python falso de cada um), e ali os nomes de sessão podem repetir.

mod common;
mod fake;
#[cfg(unix)]
mod list_support;
mod mods_support;
mod workspace_fixture;

/// `CP_RUST_CANO_BIN` apontando para o `hangar-cano` de `target/<perfil>/`, ao lado de `deps/` (o binário
/// de outro crate não tem `CARGO_BIN_EXE_*`). Gravado uma vez no processo, por quem precisar primeiro: o
/// `cano_binary` guarda o primeiro que acha, e mais de um teste gravando competiria com quem lê.
pub fn use_cano_bin() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let bin = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("hangar-cano");
        assert!(bin.exists(), "rode `cargo build -p hangar-cano` antes: {}", bin.display());
        // SAFETY: valor único, posto antes de qualquer sonda do binário neste processo.
        unsafe { std::env::set_var("CP_RUST_CANO_BIN", bin) }
    });
}

/// Nome de um teste deste executável para o `--exact`: o caminho do módulo de `module_path!()` sem o
/// do crate, que o libtest não mostra.
pub fn exact_name(module: &str, test: &str) -> String {
    match module.split_once("::") {
        Some((_, path)) => format!("{path}::{test}"),
        None => test.to_string(),
    }
}

/// Saída de um teste que reexecuta este executável com `--exact`: exatamente um teste rodou. Nome que
/// não casa não acha teste nenhum, e o filho sai com sucesso sem ter testado nada.
#[track_caller]
pub fn assert_ran_one(stdout: &[u8]) {
    let stdout = String::from_utf8_lossy(stdout);
    assert!(stdout.contains("running 1 test\n"), "o filho não rodou exatamente um teste:\n{stdout}");
}

mod accounts_catalog;
mod accounts_claude_login;
mod accounts_codex_login;
mod accounts_lifecycle;
mod accounts_preparation;
mod contract_codex_routes;
mod contract_costs_areas;
mod contract_costs_index;
mod contract_costs_pricing;
mod contract_costs_reports;
mod contract_costs_simple;
mod contract_history;
mod contract_list;
mod contract_pyjson;
mod contract_tail;
mod contract_terminal;
mod conversations;
mod costs_fast_paths;
mod costs_fx;
mod costs_origins;
mod costs_report_unit;
mod costs_routes;
mod costs_session_unit;
mod costs_uso_unit;
mod groups_bridge;
mod groups_deliver;
mod groups_legacy;
mod groups_local;
mod groups_peers;
mod groups_routes;
mod groups_store;
mod groups_sweep;
mod list_bridge;
mod list_facts;
mod list_routes;
mod list_wake;
mod mods_actor;
mod mods_bridge;
mod mods_bridge_terminal;
mod mods_captures;
mod mods_click;
mod mods_e2e;
mod mods_fixtures;
mod mods_routes;
mod mods_screen;
mod mods_side;
mod mods_state;
mod mods_surface;
mod mods_terminal_link;
mod mods_terminal_routes;
mod mods_terminal_state;
mod mods_tree;
mod pages_routes;
mod perf_accounts_uploads;
mod plugin_loopback;
mod proxy;
mod queue_v2_terminal;
mod runtime_actor;
mod runtime_cano;
mod runtime_claude;
mod runtime_codex;
mod runtime_contract;
mod runtime_gateway;
mod runtime_ingress;
mod runtime_lease;
mod runtime_open;
mod runtime_process;
mod runtime_protocol;
mod runtime_queue;
mod runtime_quota;
mod runtime_receipt;
mod runtime_recovery;
mod runtime_side_events;
mod session_write_answer;
mod session_write_control;
mod session_write_input;
mod session_write_routes;
mod state_monitor_round;
mod terminal_control;
mod terminal_input;
mod terminal_input_process;
mod terminal_input_tmux;
mod terminal_routes;
mod terminal_runtime;
mod terminal_runtime_tmux;
mod uploads_store;
mod workspace_routes;
mod worktree_routes;
