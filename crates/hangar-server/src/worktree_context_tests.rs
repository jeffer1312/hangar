use super::*;
use serde_json::json;
use std::{collections::BTreeMap, fs, time::{Duration, UNIX_EPOCH}};

const NORMAL: &str = ".claude-a/projects/-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1/0cc60e6c-eafc-4915-9936-f0c7b81191d9.jsonl";
const RESERVE: &str = ".claude-a/projects/-home-jefferson/f2afaa93-d2aa-45db-9d68-b430c22deab2.jsonl";
const INTERNAL: &str = ".claude-b/projects/-home-jefferson-Projetos-hangar/a5a5a5a5-0000-4000-8000-000000000001.jsonl";
const SID: &str = "0cc60e6c-eafc-4915-9936-f0c7b81191d9.jsonl";
const NOW: f64 = 1_800_000_000.0;
const PI: &str = ".pi/agent/sessions/--home-jefferson-Projetos-hangar-.worktrees-contas-modelos--/2026-08-11T09-37-57-248Z_d337d6b8-0b3c-4bbe-8034-939d7c48f246.jsonl";
const OMP: &str = ".omp/agent/sessions/-tmp-omp-teste/2026-09-03T21-35-10-766Z_01a06932-79ae-77e9-b95a-286b6e2e9288.jsonl";
const CODEX: &str = ".codex/sessions/2026/08/30/rollout-2026-08-30T20-47-12-01a05511-e980-71b3-93a9-befda5704b20.jsonl";

fn state(home: &Path, extra: &[(&str,String)]) -> AppState {
    use crate::{accounts::AccountService,config::Config,list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux}};
    fs::create_dir_all(home.join("roots")).unwrap();
    fs::create_dir_all(home.join(".claude/projects")).unwrap();
    let mut env=BTreeMap::from([
        ("HOME".into(),home.to_string_lossy().into_owned()),("USERPROFILE".into(),home.to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR".into(),home.join(".claude").to_string_lossy().into_owned()),
        ("CP_SCAN_ROOTS".into(),home.join("roots").to_string_lossy().into_owned()),
    ]);
    env.extend(extra.iter().map(|(key,value)|(key.to_string(),value.clone())));
    let upstream="127.0.0.1:9".parse().unwrap();
    let mut st=AppState::new(Config {listen:"127.0.0.1:0".parse().unwrap(),upstream,internal_secret:"context-test".into(),auth_token:"context-owner".into(),log_path:None,trusted:crate::auth::TrustedHosts::parse("127.0.0.1")});
    st.origins_home=home.into();
    st.accounts=AccountService::new(AccountEnvironment::from_map(env));
    st.search=crate::search::Index::server(st.accounts.clone());
    let dirs=parse_dirs(&json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string());
    st.list=Arc::new(ListBridge::new(ListEnv {mux:Mux::default(),capture_program:"unused".into(),procs:Arc::new(crate::list::procs::SystemProcs::default()),dirs},FactsClient::new(upstream,"context-test".into())));
    st
}

fn transcript(target: &Path, input: &str, cwd: Option<&Path>, mtime: f64) {
    let fixture=Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/home").join(input);
    let mut text=fs::read_to_string(fixture).unwrap();
    if let Some(cwd)=cwd {
        let quoted=serde_json::to_string(&cwd.to_string_lossy()).unwrap();
        text=text.replace("/home/jefferson/Projetos/hangar-computer-control-9c0b634f-t1",quoted.trim_matches('"'));
    }
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target,text).unwrap();
    fs::File::options().write(true).open(target).unwrap().set_modified(UNIX_EPOCH+Duration::from_secs_f64(mtime)).unwrap();
}

#[tokio::test]
async fn runtime_roots_override_environment_and_drop_missing_paths() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path());
    let st=state(&home,&[]);
    let chosen=home.join("chosen"); fs::create_dir(&chosen).unwrap();
    fs::write(home.join(".claude/runtime-config.json"),json!({"scan_roots":format!("{},{}",chosen.display(),home.join("missing").display())}).to_string()).unwrap();
    assert_eq!(roots(&st).unwrap(),vec![chosen.to_string_lossy().into_owned()]);
    fs::write(home.join(".claude/runtime-config.json"),json!({"scan_roots":""}).to_string()).unwrap();
    assert_eq!(roots(&st).unwrap(),vec![home.join("roots").to_string_lossy().into_owned()]);
}

#[tokio::test]
async fn own_projects_override_is_included_after_catalog_bases() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path());
    let own=home.join("custom-projects");
    let st=state(&home,&[("CP_PROJECTS_DIR",own.to_string_lossy().into_owned())]);
    assert_eq!(project_bases(&st).unwrap(),vec![home.join(".claude/projects"),own]);
}

#[cfg(unix)]
#[tokio::test]
async fn unmanaged_account_alias_is_read_without_changing_public_catalog_policy() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path());
    let target=home.join("external-account"); fs::create_dir_all(target.join("projects")).unwrap();
    fs::write(target.join(".credentials.json"),"{}").unwrap();
    std::os::unix::fs::symlink(&target,home.join(".claude-alias")).unwrap();
    let st=state(&home,&[]);
    assert_eq!(project_bases(&st).unwrap(),vec![home.join(".claude/projects"),target.join("projects")]);
    assert!(!st.accounts.claude_catalog().unwrap().as_array().unwrap().iter().any(|row|row["path"]==target.to_string_lossy().as_ref()));
}

#[tokio::test]
async fn thirty_day_boundary_filters_candidates_not_activities_or_sessions() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path());
    let st=state(&home,&[]); let at=home.join("roots/at"); let old=home.join("roots/old");
    transcript(&home.join(".claude/projects/-at").join(SID),NORMAL,Some(&at),NOW-30.0*86400.0);
    transcript(&home.join(".claude/projects/-old").join(SID),NORMAL,Some(&old),NOW-30.0*86400.0-1.0);
    let row:hangar_api::session::SessionRow=serde_json::from_value(json!({"name":"outside","cwd":"/outside","git_cwd":"/outside-worktree","worktree_path":"/outside-worktree","jsonl":"/outside.jsonl"})).unwrap();
    let ctx=collect(&st,&[row],NOW).unwrap();
    assert_eq!(ctx.cwds,vec![at.to_string_lossy().into_owned()]);
    assert_eq!(ctx.activities.unwrap().len(),2);
    assert_eq!(ctx.sessions[0].cwd.as_deref(),Some("/outside"));
    assert_eq!(ctx.sessions[0].git_cwd.as_deref(),Some("/outside-worktree"));
    assert_eq!(ctx.sessions[0].jsonl.as_deref(),Some("/outside.jsonl"));
}

#[tokio::test]
async fn duplicate_ids_and_internal_conversations_remain_worktree_activity() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path()); let st=state(&home,&[]);
    transcript(&home.join(".claude/projects/-one").join(SID),NORMAL,None,NOW-1000.0);
    transcript(&home.join(".claude/projects/-two").join(SID),NORMAL,None,NOW-1100.0);
    transcript(&home.join(".claude/projects/-internal/a5a5a5a5-0000-4000-8000-000000000001.jsonl"),INTERNAL,None,NOW-1200.0);
    let ctx=collect(&st,&[],NOW).unwrap(); let activities=ctx.activities.unwrap();
    assert_eq!(activities.len(),3);
    assert_eq!(activities.iter().filter(|a|a.cwd=="/home/jefferson/Projetos/hangar-computer-control-9c0b634f-t1").count(),2);
    assert!(activities.iter().any(|a|a.cwd=="/home/jefferson/Projetos/hangar-mobile-deliveries-cad3e6fe-t32"));
}

#[tokio::test]
async fn prewarmed_file_without_user_becomes_activity_only_when_settled() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path()); let st=state(&home,&[]);
    let path=home.join(".claude/projects/-reserve/f2afaa93-d2aa-45db-9d68-b430c22deab2.jsonl");
    transcript(&path,RESERVE,None,NOW-60.0);
    assert!(collect(&st,&[],NOW).unwrap().activities.unwrap().is_empty());
    fs::File::options().write(true).open(path).unwrap().set_modified(UNIX_EPOCH+Duration::from_secs_f64(NOW-901.0)).unwrap();
    let activities=collect(&st,&[],NOW).unwrap().activities.unwrap();
    assert_eq!(activities.len(),1); assert_eq!(activities[0].cwd,"/home/jefferson");
}

#[tokio::test]
async fn server_omp_profile_supplies_worktree_activity() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path());
    let st=state(&home,&[("OMP_PROFILE","work".into())]);
    let file=home.join(".omp/profiles/work/agent/sessions/-profile").join(SID);
    transcript(&file,OMP,None,NOW-1000.0);
    let activities=collect(&st,&[],NOW).unwrap().activities.unwrap();
    assert_eq!(activities.len(),1);
    assert_eq!(activities[0].cwd,"/tmp/claude-1000/-home-jefferson-Projetos-hangar/ac1a7b0f-d0a7-4e66-8f05-f53ea5d4b4da/scratchpad/omp-teste");
}

#[tokio::test]
async fn invalid_utf8_in_an_unrelated_header_field_keeps_the_pi_cwd() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path()); let st=state(&home,&[]);
    let file=home.join(".pi/agent/sessions/-bytes").join(SID);
    transcript(&file,PI,None,NOW-1000.0);
    let mut bytes=fs::read(&file).unwrap();
    let end=bytes.iter().position(|byte|*byte==b'\n').unwrap();
    assert_eq!(bytes[end-1],b'}');
    bytes.splice(end-1..end-1,b",\"extra\":\"\xff\"".iter().copied());
    fs::write(&file,bytes).unwrap();
    let activities=collect(&st,&[],NOW).unwrap().activities.unwrap();
    assert_eq!(activities.len(),1);
    assert_eq!(activities[0].cwd,"/home/jefferson/Projetos/hangar/.worktrees/contas-modelos");
}

#[tokio::test]
async fn invalid_utf8_in_an_unrelated_header_field_keeps_the_codex_cwd() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path()); let st=state(&home,&[]);
    let file=home.join(CODEX);
    transcript(&file,CODEX,None,NOW-1000.0);
    let mut bytes=fs::read(&file).unwrap();
    let end=bytes.iter().position(|byte|*byte==b'\n').unwrap();
    assert_eq!(bytes[end-1],b'}');
    bytes.splice(end-1..end-1,b",\"extra\":\"\xff\"".iter().copied());
    fs::write(&file,bytes).unwrap();
    let activities=collect(&st,&[],NOW).unwrap().activities.unwrap();
    assert_eq!(activities.len(),1);
    assert_eq!(activities[0].cwd,"/home/jefferson/Projetos/hangar");
}

#[tokio::test]
async fn orq_rows_leave_and_return_with_the_same_name_without_stale_cwd() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path()); let st=state(&home,&[]);
    let run=home.join(".hangar/orq/context-run"); fs::create_dir_all(&run).unwrap();
    let repo=home.join("roots/first"); let next=home.join("roots/second-place");
    let now=NOW;
    let stamp=chrono::DateTime::<chrono::Utc>::from_timestamp(NOW as i64,0).unwrap().to_rfc3339();
    fs::write(run.join("orq.json"),json!({"auto":true,"repo":repo,"arbiter":"a"}).to_string()).unwrap();
    let start=json!({"tipo":"execucao_inicio","gid":"context-group","ts":stamp}).to_string();
    fs::write(run.join("eventos.jsonl"),format!("{start}\n")).unwrap();
    fs::write(run.join("timeline-context-run.jsonl"),"").unwrap();
    fs::write(run.join("vigia.json"),json!({"pid":std::process::id(),"interval_s":3600,"ts":stamp,"arbiter":"a"}).to_string()).unwrap();
    let first=collect(&st,&[],now).unwrap();
    assert_eq!(first.sessions.len(),1);
    assert_eq!(first.sessions[0].name,"context-group-orq");
    assert_eq!(first.sessions[0].cwd.as_deref(),Some(repo.to_str().unwrap()));
    fs::write(run.join("eventos.jsonl"),format!("{start}\n{}\n",json!({"tipo":"execucao_fim","ts":stamp}))).unwrap();
    assert!(collect(&st,&[],now).unwrap().sessions.is_empty());
    fs::write(run.join("orq.json"),json!({"auto":true,"repo":next,"arbiter":"a"}).to_string()).unwrap();
    fs::write(run.join("eventos.jsonl"),format!("{start}\n")).unwrap();
    let reopened=collect(&st,&[],now).unwrap();
    assert_eq!(reopened.sessions[0].name,"context-group-orq");
    assert_eq!(reopened.sessions[0].cwd.as_deref(),Some(next.to_str().unwrap()));
    assert_eq!(reopened.sessions[0].git_cwd.as_deref(),Some(next.to_str().unwrap()));
}

#[cfg(unix)]
#[tokio::test]
async fn linked_transcript_activity_uses_target_modified_time() {
    let dir=tempfile::tempdir().unwrap(); let home=hangar_workspace::real(dir.path()); let st=state(&home,&[]);
    let source=home.join("source.jsonl"); transcript(&source,NORMAL,None,NOW-1000.0);
    let project=home.join(".claude/projects/-linked"); fs::create_dir(&project).unwrap();
    std::os::unix::fs::symlink(source,project.join(SID)).unwrap();
    let activities=collect(&st,&[],NOW).unwrap().activities.unwrap();
    assert_eq!(activities.len(),1);
    assert_eq!(activities[0].mtime,NOW-1000.0);
}
