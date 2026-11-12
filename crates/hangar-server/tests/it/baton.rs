//! B01: corpos integrais do produtor original 08db6bc92 e asserts de test_bastao.py.
use crate::{fake::{OWNER, client}, workspace_fixture};
use serde_json::{Value, json};
use std::{path::{Path, PathBuf}, sync::atomic::Ordering};

const CLAUDE_ID: &str = "2751096f-84dc-4986-b0a8-a40d641ee43d";
const LIVE_ID: &str = "00000000-0000-4000-8000-000000000101";
const CODEX_ID: &str = "019f0000-0000-7000-8000-000000000001";

fn git(root: &Path, args: &[&str]) -> std::process::Output {
    let out = std::process::Command::new("git").arg("-C").arg(root)
        .args(["-c", if cfg!(windows) {"core.hooksPath=NUL"} else {"core.hooksPath=/dev/null"}, "-c", "commit.gpgSign=false"])
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) {"NUL"} else {"/dev/null"})
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .env("GIT_AUTHOR_NAME", "Fixture").env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture").env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z").env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out
}

fn relocate_json_path(raw: &str, original: &str, path: &Path) -> String {
    // A substituição ocorre dentro de strings JSON, preservando os demais bytes da fixture.
    let encoded = serde_json::to_string(path.to_str().unwrap()).unwrap();
    raw.replace(original, &encoded[1..encoded.len() - 1])
}

struct Origin {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    path: PathBuf,
    fixture: workspace_fixture::Fixture,
}
impl Origin {
    async fn new(codex: bool, live: bool) -> Self { Self::build(codex,live,false).await }
    async fn router_only(codex: bool, live: bool) -> Self { Self::build(codex,live,true).await }
    async fn build(codex: bool, live: bool, router_only: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let isolated = std::env::var_os("HANGAR_BATON_CHILD_ROOT").map(PathBuf::from);
        // Forma longa: no Windows a pasta temporária pode vir curta (`RUNNER~1`), e o catálogo de contas,
        // que o `config_dir` do pedido precisa igualar letra a letra, sai resolvido.
        let resolved =hangar_workspace::real(isolated.as_deref().unwrap_or(dir.path()));
        let base = resolved.as_path();
        let (root, home) = (base.join("repo"), base.join("home"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(home.join(".claude/projects/-mnt-case-work-repo")).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        let path = if codex {
            let account = home.join(".codex-work");
            std::fs::create_dir_all(account.join("sessions/2026/09/02")).unwrap();
            std::fs::write(account.join(".hangar-codex-conta"), r#"{"version":1,"id":"work"}"#).unwrap();
            let path = account.join("sessions").join("2026").join("09").join("02")
                .join(format!("rollout-2026-09-02T14-00-00-{CODEX_ID}.jsonl"));
            std::fs::write(&path, relocate_json_path(include_str!("../fixtures/baton/codex.jsonl"), "/tmp/proj", &root)).unwrap();
            git(&root, &["commit", "-q", "--allow-empty", "-m", "um"]);
            path
        } else {
            let plan = root.join("docs/superpowers/plans/2026-09-02-grupo-pareamento-fim-sessao.md");
            std::fs::create_dir_all(plan.parent().unwrap()).unwrap();
            let mut text = "# Grupo, pareamento e fim de sessão — Implementation Plan\n\n".to_owned();
            for task in 1..=10 {
                text += &format!("### Task {task}: Tarefa {task}\n\n");
                for step in 1..=6 { text += &format!("- [x] **Step {step}: passo {task}.{step}**\n"); }
                text += "\n";
            }
            std::fs::write(&plan, &text).unwrap();
            git(&root, &["add", "--", "docs/superpowers/plans/2026-09-02-grupo-pareamento-fim-sessao.md"]);
            git(&root, &["commit", "-q", "-m", "Inicial"]);
            std::fs::write(&plan, text + "\n").unwrap();
            std::fs::write(root.join("outro.txt"), "outro\n").unwrap();
            let path = home.join(".claude").join("projects").join("-mnt-case-work-repo")
                .join(format!("{}.jsonl", if live {LIVE_ID} else {CLAUDE_ID}));
            let relocated = relocate_json_path(include_str!("../fixtures/baton/claude.jsonl"), "/repo", &root);
            let mut rows = String::new();
            for raw in relocated.lines() {
                let mut value: Value = serde_json::from_str(raw).unwrap();
                if let Some(blocks) = value["message"]["content"].as_array_mut() {
                    for block in blocks {
                        if let Some(command) = block["input"]["command"].as_str() {
                            block["input"]["command"] = json!(command.replace(root.to_str().unwrap(), "/mnt/case/work/repo"));
                        }
                    }
                }
                rows += &format!("{value}\n");
            }
            std::fs::write(&path, rows).unwrap();
            path
        };
        let mux = home.join(if cfg!(windows) { "baton-mux.cmd" } else { "baton-mux" });
        let pane = if live { format!("fixture\t1\t\t{}\t%1\t\tclaude\t1793966722\t0\t0\n", root.display()) } else { String::new() };
        #[cfg(unix)]
        crate::write_executable(&mux, format!("#!/bin/sh\n[ \"$1\" = list-panes ] || exit 0\nprintf '%s' '{}'\n", pane.replace('\'', "'\\''")));
        #[cfg(windows)]
        // `echo` sem nada imprime "ECHO is off."; sem pane, o script não imprime.
        std::fs::write(&mux, if pane.is_empty() { "@echo off\r\nexit /b 0\r\n".to_owned() } else {
            format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\necho {}\r\n", pane.trim_end().replace('%', "%%"))
        }).unwrap();
        let opening_mux = isolated.as_ref().map(|root|root.join("bin/tmux"));
        let fixture = if let Some(opening_mux)=opening_mux.as_deref() {
            workspace_fixture::fixture_with_account_env(&root,false,&home,opening_mux,hangar_server::accounts::environment::AccountEnvironment::capture(),router_only).await
        } else if router_only {
            workspace_fixture::policy_fixture(&root,&home,&mux).await
        } else {workspace_fixture::fixture_with_mux(&root,false,Some(&home),Some(&mux)).await};
        if live { fixture.list.seed("fixture", path.to_str().unwrap()); }
        Self { _dir:dir, root, home, path, fixture }
    }
    fn expected(&self, codex: bool, name: &str) -> String {
        let original = if codex { include_str!("../fixtures/baton/codex-expected.txt") } else { include_str!("../fixtures/baton/claude-expected.txt") };
        let root = self.root.to_str().unwrap();
        original.replace(if codex {"`morta`"} else {"`fixture-claude`"}, &format!("`{name}`"))
            .replace(if codex {"/mnt/case/home/.codex-work/sessions/2026/09/02/rollout-2026-09-02T14-00-00-019f0000-0000-7000-8000-000000000001.jsonl"} else {"/mnt/case/home/.claude/projects/-mnt-case-work-repo/00000000-0000-4000-8000-000000000101.jsonl"}, self.path.to_str().unwrap())
            .replace("`/mnt/case/work/repo`", &format!("`{}`", self.root.display()))
            // Estes caminhos vêm da transcrição, onde o resto depois da raiz está escrito com `/`.
            .replace("/mnt/case/work/repo/docs/superpowers/plans/2026-09-02-grupo-pareamento-fim-sessao.md", &format!("{root}/docs/superpowers/plans/2026-09-02-grupo-pareamento-fim-sessao.md"))
            .replace("/mnt/case/work/repo/docs/superpowers/specs/2026-09-03-bastao-v2-design.md", &format!("{root}/docs/superpowers/specs/2026-09-03-bastao-v2-design.md"))
    }
    fn url(&self, tail: &str) -> String { format!("http://{}/api/sessions/{tail}", self.fixture.addr) }
    fn archived(&self, codex: bool) -> String {
        let mut url = reqwest::Url::parse(&self.url("morta/bastao")).unwrap();
        url.query_pairs_mut().append_pair("project", "-mnt-case-work-repo")
            .append_pair("session_id", if codex { CODEX_ID } else { CLAUDE_ID })
            .append_pair("provider", if codex {"codex"} else {"claude"});
        if !codex { url.query_pairs_mut().append_pair("config_dir", self.home.join(".claude").to_str().unwrap()); }
        url.to_string()
    }
    fn no_python(&self) {
        assert_eq!(self.fixture.hits.load(Ordering::Relaxed), 0, "B01 não repassa preparo ou recusa");
        assert_eq!(self.fixture.context_hits.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn b01_get_live_origin_without_a_transcript_is_404() {
    #[cfg(unix)]
    if post_child("baton::b01_get_live_origin_without_a_transcript_is_404") {return;}
    let h=Origin::new(false,false).await;
    std::fs::remove_file(&h.path).unwrap();
    #[cfg(unix)]
    std::fs::write(h.home.parent().unwrap().join("session-name"),"fixture\n").unwrap();
    #[cfg(windows)]
    std::fs::write(h.home.join("baton-mux.cmd"),format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\necho fixture\t1\t\t{}\t%%1\t\tclaude\t1793966722\t0\t0\r\n",h.root.display())).unwrap();
    let rows=h.fixture.list.discover(Some(f64::MAX)).await.unwrap();
    let origin=rows.iter().find(|row|row.name=="fixture").expect("origem viva descoberta pelo mux privado");
    assert!(origin.jsonl.is_none(),"precondição: linha realmente sem transcript, não IO parcial: {}",serde_json::to_string(origin).unwrap());
    let response=client().get(h.url("fixture/bastao")).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let body=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,404,"{body}");assert!(!h.home.join(".claude/.hangar-bastao").exists());
    h.no_python();
}

#[tokio::test]
async fn b01_post_live_origin_without_a_transcript_is_404_without_a_successor() {
    #[cfg(unix)]
    if post_child("baton::b01_post_live_origin_without_a_transcript_is_404_without_a_successor") {return;}
    let h=Origin::new(false,false).await;
    std::fs::remove_file(&h.path).unwrap();
    #[cfg(unix)]
    std::fs::write(h.home.parent().unwrap().join("session-name"),"fixture\n").unwrap();
    #[cfg(windows)]
    std::fs::write(h.home.join("baton-mux.cmd"),format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\necho fixture\t1\t\t{}\t%%1\t\tclaude\t1793966722\t0\t0\r\n",h.root.display())).unwrap();
    let rows=h.fixture.list.discover(Some(f64::MAX)).await.unwrap();
    let origin=rows.iter().find(|row|row.name=="fixture").expect("origem viva descoberta pelo mux privado");
    assert!(origin.jsonl.is_none(),"precondição: linha realmente sem transcript, não IO parcial: {}",serde_json::to_string(origin).unwrap());
    let response=client().post(h.url("fixture/bastao")).bearer_auth(OWNER)
        .json(&json!({"name":"cc2","provider":"claude"})).send().await.unwrap();
    let status=response.status();let body=response.text().await.unwrap();
    let after=h.fixture.list.discover(Some(f64::MAX)).await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,404,"{body}");assert!(after.iter().all(|row|row.name!="cc2"));
    assert!(!h.home.parent().unwrap().join("new-session-argv").exists());
    assert!(!h.home.join(".hangar/claude-headless/cc2.json").exists());
    assert!(!h.home.join(".claude/.hangar-bastao").exists() && !h.home.join(".claude/.hangar-queue/cc2.jsonl").exists());
    h.no_python();
}

#[tokio::test]
async fn b01_get_existing_archive_in_an_unknown_config_is_rejected() {
    let h=Origin::new(false,false).await;
    let config=h.home.join("conta");let sid="45ce9c2d-48d4-491e-8ff3-672bb10a69b6";
    let transcript=config.join(format!("projects/-repo/{sid}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();std::fs::copy(&h.path,&transcript).unwrap();
    let mut url=reqwest::Url::parse(&h.url("morta/bastao")).unwrap();
    url.query_pairs_mut().append_pair("project","-repo").append_pair("session_id",sid).append_pair("config_dir",config.to_str().unwrap());
    let response=client().get(url).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let body=response.json::<Value>().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,400,"{body}");assert_eq!(body["detail"]["code"],"erro_config_dir_invalido");
    assert!(transcript.is_file());assert!(!h.home.join(".claude/.hangar-bastao").exists());
    h.no_python();
}

#[tokio::test]
async fn b01_get_without_live_or_archived_origin_is_404() {
    let h=Origin::new(false,false).await;
    let response=client().get(format!("{}?project=-x&session_id=45ce9c2d-48d4-491e-8ff3-672bb10a69b6",h.url("morta/bastao")))
        .bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let body=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,404,"{body}");assert!(!h.home.join(".claude/.hangar-bastao").exists());
    h.no_python();
}

#[tokio::test]
async fn b01_get_known_archive_with_an_unknown_provider_is_rejected() {
    let h=Origin::new(false,false).await;
    let mut url=reqwest::Url::parse(&h.url("morta/bastao")).unwrap();
    url.query_pairs_mut().append_pair("project","-mnt-case-work-repo").append_pair("session_id",CLAUDE_ID)
        .append_pair("config_dir",h.home.join(".claude").to_str().unwrap()).append_pair("provider","nao-existe");
    let response=client().get(url).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let body=response.json::<Value>().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,400,"{body}");assert_eq!(body["detail"]["code"],"erro_provider_invalido");
    assert!(h.path.is_file());assert!(!h.home.join(".claude/.hangar-bastao").exists());
    h.no_python();
}

#[tokio::test]
async fn b01_group_contract_points_to_local_or_remote_owner_without_moving_links() {
    #[cfg(unix)]
    if post_child("baton::b01_group_contract_points_to_local_or_remote_owner_without_moving_links") {return;}
    let h=Origin::router_only(false,false).await;
    let directory=h.home.join(".claude/.hangar-pair");std::fs::create_dir_all(&directory).unwrap();
    let reader=hangar_server::groups::store::PairDir::new(directory.clone(),h.home.join(".hangar/pair-arquivo"));
    let mut url=reqwest::Url::parse(&h.archived(false)).unwrap();url.set_path("/api/sessions/origem/bastao");
    let c=client();let mut observed=Vec::new();
    for (data,remote) in [
        (json!({"peers":["parceira"],"task":"ABC-1234","gid":"g1"}),false),
        (json!({"peers":["casa::parceira"],"task":"","gid":"g1","fed":{"owner":"casa","local":false,"version":1}}),true),
    ] {
        std::fs::write(directory.join("origem.json"),serde_json::to_vec(&data).unwrap()).unwrap();
        let sidecar=reader.sidecar("origem").unwrap().expect("formato de grupo original aceito pelo PairDir");
        assert_eq!(sidecar.gid,"g1");assert_eq!(sidecar.fed.as_ref().is_some_and(|fed|!fed.local),remote);
        let response=c.get(url.clone()).bearer_auth(OWNER).send().await.unwrap();
        observed.push((remote,response.status(),response.text().await.unwrap()));
    }
    h.fixture.runtime.shutdown().await.unwrap();
    for (remote,status,md) in observed {
        assert_eq!(status,200,"remote={remote}: {md}");
        let group=md.split("## Grupo e par").nth(1).expect("seção Grupo e par").split("\n## ").next().unwrap();
        assert!(group.contains("parceira") && group.contains("Contrato do grupo"),"{group}");
        assert!(group.contains("não move estes vínculos") && group.contains("vale o contrato"),"{group}");
        if remote {
            assert!(group.contains("`casa::parceira`") && group.contains("na máquina `casa`") && group.contains("hangar-send --contrato"),"{group}");
            assert!(!group.contains("grupo-g1.md"),"{group}");
        } else {
            assert!(group.contains("`parceira`") && group.contains("ABC-1234") && group.contains("grupo-g1.md"),"{group}");
        }
    }
    h.no_python();
}

#[tokio::test]
async fn b01_first_get_live_claude_full_markdown_and_headers() {
    let h = Origin::new(false, true).await;
    let r = client().get(h.url("fixture/bastao")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/markdown; charset=utf-8");
    let expected = h.expected(false, "fixture");
    assert_eq!(r.headers()["content-length"], expected.len().to_string());
    assert_eq!(r.text().await.unwrap(), expected);
    h.no_python();
}

#[tokio::test]
async fn b01_first_get_archived_claude_full_markdown() {
    let h = Origin::new(false, false).await;
    let r = client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/markdown; charset=utf-8");
    assert_eq!(r.text().await.unwrap(), h.expected(false, "morta"));
    h.no_python();
}

#[tokio::test]
async fn b01_first_get_archived_codex_full_markdown_and_mismatch() {
    let h = Origin::new(true, false).await;
    let c = client();
    let r = c.get(h.archived(true)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/markdown; charset=utf-8");
    assert_eq!(r.text().await.unwrap(), h.expected(true, "morta"));
    let r = c.get(format!("{}&origem_codex_account=default", h.archived(true))).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 409);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({"detail":{"code":"codex_account_archive_mismatch",
        "params":{"account_id":"default","origin_account":"work"},"msg":"a conversa é de outra conta Codex"}}));
    h.no_python();
}

#[tokio::test]
async fn b01_preview_and_received_keep_gzip_identity_and_final_content_length() {
    use std::io::Read;
    let h = Origin::new(false, false).await;
    let c = reqwest::Client::builder().no_gzip().build().unwrap();
    let directory = h.home.join(".claude/.hangar-bastao");
    std::fs::create_dir(&directory).unwrap();
    let expected = h.expected(false, "morta");
    std::fs::write(directory.join("saved.md"), &expected).unwrap();
    for url in [h.archived(false), h.url("saved/bastao/dossie")] {
        for encoding in [None, Some("identity"), Some("gzip")] {
            let mut req = c.get(&url).bearer_auth(OWNER);
            if let Some(encoding) = encoding {req = req.header("accept-encoding", encoding);}
            let response = req.send().await.unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["vary"], "Accept-Encoding");
            let length: usize = response.headers()["content-length"].to_str().unwrap().parse().unwrap();
            let compressed = response.headers().get("content-encoding").is_some_and(|v|v == "gzip");
            assert_eq!(compressed, encoding == Some("gzip"));
            let bytes = response.bytes().await.unwrap();
            assert_eq!(length, bytes.len());
            let text = if compressed {let mut text = String::new(); flate2::read::GzDecoder::new(bytes.as_ref()).read_to_string(&mut text).unwrap(); text} else {String::from_utf8(bytes.to_vec()).unwrap()};
            assert_eq!(text, expected);
        }
    }
    h.no_python();
}

#[tokio::test]
async fn b01_live_codex_uses_its_own_account_and_same_full_dossier() {
    let h = Origin::new(true, false).await;
    let sidecar = h.home.join(".hangar/codex-sessions/fixture.json");
    std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
    std::fs::write(&sidecar, json!({"name":"fixture","provider":"codex","key":"baton-live","cwd":h.root,
        "codex_home":h.home.join(".codex-work"),"thread_id":CODEX_ID,"rollout_path":h.path,"headless":true}).to_string()).unwrap();
    let r = client().get(h.url("fixture/bastao")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), h.expected(true, "fixture"));
    h.no_python();
}

#[tokio::test]
async fn b01_completed_plan_and_running_loop_suppress_unanswered_requests() {
    let h=Origin::router_only(false,false).await;
    let store=hangar_server::loops::store::Store::new(&h.home.join(".claude"));
    let data=json!({"goal":"passar a suíte","check_cmd":"uv run pytest -q","status":"running","max_iters":5,"iter":1});
    store.set("morta",data.as_object().unwrap()).unwrap();
    let loop_before=store.get("morta");
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();
    let loop_after=store.get("morta");
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let remaining=md.split("## O que falta").nth(1).expect("seção O que falta").split("\n## ").next().unwrap();
    assert!(remaining.contains("concluído") && remaining.contains("60/60"),"{remaining}");
    assert!(remaining.contains("Loop `running`"),"{remaining}; loop_before={loop_before:?}; loop_after={loop_after:?}");assert!(!remaining.contains("AINDA SEM resposta"),"{remaining}");
    h.no_python();
}

#[tokio::test]
async fn b01_remaining_steps_show_exact_next_step_per_task() {
    let h=Origin::router_only(false,false).await;
    let plan=h.root.join("docs/superpowers/plans/2026-09-02-grupo-pareamento-fim-sessao.md");
    let raw=std::fs::read_to_string(&plan).unwrap();
    let raw=raw.replace("- [x] **Step 3: passo 9.3**","- [ ] **Step 3: passo 9.3**")
        .replace("- [x] **Step 1: passo 10.1**","- [ ] **Step 1: passo 10.1**");
    std::fs::write(plan,raw).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let remaining=md.split("## O que falta").nth(1).expect("seção O que falta").split("\n## ").next().unwrap();
    assert!(remaining.contains("58/60"),"{remaining}");
    assert!(remaining.contains("Task 9: Tarefa 9: 5/6 — próximo: Step 3: passo 9.3"),"{remaining}");
    assert!(remaining.contains("Task 10: Tarefa 10: 5/6 — próximo: Step 1: passo 10.1"),"{remaining}");
    h.no_python();
}

#[tokio::test]
async fn b01_running_loop_disappears_when_done_on_the_same_url() {
    let h=Origin::router_only(false,false).await;
    std::fs::write(&h.path,format!("{}\n",json!({"type":"assistant","uuid":"a0","message":{"role":"assistant","content":[{"type":"text","text":"PONG"}]}}))).unwrap();
    let store=hangar_server::loops::store::Store::new(&h.home.join(".claude"));
    let mut data=json!({"goal":"passar a suíte","check_cmd":"uv run pytest -q","status":"running","max_iters":5,"iter":2});
    store.set("morta",data.as_object().unwrap()).unwrap();
    let c=client();let url=h.archived(false);
    let running=c.get(&url).bearer_auth(OWNER).send().await.unwrap();
    let running_status=running.status();let running_md=running.text().await.unwrap();
    data["status"]=json!("done");store.set("morta",data.as_object().unwrap()).unwrap();
    let done=c.get(&url).bearer_auth(OWNER).send().await.unwrap();
    let done_status=done.status();let done_md=done.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(running_status,200,"{running_md}");assert_eq!(done_status,200,"{done_md}");
    let running=running_md.split("## O que falta").nth(1).expect("seção O que falta").split("\n## ").next().unwrap();
    let done=done_md.split("## O que falta").nth(1).expect("seção O que falta").split("\n## ").next().unwrap();
    assert!(running.contains("Loop `running`") && running.contains("2/5") && running.contains("passar a suíte"),"{running}");
    assert!(!done.contains("Loop"),"{done}");
    h.no_python();
}

#[tokio::test]
async fn b01_no_plan_or_loop_lists_only_unanswered_user_requests() {
    let h=Origin::router_only(false,false).await;
    let mut rows=String::new();
    for (i,(agent,user)) in [(Some("Feito o passo 1."),"agora roda os testes"),(Some("Rodei, passou."),"então commita"),
        (None,"e depois faz o commit do README"),(None,"ah, e avisa o par")].into_iter().enumerate() {
        if let Some(agent)=agent {
            rows+=&format!("{}\n",json!({"type":"assistant","uuid":format!("a{i}"),"message":{"role":"assistant","content":[{"type":"text","text":agent}]}}));
        }
        rows+=&format!("{}\n",json!({"type":"user","uuid":format!("u{i}"),"message":{"role":"user","content":user}}));
    }
    std::fs::write(&h.path,rows).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let remaining=md.split("## O que falta").nth(1).expect("seção O que falta").split("\n## ").next().unwrap();
    assert!(remaining.contains("AINDA SEM resposta"),"{remaining}");
    for pending in ["então commita","commit do README","avisa o par"] {assert!(remaining.contains(pending),"{remaining}");}
    assert!(!remaining.contains("agora roda os testes"),"pedido já respondido: {remaining}");
    h.no_python();
}

#[tokio::test]
async fn b01_work_section_lists_real_commits_since_the_transcript_without_ahead_count() {
    #[cfg(unix)]
    if post_child("baton::b01_work_section_lists_real_commits_since_the_transcript_without_ahead_count") {return;}
    let h=Origin::router_only(false,false).await;let remote=h.home.parent().unwrap().join("remote.git");
    git(&h.root,&["init","--bare",remote.to_str().unwrap()]);
    git(&h.root,&["remote","add","origin",remote.to_str().unwrap()]);git(&h.root,&["push","-u","origin","main"]);
    git(&h.root,&["commit","--allow-empty","-m","refactor(pair): textos"]);
    let first=String::from_utf8(git(&h.root,&["rev-parse","--short","HEAD"]).stdout).unwrap();
    git(&h.root,&["commit","--allow-empty","-m","fix(pair): varredura"]);
    let second=String::from_utf8(git(&h.root,&["rev-parse","--short","HEAD"]).stdout).unwrap();
    let divergence=String::from_utf8(git(&h.root,&["rev-list","--left-right","--count","HEAD...@{upstream}"]).stdout).unwrap();
    assert_eq!(divergence.split_whitespace().map(|n|n.parse::<u64>().unwrap()).collect::<Vec<_>>(),[2,0],"controle Gitindependente: {divergence}");
    std::fs::write(&h.path,format!("{}\n",json!({"type":"user","uuid":"u0","timestamp":"2025-12-31T00:00:00Z","cwd":h.root,
        "message":{"role":"user","content":"agora roda os testes"}}))).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let work=md.split("## Onde está o trabalho").nth(1).expect("seção Onde está o trabalho").split("\n## ").next().unwrap();
    assert!(work.contains("Commits desde"),"{work}");
    assert!(work.contains(&format!("`{}` refactor(pair): textos",first.trim())),"{work}");
    assert!(work.contains(&format!("`{}` fix(pair): varredura",second.trim())),"{work}");
    assert!(!work.contains("à frente"),"{work}");h.no_python();
}

#[tokio::test]
async fn b01_work_section_keeps_three_behind_when_real_commits_are_listed() {
    #[cfg(unix)]
    if post_child("baton::b01_work_section_keeps_three_behind_when_real_commits_are_listed") {return;}
    let h=Origin::router_only(false,false).await;let remote=h.home.parent().unwrap().join("remote.git");
    let peer=h.home.parent().unwrap().join("peer");
    git(&h.root,&["init","--bare",remote.to_str().unwrap()]);
    git(&h.root,&["remote","add","origin",remote.to_str().unwrap()]);git(&h.root,&["push","-u","origin","main"]);
    git(&h.root,&["clone","--branch","main",remote.to_str().unwrap(),peer.to_str().unwrap()]);
    for subject in ["remote: um","remote: dois","remote: três"] {git(&peer,&["commit","--allow-empty","-m",subject]);}
    git(&peer,&["push","origin","main"]);
    git(&h.root,&["commit","--allow-empty","-m","local: primeiro"]);
    git(&h.root,&["commit","--allow-empty","-m","local: segundo"]);git(&h.root,&["fetch","origin"]);
    let last=String::from_utf8(git(&h.root,&["rev-parse","--short","HEAD"]).stdout).unwrap();
    let divergence=String::from_utf8(git(&h.root,&["rev-list","--left-right","--count","HEAD...@{upstream}"]).stdout).unwrap();
    assert_eq!(divergence.split_whitespace().map(|n|n.parse::<u64>().unwrap()).collect::<Vec<_>>(),[2,3],"controle Gitindependente: {divergence}");
    let independent=String::from_utf8(git(&h.root,&["log","--since=2025-12-31T00:00:00Z","--format=%h"]).stdout).unwrap();
    assert!(independent.lines().any(|hash|hash==last.trim()),"commit local precisa estar após a origem: {independent}");
    std::fs::write(&h.path,format!("{}\n",json!({"type":"user","uuid":"u0","timestamp":"2025-12-31T00:00:00Z","cwd":h.root,
        "message":{"role":"user","content":"agora roda os testes"}}))).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let work=md.split("## Onde está o trabalho").nth(1).expect("seção Onde está o trabalho").split("\n## ").next().unwrap();
    assert!(work.contains("Commits desde") && work.contains(&format!("`{}` local: segundo",last.trim())),"{work}");
    assert!(work.contains("3 atrás") && !work.contains("à frente"),"{work}");h.no_python();
}

#[tokio::test]
async fn b01_work_section_keeps_ahead_count_when_no_commits_follow_the_transcript_start() {
    #[cfg(unix)]
    if post_child("baton::b01_work_section_keeps_ahead_count_when_no_commits_follow_the_transcript_start") {return;}
    let h=Origin::router_only(false,false).await;let remote=h.home.parent().unwrap().join("remote.git");
    git(&h.root,&["init","--bare",remote.to_str().unwrap()]);
    git(&h.root,&["remote","add","origin",remote.to_str().unwrap()]);git(&h.root,&["push","-u","origin","main"]);
    git(&h.root,&["commit","--allow-empty","-m","local: primeiro"]);git(&h.root,&["commit","--allow-empty","-m","local: segundo"]);
    let divergence=String::from_utf8(git(&h.root,&["rev-list","--left-right","--count","HEAD...@{upstream}"]).stdout).unwrap();
    assert_eq!(divergence.split_whitespace().map(|n|n.parse::<u64>().unwrap()).collect::<Vec<_>>(),[2,0],"controle Gitindependente: {divergence}");
    let independent=String::from_utf8(git(&h.root,&["log","--since=2026-01-02T00:00:00Z","--format=%h"]).stdout).unwrap();
    assert!(independent.trim().is_empty(),"commits dafixtureJan1 não podem cair após origemJan2: {independent}");
    std::fs::write(&h.path,format!("{}\n",json!({"type":"user","uuid":"u0","timestamp":"2026-01-02T00:00:00Z","cwd":h.root,
        "message":{"role":"user","content":"agora roda os testes"}}))).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let work=md.split("## Onde está o trabalho").nth(1).expect("seção Onde está o trabalho").split("\n## ").next().unwrap();
    assert!(work.contains("2 commit(s) à frente"),"{work}");
    assert!(!work.contains("Commits desde") && !work.contains("agora roda os testes"),"{work}");h.no_python();
}

#[tokio::test]
async fn b01_empty_grep_in_compound_commands_is_not_a_tool_failure() {
    for (command,result,failed,listed) in [
        ("git status --short; echo x; grep -c foo bar","Exit code 1\n89 matches",false,true),
        ("rg -n foo src/","Exit code 1",false,false),
        ("grep -c foo bar","Exit code 2\ngrep: bar: No such file or directory",true,false),
        ("uv run pytest -q","Exit code 1\n1 failed",true,true),
        ("git status --short | grep -c '^ M'","Exit code 1",false,true),
        (r#"grep -n "a" x.py | head -80; echo ====; grep -rn "b" y.py z.py | head -30"#,"Exit code 1\n89 matches in 1 files:",false,false),
        ("uv run pytest -q && grep -c PASS output.log","Exit code 1\n1 failed",true,true),
        ("grep -c foo bar; uv run pytest -q","Exit code 1\n1 failed",true,false),
    ] {
        let h=Origin::router_only(false,false).await;
        let rows=format!("{}\n{}\n",json!({"type":"assistant","uuid":"a0","cwd":h.root,"message":{"role":"assistant","content":[
            {"type":"tool_use","id":"bash-case","name":"Bash","input":{"command":command}}]}}),
            json!({"type":"user","uuid":"u0","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bash-case","content":result,"is_error":true}]}}));
        std::fs::write(&h.path,rows).unwrap();
        let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
        let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
        assert_eq!(status,200,"{md}");
        let files=md.split("## Arquivos e comandos").nth(1).expect("seção Arquivos e comandos").split("\n## ").next().unwrap();
        assert_eq!(files.contains(command),listed,"listagem original do comando: {command}: {files}");
        assert_eq!(files.contains("Ferramentas que FALHARAM"),failed,"{command} / {result}: {files}");
        if failed {assert!(files.split("Ferramentas que FALHARAM").nth(1).unwrap().contains("`Bash`:"),"{files}");}
        h.no_python();
    }
}

#[tokio::test]
async fn b01_system_errors_remain_failures_but_hook_tags_are_ignored() {
    let command="uv run pytest -q";
    for (result,failed) in [
        ("[Errno 2] No such file or directory",true),("[Errno 13] Permission denied",true),("[WinError 5] Access is denied",true),
        ("[Fact-Forcing Gate] bloqueado",false),("[pass-adversarial] …",false),("[hangar-state] linha conectada",false),
        ("algo com lembrete automático no meio",false),
    ] {
        let h=Origin::router_only(false,false).await;
        let rows=format!("{}\n{}\n",json!({"type":"assistant","uuid":"a0","cwd":h.root,"message":{"role":"assistant","content":[
            {"type":"tool_use","id":"bash-case","name":"Bash","input":{"command":command}}]}}),
            json!({"type":"user","uuid":"u0","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bash-case","content":result,"is_error":true}]}}));
        std::fs::write(&h.path,rows).unwrap();
        let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
        let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
        assert_eq!(status,200,"{md}");
        let files=md.split("## Arquivos e comandos").nth(1).expect("seção Arquivos e comandos").split("\n## ").next().unwrap();
        assert!(files.contains(command),"comando continua listado: {files}");
        assert_eq!(files.contains("Ferramentas que FALHARAM"),failed,"{result}: {files}");
        if failed {let failures=files.split("Ferramentas que FALHARAM").nth(1).unwrap();assert!(failures.contains("`Bash`:") && failures.contains(result),"{files}");}
        h.no_python();
    }
}

#[tokio::test]
async fn b01_severity_tags_in_brackets_remain_tool_failures() {
    let command="uv run pytest -q";
    for result in ["[ERROR] Could not connect…","[FAILED] npm run build…","[error] Cannot find module…","[FATAL] disk full"] {
        let h=Origin::router_only(false,false).await;
        let rows=format!("{}\n{}\n",json!({"type":"assistant","uuid":"a0","cwd":h.root,"message":{"role":"assistant","content":[
            {"type":"tool_use","id":"bash-case","name":"Bash","input":{"command":command}}]}}),
            json!({"type":"user","uuid":"u0","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bash-case","content":result,"is_error":true}]}}));
        std::fs::write(&h.path,rows).unwrap();
        let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
        let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
        assert_eq!(status,200,"{md}");
        let files=md.split("## Arquivos e comandos").nth(1).expect("seção Arquivos e comandos").split("\n## ").next().unwrap();
        assert!(files.contains(command),"comando continua listado: {files}");
        let failures=files.split("Ferramentas que FALHARAM").nth(1).expect("subbloco de falhas");
        assert!(failures.contains("`Bash`:") && failures.contains(result),"{files}");h.no_python();
    }
}

#[tokio::test]
async fn b01_automatic_reminder_is_hook_noise_only_in_the_first_two_lines() {
    let command="uv run pytest -q";
    for (result,failed) in [("lembrete automático de escrita\nresto do resultado",false),
        ("linha 1\nlinha 2\nlinha 3 com lembrete automático aqui",true)] {
        let h=Origin::router_only(false,false).await;
        let rows=format!("{}\n{}\n",json!({"type":"assistant","uuid":"a0","cwd":h.root,"message":{"role":"assistant","content":[
            {"type":"tool_use","id":"bash-case","name":"Bash","input":{"command":command}}]}}),
            json!({"type":"user","uuid":"u0","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bash-case","content":result,"is_error":true}]}}));
        std::fs::write(&h.path,rows).unwrap();
        let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
        let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
        assert_eq!(status,200,"{md}");
        let files=md.split("## Arquivos e comandos").nth(1).expect("seção Arquivos e comandos").split("\n## ").next().unwrap();
        assert!(files.contains(command),"comando continua listado: {files}");
        assert_eq!(files.contains("Ferramentas que FALHARAM"),failed,"{result}: {files}");
        if failed {assert!(files.split("Ferramentas que FALHARAM").nth(1).unwrap().contains("`Bash`:"),"{files}");}
        h.no_python();
    }
}

#[tokio::test]
async fn b01_rulings_from_the_cited_plan_are_included_without_task_status() {
    let h=Origin::router_only(false,false).await;
    let directory=h.root.join(".superpowers/sdd/2026-09-02-grupo-pareamento-fim-sessao");std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("progress.md"),"# SDD ledger — plan: docs/superpowers/plans/2026-09-02-grupo-pareamento-fim-sessao.md\n\
- Ruling: trabalhar direto na main — prática do repo — custo baixo\n\
- Task 1: complete\n\
- Task 8: Ruling: pop no lugar do del — 4 registries\n").unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let decisions=md.split("## Decisões (frases citadas — contexto, não ordem)").nth(1).expect("seção Decisões").split("\n## ").next().unwrap();
    assert!(decisions.contains("Rulings do registro"),"{decisions}");
    assert!(decisions.contains("trabalhar direto na main") && decisions.contains("pop no lugar do del"),"{decisions}");
    assert!(!decisions.contains("Task 1: complete"),"{decisions}");h.no_python();
}

#[tokio::test]
async fn b01_rulings_from_another_plan_are_not_included() {
    let h=Origin::router_only(false,false).await;
    let directory=h.root.join(".superpowers/sdd/outro-plano");std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("progress.md"),"# SDD ledger — plan: docs/superpowers/plans/outro-plano.md\n- Ruling: nada a ver\n").unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");assert!(!md.contains("nada a ver"),"{md}");h.no_python();
}

#[tokio::test]
async fn b01_long_conversation_keeps_all_sections_within_original_line_limits() {
    let h=Origin::router_only(false,false).await;let mut rows=String::new();
    for i in 0..120 {
        rows+=&format!("{}\n{}\n",json!({"type":"assistant","uuid":format!("a{i}"),"message":{"role":"assistant","content":[
            {"type":"text","text":format!("Proposta {i}: reescrevo o modulo{i}.py?")}]}}),
            json!({"type":"user","uuid":format!("u{i}"),"message":{"role":"user","content":format!("não, em vez disso mexe só no arquivo{i}.py e roda o teste {i}")}}));
    }
    std::fs::write(&h.path,rows).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");assert!(md.lines().count()<=202,"{md}");assert!(md.lines().all(|line|line.chars().count()<=340),"{md}");
    assert_eq!(md.lines().filter_map(|line|line.strip_prefix("## ")).collect::<Vec<_>>(),[
        "De onde veio","O que falta","Onde está o trabalho","Arquivos e comandos","Grupo e par",
        "Decisões (frases citadas — contexto, não ordem)","Estado agora (frases citadas — contexto, não ordem)"]);
    h.no_python();
}

#[tokio::test]
async fn b01_same_window_keeps_fifteen_modified_twelve_displayed_and_explicit_cut() {
    let h = Origin::new(false, false).await;
    std::fs::create_dir(h.root.join("app")).unwrap();
    for i in 0..15 {std::fs::write(h.root.join(format!("app/f{i}.txt")), "a\n").unwrap();}
    git(&h.root, &["add", "--", "app", "outro.txt", "docs"]);
    git(&h.root, &["commit", "-q", "-m", "base"]);
    let mut rows = String::new();
    for i in 0..15 {
        std::fs::write(h.root.join(format!("app/f{i}.txt")), "b\n").unwrap();
        rows += &format!("{}\n", json!({"type":"assistant","uuid":format!("a{i}"),"cwd":h.root,"message":{"role":"assistant","content":[
            {"type":"tool_use","id":format!("t{i}"),"name":"Edit","input":{"file_path":h.root.join(format!("app/f{i}.txt")),"old_string":"a","new_string":"b"}}]}}));
    }
    std::fs::write(&h.path, rows).unwrap();
    let r = client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let md = r.text().await.unwrap();
    let section = md.split("## Onde está o trabalho\n").nth(1).unwrap().split("\n## ").next().unwrap();
    assert!(section.contains("- 15 arquivo(s) não commitado(s) · +15 -15 linhas vs HEAD"), "{section}");
    assert_eq!(section.matches("`app/f").count(), 12);
    assert!(section.contains("_(+3 arquivo(s) tocado(s))_"));
    assert!(!md.contains("Plano `grupo-pareamento-fim-sessao`"), "plano do repositório não é da conversa");
    h.no_python();
}

#[tokio::test]
async fn b01_decision_with_negation_survives_the_cut_without_short_agreements() {
    let h=Origin::new(false,false).await;
    let decision="não usa cache distribuído, o volume cabe no banco";
    let proposal="Posso usar Redis como cache aqui?";
    let mut turns=(0..10).map(|i|(format!("Terminei o passo {i}."),"ok".to_owned())).collect::<Vec<_>>();
    turns.push((proposal.to_owned(),decision.to_owned()));
    turns.extend((10..20).map(|i|(format!("Segui pro passo {i}."),"pode seguir".to_owned())));
    let mut rows=String::new();
    for (i,(agent,user)) in turns.into_iter().enumerate() {
        rows+=&format!("{}\n{}\n",json!({"type":"assistant","uuid":format!("a{i}"),"message":{"role":"assistant","content":[{"type":"text","text":agent}]}}),
            json!({"type":"user","uuid":format!("u{i}"),"message":{"role":"user","content":user}}));
    }
    std::fs::write(&h.path,rows).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let decisions=md.split("## Decisões").nth(1).expect("seção Decisões").split("## ").next().unwrap();
    assert!(decisions.contains(decision),"{decisions}");assert!(decisions.contains(proposal),"{decisions}");
    assert!(!decisions.contains("pode seguir"),"{decisions}");assert!(!decisions.contains("Terminei o passo"),"{decisions}");
    h.no_python();
}

#[tokio::test]
async fn b01_short_agreement_to_an_agent_question_is_not_discarded() {
    let h=Origin::new(false,false).await;
    let proposal="Reescrevo o parser inteiro?";
    let rows=format!("{}\n{}\n",json!({"type":"assistant","uuid":"a0","message":{"role":"assistant","content":[{"type":"text","text":proposal}]}}),
        json!({"type":"user","uuid":"u0","message":{"role":"user","content":"ok"}}));
    std::fs::write(&h.path,rows).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");assert!(md.contains(proposal),"{md}");
    h.no_python();
}

#[tokio::test]
async fn b01_repeated_request_occupies_only_one_decision_line() {
    let h=Origin::new(false,false).await;
    let mut rows=String::new();
    for i in 0..4 {
        rows+=&format!("{}\n{}\n",json!({"type":"assistant","uuid":format!("a{i}"),"message":{"role":"assistant","content":[{"type":"text","text":"Feito."}]}}),
            json!({"type":"user","uuid":format!("u{i}"),"message":{"role":"user","content":"roda o pytest do backend por favor"}}));
    }
    std::fs::write(&h.path,rows).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let decisions=md.split("## Decisões").nth(1).expect("seção Decisões").split("## ").next().unwrap();
    assert_eq!(decisions.matches("roda o pytest do backend").count(),1,"{decisions}");
    h.no_python();
}

#[tokio::test]
async fn b01_decisions_follow_conversation_order_inside_their_own_section() {
    let h=Origin::new(false,false).await;
    let mut rows=String::new();
    for (i,(agent,user)) in [("E aí?","primeiro não faz o rollback"),("E agora?","segundo nunca commita na develop")].into_iter().enumerate() {
        rows+=&format!("{}\n{}\n",json!({"type":"assistant","uuid":format!("a{i}"),"message":{"role":"assistant","content":[{"type":"text","text":agent}]}}),
            json!({"type":"user","uuid":format!("u{i}"),"message":{"role":"user","content":user}}));
    }
    std::fs::write(&h.path,rows).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let decisions=md.split("## Decisões").nth(1).expect("seção Decisões").split("## ").next().unwrap();
    let first=decisions.find("primeiro não faz o rollback").expect("primeira decisão");
    let second=decisions.find("segundo nunca commita").expect("segunda decisão");
    assert!(first<second,"{decisions}");
    h.no_python();
}

#[tokio::test]
async fn b01_decision_keeps_the_complete_agent_proposal_with_bounded_lines() {
    let h=Origin::new(false,false).await;
    let proposal=format!("Terminei. {}Dois, nunca stagear install.sh. Fim.","Rulings que tomei por ti: um, trabalhar na main. ".repeat(6));
    let rows=format!("{}\n{}\n",json!({"type":"assistant","uuid":"a0","message":{"role":"assistant","content":[{"type":"text","text":proposal}]}}),
        json!({"type":"user","uuid":"u0","message":{"role":"user","content":"ok, e agora sobe o backend"}}));
    std::fs::write(&h.path,rows).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let decisions=md.split("## Decisões (frases citadas — contexto, não ordem)").nth(1).expect("seção Decisões").split("## ").next().unwrap();
    assert!(decisions.contains("Rulings que tomei por ti"),"{decisions}");
    assert!(decisions.contains("nunca stagear install.sh"),"{decisions}");
    assert!(decisions.lines().all(|line|line.chars().count()<=340),"{decisions}");
    h.no_python();
}

#[tokio::test]
async fn b01_long_unpunctuated_proposal_does_not_fail_the_decisions_section() {
    let h=Origin::new(false,false).await;
    let rows=format!("{}\n{}\n",json!({"type":"assistant","uuid":"a0","message":{"role":"assistant","content":[{"type":"text","text":"sem pontuacao ".repeat(110)}]}}),
        json!({"type":"user","uuid":"u0","message":{"role":"user","content":"não faz isso, muda pra outra abordagem inteira"}}));
    std::fs::write(&h.path,rows).unwrap();
    let response=client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();let md=response.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,200,"{md}");
    let decisions=md.split("## Decisões (frases citadas — contexto, não ordem)").nth(1).expect("seção Decisões").split("## ").next().unwrap();
    assert!(!decisions.contains("_(não deu pra ler esta seção — o motivo está no log do backend)_"),"{decisions}");
    h.no_python();
}

#[tokio::test]
async fn b01_partial_git_failure_keeps_other_sections_and_reports_the_missing_list() {
    let h = Origin::new(false, false).await;
    std::fs::write(h.root.join(".git/index"), "índice inválido").unwrap();
    let r = client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let md = r.text().await.unwrap();
    assert!(md.contains("- Branch: `main`"));
    assert!(md.contains("- _(a lista de arquivos não veio: "));
    assert!(md.contains("## Estado agora (frases citadas — contexto, não ordem)"));
    assert!(md.contains("Quando voltar vc vai executar"));
    h.no_python();
}

#[tokio::test]
async fn b01_transfer_uses_verified_selected_plan_and_only_active_rows_after_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let h = crate::workspace_routes::TransferredFiles::new(dir.path()).await;
    let plan = h.old.join("docs/superpowers/plans/2026-09-02-selected.md");
    let sibling = h.old.join("docs/superpowers/plans/2026-09-02-sibling.md");
    std::fs::create_dir_all(plan.parent().unwrap()).unwrap();
    std::fs::write(&plan, "### Task 1: escolhida\n- [ ] **Step 1: próximo selecionado**\n").unwrap();
    std::fs::write(&sibling, "### Task 1: alheia\n- [ ] **Step 1: nunca herdar**\n").unwrap();
    let mut rows: Vec<Value> = std::fs::read_to_string(&h.snapshot).unwrap().lines().map(|l|serde_json::from_str(l).unwrap()).collect();
    rows[1]["message"]["content"] = json!([{"type":"tool_use","id":"selected-tool","name":"Read","input":{"file_path":plan}}]);
    let fork = rows.iter_mut().find(|r|r["uuid"] == "fork").unwrap();
    fork["message"]["content"] = json!([{"type":"tool_use","id":"sibling-tool","name":"Read","input":{"file_path":sibling}}]);
    let raw = rows.iter().map(|r|format!("{r}\n")).collect::<String>();
    std::fs::write(&h.snapshot, &raw).unwrap();
    let mut record = h.record.clone();
    record["source"]["digest"] = json!(ring::digest::digest(&ring::digest::SHA256,raw.as_bytes()).as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>());
    std::fs::write(&h.record_path, record.to_string()).unwrap();
    let url = format!("http://{}/api/sessions/fixture/bastao", h.f.addr);
    let r = client().get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let md = r.text().await.unwrap();
    assert!(md.contains("Plano `selected` (citado pela sessão): 0/1 steps"), "{md}");
    assert!(md.contains("próximo selecionado"));
    assert!(!md.contains("sibling") && !md.contains("nunca herdar") && !md.contains("uncited.md"));
    assert!(md.contains("active.md"));
    h.no_python();
}

#[tokio::test]
async fn b01_transfer_partial_failure_keeps_the_original_literal_dossier() {
    let dir=tempfile::tempdir().unwrap();
    let h=crate::workspace_routes::TransferredFiles::new(dir.path()).await;
    git(&h.born,&["init","-q","-b","main"]);
    std::fs::write(h.born.join("tracked.txt"),"before\n").unwrap();
    git(&h.born,&["add","--","tracked.txt"]);git(&h.born,&["commit","-q","-m","base"]);
    std::fs::write(h.born.join("tracked.txt"),"after\n").unwrap();
    for name in ["one.txt","two.txt","three.txt"] {std::fs::write(h.born.join(name),"untracked\n").unwrap();}
    let endpoint=format!("http://{}/api/sessions/morta/bastao?project=-&session_id=019f99de-9572-7221-89c0-80c62f883d44&provider=codex&origem_codex_account=default",h.f.addr);
    let control=client().get(&endpoint).bearer_auth(OWNER).send().await.unwrap();
    let control_status=control.status();let control_body=control.text().await.unwrap();
    assert_eq!(control_status,200);assert!(control_body.contains("active.md") && control_body.contains("início"));
    let expected=include_str!("../fixtures/baton/transfer-corrupt-expected.txt")
        .replace("`transferred`","`morta`")
        .replace("`/mnt/case/work/repo`",&format!("`{}`",h.born.display()))
        .replace("/mnt/case/home/.codex-work/sessions/rollout-fixture-019f99de-9572-7221-89c0-80c62f883d44.jsonl",h.rollout.to_str().unwrap())
        .replace("- Conta: `work`","- Conta: `default`");
    let source=std::fs::read(&h.snapshot).unwrap();let rollout=std::fs::read(&h.rollout).unwrap();
    let mut results=Vec::new();
    for variant in ["source","prefix"] {
        if variant=="source" {std::fs::write(&h.snapshot,b"snapshot adulterado\n").unwrap();}
        else {
            let mut changed=rollout.clone();let offset=h.record["boundary"]["min_offset"].as_u64().unwrap() as usize;
            changed[offset-2]=b' ';std::fs::write(&h.rollout,changed).unwrap();
        }
        let response=client().get(&endpoint).bearer_auth(OWNER).send().await.unwrap();
        let status=response.status();let headers=response.headers().clone();let body=response.text().await.unwrap();
        results.push((variant,status,headers,body));
        std::fs::write(&h.snapshot,&source).unwrap();std::fs::write(&h.rollout,&rollout).unwrap();
    }
    h.f.runtime.shutdown().await.unwrap();
    assert_eq!(results.iter().map(|(_,status,_,_)|status.as_u16()).collect::<Vec<_>>(),[200,200],"snapshot e prefixo independentes: B01 parcial não é a recusa409 de citações");
    for (variant,status,headers,body) in results {
        assert_eq!(status,200,"B01 é parcial, não a recusa409 de citações: {variant}: {body}");
        assert_eq!(headers["content-type"],"text/markdown; charset=utf-8");
        assert_eq!(headers["content-length"],expected.len().to_string());
        assert_eq!(body,expected,"{variant}: corpo original integral, sem normalizar/truncar");
    }
    h.no_python();
}

#[tokio::test]
async fn b01_next_unreadable_pair_is_logged_without_hiding_the_measured_sections() {
    let h = Origin::new(false,false).await;
    let pair = h.home.join(".claude/.hangar-pair");
    std::fs::create_dir_all(&pair).unwrap();std::fs::write(pair.join("morta.json"),"{inválido").unwrap();
    let r = client().get(h.archived(false)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(),200);
    assert_eq!(r.text().await.unwrap(),h.expected(false,"morta"));
    h.no_python();
}

#[tokio::test]
async fn b01_transfer_partial_archived_keeps_the_checked_rollout_header_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let h = crate::workspace_routes::TransferredFiles::new(dir.path()).await;
    let rollout=std::fs::read_to_string(&h.rollout).unwrap();
    let header:Value=serde_json::from_str(rollout.lines().next().unwrap()).unwrap();
    assert_eq!(header["type"],"session_meta");assert_eq!(header["payload"]["cwd"],json!(h.born));
    let mut metadata:Value=serde_json::from_slice(&std::fs::read(&h.sidecar).unwrap()).unwrap();
    metadata["cwd"]=json!(h.old);hangar_server::runtime::codex_sidecar::put(h.sidecar.parent().unwrap(),"fixture",metadata.as_object().unwrap()).unwrap();
    let r = client().get(format!("http://{}/api/sessions/morta/bastao?project=-&session_id=019f99de-9572-7221-89c0-80c62f883d44&provider=codex", h.f.addr)).bearer_auth(OWNER).send().await.unwrap();
    let status=r.status();let md=r.text().await.unwrap();h.f.runtime.shutdown().await.unwrap();
    assert_eq!(status,200);
    assert!(md.contains(&format!("- Diretório de trabalho: `{}`",h.born.display())),"{md}");
    assert_eq!(std::fs::read_to_string(&h.rollout).unwrap(),rollout,"não mudar prefixo/rollout para fabricar equivalência");
    assert!(md.contains("active.md") && !md.contains("uncited.md"));
    h.no_python();
}

#[tokio::test]
async fn b01_next_pi_omp_keep_full_tools_failures_and_stream_hook_context() {
    for provider in ["pi","omp"] {
        let h = Origin::new(false,false).await;
        let sid = "45ce9c2d-48d4-491e-8ff3-672bb10a69b6";
        let directory = h.home.join(if provider=="pi" {".pi/agent/sessions/wd"} else {".omp/agent/sessions/wd"});
        std::fs::create_dir_all(&directory).unwrap();
        let mut raw = relocate_json_path(include_str!("../fixtures/baton/pi.jsonl"), "/work/demo", &h.root);
        raw += &format!("{}\n{}\n",json!({"type":"message","id":"hook-user","message":{"role":"user","timestamp":1767225620000u64,"content":[{"type":"text","text":"contexto do hook\npedido sem prefixo"}]}}),
            json!({"type":"custom_message","customType":"claude-hook-context","parentId":"hook-user","content":"contexto do hook"}));
        std::fs::write(directory.join(format!("2026-01-01_{sid}.jsonl")),raw).unwrap();
        let r = client().get(format!("{}?project=-&session_id={sid}&provider={provider}",h.url("morta/bastao"))).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(r.status(),200);
        let md = r.text().await.unwrap();
        assert!(md.contains(&format!("`{}/summary.txt`",h.root.display())));
        assert!(md.contains("- Ferramentas que FALHARAM (as mais recentes):\n  - `write`: permission denied"),"{provider}: {md}");
        assert!(md.contains("pedido sem prefixo") && !md.contains("contexto do hook"));
        h.no_python();
    }
}

#[tokio::test]
async fn b01_next_kimi_archive_reads_the_index_cwd_and_removed_worktree_redirect() {
    let h = Origin::new(false,false).await;
    let sid = "session_ab697d05-66da-4b2a-8e80-3d769951e752";
    let session = h.home.join(".kimi-code/sessions/wd").join(sid);
    std::fs::create_dir_all(session.join("agents/main")).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/home/.kimi-code/sessions/wd_hangar_5112ff7a84e0/session_ab697d05-66da-4b2a-8e80-3d769951e752/agents/main/wire.jsonl");
    std::fs::copy(source,session.join("agents/main/wire.jsonl")).unwrap();
    let removed = h.home.join("removed-copy");
    std::fs::write(h.home.join(".kimi-code/session_index.jsonl"),format!("{}\n",json!({"sessionId":sid,"sessionDir":session,"workDir":removed}))).unwrap();
    std::fs::create_dir_all(h.home.join(".hangar")).unwrap();
    std::fs::write(h.home.join(".hangar/worktrees-removidas.json"),json!({removed.to_str().unwrap():h.root}).to_string()).unwrap();
    let r = client().get(format!("{}?project=-&session_id={sid}&provider=kimi",h.url("morta/bastao"))).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(),200);
    let md = r.text().await.unwrap();
    assert!(md.contains(&format!("- Diretório de trabalho: `{}`",h.root.display())),"{md}");
    assert!(md.contains("Olá!") && md.contains("- Branch: `main`"));
    h.no_python();
}

#[tokio::test]
async fn b01_clear_never_inherits_the_frozen_origin_or_imported_plan() {
    let dir = tempfile::tempdir().unwrap();
    let h = crate::workspace_routes::TransferredFiles::new(dir.path()).await;
    let thread = "019f99de-9572-7221-89c0-80c62f883d45";
    let path = h.rollout.with_file_name(format!("rollout-cleared-{thread}.jsonl"));
    std::fs::write(&path, format!("{}\n{}\n", json!({"type":"session_meta","payload":{"id":thread,"cwd":h.old}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"pedido depois do clear"}]}}))).unwrap();
    let mut metadata: Value = serde_json::from_slice(&std::fs::read(&h.sidecar).unwrap()).unwrap();
    metadata["thread_id"] = json!(thread); metadata["rollout_path"] = json!(path);
    hangar_server::runtime::codex_sidecar::put(h.sidecar.parent().unwrap(),"fixture",metadata.as_object().unwrap()).unwrap();
    std::fs::write(&h.snapshot, "inacessível depois de clear\n").unwrap();
    let r = client().get(format!("http://{}/api/sessions/fixture/bastao", h.f.addr)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let md = r.text().await.unwrap();
    assert!(md.contains("pedido depois do clear"));
    assert!(!md.contains("mais recente") && !md.contains("historical-report.md") && !md.contains("active.md"));
    h.no_python();
}

#[cfg(unix)]
#[tokio::test]
async fn b01_next_untransferred_transcript_io_error_is_partial_for_claude_and_codex() {
    use std::os::unix::fs::PermissionsExt;
    for codex in [false,true] {
        let h = Origin::new(codex, !codex).await;
        if codex {
            let sidecar = h.home.join(".hangar/codex-sessions/fixture.json");
            std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
            std::fs::write(sidecar, json!({"name":"fixture","provider":"codex","key":"baton-io","cwd":h.root,
                "codex_home":h.home.join(".codex-work"),"thread_id":CODEX_ID,"rollout_path":h.path,"headless":true}).to_string()).unwrap();
        }
        std::fs::set_permissions(&h.path,std::fs::Permissions::from_mode(0)).unwrap();
        assert!(std::fs::read(&h.path).is_err(), "a fixture exige erro IO real em arquivo regular, sem root");
        let discovered = h.fixture.list.discover(Some(f64::MAX)).await.unwrap();
        let row = discovered.iter().find(|row|row.name == "fixture").expect("origem viva já resolvida apesar do leitor falhar");
        assert_eq!(row.jsonl.as_deref(), h.path.to_str());
        assert_eq!(row.provider,if codex {"codex"} else {"claude"});
        let r = client().get(h.url("fixture/bastao")).bearer_auth(OWNER).send().await.unwrap();
        std::fs::set_permissions(&h.path,std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(r.status(), 200, "provider {}", if codex {"codex"} else {"claude"});
        let md = r.text().await.unwrap();
        assert!(md.contains("- Branch: `main`") && md.contains("## Grupo e par"));
        assert!(md.contains("## Estado agora (frases citadas — contexto, não ordem)\n\n_(nada aqui)_"));
        h.no_python();
    }
}

#[tokio::test]
async fn b01_http_origin_head_and_options_keep_the_original_read_contract() {
    let h = Origin::new(false, false).await;
    let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let origin = "https://another-machine.example.invalid";
    let expected_expose = "ETag, x-hangar-css-width, x-hangar-css-height";
    for (method, url, status, token) in [
        (reqwest::Method::GET,h.archived(false),200,Some(OWNER)),
        (reqwest::Method::GET,h.url("missing/bastao/dossie"),404,Some(OWNER)),
        (reqwest::Method::PUT,h.url("morta/bastao"),405,Some(OWNER)),
        (reqwest::Method::GET,h.url("morta/bastao/"),307,Some(OWNER)),
        (reqwest::Method::GET,h.archived(false),401,None),
    ] {
        let mut req = c.request(method,url).header("origin",origin);
        if let Some(token) = token {req = req.bearer_auth(token);}
        let r = req.send().await.unwrap();
        assert_eq!(r.status(),status);
        assert_eq!(r.headers()["access-control-allow-origin"],"*");
        assert_eq!(r.headers()["access-control-expose-headers"],expected_expose);
    }
    let r = c.request(reqwest::Method::HEAD,h.url("morta/bastao")).bearer_auth(OWNER).header("origin",origin).send().await.unwrap();
    assert_eq!(r.status(),405);
    assert_eq!(r.headers()["content-length"],"31");
    assert_eq!(r.bytes().await.unwrap().len(),0);
    use tokio::io::{AsyncReadExt,AsyncWriteExt};
    let mut socket = tokio::net::TcpStream::connect(h.fixture.addr).await.unwrap();
    socket.write_all(format!("HEAD /api/sessions/morta/bastao HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {OWNER}\r\nConnection: close\r\n\r\n",h.fixture.addr).as_bytes()).await.unwrap();
    let mut raw = Vec::new(); socket.read_to_end(&mut raw).await.unwrap();
    let boundary = raw.windows(4).position(|bytes|bytes == b"\r\n\r\n").unwrap()+4;
    assert!(String::from_utf8_lossy(&raw[..boundary]).starts_with("HTTP/1.1 405"));
    assert_eq!(raw.len(),boundary,"HEAD não envia corpo nem no fio, sem o filtro do reqwest");
    let r = c.request(reqwest::Method::OPTIONS,h.url("morta/bastao")).header("origin",origin)
        .header("access-control-request-method","GET").header("access-control-request-headers","authorization").send().await.unwrap();
    assert_eq!(r.status(),200);
    assert_eq!(r.headers()["access-control-allow-methods"],"DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT");
    assert_eq!(r.headers()["access-control-allow-headers"],"authorization");
    assert_eq!(r.text().await.unwrap(),"OK");
    h.no_python();
}

#[tokio::test]
async fn b01_guard_global_unrelated_corrupt_transfer_does_not_block_a_healthy_origin() {
    let h = Origin::new(false,true).await;
    let store = h.home.join(".hangar/conversation-transfers");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.json"),"{broken").unwrap();
    let before = std::fs::read(&h.path).unwrap();
    let r = client().get(h.url("fixture/bastao")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(),200);
    assert_eq!(r.text().await.unwrap(),h.expected(false,"fixture"));
    let r = client().post(h.url("fixture/bastao")).bearer_auth(OWNER)
        .json(&json!({"name":"fixture ","provider":"claude"})).send().await.unwrap();
    let status=r.status();let body=r.json::<Value>().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(status,400,"a recusa deve ser self, não corrupção de outra sessão: {body}");
    assert_eq!(body,json!({"detail":{"code":"erro_bastao_para_si_mesma","params":{},"msg":"a sessão não passa o bastão pra si mesma"}}));
    assert_eq!(std::fs::read(&h.path).unwrap(),before);
    assert!(!h.home.join(".claude/.hangar-bastao").exists());
    h.no_python();
}

#[tokio::test]
async fn b01_guard_parked_origin_uses_own_index_but_not_a_new_sidecar_life() {
    let dir = tempfile::tempdir().unwrap();
    let h = crate::workspace_routes::TransferredFiles::new(dir.path()).await;
    let mut record = h.record.clone();record["name"]=json!("morta");record["phase"]=json!("source_stopped");
    record["source_life"]=json!("k:old-parked");record["destination_meta"]=Value::Null;record["boundary"]=Value::Null;
    std::fs::write(&h.record_path,record.to_string()).unwrap();
    let digest:String=ring::digest::digest(&ring::digest::SHA256,b"morta").as_ref().iter().map(|b|format!("{b:02x}")).collect();
    std::fs::write(h.record_path.parent().unwrap().join("indexes").join(format!("session-{digest}.json")),json!([record["id"]]).to_string()).unwrap();
    let endpoint=format!("http://{}/api/sessions/morta/bastao",h.f.addr);
    let r=client().post(&endpoint).bearer_auth(OWNER).json(&json!({"provider":"claude"})).send().await.unwrap();
    let first_status=r.status();let first_body=r.json::<Value>().await.unwrap();
    let sidecar=h.home.join(".hangar/claude-headless/morta.json");std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
    std::fs::write(&sidecar,json!({"name":"morta","provider":"claude","key":"new-absent-life","headless":true}).to_string()).unwrap();
    let rows=h.f.list.discover(Some(f64::MAX)).await.unwrap();
    assert!(rows.iter().all(|r|r.name!="morta"),"o sidecar incompleto existe sem linha utilizável");
    let r=client().post(&endpoint).bearer_auth(OWNER).json(&json!({"provider":"claude"})).send().await.unwrap();
    let second_status=r.status();let second_body=r.json::<Value>().await.unwrap();
    h.f.runtime.shutdown().await.unwrap();
    assert_eq!(first_status,409);assert_eq!(first_body["detail"]["code"],"session_transfer_busy");
    assert_eq!(second_status,422,"sidecar de outra vida não é parked: {second_body}");
    assert_eq!(second_body,json!({"detail":[{"type":"missing","loc":["body","name"],"msg":"Field required","input":{"provider":"claude"}}]}));
    h.no_python();
}

#[tokio::test]
async fn b01_guard_source_busy_unreadable_and_recreated_life_are_separate() {
    let dir = tempfile::tempdir().unwrap();
    let h = crate::workspace_routes::TransferredFiles::new(dir.path()).await;
    let mut record = h.record.clone();record["phase"]=json!("source_stopped");
    std::fs::write(&h.record_path,record.to_string()).unwrap();
    let endpoint = format!("http://{}/api/sessions/fixture/bastao",h.f.addr);
    let r = client().post(&endpoint).bearer_auth(OWNER).json(&json!({"provider":"claude"})).send().await.unwrap();
    assert_eq!(r.status(),409);
    assert_eq!(r.json::<Value>().await.unwrap(),json!({"detail":{"code":"session_transfer_busy","params":{},"msg":"A sessão está trocando de agente; tente novamente quando terminar."}}));
    std::fs::write(&h.record_path,"{broken").unwrap();
    let r = client().post(&endpoint).bearer_auth(OWNER).json(&json!({"provider":"claude"})).send().await.unwrap();
    assert_eq!(r.status(),503,"marcador do próprio alvo ilegível não autoriza efeitos");
    assert_eq!(r.json::<Value>().await.unwrap()["detail"]["code"],"workspace_context");
    std::fs::write(&h.record_path,record.to_string()).unwrap();
    let mut metadata:Value=serde_json::from_slice(&std::fs::read(&h.sidecar).unwrap()).unwrap();
    metadata.as_object_mut().unwrap().remove("transfer_id");metadata["key"]=json!("new-baton-life");
    hangar_server::runtime::codex_sidecar::put(h.sidecar.parent().unwrap(),"fixture",metadata.as_object().unwrap()).unwrap();
    let rows=h.f.list.discover(Some(f64::MAX)).await.unwrap();
    assert_eq!(rows.iter().find(|r|r.name=="fixture").unwrap().lifecycle_id.as_deref(),Some("k:new-baton-life"));
    let r = client().post(&endpoint).bearer_auth(OWNER).json(&json!({"provider":"claude"})).send().await.unwrap();
    assert_eq!(r.status(),422,"a nova vida chega à validação DTO sem herdar a transferência antiga");
    assert_eq!(r.json::<Value>().await.unwrap(),json!({"detail":[{"type":"missing","loc":["body","name"],"msg":"Field required","input":{"provider":"claude"}}]}));
    assert!(!h.home.join(".claude/.hangar-bastao").exists());
    h.no_python();
}

#[tokio::test]
async fn b01_post_guards_precede_saving_and_never_reach_python() {
    let h = Origin::new(false, true).await;
    let c = client();
    for (body, status, code) in [
        (json!({"name":"fixture ","provider":"claude"}), 400, "erro_bastao_para_si_mesma"),
        (json!({"name":"  ","provider":"claude"}), 400, "erro_nome_invalido"),
        (json!({"name":"next","cwd":h.home.join("missing"),"provider":"claude"}), 400, "erro_bastao_cwd_inexistente"),
        (json!({"name":"next","codex_account":"default","provider":"claude"}), 400, "codex_account_so_codex"),
    ] {
        let r = c.post(h.url("fixture/bastao")).bearer_auth(OWNER).json(&body).send().await.unwrap();
        assert_eq!(r.status(), status, "{body}");
        assert_eq!(r.json::<Value>().await.unwrap()["detail"]["code"], code);
        assert!(!h.home.join(".claude/.hangar-bastao").exists(), "nenhuma gravação antes da guarda");
    }
    h.no_python();
}

#[tokio::test]
async fn b01_external_http_keeps_methods_redirects_mime_and_validation_local() {
    #[cfg(unix)]
    {
        if post_child("baton::b01_external_http_keeps_methods_redirects_mime_and_validation_local") {return;}
        let root=PathBuf::from(std::env::var_os("HANGAR_BATON_CHILD_ROOT").unwrap());
        std::fs::write(root.join("home/.profile"),format!("PATH='{}:/usr/bin'\nexport PATH\n",root.join("bin").display())).unwrap();
    }
    hangar_server::accounts::quotas::disable_open_network_read();hangar_server::diag::disable_diary();
    let h = Origin::new(false, false).await;
    std::fs::write(h.home.join(".claude/runtime-config.json"),r#"{"last_session_provider":"claude"}"#).unwrap();
    let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let r = c.put(h.url("morta/bastao")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 405);
    let r = c.get(h.url("morta/bastao/")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 307);
    assert_eq!(r.headers()["location"], h.url("morta/bastao"));
    for (content_type, body, expected) in [
        ("application/json", "{", json!({"type":"json_invalid","loc":["body",1],"msg":"JSON decode error","input":{},"ctx":{"error":"Expecting property name enclosed in double quotes"}})),
        ("text/plain", r#"{"name":"next","provider":"claude"}"#, json!({"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":r#"{"name":"next","provider":"claude"}"#})),
        ("application/json", "{}", json!({"type":"missing","loc":["body","name"],"msg":"Field required","input":{"provider":"claude"}})),
        ("text/plain", "{}", json!({"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":r#"{"provider":"claude"}"#})),
    ] {
        let r = c.post(h.url("morta/bastao")).bearer_auth(OWNER).header("content-type", content_type).body(body).send().await.unwrap();
        assert_eq!(r.status(), 422);
        assert_eq!(r.json::<Value>().await.unwrap(), json!({"detail":[expected]}));
    }
    h.fixture.runtime.shutdown().await.unwrap();
    h.no_python();
}

#[tokio::test]
async fn b01_seeded_external_payloads_never_panic_or_forward() {
    use proptest::{prelude::*, test_runner::{Config, RngAlgorithm, TestRng, TestRunner}};
    let h = Origin::new(false, false).await;
    let mut runner = TestRunner::new_with_rng(Config::default(), TestRng::from_seed(RngAlgorithm::ChaCha, &[31;32]));
    let strategy = (".{0,100}", any::<i64>(), any::<bool>());
    for _ in 0..48 {
        use proptest::strategy::ValueTree;
        let (name, number, flag) = strategy.new_tree(&mut runner).unwrap().current();
        let r = client().post(h.url("morta/bastao")).bearer_auth(OWNER)
            .json(&json!({"name":name,"provider":"claude","headless":number,"extra":flag})).send().await.unwrap();
        assert_eq!(r.status(), 422);
        assert!(r.json::<Value>().await.unwrap()["detail"].is_array());
    }
    assert!(!h.home.join(".claude/.hangar-bastao").exists());
    h.no_python();
}

#[tokio::test]
async fn b01_disk_refusal_is_500_without_opening_a_successor() {
    let h = Origin::new(false, true).await;
    let blocked = h.home.join(".claude/.hangar-bastao");
    std::fs::write(&blocked, "não sobrescrever").unwrap();
    let r = client().post(h.url("fixture/bastao")).bearer_auth(OWNER).json(&json!({"name":"next","provider":"claude"})).send().await.unwrap();
    assert_eq!(r.status(), 500);
    let body = r.json::<Value>().await.unwrap();
    assert_eq!(body["detail"]["code"], "erro_bastao_gravar");
    assert!(body["detail"]["params"]["motivo"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(std::fs::read_to_string(blocked).unwrap(), "não sobrescrever");
    assert!(std::fs::read_dir(h.home.join(".hangar/session-specs")).is_err());
    h.no_python();
}

#[cfg(unix)]
fn post_child(test: &str) -> bool {
    if std::env::var_os("HANGAR_BATON_CHILD_ROOT").is_none() {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["bin","tmp","runtime","home/.claude"] {std::fs::create_dir_all(dir.path().join(sub)).unwrap();}
        let bin = dir.path().join("bin");
        crate::write_executable(&bin.join("claude"), r#"#!/bin/sh
case "$1" in
--version) printf '2.1.195\n';;
--help) printf '%s\n' '--plugin-dir --permission-mode --dangerously-skip-permissions --model --effort';;
auth) [ "$2" = status ] || exit 78; printf '%s\n' '{"loggedIn":true,"email":"fixture@example.invalid","subscriptionType":"pro"}';;
-p) printf '%s\n' "$CLAUDE_CONFIG_DIR" > "$HANGAR_BATON_CHILD_ROOT/rewrite-account"; printf '%s\000' "$@" > "$HANGAR_BATON_CHILD_ROOT/rewrite-argv"; input=''; while IFS= read -r row || [ -n "$row" ]; do input="$input$row
"; done; printf '%s' "$input" > "$HANGAR_BATON_CHILD_ROOT/rewrite-input"; printf 'call\n' >> "$HANGAR_BATON_CHILD_ROOT/rewrite-calls"
mode=positive; if [ -f "$HANGAR_BATON_CHILD_ROOT/rewrite-mode" ]; then IFS= read -r mode < "$HANGAR_BATON_CHILD_ROOT/rewrite-mode"; fi
case "$mode" in
positive) printf '# Resumo reescrito pela fixture shell\n\n%s' "${input#*--- resumo automático ---
}";;
short) printf 'Não.\n';;
fail) printf 'fixture refusal\n' >&2; exit 17;;
*) printf 'Modo sintético desconhecido\n' >&2; exit 78;;
esac;;
*) printf 'CLI sintético não implementa esta chamada\n' >&2; exit 78;;
esac
"#);
        crate::write_executable(&bin.join("tmux"), r#"#!/bin/sh
root="$HANGAR_BATON_CHILD_ROOT"
case "$1" in
list-panes) if [ -f "$root/session-name" ]; then IFS= read -r name < "$root/session-name"; provider=claude; [ ! -f "$root/codex-fixture" ] || provider=codex; printf '%s\t1\t\t%s/repo\t%%1\t\t%s\t1793966722\t0\t0\n' "$name" "$root" "$provider"; fi;;
list-sessions) [ -f "$root/codex-fixture" ] || { printf 'Sonda fora do perfil sintético\n' >&2; exit 78; }
if [ -f "$root/session-name" ]; then IFS= read -r name < "$root/session-name"; printf '%s\t%s:$1:1793966722\n' "$name" "$$"; fi;;
new-session) previous=''; name=''; for value in "$@"; do [ "$previous" != -s ] || name="$value"; previous="$value"; done
printf '%s\000' "$@" >> "$root/new-session-argv"
if [ -f "$root/hold-opening" ]; then printf ready > "$root/new-session-ready"; IFS= read -r release < "$root/open-release" || exit 1; fi
printf '%s\n' "$name" > "$root/session-name"; printf done > "$root/new-session-done";;
has-session) target=''; previous=''; for value in "$@"; do [ "$previous" != -t ] || target="$value"; previous="$value"; done
target="${target#=}"; target="${target%:}"
if [ -f "$root/session-name" ]; then IFS= read -r current < "$root/session-name"; [ "$current" != "$target" ] || exit 0; fi
printf "can't find session\n" >&2; exit 1;;
display-message) case "$*" in
*'#{pane_pid}'*) :;;
*'#{session_created}'*) printf '1793966722\n';;
*) printf 'Formato sintético não implementado\n' >&2; exit 78;;
esac;;
-V) printf 'tmux 3.4\n';;
capture-pane) printf '> \n';;
set-environment|set-option|select-window|select-pane|send-keys|resize-pane) :;;
*) printf 'Mux sintético não implementa %s\n' "$1" >&2; exit 78;;
esac
"#);
        crate::write_executable(&bin.join("systemctl"), "#!/bin/sh\nexit 1\n");
        std::fs::write(dir.path().join("home/.claude/.credentials.json"),r#"{"claudeAiOauth":{"accessToken":"fixture-only","subscriptionType":"pro"}}"#).unwrap();
        std::fs::write(dir.path().join("home/.claude/settings.json"),r#"{"permissions":{"defaultMode":"bypassPermissions"}}"#).unwrap();
        std::fs::write(dir.path().join("home/.claude.json"),r#"{"numStartups":3}"#).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact",test,"--nocapture"])
            .env_clear().env("PATH",format!("{}:/usr/bin",bin.display()))
            .env("HOME",dir.path().join("home")).env("USERPROFILE",dir.path().join("home"))
            .env("CLAUDE_CONFIG_DIR",dir.path().join("home/.claude"))
            .env("TMPDIR",dir.path().join("tmp")).env("XDG_RUNTIME_DIR",dir.path().join("runtime"))
            .env("XDG_CACHE_HOME",dir.path().join("cache")).env("LANG","C.UTF-8")
            .env("HANGAR_BATON_CHILD_ROOT",dir.path()).env("CP_RUST_NO_ORPHAN_SWEEP","1").env("HANGAR_QUOTA_NETWORK","0")
            .envs(matches!(test,"baton::b01_codex_terminal_successor_inherits_the_live_origin_account"|"baton::b01_codex_post_rejects_a_different_known_successor_account_before_effects")
                .then_some(("HANGAR_RUNTIME_INSTANCE","workspace-fixture")))
            .env("CP_SCAN_ROOTS",dir.path().join("repo")).env("CP_CODEX_SYNC_ENABLED","0")
            .output().unwrap();
        assert!(output.status.success(),"filho {:?}\n{}\n{}",output.status.code(),String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        return true;
    }
    false
}

#[cfg(unix)]
#[test]
fn b01_next_post_public_dossier_open_kickoff_and_occupied_name() {
    if post_child("baton::b01_next_post_public_dossier_open_kickoff_and_occupied_name") {return;}
    hangar_server::accounts::quotas::disable_open_network_read();
    hangar_server::diag::disable_diary();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let h = Origin::new(false,false).await;
        let c = client();
        let request = json!({"name":"api.v2","project":"-mnt-case-work-repo","session_id":CLAUDE_ID,
            "origem_config_dir":h.home.join(".claude"),"provider":"claude"});
        let r = c.post(h.url("morta/bastao")).bearer_auth(OWNER).json(&request).send().await.unwrap();
        let status=r.status();let headers=r.headers().clone();let raw=r.text().await.unwrap();
        assert_eq!(status,200,"{raw}");
        assert_eq!(headers["content-type"],"application/json");
        let body:Value=serde_json::from_str(&raw).unwrap();
        let file = h.home.join(".claude/.hangar-bastao/api-v2.md");
        assert_eq!(body["name"],"api-v2");assert_eq!(body["dossie"],json!(file));
        assert_eq!(body["texto"],h.expected(false,"morta"));assert_eq!(body["aviso"],Value::Null);
        assert!(body["avisos"].is_array());
        assert_eq!(std::fs::read_to_string(&file).unwrap(),body["texto"].as_str().unwrap());
        let received = c.get(h.url("api-v2/bastao/dossie")).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(received.status(),200);assert_eq!(received.text().await.unwrap(),body["texto"].as_str().unwrap());
        let args = std::fs::read_to_string(h.home.parent().unwrap().join("new-session-argv")).unwrap();
        assert_eq!(args.matches(body["kickoff"].as_str().unwrap()).count(),1,"kickoff vai uma vez pelo initial_prompt");
        assert!(!h.home.join(".claude/.hangar-queue/api-v2.jsonl").exists(),"terminal não duplica kickoff na fila");
        let before = std::fs::read(&file).unwrap();
        let r = c.post(h.url("morta/bastao")).bearer_auth(OWNER).json(&request).send().await.unwrap();
        assert_eq!(r.status(),409);assert_eq!(r.json::<Value>().await.unwrap()["detail"]["code"],"erro_nome_em_uso");
        assert_eq!(std::fs::read(&file).unwrap(),before);
        h.no_python();
    });
}

#[cfg(unix)]
#[test]
fn b01_codex_terminal_successor_inherits_the_live_origin_account() {
    if post_child("baton::b01_codex_terminal_successor_inherits_the_live_origin_account") {return;}
    hangar_server::accounts::quotas::disable_open_network_read();hangar_server::diag::disable_diary();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let h=Origin::router_only(true,false).await;let root=h.home.parent().unwrap();let work=h.home.join(".codex-work");
        assert_eq!(std::env::var_os("HOME"),Some(h.home.clone().into_os_string()));
        assert_eq!(std::env::var("CP_RUST_NO_ORPHAN_SWEEP").unwrap(),"1");assert_eq!(std::env::var("HANGAR_QUOTA_NETWORK").unwrap(),"0");
        assert!(std::env::var_os("CP_RUST_CANO_BIN").is_none());
        std::fs::write(root.join("codex-fixture"),"codex\n").unwrap();
        crate::write_executable(&root.join("bin/codex"),r#"#!/bin/sh
root="$HANGAR_BATON_CHILD_ROOT"
printf '%s\000' "$@" >> "$root/codex-probes"
case "$1" in
--version) printf 'codex-cli 0.159.3\n';;
--help) printf '%s\n' 'app-server --remote';;
*) printf '%s\000' "$@" >> "$root/codex-unknown"; printf 'Sonda Codex sintética não implementada\n' >&2; exit 78;;
esac
"#);
        crate::write_executable(&root.join("bin/hangar-codex-tui"),"#!/bin/sh\nprintf '%s\\000' \"$@\" >> \"$HANGAR_BATON_CHILD_ROOT/launcher-executed\"\nprintf 'Lançador não deve executar fora do mux sintético\\n' >&2\nexit 78\n");
        let sidecar=h.home.join(".hangar/codex-sessions/fixture.json");std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(&sidecar,json!({"name":"fixture","provider":"codex","key":"baton-codex-origin","cwd":h.root,
            "codex_home":work,"thread_id":CODEX_ID,"rollout_path":h.path,"headless":true}).to_string()).unwrap();
        let accounts=hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::capture());
        let account=accounts.resolve(hangar_server::accounts::Provider::Codex,"work").unwrap();assert_eq!(account.home,work);
        assert_eq!(hangar_server::config::Config::runtime_instance().unwrap().as_deref(),Some("workspace-fixture"));
        let key=hangar_server::accounts::AccountKey::new(hangar_server::accounts::Provider::Codex,&work).unwrap();
        let prepared=accounts.locks.sidecar(&key,"prepare-result.json").unwrap();
        std::fs::create_dir_all(prepared.parent().unwrap()).unwrap();
        std::fs::write(&prepared,serde_json::to_vec(&json!({"status":"ready","trust_pending":false,"issues":[],"etapa":null,"herdado":null})).unwrap()).unwrap();
        let marker=work.join(hangar_server::accounts::preparation::PREPARED_MARKER);
        std::fs::write(&marker,b"{\"version\":1}").unwrap();assert!(marker.is_file());
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&prepared).unwrap()).unwrap()["status"],"ready");
        assert!(!accounts.locks.sidecar(&key,"prepare.json").unwrap().exists() && !accounts.locks.sidecar(&key,"prepare-force.json").unwrap().exists());
        let bridge=hangar_server::accounts::bridge::AccountsBridge::new("workspace-fixture".into());
        assert_eq!(accounts.preparation_status(&account,&bridge).await["status"],"ready");
        let rows=h.fixture.list.discover(Some(f64::MAX)).await.unwrap();
        let origin=rows.iter().find(|row|row.name=="fixture").expect("origem Codex viva na descoberta nativa");
        assert_eq!(origin.provider,"codex");assert_eq!(origin.codex_home.as_deref(),work.to_str());assert_eq!(origin.jsonl.as_deref(),h.path.to_str());
        let response=client().post(h.url("fixture/bastao")).bearer_auth(OWNER)
            .json(&json!({"name":"cx2","provider":"codex","headless":false})).send().await.unwrap();
        let status=response.status();let body=response.json::<Value>().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
        assert_eq!(status,200,"{body}");assert_eq!(body["name"],"cx2");
        let file=h.home.join(".claude/.hangar-bastao/cx2.md");assert_eq!(body["dossie"],json!(file));
        assert_eq!(body["texto"],h.expected(true,"fixture"));assert_eq!(std::fs::read_to_string(file).unwrap(),body["texto"].as_str().unwrap());
        let specs=std::fs::read_dir(h.home.join(".hangar/session-specs")).unwrap().filter_map(|entry| {
            let path=entry.unwrap().path();(path.extension().and_then(|s|s.to_str())==Some("json"))
                .then(||serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap())
        }).filter(|spec|spec["name"]=="cx2").collect::<Vec<_>>();
        assert_eq!(specs.len(),1,"um spec da abertura efetiva: {specs:?}");assert_eq!(specs[0]["provider"],"codex");
        assert_eq!(specs[0]["surface"],"terminal");assert_eq!(specs[0]["account"],json!({"kind":"codex","account_id":"work"}));
        let args=std::fs::read_to_string(root.join("new-session-argv")).unwrap();
        assert!(args.contains("hangar-codex-tui") && args.contains("--codex-home") && args.contains(work.to_str().unwrap()) && args.contains("--codex-account"),"{args}");
        assert_eq!(args.matches(body["kickoff"].as_str().unwrap()).count(),1,"kickoff único no comando do lançador");
        assert!(!root.join("launcher-executed").exists(),
            "launcher-executed={:?}",std::fs::read(root.join("launcher-executed")));
        assert!(!h.home.join(".claude/.hangar-queue/cx2.jsonl").exists());h.no_python();
    });
}

#[cfg(unix)]
#[test]
fn b01_codex_post_rejects_a_different_known_successor_account_before_effects() {
    if post_child("baton::b01_codex_post_rejects_a_different_known_successor_account_before_effects") {return;}
    hangar_server::accounts::quotas::disable_open_network_read();hangar_server::diag::disable_diary();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let h=Origin::router_only(true,false).await;let root=h.home.parent().unwrap();let work=h.home.join(".codex-work");let other=h.home.join(".codex-other");
        assert_eq!(std::env::var_os("HOME"),Some(h.home.clone().into_os_string()));
        assert_eq!(std::env::var("CP_RUST_NO_ORPHAN_SWEEP").unwrap(),"1");assert_eq!(std::env::var("HANGAR_QUOTA_NETWORK").unwrap(),"0");
        assert!(std::env::var_os("CP_RUST_CANO_BIN").is_none());
        std::fs::create_dir_all(other.join("sessions")).unwrap();std::fs::write(other.join(".hangar-codex-conta"),r#"{"version":1,"id":"other"}"#).unwrap();
        std::fs::write(root.join("codex-fixture"),"codex\n").unwrap();
        crate::write_executable(&root.join("bin/codex"),"#!/bin/sh\nprintf '%s\\000' \"$@\" >> \"$HANGAR_BATON_CHILD_ROOT/codex-probes\"\nprintf 'Codex não deve executar na recusa prévia\\n' >&2\nexit 78\n");
        crate::write_executable(&root.join("bin/hangar-codex-tui"),"#!/bin/sh\nprintf '%s\\000' \"$@\" >> \"$HANGAR_BATON_CHILD_ROOT/launcher-executed\"\nexit 78\n");
        let sidecar=h.home.join(".hangar/codex-sessions/fixture.json");std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(&sidecar,json!({"name":"fixture","provider":"codex","key":"baton-codex-origin","cwd":h.root,
            "codex_home":work,"thread_id":CODEX_ID,"rollout_path":h.path,"headless":true}).to_string()).unwrap();
        let accounts=hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::capture());
        assert_eq!(accounts.resolve(hangar_server::accounts::Provider::Codex,"work").unwrap().home,work);
        assert_eq!(accounts.resolve(hangar_server::accounts::Provider::Codex,"other").unwrap().home,other);
        assert_eq!(hangar_server::config::Config::runtime_instance().unwrap().as_deref(),Some("workspace-fixture"));
        let rows=h.fixture.list.discover(Some(f64::MAX)).await.unwrap();
        let origin=rows.iter().find(|row|row.name=="fixture").expect("origem Codex viva na descoberta nativa");
        assert_eq!(origin.provider,"codex");assert_eq!(origin.codex_home.as_deref(),work.to_str());assert_eq!(origin.jsonl.as_deref(),h.path.to_str());
        let before=std::fs::read(&h.path).unwrap();
        let response=client().post(h.url("fixture/bastao")).bearer_auth(OWNER)
            .json(&json!({"name":"cx2","provider":"codex","codex_account":"other","headless":false})).send().await.unwrap();
        let status=response.status();let body=response.json::<Value>().await.unwrap();h.fixture.runtime.shutdown().await.unwrap();
        assert_eq!(status,409,"{body}");assert_eq!(body["detail"]["code"],"codex_account_archive_mismatch");
        assert_eq!(std::fs::read(&h.path).unwrap(),before);assert!(h.path.is_file());
        assert!(!h.home.join(".claude/.hangar-bastao").exists());assert!(!h.home.join(".claude/.hangar-queue/cx2.jsonl").exists());
        assert!(!h.home.join(".hangar/codex-sessions/cx2.json").exists());
        assert!(!root.join("new-session-argv").exists() && !root.join("launcher-executed").exists() && !root.join("codex-probes").exists());
        let specs=match std::fs::read_dir(h.home.join(".hangar/session-specs")) {
            Ok(entries)=>entries.filter_map(|entry| {
                let path=entry.unwrap().path();(path.extension().and_then(|s|s.to_str())==Some("json"))
                    .then(||serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap())
            }).collect::<Vec<_>>(),
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Vec::new(),
            Err(error)=>panic!("não foi possível conferir os specs privados: {error}"),
        };
        assert!(specs.iter().all(|spec|spec["name"]!="cx2"),"nenhuma abertura efetiva: {specs:?}");h.no_python();
    });
}

#[cfg(unix)]
#[test]
fn b01_manual_kickoff_preserves_six_lines_and_explicit_origin_metadata() {
    if post_child("baton::b01_manual_kickoff_preserves_six_lines_and_explicit_origin_metadata") {return;}
    hangar_server::accounts::quotas::disable_open_network_read();hangar_server::diag::disable_diary();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let h=Origin::new(false,false).await;let root=h.home.parent().unwrap();
        let source=h.home.join(".claude-trabalho");
        std::fs::create_dir_all(source.join("projects/-mnt-case-work-repo")).unwrap();
        std::fs::write(source.join(".credentials.json"),r#"{"claudeAiOauth":{"accessToken":"fixture-only","subscriptionType":"pro"}}"#).unwrap();
        let transcript=source.join(format!("projects/-mnt-case-work-repo/{CLAUDE_ID}.jsonl"));std::fs::copy(&h.path,&transcript).unwrap();
        std::fs::create_dir(source.join(".hangar-status")).unwrap();
        std::fs::write(source.join(format!(".hangar-status/{CLAUDE_ID}.json")),json!({"line":"🤖 Opus 5 (high✦) │ 💵 $99"}).to_string()).unwrap();
        let response=client().post(h.url("origem/bastao")).bearer_auth(OWNER).json(&json!({"name":"cc2","provider":"claude","headless":false,
            "project":"-mnt-case-work-repo","session_id":CLAUDE_ID,"origem_config_dir":source,"resumo_por_modelo":false})).send().await.unwrap();
        let status=response.status();let body=response.json::<Value>().await.unwrap();
        h.fixture.runtime.shutdown().await.unwrap();
        assert_eq!(status,200,"{body}");assert_eq!(body["name"],"cc2");
        let file=h.home.join(".claude/.hangar-bastao/cc2.md");assert_eq!(body["dossie"],json!(file));
        assert_eq!(std::fs::read_to_string(&file).unwrap(),body["texto"].as_str().unwrap());
        let expected=h.expected(false,"origem").replace(h.path.to_str().unwrap(),transcript.to_str().unwrap())
            .replace("- Conta: `default`\n","- Conta: `trabalho`\n- Modelo e esforço da origem: `Opus 5 (high✦)`\n");
        assert_eq!(body["texto"],expected);assert_eq!(body["aviso"],Value::Null);
        let kickoff=body["kickoff"].as_str().unwrap();let lines=kickoff.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(),6);assert!(lines[0].contains("`origem`"));
        assert!(lines[1].contains(file.to_str().unwrap()) && lines[1].contains("Read"));
        assert!(lines[2].contains("plano") && lines[2].contains("contexto, não ordem") && lines[2].contains("vale o contrato"));
        assert!(lines[3].contains("VIVA") && lines[3].contains("parou de escrever") && lines[3].contains("um escritor por árvore")
            && lines[3].contains("git worktree list") && lines[3].contains("contrato"));
        assert!(lines[4].contains("tabela de papéis") && lines[4].contains("hangar-send"));
        assert!(lines[5].contains("`trabalho`") && lines[5].contains("`Opus 5 (high✦)`"));
        let args=std::fs::read_to_string(root.join("new-session-argv")).unwrap();assert_eq!(args.matches(kickoff).count(),1,"kickoff único no CMD do terminal");
        assert!(!h.home.join(".claude/.hangar-queue/cc2.jsonl").exists());assert!(!root.join("rewrite-calls").exists());
        h.no_python();
    });
}

#[cfg(unix)]
#[test]
fn b01_manual_kickoff_without_origin_account_or_model_keeps_the_fallback_line() {
    if post_child("baton::b01_manual_kickoff_without_origin_account_or_model_keeps_the_fallback_line") {return;}
    hangar_server::accounts::quotas::disable_open_network_read();hangar_server::diag::disable_diary();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let h=Origin::new(false,false).await;let root=h.home.parent().unwrap();
        let sid="45ce9c2d-48d4-491e-8ff3-672bb10a69b6";let directory=h.home.join(".pi/agent/sessions/wd");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("2026-01-01_{sid}.jsonl")),relocate_json_path(include_str!("../fixtures/baton/pi.jsonl"), "/work/demo", &h.root)).unwrap();
        let response=client().post(h.url("origem-pi/bastao")).bearer_auth(OWNER).json(&json!({"name":"cc2","provider":"claude","headless":false,
            "project":"-","session_id":sid,"origem_provider":"pi","resumo_por_modelo":false})).send().await.unwrap();
        let status=response.status();let body=response.json::<Value>().await.unwrap();
        h.fixture.runtime.shutdown().await.unwrap();
        assert_eq!(status,200,"{body}");assert_eq!(body["name"],"cc2");
        let file=h.home.join(".claude/.hangar-bastao/cc2.md");assert_eq!(body["dossie"],json!(file));
        let dossier=body["texto"].as_str().unwrap();assert_eq!(std::fs::read_to_string(file).unwrap(),dossier);
        let origin=dossier.split("## De onde veio").nth(1).expect("seção da origem").split("\n## ").next().unwrap();
        assert!(!origin.contains("- Conta:") && !origin.contains("- Modelo e esforço da origem:"),"{origin}");
        let kickoff=body["kickoff"].as_str().unwrap();assert_eq!(kickoff.lines().count(),6);
        assert!(!kickoff.contains("``") && !kickoff.contains("None"),"{kickoff}");assert!(kickoff.contains("primeira seção do resumo"));
        let args=std::fs::read_to_string(root.join("new-session-argv")).unwrap();assert_eq!(args.matches(kickoff).count(),1,"kickoff único no CMD do terminal");
        assert!(!h.home.join(".claude/.hangar-queue/cc2.jsonl").exists());assert!(!root.join("rewrite-calls").exists());
        h.no_python();
    });
}

#[cfg(unix)]
#[test]
fn b01_post_live_claude_headless_delivers_once_with_one_runtime_queue_entry() {
    if post_child("baton::b01_post_live_claude_headless_delivers_once_with_one_runtime_queue_entry") {return;}
    hangar_server::accounts::quotas::disable_open_network_read();hangar_server::diag::disable_diary();
    crate::use_cano_bin();
    assert_eq!(std::env::var("CP_RUST_NO_ORPHAN_SWEEP").unwrap(),"1");
    assert_eq!(std::env::var("HANGAR_QUOTA_NETWORK").unwrap(),"0");
    assert!(std::env::var_os("CP_RUST_CANO_BIN").is_none());
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt,BufReader};
        let root=PathBuf::from(std::env::var_os("HANGAR_BATON_CHILD_ROOT").unwrap());
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        std::fs::write(root.join("headless-port"),format!("{}\n",listener.local_addr().unwrap().port())).unwrap();
        std::fs::write(root.join("session-name"),"fixture\n").unwrap();
        let fake=root.join("baton-headless-agent.sh");
        crate::write_executable(&fake,r#"#!/bin/bash
root="$HANGAR_BATON_CHILD_ROOT"
printf '%s\n' "$$" > "$root/headless-pid"
printf '%s\000' "$@" > "$root/headless-argv"
printf '%s\n' "$PWD" "$CLAUDE_CONFIG_DIR" "$HOME" "$HANGAR_CANO_OWNER" > "$root/headless-context"
trap 'exit 0' USR1
while IFS= read -r -t 120 line; do
  case "$line" in
    *'"type":"control_request"'*)
      rid=$(printf '%s\n' "$line" | jq -c '.request_id') || exit 78
      printf '{"type":"control_response","response":{"subtype":"success","request_id":%s,"response":{"commands":[]}}}\n' "$rid";;
    *'"type":"user"'*)
      printf '%s\n' "$line" >> "$root/headless-users.jsonl"
      printf '%s\n' "$line" | jq -c '.isReplay = true' || exit 78
      printf '%s\n' '{"type":"result","subtype":"success","result":"fixture-only"}'
      IFS= read -r port < "$root/headless-port"
      printf 'baton-headless\n%s\n' "$line" > "/dev/tcp/127.0.0.1/$port" || exit 78;;
    *) printf 'Quadro sintético não implementado\n' >&2; exit 78;;
  esac
done
"#);
        crate::write_executable(&root.join("bin/claude"),format!(r#"#!/bin/bash
case "$1" in
--version) printf '2.1.195\n';;
--help) printf '%s\n' '--plugin-dir --permission-mode --dangerously-skip-permissions --model --effort';;
auth) [ "$2" = status ] || exit 78; printf '%s\n' '{{"loggedIn":true,"email":"fixture@example.invalid","subscriptionType":"pro"}}';;
-p) case " $* " in *' --input-format stream-json '*) exec -a claude /bin/bash '{}' "$@";; *) exit 78;; esac;;
*) printf 'CLI sintético não implementa esta chamada\n' >&2; exit 78;;
esac
"#,fake.display()));
        let h=Origin::new(false,true).await;
        let hubs=hangar_server::side::Hubs::default();
        h.fixture.runtime.bind_lifecycle();h.fixture.runtime.bind_hubs(hubs.downgrade());
        let mut events=h.fixture.runtime.subscribe();
        let observed:Result<_,String>=async {
            let response=client().post(h.url("fixture/bastao")).bearer_auth(OWNER)
                .json(&json!({"name":"cc2","provider":"claude","model":"opus","headless":true})).send().await.map_err(|e|e.to_string())?;
            let status=response.status();let body=response.json::<Value>().await.map_err(|e|e.to_string())?;
            let delivered=if status==200 {
                tokio::time::timeout(Duration::from_secs(25),async {
                    loop {
                        let (stream,_)=listener.accept().await.map_err(|e|e.to_string())?;
                        let mut reader=BufReader::new(stream);let mut marker=String::new();
                        reader.read_line(&mut marker).await.map_err(|e|e.to_string())?;
                        if marker!="baton-headless\n" {continue;}
                        let mut frame=String::new();reader.read_line(&mut frame).await.map_err(|e|e.to_string())?;
                        return serde_json::from_str::<Value>(&frame).map_err(|e|e.to_string());
                    }
                }).await.map_err(|_|"primeira entrega sem sinal da CLI privada".to_owned())??
            } else {Value::Null};
            Ok((status,body,delivered))
        }.await;
        let meta=std::fs::read(root.join("home/.hangar/claude-headless/cc2.json")).ok()
            .and_then(|raw|serde_json::from_slice::<Value>(&raw).ok()).unwrap_or(Value::Null);
        let pid=std::fs::read_to_string(root.join("headless-pid")).ok().and_then(|s|s.trim().parse::<u32>().ok());
        let ours=pid.is_some_and(|pid|hangar_server::runtime::process::argv(pid).is_some_and(|argv|argv.iter().any(|arg|arg==fake.to_str().unwrap())));
        // A CLI sai antes do cano: claude_exit só chega depois que ele colheu o filho.
        let signalled=ours && unsafe {libc::kill(pid.unwrap() as i32,libc::SIGUSR1)}==0;
        let joined=if signalled {
            tokio::time::timeout(Duration::from_secs(25),async {
                while let Ok(event)=events.recv().await {
                    if event.channel=="claude_exit" && meta["key"].as_str()==Some(event.key.as_str()) {return true;}
                }
                false
            }).await.unwrap_or(false)
        } else {false};
        let stopped=h.fixture.runtime.shutdown().await;
        let cano=serde_json::from_value::<hangar_server::runtime::process::Cano>(meta["cano"].clone());
        let killed=match &cano {
            Ok(cano)=>hangar_server::runtime::process::kill(cano,meta["key"].as_str().unwrap_or(""),&h.home.join(".hangar/claude-headless")).await.is_ok(),
            Err(_)=>false,
        };
        let agent_stopped=pid.is_some_and(|pid|hangar_server::runtime::process::argv(pid).is_none_or(|argv|argv.is_empty()));
        let cano_stopped=meta["cano"]["pid"].as_u64().is_some_and(|pid|matches!(hangar_server::runtime::process::liveness(pid as u32,meta["key"].as_str().unwrap_or("")),hangar_server::runtime::process::Liveness::Dead));
        drop(hubs);
        assert!(stopped.is_ok(),"shutdown: {stopped:?}; signal={signalled},join={joined},kill={killed},cli={agent_stopped},cano={cano_stopped}; HTTP={observed:?}");
        assert!(signalled && joined && killed && agent_stopped && cano_stopped,"CLI e cano privados precisam terminar: signal={signalled},join={joined},kill={killed},cli={agent_stopped},cano={cano_stopped}; HTTP={observed:?}");
        let (status,body,delivered)=observed.unwrap();assert_eq!(status,200,"{body}");assert_eq!(body["name"],"cc2");
        let file=h.home.join(".claude/.hangar-bastao/cc2.md");assert_eq!(body["dossie"],json!(file));
        assert_eq!(std::fs::read_to_string(file).unwrap(),body["texto"].as_str().unwrap());
        assert_eq!(body["texto"],h.expected(false,"fixture"));assert_eq!(meta["cwd"],json!(h.root));assert_eq!(meta["model"],"opus");
        let users=std::fs::read_to_string(root.join("headless-users.jsonl")).unwrap().lines()
            .map(|line|serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
        assert_eq!(users.len(),1,"primeira entrega não pode duplicar: {users:?}");assert_eq!(users[0],delivered);
        assert_eq!(users[0]["message"]["content"],json!([{"type":"text","text":body["kickoff"]}]),"initial_prompt é a primeira mensagem da CLI em um único bloco de texto");
        let argv=std::fs::read_to_string(root.join("headless-argv")).unwrap();let args=argv.split('\0').collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair|pair==["--model","opus"]),"{args:?}");
        let context=std::fs::read_to_string(root.join("headless-context")).unwrap();
        assert_eq!(context,format!("{}\n{}\n{}\n{}\n",h.root.display(),h.home.join(".claude").display(),h.home.display(),h.home.display()));
        let queue=h.home.join(".claude/.hangar-queue");
        let projection=std::fs::read_to_string(queue.join("cc2.jsonl")).unwrap().lines().filter(|line|!line.trim().is_empty())
            .map(|line|serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
        assert_eq!(projection.len(),1,"projeção contém somente a entrada do lançador: {projection:?}");
        assert_eq!(projection[0]["delivered"],true);assert_eq!(projection[0]["pre_transcript"],true);
        assert_eq!(projection[0]["text"],body["kickoff"]);
        let raw=std::fs::read(queue.join("runtime").join(format!("{}.json",meta["key"].as_str().unwrap()))).unwrap();
        let (state,_)=hangar_server::runtime::queue::State::load(&raw).unwrap();
        assert_eq!(state.rows.len(),1,"uma única entrada no estado runtime");
        assert!(projection[0]["id"].as_str().is_some_and(|id|!id.is_empty()));
        assert_eq!(state.rows[0]["id"],projection[0]["id"],"projeção e runtime são a mesma entrada, não dois kickoffs");
        h.no_python();
    });
}

#[cfg(unix)]
#[test]
fn b01_effective_projects_directory_controls_saved_received_and_archived_dossiers() {
    if post_child("baton::b01_effective_projects_directory_controls_saved_received_and_archived_dossiers") {return;}
    hangar_server::accounts::quotas::disable_open_network_read();hangar_server::diag::disable_diary();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let root=PathBuf::from(std::env::var_os("HANGAR_BATON_CHILD_ROOT").unwrap());
        let projects=root.join("effective/projects");let directory=root.join("effective/.hangar-bastao");
        let settings=root.join("home/.hangar/config/.env");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings,format!("CP_PROJECTS_DIR={}\n",projects.display())).unwrap();
        let h=Origin::new(false,false).await;let c=client();
        let source=h.home.join(".claude");let target=h.home.join(".claude-target");
        std::fs::create_dir_all(target.join("projects")).unwrap();
        std::fs::write(target.join(".credentials.json"),r#"{"claudeAiOauth":{"accessToken":"fixture-only","subscriptionType":"pro"}}"#).unwrap();
        std::fs::write(target.join(".claude.json"),r#"{"numStartups":3}"#).unwrap();
        std::fs::write(target.join("settings.json"),r#"{"permissions":{"defaultMode":"bypassPermissions"}}"#).unwrap();
        std::fs::create_dir_all(source.join(".hangar-status")).unwrap();
        std::fs::write(source.join(format!(".hangar-status/{CLAUDE_ID}.json")),json!({"line":"🤖 Opus5·1M (high✦) │ 💵 $99"}).to_string()).unwrap();
        let missing=c.get(h.url("missing/bastao/dossie")).bearer_auth(OWNER).send().await.unwrap();
        let missing_status=missing.status();let created=directory.is_dir();
        let archived_id="00000000-0000-4000-8000-000000000102";
        let archived=projects.join(format!("-mnt-case-work-repo/{archived_id}.jsonl"));
        std::fs::create_dir_all(archived.parent().unwrap()).unwrap();std::fs::copy(&h.path,&archived).unwrap();
        let preview=c.get(format!("{}?project=-mnt-case-work-repo&session_id={archived_id}&provider=claude",h.url("custom-archive/bastao")))
            .bearer_auth(OWNER).send().await.unwrap();
        let preview_status=preview.status();let preview_body=preview.text().await.unwrap();
        let response=c.post(h.url("morta/bastao")).bearer_auth(OWNER).json(&json!({"name":"api.v2","provider":"claude","config_dir":target,
            "project":"-mnt-case-work-repo","session_id":CLAUDE_ID,"origem_config_dir":source})).send().await.unwrap();
        let status=response.status();let body=response.json::<Value>().await.unwrap();
        let received=c.get(h.url("api-v2/bastao/dossie")).bearer_auth(OWNER).send().await.unwrap();
        let received_status=received.status();let received_body=received.text().await.unwrap();
        h.fixture.runtime.shutdown().await.unwrap();
        assert_eq!(missing_status,404);assert!(created,"Path(settings.projects_dir).parent recebe a pasta antes do404");
        assert_eq!(preview_status,200,"Where.server_projects deve encontrar o arquivo exclusivo do override: {preview_body}");
        assert!(preview_body.contains(archived.to_str().unwrap()) && preview_body.contains("- Branch: `main`"));
        assert_eq!(status,200,"{body}");let file=directory.join("api-v2.md");assert_eq!(body["dossie"],json!(file));
        let expected=h.expected(false,"morta").replace("- Conta: `default`\n","- Conta: `default`\n- Modelo e esforço da origem: `Opus5·1M (high✦)`\n");
        assert_eq!(body["texto"],expected);assert_eq!(std::fs::read_to_string(&file).ok(),Some(expected.clone()));
        assert_eq!(received_status,200);assert_eq!(received_body,expected);
        assert!(body["kickoff"].as_str().unwrap().contains("conta `default` · modelo `Opus5·1M (high✦)`"));
        assert!(!source.join(".hangar-bastao").exists() && !target.join(".hangar-bastao").exists(),"contas não substituem a pasta efetiva do servidor");
        h.no_python();
    });
}

#[cfg(unix)]
#[test]
fn b01_rewrite_public_uses_explicit_origin_and_keeps_original_on_failure() {
    if post_child("baton::b01_rewrite_public_uses_explicit_origin_and_keeps_original_on_failure") {return;}
    hangar_server::accounts::quotas::disable_open_network_read();hangar_server::diag::disable_diary();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let h=Origin::new(false,false).await;let root=h.home.parent().unwrap();
        let target=h.home.join(".claude-target");let source=h.home.join(".claude-origin");
        for config in [&target,&source] {
            std::fs::create_dir_all(config.join("projects/-mnt-case-work-repo")).unwrap();
            std::fs::write(config.join(".credentials.json"),r#"{"claudeAiOauth":{"accessToken":"fixture-only","subscriptionType":"pro"}}"#).unwrap();
            std::fs::write(config.join(".claude.json"),r#"{"numStartups":3}"#).unwrap();
            std::fs::write(config.join("settings.json"),r#"{"permissions":{"defaultMode":"bypassPermissions"}}"#).unwrap();
        }
        let transcript=source.join(format!("projects/-mnt-case-work-repo/{CLAUDE_ID}.jsonl"));std::fs::copy(&h.path,&transcript).unwrap();
        std::fs::create_dir(source.join(".hangar-status")).unwrap();
        std::fs::write(source.join(format!(".hangar-status/{CLAUDE_ID}.json")),json!({"line":"🤖 Opus5·1M (high✦) │ 💵 $99"}).to_string()).unwrap();
        let original=h.expected(false,"morta").replace(h.path.to_str().unwrap(),transcript.to_str().unwrap())
            .replace("- Conta: `default`\n","- Conta: `origin`\n- Modelo e esforço da origem: `Opus5·1M (high✦)`\n");
        let c=client();
        for (mode,name,warning) in [("positive","rewrite-ok",None),("short","rewrite-short",Some("o modelo devolveu uma resposta curta demais")),("fail","rewrite-fail",Some("o modelo não conseguiu responder (cota ou erro do CLI)"))] {
            std::fs::write(root.join("rewrite-mode"),format!("{mode}\n")).unwrap();
            let response=c.post(h.url("morta/bastao")).bearer_auth(OWNER).json(&json!({"name":name,"provider":"claude","config_dir":target,
                "project":"-mnt-case-work-repo","session_id":CLAUDE_ID,"origem_config_dir":source,"resumo_por_modelo":true})).send().await.unwrap();
            let status=response.status();let body=response.json::<Value>().await.unwrap();assert_eq!(status,200,"{mode}: {body}");
            let expected=if mode=="positive" {format!("# Resumo reescrito pela fixture shell\n\n{}",original.trim_end())} else {original.clone()};
            assert_eq!(body["texto"],expected,"{mode}");assert_eq!(body["aviso"],json!(warning));
            assert_eq!(std::fs::read_to_string(root.join("rewrite-account")).unwrap(),format!("{}\n",source.display()),"a origem não é a conta padrão nem a sucessora");
            assert!(std::fs::read_to_string(root.join("rewrite-input")).unwrap().ends_with(&original));
            let argv=std::fs::read_to_string(root.join("rewrite-argv")).unwrap();let args=argv.split('\0').collect::<Vec<_>>();
            assert!(args.windows(2).any(|pair|pair==["--model","sonnet"]));assert!(!argv.contains(&original),"prompt somente no stdin");
            let path=Path::new(body["dossie"].as_str().unwrap());assert_eq!(std::fs::read_to_string(path).unwrap(),expected);
            assert!(body["kickoff"].as_str().unwrap().contains("conta `origin` · modelo `Opus5·1M (high✦)`"));
            let received=c.get(h.url(&format!("{name}/bastao/dossie"))).bearer_auth(OWNER).send().await.unwrap();assert_eq!(received.text().await.unwrap(),expected);
        }
        let codex=h.home.join(".codex-work");std::fs::create_dir_all(codex.join("sessions")).unwrap();
        std::fs::write(codex.join(".hangar-codex-conta"),r#"{"version":1,"id":"work"}"#).unwrap();
        std::fs::write(codex.join(format!("sessions/rollout-fixture-{CODEX_ID}.jsonl")),relocate_json_path(include_str!("../fixtures/baton/codex.jsonl"), "/tmp/proj", &h.root)).unwrap();
        let calls=std::fs::read(root.join("rewrite-calls")).unwrap();
        let response=c.post(h.url("morta/bastao")).bearer_auth(OWNER).json(&json!({"name":"no-claude","provider":"claude","config_dir":target,
            "project":"-","session_id":CODEX_ID,"origem_provider":"codex","resumo_por_modelo":true})).send().await.unwrap();
        let status=response.status();let body=response.json::<Value>().await.unwrap();assert_eq!(status,200,"{body}");
        assert_eq!(body["aviso"],"só dá pra usar o modelo quando a sessão de origem é Claude nesta máquina");
        assert_eq!(std::fs::read(root.join("rewrite-calls")).unwrap(),calls,"origem Codex não executa/cobra Claude");
        assert!(body["texto"].as_str().unwrap().starts_with("# Continuação da sessão `morta`"));
        h.fixture.runtime.shutdown().await.unwrap();h.no_python();
    });
}

#[cfg(unix)]
#[test]
fn b01_cancel_opening_keeps_ingress_until_the_detached_last_effect() {
    if post_child("baton::b01_cancel_opening_keeps_ingress_until_the_detached_last_effect") {return;}
    hangar_server::accounts::quotas::disable_open_network_read();
    hangar_server::diag::disable_diary();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        use std::{io::Write,sync::{Arc,Mutex},time::Duration};
        use notify::Watcher;
        use tower::ServiceExt;
        let h=Origin::new(false,false).await;
        let root=h.home.parent().unwrap();
        let fifo=root.join("open-release");
        let name=std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe {libc::mkfifo(name.as_ptr(),0o600)},0);
        struct Release(std::fs::File);
        impl Drop for Release {fn drop(&mut self){let _=self.0.write_all(b"go\n");}}
        let mut release=Release(std::fs::OpenOptions::new().read(true).write(true).open(&fifo).unwrap());
        std::fs::write(root.join("hold-opening"),"hold").unwrap();
        let (ready_tx,ready_rx)=tokio::sync::oneshot::channel();
        let (finished_tx,finished_rx)=tokio::sync::oneshot::channel();
        let ready=Arc::new(Mutex::new(Some(ready_tx)));let finished=Arc::new(Mutex::new(Some(finished_tx)));
        let signal=root.join("new-session-ready");
        let mut watcher=notify::recommended_watcher(move|event:notify::Result<notify::Event>| {
            if let Ok(event)=event {
                if event.paths.contains(&signal) {if let Some(tx)=ready.lock().unwrap().take(){let _=tx.send(());}}
                if event.paths.iter().any(|path|path.extension().is_some_and(|ext|ext=="json") && path.parent().is_some_and(|p|p.ends_with("session-specs"))) {
                    if let Some(tx)=finished.lock().unwrap().take(){let _=tx.send(());}
                }
            }
        }).unwrap();
        watcher.watch(root,notify::RecursiveMode::Recursive).unwrap();
        let mut state=hangar_server::routes::AppState::new(crate::fake::config(crate::refused_address(),"127.0.0.1"));
        state.accounts=hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::capture());
        state.origins_home=h.home.clone();state.list=h.fixture.list.clone();
        assert!(state.state.runtime.set(h.fixture.runtime.clone()).is_ok());
        let app=hangar_server::routes::router(Arc::new(state));
        let mut request=axum::http::Request::builder().method("POST").uri("/api/sessions/morta/bastao")
            .header("authorization",format!("Bearer {OWNER}")).header("content-type","application/json")
            .body(axum::body::Body::from(json!({"name":"cancel-cont","provider":"claude","project":"-mnt-case-work-repo",
                "session_id":CLAUDE_ID,"origem_config_dir":h.home.join(".claude")}).to_string())).unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo("127.0.0.1:32123".parse::<std::net::SocketAddr>().unwrap()));
        let handler=tokio::spawn(app.oneshot(request));
        let reached=tokio::time::timeout(Duration::from_secs(30),ready_rx).await.is_ok_and(|r|r.is_ok());
        let runtime=h.fixture.runtime.clone();
        let mut closing=Box::pin(runtime.ingress().hold("morta",Duration::from_secs(25)));
        let before=futures_util::poll!(closing.as_mut());
        handler.abort();
        let cancelled=handler.await.is_err_and(|e|e.is_cancelled());
        let after=if before.is_pending(){Some(futures_util::poll!(closing.as_mut()))}else{None};
        let released_early=before.is_ready() || after.as_ref().is_some_and(std::task::Poll::is_ready);
        release.0.write_all(b"go\n").unwrap();
        let completed=tokio::time::timeout(Duration::from_secs(25),finished_rx).await.is_ok_and(|r|r.is_ok());
        let closed=match before {
            std::task::Poll::Ready(result)=>result.is_ok(),
            std::task::Poll::Pending=>match after {
                Some(std::task::Poll::Ready(result))=>result.is_ok(),
                _=>closing.await.is_ok(),
            }
        };
        runtime.ingress().release("morta");
        runtime.shutdown().await.unwrap();
        assert!(reached,"a abertura deve alcançar o Mux antes do cancelamento");
        assert!(cancelled,"abortar a própria futura HTTP, não apenas desconectar um cliente");
        assert!(completed && root.join("new-session-done").exists(),"a abertura destacada deve terminar e publicar o spec antes da limpeza");
        assert!(closed);
        assert!(!released_early,"cancelar o HTTP liberou a entrada antes do último efeito da abertura destacada");
        h.no_python();
    });
}

#[tokio::test]
async fn b01_received_missing_directory_newlines_and_invalid_utf8_keep_original_contract() {
    let h=Origin::new(false,false).await;let directory=h.home.join(".claude/.hangar-bastao");
    let c=client();let origin="https://another-machine.example.invalid";
    let missing=c.get(h.url("missing/bastao/dossie")).bearer_auth(OWNER).header("origin",origin).send().await.unwrap();
    let missing_status=missing.status();let missing_body=missing.json::<Value>().await.unwrap();
    let created=directory.is_dir();
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("saved.md"),b"# recebido\r\nlinha\rfim\r\n").unwrap();
    let read=c.get(h.url("saved/bastao/dossie")).bearer_auth(OWNER).send().await.unwrap();
    let read_status=read.status();let read_body=read.text().await.unwrap();
    std::fs::write(directory.join("saved.md"),[0xff]).unwrap();
    let invalid=c.get(h.url("saved/bastao/dossie")).bearer_auth(OWNER).header("origin",origin).send().await.unwrap();
    let invalid_status=invalid.status();let invalid_headers=invalid.headers().clone();let invalid_body=invalid.text().await.unwrap();
    h.fixture.runtime.shutdown().await.unwrap();
    assert_eq!(missing_status,404);assert_eq!(missing_body,json!({"detail":{"code":"erro_bastao_sem_dossie","params":{},"msg":"no handover dossier for this session"}}));
    assert_eq!(read_status,200);assert_eq!(read_body,"# recebido\nlinha\nfim\n");
    assert_eq!(invalid_status,500);assert_eq!(invalid_headers["content-type"],"text/plain; charset=utf-8");assert_eq!(invalid_body,"Internal Server Error");
    assert_eq!([created,!invalid_headers.contains_key("access-control-allow-origin"),!invalid_headers.contains_key("access-control-expose-headers")],[true,true,true],"caminho() cria a pasta antes deexists/read e erro UTF8 fica fora do CORSMiddleware original");
    h.no_python();
}

#[tokio::test]
async fn b01_first_received_dossier_is_exactly_the_saved_file_not_a_rebuild() {
    let h = Origin::new(false, false).await;
    let directory = h.home.join(".claude/.hangar-bastao");
    std::fs::create_dir(&directory).unwrap();
    let text = "# Dossiê recebido\n\né **literal**\n";
    std::fs::write(directory.join("sucessora.md"), text).unwrap();
    let r = client().get(h.url("sucessora/bastao/dossie")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/markdown; charset=utf-8");
    assert_eq!(r.text().await.unwrap(), text);
    let r = client().get(h.url("ausente/bastao/dossie")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({"detail":{"code":"erro_bastao_sem_dossie","params":{},"msg":"no handover dossier for this session"}}));
    h.no_python();
}
