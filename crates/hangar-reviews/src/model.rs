//! Formato único de PR e MR para os clientes, e os ajudantes que os dois provedores dividem.
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider { Github, Gitlab }

/// O que as listas e o chip mostram de um PR ou MR.
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub provider: Provider,
    pub host: String,
    /// Caminho do repositório no host (`dono/nome`, `grupo/sub/projeto`).
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub title: String,
    pub author: String,
    pub mine: bool,
    pub review_requested: bool,
    pub draft: bool,
    /// `open`, `merged` ou `closed`.
    pub state: String,
    pub source: String,
    /// Repositório de onde vem a branch: o fork, quando a revisão é de um fork; sem como saber, o próprio `repo`.
    pub source_repo: String,
    pub target: String,
    /// `success`, `failed`, `running`, `pending`, `canceled`, `skipped`; sem pipeline, ausente.
    pub pipeline: Option<String>,
    pub open_threads: Option<u32>,
    pub conflicts: Option<bool>,
    /// Sessões abertas na branch de origem deste repositório.
    pub sessions: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Approvals {
    /// `approved`, `required`, `changes` ou `none`.
    pub state: String,
    /// Quantas o repositório exige, quando o provedor diz.
    pub need: Option<u32>,
    pub by: Vec<String>,
    pub requested: Vec<String>,
    pub can_approve: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Merge {
    pub can_merge: bool,
    /// Dá para pedir que mescle sozinho quando o pipeline passar.
    pub can_auto: bool,
    pub auto_enabled: bool,
    /// Códigos estáveis do que impede mesclar agora (a tela traduz).
    pub blockers: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Job { pub name: String, pub status: String, pub duration: Option<u64> }

#[derive(Clone, Debug, Serialize)]
pub struct Comment { pub author: String, pub body: String, pub at: String, pub bot: bool }

#[derive(Clone, Debug, Serialize)]
pub struct Thread {
    pub id: String,
    pub file: Option<String>,
    pub line: Option<u64>,
    pub resolved: bool,
    /// Trecho do diff que a thread comenta, quando o provedor dá.
    pub code: String,
    pub comments: Vec<Comment>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileChange { pub path: String, pub additions: u32, pub deletions: u32, pub patch: String }

/// Um acontecimento do PR ou MR. `kind` é estável e a tela traduz: `commit`, `approved`, `changes_requested`,
/// `reviewed`, `comment`, `merged`, `review_requested`, `ready`, `force_pushed`, `closed`, `reopened` e `note` (nota
/// de sistema do GitLab, já em texto).
#[derive(Clone, Debug, Serialize)]
pub struct Activity { pub kind: String, pub who: String, pub text: String, pub at: String }

#[derive(Clone, Debug, Serialize)]
pub struct Detail {
    #[serde(flatten)]
    pub summary: Summary,
    /// Commits que faltam da base, quando o provedor conta.
    pub behind: Option<u64>,
    pub approvals: Approvals,
    pub merge: Merge,
    pub jobs: Vec<Job>,
    pub threads: Vec<Thread>,
    pub files: Vec<FileChange>,
    pub activity: Vec<Activity>,
}

/// Teto do patch de um arquivo e do conjunto: o detalhe vai inteiro numa resposta.
const PATCH_MAX: usize = 60_000;
const PATCHES_MAX: usize = 600_000;

/// Texto num caminho do JSON; ausente vira vazio.
pub fn text<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    path.iter().try_fold(v, |v, k| v.get(*k)).and_then(Value::as_str).unwrap_or("")
}

/// O `data` de uma resposta GraphQL; `errors` vira falha com as mensagens do provedor. Com `partial` (a busca de PRs, onde
/// uma organização com SAML só some do resultado), o que veio vale e o erro vai ao log, desde que algum campo de `data`
/// tenha vindo preenchido.
pub fn graphql_data(out: &str, cli: &str, partial: bool) -> Result<Value, crate::Failure> {
    let mut value: Value = serde_json::from_str(out)
        .map_err(|_| crate::Failure::new("invalid_response", format!("resposta do {cli} ilegível")))?;
    let data = value.get_mut("data").map(Value::take).unwrap_or(Value::Null);
    if let Some(errors) = value.get("errors").and_then(Value::as_array).filter(|e| !e.is_empty()) {
        let msg = errors.iter().filter_map(|e| e.get("message").and_then(Value::as_str)).collect::<Vec<_>>().join("; ");
        if !partial || !data.as_object().is_some_and(|fields| fields.values().any(|v| !v.is_null())) {
            return Err(crate::Failure::new("provider_error", msg));
        }
        tracing::warn!(cli, erro = %msg, "revisões: resposta GraphQL parcial");
    }
    Ok(data)
}

/// Os itens de todas as páginas de uma leitura REST com `--paginate`: o `gh` junta num array só, o `glab` manda um
/// array por página.
pub fn pages(out: &str, cli: &str) -> Result<Vec<Value>, crate::Failure> {
    let invalid = || crate::Failure::new("invalid_response", format!("resposta do {cli} ilegível"));
    let mut items = Vec::new();
    for page in serde_json::Deserializer::from_str(out).into_iter::<Value>() {
        match page.map_err(|_| invalid())? {
            Value::Array(a) => items.extend(a),
            _ => return Err(invalid()),
        }
    }
    Ok(items)
}

/// O cursor da próxima página de uma conexão GraphQL, quando há.
pub fn next_page(connection: Option<&Value>) -> Option<String> {
    let info = connection?.get("pageInfo")?;
    info.get("hasNextPage").and_then(Value::as_bool).filter(|more| *more)?;
    info.get("endCursor").and_then(Value::as_str).map(str::to_owned)
}

/// O cursor seguinte de um laço de páginas; o mesmo cursor de novo repetiria a página para sempre.
pub fn advance(connection: Option<&Value>, sent: Option<&str>) -> Result<Option<String>, crate::Failure> {
    match next_page(connection) {
        Some(next) if Some(next.as_str()) == sent => Err(crate::Failure::new("invalid_response", "a paginação do provedor não avançou")),
        next => Ok(next),
    }
}

/// Segundos desde a época de um instante RFC 3339.
fn stamp(text: &str) -> Option<i64> { chrono::DateTime::parse_from_rfc3339(text).ok().map(|t| t.timestamp()) }

/// Duração entre dois instantes RFC 3339, quando os dois existem e estão em ordem.
pub fn span(start: &str, end: &str) -> Option<u64> {
    match (stamp(start), stamp(end)) {
        (Some(a), Some(b)) if b >= a => Some((b - a) as u64),
        _ => None,
    }
}

/// Os arquivos com os patches cortados no teto de cada um e no do conjunto.
pub fn files(list: impl IntoIterator<Item = (String, u32, u32, String)>) -> Vec<FileChange> {
    let mut budget = PATCHES_MAX;
    list.into_iter().map(|(path, additions, deletions, patch)| {
        let patch = clip(&patch, PATCH_MAX.min(budget));
        budget = budget.saturating_sub(patch.len());
        FileChange { path, additions, deletions, patch }
    }).collect()
}

/// Corta o texto num limite de caractere, marcando o corte.
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max { return text.to_owned(); }
    let mut end = max;
    while !text.is_char_boundary(end) { end -= 1; }
    format!("{}\n…", &text[..end])
}

/// Conta de bot quando o provedor não diz: o nome entrega (`[bot]`, `-bot`, `_bot`, os bots de projeto do GitLab,
/// `coderabbitai`).
pub fn is_bot(login: &str) -> bool {
    let l = login.to_ascii_lowercase();
    l.ends_with("[bot]") || l.ends_with("-bot") || l.ends_with("_bot") || l == "coderabbitai"
        || (l.starts_with("project_") || l.starts_with("group_")) && l.contains("_bot")
}

/// Estado do pipeline nos nomes do contrato.
pub fn pipeline_status(raw: &str) -> Option<String> {
    let s = raw.to_ascii_lowercase();
    Some(match s.as_str() {
        "" | "null" => return None,
        "success" | "passed" | "neutral" => "success",
        "failure" | "failed" | "error" | "timed_out" | "action_required" | "startup_failure" => "failed",
        "pending" | "expected" | "created" | "waiting_for_resource" | "preparing" | "scheduled" | "queued" | "waiting"
            | "requested" | "manual" | "waiting_for_callback" => "pending",
        "running" | "in_progress" => "running",
        "canceled" | "cancelled" | "canceling" => "canceled",
        "skipped" | "stale" => "skipped",
        _ => "pending",
    }.to_owned())
}

/// Estado de uma etapa do pipeline; sem estado conhecido, na fila.
pub fn job_status(raw: &str) -> String { pipeline_status(raw).unwrap_or_else(|| "pending".into()) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_of_both_providers_map_to_the_contract() {
        assert_eq!(pipeline_status("SUCCESS").as_deref(), Some("success"));
        assert_eq!(pipeline_status("FAILURE").as_deref(), Some("failed"));
        assert_eq!(pipeline_status("waiting_for_resource").as_deref(), Some("pending"));
        assert_eq!(pipeline_status("IN_PROGRESS").as_deref(), Some("running"));
        assert_eq!(pipeline_status("canceled").as_deref(), Some("canceled"));
        assert_eq!(pipeline_status(""), None);
        assert_eq!(job_status(""), "pending");
    }

    #[test]
    fn bots_are_recognized_by_name() {
        assert!(is_bot("coderabbitai"));
        assert!(is_bot("dependabot[bot]"));
        assert!(is_bot("renovate-bot"));
        assert!(is_bot("project_42_bot_3f2a"));
        assert!(!is_bot("marina"));
        assert!(!is_bot("talbot"));
    }

    #[test]
    fn clip_keeps_char_boundaries() {
        assert_eq!(clip("ação", 100), "ação");
        assert!(clip("ãããã", 3).starts_with('ã'));
    }

    #[test]
    fn patches_share_one_budget() {
        let big = "x".repeat(PATCH_MAX + 10);
        let list = files((0..12).map(|n| (format!("f{n}"), 1, 0, big.clone())));
        assert!(list[0].patch.len() <= PATCH_MAX + 4);
        assert!(list.iter().map(|f| f.patch.len()).sum::<usize>() <= PATCHES_MAX + 12 * 4);
    }

    #[test]
    fn pages_join_one_array_or_one_per_page() {
        assert_eq!(pages("[1,2,3]", "gh").unwrap().len(), 3);
        assert_eq!(pages("[1,2][3]\n[4]", "glab").unwrap().len(), 4);
        assert_eq!(pages("", "glab").unwrap().len(), 0);
        assert_eq!(pages(r#"{"message": "404"}"#, "glab").unwrap_err().code, "invalid_response");
        assert_eq!(pages("[1,", "gh").unwrap_err().code, "invalid_response");
    }

    #[test]
    fn next_page_only_when_the_provider_says_there_is_more() {
        let more = serde_json::json!({"pageInfo": {"hasNextPage": true, "endCursor": "c2"}});
        let done = serde_json::json!({"pageInfo": {"hasNextPage": false, "endCursor": "c3"}});
        assert_eq!(next_page(Some(&more)).as_deref(), Some("c2"));
        assert_eq!(next_page(Some(&done)), None);
        assert_eq!(next_page(None), None);
        assert_eq!(advance(Some(&more), Some("c1")).unwrap().as_deref(), Some("c2"));
        assert_eq!(advance(Some(&more), Some("c2")).unwrap_err().code, "invalid_response");
        assert_eq!(advance(Some(&done), Some("c3")).unwrap(), None);
    }

    #[test]
    fn graphql_errors_become_failures() {
        assert_eq!(graphql_data(r#"{"data": {"a": 1}}"#, "gh", false).unwrap()["a"], 1);
        assert_eq!(graphql_data(r#"{"errors": [{"message": "ruim"}]}"#, "gh", false).unwrap_err().message, "ruim");
        assert_eq!(graphql_data(r#"{"data": null, "errors": [{"message": "ruim"}]}"#, "gh", true).unwrap_err().code, "provider_error");
        assert_eq!(graphql_data(r#"{"data": {"a": 2}, "errors": [{"message": "SAML"}]}"#, "gh", true).unwrap()["a"], 2);
        assert_eq!(graphql_data(r#"{"data": {"viewer": {}, "repository": null}, "errors": [{"message": "x"}]}"#, "gh", false).unwrap_err().code, "provider_error");
        assert_eq!(span("2026-11-12T10:00:00Z", "2026-11-12T10:01:30Z"), Some(90));
    }
}
