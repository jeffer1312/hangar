//! A rota completa usa processos isolados que falam os protocolos dos dois agentes.
use super::*;
use hangar_conversation_transfer::{Store, TransferPhase};

fn transfer_programs(fx: &Fixture) -> AccountEnvironment {
    let bin = fx.home.join("bin");
    let codex_home = fx.home.join(".codex");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::write(
        codex_home.join("auth.json"),
        r#"{"OPENAI_API_KEY":"fixture-only"}"#,
    )
    .unwrap();
    let catalog = codex_home.join("catalog.json");
    std::fs::write(&catalog, json!({"models":[{"slug":"fixture-model","context_window":200000,"max_context_window":200000,
        "effective_context_window_percent":95,"input_modalities":["text","image"],"base_instructions":"Instruções de prova."}]}).to_string()).unwrap();
    let config = json!({"model_provider":"openai","model_catalog_json":catalog});
    let script = fx.home.join("codex-fixture.cjs");
    let js = r#"
const fs=require('node:fs'), readline=require('node:readline');
setTimeout(()=>process.exit(0),120000).unref();
const home=process.env.CODEX_HOME, cwd=process.cwd();
const rollout=home+'/imported.jsonl', threadId='22222222-2222-4222-8222-222222222222';
const config=CONFIG;
let effort='high', mode='default';
function thread(){return {id:threadId,path:rollout,cwd,model:'fixture-model',reasoningEffort:effort,turns:[],status:{type:'idle'}};}
function settings(){return {thread:thread(),cwd,model:'fixture-model',modelProvider:'openai',reasoningEffort:effort,
 approvalPolicy:'on-request',sandbox:{type:'workspaceWrite',writableRoots:[cwd],networkAccess:false,excludeTmpdirEnvVar:false,excludeSlashTmp:false},
 sandboxPolicy:{type:'workspaceWrite',writableRoots:[cwd],networkAccess:false,excludeTmpdirEnvVar:false,excludeSlashTmp:false},
 collaborationMode:{mode,settings:{model:'fixture-model',reasoning_effort:effort,developer_instructions:null}},instructionSources:[]};}
readline.createInterface({input:process.stdin}).on('line',line=>{
 let q;try{q=JSON.parse(line);}catch{return;}
 if(q.id===undefined)return;
 const p=q.params||{};let result={};
 switch(q.method){
 case 'initialize': result={userAgent:'fixture',platformFamily:'unix',platformOs:'linux'};break;
 case 'account/read': result={account:{type:'apiKey'},requiresOpenaiAuth:true};break;
 case 'account/rateLimits/read': result={rateLimits:{primary:{usedPercent:0,windowDurationMins:300,resetsAt:9999999999}}};break;
 case 'model/list': result={data:[{id:'fixture-model',model:'fixture-model',displayName:'Modelo de prova',isDefault:true,supportedReasoningEfforts:[{reasoningEffort:'high'}],defaultReasoningEffort:'high'}],nextCursor:null};break;
 case 'config/read': result={config};break;
 case 'thread/start': fs.writeFileSync(rollout,JSON.stringify({type:'session_meta',payload:{id:threadId,cwd}})+'\n');result=settings();break;
 case 'thread/resume': result=settings();break;
 case 'thread/read': result={thread:thread()};break;
 case 'thread/settings/update': if(p.effort)effort=p.effort;if(p.collaborationMode)mode=p.collaborationMode.mode;result=settings();break;
 case 'skills/list': result={data:[{cwd,skills:[],errors:[]}]};break;
 case 'mcpServerStatus/list': result={data:[],nextCursor:null};break;
 case 'thread/inject_items':
   if(fs.existsSync(home+'/fail-import')){process.stdout.write(JSON.stringify({id:q.id,error:{code:-32000,message:'falha de prova'}})+'\n');return;}
   for(const item of p.items)fs.appendFileSync(rollout,JSON.stringify({type:'response_item',payload:item})+'\n');break;
 }
 process.stdout.write(JSON.stringify({id:q.id,result})+'\n');
});
"#.replace("CONFIG", &config.to_string());
    std::fs::write(&script, js).unwrap();
    let node = std::process::Command::new("sh")
        .args(["-c", "command -v node"])
        .output()
        .unwrap();
    assert!(node.status.success(), "Node é requisito do projeto");
    let node = String::from_utf8(node.stdout).unwrap();
    crate::write_executable(
        &bin.join("codex"),
        format!(
            "#!/bin/bash\nif [ \"$1\" = --version ]; then echo 'codex-cli 0.159.3'; exit; fi\nexec -a codex '{}' '{}' \"$@\"\n",
            node.trim(),
            script.display()
        ),
    );
    let claude = fx.home.join("claude-fixture.sh");
    crate::write_executable(
        &claude,
        r#"#!/bin/bash
while IFS= read -r -t 120 line; do
 case "$line" in
 *'"type":"control_request"'*)
 rid=$(sed -E 's/^.*"request_id":"([^"]*)".*$/\1/' <<<"$line")
 printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"commands":[]}}}\n' "$rid";;
 esac
done
"#,
    );
    crate::write_executable(
        &bin.join("claude"),
        format!(
            "#!/bin/bash\nexec -a claude /bin/bash '{}' \"$@\"\n",
            claude.display()
        ),
    );
    AccountEnvironment::from_map(
        [
            ("HOME".to_owned(), fx.home.to_string_lossy().into_owned()),
            (
                "PATH".to_owned(),
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            ),
        ]
        .into(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_route_publishes_imported_thread_and_restores_source_on_import_failure() {
    crate::use_cano_bin();
    for fail in [false, true] {
        let fx = fixture();
        let key = "d4".repeat(16);
        let env = transfer_programs(&fx);
        fx.transcript();
        let source = fx
            .home
            .join(".claude/projects")
            .join(hangar_workspace::worktrees::sanitize_cwd(
                &fx.cwd.to_string_lossy(),
            ))
            .join(format!("{SID}.jsonl"));
        let original = format!(
            "{}\n",
            json!({"type":"user","uuid":"original-u","parentUuid":null,"message":{"role":"user","content":"Mensagem original integral."}})
        );
        std::fs::write(&source, &original).unwrap();
        fx.put_sidecar(json!({"name":"s1","provider":"claude","headless":true,"key":key,"cwd":fx.cwd,"session_id":SID,"permission_mode":"acceptEdits","cano":null}));
        if fail {
            std::fs::write(fx.home.join(".codex/fail-import"), "").unwrap();
        }
        let registry = crate::runtime_open::registry_with(|r| {
            r.with_codex_environment(env.clone())
                .with_processes(Arc::new(hangar_server::list::procs::SystemProcs::default()))
        })
        .await;
        registry.bind_watcher(Arc::new(
            Watcher::new(WatchConfig {
                home: fx.home.clone(),
                queue_dir: fx.queue.clone(),
                mux: Mux::with_program(&fx.tmux, Duration::from_secs(5)),
                mux_program: fx.tmux.clone().into_os_string(),
            })
            .with_scope_probe(Vec::new),
        ));
        let (python, upstream) = crate::fake::spawn_fake().await;
        let mut state =
            hangar_server::routes::AppState::new(crate::fake::config(upstream, "127.0.0.1"));
        state.accounts = hangar_server::accounts::AccountService::new(env);
        assert!(state.state.runtime.set(registry.clone()).is_ok());
        let server = crate::fake::spawn_state(state).await;
        let response=crate::fake::client().post(format!("http://{server}/api/sessions/s1/conta")).bearer_auth(crate::fake::OWNER)
            .json(&json!({"credential_id":format!("codex:{}",fx.home.join(".codex").display()),"source_life":format!("k:{key}"),"source_jsonl":source,"model":"fixture-model","effort":"high"})).send().await.unwrap();
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap();
        let store = Store::new(fx.home.join(".hangar/conversation-transfers"));
        let records = std::fs::read_dir(store.base())
            .unwrap_or_else(|error| {
                panic!("a transferência não iniciou: status={status} body={body}; {error}")
            })
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .map(|e| {
                store
                    .load(e.path().file_stem().unwrap().to_str().unwrap())
                    .unwrap()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 1, "status={status} body={body}");
        let record = &records[0];
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
        assert_eq!(python.hits_to("/api/sessions/s1/conta"), 0);
        if fail {
            assert_eq!(status, 409, "{body}");
            assert_eq!(
                body["detail"]["code"],
                "session_transfer_native_import_failed"
            );
            assert_eq!(record.phase, TransferPhase::RolledBack);
            let meta: Value =
                serde_json::from_slice(&std::fs::read(fx.sidecar_path("s1")).unwrap()).unwrap();
            assert_eq!(meta["session_id"], SID);
            assert!(meta["cano"]["pid"].as_u64().is_some());
        } else {
            assert_eq!(status, 200, "{body}");
            assert_eq!(body["provider"], "codex");
            assert_eq!(record.phase, TransferPhase::Complete);
            let output = std::fs::read_to_string(fx.home.join(".codex/imported.jsonl")).unwrap();
            assert!(output.contains("Mensagem original integral."));
            assert!(!fx.sidecar_path("s1").exists());
            let meta: Value = serde_json::from_slice(
                &std::fs::read(fx.home.join(".hangar/codex-sessions/s1.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(meta["transfer_id"], record.id);
        }
        let _ = crate::fake::client()
            .delete(format!("http://{server}/api/sessions/s1"))
            .bearer_auth(crate::fake::OWNER)
            .send()
            .await;
    }
}
