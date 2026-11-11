use std::collections::BTreeMap;

use hangar_session_tools::external;
use serde_json::json;

#[test]
fn absent_emulator_has_the_legacy_error() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([("PATH".into(), root.path().to_string_lossy().into_owned())]);
    let error = runtime
        .block_on(external::open("session", &env, vec![]))
        .unwrap_err();
    assert_eq!(error.status, 503);
    assert_eq!(error.code, "erro_terminal_ausente");
}

#[test]
fn unsupported_configured_emulator_is_not_replaced_by_a_probe() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([
        ("PATH".into(), root.path().to_string_lossy().into_owned()),
        ("CP_TERMINAL".into(), "unsupported".into()),
    ]);
    let error = runtime
        .block_on(external::open("session", &env, vec![]))
        .unwrap_err();
    assert_eq!(error.status, 503);
    assert_eq!(error.code, "erro_terminal_invalido");
    assert_eq!(error.params["nome"], "unsupported");
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        root: tempfile::TempDir,
        env: BTreeMap<String, String>,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().to_string_lossy().into_owned();
            Self {
                root,
                env: BTreeMap::from([
                    ("PATH".into(), path.clone()),
                    ("HOME".into(), path.clone()),
                    ("XDG_RUNTIME_DIR".into(), path.clone()),
                    ("RESULT".into(), format!("{path}/result")),
                ]),
            }
        }
        fn program(&self, name: &str, body: &str) {
            let file = self.root.path().join(name);
            fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fn result(&self) -> String {
            fs::read_to_string(self.root.path().join("result")).unwrap()
        }
    }

    #[tokio::test]
    async fn configured_emulator_must_exist_even_when_another_is_available() {
        let mut fixture = Fixture::new();
        fixture.program("kitty", "exit 0");
        fixture.env.insert("CP_TERMINAL".into(), "wezterm".into());
        let error = external::open("session", &fixture.env, vec![])
            .await
            .unwrap_err();
        assert_eq!(error.code, "erro_terminal_invalido");
        assert_eq!(error.params["nome"], "wezterm");
    }

    #[tokio::test]
    async fn probe_uses_the_first_executable_in_the_legacy_order() {
        let fixture = Fixture::new();
        fixture.program("kitty", "printf kitty > \"$RESULT\"");
        fixture.program("wezterm", "printf wezterm > \"$RESULT\"");
        assert_eq!(
            external::open("session", &fixture.env, vec![])
                .await
                .unwrap(),
            json!({"ok": true})
        );
        assert_eq!(fixture.result(), "wezterm");
    }

    #[tokio::test]
    async fn non_executable_candidate_does_not_hide_a_later_emulator() {
        let fixture = Fixture::new();
        fs::write(fixture.root.path().join("wezterm"), "not executable").unwrap();
        fixture.program("kitty", "printf kitty > \"$RESULT\"");
        external::open("session", &fixture.env, vec![])
            .await
            .unwrap();
        assert_eq!(fixture.result(), "kitty");
    }

    #[tokio::test]
    async fn each_emulator_receives_its_own_flags_and_an_exact_attach_target() {
        for (emulator, prefix) in [
            ("wezterm", "start\n--\n"),
            ("kitty", ""),
            ("alacritty", "-e\n"),
            ("konsole", "-e\n"),
            ("gnome-terminal", "--\n"),
            ("xterm", "-e\n"),
        ] {
            let mut fixture = Fixture::new();
            fixture.program(emulator, "printf '%s\\n' \"$@\" > \"$RESULT\"");
            fixture.env.insert("CP_TERMINAL".into(), emulator.into());
            external::open("a-2", &fixture.env, vec![]).await.unwrap();
            assert_eq!(
                fixture.result(),
                format!("{prefix}tmux\nattach\n-t\n=a-2:\n")
            );
        }
    }

    #[tokio::test]
    async fn scope_is_a_prefix_and_display_is_inherited_or_discovered() {
        let mut fixture = Fixture::new();
        fixture.program("scope", "printf '%s\\n' \"$@\" > \"$RESULT\"; exec \"$@\"");
        fixture.program(
            "kitty",
            "printf '%s\\n' \"$DISPLAY\" \"$WAYLAND_DISPLAY\" >> \"$RESULT\"",
        );
        fixture.env.insert("DISPLAY".into(), ":72".into());
        fs::write(fixture.root.path().join("wayland-2"), "").unwrap();
        fs::write(fixture.root.path().join("wayland-1.lock"), "").unwrap();
        external::open(
            "session",
            &fixture.env,
            vec![
                fixture
                    .root
                    .path()
                    .join("scope")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            fixture.result(),
            "kitty\ntmux\nattach\n-t\n=session:\n:72\nwayland-2\n"
        );
        fixture
            .env
            .insert("WAYLAND_DISPLAY".into(), "chosen".into());
        external::open(
            "session",
            &fixture.env,
            vec![
                fixture
                    .root
                    .path()
                    .join("scope")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )
        .await
        .unwrap();
        assert!(fixture.result().ends_with(":72\nchosen\n"));
    }

    #[tokio::test]
    async fn emulator_starts_in_its_own_unix_session() {
        let fixture = Fixture::new();
        fixture.program(
            "kitty",
            "printf '%s\\n' \"$$\" > \"$RESULT\"; /bin/ps -o sid= -p $$ >> \"$RESULT\"",
        );
        external::open("session", &fixture.env, vec![])
            .await
            .unwrap();
        let result = fixture.result();
        let ids: Vec<u32> = result
            .split_whitespace()
            .map(|part| part.parse().unwrap())
            .collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], ids[1]);
    }

    #[tokio::test]
    async fn spawn_failure_is_reported_with_the_legacy_code() {
        let fixture = Fixture::new();
        let error = external::open(
            "session",
            &fixture.env,
            vec![
                fixture
                    .root
                    .path()
                    .join("missing-scope")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )
        .await
        .unwrap_err();
        // O emulador é conferido antes da tentativa de abrir o processo.
        assert_eq!(error.code, "erro_terminal_ausente");
        fixture.program("kitty", "exit 0");
        let error = external::open(
            "session",
            &fixture.env,
            vec![
                fixture
                    .root
                    .path()
                    .join("missing-scope")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, 503);
        assert_eq!(error.code, "erro_terminal_abertura_falhou");
        assert!(
            error.params["erro"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
    }

    #[tokio::test]
    async fn early_failure_reports_stderr_or_the_exit_code() {
        for (body, expected) in [
            (
                "printf '  cannot open display\\n' >&2; exit 7",
                "cannot open display",
            ),
            ("exit 7", "codigo 7"),
        ] {
            let fixture = Fixture::new();
            fixture.program("kitty", body);
            let error = external::open("session", &fixture.env, vec![])
                .await
                .unwrap_err();
            assert_eq!(error.status, 503);
            assert_eq!(error.code, "erro_terminal_saiu_cedo");
            assert_eq!(error.params["saida"], expected);
        }
    }

    #[tokio::test]
    async fn single_instance_client_can_exit_zero_after_opening() {
        let fixture = Fixture::new();
        fixture.program("gnome-terminal", "exit 0");
        assert_eq!(
            external::open("session", &fixture.env, vec![])
                .await
                .unwrap(),
            json!({"ok": true})
        );
    }

    #[tokio::test]
    async fn stderr_is_a_regular_file_instead_of_an_unread_pipe() {
        let fixture = Fixture::new();
        fixture.program("kitty", "test -f /dev/stderr || exit 8; i=0; while test $i -lt 2000; do printf '0123456789012345678901234567890123456789012345678901234567890123\\n' >&2; i=$((i+1)); done; printf done > \"$RESULT\"");
        assert_eq!(
            external::open("session", &fixture.env, vec![])
                .await
                .unwrap(),
            json!({"ok": true})
        );
        assert_eq!(fixture.result(), "done");
    }
}
