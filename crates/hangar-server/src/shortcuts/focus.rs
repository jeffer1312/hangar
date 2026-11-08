//! Foco é conforto: uma janela não encontrada não desfaz o terminal reaproveitado.
use super::terminal::{Service, Terminal};
use crate::routes::AppState;
use std::{collections::HashSet, time::Duration};

pub(super) async fn focus(st: &AppState, service: &Service, terminal: &Terminal) -> bool {
    let row = match service.rows().await {
        Ok(rows) => rows.into_iter().find(|row|row.owner.is_empty() && row.id==terminal.id),
        Err(_) => { tracing::warn!("multiplexador sem resposta ao focar o atalho"); return false; }
    };
    let Some(pid) = row.filter(|row|row.alive).and_then(|row|row.pid).filter(|pid|*pid>0) else { return false; };
    if !cfg!(windows) && !service.env.get("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|v|!v.is_empty()) { return false; }
    let procs = st.list.env().procs.clone();
    let tree = match tokio::task::spawn_blocking(move || {
        procs.children(Duration::ZERO).map(|children|crate::list::discover::descendants(i64::from(pid),&children).into_iter().collect::<HashSet<_>>())
    }).await {
        Ok(Ok(tree)) => tree,
        _ => { tracing::warn!("árvore do processo indisponível ao focar o atalho"); return false; }
    };
    #[cfg(windows)]
    { tokio::task::spawn_blocking(move || windows(&tree)).await.unwrap_or(false) }
    #[cfg(not(windows))]
    {
        let Some(program) = super::guest::find_shell("hyprctl",&service.env) else { return false; };
        let run = |args:Vec<String>| {
            let mut command=crate::terminal_input::child_command(&program);
            command.args(args).env_clear().envs(&service.env).kill_on_drop(true);
            async move { tokio::time::timeout(Duration::from_secs(3),command.output()).await.ok()?.ok() }
        };
        let Some(output)=run(vec!["clients".into(),"-j".into()]).await else { return false; };
        let Ok(clients)=serde_json::from_slice::<serde_json::Value>(&output.stdout) else { return false; };
        let pid=clients.as_array().and_then(|clients|clients.iter().find_map(|client|client["pid"].as_i64().filter(|pid|tree.contains(pid))));
        let Some(pid)=pid else { return false; };
        run(vec!["dispatch".into(),"focuswindow".into(),format!("pid:{pid}")]).await
            .is_some_and(|output|output.status.success() && output.stdout.trim_ascii()==b"ok")
    }
}

#[cfg(windows)]
fn windows(tree:&HashSet<i64>)->bool {
    use windows_sys::core::BOOL;
    use windows_sys::Win32::{Foundation::{HWND,LPARAM},UI::{WindowsAndMessaging::{EnumWindows,GetWindowThreadProcessId,IsWindowVisible,GetWindowTextLengthW,IsIconic,ShowWindow,SW_RESTORE,SetForegroundWindow},Input::KeyboardAndMouse::{keybd_event,VK_MENU,KEYEVENTF_KEYUP}}};
    struct Search<'a> { tree:&'a HashSet<i64>, window:HWND }
    unsafe extern "system" fn each(window:HWND,param:LPARAM)->BOOL {
        let search=unsafe { &mut *(param as *mut Search<'_>) };
        let mut pid=0;
        unsafe { GetWindowThreadProcessId(window,&mut pid); }
        if search.tree.contains(&i64::from(pid)) && unsafe { IsWindowVisible(window)!=0 && GetWindowTextLengthW(window)>0 } {
            search.window=window;
            0
        } else { 1 }
    }
    let mut search=Search {tree,window:std::ptr::null_mut()};
    unsafe {
        EnumWindows(Some(each),&mut search as *mut Search<'_> as LPARAM);
        if search.window.is_null() { return false; }
        if IsIconic(search.window)!=0 { ShowWindow(search.window,SW_RESTORE); }
        keybd_event(VK_MENU as u8,0,0,0);
        let focused=SetForegroundWindow(search.window)!=0;
        keybd_event(VK_MENU as u8,0,KEYEVENTF_KEYUP,0);
        focused
    }
}
