//! Executa contratos sintéticos pela entrada padrão; não participa do instalador.
use std::io::BufRead;
fn main() {
    if std::env::args().any(|a| a == "--benchmark") {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        let mut times = Vec::new();
        for _ in 0..30 {
            let op = serde_json::from_value::<hangar_workspace::Operation>(value.clone()).unwrap();
            let start = std::time::Instant::now();
            let result = hangar_workspace::execute(op).unwrap();
            std::hint::black_box(result);
            times.push(start.elapsed().as_micros());
        }
        times.sort();
        let rss = std::fs::read_to_string("/proc/self/status").ok().and_then(|s|s.lines().find(|l|l.starts_with("VmHWM:")).and_then(|l|l.split_whitespace().nth(1)).and_then(|s|s.parse::<u64>().ok()));
        println!("{}", serde_json::json!({"median_us":times[15],"peak_rss_kib":rss,"iterations":30}));
        return;
    }
    if std::env::args().any(|a| a == "--process-fixture") {
        let mut command = if cfg!(windows) {
            let mut c = std::process::Command::new("powershell");
            c.args(["-NoProfile", "-Command", "$child=Start-Process ping -ArgumentList '-n 30 127.0.0.1' -PassThru -NoNewWindow; [IO.File]::WriteAllText($env:HANGAR_TEST_ROOT_PID, [string]$PID); [IO.File]::WriteAllText($env:HANGAR_TEST_CHILD_PID, [string]$child.Id); $child.WaitForExit()"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "echo $$ > \"$HANGAR_TEST_ROOT_PID\"; sleep 30 & echo $! > \"$HANGAR_TEST_CHILD_PID\"; wait"]);
            c
        };
        let _ = hangar_workspace::process::run_program(
            &mut command,
            std::time::Duration::from_secs(40),
        );
        return;
    }
    for line in std::io::stdin().lock().lines() {
        let response = match line
            .ok()
            .and_then(|s| serde_json::from_str::<hangar_workspace::Operation>(&s).ok())
        {
            Some(op) => match hangar_workspace::execute(op) {
                Ok(result) => serde_json::json!({"ok":true,"result":result}),
                Err(error) => serde_json::json!({"ok":false,"error":error}),
            },
            None => {
                serde_json::json!({"ok":false,"error":{"status":400,"detail":"invalid operation"}})
            }
        };
        println!("{response}");
    }
}
