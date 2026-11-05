use hangar_server::{
    codex_setup::ecc::{self, Runner},
    command_capture::{ByteOutput, CaptureFailure, CaptureOptions},
};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
struct NoExecution;
impl Runner for NoExecution {
    fn resolve(&self, _: &str) -> Option<PathBuf> {
        panic!("dependência inválida chegou à descoberta de programas")
    }
    async fn run(
        &self,
        _: Vec<String>,
        _: Duration,
        _: CaptureOptions,
    ) -> Result<ByteOutput, CaptureFailure> {
        panic!("dependência inválida iniciou um programa")
    }
}
proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(48))]
    #[test]
    fn codex_ecc_rejects_external_path_names_before_programs(suffix in "[a-z0-9._-]{0,80}", prefix in proptest::sample::select(vec!["../", "/", "A", "a;", "a\\", "a "])) {
        let root=tempfile::tempdir().unwrap();
        let name=format!("{prefix}{suffix}");
        let package=json!({"dependencies":{name:"1.2.3"}});
        std::fs::write(root.path().join("package.json"),serde_json::to_vec(&package).unwrap()).unwrap();
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result=runtime.block_on(ecc::prepare(root.path(),&root.path().join("backups"),&NoExecution));
        proptest::prop_assert_eq!(result,Err("Nome de dependência do ECC inválido".into()));
        proptest::prop_assert!(!root.path().join("backups").exists());
    }
}
