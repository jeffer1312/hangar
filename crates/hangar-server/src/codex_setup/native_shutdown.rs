//! O prazo normal permite concluir gravações; diretórios só saem após terminar a árvore.
use crate::terminal_process::{self,CommandTree,OwnedTree};
use std::{process::ExitStatus,time::Duration};
use tokio::process::Child;

const GRACE:Duration=Duration::from_secs(3);

pub(super) async fn finish(child:&mut Child,tree:&mut CommandTree)->Result<ExitStatus,String> {
    if !matches!(tokio::time::timeout(GRACE,terminal_process::leader_exited(child)).await,Ok(Ok(()))) {
        terminate_leader(child);
        let _=tokio::time::timeout(GRACE,terminal_process::leader_exited(child)).await;
    }
    let deadline=tokio::time::Instant::now()+GRACE;
    loop {
        let active=if tokio::runtime::Handle::current().runtime_flavor()==tokio::runtime::RuntimeFlavor::MultiThread {
            tokio::task::block_in_place(||tree.active())
        } else {tree.active()};
        if !matches!(active,Ok(true)) || tokio::time::Instant::now()>=deadline {break;}
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    terminal_process::finish(tree).await;
    child.wait().await.map_err(|_|"Não foi possível aguardar o Codex.".into())
}

#[cfg(unix)]
fn terminate_leader(child:&mut Child) {
    unsafe extern "C" {fn kill(pid:i32,signal:i32)->i32;}
    if let Some(pid)=child.id() && unsafe {kill(pid as i32,15)}!=0 {
        let error=std::io::Error::last_os_error();
        if error.raw_os_error()!=Some(3) {tracing::warn!(code="codex_terminate_failed",kind=?error.kind(),"Aguardando contenção da árvore do Codex");}
    }
}
#[cfg(not(unix))]
fn terminate_leader(child:&mut Child) {
    if let Err(error)=child.start_kill(){tracing::warn!(code="codex_terminate_failed",kind=?error.kind(),"Aguardando contenção da árvore do Codex");}
}
