use futures_util::future::BoxFuture;
use hangar_server::share::tunnel::{self, Output, Prereqs, Tailscale, FIX_FUNNEL};
use serde_json::json;
use std::sync::Mutex;

struct Fake {
    state: Mutex<FakeState>,
    linux: bool,
}

struct FakeState {
    funnel_cap: bool,
    operator: Option<&'static str>,
    on: bool,
    funnel_reply: Option<(String, bool)>,
    calls: Vec<Vec<String>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            state: Mutex::new(FakeState { funnel_cap: true, operator: Some("jefferson"), on: false, funnel_reply: None, calls: vec![] }),
            linux: true,
        }
    }
    fn set(&self, f: impl FnOnce(&mut FakeState)) { f(&mut self.state.lock().unwrap()) }
    fn calls(&self) -> Vec<Vec<String>> { self.state.lock().unwrap().calls.clone() }
    fn on(&self) -> bool { self.state.lock().unwrap().on }
}

impl Tailscale for Fake {
    fn run<'a>(&'a self, args: &'a [&'a str]) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move { self.reply(args) })
    }
    fn needs_operator(&self) -> bool { self.linux }
    fn user(&self) -> String { "jefferson".into() }
}

impl Fake {
    fn reply(&self, args: &[&str]) -> Result<Output, String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(args.iter().map(|a| a.to_string()).collect());
        let (mut out, mut success) = (String::new(), true);
        if args.starts_with(&["status", "--json"]) {
            let cap = if s.funnel_cap { json!({"funnel": null, "https": null}) } else { json!({"https": null}) };
            out = json!({"Self": {"ID": "nKA3Du", "DNSName": "nb.tail1.ts.net.", "CapMap": cap}}).to_string();
        } else if args.starts_with(&["debug", "prefs"]) {
            out = match s.operator { Some(op) => json!({"OperatorUser": op}), None => json!({}) }.to_string();
        } else if args.starts_with(&["funnel", "status", "--json"]) {
            out = if s.on { json!({"AllowFunnel": {"nb.tail1.ts.net:8443": true}}) } else { json!({}) }.to_string();
        } else if args[0] == "funnel" && args.last() == Some(&"off") {
            if !s.on { (out, success) = ("error: handler does not exist".into(), false); }
            s.on = false;
        } else if args[0] == "funnel" && s.funnel_reply.is_some() {
            (out, success) = s.funnel_reply.clone().unwrap();
        } else if args[0] == "funnel" {
            s.on = true;
        }
        Ok(Output { success, stdout: out, stderr: String::new() })
    }
}

fn turns_funnel(c: &[String]) -> bool { c[0] == "funnel" && !c.iter().any(|a| a == "status") }

#[tokio::test]
async fn storable_handle_reaches_the_inner_runner() {
    let handle: std::sync::Arc<dyn Tailscale> = std::sync::Arc::new(Fake::new());
    assert_eq!(tunnel::host(&*handle).await.unwrap(), "nb.tail1.ts.net");
    assert!(handle.needs_operator());
    assert_eq!(handle.user(), "jefferson");
}

#[tokio::test]
async fn ensure_on_with_funnel_already_on_does_not_reopen_it() {
    let fake = Fake::new();
    fake.set(|s| s.on = true);
    assert_eq!(tunnel::ensure_on(&fake).await.unwrap(), "https://nb.tail1.ts.net:8443");
    assert!(!fake.calls().iter().any(|c| turns_funnel(c)), "{:?}", fake.calls());
}

#[tokio::test]
async fn cached_host_asks_tailscale_once() {
    let (fake, memo) = (Fake::new(), tunnel::Memo::default());
    assert_eq!(tunnel::cached_host(&fake, &memo).await.unwrap(), "nb.tail1.ts.net");
    assert_eq!(tunnel::cached_host(&fake, &memo).await.unwrap(), "nb.tail1.ts.net");
    assert_eq!(fake.calls().len(), 1, "{:?}", fake.calls());
}

#[tokio::test]
async fn cached_host_remembers_a_failure_for_a_moment() {
    let (memo, fake) = (tunnel::Memo::default(), Fake::new());
    let fix = tunnel::cached_host(&Missing, &memo).await.unwrap_err();
    assert!(tunnel::confirmed_absent(&fix));
    assert!(!tunnel::confirmed_absent("tailscale timeout"));
    assert_eq!(tunnel::cached_host(&fake, &memo).await.unwrap_err(), fix);
    assert!(fake.calls().is_empty(), "falha lembrada não abre processo");
    memo.forget_failure();
    assert_eq!(tunnel::cached_host(&fake, &memo).await.unwrap(), "nb.tail1.ts.net");
}

#[tokio::test]
async fn host_strips_trailing_dot() {
    assert_eq!(tunnel::host(&Fake::new()).await.unwrap(), "nb.tail1.ts.net");
}

#[tokio::test]
async fn ensure_on_opens_only_8443_pointing_at_guest_port() {
    let fake = Fake::new();
    assert_eq!(tunnel::ensure_on(&fake).await.unwrap(), "https://nb.tail1.ts.net:8443");
    let on: Vec<_> = fake.calls().into_iter().filter(|c| turns_funnel(c) && c.last().unwrap() != "off").collect();
    assert_eq!(on, vec![vec!["funnel", "--bg", "--yes", "--https=8443", "http://127.0.0.1:8766"]]);
}

#[tokio::test]
async fn ensure_on_without_operator_and_funnel_says_what_is_missing() {
    let fake = Fake::new();
    fake.set(|s| { s.funnel_cap = false; s.operator = None; });
    let e = tunnel::ensure_on(&fake).await.unwrap_err();
    assert_eq!(e.missing, vec!["operator", "funnel"]);
    assert!(e.fix.contains("tailscale set --operator="));
    assert_eq!(e.enable_url.as_deref(), Some("https://login.tailscale.com/f/funnel?node=nKA3Du"));
    assert!(!fake.calls().iter().any(|c| turns_funnel(c)));
}

#[tokio::test]
async fn only_operator_missing_has_no_funnel_link() {
    let fake = Fake::new();
    fake.set(|s| s.operator = None);
    let e = tunnel::ensure_on(&fake).await.unwrap_err();
    assert_eq!(e.missing, vec!["operator"]);
    assert_eq!(e.enable_url, None);
}

#[tokio::test]
async fn outside_linux_operator_is_not_required() {
    let mut fake = Fake::new();
    fake.linux = false;
    fake.set(|s| s.operator = None);
    assert_eq!(tunnel::ensure_on(&fake).await.unwrap(), "https://nb.tail1.ts.net:8443");
    assert!(!fake.calls().iter().any(|c| c.starts_with(&["debug".into(), "prefs".into()])));
}

#[tokio::test]
async fn link_printed_by_tailscale_beats_the_built_one() {
    let fake = Fake::new();
    fake.set(|s| s.funnel_reply = Some(("Funnel is not enabled on your tailnet.\nTo enable, visit:\n\n         https://login.tailscale.com/f/funnel?node=nKA3Du&x=1\n".into(), false)));
    let e = tunnel::ensure_on(&fake).await.unwrap_err();
    assert_eq!(e.missing, vec!["funnel"]);
    assert_eq!(e.enable_url.as_deref(), Some("https://login.tailscale.com/f/funnel?node=nKA3Du&x=1"));
}

#[tokio::test]
async fn printed_link_loses_sentence_punctuation() {
    let fake = Fake::new();
    fake.set(|s| s.funnel_reply = Some(("To enable, visit (https://login.tailscale.com/f/funnel?node=nKA3Du).".into(), false)));
    let e = tunnel::ensure_on(&fake).await.unwrap_err();
    assert_eq!(e.enable_url.as_deref(), Some("https://login.tailscale.com/f/funnel?node=nKA3Du"));
}

#[tokio::test]
async fn prereqs_only_reads_and_says_what_is_missing() {
    let fake = Fake::new();
    fake.set(|s| s.funnel_cap = false);
    assert_eq!(tunnel::prereqs(&fake).await.unwrap(), Prereqs {
        missing: vec!["funnel".into()], fix: FIX_FUNNEL.into(),
        enable_url: Some("https://login.tailscale.com/f/funnel?node=nKA3Du".into()) });
    fake.set(|s| s.funnel_cap = true);
    assert_eq!(tunnel::prereqs(&fake).await.unwrap(), Prereqs { missing: vec![], fix: String::new(), enable_url: None });
    assert!(!fake.calls().iter().any(|c| c[0] == "funnel"));
}

#[tokio::test]
async fn ensure_off_is_idempotent() {
    let fake = Fake::new();
    tunnel::ensure_off(&fake).await.unwrap();
    fake.set(|s| s.on = true);
    tunnel::ensure_off(&fake).await.unwrap();
    assert!(!fake.on());
}

#[tokio::test]
async fn sync_acts_only_on_the_difference() {
    let fake = Fake::new();
    tunnel::sync(&fake, false).await.unwrap();
    assert!(!fake.calls().iter().any(|c| c.last().unwrap() == "off"));
    tunnel::sync(&fake, true).await.unwrap();
    assert!(fake.on());
    let n = fake.calls().len();
    tunnel::sync(&fake, true).await.unwrap();
    assert!(!fake.calls()[n..].iter().any(|c| turns_funnel(c)));
    tunnel::sync(&fake, false).await.unwrap();
    assert!(!fake.on());
}

struct Missing;
impl Tailscale for Missing {
    fn run<'a>(&'a self, _: &'a [&'a str]) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async { Err("tailscale nao encontrado".into()) })
    }
}

#[tokio::test]
async fn sync_without_tailscale_and_nothing_active_stays_quiet() {
    tunnel::sync(&Missing, false).await.unwrap();
    assert!(tunnel::sync(&Missing, true).await.is_err());
}

// O filho roda com segredo e token no ambiente; o `tailscale` falso imprime o que herdou.
#[cfg(unix)]
#[tokio::test]
async fn real_tailscale_does_not_inherit_private_env() {
    if std::env::var_os("HANGAR_TUNNEL_CHILD_CHECK").is_some() {
        let out = tunnel::Real.run(&["status"]).await.unwrap();
        assert!(out.success, "{}", out.stderr);
        assert!(out.stdout.contains("HANGAR_TUNNEL_CHILD_CHECK=1"), "ambiente não chegou: {}", out.stdout);
        assert!(!out.stdout.contains("HANGAR_INTERNAL_SECRET="), "segredo vazou");
        assert!(!out.stdout.contains("CP_AUTH_TOKEN="), "token vazou");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("tailscale");
    std::fs::write(&script, "#!/bin/sh\nenv\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", dir.path().display(), std::env::var("PATH").unwrap_or_default());
    let name = crate::exact_name(module_path!(), "real_tailscale_does_not_inherit_private_env");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--nocapture"])
        .env("HANGAR_TUNNEL_CHILD_CHECK", "1").env("PATH", path)
        .env("HANGAR_INTERNAL_SECRET", "test-secret").env("CP_AUTH_TOKEN", "test-token")
        .output().unwrap();
    assert!(output.status.success(), "filho falhou: {}", String::from_utf8_lossy(&output.stdout));
    crate::assert_ran_one(&output.stdout);
}
