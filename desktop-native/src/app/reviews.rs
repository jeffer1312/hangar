//! Revisões (pull requests e merge requests). O ícone no topo, ao lado das worktrees, abre a página com todas as das
//! máquinas conectadas; o diálogo de git da sessão ganha a aba da revisão da branch; a faixa do compositor mostra um
//! chip com o estado dela. Os dados vêm do servidor (`/api/reviews*`), que usa o `gh` e o `glab` logados nele: aqui
//! não há configuração.
use super::*;
use super::costs::{empty_state, error_state, loading_state, page_frame};
use super::device::Remote;
use serde::Deserialize;
use std::{collections::BTreeMap, rc::Rc};

/// O chip relê a revisão da branch depois disto; o servidor ainda guarda a resposta por uns segundos.
const CHIP_TTL: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Summary {
    pub provider: String,
    pub host: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub title: String,
    pub author: String,
    pub mine: bool,
    pub review_requested: bool,
    pub draft: bool,
    pub state: String,
    pub source: String,
    pub target: String,
    pub pipeline: Option<String>,
    pub open_threads: Option<u32>,
    pub conflicts: Option<bool>,
    pub sessions: Vec<String>,
    /// Máquina que respondeu: as ações e as sessões são dela.
    #[serde(skip)]
    pub server: String,
}

impl Summary {
    fn key(&self) -> String { format!("{}/{}#{}", self.host, self.repo, self.number) }
    fn gitlab(&self) -> bool { self.provider == "gitlab" }
    fn number_label(&self) -> String { format!("{}{}", if self.gitlab() { "!" } else { "#" }, self.number) }
    /// Pede a atenção da pessoa: pediram a revisão dela, ou é dela e tem pipeline falho, conflito ou thread aberta.
    fn needs_attention(&self) -> bool {
        self.state == "open" && (self.review_requested && !self.mine || self.pipeline.as_deref() == Some("failed")
            || self.conflicts == Some(true) || self.open_threads.unwrap_or(0) > 0)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Approvals { state: String, need: Option<u32>, by: Vec<String>, requested: Vec<String>, can_approve: bool }

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Merge { can_merge: bool, can_auto: bool, auto_enabled: bool, blockers: Vec<String> }

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Job { name: String, status: String, duration: Option<u64> }

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Comment {
    pub author: String, pub body: String, pub at: String, pub bot: bool,
    /// O corpo já limpo e em Markdown seguro, feito uma vez quando a revisão chega.
    #[serde(skip)]
    pub shown: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Thread { pub id: String, pub file: Option<String>, pub line: Option<u64>, pub resolved: bool, pub code: String, pub comments: Vec<Comment> }

impl Thread {
    fn place(&self) -> String {
        match (&self.file, self.line) {
            (Some(file), Some(line)) => format!("{file}:{line}"),
            (Some(file), None) => file.clone(),
            _ => tr("review_thread_general"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct FileChange { path: String, additions: u32, deletions: u32, patch: String }

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Activity { kind: String, who: String, text: String, at: String }

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct Detail {
    #[serde(flatten)]
    summary: Summary,
    behind: Option<u64>,
    approvals: Approvals,
    merge: Merge,
    jobs: Vec<Job>,
    threads: Vec<Thread>,
    files: Vec<FileChange>,
    activity: Vec<Activity>,
}

/// A revisão da branch de uma pasta: o estado da busca e, quando há, o detalhe.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct BranchReview { state: String, host: String, branch: String, cli: Option<String>, summary: Option<Summary>, review: Option<Detail> }

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct HostRow { host: String, repos: u32, state: String, cli: Option<String>, user: Option<String>, error: Option<String> }

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Listing { hosts: Vec<HostRow>, reviews: Vec<Summary> }

/// Texto de um bloqueio de merge pelo código estável do servidor.
fn blocker_text(code: &str) -> String {
    let key = format!("review_block_{code}");
    let text = tr(&key);
    if text == key { tr("review_block_blocked") } else { text }
}

fn pipeline_label(status: Option<&str>) -> String {
    tr(match status {
        Some("success") => "review_pipeline_success",
        Some("failed") => "review_pipeline_failed",
        Some("running") => "review_pipeline_running",
        Some("pending") => "review_pipeline_pending",
        Some("canceled") => "review_pipeline_canceled",
        Some("skipped") => "review_pipeline_skipped",
        _ => "review_pipeline_none",
    })
}

fn status_color(status: Option<&str>) -> Hsla {
    match status {
        Some("success") => theme::success(),
        Some("failed") => theme::danger(),
        Some("running" | "pending") => theme::warning(),
        _ => theme::faint(),
    }
}

/// Pílula de estado: ponto e texto na cor dele, contorno discreto.
fn pill(text: String, color: Hsla, dot: bool) -> Div {
    div().flex_shrink_0().h(px(20.)).px(px(8.)).flex().items_center().gap(px(5.)).rounded_full().border_1().border_color(color.opacity(0.4))
        .text_size(px(11.5)).text_color(color).whitespace_nowrap()
        .when(dot, |el| el.child(div().size(px(6.)).rounded_full().bg(color)))
        .child(text)
}

fn host_tag(host: &str) -> Div { badge(host.to_owned(), theme::muted()).font_family(theme::MONO) }

/// Instante RFC 3339 como "há 5 min".
fn ago(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at).ok()
        .map(|t| super::side::ago((chrono::Utc::now().timestamp() - t.timestamp()).max(0) as f64))
        .unwrap_or_default()
}

/// Corpo de comentário para ler: sem comentários HTML (marcas de bot), blocos `<details>` (prompts e diffs longos) e
/// figuras, e sem as etiquetas HTML que os bots usam para formatar (o texto de dentro fica).
fn readable_body(body: &str) -> String {
    const BLOCKS: [(&str, &str); 3] = [("<!--", "-->"), ("<details", "</details>"), ("<picture", "</picture>")];
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    loop {
        let Some((at, close)) = BLOCKS.iter().filter_map(|(open, close)| rest.find(open).map(|at| (at, *close))).min_by_key(|(at, _)| *at) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..at]);
        match rest[at..].find(close) {
            Some(end) => rest = &rest[at + end + close.len()..],
            None => break,
        }
    }
    let text = strip_tags_outside_code(&out).lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    let text = text.trim();
    if text.is_empty() { body.trim().to_owned() } else { text.to_owned() }
}

/// `strip_tags` só na prosa: bloco cercado (```, ~~~) e trecho entre crases ficam como vieram, com o `<div>` que mostram.
fn strip_tags_outside_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prose = String::new();
    let mut fence: Option<char> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") { Some('`') } else if trimmed.starts_with("~~~") { Some('~') } else { None };
        match (fence, marker) {
            (None, Some(m)) => { out.push_str(&strip_inline(&std::mem::take(&mut prose))); out.push_str(line); fence = Some(m); }
            (Some(f), Some(m)) if f == m => { out.push_str(line); fence = None; }
            (Some(_), _) => out.push_str(line),
            (None, None) => prose.push_str(line),
        }
    }
    out.push_str(&strip_inline(&prose));
    out
}

/// Fora de bloco cercado: o código em linha fecha com a mesma quantidade de crases que abriu; sem fechamento, é texto.
fn strip_inline(text: &str) -> String {
    let run = |s: &str| s.bytes().take_while(|b| *b == b'`').count();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('`') {
        let n = run(&rest[at..]);
        let after = &rest[at + n..];
        let mut close = None;
        let mut i = 0;
        while let Some(found) = after[i..].find('`') {
            let m = run(&after[i + found..]);
            if m == n { close = Some(i + found); break; }
            i += found + m;
        }
        match close {
            Some(end) => { out.push_str(&strip_tags(&rest[..at])); out.push_str(&rest[at..at + n + end + n]); rest = &after[end + n..]; }
            None => { out.push_str(&strip_tags(&rest[..at + n])); rest = after; }
        }
    }
    out.push_str(&strip_tags(rest));
    out
}

/// Tira `<img>`, `<b>`, `<sub>`… e as de fechamento; `<br>` e `<p>` viram quebra. Outro `<` (genéricos, setas) fica.
fn strip_tags(text: &str) -> String {
    const TAGS: [&str; 15] = ["img", "sub", "sup", "br", "p", "div", "span", "b", "i", "strong", "em", "summary", "a", "kbd", "source"];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let name: String = after.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
        let next = after.trim_start_matches('/').chars().nth(name.len());
        let tag = TAGS.contains(&name.as_str()) && matches!(next, Some(' ' | '>' | '/'));
        match after.find('>').filter(|_| tag) {
            Some(end) => {
                if matches!(name.as_str(), "br" | "p") { out.push('\n'); }
                rest = &after[end + 1..];
            }
            None => { out.push('<'); rest = after; }
        }
    }
    out.push_str(rest);
    out
}

/// `![texto](endereço)` vira só o texto: o selo do bot diz a categoria, o endereço da imagem não serve ao agente.
fn image_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("![") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        match after.find("](").and_then(|mid| after[mid + 2..].find(')').map(|end| (mid, mid + 2 + end))) {
            Some((mid, end)) => { out.push_str(&after[..mid]); rest = &after[end + 1..]; }
            None => { out.push_str("!["); rest = after; }
        }
    }
    out.push_str(rest);
    out
}

/// O texto que vai ao compositor da sessão: onde, quem disse o quê, o trecho e o link.
fn thread_prompt(summary: &Summary, threads: &[&Thread]) -> String {
    let mut out = tr("review_send_intro").replace("{numero}", &summary.number_label()).replace("{titulo}", &summary.title);
    for thread in threads {
        out.push_str("\n\n");
        out.push_str(&format!("### {}\n", thread.place()));
        if !thread.code.trim().is_empty() { out.push_str(&format!("```diff\n{}\n```\n", thread.code.trim_end())); }
        for comment in &thread.comments {
            let body = image_text(&readable_body(&comment.body));
            out.push_str(&format!("**{}**: {}\n", comment.author, body.lines().collect::<Vec<_>>().join("\n> ")));
        }
    }
    out.push_str(&format!("\n{}", summary.url));
    out
}

/// O que a vista pede ao app: levar threads para a sessão da branch.
pub(super) struct SendThreads { pub server: String, pub sessions: Vec<String>, pub text: String }

pub(super) type SendFn = Rc<dyn Fn(SendThreads, &mut Window, &mut App)>;

#[derive(Clone)]
pub(super) enum Target { Branch(String), Item { host: String, repo: String, number: u64 } }

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Sub { #[default] Threads, Files, Activity }

/// A revisão aberta, com as abas e as ações. Serve à página e à aba do git.
pub(super) struct ReviewView {
    api: Api,
    server: String,
    runtime: Arc<Runtime>,
    target: Target,
    data: Remote<Rc<BranchReview>>,
    sub: Sub,
    show_resolved: bool,
    jobs_open: bool,
    open_files: HashSet<usize>,
    reply: Option<(String, Entity<TextareaState>)>,
    /// Respostas começadas em outras threads: abrir outra não joga fora o texto, voltar a ela o devolve.
    drafts: HashMap<String, Entity<TextareaState>>,
    /// Sobe a cada troca de branch: ação que volta de uma geração anterior era de outra revisão.
    generation: u64,
    reply_error: Option<String>,
    busy: Option<&'static str>,
    error: Option<String>,
    send: SendFn,
    /// Volta para a lista (só na página).
    back: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    /// Quem abriu atualiza o que depende desta revisão (o chip, a linha da lista) com o resumo relido depois de uma ação.
    changed: ChangedFn,
    after_action: bool,
}

pub(super) type ChangedFn = Rc<dyn Fn(&Summary, &mut App)>;

/// Prepara o que o desenho usa: o corpo de cada comentário limpo e em Markdown seguro, uma vez por leitura.
fn prepared(mut data: BranchReview) -> Rc<BranchReview> {
    for comment in data.review.iter_mut().flat_map(|d| d.threads.iter_mut()).flat_map(|t| t.comments.iter_mut()) {
        comment.shown = safe_markdown(&readable_body(&comment.body));
    }
    Rc::new(data)
}

/// A revisão da branch de uma pasta; `summary` pede só o resumo (o chip), sem o detalhe.
fn fetch_branch(runtime: &Runtime, api: Api, dir: String, summary: bool) -> tokio::task::JoinHandle<Result<BranchReview, Failure>> {
    runtime.spawn(async move {
        let query: &[(&str, &str)] = if summary { &[("path", &dir), ("summary", "1")] } else { &[("path", &dir)] };
        api.server_read(&["reviews", "branch"], query, 60).await
            .and_then(|v| serde_json::from_value::<BranchReview>(v).map_err(|_| Failure::local("invalid_response")))
    })
}

impl ReviewView {
    pub(super) fn new(api: Api, server: String, runtime: Arc<Runtime>, target: Target, send: SendFn, changed: ChangedFn) -> Self {
        Self { api, server, runtime, target, data: Remote::default(), sub: Sub::default(), show_resolved: false, jobs_open: false,
            open_files: HashSet::new(), reply: None, drafts: HashMap::new(), generation: 0, reply_error: None, busy: None, error: None, send, back: None, changed, after_action: false }
    }

    pub(super) fn with_back(mut self, back: Rc<dyn Fn(&mut Window, &mut App)>) -> Self { self.back = Some(back); self }

    pub(super) fn load(&mut self, cx: &mut Context<Self>) {
        let seq = self.data.start();
        let (api, target) = (self.api.clone(), self.target.clone());
        let job = match target {
            Target::Branch(dir) => fetch_branch(&self.runtime, api, dir, false),
            Target::Item { host, repo, number } => self.runtime.spawn(async move {
                api.server_read(&["reviews", "detail"], &[("host", &host), ("repo", &repo), ("number", &number.to_string())], 60).await
                    .and_then(|v| serde_json::from_value::<Detail>(v).map_err(|_| Failure::local("invalid_response")))
                    .map(|d| BranchReview { state: "ok".into(), host: d.summary.host.clone(), branch: d.summary.source.clone(), review: Some(d), ..Default::default() })
            }),
        };
        cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|_| Err(Failure::local("connection_failed")));
            let _ = this.update(cx, |this, cx| {
                if !this.data.finish(seq, result.map(prepared).map_err(|e| failure_text(&e))) { return; }
                if std::mem::take(&mut this.after_action) && let Some(summary) = this.detail().map(|d| Summary { server: this.server.clone(), ..d.summary.clone() }) {
                    (this.changed.clone())(&summary, cx);
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// Lê na primeira vez que aparece: abrir o diálogo do git sem ir à aba não roda gh/glab no servidor.
    pub(super) fn show(&mut self, cx: &mut Context<Self>) {
        if self.data.value.is_none() && !self.data.loading { self.load(cx); }
    }

    /// A branch da pasta mudou: some o detalhe da anterior (as ações dependem dele), e a resposta em voo dela é descartada.
    /// Volta ao não lido; quem mostra a aba chama `show`.
    pub(super) fn branch_changed(&mut self, cx: &mut Context<Self>) {
        self.data.reset();
        self.generation += 1;
        (self.reply, self.reply_error, self.error, self.after_action) = (None, None, None, false);
        self.drafts.clear();
        self.open_files.clear();
        cx.notify();
    }

    fn detail(&self) -> Option<&Detail> { self.data.ok().and_then(|b| b.review.as_ref()) }

    fn act(&mut self, action: &'static str, thread: Option<String>, body: Option<String>, cx: &mut Context<Self>) {
        let Some(summary) = self.detail().map(|d| d.summary.clone()) else { return };
        if self.busy.is_some() { return; }
        self.busy = Some(action);
        self.error = None;
        let (api, generation) = (self.api.clone(), self.generation);
        let mut payload = json!({"host": summary.host, "repo": summary.repo, "number": summary.number, "action": action});
        if let Some(thread) = thread { payload["thread"] = json!(thread); }
        if let Some(body) = body { payload["body"] = json!(body); }
        let job = self.runtime.spawn(async move { api.server_send(reqwest::Method::POST, &["reviews", "action"], Some(payload), 90).await });
        cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|_| Err(Failure::local("connection_failed")));
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                // A branch trocou no meio: a resposta é da revisão anterior e não mexe na que está aberta.
                if this.generation != generation { cx.notify(); return; }
                match result {
                    Ok(_) => {
                        if action == "reply" { this.reply = None; }
                        this.after_action = true;
                        this.load(cx);
                    }
                    Err(e) => this.error = Some(failure_text(&e)),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn send_threads(&mut self, only: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.detail() else { return };
        let threads: Vec<&Thread> = match only {
            Some(index) => detail.threads.get(index).into_iter().collect(),
            None => detail.threads.iter().filter(|t| !t.resolved).collect(),
        };
        if threads.is_empty() { return; }
        let request = SendThreads { server: self.server.clone(), sessions: detail.summary.sessions.clone(), text: thread_prompt(&detail.summary, &threads) };
        (self.send.clone())(request, window, cx);
    }

    fn open_reply(&mut self, thread: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, state)) = self.reply.take() { self.drafts.insert(id, state); }
        let state = self.drafts.remove(&thread)
            .unwrap_or_else(|| cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 8).placeholder(tr("review_reply_placeholder"))));
        state.update(cx, |input, cx| input.focus(window, cx));
        self.reply = Some((thread, state));
        self.reply_error = None;
        cx.notify();
    }

    fn post_reply(&mut self, cx: &mut Context<Self>) {
        let Some((thread, state)) = self.reply.clone() else { return };
        let text = state.read(cx).value().trim().to_owned();
        if text.is_empty() { self.reply_error = Some(tr("review_reply_empty")); cx.notify(); return; }
        self.reply_error = None;
        self.act("reply", Some(thread), Some(text), cx);
    }
}

pub(super) fn failure_text(error: &Failure) -> String {
    if let Some(code) = error.code.as_deref() {
        let key = format!("review_err_{code}");
        let text = tr(&key);
        if text != key { return if error.detail.is_empty() || code == "no_access" { text } else { format!("{text}: {}", error.detail) }; }
    }
    if error.status.is_none() { tr("connection_failed") } else if error.detail.is_empty() { tr("review_err_generic") } else { error.detail.clone() }
}

impl ReviewView {
    fn render_state(&self, data: &BranchReview, cx: &mut Context<Self>) -> Option<AnyElement> {
        let command = |cli: &str, host: &str| format!("{cli} auth login --hostname {host}");
        let (title, hint) = match data.state.as_str() {
            "ok" => return None,
            "none" => (tr("review_none_title").replace("{branch}", &data.branch), tr("review_none_hint").replace("{host}", &data.host)),
            "no_login" => (tr("review_no_login_title").replace("{host}", &data.host),
                tr("review_no_login_hint").replace("{comando}", &command(data.cli.as_deref().unwrap_or("gh"), &data.host))),
            "cli_missing" => (tr("review_cli_missing_title").replace("{cli}", data.cli.as_deref().unwrap_or("gh")), tr("review_cli_missing_hint")),
            "unsupported" => (tr("review_unsupported_title").replace("{host}", &data.host), tr("review_unsupported_hint")),
            _ => (tr("review_no_remote_title"), tr("review_no_remote_hint")),
        };
        Some(div().py(px(36.)).px_4().flex().flex_col().items_center().gap(px(6.))
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).whitespace_normal().child(title))
            .child(div().max_w(px(520.)).text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().text_center().child(hint))
            .when(matches!(data.state.as_str(), "no_login" | "cli_missing"), |el| el.child(
                Button::new("review-retry").outline().small().mt_2().label(tr("retry")).on_click(cx.listener(|this, _, _, cx| this.load(cx)))))
            .into_any_element())
    }

    fn render_cards(&self, d: &Detail, cx: &mut Context<Self>) -> Div {
        let card = |id: &'static str, label: String| div().id(id).flex_1().min_w(px(140.)).p(px(10.)).rounded(px(10.)).border_1()
            .border_color(theme::border()).bg(theme::boxed()).flex().flex_col().items_start().gap(px(5.))
            .child(div().text_size(px(11.)).text_color(theme::muted()).child(label));
        let s = &d.summary;
        let pipeline = card("review-card-pipeline", tr("review_card_pipeline"))
            .cursor_pointer().on_click(cx.listener(|this, _, _, cx| { this.jobs_open = !this.jobs_open; cx.notify(); }))
            .child(div().flex().items_center().gap_1()
                .child(pill(pipeline_label(s.pipeline.as_deref()), status_color(s.pipeline.as_deref()), true))
                .when(!d.jobs.is_empty(), |el| el.child(chrome::small_icon(if self.jobs_open { IconName::ChevronUp } else { IconName::ChevronDown }, 12., theme::faint()))))
            .when(self.jobs_open, |el| el.children(d.jobs.iter().map(|job| div().flex().items_center().gap(px(6.)).text_size(px(11.5))
                .child(div().size(px(6.)).rounded_full().bg(status_color(Some(job.status.as_str()))))
                .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).child(job.name.clone()))
                .when_some(job.duration, |el, s| el.child(div().text_color(theme::faint()).child(super::orq_panel::elapsed_text(Some(s))))))));
        let a = &d.approvals;
        let (approval_text, approval_color) = match a.state.as_str() {
            "approved" => (tr("review_approval_approved"), theme::success()),
            "changes" => (tr("review_approval_changes"), theme::danger()),
            "required" => (match a.need { Some(need) => tr("review_approval_count").replace("{n}", &a.by.len().to_string()).replace("{total}", &need.to_string()),
                None => tr("review_approval_required") }, theme::warning()),
            _ => (tr("review_approval_none"), theme::faint()),
        };
        let people = if a.by.is_empty() { a.requested.join(", ") } else { a.by.join(", ") };
        let approvals = card("review-card-approvals", tr("review_card_approvals"))
            .child(div().flex().items_center().gap(px(6.)).min_w_0().child(pill(approval_text, approval_color, false))
                .when(!people.is_empty(), |el| el.child(div().min_w_0().truncate().text_size(px(11.5)).text_color(theme::muted()).child(people))));
        let conflicts = card("review-card-conflicts", tr("review_card_conflicts")).child(match s.conflicts {
            Some(true) => pill(tr("review_conflict_yes"), theme::danger(), false),
            Some(false) => pill(tr("review_conflict_no"), theme::success(), false),
            None => pill(tr("review_conflict_unknown"), theme::faint(), false),
        });
        let open = d.threads.iter().filter(|t| !t.resolved).count();
        let threads = card("review-card-threads", tr("review_card_threads")).child(if open > 0 {
            pill(super::rows::counted("review_threads_open", open), theme::warning(), false)
        } else { pill(tr("review_threads_none"), theme::success(), false) });
        div().flex().flex_wrap().gap(px(8.)).child(pipeline).child(approvals).child(conflicts).child(threads)
    }

    fn render_thread(&self, index: usize, thread: &Thread, d: &Detail, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        let has_session = !d.summary.sessions.is_empty();
        let send_label = if has_session { tr("review_send") } else { tr("review_copy") };
        let replying = self.reply.as_ref().filter(|(id, _)| id == &thread.id).map(|(_, state)| state.clone());
        let prefix = format!("review-thread-{index}");
        let id = |part: &str| SharedString::from(format!("{prefix}-{part}"));
        let (send, reply, resolve) = thread_buttons(&prefix, send_label);
        thread_box(&prefix, thread, cx)
            .when(!thread.resolved, |el| el.child(thread_actions()
                .child(send.disabled(busy).on_click(cx.listener(move |this, _, window, cx| this.send_threads(Some(index), window, cx))))
                .child(reply.disabled(busy || replying.is_some())
                    .on_click(cx.listener({ let thread = thread.id.clone(); move |this, _, window, cx| this.open_reply(thread.clone(), window, cx) })))
                .child(resolve.disabled(busy).loading(self.busy == Some("resolve"))
                    .on_click(cx.listener({ let thread = thread.id.clone(); move |this, _, _, cx| this.act("resolve", Some(thread.clone()), None, cx) })))))
            .when_some(replying, |el, state| el.child(div().px(px(10.)).pb(px(10.)).pl(px(40.)).flex().flex_col().gap(px(6.))
                .child(Textarea::new(&state).aria_label(tr("review_reply")).disabled(busy))
                .when_some(self.reply_error.clone(), |el, e| el.child(div().text_size(px(12.)).text_color(theme::danger()).child(e)))
                .child(div().flex().gap(px(6.))
                    .child(Button::new(id("post")).primary().xsmall().label(tr("review_reply")).loading(self.busy == Some("reply")).disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.post_reply(cx))))
                    .child(Button::new(id("cancel")).ghost().xsmall().label(tr("cancel")).disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| { this.reply = None; this.reply_error = None; cx.notify(); }))))))
            .into_any_element()
    }

    fn render_files(&self, d: &Detail, cx: &mut Context<Self>) -> Vec<AnyElement> {
        d.files.iter().enumerate().map(|(index, file)| {
            let open = self.open_files.contains(&index);
            div().id(SharedString::from(format!("review-file-{index}"))).rounded(px(8.)).border_1().border_color(theme::border()).overflow_hidden()
                .child(div().id(SharedString::from(format!("review-file-head-{index}"))).px(px(10.)).py(px(6.)).flex().items_center().gap_2()
                    .bg(theme::boxed()).cursor_pointer().hover(|el| el.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, _, cx| { if !this.open_files.remove(&index) { this.open_files.insert(index); } cx.notify(); }))
                    .child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., theme::faint()))
                    .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(12.)).child(file.path.clone()))
                    .child(div().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::success()).child(format!("+{}", file.additions)))
                    .child(div().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::removed()).child(format!("−{}", file.deletions))))
                .when(open, |el| el.child(div().bg(theme::inset()).py(px(4.)).children(patch_rows(&file.patch))))
                .into_any_element()
        }).collect()
    }
}

/// A caixa da thread: onde, o trecho do diff e os comentários. A página e a animação da novidade desenham por aqui.
pub(crate) fn thread_box(prefix: &str, thread: &Thread, cx: &App) -> Stateful<Div> {
    let id = |part: &str| SharedString::from(format!("{prefix}-{part}"));
    div().id(id("box")).rounded(px(10.)).border_1().border_color(theme::border()).overflow_hidden().when(thread.resolved, |el| el.opacity(0.65))
        .child(div().px(px(10.)).py(px(6.)).flex().items_center().gap_2().bg(theme::boxed())
            .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::muted()).child(thread.place()))
            .when(thread.resolved, |el| el.child(pill(tr("review_thread_resolved"), theme::success(), false))))
        .when(!thread.code.trim().is_empty(), |el| el.child(div().border_t_1().border_b_1().border_color(theme::border()).bg(theme::inset()).py(px(4.))
            .children(thread.code.lines().map(|line| {
                let (kind, text) = match line.as_bytes().first() {
                    Some(b'+') => (super::git::Kind::Add, &line[1..]),
                    Some(b'-') => (super::git::Kind::Del, &line[1..]),
                    _ => (super::git::Kind::Context, line.get(1..).unwrap_or("")),
                };
                super::git::line_row(kind, None, None, SharedString::from(text.replace('\t', "    ")), true)
            }))))
        .children(thread.comments.iter().enumerate().map(|(n, c)| div().px(px(10.)).py(px(8.)).flex().gap(px(8.))
            .child(div().size(px(22.)).flex_shrink_0().rounded_full().flex().items_center().justify_center().text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD).bg(if c.bot { theme::accent().opacity(0.18) } else { theme::hover() })
                .text_color(if c.bot { theme::accent() } else { theme::muted() })
                .child(c.author.chars().take(2).collect::<String>().to_uppercase()))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().flex().items_center().gap(px(6.)).text_size(px(12.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(c.author.clone()))
                    .child(div().text_color(theme::faint()).child(ago(&c.at))))
                .child(div().text_size(px(13.)).child(gpui_kit::base::TextView::markdown(id(&format!("c{n}")),
                    if c.shown.is_empty() { safe_markdown(&readable_body(&c.body)) } else { c.shown.clone() })
                    .selectable(true).scrollable(false).style(theme::conversation_markdown(cx)))))))
}

/// A faixa dos botões de uma thread aberta, alinhada ao texto dos comentários.
pub(crate) fn thread_actions() -> Div { div().px(px(10.)).pb(px(8.)).pl(px(40.)).flex().flex_wrap().gap(px(6.)) }

/// Mandar (ou copiar), responder e resolver, sem a ação: quem desenha liga cada um.
pub(crate) fn thread_buttons(prefix: &str, send_label: String) -> (Button, Button, Button) {
    let id = |part: &str| SharedString::from(format!("{prefix}-{part}"));
    (Button::new(id("send")).primary().xsmall().label(send_label),
        Button::new(id("reply")).outline().xsmall().label(tr("review_reply")),
        Button::new(id("resolve")).ghost().xsmall().label(tr("review_resolve")))
}

/// Uma revisão na lista: host, título, repositório e as pílulas de estado.
pub(crate) fn row_view(item: &Summary) -> Stateful<Div> {
    let open = item.open_threads.unwrap_or(0) as usize;
    let who = if item.mine { tr("review_row_mine") } else { tr("review_row_by").replace("{autor}", &item.author) };
    div().id(SharedString::from(format!("review-row-{}", item.key()))).px(px(10.)).py(px(8.)).rounded(px(9.)).flex().items_center().gap(px(10.))
        .child(host_tag(&item.host))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().truncate().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(item.title.clone()))
            .child(div().truncate().text_size(px(12.)).text_color(theme::muted())
                .child(format!("{} {} · {}{}", item.repo, item.number_label(), who,
                    if item.sessions.is_empty() { String::new() } else { format!(" · {}", tr("review_session").replace("{nome}", &item.sessions.join(", "))) }))))
        .child(pill(pipeline_label(item.pipeline.as_deref()), status_color(item.pipeline.as_deref()), true))
        .when(open > 0, |el| el.child(pill(super::rows::counted("review_threads_open", open), theme::warning(), false)))
        .when(item.conflicts == Some(true), |el| el.child(pill(tr("review_conflict_short"), theme::danger(), false)))
        .when(item.draft, |el| el.child(pill(tr("review_state_draft"), theme::faint(), false)))
}

/// Linhas do patch de um arquivo, pelo mesmo leitor e desenho do diff do git.
fn patch_rows(patch: &str) -> Vec<AnyElement> {
    use super::git::Body;
    let Some(file) = super::git::parse_patch(patch).into_iter().next().filter(|f| !f.body.is_empty()) else {
        return vec![div().px_3().py_2().text_size(px(12.)).text_color(theme::faint()).child(tr("review_file_no_patch")).into_any_element()];
    };
    file.body.iter().map(|row| match row {
        Body::Hunk(text) => super::git::hunk_row(text),
        Body::Notice(text) => super::git::notice_row(text.clone(), theme::faint()),
        Body::Line { kind, old, new, text } => super::git::line_row(*kind, *old, *new, text.clone(), true),
    }).collect()
}

fn activity_text(a: &Activity) -> String {
    let key = format!("review_activity_{}", a.kind);
    let base = tr(&key);
    let base = if base == key { a.text.clone() } else { base.replace("{texto}", &a.text) };
    base.lines().next().unwrap_or("").to_owned()
}

impl Render for ReviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div().id("review-view").size_full().flex().flex_col().min_h_0();
        let data = match &self.data.value {
            None => return root.child(popup::skeleton("review-loading", 6)).into_any_element(),
            Some(Err(reason)) => return root.child(error_state(reason.clone(), cx.listener(|this, _, _, cx| this.load(cx)))).into_any_element(),
            Some(Ok(data)) => data.clone(),
        };
        if let Some(state) = self.render_state(&data, cx) { return root.child(state).into_any_element(); }
        let Some(d) = data.review.as_ref() else { return root.into_any_element() };
        let s = &d.summary;
        let open_threads = d.threads.iter().filter(|t| !t.resolved).count();
        let site = if s.gitlab() { "GitLab" } else { "GitHub" };
        let state_text = tr(match s.state.as_str() { "merged" => "review_state_merged", "closed" => "review_state_closed", _ if s.draft => "review_state_draft", _ => "review_state_open" });
        let url = s.url.clone();
        let header = div().flex().items_start().gap_3()
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(4.))
                .child(div().flex().items_center().gap(px(6.)).flex_wrap()
                    .when_some(self.back.clone(), |el, back| el.child(Button::new("review-back").ghost().xsmall().icon(IconName::ArrowLeft)
                        .label(tr("review_all")).on_click(move |_, window, cx| back(window, cx))))
                    .child(pill(state_text, theme::accent(), false))
                    .child(host_tag(&s.host))
                    .child(div().font_family(theme::MONO).text_size(px(12.)).text_color(theme::muted()).child(format!("{} {}", s.repo, s.number_label()))))
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).whitespace_normal().child(s.title.clone()))
                .child(div().flex().items_center().gap(px(5.)).flex_wrap().text_size(px(12.)).text_color(theme::muted())
                    .child(div().font_family(theme::MONO).child(s.source.clone())).child(tr("review_into"))
                    .child(div().font_family(theme::MONO).child(s.target.clone()))
                    .child(format!("· {}", tr("review_by").replace("{autor}", &s.author)))
                    .when_some(d.behind.filter(|b| *b > 0), |el, b| el.child(format!("· {}", tr("review_behind").replace("{n}", &b.to_string()))))
                    .when(!s.sessions.is_empty(), |el| el.child(format!("· {}", tr("review_session").replace("{nome}", &s.sessions.join(", ")))))))
            .child(div().flex().items_center().gap(px(6.)).flex_shrink_0()
                .child(chrome::icon_button("review-refresh", IconName::RefreshCw, tr("review_refresh"), cx).disabled(self.data.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))))
                .child(Button::new("review-web").outline().small().label(tr("review_open_web").replace("{site}", site))
                    .on_click(move |_, _, cx| cx.open_url(&url))));
        let tabs = [(Sub::Threads, tr("review_tab_threads"), open_threads), (Sub::Files, tr("review_tab_files"), d.files.len()), (Sub::Activity, tr("review_tab_activity"), 0)];
        let subs = div().flex().items_center().gap(px(6.)).flex_wrap()
            .children(tabs.into_iter().map(|(sub, label, n)| Button::new(SharedString::from(format!("review-sub-{label}"))).small()
                .map(|b| if self.sub == sub { b } else { b.ghost() }).selected(self.sub == sub)
                .label(if n > 0 { format!("{label} ({n})") } else { label })
                .on_click(cx.listener(move |this, _, _, cx| { this.sub = sub; cx.notify(); }))))
            .child(div().flex_1())
            .when(self.sub == Sub::Threads && open_threads > 0, |el| el.child(Button::new("review-send-all").outline().small()
                .label(if s.sessions.is_empty() { super::rows::counted("review_copy_all", open_threads) } else { super::rows::counted("review_send_all", open_threads) })
                .on_click(cx.listener(|this, _, window, cx| this.send_threads(None, window, cx)))));
        let body: Vec<AnyElement> = match self.sub {
            Sub::Threads => {
                let resolved = d.threads.iter().filter(|t| t.resolved).count();
                let mut items = Vec::new();
                if resolved > 0 {
                    items.push(div().flex().child(Button::new("review-show-resolved").ghost().xsmall()
                        .label(if self.show_resolved { tr("review_hide_resolved") } else { tr("review_show_resolved").replace("{n}", &resolved.to_string()) })
                        .on_click(cx.listener(|this, _, _, cx| { this.show_resolved = !this.show_resolved; cx.notify(); }))).into_any_element());
                }
                let shown: Vec<(usize, &Thread)> = d.threads.iter().enumerate().filter(|(_, t)| self.show_resolved || !t.resolved).collect();
                if shown.is_empty() { items.push(empty_state(tr("review_threads_empty")).into_any_element()); }
                for (index, thread) in shown { items.push(self.render_thread(index, thread, &d, cx)); }
                items
            }
            Sub::Files => {
                let files = self.render_files(&d, cx);
                if files.is_empty() { vec![empty_state(tr("review_files_empty")).into_any_element()] } else { files }
            }
            Sub::Activity => if d.activity.is_empty() { vec![empty_state(tr("review_activity_empty")).into_any_element()] } else {
                d.activity.iter().map(|a| div().pl(px(10.)).py(px(5.)).border_l_2().border_color(theme::border()).flex().items_baseline().gap(px(8.))
                    .text_size(px(12.5))
                    .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(a.who.clone()))
                    .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(activity_text(a)))
                    .child(div().flex_shrink_0().text_color(theme::faint()).child(ago(&a.at))).into_any_element()).collect()
            },
        };
        let m = &d.merge;
        let busy = self.busy.is_some();
        let why = if m.blockers.is_empty() { tr("review_ready") } else {
            tr("review_blocked").replace("{motivos}", &m.blockers.iter().map(|b| blocker_text(b)).collect::<Vec<_>>().join(" · "))
        };
        let footer = div().pt(px(10.)).border_t_1().border_color(theme::border()).flex().items_center().gap_2().flex_wrap()
            .child(div().flex_1().min_w(px(200.)).text_size(px(12.5)).whitespace_normal()
                .text_color(if m.blockers.is_empty() { theme::success() } else { theme::warning_text() }).child(why))
            .when(d.approvals.can_approve, |el| el.child(Button::new("review-approve").outline().small().label(tr("review_approve"))
                .loading(self.busy == Some("approve")).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.act("approve", None, None, cx)))))
            .when(m.can_auto, |el| el.child(Button::new("review-auto").outline().small().label(tr("review_auto_merge"))
                .loading(self.busy == Some("auto_merge")).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.act("auto_merge", None, None, cx)))))
            .when(m.auto_enabled, |el| el.child(pill(tr("review_auto_on"), theme::accent(), false)))
            .when(s.state == "open", |el| el.child(Button::new("review-merge").primary().small().label(tr("review_merge"))
                .loading(self.busy == Some("merge")).disabled(busy || !m.can_merge)
                .on_click(cx.listener(|this, _, _, cx| this.act("merge", None, None, cx)))));
        root.gap(px(12.))
            .child(header)
            .child(self.render_cards(&d, cx))
            .child(subs)
            // Cada item fica na altura dele e a caixa rola; sem isso, um arquivo aberto espreme os outros.
            .child(div().id("review-body").flex_1().min_h(px(120.)).overflow_y_scroll().flex().flex_col().gap(px(8.))
                .children(body.into_iter().map(|item| div().flex_shrink_0().child(item))))
            .when_some(self.error.clone(), |el, e| el.child(div().id("review-error").role(Role::Alert).text_size(px(12.5))
                .text_color(theme::danger()).whitespace_normal().child(e)))
            .child(footer)
            .into_any_element()
    }
}

// ── Página e chip ──

#[derive(Default)]
struct MachineReviews { label: String, listing: Option<Listing>, loading: bool, error: Option<String> }

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Filter { #[default] All, Attention, Mine, Requested }

#[derive(Default)]
pub(super) struct Reviews {
    pub(super) view: Option<()>,
    machines: BTreeMap<String, MachineReviews>,
    seq: u64,
    filter: Filter,
    host_filter: Option<String>,
    search: Option<Entity<InputState>>,
    _search_events: Option<Subscription>,
    open: Option<Entity<ReviewView>>,
    /// Chip da faixa: por (servidor, pasta), quando foi pedido e o resumo da revisão da branch (sem revisão ou com
    /// falha, nada).
    chips: HashMap<(String, String), (Instant, Option<Summary>)>,
}

impl Hangar {
    pub(super) fn toggle_reviews(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reviews.view.is_some() { self.close_reviews(window, cx) } else { self.open_reviews(window, cx) }
    }

    pub(super) fn open_reviews(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection_dialog { return; }
        self.close_settings(window, cx);
        self.close_costs(window, cx);
        self.close_worktrees(window, cx);
        self.close_popups();
        self.command_panel = false;
        self.reviews.view = Some(());
        if self.reviews.search.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("review_search")).clean_on_escape());
            self.reviews._search_events = Some(cx.subscribe(&input, |_, _, event: &InputEvent, cx| if matches!(event, InputEvent::Change) { cx.notify() }));
            self.reviews.search = Some(input);
        }
        self.root_focus.focus(window, cx);
        self.load_reviews(cx);
        cx.notify();
    }

    pub(super) fn close_reviews(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reviews.view.take().is_some() {
            self.reviews.open = None;
            self.root_focus.focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn reviews_reconnected(&mut self, cx: &mut Context<Self>) {
        self.reviews.machines.clear();
        self.reviews.chips.clear();
        self.reviews.open = None;
        if self.reviews.view.is_some() { self.load_reviews(cx); }
    }

    fn load_reviews(&mut self, cx: &mut Context<Self>) {
        self.reviews.seq += 1;
        let seq = self.reviews.seq;
        let sources = self.worktree_sources();
        self.reviews.machines.retain(|key, _| sources.iter().any(|(k, _, _)| k == key));
        for (key, label, api) in sources {
            let entry = self.reviews.machines.entry(key.clone()).or_default();
            (entry.label, entry.loading, entry.error) = (label, true, None);
            let job = self.runtime.spawn(async move {
                api.server_read(&["reviews"], &[], 120).await
                    .and_then(|v| serde_json::from_value::<Listing>(v).map_err(|_| Failure::local("invalid_response")))
            });
            cx.spawn(async move |this, cx| {
                let result = job.await.unwrap_or_else(|_| Err(Failure::local("connection_failed")));
                let _ = this.update(cx, |this, cx| {
                    if this.reviews.seq != seq { return; }
                    let Some(entry) = this.reviews.machines.get_mut(&key) else { return };
                    entry.loading = false;
                    match result {
                        Ok(mut listing) => {
                            for item in &mut listing.reviews { item.server = key.clone(); }
                            entry.listing = Some(listing);
                        }
                        Err(e) => entry.error = Some(failure_text(&e)),
                    }
                    cx.notify();
                });
            }).detach();
        }
        cx.notify();
    }

    /// Todas as revisões, sem repetir a mesma vista por duas máquinas (fica a que tem sessão na branch).
    fn all_reviews(&self) -> Vec<Summary> {
        let mut out: Vec<Summary> = Vec::new();
        for machine in self.reviews.machines.values() {
            for item in machine.listing.iter().flat_map(|l| &l.reviews) {
                match out.iter_mut().find(|o| o.url == item.url) {
                    Some(found) if found.sessions.is_empty() && !item.sessions.is_empty() => *found = item.clone(),
                    Some(_) => {}
                    None => out.push(item.clone()),
                }
            }
        }
        out
    }

    /// Manda o texto das threads para a sessão da branch: aberta, ele entra no fim do campo; sem sessão, vai para a
    /// área de transferência.
    pub(super) fn send_review_threads(&mut self, request: SendThreads, window: &mut Window, cx: &mut Context<Self>) {
        let found = request.sessions.iter().find_map(|name| {
            if self.is_active_key(&request.server) { self.sessions.iter().find(|s| &s.name == name).cloned() }
            else { self.remote.get(&request.server).and_then(|l| l.sessions.iter().find(|s| &s.name == name).cloned()) }
        });
        let Some(session) = found else {
            cx.write_to_clipboard(ClipboardItem::new_string(request.text));
            window.push_notification(Notification::info(tr("review_copied")), cx);
            return;
        };
        self.close_reviews(window, cx);
        window.close_all_dialogs(cx);
        let same = self.selected.as_ref().is_some_and(|s| s.name == session.name) && self.open_server() == request.server;
        if !same && !self.select_on(&request.server, session.clone(), window, cx) { return; }
        let current = self.composer.read(cx).value().to_string();
        let text = if current.trim().is_empty() { request.text } else { format!("{}\n\n{}", current.trim_end(), request.text) };
        self.composer.update(cx, |input, cx| { input.set_value(text, window, cx); input.focus(window, cx); });
        window.push_notification(Notification::info(tr("review_sent").replace("{nome}", &session.name)), cx);
        cx.notify();
    }

    fn review_send_fn(&self, cx: &mut Context<Self>) -> SendFn {
        let weak = cx.entity().downgrade();
        Rc::new(move |request, window, cx| { let _ = weak.update(cx, |this, cx| this.send_review_threads(request, window, cx)); })
    }

    fn open_review_item(&mut self, item: &Summary, cx: &mut Context<Self>) {
        let Some(api) = self.worktree_sources().into_iter().find(|(k, _, _)| k == &item.server).map(|(_, _, api)| api) else { return };
        let target = Target::Item { host: item.host.clone(), repo: item.repo.clone(), number: item.number };
        let send = self.review_send_fn(cx);
        let weak = cx.entity().downgrade();
        let back: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |_, cx| { let _ = weak.update(cx, |this, cx| { this.reviews.open = None; cx.notify(); }); });
        let changed = self.review_changed_fn(None, cx);
        let view = cx.new(|_| ReviewView::new(api, item.server.clone(), self.runtime.clone(), target, send, changed).with_back(back));
        view.update(cx, |view, cx| view.load(cx));
        self.reviews.open = Some(view);
        cx.notify();
    }

    /// Depois de uma ação, o resumo relido troca a linha da lista e o chip daquela revisão, sem reler o resto.
    fn review_changed_fn(&self, chip: Option<(String, String)>, cx: &mut Context<Self>) -> ChangedFn {
        let weak = cx.entity().downgrade();
        Rc::new(move |summary, cx| {
            let summary = summary.clone();
            let chip = chip.clone();
            let _ = weak.update(cx, |this, cx| {
                for item in this.reviews.machines.values_mut().flat_map(|m| m.listing.iter_mut().flat_map(|l| l.reviews.iter_mut())) {
                    // As sessões são da máquina da linha: a resposta de outra não sabe das dela.
                    if item.url != summary.url { continue; }
                    let sessions = if item.server == summary.server { summary.sessions.clone() } else { std::mem::take(&mut item.sessions) };
                    *item = Summary { server: item.server.clone(), sessions, ..summary.clone() };
                }
                for (key, (_, value)) in this.reviews.chips.iter_mut() {
                    if chip.as_ref() == Some(key) || value.as_ref().is_some_and(|v| v.url == summary.url) { *value = Some(summary.clone()); }
                }
                cx.notify();
            });
        })
    }

    /// A vista da revisão da branch de uma sessão, para a aba do git.
    pub(super) fn branch_review_view(&mut self, cx: &mut Context<Self>) -> Option<Entity<ReviewView>> {
        let (api, session) = (self.session_api()?, self.selected.clone()?);
        let dir = session.git_dir()?.to_owned();
        let server = super::servers::norm(&api.identity());
        let send = self.review_send_fn(cx);
        let changed = self.review_changed_fn(Some((server.clone(), dir.clone())), cx);
        Some(cx.new(|_| ReviewView::new(api, server, self.runtime.clone(), Target::Branch(dir), send, changed)))
    }

    /// A branch da pasta trocou: o chip dela é pedido de novo no próximo desenho, sem mostrar o da anterior.
    pub(super) fn forget_review_chip(&mut self, key: &(String, String), cx: &mut Context<Self>) {
        if self.reviews.chips.remove(key).is_some() { cx.notify(); }
    }

    /// O chip da faixa: pede o resumo da revisão da branch da sessão aberta quando o pedido anterior venceu.
    pub(super) fn review_chip(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dir = self.selected.as_ref()?.git_dir()?.to_owned();
        let api = self.session_api()?;
        let key = (super::servers::norm(&api.identity()), dir);
        if self.reviews.chips.get(&key).is_none_or(|(at, _)| at.elapsed() > CHIP_TTL) {
            let previous = self.reviews.chips.remove(&key).and_then(|(_, v)| v);
            let asked = Instant::now();
            self.reviews.chips.insert(key.clone(), (asked, previous));
            let job = fetch_branch(&self.runtime, api, key.1.clone(), true);
            let k = key.clone();
            cx.spawn(async move |this, cx| {
                let summary = job.await.ok().and_then(Result::ok).and_then(|b| b.summary);
                let _ = this.update(cx, |this, cx| {
                    // Entrada trocada (a branch mudou e pediu de novo): esta resposta pode ser da branch anterior.
                    if let Some((at, value)) = this.reviews.chips.get_mut(&k) && *at == asked { *value = summary; }
                    cx.notify();
                });
            }).detach();
        }
        let s = self.reviews.chips.get(&key)?.1.as_ref()?;
        let open = s.open_threads.unwrap_or(0) as usize;
        let label = format!("{} {}", if s.gitlab() { tr("review_kind_mr") } else { tr("review_kind_pr") }, s.number_label());
        Some(Button::new("composer-review")
            .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::faint()).hover(theme::hover()).active(theme::hover()))
            .flex_shrink_0().h(px(22.)).px(px(6.)).rounded(px(6.)).tooltip(tr("review_chip_tip")).accessibility_label(tr("review_chip_tip"))
            .child(div().flex().items_center().gap(px(5.)).text_xs()
                .child(chrome::small_icon(IconName::GitPullRequest, 14., theme::faint()))
                .child(div().text_color(theme::muted()).child(label))
                .child(div().size(px(6.)).rounded_full().bg(status_color(s.pipeline.as_deref())))
                .when(open > 0, |el| el.child(div().text_color(theme::warning()).child(super::rows::counted("review_threads_open", open)))))
            .on_click(cx.listener(|this, _, window, cx| this.open_git_review(window, cx)))
            .into_any_element())
    }

    pub(super) fn render_reviews(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.reviews.machines.values().any(|m| m.loading);
        let search = self.reviews.search.clone().map(|input| Input::new(&input).small().w(px(240.)).cleanable(true)
            .aria_label(tr("review_search")).prefix(chrome::small_icon(IconName::Search, 13., theme::faint())).into_any_element());
        let header = self.costs_header(tr("review_title"), |this, window, cx| this.close_reviews(window, cx),
            search.into_iter().chain([
                Button::new("reviews-refresh").outline().small().label(tr("review_refresh")).loading(loading).disabled(loading)
                    .on_click(cx.listener(|this, _, _, cx| { this.reviews.chips.clear(); this.load_reviews(cx); })).into_any_element(),
            ]).collect(), cx);
        let body = match self.reviews.open.clone() {
            Some(view) => div().size_full().child(view).into_any_element(),
            None => self.render_review_list(cx),
        };
        let content = div().id("reviews-scroll").flex_1().min_h_0().overflow_y_scroll()
            .child(div().w_full().flex().justify_center()
                .child(div().w_full().max_w(px(1080.)).px(px(24.)).pt(px(20.)).pb(px(32.)).flex().flex_col().gap(px(14.))
                    .when(self.reviews.open.is_some(), |el| el.h_full())
                    .child(body)));
        page_frame(div().size_full().flex().flex_col().child(header).child(content))
    }

    fn render_review_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let machines = &self.reviews.machines;
        if machines.is_empty() || machines.values().all(|m| m.loading && m.listing.is_none()) {
            return loading_state().into_any_element();
        }
        let query = self.reviews.search.as_ref().map(|i| i.read(cx).value().trim().to_lowercase()).unwrap_or_default();
        let all = self.all_reviews();
        // O host escolhido fica entre os botões mesmo sem revisões dele na leitura nova: é por ele que se desfaz o filtro.
        let host_filter = self.reviews.host_filter.clone();
        let hosts: Vec<String> = { let mut h: Vec<String> = all.iter().map(|r| r.host.clone()).chain(host_filter.clone()).collect(); h.sort(); h.dedup(); h };
        let filter = self.reviews.filter;
        let items: Vec<Summary> = all.into_iter()
            .filter(|r| self.reviews.host_filter.as_ref().is_none_or(|h| &r.host == h))
            .filter(|r| match filter { Filter::All => true, Filter::Attention => r.needs_attention(), Filter::Mine => r.mine, Filter::Requested => r.review_requested })
            .filter(|r| query.is_empty() || format!("{} {} {} {}", r.title, r.repo, r.number_label(), r.source).to_lowercase().contains(&query))
            .collect();
        let chip = |id: String, label: String, on: bool| Button::new(SharedString::from(id)).small().map(|b| if on { b } else { b.ghost() }).selected(on).label(label);
        let filters = div().flex().items_center().gap(px(6.)).flex_wrap()
            .children([(Filter::All, tr("review_filter_all")), (Filter::Attention, tr("review_filter_attention")), (Filter::Mine, tr("review_filter_mine")),
                (Filter::Requested, tr("review_filter_requested"))].into_iter().map(|(f, label)| chip(format!("reviews-filter-{label}"), label, filter == f)
                .on_click(cx.listener(move |this, _, _, cx| { this.reviews.filter = f; cx.notify(); }))))
            .when(hosts.len() > 1 || host_filter.is_some(), |el| el.child(div().w(px(1.)).h(px(18.)).mx_1().bg(theme::border()))
                .children(hosts.into_iter().map(|host| {
                    let on = self.reviews.host_filter.as_deref() == Some(host.as_str());
                    chip(format!("reviews-host-{host}"), host.clone(), on)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reviews.host_filter = if this.reviews.host_filter.as_deref() == Some(host.as_str()) { None } else { Some(host.clone()) };
                            cx.notify();
                        }))
                })));
        let mut list = div().flex().flex_col().gap(px(2.));
        let errors: Vec<(String, String)> = machines.values().filter_map(|m| m.error.clone().map(|e| (m.label.clone(), e))).collect();
        let host_errors: Vec<HostRow> = machines.values().flat_map(|m| m.listing.iter().flat_map(|l| l.hosts.iter().filter(|h| h.state == "error").cloned())).collect();
        if items.is_empty() {
            list = list.child(empty_state(if query.is_empty() && filter == Filter::All && host_filter.is_none() { tr("review_list_empty") } else { tr("review_list_filtered") }));
        } else {
            let (attention, rest): (Vec<_>, Vec<_>) = items.into_iter().partition(Summary::needs_attention);
            for (title, group) in [(tr("review_group_attention"), attention), (tr("review_group_other"), rest)] {
                if group.is_empty() { continue; }
                list = list.child(div().mt(px(10.)).mb(px(2.)).text_size(px(11.)).text_color(theme::muted()).child(format!("{} ({})", title.to_uppercase(), group.len())));
                for item in group { list = list.child(self.review_row(&item, cx)); }
            }
        }
        div().flex().flex_col().gap(px(10.))
            .child(filters)
            .children(errors.into_iter().map(|(label, e): (String, String)| super::costs::note_box(tr("review_machine_error").replace("{maquina}", &label).replace("{erro}", &e), theme::warning())))
            .children(host_errors.into_iter().map(|h| super::costs::note_box(tr("review_host_error").replace("{host}", &h.host)
                .replace("{erro}", h.error.as_deref().unwrap_or("")), theme::warning())))
            .child(list)
            .child(self.render_review_hosts())
            .into_any_element()
    }

    fn review_row(&self, item: &Summary, cx: &mut Context<Self>) -> AnyElement {
        let clicked = item.clone();
        // Focável, a GPUI entrega Enter e Espaço como clique.
        row_view(item).cursor_pointer().hover(|el| el.bg(theme::hover()))
            .focusable().tab_stop(true).role(Role::Button).aria_label(item.title.clone())
            .focus_visible(|el| el.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, _, cx| this.open_review_item(&clicked, cx)))
            .into_any_element()
    }

    /// Os hosts que a máquina achou nos repositórios e quem atende cada um: é o que substitui uma configuração.
    fn render_review_hosts(&self) -> AnyElement {
        let rows: Vec<HostRow> = self.reviews.machines.values().flat_map(|m| m.listing.iter().flat_map(|l| l.hosts.clone())).collect();
        if rows.is_empty() { return div().into_any_element(); }
        div().mt(px(16.)).pt(px(12.)).border_t_1().border_color(theme::border()).flex().flex_col().gap(px(6.))
            .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr("review_hosts_title")))
            .children(rows.into_iter().map(|h| {
                let (text, color) = match h.state.as_str() {
                    "ok" => (format!("{} · {}", h.cli.clone().unwrap_or_default(), h.user.clone().unwrap_or_default()), theme::success()),
                    "no_login" => (tr("review_host_no_login").replace("{comando}", &format!("{} auth login --hostname {}", h.cli.clone().unwrap_or_default(), h.host)), theme::warning()),
                    "cli_missing" => (tr("review_host_cli_missing").replace("{cli}", &h.cli.clone().unwrap_or_default()), theme::warning()),
                    "error" => (tr("review_host_failed"), theme::danger()),
                    _ => (tr("review_host_unknown"), theme::faint()),
                };
                div().flex().items_center().gap(px(10.)).text_size(px(12.))
                    .child(div().w(px(220.)).flex_shrink_0().truncate().font_family(theme::MONO).text_color(theme::text()).child(h.host.clone()))
                    .child(div().w(px(110.)).flex_shrink_0().text_color(theme::faint()).child(if h.repos == 1 { tr("review_host_repo") } else { tr("review_host_repos").replace("{n}", &h.repos.to_string()) }))
                    .child(div().flex_1().min_w_0().whitespace_normal().text_color(color).child(text))
            }))
            .child(div().text_size(px(11.5)).text_color(theme::faint()).whitespace_normal().child(tr("review_hosts_hint")))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob traz o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn bot_markup_is_dropped_from_comments() {
        let body = "<!-- marca -->\nO upload roda em série.\n<details>\n<summary>Prompt</summary>\nlongo\n</details>\nFim.";
        assert_eq!(readable_body(body), "O upload roda em série.\n\nFim.");
        assert_eq!(readable_body("<!-- só marca -->"), "<!-- só marca -->");
        let rabbit = "<picture><img src=\"x.svg\" width=\"16\"></picture> Minor · <b>Meça</b> o cartão<br>Vec<String> e a -> b";
        assert_eq!(readable_body(rabbit), "Minor · Meça o cartão\nVec<String> e a -> b");
        assert_eq!(image_text("![Testes](https://x/t.svg) ![Menor](https://x/m.svg) fim ![sem fim"), "Testes Menor fim ![sem fim");
    }

    #[test]
    fn html_inside_markdown_code_is_kept() {
        assert_eq!(readable_body("Troque `<div>` por ``<span>` ``, <b>já</b>."), "Troque `<div>` por ``<span>` ``, já.");
        let fenced = "Assim:\n```html\n<div><span>x</span></div>\n```\n<b>fim</b> `sem fecho <i>a</i>";
        assert_eq!(readable_body(fenced), "Assim:\n```html\n<div><span>x</span></div>\n```\nfim `sem fecho a");
    }

    #[test]
    fn attention_is_review_requested_or_something_broken() {
        let base = Summary { state: "open".into(), mine: true, ..Default::default() };
        assert!(!base.needs_attention());
        assert!(Summary { pipeline: Some("failed".into()), ..base.clone() }.needs_attention());
        assert!(Summary { open_threads: Some(2), ..base.clone() }.needs_attention());
        assert!(Summary { mine: false, review_requested: true, ..base.clone() }.needs_attention());
        assert!(!Summary { state: "merged".into(), open_threads: Some(1), ..base }.needs_attention());
    }

    #[test]
    fn detail_reads_the_server_contract() {
        let value = r#"{"provider": "gitlab", "host": "gitlab.example.com", "repo": "a/b", "number": 3, "url": "u", "title": "T",
            "author": "ana", "mine": true, "review_requested": false, "draft": false, "state": "open", "source": "feat", "target": "main",
            "pipeline": "running", "open_threads": 1, "conflicts": false, "sessions": ["s1"], 
            "behind": 2, "approvals": {"state": "required", "need": 1, "by": [], "requested": ["bia"], "can_approve": false}, "merge": {"can_merge": false, "can_auto": true, "auto_enabled": false, "blockers": ["pipeline_running"]},
            "jobs": [{"name": "build", "status": "running", "duration": null}], "threads": [{"id": "d1", "file": "x.rs", "line": 4,
            "resolved": false, "code": "+a", "comments": [{"author": "bia", "body": "b", "at": "2026-11-12T10:00:00Z", "bot": false}]}],
            "files": [{"path": "x.rs", "additions": 1, "deletions": 0, "patch": "@@ -1 +1 @@\n+a"}],
            "activity": [{"kind": "commit", "who": "ana", "text": "t", "at": "x"}]}"#;
        let d: Detail = serde_json::from_str(value).unwrap();
        assert_eq!(d.summary.number_label(), "!3");
        assert_eq!(d.summary.sessions, vec!["s1".to_owned()]);
        assert_eq!(d.merge.blockers, vec!["pipeline_running".to_owned()]);
        assert_eq!(d.threads[0].place(), "x.rs:4");
        assert_eq!(d.approvals.need, Some(1));
    }
}
