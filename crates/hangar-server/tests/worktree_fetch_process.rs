#[path = "it/fake/mod.rs"]
mod fake;
#[path = "it/workspace_fixture/mod.rs"]
mod workspace_fixture;
use fake::{OWNER,client};
use std::{process::Command,time::{Duration,Instant}};

#[tokio::test]
async fn fetch_process_contract() {
    if let Ok(kind)=std::env::var("HANGAR_FETCH_PROOF_KIND") {
        if kind=="timeout" {
            #[cfg(unix)]
            let mut command={let mut c=Command::new("/bin/sh"); c.args(["-c","/bin/sleep 20 & wait"]); c};
            #[cfg(windows)]
            let mut command={
                use std::os::windows::process::CommandExt;
                let system=std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
                let mut c=Command::new(system.join("cmd.exe"));
                c.args(["/d","/s","/c"]).raw_arg(format!("\"\"{}\" -n 30 127.0.0.1 >NUL\"",system.join("ping.exe").display())); c
            };
            // estavel: mede encerramento real dos processos; relógio falso não controla o pipe do sistema.
            let start=Instant::now();
            let error=hangar_workspace::process::run_program(&mut command,Duration::from_millis(250)).err().unwrap();
            assert_eq!(error.status,504); assert_eq!(error.detail,"git timeout");
            assert!(start.elapsed()<Duration::from_secs(5),"o descendente reteve o pipe por {:?}",start.elapsed());
            return;
        }
        let dir=tempfile::tempdir().unwrap();
        let f=workspace_fixture::fixture(dir.path(),false).await;
        let response=client().post(format!("http://{}/api/worktrees/fetch",f.addr)).bearer_auth(OWNER)
            .json(&serde_json::json!({"repo":dir.path()})).send().await.unwrap();
        assert_eq!(response.status(),500);
        let body:serde_json::Value=response.json().await.unwrap();
        if kind=="missing" { assert_eq!(body,serde_json::json!({"detail":"git nao encontrado"})); }
        else {
            let text=body["detail"].as_str().unwrap();
            #[cfg(unix)] assert_eq!(text,"git falhou: [Errno 8] Exec format error: 'git'");
            #[cfg(windows)] assert!(text.starts_with("git falhou: [WinError 193] ")||text.starts_with("git falhou: [WinError 216] "),"{text}");
            assert!(!text.contains("(os error"));
        }
        assert_eq!(f.hits(),0);
        return;
    }
    let dir=tempfile::tempdir().unwrap();
    let missing=dir.path().join("missing"); let invalid=dir.path().join("invalid");
    std::fs::create_dir(&missing).unwrap(); std::fs::create_dir(&invalid).unwrap();
    let executable=invalid.join(if cfg!(windows){"git.exe"}else{"git"});
    std::fs::write(&executable,b"this is not an executable\n").unwrap();
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o755)).unwrap(); }
    for (kind,path) in [("missing",missing.as_path()),("invalid",invalid.as_path()),("timeout",missing.as_path())] {
        let home=dir.path().join(kind); std::fs::create_dir_all(home.join("tmp")).unwrap();
        let mut child=Command::new(std::env::current_exe().unwrap());
        child.args(["--exact","fetch_process_contract","--nocapture"]).env_clear();
        for key in ["SystemRoot","SystemDrive","COMSPEC","WINDIR","LD_LIBRARY_PATH","DYLD_LIBRARY_PATH","DYLD_FALLBACK_LIBRARY_PATH"] {
            if let Some(value)=std::env::var_os(key) { child.env(key,value); }
        }
        child.env("HANGAR_FETCH_PROOF_KIND",kind).env("PATH",path).env("HOME",&home).env("USERPROFILE",&home)
            .env("LOCALAPPDATA",home.join("local")).env("XDG_CACHE_HOME",home.join("cache"))
            .env("XDG_CONFIG_HOME",home.join("config")).env("XDG_DATA_HOME",home.join("data"))
            .env("TMPDIR",home.join("tmp")).env("TMP",home.join("tmp")).env("TEMP",home.join("tmp"));
        let output=child.output().unwrap();
        assert!(output.status.success(),"{kind}: {}{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    }
}
