//! S01: esperado dos asserts originais de test_project_shortcuts.py e capturas S01.
use crate::{fake::{OWNER, client}, workspace_fixture};
use serde_json::{Value, json};
use std::{path::{Path, PathBuf}, sync::atomic::Ordering};

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git").arg("-C").arg(root).args(args)
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

struct Scene {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    wt: PathBuf,
    home: PathBuf,
    panes: PathBuf,
    f: workspace_fixture::Fixture,
}

impl Scene {
    async fn new() -> Self { Self::configured(false).await }
    async fn configured(custom: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = hangar_workspace::real(dir.path());
        let repo = root.join("meu-repo");
        let wt = root.join("wt");
        let home = root.join("home");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        std::fs::create_dir(&home).unwrap();
        if custom {
            let env=home.join(".hangar/config/.env");
            std::fs::create_dir_all(env.parent().unwrap()).unwrap();
            std::fs::write(env,format!("CP_PROJECTS_DIR={}\n",home.join("custom-state/projects").display())).unwrap();
        }
        git(&repo, &["init", "-q", "-b", "main"]);
        for (key, value) in [("user.name", "Teste"), ("user.email", "teste@example.invalid"),
            ("core.hooksPath", ""), ("commit.gpgSign", "false")] { git(&repo, &["config", key, value]); }
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "Inicial"]);
        git(&repo, &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()]);
        let panes = home.join("panes.txt");
        let paths = [("main", repo.clone()), ("sub", repo.join("sub")), ("wt", wt.clone()), ("plain", root.clone())];
        std::fs::write(&panes, paths.iter().enumerate().map(|(i,(name,path))|
            format!("fixture-{name}\t1\t\t{}\t%{}\t\tclaude\t1700000000\t0\t0\n", path.display(), i+1)).collect::<String>()).unwrap();
        #[cfg(unix)]
        let mux = {
            let mux = home.join("s01-mux");
            crate::write_executable(&mux, format!("#!/bin/sh\ncase \"$1\" in\nlist-panes) cat '{}' ;;\ndisplay-message) printf '1700000000\\n' ;;\nhas-session) exit 0 ;;\nesac\n", panes.display()));
            mux
        };
        #[cfg(windows)]
        let mux = {
            let mux = home.join("s01-mux.cmd");
            std::fs::write(&mux, format!("@echo off\r\nif \"%~1\"==\"list-panes\" type \"{}\"\r\nif \"%~1\"==\"display-message\" echo 1700000000\r\nexit /b 0\r\n", panes.display())).unwrap();
            mux
        };
        let f = workspace_fixture::policy_fixture(&root, &home, &mux).await;
        Self { _dir:dir, root, repo, wt, home, panes, f }
    }
    fn store(&self) -> PathBuf { self.home.join(".claude").join(".hangar-project-shortcuts.json") }
    fn request(&self, method: &str, name: &str, body: Value) -> reqwest::RequestBuilder {
        client().request(method.parse::<reqwest::Method>().unwrap(), format!("http://{}/api/sessions/fixture-{name}/project-shortcuts", self.f.addr))
            .bearer_auth(OWNER).json(&body)
    }
    fn no_upstream(&self) {
        assert_eq!(self.f.context_hits.load(Ordering::Relaxed), 0);
        assert_eq!(self.f.hits(), 0);
    }
}

async fn value(response: reqwest::Response, status: u16) -> Value {
    let actual = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response.text().await.unwrap();
    assert_eq!(actual, status, "{body}");
    assert_eq!(headers["content-type"], "application/json");
    serde_json::from_str(&body).unwrap()
}
fn item() -> Value { json!({"id":"p1","type":"shell","label":"Debug","command":"true","pasta":"sub","extra":{"unchanged":[1,true,null]}}) }
// O assert original usa os.path.normcase; não muda a grafia da raiz da cópia.
fn project_key(path:&Path)->String {
    let text=path.to_string_lossy().into_owned();
    #[cfg(windows)] { text.replace('/',"\\").to_lowercase() }
    #[cfg(not(windows))] { text }
}

#[cfg(unix)]
struct Execution {
    scene: Scene,
    addr: std::net::SocketAddr,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    runtime: std::sync::Arc<hangar_server::runtime::gateway::RuntimeRegistry>,
    state: std::sync::Arc<hangar_server::routes::AppState>,
}
#[cfg(unix)]
impl Drop for Execution { fn drop(&mut self) { for task in &self.tasks { task.abort(); } } }
#[cfg(unix)]
impl Execution {
    async fn new() -> Self { Self::with_git(None).await }
    async fn with_git(mode: Option<&str>) -> Self { Self::configured(mode,"/bin/sh").await }
    async fn with_shell(shell:&str)->Self { Self::configured(None,shell).await }
    async fn configured(mode:Option<&str>,shell:&str)->Self {
        let scene = Scene::new().await;
        let bin = scene.home.join("bin");
        std::fs::create_dir(&bin).unwrap();
        crate::write_executable(&bin.join("systemctl"),"#!/bin/sh\nexit 1\n");
        crate::write_executable(&bin.join("hyprctl"),format!(r#"#!/bin/sh
if [ "$1" = clients ]; then
  if [ -f '{}' ]; then printf '[{{"pid":%s}}]' "$(cat '{}')"; else printf '[]'; fi
else printf '%s' "$3" > '{}'; printf 'ok\n'; fi
"#,scene.home.join("focus-pid").display(),scene.home.join("focus-pid").display(),scene.home.join("focused").display()));
        if let Some(mode)=mode {
            let script = if mode=="timeout" {
                use std::os::unix::ffi::OsStrExt;
                let fifo=std::ffi::CString::new(scene.home.join("git-block").as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(),0o600) },0);
                format!("#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec /bin/cat '{}'\n",scene.home.join("git-pid").display(),scene.home.join("git-block").display())
            } else { "#!/bin/sh\nprintf 'fatal: injected failure' >&2\nexit 42\n".into() };
            crate::write_executable(&bin.join("git"),script);
        }
        let mux = scene.home.join("execute-mux");
        let terms = scene.home.join("terms");
        std::fs::create_dir(&terms).unwrap();
        crate::write_executable(&mux,format!(r#"#!/bin/sh
base='{}'
case "$1" in
list-panes) cat '{}'; if [ -f "$base/../observe" ]; then printf seen > "$base/../observed"; fi; exit ;;
list-sessions)
  for term in "$base"/*; do
    [ -d "$term" ] || continue
    id=$(cat "$term/@cp_shortcut_id")
    label=$(cat "$term/@cp_shortcut_label")
    owner=$(cat "$term/@cp_shortcut_owner" 2>/dev/null)
    key=$(cat "$term/@cp_shortcut_key" 2>/dev/null)
    origin=$(cat "$term/@cp_shortcut_origin" 2>/dev/null)
    ask=$(cat "$term/@cp_shortcut_ask")
    dead=1; [ -f "$term/live" ] && dead=0
    code=$(cat "$term/code"); pid=0
    [ -f "$term/pid" ] && pid=$(cat "$term/pid")
    printf '%s\t%s\t%s\t1700000000\t%s\t%s\t%s\t1\t%s\t%s\t%s\t%s\n' "${{term##*/}}" "$owner" "$id" "$dead" "$code" "$pid" "$key" "$origin" "$ask" "$label"
  done; exit ;;
new-session)
  shift
  while [ "$1" != -- ]; do
    case "$1" in -s) name=$2; shift;; -c) cwd=$2; shift;; esac
    shift
  done
  shift; shell=$1; shift; shift; command=$1; shift
  term="$base/$name"; mkdir "$term" || exit 1
  printf '%s' "$cwd" > "$term/cwd"
  printf '%s' "$command" > "$term/command"
  code=0
  if [ "$command" = hold ]; then printf live > "$term/live"
  else (cd "$cwd" && "$shell" -c "$command") > "$term/output" 2>&1; code=$?; fi
  printf '%s' "$code" > "$term/code"
  while [ "$#" -gt 0 ]; do
    [ "$1" = ';' ] && shift
    [ "$1" = set-option ] && shift
    [ "$1" = -w ] && shift
    [ "$1" = -t ] && shift
    shift
    option=$1; shift; val=$1; shift
    printf '%s' "$val" > "$term/$option"
  done
  exit 0 ;;
set-option)
  shift; shift; target=$1; shift; option=$1; shift
  target=${{target#=}}; target=${{target%:}}
  printf '%s' "$1" > "$base/$target/$option"; exit ;;
show-options)
  target=${{4#=}}; target=${{target%:}}; option=$5
  cat "$base/$target/$option" 2>/dev/null
  if [ "$option" = @cp_shortcut_cmd ] && [ -f "$base/../restart-observe" ]; then
    printf seen > "$base/../restart-observed"; cat "$base/../restart-release" > /dev/null
  fi
  printf '\n'; exit 0 ;;
display-message) printf '1700000000\n'; exit ;;
display)
  target=$4; target=${{target#=}}; target=${{target%:}}
  if [ -e "$base/$target/live" ]; then printf '0\t\t\n'
  else printf '1\t%s\t\n' "$(cat "$base/$target/code")"; fi; exit ;;
has-session) target=${{3#=}}; [ -d "$base/$target" ]; exit ;;
kill-session) target=${{3#=}}; rm -rf "$base/$target"; exit ;;
capture-pane) target=${{6#=}}; target=${{target%:}}; cat "$base/$target/output"; exit ;;
esac
exit 1
"#,terms.display(),scene.panes.display()));
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hit = hits.clone();
        let trap = axum::Router::new().fallback(move || { let hit=hit.clone(); async move { hit.fetch_add(1,Ordering::Relaxed); "upstream" } });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        let trap_task = tokio::spawn(async move { axum::serve(listener,trap).await.unwrap(); });
        let mut state = hangar_server::routes::AppState::new(crate::fake::config(upstream,"127.0.0.1"));
        state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map([
            ("HOME".into(),scene.home.to_string_lossy().into_owned()),("USERPROFILE".into(),scene.home.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR".into(),scene.home.join(".claude").to_string_lossy().into_owned()),("SHELL".into(),shell.into()),
            ("HYPRLAND_INSTANCE_SIGNATURE".into(),"s01-fixture".into()),
            ("PATH".into(),format!("{}:/usr/bin:/bin",bin.display())),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into()),
        ].into()));
        state.login_guests=scene.f.policy.as_ref().unwrap().login.clone();
        let old = scene.f.list.env();
        let mut env = hangar_server::list::bridge::ListEnv::from_env();
        env.dirs=old.dirs.clone();
        env.capture_program=mux.as_os_str().into();
        env.mux=hangar_server::list::mux::Mux::with_program(&mux,std::time::Duration::from_secs(5));
        state.list=std::sync::Arc::new(hangar_server::list::bridge::ListBridge::new(env,hangar_server::list::facts::FactsClient::new(upstream,crate::fake::SECRET.into())));
        let runtime=std::sync::Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(upstream,crate::fake::SECRET.into(),"s01-execution".into()).with_codex_environment(state.accounts.env.clone()));
        assert!(state.state.runtime.set(runtime.clone()).is_ok());
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr=listener.local_addr().unwrap();
        let state=std::sync::Arc::new(state);
        let app=hangar_server::routes::router(state.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>();
        let task=tokio::spawn(async move { axum::serve(listener,app).await.unwrap(); });
        Self {scene,addr,tasks:vec![trap_task,task],hits,runtime,state}
    }
    async fn run(&self, name:&str, operation:&str, body:Value) -> reqwest::Response {
        client().post(format!("http://{}/api/sessions/fixture-{name}/{operation}",self.addr)).bearer_auth(OWNER).json(&body).send().await.unwrap()
    }
}

#[cfg(unix)]
fn isolated(test:&str) -> bool {
    if std::env::var("S01_EXECUTION_CHILD").as_deref()==Ok(test) { return false; }
    let dir=tempfile::tempdir().unwrap();
    let mut command=std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact",test,"--nocapture"]).env_clear().env("S01_EXECUTION_CHILD",test)
        .env("HOME",dir.path()).env("USERPROFILE",dir.path()).env("TMPDIR",dir.path())
        .env("PATH","/usr/bin:/bin").env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1");
    if let Some(value)=std::env::var_os("LD_LIBRARY_PATH") { command.env("LD_LIBRARY_PATH",value); }
    let output=command.output().unwrap();
    assert!(output.status.success(),"{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    true
}

#[cfg(unix)]
fn fifo(path:&Path) {
    use std::os::unix::ffi::OsStrExt;
    let name=std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe {libc::mkfifo(name.as_ptr(),0o600)},0);
}
#[cfg(unix)]
async fn pipe_signal(path:&Path) {
    use std::{io::Read,os::unix::fs::OpenOptionsExt};
    let file=std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(path).unwrap();
    let signal=tokio::io::unix::AsyncFd::new(file).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            let mut ready=signal.readable().await.unwrap();
            match ready.try_io(|file| { let mut bytes=[0;4]; let mut file=file.get_ref(); file.read(&mut bytes) }) {
                Ok(Ok(n)) if n>0=>break,
                Ok(Err(error))=>panic!("{error}"),
                _=>{},
            }
        }
    }).await.expect("sinal da operação");
}

#[cfg(unix)]
#[tokio::test]
async fn s01_waiting_execution_rechecks_the_life_before_effects() {
    if isolated("project_shortcuts::s01_waiting_execution_rechecks_the_life_before_effects") { return; }
    let execution=Execution::new().await;
    execution.runtime.ingress().close("fixture-main",std::time::Duration::from_secs(5)).await.unwrap();
    let observed=execution.scene.home.join("observed");fifo(&observed);
    std::fs::write(execution.scene.home.join("observe"),"observe").unwrap();
    let output=execution.scene.home.join("new-life.txt");
    let url=format!("http://{}/api/sessions/fixture-main/shortcut-shell",execution.addr);
    let command=format!("pwd > '{}'",output.display());
    let request=tokio::spawn(client().post(url).bearer_auth(OWNER).json(&json!({"command":command})).send());
    pipe_signal(&observed).await;
    std::fs::write(&execution.scene.panes,format!("fixture-main\t1\t\t{}\t%1\t\tclaude\t1700000001\t0\t0\n",execution.scene.wt.display())).unwrap();
    std::fs::remove_file(execution.scene.home.join("observe")).unwrap();
    execution.runtime.ingress().open("fixture-main");
    assert_eq!(value(request.await.unwrap().unwrap(),409).await["detail"]["code"],"session_transfer_busy");
    assert!(!output.exists());
    assert_eq!(std::fs::read_dir(execution.scene.home.join("terms")).unwrap().count(),0);
    value(execution.run("main","shortcut-shell",json!({"command":command})).await,202).await;
    assert_eq!(std::fs::read_to_string(output).unwrap().trim_end(),execution.scene.wt.to_str().unwrap());
    assert_eq!(execution.hits.load(Ordering::Relaxed),0);
}

#[cfg(unix)]
#[tokio::test]
async fn s01_execution_holds_ingress_until_terminal_effects_finish() {
    if isolated("project_shortcuts::s01_execution_holds_ingress_until_terminal_effects_finish") { return; }
    let execution=Execution::new().await;
    let observed=execution.scene.home.join("running");fifo(&observed);
    let release=execution.scene.home.join("release");fifo(&release);
    let output=execution.scene.home.join("effect.txt");
    let command=format!("printf seen > '{}'; cat '{}' > /dev/null; printf effect > '{}'",observed.display(),release.display(),output.display());
    let input=axum::http::Request::builder().method("POST").uri("/api/sessions/fixture-main/run-code")
        .header("authorization",format!("Bearer {OWNER}")).header("content-type","application/json")
        .body(axum::body::Body::from(json!({"command":command}).to_string())).unwrap();
    let request=tokio::spawn(hangar_server::shortcuts::http::public(execution.state.clone(),"127.0.0.1:12345".parse().unwrap(),input));
    pipe_signal(&observed).await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    let mut closing=std::pin::pin!(execution.runtime.ingress().close("fixture-main",std::time::Duration::from_secs(10)));
    assert!(futures_util::poll!(&mut closing).is_pending(),"a transferência não pode passar enquanto o terminal ainda executa");
    assert!(!output.exists());
    std::fs::write(&release,b"go").unwrap();
    closing.await.unwrap();
    execution.runtime.ingress().open("fixture-main");
    assert_eq!(std::fs::read_to_string(output).unwrap(),"effect");
    assert_eq!(execution.hits.load(Ordering::Relaxed),0);
}

#[cfg(unix)]
#[tokio::test]
async fn s01_shell_and_code_execute_in_the_requested_copy_folder() {
    if isolated("project_shortcuts::s01_shell_and_code_execute_in_the_requested_copy_folder") { return; }
    let execution=Execution::new().await;
    for (extra,expected) in [(json!({}),execution.scene.repo.join("sub")),(json!({"pasta":"sub"}),execution.scene.repo.join("sub")),(json!({"pasta":execution.scene.root}),execution.scene.root.clone())] {
        let output=execution.scene.home.join("pwd.txt");
        let mut body=json!({"command":format!("pwd > '{}'",output.display()),"label":"Onde"});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let response=value(execution.run("sub","shortcut-shell",body).await,202).await;
        assert_eq!(response["terminal"]["label"],"Onde");
        assert_eq!(std::fs::read_to_string(&output).unwrap().trim_end(),expected.to_str().unwrap());
    }
    let declared=Execution::with_shell("/bin/false").await;
    let output=declared.scene.home.join("code.txt");
    let marker=declared.scene.home.join("bash.txt");
    let body=json!({"command":format!("[[ -n ${{BASH_VERSION:-}} ]] && printf BASH > '{}'; pwd > '{}'",marker.display(),output.display()),"language":"bash","key":"run-code:proof","pasta":"missing","runs_in":"hangar","label":"not-a-label"});
    let response=value(declared.run("sub","run-code",body).await,202).await;
    assert_eq!(response["terminal"]["label"],"Terminal");
    assert_eq!(std::fs::read_to_string(marker).unwrap(),"BASH");
    assert_eq!(std::fs::read_to_string(output).unwrap().trim_end(),declared.scene.repo.join("sub").to_str().unwrap());
    assert_eq!(declared.hits.load(Ordering::Relaxed),0);
    let response=value(execution.run("sub","shortcut-shell",json!({"command":"true","pasta":"missing"})).await,400).await;
    assert_eq!(response["detail"]["code"],"erro_shortcut_pasta");
    let response=value(execution.run("sub","run-code",json!({"command":"Write-Output 1","language":"powershell"})).await,409).await;
    assert_eq!(response["detail"]["code"],"erro_run_code_shell_incompativel");
    let response=value(execution.run("sub","shortcut-shell",json!({"command":"echo boom >&2; exit 3","label":"falhou"})).await,422).await;
    assert_eq!(response["detail"]["msg"],"o comando saiu com o código 3: boom");
    assert_eq!(response["detail"]["code"],"erro_shortcut_falhou");
    assert_eq!(response["detail"]["params"]["codigo"],3);
    assert_eq!(response["detail"]["params"]["saida"],"boom");
    assert_eq!(response["detail"]["params"]["terminal"]["label"],"falhou");
    assert_eq!(response["detail"]["params"]["terminal"]["alive"],false);
    for (operation,status) in [("shortcut-shell",422),("run-code",400)] {
        let response=value(execution.run("sub",operation,json!({"command":"⟦SEGREDO:PASS⟧"})).await,status).await;
        assert_eq!(response,json!({"detail":{"code":"erro_shortcut_segredo","msg":"preencha a credencial PASS antes de usar","params":{"nome":"PASS"}}}));
    }
    assert_eq!(execution.hits.load(Ordering::Relaxed),0);
}


#[cfg(unix)]
#[tokio::test]
async fn s01_hangar_reuses_one_ownerless_terminal_for_concurrent_clicks() {
    if isolated("project_shortcuts::s01_hangar_reuses_one_ownerless_terminal_for_concurrent_clicks") { return; }
    let execution=Execution::new().await;
    let barrier=std::sync::Arc::new(tokio::sync::Barrier::new(5));
    let mut tasks=Vec::new();
    for name in ["main","sub","wt","plain"] {
        let barrier=barrier.clone();
        let url=format!("http://{}/api/sessions/fixture-{name}/shortcut-shell",execution.addr);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            value(client().post(url).bearer_auth(OWNER).json(&json!({"command":"hold","label":"RDP","runs_in":"hangar","key":"global:k1","pasta":"missing","ask":false})).send().await.unwrap(),202).await
        }));
    }
    barrier.wait().await;
    let mut results=Vec::new();
    for task in tasks { results.push(task.await.unwrap()); }
    assert_eq!(results.iter().filter(|v|v["reused"]==false).count(),1);
    assert_eq!(results.iter().filter(|v|v["reused"]==true).count(),3);
    for result in &results {
        assert_eq!(result["terminal"],results[0]["terminal"]);
        assert_eq!(result["terminal"]["label"],"RDP");
        assert_eq!(result["terminal"]["alive"],true);
        assert_eq!(result["focused"],false);
    }
    let terms=std::fs::read_dir(execution.scene.home.join("terms")).unwrap().map(|e|e.unwrap().path()).collect::<Vec<_>>();
    assert_eq!(terms.len(),1);
    let term=&terms[0];
    assert!(!term.join("@cp_shortcut_owner").exists());
    assert_eq!(std::fs::read_to_string(term.join("@cp_shortcut_key")).unwrap(),"global:k1");
    assert_eq!(std::fs::read_to_string(term.join("@cp_shortcut_ask")).unwrap(),"0");
    let origin=std::fs::read_to_string(term.join("@cp_shortcut_origin")).unwrap();
    assert!(["fixture-main","fixture-sub","fixture-wt","fixture-plain"].contains(&origin.as_str()));
    assert_eq!(std::fs::read_to_string(term.join("cwd")).unwrap(),execution.scene.home.to_str().unwrap());
    let pid=std::process::id().to_string();
    std::fs::write(term.join("pid"),&pid).unwrap();
    std::fs::write(execution.scene.home.join("focus-pid"),&pid).unwrap();
    let focused=value(execution.run("main","shortcut-shell",json!({"command":"hold","runs_in":"hangar","key":"global:k1"})).await,202).await;
    assert_eq!(focused["reused"],true);
    assert_eq!(focused["focused"],true);
    assert_eq!(std::fs::read_to_string(execution.scene.home.join("focused")).unwrap(),format!("pid:{pid}"));
    let output=execution.scene.home.join("home-off.txt");
    let home_off=value(execution.run("main","shortcut-shell",json!({"command":format!("pwd > '{}'",output.display()),"runs_in":"hangar","key":"global:k2","home":false,"pasta":"sub"})).await,202).await;
    assert_eq!(home_off["reused"],false);
    assert_eq!(std::fs::read_to_string(output).unwrap().trim_end(),execution.scene.repo.join("sub").to_str().unwrap());
    let replaced=value(execution.run("main","shortcut-shell",json!({"command":"true","runs_in":"hangar","key":"global:k2"})).await,202).await;
    assert_eq!(replaced["reused"],false);
    assert_ne!(replaced["terminal"]["id"],home_off["terminal"]["id"]);
    let response=value(execution.run("main","shortcut-shell",json!({"command":"true","runs_in":"hangar"})).await,400).await;
    assert_eq!(response["detail"]["code"],"erro_shortcut_sem_chave");
    assert_eq!(execution.hits.load(Ordering::Relaxed),0);
}

#[cfg(unix)]
#[tokio::test]
async fn s01_r1_restart_and_click_share_one_copy_and_preserve_original_spec() {
    if isolated("project_shortcuts::s01_r1_restart_and_click_share_one_copy_and_preserve_original_spec") { return; }
    let execution=Execution::new().await;
    let output=execution.scene.home.join("restart-output.txt");
    let command=format!("printf 'one\\n' >> '{}'\nprintf 'two\\n' >> '{}'",output.display(),output.display());
    let created=value(execution.run("sub","shortcut-shell",json!({"command":command,"label":"Original","runs_in":"hangar","key":"global:restart","ask":false,"home":false,"pasta":"sub"})).await,202).await;
    let id=created["terminal"]["id"].as_str().unwrap();
    let endpoint=|id:&str|format!("http://{}/api/hangar-terminals/{id}/restart",execution.addr);
    let restarted=value(client().post(endpoint(id)).bearer_auth(OWNER).body("{ignored-body").send().await.unwrap(),202).await;
    assert_eq!(restarted["reused"],false);
    assert_eq!(restarted["focused"],false);
    assert_ne!(restarted["terminal"]["id"],created["terminal"]["id"]);
    assert_eq!(std::fs::read_to_string(&output).unwrap(),"one\ntwo\none\ntwo\n");
    let rows=std::fs::read_dir(execution.scene.home.join("terms")).unwrap().map(|e|e.unwrap().path()).collect::<Vec<_>>();
    assert_eq!(rows.len(),1);
    for (option,expected) in [("@cp_shortcut_key","global:restart"),("@cp_shortcut_ask","0"),("@cp_shortcut_origin","fixture-sub"),("@cp_shortcut_label","Original")] {
        assert_eq!(std::fs::read_to_string(rows[0].join(option)).unwrap(),expected);
    }
    assert_eq!(std::fs::read_to_string(rows[0].join("cwd")).unwrap(),execution.scene.repo.join("sub").to_str().unwrap());
    assert_eq!(std::fs::read_to_string(rows[0].join("@cp_shortcut_cmd")).unwrap(),command);
    assert_eq!(value(client().post(endpoint(id)).bearer_auth(OWNER).send().await.unwrap(),404).await["detail"]["code"],"erro_hangar_terminal_inexistente");
    let active_id=restarted["terminal"]["id"].as_str().unwrap();
    std::fs::remove_file(rows[0].join("@cp_shortcut_cmd")).unwrap();
    assert_eq!(value(client().post(endpoint(active_id)).bearer_auth(OWNER).send().await.unwrap(),500).await,
        json!({"detail":{"code":"erro_hangar_terminal_rodar_de_novo","msg":"nao foi possivel recuperar o comando do terminal No Hangar","params":{}}}));
    let mux=execution.scene.home.join("execute-mux");let unavailable=execution.scene.home.join("unavailable-mux");
    std::fs::rename(&mux,&unavailable).unwrap();
    let refused=client().post(endpoint(active_id)).bearer_auth(OWNER).send().await.unwrap();
    std::fs::rename(unavailable,&mux).unwrap();
    assert_eq!(value(refused,500).await["detail"]["code"],"erro_shortcut_mux_indisponivel");
    let held=value(execution.run("main","shortcut-shell",json!({"command":"hold","label":"Shared","runs_in":"hangar","key":"global:concurrent-restart","ask":false})).await,202).await;
    let held_id=held["terminal"]["id"].as_str().unwrap();
    let held_dir=execution.scene.home.join("terms").join(format!("shortcut-hangar-{held_id}"));
    std::fs::remove_file(held_dir.join("live")).unwrap();
    let observed=execution.scene.home.join("restart-observed");fifo(&observed);
    let release=execution.scene.home.join("restart-release");fifo(&release);
    std::fs::write(execution.scene.home.join("restart-observe"),"observe").unwrap();
    let restarting=tokio::spawn(client().post(endpoint(held_id)).bearer_auth(OWNER).send());
    pipe_signal(&observed).await;
    let clicking=tokio::spawn(client().post(format!("http://{}/api/sessions/fixture-wt/shortcut-shell",execution.addr)).bearer_auth(OWNER)
        .json(&json!({"command":"hold","label":"Shared","runs_in":"hangar","key":"global:concurrent-restart","ask":false})).send());
    std::fs::remove_file(execution.scene.home.join("restart-observe")).unwrap();
    std::fs::write(release,b"go").unwrap();
    let a=value(restarting.await.unwrap().unwrap(),202).await;
    let b=value(clicking.await.unwrap().unwrap(),202).await;
    assert_eq!(a["terminal"]["id"],b["terminal"]["id"]);
    assert_eq!([&a,&b].iter().filter(|v|v["reused"]==false).count(),1);
    let live=std::fs::read_dir(execution.scene.home.join("terms")).unwrap().map(|e|e.unwrap().path())
        .filter(|p|std::fs::read_to_string(p.join("@cp_shortcut_key")).unwrap_or_default()=="global:concurrent-restart").collect::<Vec<_>>();
    assert_eq!(live.len(),1);
    assert!(live[0].join("live").is_file());
    assert!(!live[0].join("@cp_shortcut_owner").exists());
    assert_eq!(std::fs::read_to_string(live[0].join("@cp_shortcut_ask")).unwrap(),"0");
    assert!(["fixture-main","fixture-wt"].contains(&std::fs::read_to_string(live[0].join("@cp_shortcut_origin")).unwrap().as_str()));
    assert_eq!(execution.hits.load(Ordering::Relaxed),0);
}

#[cfg(unix)]
#[tokio::test]
async fn s01_git_failure_and_timeout_never_fall_back_inside_repository() {
    if isolated("project_shortcuts::s01_git_failure_and_timeout_never_fall_back_inside_repository") { return; }
    for mode in ["failure","timeout"] {
        let execution=Execution::with_git(Some(mode)).await;
        let response=client().put(format!("http://{}/api/sessions/fixture-sub/project-shortcuts",execution.addr)).bearer_auth(OWNER).json(&json!({"items":[item()]})).send().await.unwrap();
        let reason=if mode=="timeout" { "git timeout" } else { "fatal: injected failure" };
        let message=format!("git falhou em {}: {reason}",execution.scene.repo.join("sub").display());
        assert_eq!(value(response,409).await,json!({"detail":{"code":"erro_project_shortcuts_projeto","msg":message,"params":{"detalhe":message}}}));
        assert!(!execution.scene.store().exists());
        if mode=="timeout" {
            let pid: i32=std::fs::read_to_string(execution.scene.home.join("git-pid")).unwrap().parse().unwrap();
            assert_eq!(unsafe {libc::kill(pid,0)},-1,"o processo do Git deve ser encerrado pelo prazo");
            assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::ESRCH));
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn s01_plain_alias_and_missing_folder_keep_physical_identity() {
    let scene=Scene::new().await;
    let plain=scene.root.join("solta");
    let alias=scene.root.join("atalho");
    std::fs::create_dir(&plain).unwrap();
    std::os::unix::fs::symlink(&plain,&alias).unwrap();
    let mut rows=std::fs::read_to_string(&scene.panes).unwrap();
    rows.push_str(&format!("fixture-alias\t1\t\t{}\t%8\t\tclaude\t1700000000\t0\t0\nfixture-missing\t1\t\t{}\t%9\t\tclaude\t1700000000\t0\t0\n",alias.display(),scene.root.join("sumiu").display()));
    std::fs::write(&scene.panes,rows).unwrap();
    assert_eq!(value(scene.request("GET","alias",Value::Null).send().await.unwrap(),200).await,
        json!({"key":plain,"name":"solta","root":plain,"items":[]}));
    let message=format!("pasta da sessao nao existe: {}",scene.root.join("sumiu").display());
    assert_eq!(value(scene.request("GET","missing",Value::Null).send().await.unwrap(),409).await,
        json!({"detail":{"code":"erro_project_shortcuts_projeto","msg":message,"params":{"detalhe":message}}}));
    scene.no_upstream();
}

#[cfg(unix)]
#[tokio::test]
async fn s01_login_folder_boundary_precedes_effects_and_valid_shell_still_runs() {
    if isolated("project_shortcuts::s01_login_folder_boundary_precedes_effects_and_valid_shell_still_runs") { return; }
    let execution=Execution::new().await;
    let login=&execution.scene.f.policy.as_ref().unwrap().login;
    login.store.create("Ana",execution.scene.repo.to_str().unwrap(),false,true,"ana".into(),"s01-root-token".into()).unwrap();
    login.store.claim("fixture-main","ana",Some("t:1700000000")).unwrap();
    let escape=execution.scene.repo.join("escape");
    std::os::unix::fs::symlink(&execution.scene.root,&escape).unwrap();
    let url=format!("http://{}/api/sessions/fixture-main/shortcut-shell",execution.addr);
    for pasta in [execution.scene.root.to_str().unwrap(),"escape"] {
        let response=client().post(&url).bearer_auth("s01-root-token").json(&json!({"command":"true","pasta":pasta})).send().await.unwrap();
        assert_eq!(value(response,403).await,json!({"detail":{"code":"erro_fora_da_pasta","msg":"o convidado só abre sessão dentro da pasta dele","params":{}}}));
        assert_eq!(std::fs::read_dir(execution.scene.home.join("terms")).unwrap().count(),0);
    }
    let output=execution.scene.repo.join("guest.txt");
    let response=client().post(&url).bearer_auth("s01-root-token").json(&json!({"command":format!("pwd > '{}'",output.display()),"pasta":"sub"})).send().await.unwrap();
    value(response,202).await;
    assert_eq!(std::fs::read_to_string(output).unwrap().trim_end(),execution.scene.repo.join("sub").to_str().unwrap());
    assert_eq!(execution.hits.load(Ordering::Relaxed),0);
}

#[tokio::test]
async fn s01_guest_hangar_denial_precedes_command_and_does_not_reach_python() {
    let scene=Scene::new().await;
    let policy=scene.f.policy.as_ref().unwrap();
    policy.login.store.create("Ana",scene.repo.to_str().unwrap(),false,true,"ana".into(),"s01-login-token".into()).unwrap();
    policy.login.store.claim("fixture-main","ana",Some("t:1700000000")).unwrap();
    let full=policy.share.store.create_redeemed("fixture-main",Some("t:1700000000".into()),hangar_server::share::store::Kind::Share).unwrap().1;
    let pair=policy.share.store.create_redeemed("fixture-main",Some("t:1700000000".into()),hangar_server::share::store::Kind::Pair).unwrap().1;
    for (addr,token,expected) in [
        (scene.f.addr,"s01-login-token","erro_shortcut_hangar_convidado"),
        (policy.guest,full.as_str(),"erro_shortcut_hangar_convidado"),
        (policy.guest,pair.as_str(),"erro_fora_do_convite"),
    ] {
        let response=client().post(format!("http://{addr}/api/sessions/fixture-main/shortcut-shell")).bearer_auth(token)
            .json(&json!({"command":"","runs_in":"hangar","key":"global:k"})).send().await.unwrap();
        assert_eq!(value(response,403).await["detail"]["code"],expected);
    }
    assert!(!scene.home.join("terms").exists());
    scene.no_upstream();
}

#[test]
fn s01_external_json_body_property_has_fixed_seed_and_never_panics() {
    assert!(hangar_server::config_sync::preferences::validate_shortcuts(r#"[{"id":"p1","type":"shell","label":"Debug","command":"true","pasta":3}]"#).is_ok());
    use proptest::{prelude::*, test_runner::{Config,TestRng,TestRunner,RngAlgorithm}};
    let mut runner=TestRunner::new_with_rng(Config { cases:128, failure_persistence:None, ..Config::default() },TestRng::from_seed(RngAlgorithm::ChaCha,&[41;32]));
    let mut headers=axum::http::HeaderMap::new();
    headers.insert("content-type",axum::http::HeaderValue::from_static("application/json"));
    let leaf=prop_oneof![Just(Value::Null),any::<bool>().prop_map(Value::Bool),any::<i64>().prop_map(|v|json!(v)),".{0,40}".prop_map(Value::String)];
    let tree=leaf.prop_recursive(6,128,8,|inner|prop_oneof![
        proptest::collection::vec(inner.clone(),0..8).prop_map(Value::Array),
        proptest::collection::btree_map(".{0,16}",inner,0..8).prop_map(|v|Value::Object(v.into_iter().collect())),
    ]);
    let input=prop_oneof![proptest::collection::vec(any::<u8>(),0..2048),tree.prop_map(|v|serde_json::to_vec(&json!({"items":[v],"command":"true","extra":{"unchanged":true}})).unwrap())];
    runner.run(&input,|input| {
        for schema in [hangar_server::shortcuts::body::Schema::Project,hangar_server::shortcuts::body::Schema::Shell,hangar_server::shortcuts::body::Schema::RunCode] {
            if let Ok(value)=hangar_server::shortcuts::body::parse(schema,&input,&headers) {
                prop_assert!(value.is_object());
                hangar_server::orchestration::codec::discard(value);
            }
        }
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn s01_shared_persistence_through_public_routes_keeps_whole_json() {
    let scene = Scene::new().await;
    std::fs::create_dir_all(scene.store().parent().unwrap()).unwrap();
    std::fs::write(scene.store(), json!({"existing":{"items":[{"id":"keep"}],"extra":true},"metadata":[1,"kept"]}).to_string()).unwrap();
    let expected = json!({"key":project_key(&scene.repo),"name":"meu-repo","root":scene.wt,"items":[item()]});
    let put = value(scene.request("PUT", "wt", json!({"items":[item()],"ignored":true})).send().await.unwrap(), 200).await;
    assert_eq!(put, expected);
    for name in ["main", "sub"] {
        assert_eq!(value(scene.request("GET", name, Value::Null).send().await.unwrap(), 200).await,
            json!({"key":project_key(&scene.repo),"name":"meu-repo","root":scene.repo,"items":[item()]}));
    }
    value(scene.request("PUT", "plain", json!({"items":[{"id":"x","type":"send_text","label":"X","text":"/x"}]})).send().await.unwrap(), 200).await;
    assert_eq!(value(scene.request("PUT", "wt", json!({"items":[]})).send().await.unwrap(), 200).await,
        json!({"key":project_key(&scene.repo),"name":"meu-repo","root":scene.wt,"items":[]}));
    let stored: Value = serde_json::from_slice(&std::fs::read(scene.store()).unwrap()).unwrap();
    assert_eq!(stored, json!({"existing":{"items":[{"id":"keep"}],"extra":true},"metadata":[1,"kept"],
        project_key(&scene.root):{"items":[{"id":"x","type":"send_text","label":"X","text":"/x"}]}}));
    scene.no_upstream();
}

#[tokio::test]
async fn s01_store_uses_the_original_effective_projects_directory() {
    let scene=Scene::configured(true).await;
    let configured=scene.home.join("custom-state/projects");
    let response=value(scene.request("PUT","main",json!({"items":[item()]})).send().await.unwrap(),200).await;
    assert_eq!(response,json!({"key":project_key(&scene.repo),"name":"meu-repo","root":scene.repo,"items":[item()]}));
    let store=configured.parent().unwrap().join(".hangar-project-shortcuts.json");
    assert!(store.is_file(),"o caminho original é settings.projects_dir.parent, incluindo CP_PROJECTS_DIR do .env");
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(store).unwrap()).unwrap(),json!({project_key(&scene.repo):{"items":[item()]}}));
    assert!(!scene.store().exists());
    scene.no_upstream();
}

#[tokio::test]
async fn s01_concurrent_twenty_projects_do_not_lose_updates() {
    let scene = Scene::new().await;
    let mut pane_rows = String::new();
    let mut expected = serde_json::Map::new();
    for i in 0..20 {
        let path = scene.root.join(format!("p{i}"));
        std::fs::create_dir(&path).unwrap();
        pane_rows.push_str(&format!("fixture-p{i}\t1\t\t{}\t%{}\t\tclaude\t1700000000\t0\t0\n", path.display(), i+1));
        expected.insert(project_key(&path),json!({"items":[item()]}));
    }
    std::fs::write(&scene.panes,pane_rows).unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(21));
    let mut tasks = Vec::new();
    for i in 0..20 {
        let request = scene.request("PUT",&format!("p{i}"),json!({"items":[item()]}));
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move { barrier.wait().await; value(request.send().await.unwrap(),200).await }));
    }
    barrier.wait().await;
    for task in tasks { assert_eq!(task.await.unwrap()["items"],json!([item()])); }
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(scene.store()).unwrap()).unwrap(),Value::Object(expected));
    for i in 0..20 { assert_eq!(value(scene.request("GET",&format!("p{i}"),Value::Null).send().await.unwrap(),200).await["items"],json!([item()])); }
    scene.no_upstream();
}

#[tokio::test]
async fn s01_unreadable_store_is_500_and_put_preserves_exact_bytes() {
    let scene = Scene::new().await;
    std::fs::create_dir_all(scene.store().parent().unwrap()).unwrap();
    for (bytes,reason) in [
        (b"{nao e json".as_slice(),"Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"),
        (b"[1, 2]".as_slice(),"esperado um objeto JSON"),
        (b"\xff\xfe".as_slice(),"'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"),
    ] {
        std::fs::write(scene.store(),bytes).unwrap();
        let message = format!("{}: {reason}",scene.store().display());
        let expected = json!({"detail":{"code":"erro_project_shortcuts_arquivo","msg":message,"params":{"detalhe":message}}});
        for method in ["GET","PUT"] {
            assert_eq!(value(scene.request(method,"main",json!({"items":[item()]})).send().await.unwrap(),500).await,expected);
            assert_eq!(std::fs::read(scene.store()).unwrap(),bytes);
        }
    }
    scene.no_upstream();
}

#[tokio::test]
async fn s01_r2_gzip_identity_threshold_cors_and_errors_match_starlette() {
    use std::io::Read;
    let scene=Scene::new().await;
    let client=reqwest::Client::builder().no_gzip().build().unwrap();
    let url=format!("http://{}/api/sessions/fixture-main/project-shortcuts",scene.f.addr);
    let mut sample=item();sample["extra"]=json!("");
    let base=json!({"key":project_key(&scene.repo),"name":"meu-repo","root":scene.repo,"items":[sample.clone()]});
    let base_len=serde_json::to_vec(&base).unwrap().len();
    assert!(base_len<512);
    let mut failures=Vec::new();
    for size in [512,1023,1024,2048] {
        sample["extra"]=json!("x".repeat(size-base_len));
        let expected=json!({"key":project_key(&scene.repo),"name":"meu-repo","root":scene.repo,"items":[sample.clone()]});
        for encoding in ["identity","gzip"] {
            for method in [reqwest::Method::PUT,reqwest::Method::GET] {
                let mut request=client.request(method.clone(),&url).bearer_auth(OWNER).header("origin","http://other-server.invalid").header("accept-encoding",encoding);
                if method==reqwest::Method::PUT { request=request.json(&json!({"items":[sample.clone()]})); }
                let response=request.send().await.unwrap();
                let status=response.status().as_u16();let headers=response.headers().clone();let bytes=response.bytes().await.unwrap();
                let gzip=encoding=="gzip";
                let actual_gzip=headers.get("content-encoding").is_some_and(|h|h=="gzip");
                let mut decoded=Vec::new();
                if actual_gzip { flate2::read::GzDecoder::new(bytes.as_ref()).read_to_end(&mut decoded).unwrap(); }
                else { decoded.extend_from_slice(&bytes); }
                let vary=headers.get("vary").and_then(|h|h.to_str().ok());
                if status!=200 || actual_gzip!=gzip || vary!=Some("Accept-Encoding")
                    || headers.get("content-length").and_then(|h|h.to_str().ok()).and_then(|s|s.parse::<usize>().ok())!=(if gzip{None}else{Some(size)})
                    || headers.get("transfer-encoding").and_then(|h|h.to_str().ok())!=(if gzip{Some("chunked")}else{None})
                    || headers.get("access-control-expose-headers").and_then(|h|h.to_str().ok())!=Some("ETag, x-hangar-css-width, x-hangar-css-height")
                    || headers.get("access-control-allow-origin").and_then(|h|h.to_str().ok())!=Some("*")
                    || decoded.len()!=size || serde_json::from_slice::<Value>(&decoded).ok()!=Some(expected.clone()) {
                    failures.push(format!("{method} {encoding} size={size}: status={status},headers={headers:?},encoded={},decoded={}",bytes.len(),decoded.len()));
                }
            }
        }
    }
    let long="x".repeat(2048);
    let response=client.put(&url).bearer_auth(OWNER).header("accept-encoding","gzip").header("origin","http://other-server.invalid").json(&json!({"items":long})).send().await.unwrap();
    let status=response.status().as_u16();let headers=response.headers().clone();let bytes=response.bytes().await.unwrap();
    let mut decoded=Vec::new();
    if headers.get("content-encoding").is_some_and(|h|h=="gzip") { flate2::read::GzDecoder::new(bytes.as_ref()).read_to_end(&mut decoded).unwrap(); }
    else { decoded.extend_from_slice(&bytes); }
    let expected=json!({"detail":[{"type":"list_type","loc":["body","items"],"msg":"Input should be a valid list","input":long}]});
    if status!=422 || headers.get("vary").and_then(|h|h.to_str().ok())!=Some("Accept-Encoding")
        || headers.get("content-encoding").and_then(|h|h.to_str().ok())!=Some("gzip")
        || serde_json::from_slice::<Value>(&decoded).ok()!=Some(expected) {
        failures.push(format!("erro grande status={status},headers={headers:?}"));
    }
    for (authorized,body,status) in [(false,None,401),(true,Some(json!({"items":"wrong"})),422)] {
        let request=if let Some(body)=body { client.put(&url).json(&body) } else { client.get(&url) };
        let request=if authorized { request.bearer_auth(OWNER) } else { request };
        let response=request.header("accept-encoding","gzip").send().await.unwrap();
        assert_eq!(response.status(),status);
        assert!(!response.headers().contains_key("content-encoding"));assert!(!response.headers().contains_key("vary"));
        let expected=if authorized {
            json!({"detail":[{"type":"list_type","loc":["body","items"],"msg":"Input should be a valid list","input":"wrong"}]})
        } else { json!({"detail":{"code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}) };
        assert_eq!(response.json::<Value>().await.unwrap(),expected);
    }
    let response=client.head(&url).header("accept-encoding","gzip").send().await.unwrap();
    assert_eq!(response.status(),405);assert_eq!(response.headers()["content-length"],"31");
    assert!(!response.headers().contains_key("content-encoding"));assert!(!response.headers().contains_key("vary"));
    assert!(response.bytes().await.unwrap().is_empty());
    let response=client.request(reqwest::Method::OPTIONS,&url).header("accept-encoding","gzip").header("origin","http://other-server.invalid").header("access-control-request-method","PUT").send().await.unwrap();
    assert_eq!(response.status(),200);assert_eq!(response.headers()["content-length"],"2");
    assert!(!response.headers().contains_key("content-encoding"));assert!(!response.headers().contains_key("vary"));
    assert_eq!(response.text().await.unwrap(),"OK");
    scene.no_upstream();
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}

#[tokio::test]
async fn s01_http_headers_syntax_and_cookie_precedence_do_not_need_fastapi() {
    let scene=Scene::new().await;
    let raw=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    for (operation,method,allowed) in [("project-shortcuts","POST","GET"),("project-shortcuts","HEAD","GET"),("shortcut-shell","GET","POST"),("run-code","HEAD","POST")] {
        let response=raw.request(method.parse::<reqwest::Method>().unwrap(),format!("http://{}/api/sessions/fixture-main/{operation}",scene.f.addr)).send().await.unwrap();
        assert_eq!(response.status(),405);
        assert_eq!(response.headers()["allow"],allowed);
        assert_eq!(response.headers()["content-type"],"application/json");
        assert_eq!(response.headers()["content-length"],"31");
        assert_eq!(response.text().await.unwrap(),if method=="HEAD"{""}else{r#"{"detail":"Method Not Allowed"}"#});
    }
    for (operation,method) in [("project-shortcuts","PUT"),("shortcut-shell","POST"),("run-code","POST")] {
        let response=raw.request(method.parse::<reqwest::Method>().unwrap(),format!("http://{}/api/sessions/fixture-main/{operation}",scene.f.addr)).header("content-type","application/json").body("{").send().await.unwrap();
        assert_eq!(value(response,422).await,json!({"detail":[{"type":"json_invalid","loc":["body",1],"msg":"JSON decode error","input":{},"ctx":{"error":"Expecting property name enclosed in double quotes"}}]}));
        let body=if operation=="project-shortcuts"{json!({"items":[]})}else{json!({"command":"true"})};
        let response=raw.request(method.parse::<reqwest::Method>().unwrap(),format!("http://{}/api/sessions/fixture-main/{operation}",scene.f.addr)).json(&body).send().await.unwrap();
        assert_eq!(value(response,401).await["detail"]["code"],"erro_nao_autorizado");
        let response=raw.request(method.parse::<reqwest::Method>().unwrap(),format!("http://{}/api/sessions/missing/{operation}",scene.f.addr)).bearer_auth(OWNER).json(&body).send().await.unwrap();
        assert_eq!(value(response,404).await,json!({"detail":{"code":"erro_sessao_inexistente","msg":"sessao nao encontrada","params":{}}}));
        let url=format!("http://{}/api/sessions/fixture-main/{operation}/?x=%2F",scene.f.addr);
        let response=raw.request(method.parse::<reqwest::Method>().unwrap(),url).send().await.unwrap();
        assert_eq!(response.status(),307);
        assert_eq!(response.headers()["location"],format!("http://{}/api/sessions/fixture-main/{operation}?x=%2F",scene.f.addr));
    }
    let url=format!("http://{}/api/sessions/fixture-main/project-shortcuts",scene.f.addr);
    let response=raw.get(&url).header("cookie",format!("cp_token={OWNER}")).header("origin","http://other-server.invalid").send().await.unwrap();
    assert_eq!(response.headers()["access-control-allow-origin"],"*");
    assert_eq!(response.headers()["access-control-expose-headers"],"ETag, x-hangar-css-width, x-hangar-css-height");
    assert_eq!(value(response,200).await,json!({"key":project_key(&scene.repo),"name":"meu-repo","root":scene.repo,"items":[]}));
    assert_eq!(value(raw.put(&url).header("cookie",format!("cp_token={OWNER}")).json(&json!({"items":[]})).send().await.unwrap(),401).await["detail"]["code"],"erro_nao_autorizado");
    let response=raw.request(reqwest::Method::OPTIONS,&url).header("origin","http://other-server.invalid").header("access-control-request-method","PUT").header("access-control-request-headers","authorization,content-type").send().await.unwrap();
    assert_eq!(response.status(),200);
    assert_eq!(response.headers()["access-control-allow-methods"],"DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT");
    assert_eq!(response.text().await.unwrap(),"OK");
    let items=vec![Value::Null;501];
    let response=value(scene.request("PUT","main",json!({"items":items})).send().await.unwrap(),422).await;
    assert_eq!(response["detail"][0]["type"],"too_long");
    assert_eq!(response["detail"][0]["ctx"],json!({"field_type":"List","max_length":500,"actual_length":501}));
    scene.no_upstream();
}

#[tokio::test]
async fn s01_target_guard_ignores_corrupt_other_transfer_and_refuses_own_corruption() {
    let scene=Scene::new().await;
    let expected=json!({"key":project_key(&scene.repo),"name":"meu-repo","root":scene.repo,"items":[]});
    assert_eq!(value(scene.request("GET","main",Value::Null).send().await.unwrap(),200).await,expected);
    let id="99999999-2222-4333-8444-555555555555";
    let base=scene.home.join(".hangar/conversation-transfers");
    std::fs::create_dir_all(base.join("indexes")).unwrap();
    std::fs::write(base.join(format!("{id}.json")),"{broken").unwrap();
    let index=|name:&str| {
        let hash=ring::digest::digest(&ring::digest::SHA256,name.as_bytes()).as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
        base.join("indexes").join(format!("session-{hash}.json"))
    };
    std::fs::write(index("unrelated-session"),json!([id]).to_string()).unwrap();
    assert_eq!(value(scene.request("GET","main",Value::Null).send().await.unwrap(),200).await,expected);
    let body=json!({"items":[{"id":"i","type":"internal","action":"terminal"}]});
    let response=scene.request("PUT","main",body.clone()).send().await.unwrap();
    assert!(!scene.store().exists());scene.no_upstream();
    assert_eq!(value(response,400).await,json!({"detail":{"code":"erro_project_shortcuts","msg":"item 1: atalho interno nao vale por projeto","params":{"detalhe":"item 1: atalho interno nao vale por projeto"}}}));
    std::fs::write(index("fixture-main"),json!([id]).to_string()).unwrap();
    assert_eq!(value(scene.request("PUT","main",body).send().await.unwrap(),503).await,
        json!({"detail":{"code":"shortcut_context_unavailable","msg":"contexto do terminal indisponível","params":{}}}));
    assert!(!scene.store().exists());scene.no_upstream();
}

#[tokio::test]
async fn s01_validation_methods_auth_and_lifecycle_are_native() {
    let scene = Scene::new().await;
    let url = format!("http://{}/api/sessions/fixture-main/project-shortcuts",scene.f.addr);
    for method in [reqwest::Method::GET,reqwest::Method::PUT] {
        assert_eq!(value(client().request(method,&url).json(&json!({"items":[]})).send().await.unwrap(),401).await,
            json!({"detail":{"code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}));
    }
    for (input,expected) in [
        (json!({}),json!({"detail":[{"type":"missing","loc":["body","items"],"msg":"Field required","input":{}}]})),
        (json!({"items":{}}),json!({"detail":[{"type":"list_type","loc":["body","items"],"msg":"Input should be a valid list","input":{}}]})),
    ] { assert_eq!(value(scene.request("PUT","main",input).send().await.unwrap(),422).await,expected); }
    for (items,message) in [
        (json!([{"id":"t","type":"internal","action":"terminal"}]),"item 1: atalho interno nao vale por projeto"),
        (json!([{"id":"p1","type":"shell","label":"X","command":"true","pasta":"  "}]),"item 1 (shell) com pasta vazia"),
        (json!([{"id":"p1","type":"shell","label":"X","command":"true","pasta":3}]),"item 1 (shell) com pasta vazia"),
        (json!([item(),item()]),"item 2: id 'p1' repetido"),
        (json!([{"id":"x","type":"shell","label":"X"}]),"item 1 (shell) sem comando"),
    ] {
        assert_eq!(value(scene.request("PUT","main",json!({"items":items})).send().await.unwrap(),400).await,
            json!({"detail":{"code":"erro_project_shortcuts","msg":message,"params":{"detalhe":message}}}));
    }
    let response = client().post(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.headers()["allow"],"GET");
    assert_eq!(value(response,405).await,json!({"detail":"Method Not Allowed"}));
    let id = "11111111-2222-4333-8444-555555555555";
    let base = scene.home.join(".hangar/conversation-transfers");
    std::fs::create_dir_all(base.join("indexes")).unwrap();
    std::fs::write(base.join(format!("{id}.json")),json!({"id":id,"name":"fixture-main","source_life":"t:1700000000","phase":"source_stopped","source":null,"origin_meta":{"name":"fixture-main","provider":"claude","cwd":scene.repo,"jsonl":scene.home.join("fixture.jsonl"),"key":"fixture-key"},"destination_meta":null,"boundary":null,"error_code":null}).to_string()).unwrap();
    let key = ring::digest::digest(&ring::digest::SHA256,b"fixture-main").as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
    std::fs::write(base.join("indexes").join(format!("session-{key}.json")),json!([id]).to_string()).unwrap();
    for body in [json!({"items":[]}),json!({})] {
        assert_eq!(value(scene.request("PUT","main",body).send().await.unwrap(),409).await,
            json!({"detail":{"code":"session_transfer_busy","msg":"A sessão está trocando de agente; tente novamente quando terminar.","params":{}}}));
    }
    let rows = std::fs::read_to_string(&scene.panes).unwrap().replace("1700000000","1700000001");
    std::fs::write(&scene.panes,rows).unwrap();
    assert_eq!(value(scene.request("PUT","main",json!({"items":[]})).send().await.unwrap(),200).await,
        json!({"key":project_key(&scene.repo),"name":"meu-repo","root":scene.repo,"items":[]}));
    scene.no_upstream();
}

#[cfg(target_os = "linux")]
fn s01_real_python_private_child() -> bool {
    use std::{io::Write, process::Command};
    const TEST: &str = "project_shortcuts::s01_real_python_get_reads_three_rust_terminals_from_private_tmux";
    if std::env::var("S01_REAL_PYTHON_SANDBOX").as_deref() == Ok("1")
        || std::env::var("S01_REAL_PYTHON_CHILD").as_deref() == Ok(TEST) { return false; }

    fn copy_source(from: &Path, to: &Path) {
        let metadata = std::fs::symlink_metadata(from).expect("fonte do checkout ausente");
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink(), "fonte não é diretório sem links: {}", from.display());
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            let name = entry.file_name();
            let text = name.to_string_lossy();
            assert!(!kind.is_symlink(), "link recusado na fonte privada: {}", entry.path().display());
            assert!(!matches!(text.as_ref(), ".git" | ".env" | "peers.json" | "credentials.json" | "auth.json"
                | "runtime-config.json" | ".claude" | ".codex" | ".hangar" | ".ssh") && !text.starts_with(".env."),
                "configuração pessoal recusada na fonte: {}", entry.path().display());
            if text == "__pycache__" { continue; }
            if kind.is_dir() { copy_source(&entry.path(), &to.join(&name)); }
            else {
                assert!(kind.is_file(), "entrada não regular na fonte: {}", entry.path().display());
                std::fs::copy(entry.path(), to.join(&name)).unwrap();
            }
        }
    }

    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let python = std::env::var_os("S01_TEST_PYTHON").map(PathBuf::from)
        .unwrap_or_else(|| checkout.join("backend/.venv/bin/python"));
    assert!(python.is_absolute() && python.is_file(),
        "Python3.14 do checkout ausente em {}; use apenas override explícito S01_TEST_PYTHON", python.display());
    let dir = tempfile::tempdir().unwrap();
    let base = hangar_workspace::real(dir.path());
    let source = base.join("source");
    copy_source(&checkout.join("backend/app"), &source.join("backend/app"));
    copy_source(&checkout.join("messages"), &source.join("messages"));
    let home = base.join("home");
    let tmp = base.join("tmp");
    let runtime = base.join("runtime");
    let bin = base.join("bin");
    for path in [home.join(".hangar/config"), tmp.clone(), runtime.clone(), bin.clone()] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(home.join(".hangar/config/.env"), "").unwrap();
    std::fs::write(home.join(".hangar/backend-dir"), source.join("backend").to_string_lossy().as_bytes()).unwrap();
    let mux = bin.join("tmux");
    let socket = runtime.join("s01.sock");
    crate::write_executable(&mux, "#!/bin/sh\n[ \"${S01_TMUX_SOCKET:-}\" = \"$S01_CASE_ROOT/runtime/s01.sock\" ] || exit 90\nexec /usr/bin/tmux -S \"$S01_TMUX_SOCKET\" -f /dev/null \"$@\"\n");
    let mut child = Command::new(std::env::current_exe().unwrap());
    child.args(["--exact", TEST, "--nocapture", "--test-threads=1"]).env_clear().current_dir(&base)
        .env("S01_REAL_PYTHON_CHILD", TEST).env("S01_CASE_ROOT", &base)
        .env("S01_PYTHON_BACKEND", source.join("backend")).env("S01_PYTHON_EXECUTABLE", &python)
        .env("S01_REAL_TMUX", &mux).env("S01_TMUX_SOCKET", &socket)
        .env("HOME", &home).env("USERPROFILE", &home).env("TMPDIR", &tmp)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display())).env("SHELL", "/bin/bash").env("USER", "contract")
        .env("XDG_CONFIG_HOME", home.join(".config")).env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share")).env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_RUNTIME_DIR", &runtime).env("CLAUDE_CONFIG_DIR", home.join(".claude")).env("CODEX_HOME", home.join(".codex"))
        .env("CP_AUTH_TOKEN", OWNER).env("CP_RUST_SERVER", "0").env("CP_RUST_NO_ORPHAN_SWEEP", "1").env("NETWORK", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CEILING_DIRECTORIES", &base).env("PYTHONPATH", source.join("backend"))
        .env("PYTHONNOUSERSITE", "1").env("PYTHONDONTWRITEBYTECODE", "1")
        .env("LANG", "C.UTF-8").env("LC_ALL", "C.UTF-8").env("TZ", "UTC");
    if let Some(value) = std::env::var_os("LD_LIBRARY_PATH") { child.env("LD_LIBRARY_PATH", value); }
    let output = child.output().expect("falha lançando filho privado da prova S01");
    let leftover = socket.exists();
    if leftover {
        let closed = Command::new(&mux).arg("kill-server").env_clear().env("S01_CASE_ROOT", &base)
            .env("S01_TMUX_SOCKET", &socket).output().expect("falha encerrando tmux privado deixado pelo filho");
        assert!(closed.status.success() && !socket.exists(), "teardown do tmux privado falhou: {closed:?}");
    }
    std::io::stdout().write_all(&output.stdout).unwrap();
    std::io::stderr().write_all(&output.stderr).unwrap();
    assert!(!leftover, "filho deixou socket/tmux privado após terminar");
    assert!(output.status.success(), "filho S01 falhou: {}", output.status);
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed; 0 failed; 0 ignored;"),
        "o filho deve executar esta única função, sem ignorar a prova");
    true
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn s01_real_python_get_reads_three_rust_terminals_from_private_tmux() {
    use std::{collections::BTreeMap, io::Write, process::{Command, Stdio}, time::Duration};
    use tokio::io::AsyncBufReadExt;

    if s01_real_python_private_child() { return; }
    let driver = std::env::var("S01_REAL_PYTHON_SANDBOX").as_deref() == Ok("1");
    let case_root = PathBuf::from(std::env::var_os("S01_CASE_ROOT").expect("pasta privada obrigatória"));
    let backend = PathBuf::from(std::env::var_os("S01_PYTHON_BACKEND").expect("fonte candidata privada obrigatória"));
    let python = PathBuf::from(std::env::var_os("S01_PYTHON_EXECUTABLE").expect("Python3.14 explícito obrigatório"));
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let tmp = PathBuf::from(std::env::var_os("TMPDIR").expect("TMPDIR privado obrigatório"));
    let mux = PathBuf::from(std::env::var_os("S01_REAL_TMUX").expect("tmux privado obrigatório"));
    let socket = PathBuf::from(std::env::var_os("S01_TMUX_SOCKET").expect("socket privado obrigatório"));
    assert!(case_root.is_absolute() && case_root.is_dir());
    assert_eq!(home, case_root.join("home"));
    assert_eq!(PathBuf::from(std::env::var_os("USERPROFILE").unwrap()), home);
    assert_eq!(tmp, case_root.join("tmp"));
    assert_eq!(mux, case_root.join("bin/tmux"));
    assert_eq!(socket, case_root.join("runtime/s01.sock"));
    assert!(mux.is_file() && !socket.exists());
    assert!(backend.is_absolute() && backend.join("app/api.py").is_file());
    assert!(python.is_absolute() && python.is_file(), "Python3.14 ausente em {}", python.display());
    assert_eq!(std::env::var("CP_RUST_SERVER").as_deref(), Ok("0"));
    assert_eq!(std::env::var("CP_RUST_NO_ORPHAN_SWEEP").as_deref(), Ok("1"));
    if driver {
        assert_eq!(case_root, Path::new("/mnt/case"));
        assert_eq!(backend, Path::new("/mnt/source/backend"));
        assert_eq!(python, Path::new("/usr/bin/python3.14"));
        assert_eq!(std::env::var("S01_MAIN_SOURCE_VERIFIED").as_deref(), Ok("1"));
        for (kind, host) in [("pid", "HOST_PID_NS"), ("net", "HOST_NET_NS"),
            ("mnt", "HOST_MNT_NS"), ("ipc", "HOST_IPC_NS")] {
            assert_ne!(std::fs::read_link(format!("/proc/self/ns/{kind}")).unwrap().to_string_lossy(),
                std::env::var(host).expect("namespace do host obrigatório"));
        }
        assert_eq!(std::fs::read_link("/proc/self/ns/pid").unwrap(), std::fs::read_link("/proc/1/ns/pid").unwrap());
    } else {
        assert_eq!(std::env::var("S01_REAL_PYTHON_CHILD").as_deref(),
            Ok("project_shortcuts::s01_real_python_get_reads_three_rust_terminals_from_private_tmux"));
        assert!(backend.starts_with(&case_root));
        assert_eq!(PathBuf::from(std::env::var_os("GIT_CEILING_DIRECTORIES").unwrap()), case_root);
    }

    // Mesmo Scene de metadados, sem subir seu upstream falso ou a porta de convidados.
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("meu-repo");
    let wt = root.join("wt");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::create_dir_all(home.join(".claude/projects")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    for (key, value) in [("user.name", "Teste"), ("user.email", "teste@example.invalid"),
        ("core.hooksPath", ""), ("commit.gpgSign", "false")] { git(&repo, &["config", key, value]); }
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "Inicial"]);
    git(&repo, &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()]);
    let panes = home.join("panes.txt");
    let paths = [("main", repo.clone()), ("sub", repo.join("sub")), ("wt", wt.clone()), ("plain", root.clone())];
    std::fs::write(&panes, paths.iter().enumerate().map(|(i,(name,path))|
        format!("fixture-{name}\t1\t\t{}\t%{}\t\tclaude\t1700000000\t0\t0\n", path.display(), i+1)).collect::<String>()).unwrap();
    let metadata_mux = home.join("s01-mux");
    crate::write_executable(&metadata_mux, format!("#!/bin/sh\ncase \"$1\" in\nlist-panes) cat '{}' ;;\ndisplay-message) printf '1700000000\\n' ;;\nhas-session) exit 0 ;;\nesac\n", panes.display()));

    let mut environment = std::env::vars().collect::<BTreeMap<String, String>>();
    environment.insert("CP_AUTH_TOKEN".into(), OWNER.into());
    environment.insert("CLAUDE_CONFIG_DIR".into(), home.join(".claude").to_string_lossy().into_owned());
    environment.insert("CP_PROJECTS_DIR".into(), home.join(".claude/projects").to_string_lossy().into_owned());
    environment.insert("CP_SCAN_ROOTS".into(), root.to_string_lossy().into_owned());
    crate::write_executable(&case_root.join("bin/hyprctl"), format!(r#"#!/bin/sh
if [ "$1" = clients ]; then
  if [ -f '{}' ]; then printf '[{{"pid":%s}}]' "$(cat '{}')"; else printf '[]'; fi
else printf '%s' "$3" > '{}'; printf 'ok\n'; fi
"#,home.join("focus-pid").display(),home.join("focus-pid").display(),home.join("focused").display()));
    environment.insert("HYPRLAND_INSTANCE_SIGNATURE".into(), "s01-fixture".into());
    let version = Command::new(&python).arg("--version").env_clear().envs(&environment)
        .current_dir(&backend).output().expect("falha consultando a versão do Python explícito");
    let version_text = format!("{}{}", String::from_utf8_lossy(&version.stdout), String::from_utf8_lossy(&version.stderr));
    assert!(version.status.success() && version_text.trim().starts_with("Python 3.14."),
        "{} deve ser Python3.14: status={}, saída={version_text}", python.display(), version.status);
    let log = home.join("python-uvicorn.log");
    let mut output = std::fs::File::create(&log).unwrap();
    struct Processes { python: std::process::Child, mux: PathBuf, socket: PathBuf }
    impl Drop for Processes {
        fn drop(&mut self) {
            let mut errors = Vec::new();
            let mut server = None;
            if self.socket.exists() {
                match Command::new(&self.mux).args(["display-message", "-p", "#{pid}"]).output() {
                    Ok(output) if output.status.success() => {
                        server = String::from_utf8_lossy(&output.stdout).trim().parse::<u32>().ok()
                            .and_then(|pid| hangar_server::terminal_facts::agent_birth(pid).map(|birth| (pid, birth)));
                        if server.is_none() { errors.push("identidade do tmux privado indisponível".into()); }
                    },
                    other => errors.push(format!("falha consultando o PID do tmux privado: {other:?}")),
                }
                match Command::new(&self.mux).arg("kill-server").output() {
                    Ok(output) if output.status.success() => {},
                    Ok(output) => errors.push(format!("tmux privado não encerrou: {output:?}")),
                    Err(error) => errors.push(format!("falha encerrando tmux privado: {error}")),
                }
            }
            let running = match self.python.try_wait() {
                Ok(status) => status.is_none(),
                Err(error) => { errors.push(format!("falha consultando filho Python: {error}")); true },
            };
            if running && let Err(error) = self.python.kill() {
                errors.push(format!("falha encerrando filho Python: {error}"));
            }
            if let Err(error) = self.python.wait() { errors.push(format!("falha recolhendo filho Python: {error}")); }
            if let Some((pid, birth)) = server {
                let limit = std::time::Instant::now() + Duration::from_secs(10);
                while hangar_server::terminal_facts::agent_birth(pid) == Some(birth) {
                    if std::time::Instant::now() >= limit {
                        errors.push(format!("tmux privado permaneceu vivo: {pid}"));
                        break;
                    }
                    std::thread::yield_now();
                }
                if hangar_server::terminal_facts::agent_birth(pid) != Some(birth) && self.socket.exists()
                    && let Err(error) = std::fs::remove_file(&self.socket) {
                        errors.push(format!("falha removendo socket inativo privado: {error}"));
                    }
            }
            if !errors.is_empty() {
                if std::thread::panicking() { eprintln!("teardown S01 falhou: {}", errors.join("; ")); }
                else { panic!("teardown S01 falhou: {}", errors.join("; ")); }
            }
        }
    }
    let mut processes = Processes {
        python: Command::new(&python)
            .args(["-m", "uvicorn", "app.api:app", "--lifespan", "off", "--host", "127.0.0.1", "--port", "0",
                "--no-access-log", "--no-use-colors"])
            .env_clear().envs(&environment).current_dir(&backend).stdin(Stdio::null())
            .stdout(output.try_clone().unwrap()).stderr(Stdio::piped()).spawn().unwrap(),
        mux: mux.clone(), socket,
    };
    let stderr: std::os::fd::OwnedFd = processes.python.stderr.take().expect("stderr do uvicorn ausente").into();
    let stderr = tokio::fs::File::from_std(std::fs::File::from(stderr));
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    let upstream = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let Some(line) = lines.next_line().await.expect("falha lendo stderr do uvicorn") else {
                let status = processes.python.try_wait().unwrap();
                let text = std::fs::read_to_string(&log).unwrap();
                panic!("uvicorn fechou stderr antes do endereço: status={status:?}; log={text}");
            };
            writeln!(output, "{line}").expect("falha gravando log do uvicorn");
            if let Some(address) = line.split_once("Uvicorn running on http://")
                .and_then(|(_, tail)| tail.split_whitespace().next())
                .and_then(|text| text.parse::<std::net::SocketAddr>().ok()) {
                break address;
            }
        }
    }).await.expect("uvicorn real não ficou pronto; conferir python-uvicorn.log");
    let response = client().get(format!("http://{upstream}/api/hangar-terminals"))
        .bearer_auth(OWNER).send().await.expect("GET inicial do Python real falhou");
    assert_eq!(value(response, 200).await, json!({"terminals":[]}));
    let (f, state, task) = workspace_fixture::real_python_fixture(&home, &metadata_mux, &mux, upstream, environment).await;
    let runtime = f.runtime.clone();
    let addr = f.addr;
    let hits = f.hits.clone();
    let scene = Scene { _dir:dir, root, repo, wt, home, panes, f };
    let execution = Execution { scene, addr, tasks:vec![task], hits, runtime, state };
    let hold = execution.scene.home.join("hold");
    fifo(&hold);
    let command = "/bin/cat \"$HOME/hold\"".to_owned();
    assert!(command.len() < 80);

    let first = value(execution.run("main", "shortcut-shell",
        json!({"command":command,"label":"Primeiro","key":"owned:proof","ask":true})).await, 202).await;
    let global = value(execution.run("main", "shortcut-shell",
        json!({"command":command,"runs_in":"hangar","key":"global:proof","ask":false})).await, 202).await;
    let code = value(execution.run("sub", "run-code",
        json!({"command":command,"language":"bash","key":"run-code:proof"})).await, 202).await;
    let first_id = first["terminal"]["id"].as_str().unwrap();
    let global_id = global["terminal"]["id"].as_str().unwrap();
    let code_id = code["terminal"]["id"].as_str().unwrap();
    assert_ne!(first_id, global_id);
    assert_ne!(first_id, code_id);
    assert_ne!(global_id, code_id);

    let mut observed = serde_json::Map::new();
    for (path, id, label, key, ask) in [
        ("/api/sessions/fixture-main/shortcut-terminals", first_id, "Primeiro", "owned:proof", true),
        ("/api/hangar-terminals", global_id, command.as_str(), "global:proof", false),
        ("/api/sessions/fixture-sub/shortcut-terminals", code_id, "Terminal", "run-code:proof", true),
    ] {
        let body = value(client().get(format!("http://{upstream}{path}")).bearer_auth(OWNER).send().await.unwrap(), 200).await;
        let rows = body["terminals"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "a rota deve isolar o dono: {body}");
        let row = &rows[0];
        assert_eq!(row["id"], id);
        assert_eq!(row["label"], label);
        assert_eq!(row["key"], key);
        assert_eq!(row["ask"], ask);
        assert_eq!(row["alive"], true);
        assert!(row["exit_code"].is_null());
        assert!(row["created"].as_u64().is_some_and(|created| created > 0));
        if path == "/api/hangar-terminals" {
            assert_eq!(row["owner"], "");
            assert_eq!(row["origin"], "fixture-main");
        }
        observed.insert(path.into(), body);
    }
    let other = value(client().get(format!("http://{upstream}/api/sessions/fixture-wt/shortcut-terminals"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await;
    assert_eq!(other, json!({"terminals":[]}));

    let home = &execution.scene.home;
    for path in [
        format!("/api/sessions/fixture-sub/shortcut-terminals/{first_id}/close"),
        "/api/sessions/fixture-main/shortcut-terminals/zzzzzz/close".into(),
    ] {
        value(client().post(format!("http://{upstream}{path}")).bearer_auth(OWNER).send().await.unwrap(), 404).await;
    }
    for path in [
        format!("/api/sessions/fixture-sub/term?token={OWNER}&shortcut={first_id}"),
        format!("/api/sessions/fixture-main/term?token={OWNER}&shortcut=zzzzzz"),
    ] {
        let denied = tokio::time::timeout(Duration::from_secs(10),
            tokio_tungstenite::connect_async(format!("ws://{upstream}{path}"))).await.expect("recusa WS não concluiu");
        assert!(matches!(denied,
            Err(tokio_tungstenite::tungstenite::Error::Http(ref response))
                if response.status().as_u16() == 403 || response.status().as_u16() == 404),
            "o WS estrangeiro/inexistente não pode abrir um PTY: {denied:?}");
    }
    let kept = value(client().get(format!("http://{upstream}/api/sessions/fixture-main/shortcut-terminals"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await;
    assert_eq!(kept["terminals"].as_array().unwrap().iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(), [first_id]);

    let identity = |id: &str| {
        let output = Command::new(&mux).args(["list-sessions", "-F", "#{session_name}\t#{@cp_shortcut_id}\t#{pane_pid}"])
            .output().expect("consulta ao tmux privado falhou");
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        text.lines().find_map(|line| {
            let mut fields = line.split('\t');
            let target = fields.next()?;
            let found = fields.next()?;
            let pid = fields.next()?;
            (found == id).then(|| (target.to_owned(), pid.parse::<u32>().expect("PID real do terminal ausente")))
        }).expect("ID criado não está no tmux privado")
    };
    value(client().post(format!("http://{upstream}/api/hangar-terminals/zzzzzz/focus"))
        .bearer_auth(OWNER).send().await.unwrap(), 404).await;
    let unfocused = value(client().post(format!("http://{upstream}/api/hangar-terminals/{global_id}/focus"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await;
    assert_eq!(unfocused, json!({"focused":false}));
    let (_, global_pid) = identity(global_id);
    std::fs::write(home.join("focus-pid"), global_pid.to_string()).unwrap();
    let focused = value(client().post(format!("http://{upstream}/api/hangar-terminals/{global_id}/focus"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await;
    assert_eq!(focused, json!({"focused":true}));
    assert_eq!(std::fs::read_to_string(home.join("focused")).unwrap(), format!("pid:{global_pid}"));
    std::fs::remove_file(home.join("focus-pid")).unwrap();
    value(client().post(format!("http://{upstream}/api/hangar-terminals/{global_id}/close"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await;
    assert_eq!(value(client().get(format!("http://{upstream}/api/hangar-terminals"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await, json!({"terminals":[]}));
    value(client().post(format!("http://{upstream}/api/hangar-terminals/{global_id}/close"))
        .bearer_auth(OWNER).send().await.unwrap(), 404).await;

    fifo(&home.join("question-hold"));
    let prompt = "Destino SSH da VM (usuario@host ou alias do ~/.ssh/config) [Administrator@delphi-02]: ";
    assert!(prompt.len() > 80);
    let ask_script = r#"printf 'preparando\n'
read -r -p 'Pasta do PSS na VM [C:\PSS]: ' answer
printf '%s' "$answer" > "$HOME/answer-default.txt"
for index in 0 1 2 3 4 5 6; do
  read -r -p "Valor $index: " answer
  printf '[%s]' "$answer" > "$HOME/answer-$index.txt"
done
read -r -p 'Destino SSH da VM (usuario@host ou alias do ~/.ssh/config) [Administrator@delphi-02]: ' answer
printf '%s' "$answer" > "$HOME/answer-long.txt"
printf 'Porta: '
/bin/cat "$HOME/question-hold"
exit 3"#;
    let no_ask_script = r#"trap '' HUP
sleep 300 &
printf '%s' "$!" > "$HOME/answer-child.pid"
read -r -p 'Porta [3000]: ' answer
printf '%s' "$answer" > "$HOME/answer-hidden.txt"
wait"#;
    let asking = value(execution.run("main", "shortcut-shell", json!({
        "command":ask_script,"label":"Pergunta","runs_in":"hangar","key":"global:answer-proof","ask":true
    })).await, 202).await;
    let hidden = value(execution.run("main", "shortcut-shell", json!({
        "command":no_ask_script,"label":"Sem pergunta","key":"owned:no-ask","ask":false
    })).await, 202).await;
    let asking_id = asking["terminal"]["id"].as_str().unwrap();
    let hidden_id = hidden["terminal"]["id"].as_str().unwrap();
    let (asking_target, _) = identity(asking_id);
    let (hidden_target, _) = identity(hidden_id);
    let sized = Command::new(&mux).args(["resize-window", "-t", &format!("={asking_target}:"), "-x", "80", "-y", "24"])
        .output().unwrap();
    assert!(sized.status.success(), "{sized:?}");

    let wait_question = |expected: String| async move {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let body = value(client().get(format!("http://{upstream}/api/hangar-terminals"))
                    .bearer_auth(OWNER).send().await.unwrap(), 200).await;
                let row = body["terminals"].as_array().unwrap().iter()
                    .find(|row| row["id"] == asking_id).expect("terminal com pergunta sumiu");
                if row["question"]["text"] == expected { break row["question"].clone(); }
                tokio::task::yield_now().await;
            }
        }).await.expect("pergunta pública não chegou")
    };
    let question = wait_question("Pasta do PSS na VM".into()).await;
    assert_eq!(question["default"], r"C:\PSS");
    let screen = question["screen"].as_array().unwrap();
    assert!(screen.len() >= 2);
    assert_eq!(&screen[screen.len()-2..], &[json!("preparando"), json!(r"Pasta do PSS na VM [C:\PSS]:")]);
    for bad in ["a\nrm -rf x", "a\u{3}", "\u{1b}[A"] {
        let rejected = value(client().post(format!("http://{upstream}/api/hangar-terminals/{asking_id}/answer"))
            .bearer_auth(OWNER).json(&json!({"text":bad})).send().await.unwrap(), 400).await;
        assert_eq!(rejected["detail"]["code"], "erro_shortcut_resposta_invalida");
    }
    value(client().post(format!("http://{upstream}/api/hangar-terminals/{asking_id}/answer"))
        .bearer_auth(OWNER).json(&json!({"text":r"D:\X"})).send().await.unwrap(), 200).await;
    let mut question = wait_question("Valor 0".into()).await;
    assert_eq!(std::fs::read_to_string(home.join("answer-default.txt")).unwrap(), r"D:\X");
    let texts = ["-n x;y", "-", "; touch invadido", "a;", ";", r"a\;", "--help"];
    for (index, text) in texts.iter().enumerate() {
        assert_eq!(question["text"], format!("Valor {index}"));
        value(client().post(format!("http://{upstream}/api/hangar-terminals/{asking_id}/answer"))
            .bearer_auth(OWNER).json(&json!({"text":text})).send().await.unwrap(), 200).await;
        let next = if index + 1 == texts.len() {
            "Destino SSH da VM (usuario@host ou alias do ~/.ssh/config)".to_owned()
        } else { format!("Valor {}", index + 1) };
        question = wait_question(next).await;
        assert_eq!(std::fs::read_to_string(home.join(format!("answer-{index}.txt"))).unwrap(), format!("[{text}]"));
        assert!(!home.join("invadido").exists());
    }
    assert_eq!(question["text"], "Destino SSH da VM (usuario@host ou alias do ~/.ssh/config)");
    assert_eq!(question["default"], "Administrator@delphi-02");
    value(client().post(format!("http://{upstream}/api/hangar-terminals/{asking_id}/answer"))
        .bearer_auth(OWNER).json(&json!({"text":"vm@x"})).send().await.unwrap(), 200).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let body = value(client().get(format!("http://{upstream}/api/hangar-terminals"))
                .bearer_auth(OWNER).send().await.unwrap(), 200).await;
            let row = body["terminals"].as_array().unwrap().iter().find(|row| row["id"] == asking_id).unwrap();
            if row["question"].is_null() && std::fs::read_to_string(home.join("answer-long.txt")).ok().as_deref() == Some("vm@x") {
                let pane = Command::new(&mux).args(["capture-pane", "-p", "-t", &format!("={asking_target}:")]).output().unwrap();
                assert!(pane.status.success(), "{pane:?}");
                if String::from_utf8(pane.stdout).unwrap().trim_end().ends_with("Porta:") { break; }
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("programa que não lê o tty continuou com pergunta");

    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let pane = Command::new(&mux).args(["capture-pane", "-p", "-t", &format!("={hidden_target}:")]).output().unwrap();
            assert!(pane.status.success(), "{pane:?}");
            if String::from_utf8(pane.stdout).unwrap().contains("Porta [3000]:") { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("script ask=false não publicou o prompt real");
    let owner_rows = value(client().get(format!("http://{upstream}/api/sessions/fixture-main/shortcut-terminals"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await;
    let owner_rows = owner_rows["terminals"].as_array().unwrap();
    assert_eq!(owner_rows.iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(), [first_id, hidden_id]);
    let hidden_row = &owner_rows[1];
    assert_eq!(hidden_row["ask"], false);
    assert!(hidden_row["question"].is_null());
    let child: u32 = std::fs::read_to_string(home.join("answer-child.pid")).unwrap().parse().unwrap();
    assert!(Path::new(&format!("/proc/{child}")).exists());
    value(client().post(format!("http://{upstream}/api/sessions/fixture-main/shortcut-terminals/{hidden_id}/answer"))
        .bearer_auth(OWNER).json(&json!({"text":""})).send().await.unwrap(), 200).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while !home.join("answer-hidden.txt").is_file() { tokio::task::yield_now().await; }
    }).await.expect("resposta vazia não chegou ao script");
    assert_eq!(std::fs::read_to_string(home.join("answer-hidden.txt")).unwrap(), "");
    value(client().post(format!("http://{upstream}/api/sessions/fixture-main/shortcut-terminals/{hidden_id}/close"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while Path::new(&format!("/proc/{child}")).exists() { tokio::task::yield_now().await; }
    }).await.expect("close deixou filho que ignorava HUP");
    let owner_rows = value(client().get(format!("http://{upstream}/api/sessions/fixture-main/shortcut-terminals"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await;
    assert_eq!(owner_rows["terminals"].as_array().unwrap().iter().map(|row| row["id"].as_str().unwrap()).collect::<Vec<_>>(), [first_id]);

    std::fs::write(home.join("question-hold"), b"done\n").unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let body = value(client().get(format!("http://{upstream}/api/hangar-terminals"))
                .bearer_auth(OWNER).send().await.unwrap(), 200).await;
            let row = body["terminals"].as_array().unwrap().iter().find(|row| row["id"] == asking_id).unwrap();
            if row["alive"] == false && row["exit_code"] == 3 { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("saída real 3 não chegou à listagem Python");
    value(client().post(format!("http://{upstream}/api/hangar-terminals/{asking_id}/close"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await;
    assert_eq!(value(client().get(format!("http://{upstream}/api/hangar-terminals"))
        .bearer_auth(OWNER).send().await.unwrap(), 200).await, json!({"terminals":[]}));
    println!("{}", json!({"rust_created":[first,global,code],"python_get":observed,"other_owner":other}));
}
