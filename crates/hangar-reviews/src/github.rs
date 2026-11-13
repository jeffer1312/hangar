//! Pull requests pelo `gh`: GraphQL para lista, branch, detalhe e threads; REST para os arquivos; o próprio CLI para
//! aprovar e mesclar.
use super::cli::{self, Cli};
use super::model::{self, Activity, Approvals, Comment, Detail, Job, Merge, Provider, Summary, Thread, text as s};
use crate::Failure;
use serde_json::Value;

/// Campos de um PR para a lista e o chip.
const FRAGMENT: &str = r#"fragment pr on PullRequest {
  number title url isDraft headRefName baseRefName mergeable reviewDecision state
  author { login } repository { nameWithOwner } headRepository { nameWithOwner }
  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
  reviewThreads(first: 100) { pageInfo { hasNextPage } nodes { isResolved } }
}"#;

const THREAD: &str = r#"fragment thread on PullRequestReviewThread { id isResolved path line originalLine
  comments(first: 50) { nodes { author { login __typename } body diffHunk createdAt } } }"#;

const SEARCH: &str = r#"query($q: String!, $after: String) {
  search(query: $q, type: ISSUE, first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { ...pr } }
}"#;

/// Os PRs abertos da branch; o do fork certo sai do filtro pelo repositório de origem.
const BRANCH: &str = r#"query($owner: String!, $name: String!, $branch: String!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    pullRequests(headRefName: $branch, states: [OPEN], first: 20, orderBy: {field: UPDATED_AT, direction: DESC}) { nodes { ...pr } }
  }
}"#;

const THREADS: &str = r#"query($owner: String!, $name: String!, $number: Int!, $after: String!) {
  repository(owner: $owner, name: $name) { pullRequest(number: $number) {
    reviewThreads(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { ...thread } }
  } }
}"#;

const DETAIL: &str = r#"query($owner: String!, $name: String!, $number: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    autoMergeAllowed viewerPermission
    pullRequest(number: $number) {
      ...pr mergeStateStatus viewerDidAuthor
      autoMergeRequest { enabledAt }
      reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } ... on Team { name } } } }
      latestOpinionatedReviews(first: 30) { nodes { author { login } state } }
      checks: commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 80) { nodes {
        __typename
        ... on CheckRun { name status conclusion startedAt completedAt }
        ... on StatusContext { context state }
      } } } } } }
      threads: reviewThreads(first: 100) { pageInfo { hasNextPage endCursor } nodes { ...thread } }
      timelineItems(last: 30, itemTypes: [PULL_REQUEST_COMMIT, PULL_REQUEST_REVIEW, ISSUE_COMMENT, MERGED_EVENT,
          REVIEW_REQUESTED_EVENT, READY_FOR_REVIEW_EVENT, HEAD_REF_FORCE_PUSHED_EVENT, CLOSED_EVENT, REOPENED_EVENT]) {
        nodes { __typename
          ... on PullRequestCommit { commit { messageHeadline committedDate author { user { login } name } } }
          ... on PullRequestReview { author { login } state submittedAt }
          ... on IssueComment { author { login } bodyText createdAt }
          ... on MergedEvent { actor { login } createdAt }
          ... on ReviewRequestedEvent { actor { login } createdAt requestedReviewer { ... on User { login } ... on Team { name } } }
          ... on ReadyForReviewEvent { actor { login } createdAt }
          ... on HeadRefForcePushedEvent { actor { login } createdAt }
          ... on ClosedEvent { actor { login } createdAt }
          ... on ReopenedEvent { actor { login } createdAt }
        } }
    }
  }
}"#;

const REPLY: &str = r#"mutation($thread: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { comment { id } }
}"#;
const RESOLVE: &str = r#"mutation($thread: ID!) { resolveReviewThread(input: {threadId: $thread}) { thread { isResolved } } }"#;
const METHODS: &str = r#"query($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) { mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed }
}"#;

/// Variável do GraphQL: `-f` manda texto, `-F` converte número.
enum Var<'a> { Text(&'a str, &'a str), Int(&'a str, u64) }

async fn graphql(host: &str, query: &str, vars: &[Var<'_>], timeout: std::time::Duration) -> Result<Value, Failure> {
    // Fragmento declarado e não usado é erro no GraphQL: só vai o que a consulta usa.
    let mut query = query.to_owned();
    if query.contains("...pr") { query = format!("{query}\n{FRAGMENT}"); }
    if query.contains("...thread") { query = format!("{query}\n{THREAD}"); }
    let mut args = vec!["api".into(), "--hostname".into(), host.to_owned(), "graphql".into(), "-f".into(), format!("query={query}")];
    for var in vars {
        match var {
            Var::Text(k, v) => { args.push("-f".into()); args.push(format!("{k}={v}")); }
            Var::Int(k, v) => { args.push("-F".into()); args.push(format!("{k}={v}")); }
        }
    }
    model::graphql_data(&cli::run(Cli::Gh, args, timeout).await?, "gh")
}

fn owner_name(repo: &str) -> Result<(&str, &str), Failure> {
    repo.split_once('/').filter(|(o, n)| !o.is_empty() && !n.is_empty() && !n.contains('/'))
        .ok_or_else(|| Failure::new("bad_repo", "repositório do GitHub é dono/nome"))
}

fn approval_of(decision: &str) -> String {
    match decision { "APPROVED" => "approved", "CHANGES_REQUESTED" => "changes", "REVIEW_REQUIRED" => "required", _ => "none" }.into()
}

fn state_of(raw: &str) -> String {
    match raw { "MERGED" => "merged", "CLOSED" => "closed", _ => "open" }.into()
}

fn summary(host: &str, pr: &Value, user: &str) -> Option<Summary> {
    let number = pr.get("number")?.as_u64()?;
    // Com mais de uma página de threads a contagem da lista não fecha: fica desconhecida, e o detalhe conta todas.
    let threads = pr.pointer("/reviewThreads/nodes").and_then(Value::as_array)
        .filter(|_| pr.pointer("/reviewThreads/pageInfo/hasNextPage").and_then(Value::as_bool) != Some(true));
    let author = s(pr, &["author", "login"]).to_owned();
    let repo = s(pr, &["repository", "nameWithOwner"]).to_owned();
    let source_repo = Some(s(pr, &["headRepository", "nameWithOwner"])).filter(|r| !r.is_empty()).map_or_else(|| repo.clone(), str::to_owned);
    Some(Summary {
        provider: Provider::Github,
        host: host.to_owned(),
        repo,
        number,
        url: s(pr, &["url"]).to_owned(),
        title: s(pr, &["title"]).to_owned(),
        mine: author.eq_ignore_ascii_case(user),
        author,
        review_requested: false,
        draft: pr.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        state: state_of(s(pr, &["state"])),
        source: s(pr, &["headRefName"]).to_owned(),
        source_repo,
        target: s(pr, &["baseRefName"]).to_owned(),
        // O resumo `PENDING` quer dizer que algum check ainda roda; "na fila" fica para o check isolado.
        pipeline: match pr.pointer("/commits/nodes/0/commit/statusCheckRollup/state").and_then(Value::as_str) {
            Some("PENDING" | "EXPECTED") => Some("running".into()),
            Some(state) => model::pipeline_status(state),
            None => None,
        },
        open_threads: threads.map(|t| t.iter().filter(|t| !t.get("isResolved").and_then(Value::as_bool).unwrap_or(true)).count() as u32),
        conflicts: match s(pr, &["mergeable"]) { "CONFLICTING" => Some(true), "MERGEABLE" => Some(false), _ => None },
        sessions: Vec::new(),
    })
}

/// Os PRs abertos da conta e os que pedem a revisão dela.
pub async fn list(host: &str, user: &str) -> Result<Vec<Summary>, Failure> {
    let (mine, review) = tokio::join!(
        search(host, "is:pr is:open author:@me archived:false"),
        search(host, "is:pr is:open review-requested:@me archived:false"),
    );
    let mut out: Vec<Summary> = Vec::new();
    for (nodes, review) in [(mine?, false), (review?, true)] {
        for pr in &nodes {
            let Some(mut item) = summary(host, pr, user) else { continue };
            item.review_requested = review;
            if let Some(found) = out.iter_mut().find(|o| o.url == item.url) { found.review_requested |= review; }
            else { out.push(item); }
        }
    }
    Ok(out)
}

/// Todas as páginas de uma busca de PRs.
async fn search(host: &str, q: &str) -> Result<Vec<Value>, Failure> {
    let mut nodes = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut vars = vec![Var::Text("q", q)];
        if let Some(cursor) = &after { vars.push(Var::Text("after", cursor)); }
        let mut data = graphql(host, SEARCH, &vars, cli::READ).await?;
        let mut page = data.get_mut("search").map(Value::take).filter(|p| !p.is_null())
            .ok_or_else(|| Failure::new("invalid_response", "busca de pull requests ilegível"))?;
        after = model::next_page(Some(&page));
        if let Some(Value::Array(found)) = page.get_mut("nodes").map(Value::take) { nodes.extend(found); }
        if after.is_none() { return Ok(nodes); }
    }
}

/// O PR aberto da branch no repositório `repo`, vindo de `source_repo` (o próprio `repo`, ou o fork).
pub async fn branch(host: &str, repo: &str, branch: &str, source_repo: &str) -> Result<Option<Summary>, Failure> {
    let (owner, name) = owner_name(repo)?;
    let data = graphql(host, BRANCH, &[Var::Text("owner", owner), Var::Text("name", name), Var::Text("branch", branch)], cli::READ).await?;
    let viewer = s(&data, &["viewer", "login"]).to_owned();
    Ok(data.pointer("/repository/pullRequests/nodes").and_then(Value::as_array).into_iter().flatten()
        .filter_map(|pr| summary(host, pr, &viewer)).find(|pr| pr.source_repo.eq_ignore_ascii_case(source_repo)))
}

/// As threads depois da primeira página, que veio junto do detalhe.
async fn more_threads(host: &str, owner: &str, name: &str, number: u64, mut after: String) -> Result<Vec<Value>, Failure> {
    let mut nodes = Vec::new();
    loop {
        let data = graphql(host, THREADS, &[Var::Text("owner", owner), Var::Text("name", name), Var::Int("number", number),
            Var::Text("after", &after)], cli::READ).await?;
        let page = data.pointer("/repository/pullRequest/reviewThreads").filter(|p| !p.is_null())
            .ok_or_else(|| Failure::new("invalid_response", "threads do pull request ilegíveis"))?;
        nodes.extend(page.get("nodes").and_then(Value::as_array).into_iter().flatten().cloned());
        let page = Some(page);
        match model::next_page(page) { Some(next) => after = next, None => return Ok(nodes) }
    }
}

pub async fn detail(host: &str, repo: &str, number: u64) -> Result<Detail, Failure> {
    let (owner, name) = owner_name(repo)?;
    let vars = [Var::Text("owner", owner), Var::Text("name", name), Var::Int("number", number)];
    let files_args = vec!["api".into(), "--hostname".into(), host.to_owned(), "--paginate".into(),
        format!("repos/{owner}/{name}/pulls/{number}/files?per_page=100")];
    let (data, files) = tokio::join!(graphql(host, DETAIL, &vars, cli::READ), cli::run(Cli::Gh, files_args, cli::READ));
    let mut data = data?;
    let viewer = s(&data, &["viewer", "login"]).to_owned();
    let repository = data.get_mut("repository").map(Value::take).unwrap_or(Value::Null);
    let pr = repository.get("pullRequest").filter(|v| !v.is_null())
        .ok_or_else(|| Failure::new("not_found", "pull request não encontrado"))?;
    let mut summary = summary(host, pr, &viewer).ok_or_else(|| Failure::new("invalid_response", "pull request ilegível"))?;
    summary.repo = repo.to_owned();

    let reviews = pr.pointer("/latestOpinionatedReviews/nodes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let requested: Vec<String> = pr.pointer("/reviewRequests/nodes").and_then(Value::as_array).into_iter().flatten()
        .map(|r| { let who = s(r, &["requestedReviewer", "login"]); if who.is_empty() { s(r, &["requestedReviewer", "name"]) } else { who } })
        .filter(|w| !w.is_empty()).map(str::to_owned).collect();
    summary.review_requested = requested.iter().any(|r| r.eq_ignore_ascii_case(&viewer));
    let approvals = Approvals {
        state: approval_of(s(pr, &["reviewDecision"])),
        need: None,
        by: reviews.iter().filter(|r| s(r, &["state"]) == "APPROVED").map(|r| s(r, &["author", "login"]).to_owned()).collect(),
        requested,
        can_approve: summary.state == "open" && !pr.get("viewerDidAuthor").and_then(Value::as_bool).unwrap_or(false),
    };

    let jobs = pr.pointer("/checks/nodes/0/commit/statusCheckRollup/contexts/nodes").and_then(Value::as_array).into_iter().flatten()
        .filter_map(job).collect();
    let mut threads: Vec<Thread> = pr.pointer("/threads/nodes").and_then(Value::as_array).into_iter().flatten().map(thread).collect();
    if let Some(after) = model::next_page(pr.get("threads")) {
        threads.extend(more_threads(host, owner, name, number, after).await?.iter().map(thread));
    }
    summary.open_threads = Some(threads.iter().filter(|t| !t.resolved).count() as u32);
    let activity = pr.pointer("/timelineItems/nodes").and_then(Value::as_array).into_iter().flatten().rev().filter_map(activity).collect();
    let files = model::pages(&files?, "gh")?;
    let files = model::files(files.iter().map(|f| (s(f, &["filename"]).to_owned(), num(f, "additions"), num(f, "deletions"), s(f, &["patch"]).to_owned())));

    let can_write = matches!(s(&repository, &["viewerPermission"]), "ADMIN" | "MAINTAIN" | "WRITE");
    let merge = merge_state(pr, &summary, &approvals, can_write, repository.get("autoMergeAllowed").and_then(Value::as_bool).unwrap_or(false));
    Ok(Detail { summary, behind: None, approvals, merge, jobs, threads, files, activity })
}

fn num(v: &Value, key: &str) -> u32 { v.get(key).and_then(Value::as_u64).unwrap_or(0) as u32 }

fn job(node: &Value) -> Option<Job> {
    match s(node, &["__typename"]) {
        "CheckRun" => {
            let status = if s(node, &["status"]) == "COMPLETED" { s(node, &["conclusion"]) } else { s(node, &["status"]) };
            Some(Job { name: s(node, &["name"]).to_owned(), status: model::job_status(status),
                duration: model::span(s(node, &["startedAt"]), s(node, &["completedAt"])) })
        }
        "StatusContext" => Some(Job { name: s(node, &["context"]).to_owned(), status: model::job_status(s(node, &["state"])), duration: None }),
        _ => None,
    }
}

/// As últimas linhas do trecho de diff da thread: as que levam até a linha comentada.
fn hunk_tail(hunk: &str) -> String {
    let lines: Vec<&str> = hunk.lines().filter(|l| !l.starts_with("@@")).collect();
    lines[lines.len().saturating_sub(4)..].join("\n")
}

fn thread(node: &Value) -> Thread {
    let comments: Vec<Comment> = node.pointer("/comments/nodes").and_then(Value::as_array).into_iter().flatten().map(|c| Comment {
        author: s(c, &["author", "login"]).to_owned(),
        body: s(c, &["body"]).to_owned(),
        at: s(c, &["createdAt"]).to_owned(),
        bot: s(c, &["author", "__typename"]) == "Bot" || model::is_bot(s(c, &["author", "login"])),
    }).collect();
    Thread {
        id: s(node, &["id"]).to_owned(),
        file: Some(s(node, &["path"]).to_owned()).filter(|p| !p.is_empty()),
        line: node.get("line").and_then(Value::as_u64).or_else(|| node.get("originalLine").and_then(Value::as_u64)),
        resolved: node.get("isResolved").and_then(Value::as_bool).unwrap_or(false),
        code: node.pointer("/comments/nodes/0/diffHunk").and_then(Value::as_str).map(hunk_tail).unwrap_or_default(),
        comments,
    }
}

fn activity(node: &Value) -> Option<Activity> {
    let actor = |n: &Value| s(n, &["actor", "login"]).to_owned();
    let (kind, who, text, at) = match s(node, &["__typename"]) {
        "PullRequestCommit" => {
            let who = Some(s(node, &["commit", "author", "user", "login"])).filter(|w| !w.is_empty()).unwrap_or(s(node, &["commit", "author", "name"]));
            ("commit", who.to_owned(), s(node, &["commit", "messageHeadline"]).to_owned(), s(node, &["commit", "committedDate"]))
        }
        "PullRequestReview" => {
            let kind = match s(node, &["state"]) { "APPROVED" => "approved", "CHANGES_REQUESTED" => "changes_requested", _ => "reviewed" };
            (kind, s(node, &["author", "login"]).to_owned(), String::new(), s(node, &["submittedAt"]))
        }
        "IssueComment" => ("comment", s(node, &["author", "login"]).to_owned(), model::clip(s(node, &["bodyText"]), 200), s(node, &["createdAt"])),
        "MergedEvent" => ("merged", actor(node), String::new(), s(node, &["createdAt"])),
        "ReviewRequestedEvent" => {
            let who = Some(s(node, &["requestedReviewer", "login"])).filter(|w| !w.is_empty()).unwrap_or(s(node, &["requestedReviewer", "name"]));
            ("review_requested", actor(node), who.to_owned(), s(node, &["createdAt"]))
        }
        "ReadyForReviewEvent" => ("ready", actor(node), String::new(), s(node, &["createdAt"])),
        "HeadRefForcePushedEvent" => ("force_pushed", actor(node), String::new(), s(node, &["createdAt"])),
        "ClosedEvent" => ("closed", actor(node), String::new(), s(node, &["createdAt"])),
        "ReopenedEvent" => ("reopened", actor(node), String::new(), s(node, &["createdAt"])),
        _ => return None,
    };
    Some(Activity { kind: kind.into(), who, text, at: at.to_owned() })
}

/// O que impede mesclar, pelo `mergeStateStatus` e pelo que ele resume.
fn merge_state(pr: &Value, summary: &Summary, approvals: &Approvals, can_write: bool, auto_allowed: bool) -> Merge {
    let mut blockers: Vec<&'static str> = Vec::new();
    if summary.state != "open" { blockers.push("not_open"); }
    else {
        if summary.draft { blockers.push("draft"); }
        if summary.conflicts == Some(true) { blockers.push("conflict"); }
        match summary.pipeline.as_deref() {
            Some("failed") => blockers.push("pipeline_failed"),
            Some("running" | "pending") => blockers.push("pipeline_running"),
            _ => {}
        }
        match approvals.state.as_str() {
            "changes" => blockers.push("changes_requested"),
            "required" => blockers.push("approval_required"),
            _ => {}
        }
        match s(pr, &["mergeStateStatus"]) {
            "BEHIND" => blockers.push("behind"),
            "UNKNOWN" if blockers.is_empty() => blockers.push("checking"),
            // Bloqueado sem motivo visível acima: regra do repositório (conversas, assinatura, checks exigidos).
            "BLOCKED" if blockers.is_empty() => blockers.push(if summary.open_threads.unwrap_or(0) > 0 { "threads_open" } else { "blocked" }),
            // Pipeline que falha em check não exigido não bloqueia no GitHub.
            "UNSTABLE" => blockers.retain(|b| *b != "pipeline_failed"),
            _ => {}
        }
        if !can_write { blockers.push("not_allowed"); }
    }
    let waiting = |b: &&str| matches!(*b, "pipeline_running" | "approval_required" | "checking");
    Merge {
        can_merge: blockers.is_empty(),
        can_auto: auto_allowed && can_write && !blockers.is_empty() && blockers.iter().all(waiting),
        auto_enabled: pr.get("autoMergeRequest").is_some_and(|v| !v.is_null()),
        blockers,
    }
}

pub async fn reply(host: &str, thread: &str, body: &str) -> Result<(), Failure> {
    graphql(host, REPLY, &[Var::Text("thread", thread), Var::Text("body", body)], cli::WRITE).await.map(drop)
}

pub async fn resolve(host: &str, thread: &str) -> Result<(), Failure> {
    graphql(host, RESOLVE, &[Var::Text("thread", thread)], cli::WRITE).await.map(drop)
}

fn repo_arg(host: &str, repo: &str) -> String { format!("{host}/{repo}") }

pub async fn approve(host: &str, repo: &str, number: u64) -> Result<(), Failure> {
    let args = vec!["pr".into(), "review".into(), number.to_string(), "--approve".into(), "--repo".into(), repo_arg(host, repo)];
    cli::run(Cli::Gh, args, cli::WRITE).await.map(drop)
}

/// Mescla com o primeiro método que o repositório aceita, na ordem merge, squash, rebase.
pub async fn merge(host: &str, repo: &str, number: u64, auto: bool) -> Result<(), Failure> {
    let (owner, name) = owner_name(repo)?;
    let data = graphql(host, METHODS, &[Var::Text("owner", owner), Var::Text("name", name)], cli::READ).await?;
    let allowed = |k: &str| data.pointer(&format!("/repository/{k}")).and_then(Value::as_bool).unwrap_or(false);
    let method = [("mergeCommitAllowed", "--merge"), ("squashMergeAllowed", "--squash"), ("rebaseMergeAllowed", "--rebase")]
        .into_iter().find(|(k, _)| allowed(k)).map(|(_, flag)| flag)
        .ok_or_else(|| Failure::new("merge_not_allowed", "o repositório não aceita nenhum método de merge"))?;
    let mut args = vec!["pr".into(), "merge".into(), number.to_string(), method.into(), "--repo".into(), repo_arg(host, repo)];
    if auto { args.push("--auto".into()); }
    cli::run(Cli::Gh, args, cli::WRITE).await.map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr() -> Value {
        json!({"number": 7, "title": "T", "url": "https://github.com/a/b/pull/7", "isDraft": false, "state": "OPEN",
            "headRefName": "feat/x", "baseRefName": "main", "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED",
            "mergeStateStatus": "BLOCKED", "author": {"login": "Ana"}, "repository": {"nameWithOwner": "a/b"},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "PENDING"}}}]},
            "reviewThreads": {"nodes": [{"isResolved": false}, {"isResolved": true}]}})
    }

    #[test]
    fn summary_reads_state_threads_and_author() {
        let item = summary("github.com", &pr(), "ana").unwrap();
        assert!(item.mine);
        assert_eq!(item.pipeline.as_deref(), Some("running"));
        assert_eq!(item.open_threads, Some(1));
        assert_eq!(item.conflicts, Some(false));
        assert_eq!((item.source.as_str(), item.target.as_str()), ("feat/x", "main"));
    }

    #[test]
    fn summary_knows_the_fork_and_when_the_thread_count_is_partial() {
        let mut p = pr();
        assert_eq!(summary("github.com", &p, "ana").unwrap().source_repo, "a/b");
        p["headRepository"] = json!({"nameWithOwner": "ana/b"});
        p["reviewThreads"]["pageInfo"] = json!({"hasNextPage": true});
        let item = summary("github.com", &p, "ana").unwrap();
        assert_eq!((item.repo.as_str(), item.source_repo.as_str()), ("a/b", "ana/b"));
        assert_eq!(item.open_threads, None);
    }

    #[test]
    fn merge_waits_on_pipeline_and_review_and_allows_auto() {
        let p = pr();
        let item = summary("github.com", &p, "ana").unwrap();
        let approvals = Approvals { state: "required".into(), ..Default::default() };
        let m = merge_state(&p, &item, &approvals, true, true);
        assert_eq!(m.blockers, vec!["pipeline_running", "approval_required"]);
        assert!(!m.can_merge && m.can_auto);
        let m = merge_state(&p, &item, &approvals, false, true);
        assert!(m.blockers.contains(&"not_allowed") && !m.can_auto);
    }

    #[test]
    fn conflict_blocks_auto_merge() {
        let mut p = pr();
        p["mergeable"] = json!("CONFLICTING");
        let item = summary("github.com", &p, "ana").unwrap();
        let m = merge_state(&p, &item, &Approvals { state: "approved".into(), ..Default::default() }, true, true);
        assert!(m.blockers.contains(&"conflict") && !m.can_auto);
    }

    #[test]
    fn thread_keeps_the_lines_before_the_comment() {
        let node = json!({"id": "T1", "isResolved": false, "path": "src/a.rs", "line": 12,
            "comments": {"nodes": [{"author": {"login": "coderabbitai", "__typename": "Bot"}, "body": "B", "createdAt": "x",
                "diffHunk": "@@ -1,6 +1,6 @@\n a\n b\n c\n-d\n+e\n f"}]}});
        let t = thread(&node);
        assert_eq!(t.code, " c\n-d\n+e\n f");
        assert!(t.comments[0].bot);
        assert_eq!((t.file.as_deref(), t.line), (Some("src/a.rs"), Some(12)));
    }
}
