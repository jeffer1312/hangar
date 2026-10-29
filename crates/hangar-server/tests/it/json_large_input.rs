use hangar_server::orchestration::{body, codec};
use proptest::prelude::*;

#[test]
fn json_deep_input_does_not_abort_the_process() {
    for depth in [900, 1500, 10000] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "json_large_input::json_deep_input_child", "--ignored"])
            .env("HANGAR_JSON_TEST_DEPTH", depth.to_string())
            .output().unwrap();
        assert!(output.status.success(), "profundidade {depth}: {}", String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
#[ignore = "auxiliar de pilha executado em subprocesso"]
fn json_deep_input_child() {
    let depth: usize = std::env::var("HANGAR_JSON_TEST_DEPTH").unwrap().parse().unwrap();
    for (open, close) in [("[", "]"), ("{\"x\":", "}")] {
        let input = format!("{}0{}", open.repeat(depth), close.repeat(depth));
        let value = body::parse(input.as_bytes()).unwrap();
        assert!(!codec::contains_surrogate(&value));
        assert!(!codec::contains_nonfinite(&value));
        let copy = codec::clone_value(&value);
        codec::discard(codec::normalize_response(copy));
        codec::discard(value);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn json_large_integer_diagnostics_never_panic(
        prefix in ".{0,80}", suffix in ".{0,80}", digits in 4300usize..4400,
    ) {
        let input = format!("{prefix}{}{suffix}", "9".repeat(digits));
        let _ = body::parse(input.as_bytes());
    }
}
