//! O limite de saída precisa valer antes da alocação, pela interface pública do catálogo.
use hangar_connectors::{protocol::ErrorCode, work_actions::Catalog, work_tracking::TicketDetail};
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::process::Command;

struct Bounded;

thread_local! {
    // estavel: o orçamento só é ativado na thread do processo filho, depois de preparar a entrada.
    static BUDGET: Cell<Option<usize>> = const { Cell::new(None) };
}

unsafe impl GlobalAlloc for Bounded {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let allowed = BUDGET.try_with(|budget| match budget.get() {
            Some(remaining) if layout.size() > remaining => false,
            Some(remaining) => { budget.set(Some(remaining - layout.size())); true }
            None => true,
        }).unwrap_or(true);
        // A alocação aprovada e a desalocação conservam o mesmo layout do alocador do sistema.
        if allowed { unsafe { System.alloc(layout) } } else { std::ptr::null_mut() }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Bounded = Bounded;

#[test]
fn work_actions_refuse_large_expansion_without_allocating_the_result() {
    const CHILD: &str = "HANGAR_ACTION_BUDGET_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let url = format!("https://example.invalid/{}", "x".repeat(3 * 1024 * 1024));
        let detail = TicketDetail::from_connector(json!({"id":"t-1", "key":"T-1", "title":"Exemplo", "url":url,
            "status":{"id":"open", "name":"Aberto", "category":"todo"}, "assignee":null, "project":null,
            "updated_at":null, "description_markdown":""})).unwrap();
        let mut catalog = Catalog::from_connector(json!({"actions":[{"id":"start", "label":"Iniciar", "prompt_template":"{url}".repeat(800)}]})).unwrap();
        let action = catalog.actions.remove(0);
        BUDGET.with(|budget| budget.set(Some(128 * 1024)));
        let result = action.resolve(&detail.summary);
        BUDGET.with(|budget| budget.set(None));
        assert!(matches!(result, Err(error) if error.code == ErrorCode::ProtocolError));
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "work_actions_refuse_large_expansion_without_allocating_the_result", "--nocapture"])
        .env_clear().env(CHILD, "1").env("HOME", temp.path()).env("USERPROFILE", temp.path()).env("XDG_CONFIG_HOME", temp.path())
        .env("TZ", "UTC").env("LC_ALL", "C").current_dir(temp.path());
    for name in ["SystemRoot", "WINDIR"] { if let Some(value) = std::env::var_os(name) { command.env(name, value); } }
    let output = command.output().unwrap();
    assert!(output.status.success(), "a recusa precisa terminar dentro do orçamento, sem abortar: {}", String::from_utf8_lossy(&output.stderr));
}
