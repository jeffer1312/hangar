//! Contenção por comando; nenhuma posse volta enquanto houver auxiliar escritor.
use std::{io,time::Duration};
use tokio::process::{Child,Command};

/// Autoridade do servidor que nenhum programa auxiliar (CLI de conta, ffmpeg) deve herdar.
pub(crate) const PRIVATE_ENV_KEYS: [&str; 4] = ["HANGAR_INTERNAL_SECRET", "CP_AUTH_TOKEN", "HANGAR_RUNTIME_INSTANCE", "HANGAR_PLUGIN_TOKEN"];

pub(crate) trait OwnedTree: Send {
    fn terminate(&mut self)->io::Result<()>;
    fn active(&mut self)->io::Result<bool>;
}

pub(crate) async fn finish(tree:&mut impl OwnedTree) {finish_after(tree,Duration::from_secs(5)).await}
// Sinal aceito sem a árvore acabar (estado D) não libera a posse; só passa a ficar visível.
async fn finish_after(tree:&mut impl OwnedTree,stuck_after:Duration) {
    let started=tokio::time::Instant::now();let mut warned=false;
    // No Linux a conferência varre o /proc inteiro: o worker passa as outras tarefas adiante antes.
    let multi=tokio::runtime::Handle::current().runtime_flavor()==tokio::runtime::RuntimeFlavor::MultiThread;
    loop {
        let active=if multi {tokio::task::block_in_place(||tree.active())} else {tree.active()};
        match active {
            Ok(false)=>return,
            Ok(true)=>if let Err(error)=tree.terminate(){report(&error);},
            Err(error)=>report(&error),
        }
        if !warned && started.elapsed()>=stuck_after {
            warned=true;
            if crate::warn_limit::allow(None,"command_tree_stuck") {
                tracing::warn!(code="command_tree_stuck",waited_ms=stuck_after.as_millis() as u64,reason="auxiliares não terminaram após o sinal; posse conservada");
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Espera o líder sair sem recolhê-lo: o zumbi segura o número do grupo até o `finish`.
#[cfg(any(target_os="linux",target_os="macos"))]
pub(crate) async fn leader_exited(child:&mut Child)->io::Result<()> {
    unsafe extern "C" {fn waitid(idtype:i32,id:u32,info:*mut u64,options:i32)->i32;}
    const P_PID:i32=1;const WEXITED:i32=4;
    #[cfg(target_os="linux")] const WNOWAIT:i32=0x0100_0000;
    #[cfg(target_os="macos")] const WNOWAIT:i32=0x20;
    let pid=child.id().ok_or_else(||io::Error::other("auxiliar sem identidade"))?;
    tokio::task::spawn_blocking(move || {
        let mut info=[0u64;32];
        loop {
            if unsafe {waitid(P_PID,pid,info.as_mut_ptr(),WEXITED|WNOWAIT)}==0 {return Ok(());}
            let error=io::Error::last_os_error();
            if error.kind()!=io::ErrorKind::Interrupted {return Err(error);}
        }
    }).await.map_err(io::Error::other)?
}
// Windows: o Job não depende do número do processo; demais Unix ficam com a espera comum.
#[cfg(not(any(target_os="linux",target_os="macos")))]
pub(crate) async fn leader_exited(child:&mut Child)->io::Result<()> {child.wait().await.map(|_|())}
fn report(error:&io::Error) {
    if crate::warn_limit::allow(None,"command_tree_cleanup") {
        tracing::warn!(code="command_tree_cleanup",io_kind=?error.kind(),os_error=?error.raw_os_error(),reason="fim dos auxiliares ainda sem prova; posse conservada");
    }
}

#[cfg(unix)]
pub(crate) struct CommandTree {group:i32}
#[cfg(unix)]
impl CommandTree {
    pub(crate) fn configure(command:&mut Command)->io::Result<Self> {command.process_group(0);Ok(Self {group:0})}
    pub(crate) fn attach(&mut self,child:&Child)->io::Result<()> {
        self.group=child.id().ok_or_else(||io::Error::other("auxiliar sem identidade"))? as i32;Ok(())
    }
}
#[cfg(unix)]
impl OwnedTree for CommandTree {
    fn terminate(&mut self)->io::Result<()> {
        unsafe extern "C" {fn kill(pid:i32,signal:i32)->i32;}
        if self.group<=0 {return Err(io::Error::other("grupo auxiliar inválido"));}
        if unsafe {kill(-self.group,9)}==0 {return Ok(());}
        let error=io::Error::last_os_error();if error.raw_os_error()==Some(3){Ok(())}else{Err(error)}
    }
    fn active(&mut self)->io::Result<bool> {group_active(self.group)}
}
#[cfg(target_os="linux")]
fn group_active(group:i32)->io::Result<bool> {
    for entry in std::fs::read_dir("/proc")? {
        let entry=entry?;if entry.file_name().to_string_lossy().parse::<u32>().is_err(){continue;}
        let value=match std::fs::read(entry.path().join("stat")) {
            Ok(value)=>value,Err(error) if error.kind()==io::ErrorKind::NotFound=>continue,Err(error)=>return Err(error),
        };
        let end=value.iter().rposition(|byte|*byte==b')').ok_or_else(||io::Error::other("identidade do auxiliar ilegível"))?;
        let tail=std::str::from_utf8(&value[end+1..]).map_err(|_|io::Error::other("grupo auxiliar ilegível"))?;
        let fields:Vec<_>=tail.split_whitespace().collect();
        let pgid=fields.get(2).and_then(|value|value.parse::<i32>().ok()).ok_or_else(||io::Error::other("grupo auxiliar ilegível"))?;
        if pgid==group && !matches!(fields.first().copied(),Some("Z"|"X"|"x")){return Ok(true);}
    }
    Ok(false)
}
#[cfg(all(unix,not(target_os="linux")))]
fn group_active(group:i32)->io::Result<bool> {
    let output=std::process::Command::new("/bin/ps").args(["-axo","pid=,pgid=,stat="]).output()?;
    if !output.status.success(){return Err(io::Error::other("lista de auxiliares indisponível"));}
    let text=std::str::from_utf8(&output.stdout).map_err(|_|io::Error::other("lista de auxiliares ilegível"))?;
    for line in text.lines() {
        let fields:Vec<_>=line.split_whitespace().collect();
        let pgid=fields.get(1).and_then(|value|value.parse::<i32>().ok()).ok_or_else(||io::Error::other("grupo auxiliar ilegível"))?;
        if pgid==group && !fields.get(2).is_some_and(|status|status.starts_with('Z')) {return Ok(true);}
    }
    Ok(false)
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    type Handle=*mut c_void;
    #[repr(C)]#[derive(Default)]struct Basic {user:i64,kernel:i64,flags:u32,min_ws:usize,max_ws:usize,limit:u32,affinity:usize,priority:u32,scheduling:u32}
    #[repr(C)]#[derive(Default)]struct Extended {basic:Basic,io:[u64;6],process_memory:usize,job_memory:usize,peak_process:usize,peak_job:usize}
    #[repr(C)]#[derive(Default)]struct Accounting {times:[i64;4],faults:u32,total:u32,active:u32,terminated:u32}
    #[repr(C)]#[derive(Default)]struct ThreadEntry {size:u32,usage:u32,id:u32,owner:u32,base:i32,delta:i32,flags:u32}
    #[link(name="kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes:*mut c_void,name:*const u16)->Handle;
        fn SetInformationJobObject(job:Handle,class:i32,data:*const c_void,size:u32)->i32;
        fn AssignProcessToJobObject(job:Handle,process:Handle)->i32;
        fn TerminateJobObject(job:Handle,code:u32)->i32;
        fn QueryInformationJobObject(job:Handle,class:i32,data:*mut c_void,size:u32,length:*mut u32)->i32;
        fn TerminateProcess(process:Handle,code:u32)->i32;
        fn WaitForSingleObject(handle:Handle,millis:u32)->u32;
        fn CreateToolhelp32Snapshot(flags:u32,pid:u32)->Handle;
        fn Thread32First(snapshot:Handle,entry:*mut ThreadEntry)->i32;
        fn Thread32Next(snapshot:Handle,entry:*mut ThreadEntry)->i32;
        fn OpenThread(access:u32,inherit:i32,id:u32)->Handle;
        fn ResumeThread(thread:Handle)->u32;
        fn CloseHandle(handle:Handle)->i32;
    }
    fn check(value:i32)->io::Result<()> {if value==0{Err(io::Error::last_os_error())}else{Ok(())}}
    struct Resource(Handle);
    impl Drop for Resource {fn drop(&mut self){if let Err(error)=check(unsafe {CloseHandle(self.0)}){report(&error);}}}
    pub(crate) struct CommandTree {job:Resource,process:Handle,assigned:bool}
    unsafe impl Send for CommandTree {}
    impl CommandTree {
        pub(crate) fn configure(command:&mut Command)->io::Result<Self> {
            command.creation_flags(0x00000004 | 0x08000000);
            let handle=unsafe {CreateJobObjectW(std::ptr::null_mut(),std::ptr::null())};
            if handle.is_null(){return Err(io::Error::last_os_error());}
            let job=Resource(handle);let mut limits=Extended::default();limits.basic.flags=0x2000;
            check(unsafe {SetInformationJobObject(handle,9,&limits as *const _ as _,std::mem::size_of::<Extended>() as u32)})?;
            Ok(Self {job,process:std::ptr::null_mut(),assigned:false})
        }
        pub(crate) fn attach(&mut self,child:&Child)->io::Result<()> {
            self.process=child.raw_handle().ok_or_else(||io::Error::other("auxiliar sem handle"))? as Handle;
            check(unsafe {AssignProcessToJobObject(self.job.0,self.process)})?;self.assigned=true;
            let snapshot=unsafe {CreateToolhelp32Snapshot(0x4,0)};
            if snapshot as usize==usize::MAX {return Err(io::Error::last_os_error());}
            let snapshot=Resource(snapshot);let mut entry=ThreadEntry {size:std::mem::size_of::<ThreadEntry>() as u32,..ThreadEntry::default()};
            check(unsafe {Thread32First(snapshot.0,&mut entry)})?;let mut resumed=false;
            loop {
                if Some(entry.owner)==child.id() {
                    let thread=unsafe {OpenThread(0x2,0,entry.id)};
                    if thread.is_null(){return Err(io::Error::last_os_error());}
                    let thread=Resource(thread);
                    if unsafe {ResumeThread(thread.0)}==u32::MAX {return Err(io::Error::last_os_error());}
                    resumed=true;
                }
                if unsafe {Thread32Next(snapshot.0,&mut entry)}==0 {
                    let error=io::Error::last_os_error();if error.raw_os_error()!=Some(18){return Err(error);}break;
                }
            }
            if resumed {Ok(())}else{Err(io::Error::other("thread inicial do auxiliar ausente"))}
        }
    }
    impl OwnedTree for CommandTree {
        fn terminate(&mut self)->io::Result<()> {
            check(unsafe {if self.assigned {TerminateJobObject(self.job.0,1)}else{TerminateProcess(self.process,1)}})
        }
        fn active(&mut self)->io::Result<bool> {
            if !self.assigned {
                return match unsafe {WaitForSingleObject(self.process,0)} {0=>Ok(false),258=>Ok(true),_=>Err(io::Error::last_os_error())};
            }
            let mut accounting=Accounting::default();
            check(unsafe {QueryInformationJobObject(self.job.0,1,&mut accounting as *mut _ as _,std::mem::size_of::<Accounting>() as u32,std::ptr::null_mut())})?;
            Ok(accounting.active!=0)
        }
    }
}
#[cfg(windows)]pub(crate) use windows::CommandTree;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc,atomic::{AtomicBool,AtomicUsize,Ordering}};
    struct DelayedTree {active:Arc<AtomicBool>,attempts:Arc<AtomicUsize>}
    impl OwnedTree for DelayedTree {
        fn active(&mut self)->io::Result<bool>{Ok(self.active.load(Ordering::Acquire))}
        fn terminate(&mut self)->io::Result<()> {
            self.attempts.fetch_add(1,Ordering::Release);Err(io::Error::new(io::ErrorKind::PermissionDenied,"termination failed"))
        }
    }
    #[tokio::test]
    async fn termination_failure_and_delayed_job_end_keep_cleanup_pending() {
        let active=Arc::new(AtomicBool::new(true));let attempts=Arc::new(AtomicUsize::new(0));
        let mut tree=DelayedTree {active:active.clone(),attempts:attempts.clone()};
        let task=tokio::spawn(async move {finish(&mut tree).await});
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!task.is_finished());assert!(attempts.load(Ordering::Acquire)>0);
        active.store(false,Ordering::Release);tokio::time::timeout(Duration::from_secs(1),task).await.unwrap().unwrap();
    }
    #[derive(Clone)]struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self,data:&[u8])->io::Result<usize>{self.0.lock().unwrap().extend_from_slice(data);Ok(data.len())}
        fn flush(&mut self)->io::Result<()>{Ok(())}
    }
    struct UnkillableTree {active:Arc<AtomicBool>}
    impl OwnedTree for UnkillableTree {
        fn active(&mut self)->io::Result<bool>{Ok(self.active.load(Ordering::Acquire))}
        fn terminate(&mut self)->io::Result<()>{Ok(())}
    }
    #[tokio::test]
    async fn killed_tree_that_never_ends_warns_and_keeps_ownership() {
        let output=Arc::new(std::sync::Mutex::new(Vec::new()));let capture=Capture(output.clone());
        let subscriber=tracing_subscriber::fmt().without_time().with_ansi(false).with_writer(move||capture.clone()).finish();
        let _guard=tracing::subscriber::set_default(subscriber);
        let active=Arc::new(AtomicBool::new(true));let mut tree=UnkillableTree {active:active.clone()};
        let task=tokio::spawn(async move {finish_after(&mut tree,Duration::from_millis(40)).await});
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!task.is_finished());
        let text=String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert_eq!(text.matches("command_tree_stuck").count(),1,"{text}");
        active.store(false,Ordering::Release);tokio::time::timeout(Duration::from_secs(1),task).await.unwrap().unwrap();
    }
}
