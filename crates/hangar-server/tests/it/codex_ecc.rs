use hangar_server::{
    codex_setup::ecc::{self, Runner},
    command_capture::{ByteOutput, CaptureFailure, CaptureOptions},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
struct Fixture {
    root: PathBuf,
    case: Value,
    calls: Mutex<Vec<Value>>,
}
impl Fixture {
    fn install(&self) {
        for (name, version) in self.case["package"]["dependencies"].as_object().unwrap() {
            let path = self
                .root
                .join("plugin/node_modules")
                .join(name)
                .join("package.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let version = if self.case["name"] == "locked" {
                json!("1.0.0")
            } else {
                version.clone()
            };
            std::fs::write(
                path,
                serde_json::to_vec(&json!({"version":version})).unwrap(),
            )
            .unwrap();
        }
    }
}
impl Runner for Fixture {
    fn resolve(&self, program: &str) -> Option<PathBuf> {
        if self.case["name"] == format!("no-{program}") {
            return None;
        }
        Some(self.root.join(if program == "npm" {
            if self.case["name"] == "no-cli-js" {
                "npm"
            } else {
                "npm-cli.js"
            }
        } else {
            "node"
        }))
    }
    async fn run(
        &self,
        args: Vec<String>,
        budget: Duration,
        options: CaptureOptions,
    ) -> Result<ByteOutput, CaptureFailure> {
        self.calls.lock().unwrap().push(json!({"args":args,"timeout":budget.as_secs(),"cwd":options.cwd,"stdin":if options.inherit_stdin {Value::Null} else {json!(-3)}}));
        if args.iter().any(|arg| arg == "-e") {
            return Ok(ByteOutput {
                success: self.case["name"] != "unhealthy-after",
                stdout: vec![],
                stderr: vec![],
            });
        }
        if self.case["name"] == "timeout" {
            return Err(CaptureFailure {
                kind: "timeout".into(),
                stdout: b"fixture stdout".to_vec(),
                stderr: b"fixture stderr".to_vec(),
            });
        }
        if self.case["name"] != "failure" {
            self.install();
        }
        Ok(ByteOutput {
            success: self.case["name"] != "failure",
            stdout: b"fixture stdout".to_vec(),
            stderr: b"fixture stderr".to_vec(),
        })
    }
}
fn hex(path: &Path) -> Value {
    std::fs::read(path)
        .map(|bytes| json!(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
        .unwrap_or(Value::Null)
}
#[tokio::test]
async fn codex_ecc_matches_original_runtime_decisions_and_effects() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/ecc-runtime.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in corpus["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        // TEMP pode usar um alias 8.3, enquanto o npm é resolvido pelo caminho canônico.
        let canonical = std::fs::canonicalize(temp.path()).unwrap();
        let root = canonical.as_path();
        let plugin = root.join("plugin");
        let backups = root.join("backups");
        std::fs::create_dir(&plugin).unwrap();
        std::fs::write(
            plugin.join("package.json"),
            serde_json::to_vec(&case["package"]).unwrap(),
        )
        .unwrap();
        if case["name"] == "locked" {
            std::fs::write(
                plugin.join("package-lock.json"),
                br#"{"packages":{"node_modules/a":{"version":"1.0.0"}}}"#,
            )
            .unwrap();
        }
        std::fs::write(
            root.join(if case["name"] == "no-cli-js" {
                "npm"
            } else {
                "npm-cli.js"
            }),
            "fixture",
        )
        .unwrap();
        let runner = Fixture {
            root: root.to_owned(),
            case: case.clone(),
            calls: Mutex::new(Vec::new()),
        };
        if case["name"] == "healthy" {
            runner.install();
        }
        let result = match ecc::prepare(&plugin, &backups, &runner).await {
            Ok(ok) => json!({"ok":ok}),
            Err(message) => json!({"message":message}),
        };
        let expected = if case["result"].get("error").is_some() {
            json!({"message":case["result"]["message"]})
        } else {
            case["result"].clone()
        };
        let calls = crate::contract_paths::value(&json!(*runner.calls.lock().unwrap()), root, "$ROOT");
        let diagnostic = hex(&backups.join("ecc-runtime-npm.log"));
        if result != expected || calls != case["calls"] || diagnostic != case["diagnostic_hex"] {
            failures.push(format!(
                "{}: result={result}, calls={calls}, diagnostic={diagnostic}",
                case["name"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

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
