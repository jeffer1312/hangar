//! O comando público de abertura respeita a escolha e não a reaplica na retomada.
use super::{Account, AccountSource, AgentProvider, PermissionMode, SessionSpec, Surface, SPEC_VERSION, agents};
use serde_json::json;

#[test]
fn kimi_permissions_open_only_passes_flags_to_new_conversations() {
    let home = tempfile::tempdir().unwrap();
    let ctx = agents::AgentContext { home: home.path(), server_env: &Default::default(), runtime_config: &json!({}), read_only: None, windows: false };
    for (mode, flag) in [("yolo", "--yolo"), ("auto", "--auto")] {
        let spec = SessionSpec {
            version: SPEC_VERSION, key: "k".into(), name: "s".into(), cwd: home.path().to_string_lossy().into_owned(), provider: AgentProvider::Kimi,
            surface: Surface::Terminal, account: Account::Default, account_source: AccountSource::Default,
            model: None, effort: None, service_tier: None, permission_mode: PermissionMode::kimi(mode),
            subagent_model: None, customizations: None, jev: false, code_mode: false,
            read_only: false, worktree: None, creator: None, created_at: 1.0, conversation: None,
            name_auto: None, rename_suggestion: None, rename_weak_at: None, rename_dismissed: false, hooks_off: false,
            auto_continue_off: false,
        };
        let pane = agents::pane(&spec, agents::Conversation::New("new"), &ctx).unwrap();
        assert!(pane.command.iter().any(|arg| arg == flag));
        let resumed = agents::pane(&spec, agents::Conversation::Resume("session_11111111-1111-4111-8111-111111111111"), &ctx).unwrap();
        assert!(!resumed.command.iter().any(|arg| arg == flag), "retomar preserva o modo gravado pelo Kimi");
    }
    assert!(PermissionMode::kimi("bypassPermissions").is_none());
    assert!(PermissionMode::kimi("manual").is_none());
}
