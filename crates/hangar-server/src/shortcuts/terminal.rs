//! Terminais duráveis dos atalhos: identidade e dono ficam nas opções do multiplexador.
use super::terminal_windows;
use crate::{
    list::local_shortcuts::{FORMAT, Row},
    session_open::terminal::Mux,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct Spec {
    pub owner: String,
    pub cwd: String,
    pub command: String,
    pub label: String,
    pub key: String,
    pub origin: String,
    pub ask: bool,
    pub powershell: bool,
    pub shell: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Terminal {
    pub id: String,
    pub label: String,
    pub tmux: String,
}
#[derive(Debug)]
pub enum Error {
    MuxUnavailable,
    Io(std::io::Error),
}
pub struct Service {
    pub mux: Arc<dyn Mux>,
    pub home: PathBuf,
    pub env: BTreeMap<String, String>,
    pub program: Vec<String>,
    pub windows: bool,
    pub encode: fn(&str) -> std::io::Result<Vec<u8>>,
    pub id: fn() -> String,
    pub sequence: fn() -> u128,
}
fn clean(text: &str) -> String {
    text.split(crate::transcript::py::is_space)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect()
}
fn slug(text: &str) -> String {
    let text: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    if text.is_empty() { "s".into() } else { text }
}
fn key(text: &str) -> String {
    text.split(crate::transcript::py::is_space)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
impl Service {
    async fn run(
        &self,
        args: impl IntoIterator<Item = String>,
    ) -> crate::session_open::terminal::MuxOutput {
        let mut argv = self.program.clone();
        argv.extend(args);
        self.mux.run(argv).await
    }
    fn windows_dir(&self) -> PathBuf {
        terminal_windows::directory(&self.home, &self.env)
    }
    async fn has(&self, target: &str) -> bool {
        self.run(["has-session".into(), "-t".into(), format!("={target}")])
            .await
            .code
            == Some(0)
    }
    async fn kill(&self, target: &str) -> bool {
        self.run([
            "kill-session".into(),
            "-t".into(),
            if self.windows {
                target.into()
            } else {
                format!("={target}")
            },
        ])
        .await;
        !self.has(target).await
    }
    pub(crate) async fn rows(&self) -> Result<Vec<Row>, Error> {
        let out = self
            .run(["list-sessions".into(), "-F".into(), FORMAT.into()])
            .await;
        let Some(code) = out.code else {
            return Err(Error::MuxUnavailable);
        };
        if code != 0 {
            return Ok(Vec::new());
        }
        let mut rows = crate::list::local_shortcuts::parse(&out.stdout, |_| (true, None));
        if self.windows {
            for row in &mut rows {
                let (alive, code) = self.windows_status(&row.id);
                row.alive = alive;
                row.exit_code = code;
            }
        }
        Ok(rows)
    }
    fn windows_status(&self, id: &str) -> (bool, Option<i64>) {
        let Ok(raw) = std::fs::read(self.windows_dir().join(format!("{id}.exit"))) else {
            return (true, None);
        };
        let text = String::from_utf8_lossy(&raw);
        let text = crate::transcript::py::strip(&text);
        (false, text.parse().ok())
    }
    pub async fn status(&self, target: &str) -> (bool, Option<i64>) {
        if self.windows {
            return self.windows_status(target.rsplit('-').next().unwrap_or(""));
        }
        let out = self
            .run([
                "display".into(),
                "-p".into(),
                "-t".into(),
                format!("={target}:"),
                "#{pane_dead}\t#{pane_dead_status}\t#{pane_dead_signal}".into(),
            ])
            .await;
        let mut fields = out.stdout.trim_end_matches(['\r', '\n']).split('\t');
        let dead = fields.next().unwrap_or("");
        let code = fields.next().unwrap_or("");
        let signal = fields.next().unwrap_or("");
        if out.code != Some(0) || dead != "1" || (code.is_empty() && signal.is_empty()) {
            return (true, None);
        }
        (false, code.parse().ok())
    }
    pub async fn output(&self, target: &str) -> String {
        let output = self
            .run([
                "capture-pane".into(),
                "-p".into(),
                "-S".into(),
                "-200".into(),
                "-t".into(),
                format!("={target}:"),
            ])
            .await;
        output
            .stdout
            .lines()
            .filter(|line| !line.starts_with("Pane is dead"))
            .collect::<Vec<_>>()
            .join("\n")
    }
    async fn options(&self, target: &str, options: &[(String, String)]) -> bool {
        for (option, value) in options {
            let value = if !self.windows && value.ends_with(';') {
                format!("{}\\;", &value[..value.len() - 1])
            } else {
                value.clone()
            };
            if self
                .run([
                    "set-option".into(),
                    "-t".into(),
                    format!("={target}:"),
                    option.clone(),
                    value,
                ])
                .await
                .code
                != Some(0)
            {
                return false;
            }
        }
        true
    }
    async fn abort(&self, target: &str, id: &str) {
        if let Ok(rows) = self.rows().await
            && let Some(row) = rows.into_iter().find(|r| r.tmux == target)
        {
            self.close_row(&row).await;
            return;
        }
        if self.kill(target).await && self.windows {
            let _ = terminal_windows::forget(&self.windows_dir(), id);
        }
    }
    /// O trabalho já aceito termina mesmo se o pedido HTTP for cancelado no meio das marcações.
    pub async fn start(self: Arc<Self>, spec: Spec) -> Result<Option<Terminal>, Error> {
        tokio::spawn(async move { self.start_inner(spec).await })
            .await
            .map_err(|_| Error::MuxUnavailable)?
    }
    async fn start_inner(&self, spec: Spec) -> Result<Option<Terminal>, Error> {
        let id = (self.id)();
        let target = format!(
            "shortcut-{}-{id}",
            slug(if spec.owner.is_empty() {
                "hangar"
            } else {
                &spec.owner
            })
        );
        let label = clean(&spec.label);
        let label = if label.is_empty() {
            clean(&spec.command)
        } else {
            label
        };
        let mut argv = self.mux.scope().await;
        argv.extend(self.program.clone());
        argv.extend([
            "new-session".into(),
            "-d".into(),
            "-s".into(),
            target.clone(),
            "-c".into(),
            spec.cwd.clone(),
        ]);
        for name in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "HYPRLAND_INSTANCE_SIGNATURE",
        ] {
            if let Some(value) = self.env.get(name) {
                argv.extend(["-e".into(), format!("{name}={value}")]);
            }
        }
        let fixed: Vec<(String, String)> = [
            ("@cp_hidden", "1".into()),
            ("@cp_shortcut_owner", spec.owner),
            ("@cp_shortcut_id", id.clone()),
            ("@cp_shortcut_seq", (self.sequence)().to_string()),
        ]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.into(), v))
        .collect();
        let mut free: Vec<(String, String)> = [
            ("@cp_shortcut_label", label.clone()),
            ("@cp_shortcut_key", key(&spec.key)),
            ("@cp_shortcut_origin", clean(&spec.origin)),
            (
                "@cp_shortcut_ask",
                if spec.ask { "1".into() } else { "0".into() },
            ),
            ("@cp_shortcut_cwd", spec.cwd),
        ]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.into(), v))
        .collect();
        if self.windows {
            argv.push(
                terminal_windows::write_command(
                    &self.windows_dir(),
                    &id,
                    &spec.command,
                    spec.powershell,
                    spec.shell.as_deref(),
                    &self.env,
                    self.encode,
                )
                .map_err(Error::Io)?,
            );
        } else {
            let shell = spec
                .shell
                .filter(|s| !s.is_empty())
                .or_else(|| self.env.get("SHELL").filter(|s| !s.is_empty()).cloned())
                .unwrap_or_else(|| "/bin/sh".into());
            argv.extend(["--".into(), shell, "-c".into(), spec.command.clone()]);
            for (option, value) in &fixed {
                argv.extend([
                    ";".into(),
                    "set-option".into(),
                    "-t".into(),
                    format!("={target}:"),
                    option.clone(),
                    value.clone(),
                ]);
            }
            for (window, option, value) in [
                (true, "remain-on-exit", "on"),
                (true, "remain-on-exit-format", ""),
                (false, "status", "off"),
            ] {
                argv.extend([";".into(), "set-option".into()]);
                if window {
                    argv.push("-w".into());
                }
                argv.extend([
                    "-t".into(),
                    format!("={target}:"),
                    option.into(),
                    value.into(),
                ]);
            }
            if !spec.command.is_empty() {
                free.push(("@cp_shortcut_cmd".into(), spec.command));
            }
        }
        let created = self.mux.run(argv).await;
        if created.code != Some(0) && !self.has(&target).await {
            if self.windows {
                terminal_windows::forget(&self.windows_dir(), &id).map_err(Error::Io)?;
            }
            return Ok(None);
        }
        if self.windows {
            if !self.options(&target, &fixed).await {
                self.abort(&target, &id).await;
                return Ok(None);
            }
            free.push(("status".into(), "off".into()));
        }
        if !self.options(&target, &free).await {
            self.abort(&target, &id).await;
            return Ok(None);
        }
        Ok(Some(Terminal {
            id,
            label,
            tmux: target,
        }))
    }
    async fn close_row(&self, row: &Row) -> bool {
        if let Some(pid) = row.pid.filter(|p| *p != 0)
            && (row.alive || self.windows)
        {
            self.kill_group(pid).await;
        }
        let gone = self.kill(&row.tmux).await;
        if gone && self.windows {
            if let Err(error) = terminal_windows::forget(&self.windows_dir(), &row.id) {
                tracing::warn!(kind=?error.kind(),"arquivos do terminal encerrado não saíram");
            }
        }
        gone
    }
    async fn kill_group(&self, pid: u32) {
        #[cfg(unix)]
        if !self.windows {
            if unsafe { libc::kill(-(pid as i32), libc::SIGTERM) } != 0 {
                return;
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while tokio::time::Instant::now() < deadline {
                if unsafe { libc::kill(-(pid as i32), 0) } != 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        #[cfg(windows)]
        {
            let root = self
                .env
                .get("SystemRoot")
                .map(String::as_str)
                .unwrap_or("C:\\Windows");
            let executable = PathBuf::from(root).join("System32/taskkill.exe");
            let mut command = crate::terminal_input::child_command(executable.as_os_str());
            command
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .kill_on_drop(true);
            let _ = tokio::time::timeout(Duration::from_secs(10), command.output()).await;
        }
    }
}
