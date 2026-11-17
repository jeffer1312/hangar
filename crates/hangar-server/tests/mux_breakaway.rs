//! B-32 (Windows): o `new-session` que o servidor lança nasce FORA do Job do lançador
//! (`CREATE_BREAKAWAY_FROM_JOB`): o psmux sobe dele o servidor da sessão, que sobrevive ao reinício do
//! backend (`runtime_process.py:67-69`, commit 412776934). Os outros comandos do multiplexador seguem
//! dentro. Processo próprio: o teste põe o processo inteiro num Job, como o lançador faz.
#![cfg(windows)]

use std::path::Path;
use std::time::{Duration, Instant};

use hangar_server::session_open::terminal::{Mux as _, ProcessMux};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

/// O Job do lançador (`die_with_the_launcher`) sem o `KILL_ON_JOB_CLOSE`: fechar o handle não mata o teste.
fn launcher_job() -> HANDLE {
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        assert!(!job.is_null(), "{}", std::io::Error::last_os_error());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        assert_ne!(SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&info) as u32), 0, "{}", std::io::Error::last_os_error());
        assert_ne!(AssignProcessToJobObject(job, GetCurrentProcess()), 0, "{}", std::io::Error::last_os_error());
        job
    }
}

/// Roda `subcommand` pelo multiplexador de verdade com um PowerShell no lugar do psmux (o `subcommand`
/// é o primeiro argumento, como no psmux; o PowerShell só recusa o nome e segue) e diz se o processo
/// filho está no Job enquanto vive.
async fn child_in_job(job: HANDLE, dir: &Path, subcommand: &str) -> bool {
    let powershell = Path::new(&std::env::var_os("SystemRoot").expect("SystemRoot"))
        .join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    let pid_file = dir.join(format!("{subcommand}.pid"));
    let script = format!("Set-Content -LiteralPath '{}' -Value $PID; Start-Sleep -Seconds 30", pid_file.display());
    let argv = vec![powershell.to_string_lossy().into_owned(), subcommand.to_owned(), ";".to_owned(), script];
    let run = tokio::spawn(async move { ProcessMux { timeout: Duration::from_secs(60), ..ProcessMux::default() }.run(argv).await });
    let deadline = Instant::now() + Duration::from_secs(60);
    let pid = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|text| text.trim().parse::<u32>().ok()) { break pid; }
        if run.is_finished() { panic!("{subcommand}: o processo saiu antes de anotar o pid: {:?}", run.await.unwrap()); }
        assert!(Instant::now() < deadline, "{subcommand}: o processo não anotou o pid");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let inside = unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        assert!(!process.is_null(), "{subcommand}: {}", std::io::Error::last_os_error());
        let mut inside = 0;
        let asked = IsProcessInJob(process, job, &mut inside);
        CloseHandle(process);
        assert_ne!(asked, 0, "{subcommand}: {}", std::io::Error::last_os_error());
        inside != 0
    };
    // `kill_on_drop` do multiplexador encerra o filho.
    run.abort();
    inside
}

#[tokio::test]
async fn b32_new_session_breaks_away_from_the_launcher_job_and_other_commands_stay_inside() {
    let dir = tempfile::tempdir().unwrap();
    let job = launcher_job();
    assert!(child_in_job(job, dir.path(), "list-sessions").await, "comando comum herda o Job (o teste enxerga o Job)");
    assert!(!child_in_job(job, dir.path(), "new-session").await, "o new-session nasceu dentro do Job: morreria com o backend");
}
