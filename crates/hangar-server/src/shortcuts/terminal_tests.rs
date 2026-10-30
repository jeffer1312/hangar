use super::terminal::{Service, Spec};
use crate::session_open::terminal::{Mux, MuxOutput};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

struct Fake {
    failure: String,
    calls: Mutex<Vec<Vec<String>>>,
    status: Option<(i32, String)>,
}
impl Mux for Fake {
    fn scope(&self) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async { vec!["fixture-scope".into(), "--".into()] })
    }
    fn run(&self, argv: Vec<String>) -> BoxFuture<'_, MuxOutput> {
        Box::pin(async move {
            if argv.get(1).is_some_and(|s| s == "has-session") {
                return MuxOutput {
                    code: Some(1),
                    ..Default::default()
                };
            }
            if argv.get(1).is_some_and(|s| s == "kill-session") {
                self.calls.lock().unwrap().push(vec![
                    "kill".into(),
                    argv.last().unwrap().trim_start_matches('=').into(),
                ]);
                return MuxOutput {
                    code: Some(0),
                    ..Default::default()
                };
            }
            self.calls.lock().unwrap().push(argv.clone());
            if let Some((code, stdout)) = &self.status {
                return MuxOutput {
                    code: Some(*code),
                    stdout: stdout.clone(),
                    stderr: String::new(),
                };
            }
            MuxOutput {
                code: Some(
                    if !self.failure.is_empty() && argv.contains(&self.failure) {
                        1
                    } else {
                        0
                    },
                ),
                ..Default::default()
            }
        })
    }
    fn diary(&self, _: Value) {}
}
fn service(fake: Arc<Fake>, home: &std::path::Path, windows: bool) -> Arc<Service> {
    Arc::new(Service {
        mux: fake,
        home: home.into(),
        env: BTreeMap::from([
            ("SHELL".into(), "/fixture/shell".into()),
            ("DISPLAY".into(), ":fixture".into()),
        ]),
        program: vec!["tmux".into()],
        windows,
        encode: |s| {
            Ok(s.chars()
                .map(|c| if c as u32 <= 255 { c as u8 } else { b'?' })
                .collect())
        },
        id: || "abcdef".into(),
        sequence: || 2000000000123456789,
    })
}
#[tokio::test]
async fn guest_shortcut_creation_preserves_original_commands_and_files() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/shortcut-terminal.json"
    ))
    .unwrap();
    for row in fixture["start"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake {
            failure: row["failure"].as_str().unwrap().into(),
            calls: Mutex::new(Vec::new()),
            status: None,
        });
        let service = service(fake.clone(), home.path(), row["windows"].as_bool().unwrap());
        let result = service
            .start(Spec {
                owner: "owner.a:b".into(),
                cwd: "/fixture/project".into(),
                command: "echo a;\necho b".into(),
                label: "  Rotulo\tX; ".into(),
                key: " global:k; ".into(),
                origin: String::new(),
                ask: false,
                powershell: false,
                shell: None,
            })
            .await
            .unwrap();
        assert_eq!(
            result.map(|term| term.value()).unwrap_or(Value::Null),
            row["result"],
            "GUE-086/090 resultado {row}"
        );
        let directory = home.path().join("Hangar/shortcuts");
        // Normalizar antes do JSON: as contrabarras do Windows são escapadas pelo serializer.
        // Só o separador após a pasta substituída muda; argumentos e comandos ficam intactos.
        let normalize = |text: &str| text.replace(directory.to_str().unwrap(), "<DIR>")
            .replace("<DIR>\\", "<DIR>/");
        let actual: Vec<Vec<String>> = fake.calls.lock().unwrap().iter()
            .map(|call| call.iter().map(|arg| normalize(arg)).collect()).collect();
        assert_eq!(
            json!(actual),
            row["calls"],
            "GUE-086 comandos windows={} falha={}",
            row["windows"],
            row["failure"]
        );
        let mut files = serde_json::Map::new();
        if directory.exists() {
            for entry in std::fs::read_dir(&directory).unwrap() {
                let entry = entry.unwrap();
                let bytes = std::fs::read(entry.path()).unwrap();
                let text = normalize(&String::from_utf8(bytes).unwrap());
                files.insert(
                    entry.file_name().to_str().unwrap().into(),
                    json!(
                        text.as_bytes()
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    ),
                );
            }
        }
        assert_eq!(Value::Object(files), row["files"], "GUE-090 arquivos");
    }
}
#[tokio::test]
async fn guest_shortcut_status_preserves_exit_settling() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/shortcut-terminal.json"
    ))
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    for row in fixture["status"].as_array().unwrap() {
        let fake = Arc::new(Fake {
            failure: String::new(),
            calls: Mutex::new(Vec::new()),
            status: Some((
                row["code"].as_i64().unwrap() as i32,
                row["raw"].as_str().unwrap().into(),
            )),
        });
        assert_eq!(
            json!(
                service(fake, home.path(), false)
                    .status("shortcut-fixture-abcdef")
                    .await
            ),
            row["result"],
            "GUE-088 {row}"
        );
    }
}
#[test]
fn guest_shortcut_batch_preserves_original_percent_expansion() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/shortcut-terminal.json"
    ))
    .unwrap();
    let env = BTreeMap::from([("HANGAR_TEST_VAR".into(), "1".into())]);
    for row in fixture["batch"].as_array().unwrap() {
        let escaped = super::terminal_windows::escape(row["raw"].as_str().unwrap(), &env);
        assert_eq!(escaped, row["escaped"], "GUE-090");
        assert_eq!(
            super::terminal_windows::unescape(&escaped),
            row["restored"],
            "GUE-090"
        );
    }
}
