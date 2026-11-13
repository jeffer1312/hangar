//! Merge requests pelo `glab`: GraphQL para a lista e a branch (um pedido cada) e REST para o detalhe e as ações.
use super::cli::{self, Cli};
use super::model::{self, Activity, Approvals, Comment, Detail, Job, Merge, Provider, Summary, Thread, text as s};
use crate::Failure;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;

/// Campos de um MR para a lista e o chip.
const FRAGMENT: &str = r#"fragment mr on MergeRequest {
  iid title webUrl draft sourceBranch targetBranch conflicts detailedMergeStatus approved approvalsLeft
  resolvableDiscussionsCount resolvedDiscussionsCount headPipeline { status } project { fullPath } sourceProject { fullPath }
  author { username }
}"#;

/// Uma das listas da conta (`authoredMergeRequests`, `assignedMergeRequests`, `reviewRequestedMergeRequests`), uma página.
fn list_query(connection: &str) -> String {
    format!("query($after: String) {{ currentUser {{ page: {connection}(state: opened, first: 100, after: $after) {{
  pageInfo {{ hasNextPage endCursor }} nodes {{ ...mr }} }} }} }}")
}

/// Os MRs abertos da branch; o do fork certo sai do filtro pelo projeto de origem.
const BRANCH: &str = r#"query($repo: ID!, $branch: String!) {
  currentUser { username }
  project(fullPath: $repo) { mergeRequests(sourceBranches: [$branch], state: opened, first: 20) { nodes { ...mr } } }
}"#;

/// Um segmento de caminho da API: tudo fora do não reservado do RFC 3986 vai codificado (a `/` do projeto também).
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

fn encode(part: &str) -> String { utf8_percent_encode(part, SEGMENT).to_string() }

fn mr_path(repo: &str, iid: u64) -> String { format!("projects/{}/merge_requests/{iid}", encode(repo)) }

async fn api(host: &str, method: &str, path: &str, fields: &[(&str, &str)], timeout: std::time::Duration) -> Result<Value, Failure> {
    let mut args = vec!["api".into(), "--hostname".into(), host.to_owned()];
    if method != "GET" { args.push("-X".into()); args.push(method.into()); }
    args.push(path.to_owned());
    for (k, v) in fields { args.push("-f".into()); args.push(format!("{k}={v}")); }
    let out = cli::run(Cli::Glab, args, timeout).await?;
    if out.trim().is_empty() { return Ok(Value::Null); }
    serde_json::from_str(&out).map_err(|_| Failure::new("invalid_response", "resposta do glab ilegível"))
}

async fn graphql(host: &str, query: &str, vars: &[(&str, &str)]) -> Result<Value, Failure> {
    let mut args = vec!["api".into(), "--hostname".into(), host.to_owned(), "graphql".into(), "-f".into(), format!("query={query}\n{FRAGMENT}")];
    for (k, v) in vars { args.push("-f".into()); args.push(format!("{k}={v}")); }
    model::graphql_data(&cli::run(Cli::Glab, args, cli::READ).await?, "glab", false)
}

/// Todas as páginas de uma leitura REST que devolve lista.
async fn api_pages(host: &str, path: &str) -> Result<Vec<Value>, Failure> {
    let args = vec!["api".into(), "--hostname".into(), host.to_owned(), "--paginate".into(), path.to_owned()];
    model::pages(&cli::run(Cli::Glab, args, cli::READ).await?, "glab")
}

fn n(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
}


/// Aprovação no vocabulário do contrato, a partir do estado de merge e das aprovações que faltam.
fn approval_of(detailed: &str, left: Option<u64>, approved: bool) -> String {
    if detailed.eq_ignore_ascii_case("requested_changes") { "changes" }
    else if left.unwrap_or(0) > 0 { "required" }
    else if approved { "approved" }
    else { "none" }.into()
}

fn summary(host: &str, mr: &Value, user: &str) -> Option<Summary> {
    let resolvable = n(mr, "resolvableDiscussionsCount");
    let resolved = n(mr, "resolvedDiscussionsCount");
    let author = s(mr, &["author", "username"]).to_owned();
    let repo = s(mr, &["project", "fullPath"]).to_owned();
    let source_repo = Some(s(mr, &["sourceProject", "fullPath"])).filter(|r| !r.is_empty()).map_or_else(|| repo.clone(), str::to_owned);
    Some(Summary {
        provider: Provider::Gitlab,
        host: host.to_owned(),
        repo,
        number: n(mr, "iid")?,
        url: s(mr, &["webUrl"]).to_owned(),
        title: s(mr, &["title"]).to_owned(),
        mine: author.eq_ignore_ascii_case(user),
        author,
        review_requested: false,
        draft: mr.get("draft").and_then(Value::as_bool).unwrap_or(false),
        state: "open".into(),
        source: s(mr, &["sourceBranch"]).to_owned(),
        source_repo,
        target: s(mr, &["targetBranch"]).to_owned(),
        pipeline: model::pipeline_status(s(mr, &["headPipeline", "status"])),
        open_threads: resolvable.map(|r| r.saturating_sub(resolved.unwrap_or(0)) as u32),
        conflicts: mr.get("conflicts").and_then(Value::as_bool),
        sessions: Vec::new(),
    })
}

/// Os MRs abertos da conta (autor ou responsável) e os que pedem a revisão dela.
pub async fn list(host: &str, user: &str) -> Result<Vec<Summary>, Failure> {
    let (authored, assigned, review) = tokio::join!(
        connection(host, "authoredMergeRequests"),
        connection(host, "assignedMergeRequests"),
        connection(host, "reviewRequestedMergeRequests"),
    );
    let mut out: Vec<Summary> = Vec::new();
    for (nodes, assigned, review) in [(authored?, false, false), (assigned?, true, false), (review?, false, true)] {
        for mr in &nodes {
            let Some(mut item) = summary(host, mr, user) else { continue };
            item.review_requested = review;
            // Responsável conta como seu: no GitLab o MR aberto por um bot costuma ter o autor da branch como responsável.
            item.mine = item.mine || assigned;
            if let Some(found) = out.iter_mut().find(|o| o.url == item.url) {
                found.review_requested |= item.review_requested;
                found.mine |= item.mine;
            } else { out.push(item); }
        }
    }
    Ok(out)
}

/// Todas as páginas de uma lista da conta.
async fn connection(host: &str, name: &str) -> Result<Vec<Value>, Failure> {
    let query = list_query(name);
    let mut nodes = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let vars: Vec<(&str, &str)> = after.as_deref().map(|cursor| ("after", cursor)).into_iter().collect();
        let data = graphql(host, &query, &vars).await?;
        let page = data.pointer("/currentUser/page").filter(|p| !p.is_null())
            .ok_or_else(|| Failure::new("invalid_response", "lista de merge requests ilegível"))?;
        nodes.extend(page.get("nodes").and_then(Value::as_array).into_iter().flatten().cloned());
        match model::advance(Some(page), after.as_deref())? { Some(next) => after = Some(next), None => return Ok(nodes) }
    }
}

/// O MR aberto da branch no projeto `repo`, vindo de `source_repo` (o próprio `repo`, ou o fork).
pub async fn branch(host: &str, repo: &str, branch: &str, source_repo: &str) -> Result<Option<Summary>, Failure> {
    let data = graphql(host, BRANCH, &[("repo", repo), ("branch", branch)]).await?;
    let user = s(&data, &["currentUser", "username"]).to_owned();
    Ok(data.pointer("/project/mergeRequests/nodes").and_then(Value::as_array).into_iter().flatten()
        .filter_map(|mr| summary(host, mr, &user)).find(|mr| mr.source_repo.eq_ignore_ascii_case(source_repo)))
}

pub async fn detail(host: &str, repo: &str, iid: u64, viewer: &str) -> Result<Detail, Failure> {
    let base = mr_path(repo, iid);
    let paths = [format!("{base}?include_diverged_commits_count=true"), format!("{base}/approvals"),
        format!("{base}/discussions?per_page=100"), format!("{base}/diffs?per_page=100")];
    let (mr, approvals, discussions, diffs) = tokio::join!(
        api(host, "GET", &paths[0], &[], cli::READ),
        api(host, "GET", &paths[1], &[], cli::READ),
        api_pages(host, &paths[2]),
        api_pages(host, &paths[3]),
    );
    let mr = mr?;
    if mr.get("iid").is_none() { return Err(Failure::new("not_found", "merge request não encontrado")); }
    let (approvals, discussions, diffs) = (approvals?, discussions?, diffs?);
    let pipeline = mr.get("head_pipeline").filter(|p| !p.is_null());
    // MR de fork pode rodar o pipeline no projeto de origem: os jobs são pedidos ao projeto do pipeline, os status do commit
    // ao projeto onde o commit está.
    let project = |id: Option<u64>| id.map_or_else(|| encode(repo), |id| id.to_string());
    let jobs = jobs(host, pipeline.map(|p| (project(n(p, "project_id")), n(p, "id"))), &project(n(&mr, "source_project_id")), s(&mr, &["sha"])).await?;
    let files = model::files(diffs.iter().map(|d| {
        let raw = s(d, &["diff"]);
        let (additions, deletions) = count(raw);
        (s(d, &["new_path"]).to_owned(), additions, deletions, raw.to_owned())
    }));

    let mut threads = Vec::new();
    let mut activity = Vec::new();
    for discussion in &discussions {
        let notes = discussion.get("notes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let Some(first) = notes.first() else { continue };
        if first.get("resolvable").and_then(Value::as_bool).unwrap_or(false) {
            threads.push(thread(discussion, notes, &diffs));
            continue;
        }
        activity.extend(notes.iter().map(|note| Activity {
            kind: if note.get("system").and_then(Value::as_bool).unwrap_or(false) { "note" } else { "comment" }.into(),
            who: s(note, &["author", "username"]).to_owned(),
            text: model::clip(s(note, &["body"]).lines().next().unwrap_or(""), 200),
            at: s(note, &["created_at"]).to_owned(),
        }));
    }
    activity.sort_by(|a, b| b.at.cmp(&a.at));
    activity.truncate(30);

    let detailed = s(&mr, &["detailed_merge_status"]).to_ascii_lowercase();
    let state = match s(&mr, &["state"]) { "merged" => "merged", "closed" | "locked" => "closed", _ => "open" };
    let by: Vec<String> = approvals.get("approved_by").and_then(Value::as_array).into_iter().flatten()
        .map(|a| s(a, &["user", "username"]).to_owned()).filter(|u| !u.is_empty()).collect();
    let approved_by_me = approvals.get("user_has_approved").and_then(Value::as_bool).unwrap_or(false);
    let approvals = Approvals {
        state: approval_of(&detailed, n(&approvals, "approvals_left"), !by.is_empty()),
        need: n(&approvals, "approvals_required").filter(|r| *r > 0).map(|r| r as u32),
        by,
        requested: mr.get("reviewers").and_then(Value::as_array).into_iter().flatten().map(|r| s(r, &["username"]).to_owned()).collect(),
        can_approve: state == "open" && approvals.get("user_can_approve").and_then(Value::as_bool).unwrap_or(false) && !approved_by_me,
    };
    let author = s(&mr, &["author", "username"]).to_owned();
    let summary = Summary {
        provider: Provider::Gitlab,
        host: host.to_owned(),
        repo: repo.to_owned(),
        number: iid,
        url: s(&mr, &["web_url"]).to_owned(),
        title: s(&mr, &["title"]).to_owned(),
        mine: author.eq_ignore_ascii_case(viewer) || s(&mr, &["assignee", "username"]).eq_ignore_ascii_case(viewer),
        author,
        review_requested: approvals.requested.iter().any(|r| r.eq_ignore_ascii_case(viewer)),
        draft: mr.get("draft").and_then(Value::as_bool).unwrap_or(false),
        state: state.into(),
        source: s(&mr, &["source_branch"]).to_owned(),
        source_repo: source_project(host, repo, &mr).await?,
        target: s(&mr, &["target_branch"]).to_owned(),
        pipeline: pipeline.and_then(|p| model::pipeline_status(s(p, &["status"]))),
        open_threads: Some(threads.iter().filter(|t| !t.resolved).count() as u32),
        conflicts: mr.get("has_conflicts").and_then(Value::as_bool),
        sessions: Vec::new(),
    };
    let can_merge = mr.pointer("/user/can_merge").and_then(Value::as_bool).unwrap_or(false);
    let auto_enabled = mr.get("merge_when_pipeline_succeeds").and_then(Value::as_bool).unwrap_or(false)
        || mr.get("auto_merge_enabled").and_then(Value::as_bool).unwrap_or(false);
    let merge = merge_state(&detailed, &summary, can_merge, auto_enabled);
    Ok(Detail { summary, behind: n(&mr, "diverged_commits_count"), approvals, merge, jobs, threads, files, activity })
}

/// O caminho do projeto de origem: o REST só dá o id, e só o MR de fork pede a ida a mais. Fork que a conta não pode ler
/// (403, 404) fica com o projeto de destino: só as sessões da branch deixam de aparecer. Outra falha é falha do detalhe.
async fn source_project(host: &str, repo: &str, mr: &Value) -> Result<String, Failure> {
    let (Some(source), Some(target)) = (n(mr, "source_project_id"), n(mr, "project_id")) else { return Ok(repo.to_owned()) };
    if source == target { return Ok(repo.to_owned()); }
    match api(host, "GET", &format!("projects/{source}"), &[], cli::READ).await {
        Ok(project) => Ok(Some(s(&project, &["path_with_namespace"])).filter(|p| !p.is_empty()).unwrap_or(repo).to_owned()),
        Err(e) if e.message.contains("403") || e.message.contains("404") => {
            tracing::warn!(host = %host, source, "revisões: fork do MR sem leitura, sessões pelo projeto de destino");
            Ok(repo.to_owned())
        }
        Err(e) => Err(e),
    }
}

/// As etapas do pipeline. Pipeline externo (Jenkins e afins) não tem jobs no GitLab: as etapas são os status do commit,
/// pedidos junto para não somar uma ida a mais.
/// `pipeline` é o projeto (id ou caminho codificado) e o id do pipeline; `commit_project`, o projeto do commit.
async fn jobs(host: &str, pipeline: Option<(String, Option<u64>)>, commit_project: &str, sha: &str) -> Result<Vec<Job>, Failure> {
    let jobs_path = pipeline.and_then(|(project, id)| Some(format!("projects/{project}/pipelines/{}/jobs?per_page=100", id?)));
    let status_path = (!sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("projects/{commit_project}/repository/commits/{sha}/statuses?per_page=100"));
    // Pipeline num fork que a conta não lê (403, 404) fica sem etapas, como no próprio GitLab; o resto do detalhe vale.
    let read = |path: Option<String>| async move {
        let Some(path) = path else { return Ok(Vec::new()) };
        match api_pages(host, &path).await {
            Err(e) if e.message.contains("403") || e.message.contains("404") => {
                tracing::warn!(host = %host, "revisões: etapas do pipeline sem leitura");
                Ok(Vec::new())
            }
            read => read,
        }
    };
    let (jobs, statuses) = tokio::join!(read(jobs_path), read(status_path));
    let jobs = jobs?;
    if !jobs.is_empty() {
        return Ok(jobs.iter().rev().map(|j| Job { name: s(j, &["name"]).to_owned(), status: model::job_status(s(j, &["status"])),
            duration: j.get("duration").and_then(Value::as_f64).map(|d| d.round() as u64) }).collect());
    }
    Ok(statuses?.iter().map(|j| Job { name: s(j, &["name"]).to_owned(), status: model::job_status(s(j, &["status"])),
        duration: model::span(s(j, &["started_at"]), s(j, &["finished_at"])) }).collect())
}

/// Conta a partir do primeiro `@@`: antes dele só há cabeçalho, depois `---`/`+++` são linhas (um `-- comentário` removido).
fn count(diff: &str) -> (u32, u32) {
    diff.lines().skip_while(|l| !l.starts_with("@@")).fold((0, 0), |(a, d), l| {
        if l.starts_with('+') { (a + 1, d) } else if l.starts_with('-') { (a, d + 1) } else { (a, d) }
    })
}

/// As linhas do diff até a linha comentada (a nova; sem ela, a antiga), como o trecho que o GitHub manda.
pub fn excerpt(diff: &str, new_line: Option<u64>, old_line: Option<u64>) -> String {
    let (mut old, mut new) = (0u64, 0u64);
    let mut seen: Vec<&str> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            // `@@ -a,b +c,d @@`
            let mut parts = rest.split_whitespace();
            old = parts.next().and_then(|p| p.trim_start_matches('-').split(',').next()?.parse().ok()).unwrap_or(0);
            new = parts.next().and_then(|p| p.trim_start_matches('+').split(',').next()?.parse().ok()).unwrap_or(0);
            seen.clear();
            continue;
        }
        let (at_old, at_new) = (old, new);
        match line.chars().next() {
            Some('+') => new += 1,
            Some('-') => old += 1,
            _ => { old += 1; new += 1; }
        }
        seen.push(line);
        let hit = match (new_line, old_line) {
            (Some(n), _) => !line.starts_with('-') && at_new == n,
            (None, Some(o)) => !line.starts_with('+') && at_old == o,
            _ => false,
        };
        if hit { return seen[seen.len().saturating_sub(4)..].join("\n"); }
    }
    String::new()
}

fn thread(discussion: &Value, notes: &[Value], diffs: &[Value]) -> Thread {
    let position = notes[0].get("position").filter(|p| !p.is_null());
    let file = position.map(|p| { let new = s(p, &["new_path"]); if new.is_empty() { s(p, &["old_path"]) } else { new } }.to_owned()).filter(|f| !f.is_empty());
    let new_line = position.and_then(|p| n(p, "new_line"));
    let old_line = position.and_then(|p| n(p, "old_line"));
    let code = file.as_deref().and_then(|f| diffs.iter().find(|d| s(d, &["new_path"]) == f || s(d, &["old_path"]) == f))
        .map(|d| excerpt(s(d, &["diff"]), new_line, old_line)).unwrap_or_default();
    Thread {
        id: s(discussion, &["id"]).to_owned(),
        file,
        line: new_line.or(old_line),
        resolved: notes.iter().filter(|n| n.get("resolvable").and_then(Value::as_bool).unwrap_or(false))
            .all(|n| n.get("resolved").and_then(Value::as_bool).unwrap_or(false)),
        code,
        comments: notes.iter().filter(|n| !n.get("system").and_then(Value::as_bool).unwrap_or(false)).map(|n| Comment {
            author: s(n, &["author", "username"]).to_owned(),
            body: s(n, &["body"]).to_owned(),
            at: s(n, &["created_at"]).to_owned(),
            bot: n.pointer("/author/bot").and_then(Value::as_bool).unwrap_or(false) || model::is_bot(s(n, &["author", "username"])),
        }).collect(),
    }
}

/// O que impede mesclar, pelo `detailed_merge_status` do GitLab.
fn merge_state(detailed: &str, summary: &Summary, can_merge: bool, auto_enabled: bool) -> Merge {
    // Só pipeline que ainda roda é espera; cancelado, pulado ou ausente não vai passar sozinho, e o auto-merge ficaria preso.
    let pipeline_blocker = || if matches!(summary.pipeline.as_deref(), Some("running" | "pending")) { "pipeline_running" } else { "pipeline_failed" };
    let mut blockers: Vec<&'static str> = match detailed {
        "mergeable" => Vec::new(),
        "not_open" => vec!["not_open"],
        "not_approved" | "approvals_syncing" => vec!["approval_required"],
        "requested_changes" => vec!["changes_requested"],
        "ci_must_pass" => vec![pipeline_blocker()],
        "ci_still_running" => vec!["pipeline_running"],
        "discussions_not_resolved" => vec!["threads_open"],
        "conflict" | "broken_status" => vec!["conflict"],
        "need_rebase" => vec!["behind"],
        "draft_status" => vec!["draft"],
        "checking" | "unchecked" | "preparing" | "" => vec!["checking"],
        _ => vec!["blocked"],
    };
    if summary.state == "open" && !can_merge && !blockers.contains(&"not_open") { blockers.push("not_allowed"); }
    let waiting = |b: &&str| matches!(*b, "pipeline_running" | "checking");
    Merge {
        can_merge: blockers.is_empty(),
        can_auto: can_merge && !auto_enabled && !blockers.is_empty() && blockers.iter().all(waiting),
        auto_enabled,
        blockers,
    }
}

pub async fn reply(host: &str, repo: &str, iid: u64, thread: &str, body: &str) -> Result<(), Failure> {
    api(host, "POST", &format!("{}/discussions/{}/notes", mr_path(repo, iid), encode(thread)), &[("body", body)], cli::WRITE).await.map(drop)
}

pub async fn resolve(host: &str, repo: &str, iid: u64, thread: &str) -> Result<(), Failure> {
    api(host, "PUT", &format!("{}/discussions/{}?resolved=true", mr_path(repo, iid), encode(thread)), &[], cli::WRITE).await.map(drop)
}

pub async fn approve(host: &str, repo: &str, iid: u64) -> Result<(), Failure> {
    api(host, "POST", &format!("{}/approve", mr_path(repo, iid)), &[], cli::WRITE).await.map(drop)
}

pub async fn merge(host: &str, repo: &str, iid: u64, auto: bool) -> Result<(), Failure> {
    // `auto_merge` é o nome novo; versões antigas só conhecem `merge_when_pipeline_succeeds` e ignoram o outro.
    let query = if auto { "?auto_merge=true&merge_when_pipeline_succeeds=true" } else { "" };
    api(host, "PUT", &format!("{}/merge{query}", mr_path(repo, iid)), &[], cli::WRITE).await.map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn list_node_becomes_summary() {
        let mr = json!({"iid": "214", "title": "T", "webUrl": "https://gitlab.example.com/acme/api/-/merge_requests/214", "draft": false,
            "sourceBranch": "feat/x", "targetBranch": "main", "conflicts": false, "detailedMergeStatus": "NOT_APPROVED",
            "approved": false, "approvalsLeft": 1, "resolvableDiscussionsCount": 3, "resolvedDiscussionsCount": 1,
            "headPipeline": {"status": "SUCCESS"}, "project": {"fullPath": "acme/api"}, "author": {"username": "ana"}});
        let item = summary("gitlab.example.com", &mr, "ana").unwrap();
        assert_eq!((item.number, item.repo.as_str()), (214, "acme/api"));
        assert_eq!(item.open_threads, Some(2));
        assert_eq!(item.pipeline.as_deref(), Some("success"));
        assert!(item.mine);
    }

    #[test]
    fn project_path_is_one_encoded_segment() {
        assert_eq!(encode("grupo/sub projeto.x"), "grupo%2Fsub%20projeto.x");
        assert_eq!(mr_path("a/b", 3), "projects/a%2Fb/merge_requests/3");
    }

    #[test]
    fn excerpt_ends_at_the_commented_line() {
        let diff = "@@ -10,4 +10,5 @@ fn a()\n x\n-y\n+z\n+w\n v\n";
        assert_eq!(excerpt(diff, Some(12), None), " x\n-y\n+z\n+w");
        assert_eq!(excerpt(diff, None, Some(11)), " x\n-y");
        assert_eq!(excerpt(diff, Some(99), None), "");
    }

    fn open() -> Summary {
        summary("h", &json!({"iid": 1, "project": {"fullPath": "a/b"}, "headPipeline": {"status": "RUNNING"}}), "u").unwrap()
    }

    #[test]
    fn merge_status_maps_to_blockers() {
        let m = merge_state("ci_must_pass", &open(), true, false);
        assert_eq!(m.blockers, vec!["pipeline_running"]);
        assert!(m.can_auto && !m.can_merge);
        let m = merge_state("mergeable", &open(), true, false);
        assert!(m.can_merge && m.blockers.is_empty());
        let m = merge_state("mergeable", &open(), false, false);
        assert_eq!(m.blockers, vec!["not_allowed"]);
        let m = merge_state("discussions_not_resolved", &open(), true, false);
        assert!(!m.can_auto);
        let canceled = summary("h", &json!({"iid": 1, "project": {"fullPath": "a/b"}, "headPipeline": {"status": "CANCELED"}}), "u").unwrap();
        let m = merge_state("ci_must_pass", &canceled, true, false);
        assert!(m.blockers == vec!["pipeline_failed"] && !m.can_auto);
        let m = merge_state("ci_still_running", &open(), true, true);
        assert!(!m.can_auto && m.auto_enabled);
    }

    #[test]
    fn diff_counts_ignore_headers() {
        assert_eq!(count("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n+c\n"), (2, 1));
        assert_eq!(count("@@ -1,2 +1,1 @@\n--- nota\n-- outra\n+++i;\n"), (1, 2));
    }
}
