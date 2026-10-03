//! Git da sessão, aberto pela faixa embaixo do compositor, organizado como o painel de git do Zeron (`changes.rs`,
//! `history.rs`): abas Alterações e Histórico, um diff único com todos os arquivos (cabeçalho por arquivo que dobra, hunks,
//! números de linha), o histórico em linhas e o commit abrindo numa aba própria. O Zeron não commita nem troca branch fora
//! do compositor; a barra de commit, a marca por arquivo e o seletor de branch com stash são do Hangar. Só as rotas `/git*`,
//! `/branches` e `/checkout` do backend, e nada é escrito sem gesto.
use super::*;
use super::device::Remote;
use super::sidebar::git_note;
use gpui_kit::component::{WindowExt, notification::NotificationType, popover::Popover, tab::{Tab, TabBar}};
use std::rc::Rc;

/// O diálogo no tamanho do painel expandido: `min(1100, 92% da janela)` por `min(720, 78%)`.
const MAX_W: f32 = 1100.;
const MAX_H: f32 = 720.;
// Medidas do diff do Zeron (`changes.rs`): cabeçalho de arquivo, hunk, linha, aviso, calhas e marcador.
const FILE_H: f32 = 38.;
const HUNK_H: f32 = 28.;
const LINE_H: f32 = 21.;
const NOTICE_H: f32 = 24.;
const GUTTER: f32 = 36.;
const MARKER: f32 = 28.;
const ACCENT_BAR: f32 = 3.;
/// Linha do histórico do Zeron e as colunas fixas dela.
const COMMIT_H: f32 = 36.;
const AUTHOR_W: f32 = 96.;
const DATE_W: f32 = 112.;
const SHA_W: f32 = 74.;
const LOG_PAGE: usize = 50;
const LOG_MAX: usize = 2000;

mod local;

/// O que o painel pede ao git. As duas fontes respondem o mesmo JSON das rotas `/git*` do backend.
pub(super) enum Op {
    Branches, Files, Diff(String), Discard(String), Commit { message: String, paths: Vec<String>, amend: bool }, Push, Fetch, Pull, LastMessage,
    Log(usize), CommitDiff(String), Checkout(String), Stash, CreateBranch(String),
}

/// Onde o git roda: no disco desta máquina (servidor em loopback e a pasta existe aqui) ou pelas rotas da sessão.
#[derive(Clone)]
enum Source { Local(Arc<std::path::PathBuf>), Remote(Api, String) }

impl Source {
    async fn call(&self, op: Op) -> Result<Value, Failure> {
        let (api, name) = match self {
            Source::Local(cwd) => {
                let cwd = cwd.clone();
                return tokio::task::spawn_blocking(move || local::call(&cwd, &op)).await.map_err(|_| Failure::local("invalid_response"))?
                    .map_err(|(status, detail)| Failure { status: Some(status), detail, retry_after: None, uncertain: false });
            }
            Source::Remote(api, name) => (api, name.as_str()),
        };
        match op {
            Op::Branches => api.read(name, &["branches"], &[], 30).await,
            Op::Files => api.read(name, &["git", "files"], &[], 30).await,
            Op::Diff(path) => api.act(name, &["git", "diff"], Some(json!({"path": path})), false, 30).await,
            Op::Discard(path) => api.act(name, &["git", "discard"], Some(json!({"path": path})), false, 30).await,
            Op::Commit { message, paths, amend } =>
                api.act(name, &["git", "commit"], Some(json!({"message": message, "paths": paths, "amend": amend})), false, 60).await,
            Op::Push => api.act(name, &["git", "push"], None, false, 120).await,
            Op::Fetch => api.act(name, &["git"], Some(json!({"action": "fetch"})), false, 130).await,
            Op::Pull => api.act(name, &["git"], Some(json!({"action": "pull"})), false, 130).await,
            Op::LastMessage => api.read(name, &["git", "last-message"], &[], 15).await,
            Op::Log(n) => api.read(name, &["git", "log"], &[("n", n.to_string().as_str())], 30).await,
            Op::CommitDiff(sha) => api.read(name, &["git", "commit", &sha, "diff-full"], &[], 30).await,
            Op::Checkout(branch) => api.act(name, &["checkout"], Some(json!({"branch": branch})), false, 60).await,
            Op::Stash => api.act(name, &["git"], Some(json!({"action": "stash"})), false, 60).await,
            Op::CreateBranch(branch) => api.act(name, &["git", "branch"], Some(json!({"name": branch, "switch_after": true})), false, 30).await,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pane { #[default] Changes, History, Commit }

/// Os três botões de sincronização do cabeçalho.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sync { Fetch, Pull, Push }

impl Sync {
    /// Também é a marca do `busy` enquanto roda.
    fn key(self) -> &'static str { match self { Sync::Fetch => "fetch", Sync::Pull => "pull", Sync::Push => "push" } }
    fn label(self) -> String { tr(match self { Sync::Fetch => "git_fetch", Sync::Pull => "git_pull", Sync::Push => "git_push" }) }
    fn hint(self) -> String { tr(match self { Sync::Fetch => "git_fetch_hint", Sync::Pull => "git_pull_hint", Sync::Push => "git_push_hint" }) }
    fn done(self) -> &'static str { match self { Sync::Fetch => "git_fetched", Sync::Pull => "git_pulled", Sync::Push => "git_pushed" } }
    fn icon(self) -> IconName { match self { Sync::Fetch => IconName::CloudDownload, Sync::Pull => IconName::ArrowDownToLine, Sync::Push => IconName::ArrowUpFromLine } }
}

#[derive(Clone, Debug, Default)]
struct Repo { files: Vec<(String, String)>, current: Option<String>, branches: Vec<String>, remotes: Vec<String>, dirty: bool }

fn strings(value: &Value, key: &str) -> Vec<String> {
    value.get(key).and_then(Value::as_array).map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
}

fn repo(branches: &Value, files: &Value) -> Repo {
    Repo {
        files: files.get("files").and_then(Value::as_array).map(|list| list.iter().filter_map(|f| Some((
            f.get("path")?.as_str()?.to_owned(), f.get("code").and_then(Value::as_str).unwrap_or("").to_owned(),
        ))).collect()).unwrap_or_default(),
        current: branches.get("current").and_then(Value::as_str).map(str::to_owned),
        branches: strings(branches, "branches"),
        remotes: strings(branches, "remotes"),
        dirty: branches.get("dirty").and_then(Value::as_bool).unwrap_or(false),
    }
}

// ── Diff ──

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind { Add, Del, Context, Meta }

#[derive(Clone, Debug)]
enum Body { Notice(SharedString), Hunk(SharedString), Line { kind: Kind, old: Option<u32>, new: Option<u32>, text: SharedString } }

/// Um arquivo do patch: o caminho, as linhas já numeradas e as contas.
#[derive(Clone, Debug, Default)]
struct Patch { path: String, body: Vec<Body>, added: usize, removed: usize, created: bool, deleted: bool }

/// `@@ -a,b +c,d @@` → primeiras linhas antiga e nova.
fn hunk_start(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(' ')?;
    let new = rest.strip_prefix('+')?.split(' ').next()?;
    let first = |s: &str| s.split(',').next()?.parse().ok();
    Some((first(old)?, first(new)?))
}

/// O `git diff`/`git show` em arquivos, como o `parse_patch` do Zeron: cabeçalhos viram avisos, hunks numeram as linhas.
fn parse_patch(text: &str) -> Vec<Patch> {
    let mut files: Vec<Patch> = Vec::new();
    let (mut old, mut new, mut in_hunk) = (0u32, 0u32, false);
    let clean = |s: &str| SharedString::from(s.replace('\t', "    "));
    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            let path = rest.rsplit_once(" b/").map(|(_, p)| p).unwrap_or(rest).to_owned();
            files.push(Patch { path, ..Default::default() });
            in_hunk = false;
            continue;
        }
        if files.is_empty() { files.push(Patch::default()); }
        let file = files.last_mut().expect("pushed above");
        if let Some((o, n)) = hunk_start(raw) {
            (old, new, in_hunk) = (o, n, true);
            file.body.push(Body::Hunk(raw.to_owned().into()));
            continue;
        }
        if !in_hunk {
            let notice = if raw.starts_with("new file") { file.created = true; Some(tr("git_new_file")) }
                else if raw.starts_with("deleted file") { file.deleted = true; Some(tr("git_deleted_file")) }
                else if let Some(from) = raw.strip_prefix("rename from ") { Some(tr("git_renamed_from").replace("{path}", from)) }
                else if raw.starts_with("Binary files") { Some(tr("git_binary")) }
                else { None };
            if let Some(text) = notice { file.body.push(Body::Notice(text.into())); }
            continue;
        }
        let line = match raw.as_bytes().first() {
            Some(b'+') => { file.added += 1; new += 1; Body::Line { kind: Kind::Add, old: None, new: Some(new - 1), text: clean(&raw[1..]) } }
            Some(b'-') => { file.removed += 1; old += 1; Body::Line { kind: Kind::Del, old: Some(old - 1), new: None, text: clean(&raw[1..]) } }
            Some(b'\\') => Body::Line { kind: Kind::Meta, old: None, new: None, text: clean(raw) },
            _ => { old += 1; new += 1; Body::Line { kind: Kind::Context, old: Some(old - 1), new: Some(new - 1), text: clean(raw.get(1..).unwrap_or("")) } }
        };
        file.body.push(line);
    }
    files
}

/// Um arquivo na lista: o código do porcelain (nas alterações), o patch em leitura e se está dobrado.
struct Entry { path: String, code: String, patch: Remote<Patch>, folded: bool }

#[derive(Clone, Copy, Debug)]
enum Row { Header(usize), Body(usize, usize), Status(usize) }

/// A lista do diff: uma linha por linha do patch, virtualizada; arquivo dobrado não entra.
struct Review { files: Vec<Entry>, rows: Vec<Row>, list: ListState, truncated: bool }

impl Review {
    fn new() -> Self { Self { files: Vec::new(), rows: Vec::new(), list: ListState::new(0, ListAlignment::Top, px(1024.)), truncated: false } }

    fn file_rows(&self, file: usize) -> Vec<Row> {
        let entry = &self.files[file];
        if entry.folded { return Vec::new(); }
        match &entry.patch.value {
            Some(Ok(patch)) if !patch.body.is_empty() => (0..patch.body.len()).map(|n| Row::Body(file, n)).collect(),
            _ => vec![Row::Status(file)],
        }
    }

    fn reset(&mut self) {
        self.rows = (0..self.files.len()).flat_map(|file| std::iter::once(Row::Header(file)).chain(self.file_rows(file))).collect();
        self.list.reset(self.rows.len());
    }

    /// Só as linhas de um arquivo mudam: a rolagem fica onde está.
    fn refresh(&mut self, file: usize) {
        let Some(start) = self.rows.iter().position(|row| matches!(row, Row::Header(f) if *f == file)) else { return };
        let end = self.rows[start + 1..].iter().position(|row| matches!(row, Row::Header(_))).map_or(self.rows.len(), |n| start + 1 + n);
        let rows = self.file_rows(file);
        self.list.splice(start + 1..end, rows.len());
        self.rows.splice(start + 1..end, rows);
    }

    fn totals(&self) -> (usize, usize) {
        self.files.iter().filter_map(|e| e.patch.ok()).fold((0, 0), |(a, r), p| (a + p.added, r + p.removed))
    }
}

fn hunk_row(text: &SharedString) -> AnyElement {
    div().h(px(HUNK_H)).w_full().flex_none().flex().items_center().px_4().bg(theme::accent().opacity(0.06))
        .font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).whitespace_nowrap().overflow_hidden()
        .child(text.clone()).into_any_element()
}

fn notice_row(text: SharedString, color: Hsla) -> AnyElement {
    div().h(px(NOTICE_H)).w_full().flex_none().flex().items_center().px_4().text_size(px(11.)).text_color(color).child(text).into_any_element()
}

/// Linha do diff do Zeron: barra de cor, as duas calhas de número, o marcador e o código. `wrap` quebra a linha
/// longa (o diff das edições no chat); sem ele a altura é fixa, que é o que a lista virtualizada do Git mede.
pub(super) fn line_row(kind: Kind, old: Option<u32>, new: Option<u32>, code: impl IntoElement, wrap: bool) -> AnyElement {
    let (marker, color) = match kind { Kind::Add => ("+", theme::success()), Kind::Del => ("−", theme::removed()), _ => ("·", theme::faint().opacity(0.5)) };
    let changed = matches!(kind, Kind::Add | Kind::Del);
    let gutter = |no: Option<u32>, lit: bool| div().w(px(GUTTER)).flex_none().flex().justify_end().pr(px(8.)).font_family(theme::MONO)
        .text_size(px(11.)).line_height(px(LINE_H)).text_color(if lit { color.opacity(0.9) } else { theme::faint().opacity(0.8) })
        .child(no.map(|n| n.to_string()).unwrap_or_default());
    // ponytail: no Git a linha longa é cortada na borda; rolagem lateral por arquivo (como a do Zeron) quando fizer falta.
    div().map(|el| if wrap { el.min_h(px(LINE_H)) } else { el.h(px(LINE_H)) }).w_full().flex_none().flex()
        .when(changed, |el| el.bg(color.opacity(0.055)))
        .child(div().w(px(ACCENT_BAR)).when(!wrap, |el| el.h_full()).flex_none().when(changed, |el| el.bg(color.opacity(0.55))))
        .child(gutter(old, kind == Kind::Del))
        .child(gutter(new, kind == Kind::Add))
        .child(div().w(px(MARKER)).flex_none().flex().justify_center().font_family(theme::MONO).text_size(px(12.)).line_height(px(LINE_H))
            .text_color(color).child(marker))
        .child(div().flex_1().min_w_0().pl(px(12.)).when(!wrap, |el| el.overflow_hidden().whitespace_nowrap()).font_family(theme::MONO)
            .text_size(px(12.)).line_height(px(LINE_H)).text_color(theme::text().opacity(0.92)).child(code))
        .into_any_element()
}

/// O git roda com `LC_ALL=C`: fora de um repositório a frase vem sempre igual, e a linha crua do git não ajuda ninguém.
fn failure(error: &Failure) -> String {
    if error.detail.contains("not a git repository") { tr("git_not_repo") } else { Hangar::fetch_failure(error) }
}

/// Código do porcelain em uma letra e a cor do Zeron na árvore: novo verde, mudado âmbar, apagado vermelho.
fn code_tag(code: &str) -> (&'static str, Hsla) {
    match code.trim() {
        "??" => ("U", theme::success()),
        c if c.starts_with('A') => ("A", theme::success()),
        c if c.contains('D') => ("D", theme::removed()),
        c if c.starts_with('R') => ("R", theme::accent()),
        c if c.contains('U') => ("!", theme::danger()),
        _ => ("M", theme::warning()),
    }
}

fn output(value: &Value) -> Option<String> {
    value.get("output").and_then(Value::as_str).map(str::trim).filter(|o| !o.is_empty()).map(str::to_owned)
}

/// As linhas `hint:` do git ensinam comando de terminal e empurram o motivo da recusa para fora da faixa de erro.
fn without_hints(text: &str) -> String {
    text.lines().filter(|line| !line.starts_with("hint:")).collect::<Vec<_>>().join("\n").trim().to_owned()
}

// ── Histórico ──

#[derive(Clone, Debug)]
struct Commit { hash: String, short: String, subject: String, body: String, author: String, ts: i64, refs: Vec<String>, local: bool }

fn commits(value: &Value) -> Vec<Commit> {
    value.get("commits").and_then(Value::as_array).map(|list| list.iter().filter_map(|c| {
        let text = |key: &str| c.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        Some(Commit {
            hash: c.get("hash")?.as_str()?.to_owned(), short: text("short"), subject: text("subject"), body: text("body"), author: text("author"),
            ts: c.get("ts").and_then(Value::as_i64).unwrap_or(0),
            // "HEAD -> main, origin/main, tag: v1": a seta e o HEAD solto não são nome de ref.
            refs: text("refs").split(", ").map(|r| r.trim_start_matches("HEAD -> ").to_owned())
                .filter(|r| !r.is_empty() && r != "HEAD" && !r.ends_with("/HEAD")).collect(),
            local: c.get("local").and_then(Value::as_bool).unwrap_or(false),
        })
    }).collect()).unwrap_or_default()
}

fn date(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&chrono::Local)
        .format(if crate::i18n::english() { "%b %-d, %Y %H:%M" } else { "%d/%m/%Y %H:%M" }).to_string()).unwrap_or_default()
}

/// Selo de ref do Zeron: branch em destaque, remota apagada, tag em âmbar.
fn ref_badge(name: &str, remotes: &[String]) -> Div {
    let (label, color) = if let Some(tag) = name.strip_prefix("tag: ") { (tag.to_owned(), theme::warning()) }
        else if name.contains('/') && !remotes.is_empty() || name.starts_with("origin/") { (name.to_owned(), theme::muted()) }
        else { (name.to_owned(), theme::accent()) };
    div().flex_shrink_0().max_w(px(112.)).h(px(16.)).px(px(6.)).flex().items_center().rounded(px(4.)).bg(color.opacity(0.07))
        .text_size(px(10.)).text_color(color).truncate().child(label)
}

fn unpushed_badge() -> Div {
    let color = theme::warning();
    div().flex_shrink_0().h(px(16.)).px(px(6.)).flex().items_center().rounded(px(4.)).bg(color.opacity(0.12))
        .text_size(px(10.)).text_color(color).child(format!("↑ {}", tr("git_commit_local")))
}

// ── Painel ──

/// O commit aberto na terceira aba e o diff dele.
struct Opened { commit: Commit, review: Review, state: Remote<()> }

pub(super) struct GitPanel {
    source: Source,
    runtime: Arc<Runtime>,
    name: String,
    title: String,
    pane: Pane,
    repo: Remote<Repo>,
    work: Review,
    /// Rodada da leitura das alterações: patch de uma rodada velha não entra.
    round: u64,
    /// Marcados para o commit. Todos na primeira leitura; depois, arquivo novo entra marcado e a escolha da pessoa fica.
    chosen: HashSet<String>,
    seen: HashSet<String>,
    message: Entity<TextareaState>,
    amend: bool,
    /// Uma gravação por vez; o texto diz qual.
    busy: Option<String>,
    error: Option<String>,
    output: Option<String>,
    log: Remote<Vec<Commit>>,
    log_limit: usize,
    ahead_behind: (Option<i64>, Option<i64>),
    opened: Option<Opened>,
    picker: bool,
    branch_query: Entity<InputState>,
    branch_error: Option<String>,
    /// Na aba Git do painel direito: lista estreita, e o diff e o histórico abrem no diálogo por aqui.
    expand: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    _subscriptions: Vec<Subscription>,
}

impl GitPanel {
    fn new(source: Source, runtime: Arc<Runtime>, name: String, title: String, expand: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
        window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6).placeholder(tr("git_message")));
        let branch_query = cx.new(|cx| InputState::new(window, cx).placeholder(tr("git_branch_search")));
        let notify = |_: &mut Self, _: Entity<_>, event: &InputEvent, cx: &mut Context<Self>| if matches!(event, InputEvent::Change) { cx.notify() };
        let subscriptions = vec![
            cx.subscribe(&message, notify),
            cx.subscribe_in(&branch_query, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                // Enter cria a branch digitada quando ela ainda não existe; existente, troca.
                InputEvent::PressEnter { .. } => this.branch_enter(window, cx),
                _ => {}
            }),
        ];
        Self {
            source, runtime, name, title, pane: Pane::default(), repo: Remote::default(), work: Review::new(), round: 0, chosen: HashSet::new(),
            seen: HashSet::new(), message, amend: false, busy: None, error: None, output: None, log: Remote::default(), log_limit: LOG_PAGE,
            ahead_behind: (None, None), opened: None, picker: false, branch_query, branch_error: None, expand, _subscriptions: subscriptions,
        }
    }

    /// Roda no runtime do app e devolve ao painel; painel fechado, a resposta cai.
    fn spawn<T: Send + 'static>(&self, job: impl Future<Output = T> + Send + 'static, window: &mut Window, cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static) {
        let job = self.runtime.spawn(job);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(value) = job.await else { return };
            let _ = this.update_in(cx, |this, window, cx| { done(this, value, window, cx); cx.notify(); });
        }).detach();
    }

    /// Branches e arquivos alterados juntos; depois o diff de cada arquivo, todos ao mesmo tempo.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.repo.start();
        let source = self.source.clone();
        self.spawn(async move {
            let (branches, files) = tokio::join!(source.call(Op::Branches), source.call(Op::Files));
            Ok::<_, Failure>(repo(&branches?, &files?))
        }, window, cx, move |this, result, window, cx| {
            if !this.repo.finish(seq, result.map_err(|error| failure(&error))) { return; }
            if let Some(files) = this.repo.ok().map(|r| r.files.clone()) { this.read_patches(files, window, cx); }
        });
        if self.log.value.is_some() { self.load_log(window, cx); } else { self.load_counts(window, cx); }
        cx.notify();
    }

    /// Sem o histórico aberto, só o à frente/atrás dos botões de pull e push: o log de um commit traz os dois.
    fn load_counts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.source.clone();
        self.spawn(async move { source.call(Op::Log(1)).await }, window, cx, |this, result, _, _| {
            let Ok(value) = result else { return };
            this.ahead_behind = (value.get("ahead").and_then(Value::as_i64), value.get("behind").and_then(Value::as_i64));
        });
    }

    fn read_patches(&mut self, files: Vec<(String, String)>, window: &mut Window, cx: &mut Context<Self>) {
        self.round += 1;
        let round = self.round;
        // Arquivo chega dobrado; só fica aberto o que a pessoa abriu.
        let open: HashSet<String> = self.work.files.iter().filter(|e| !e.folded).map(|e| e.path.clone()).collect();
        // Arquivo que some e volta (commitado, trocado de branch) conta como novo e entra marcado.
        let current: HashSet<String> = files.iter().map(|(p, _)| p.clone()).collect();
        self.chosen.extend(current.difference(&self.seen).cloned().collect::<Vec<_>>());
        self.chosen.retain(|path| current.contains(path));
        self.seen = current;
        self.work.files = files.into_iter().map(|(path, code)| {
            let mut patch = Remote::default();
            patch.start();
            Entry { folded: !open.contains(&path), path, code, patch }
        }).collect();
        self.work.reset();
        for (index, path) in self.work.files.iter().map(|e| e.path.clone()).enumerate() {
            let source = self.source.clone();
            self.spawn(async move { source.call(Op::Diff(path)).await }, window, cx, move |this, result, _, _| {
                if this.round != round { return; }
                let Some(entry) = this.work.files.get_mut(index) else { return };
                let seq = entry.patch.seq;
                let mut cut = false;
                entry.patch.finish(seq, result.map_err(|error| failure(&error)).map(|value| {
                    cut = value.get("truncated").and_then(Value::as_bool).unwrap_or(false);
                    parse_patch(value.get("diff").and_then(Value::as_str).unwrap_or("")).into_iter().next().unwrap_or_default()
                }));
                this.work.truncated |= cut;
                this.work.refresh(index);
            });
        }
    }

    fn load_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.log.start();
        let (source, limit) = (self.source.clone(), self.log_limit);
        self.spawn(async move { source.call(Op::Log(limit)).await }, window, cx, move |this, result, _, _| {
            if let Ok(value) = &result { this.ahead_behind = (value.get("ahead").and_then(Value::as_i64), value.get("behind").and_then(Value::as_i64)); }
            this.log.finish(seq, result.map(|value| commits(&value)).map_err(|error| failure(&error)));
        });
        cx.notify();
    }

    fn more_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.log_limit = (self.log_limit * 2).min(LOG_MAX);
        self.load_log(window, cx);
    }

    fn set_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.pane = pane;
        if pane == Pane::History && self.log.value.is_none() && !self.log.loading { self.load_log(window, cx); }
        cx.notify();
    }

    /// O commit abre na própria aba, com o diff inteiro dele (o `diff-full` do backend).
    fn open_commit(&mut self, commit: Commit, window: &mut Window, cx: &mut Context<Self>) {
        let mut state = Remote::default();
        let seq = state.start();
        let hash = commit.hash.clone();
        self.opened = Some(Opened { commit, review: Review::new(), state });
        self.pane = Pane::Commit;
        let source = self.source.clone();
        let owner = hash.clone();
        self.spawn(async move { source.call(Op::CommitDiff(hash)).await }, window, cx, move |this, result, _, _| {
            let Some(opened) = this.opened.as_mut().filter(|o| o.commit.hash == owner) else { return };
            match result {
                Ok(value) => {
                    let text = value.get("diff").and_then(Value::as_str).unwrap_or("");
                    opened.review.truncated = value.get("truncated").and_then(Value::as_bool).unwrap_or(false);
                    opened.review.files = parse_patch(text).into_iter().map(|patch| {
                        let code = if patch.created { "A" } else if patch.deleted { "D" } else { "M" }.to_owned();
                        let mut state = Remote::default();
                        let seq = state.start();
                        state.finish(seq, Ok(patch.clone()));
                        Entry { path: patch.path, code, patch: state, folded: false }
                    }).collect();
                    opened.review.reset();
                    opened.state.finish(seq, Ok(()));
                }
                Err(error) => { opened.state.finish(seq, Err(failure(&error))); }
            }
        });
        cx.notify();
    }

    fn close_commit(&mut self, cx: &mut Context<Self>) {
        self.opened = None;
        if self.pane == Pane::Commit { self.pane = Pane::History; }
        cx.notify();
    }

    fn start(&mut self, what: String) -> bool {
        if self.busy.is_some() { return false; }
        (self.busy, self.error, self.output) = (Some(what), None, None);
        true
    }

    fn ask_discard(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        chrome::confirm_alert(window, cx, tr("git_discard_title").replace("{path}", &name), tr("git_discard_body").replace("{path}", &path),
            tr("git_discard_ok"), ButtonVariant::Danger, move |window, cx| {
                let _ = weak.update(cx, |this, cx| this.discard(path.clone(), window, cx));
                true
            });
    }

    fn discard(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.start(path.clone()) { return; }
        let (source, target) = (self.source.clone(), path.clone());
        self.spawn(async move { source.call(Op::Discard(target)).await }, window, cx, move |this, result, window, cx| {
            this.busy = None;
            match result {
                Ok(_) => this.output = Some(tr("git_discarded").replace("{path}", &path)),
                Err(error) => this.error = Some(failure(&error)),
            }
            // Falhou ou não, o disco pode ter mudado: a lista relê.
            this.load(window, cx);
        });
        cx.notify();
    }

    fn ask_discard_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<String> = self.work.files.iter().map(|e| e.path.clone()).collect();
        if paths.is_empty() { return; }
        let weak = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("git_discard_all_title").replace("{n}", &paths.len().to_string()), tr("git_discard_all_body"),
            tr("git_discard_ok"), ButtonVariant::Danger, move |window, cx| {
                let _ = weak.update(cx, |this, cx| this.discard_all(paths.clone(), window, cx));
                true
            });
    }

    /// Um descarte por arquivo, pela mesma rota do botão da linha; o que falhar aparece com o caminho.
    fn discard_all(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.start("discard-all".into()) { return; }
        let source = self.source.clone();
        self.spawn(async move {
            let mut failed = Vec::new();
            for path in paths {
                if let Err(error) = source.call(Op::Discard(path.clone())).await { failed.push(format!("{path}: {}", failure(&error))); }
            }
            failed
        }, window, cx, |this, failed, window, cx| {
            this.busy = None;
            if failed.is_empty() { this.output = Some(tr("git_discarded_all")); } else { this.error = Some(failed.join("\n")); }
            this.load(window, cx);
        });
        cx.notify();
    }

    /// Com o amend ligado e a caixa vazia, a mensagem do último commit entra para ser reescrita.
    fn set_amend(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.amend = on;
        if on && self.message.read(cx).value().trim().is_empty() {
            let source = self.source.clone();
            self.spawn(async move { source.call(Op::LastMessage).await }, window, cx, |this, result, window, cx| {
                let Some(text) = result.ok().and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned)) else { return };
                if this.amend && this.message.read(cx).value().trim().is_empty() {
                    this.message.update(cx, |input, cx| input.set_value(text, window, cx));
                }
            });
        }
        cx.notify();
    }

    fn picked(&self) -> Vec<String> {
        self.work.files.iter().filter(|e| self.chosen.contains(&e.path)).map(|e| e.path.clone()).collect()
    }

    fn can_commit(&self, cx: &App) -> bool {
        self.busy.is_none() && !self.message.read(cx).value().trim().is_empty() && (self.amend || !self.picked().is_empty())
    }

    /// Commit dos marcados; com `push`, o push só depois do commit gravado. Amend nunca empurra: exigiria --force.
    fn commit(&mut self, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_commit(cx) || !self.start("commit".into()) { return; }
        let (source, amend) = (self.source.clone(), self.amend);
        let op = Op::Commit { message: self.message.read(cx).value().trim().to_owned(), paths: self.picked(), amend };
        let push = push && !amend;
        self.spawn(async move {
            let committed = source.call(op).await;
            let pushed = match (&committed, push) { (Ok(_), true) => Some(source.call(Op::Push).await), _ => None };
            (committed, pushed)
        }, window, cx, |this, (committed, pushed), window, cx| {
            this.busy = None;
            match committed {
                Err(error) => { this.error = Some(failure(&error)); return; }
                Ok(value) => {
                    let mut lines = vec![output(&value).unwrap_or_else(|| tr("git_committed"))];
                    match pushed {
                        Some(Ok(value)) => lines.push(output(&value).unwrap_or_else(|| tr("git_pushed"))),
                        Some(Err(error)) => this.error = Some(tr("git_push_failed").replace("{reason}", &failure(&error))),
                        None => {}
                    }
                    this.output = Some(lines.join("\n"));
                    this.amend = false;
                    this.message.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }
            this.load(window, cx);
        });
        cx.notify();
    }

    /// Fetch, pull ou push avulsos; depois relê a lista e o à frente/atrás.
    fn sync(&mut self, kind: Sync, window: &mut Window, cx: &mut Context<Self>) {
        if !self.start(kind.key().into()) { return; }
        let source = self.source.clone();
        let op = match kind { Sync::Fetch => Op::Fetch, Sync::Pull => Op::Pull, Sync::Push => Op::Push };
        self.spawn(async move { source.call(op).await }, window, cx, move |this, result, window, cx| {
            this.busy = None;
            match result {
                // Fetch e pull recusados pelo git voltam 200 com `ok: false` e o motivo na saída.
                Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(false) => this.error = Some(output(&value).map(|o| without_hints(&o))
                    .filter(|o| !o.is_empty()).unwrap_or_else(|| tr("git_sync_failed").replace("{op}", &kind.label()))),
                Ok(value) => this.output = Some(output(&value).unwrap_or_else(|| tr(kind.done()))),
                Err(error) => this.error = Some(without_hints(&failure(&error))),
            }
            this.load(window, cx);
        });
        cx.notify();
    }

    // ── Branches ──

    fn open_picker(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = open;
        self.branch_error = None;
        if open {
            let query = self.branch_query.clone();
            window.defer(cx, move |window, cx| query.update(cx, |input, cx| { input.set_value("", window, cx); input.focus(window, cx); }));
        }
        cx.notify();
    }

    /// Troca pelo caminho da barra lateral: árvore suja pergunta se guarda no stash antes; limpa troca direto.
    fn pick_branch(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.ok() else { return };
        if repo.current.as_deref() == Some(branch.as_str()) || self.busy.is_some() { return; }
        self.picker = false;
        if !repo.dirty { self.switch(branch, false, window, cx); return; }
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let run = |id: &'static str, label: String, stash: bool| {
                let (weak, branch) = (weak.clone(), branch.clone());
                Button::new(id).label(label).when(stash, |b| b.primary()).on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    let _ = weak.update(cx, |this, cx| this.switch(branch.clone(), stash, window, cx));
                })
            };
            let line = |label: String, text: String| div().child(div().font_weight(FontWeight::SEMIBOLD).child(label))
                .child(div().text_color(theme::muted()).child(text));
            popup::dialog(dialog).w(px(460.)).title(tr("sidebar_dirty_title"))
                .child(div().flex().flex_col().gap(px(10.)).text_sm()
                    .child(div().font_family(theme::MONO).child(format!("→ {branch}")))
                    .child(div().child(tr("sidebar_dirty_body")))
                    .child(line(tr("sidebar_stash_and_switch"), tr("sidebar_stash_help")))
                    .child(line(tr("sidebar_switch_anyway"), tr("sidebar_carry_help"))))
                .footer(div().flex().justify_end().gap_2()
                    .child(Button::new("git-dirty-cancel").label(tr("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(run("git-dirty-anyway", tr("sidebar_switch_anyway"), false))
                    .child(run("git-dirty-stash", tr("sidebar_stash_and_switch"), true)))
                .on_ok(super::machines::enter_to_focused)
        });
    }

    /// O `git_result` da barra lateral pela fonte do painel: mesmas frases, mesma notificação por sessão; depois relê.
    /// Stash recusado para aqui: trocar sem ter guardado levaria as mudanças junto.
    fn switch(&mut self, branch: String, stash: bool, window: &mut Window, cx: &mut Context<Self>) {
        let waiting = if stash { tr("sidebar_stashing") } else { tr("sidebar_checking_out").replace("{n}", &branch) };
        if !self.start(waiting.clone()) { return; }
        window.push_notification(git_note(&self.name, NotificationType::Info, waiting), cx);
        let source = self.source.clone();
        self.spawn(async move {
            let failed = |error: &Failure| (NotificationType::Error, tr("sidebar_checkout_failed").replace("{n}", &Hangar::fetch_failure(error)));
            if stash {
                match source.call(Op::Stash).await {
                    Ok(value) if value.get("ok").and_then(Value::as_bool) != Some(true) => return (NotificationType::Error,
                        tr("sidebar_checkout_failed").replace("{n}", &output(&value).unwrap_or_else(|| tr("sidebar_stash_failed")))),
                    Err(error) => return failed(&error),
                    Ok(_) => {}
                }
            }
            match source.call(Op::Checkout(branch.clone())).await {
                Ok(_) => (NotificationType::Success, tr(if stash { "sidebar_switched_stashed" } else { "sidebar_switched" }).replace("{n}", &branch)),
                Err(error) => failed(&error),
            }
        }, window, cx, |this, (kind, text), window, cx| {
            this.busy = None;
            window.push_notification(git_note(&this.name, kind, text), cx);
            this.load(window, cx);
        });
        cx.notify();
    }

    /// Cria a partir da atual e já troca (`switch -c`): muda a branch sem mexer na árvore, então não pede stash.
    fn create_branch(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.start(branch.clone()) { return; }
        let (source, name) = (self.source.clone(), branch.clone());
        self.spawn(async move { source.call(Op::CreateBranch(name)).await }, window, cx, move |this, result, window, cx| {
            this.busy = None;
            match result {
                Ok(_) => {
                    this.picker = false;
                    window.push_notification(git_note(&this.name, NotificationType::Success, tr("git_branch_created").replace("{n}", &branch)), cx);
                    this.load(window, cx);
                }
                Err(error) => this.branch_error = Some(failure(&error)),
            }
        });
        cx.notify();
    }

    fn branch_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.branch_query.read(cx).value().trim().to_owned();
        if query.is_empty() { return; }
        let Some(repo) = self.repo.ok() else { return };
        let exact = repo.branches.iter().chain(&repo.remotes).find(|b| **b == query).cloned();
        match exact {
            Some(branch) => self.pick_branch(branch, window, cx),
            None => {
                let first = repo.branches.iter().chain(&repo.remotes).find(|b| b.to_lowercase().contains(&query.to_lowercase())).cloned();
                match first { Some(branch) => self.pick_branch(branch, window, cx), None => self.create_branch(query, window, cx) }
            }
        }
    }

    fn render_picker(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.branch_query.read(cx).value().trim().to_owned();
        let needle = query.to_lowercase();
        let repo = self.repo.ok().cloned().unwrap_or_default();
        let busy = self.busy.is_some();
        let matches = |b: &&String| needle.is_empty() || b.to_lowercase().contains(&needle);
        // A atual primeiro, depois a ordem do backend (mais recentes); remotas sem local por último.
        let mut locals: Vec<&String> = repo.branches.iter().filter(matches).collect();
        locals.sort_by_key(|b| repo.current.as_deref() != Some(b.as_str()));
        let remotes: Vec<&String> = repo.remotes.iter().filter(matches).collect();
        let exists = repo.branches.iter().chain(&repo.remotes).any(|b| *b == query);
        let row = |branch: &String, tag: Option<String>, cx: &mut Context<Self>| {
            let current = repo.current.as_deref() == Some(branch.as_str());
            let target = branch.clone();
            popup::row(SharedString::from(format!("git-branch-{branch}")), current).disabled(busy)
                .child(div().w_full().flex().items_center().gap(px(10.))
                    .child(chrome::small_icon(IconName::GitBranch, 14., if current { theme::accent() } else { theme::faint() }))
                    .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(13.)).child(branch.clone()))
                    .when_some(tag, |el, tag| el.child(div().flex_shrink_0().text_size(px(10.)).text_color(theme::faint()).child(tag))))
                .on_click(cx.listener(move |this: &mut Self, _, window, cx| this.pick_branch(target.clone(), window, cx)))
                .into_any_element()
        };
        let mut list = div().id("git-branch-list").max_h(px(300.)).overflow_y_scroll().flex().flex_col().gap(px(1.));
        for branch in &locals {
            let tag = (repo.current.as_deref() == Some(branch.as_str())).then(|| tr("git_current"));
            list = list.child(row(branch, tag, cx));
        }
        if !remotes.is_empty() {
            list = list.child(popup::title(tr("git_remotes"), None));
            for branch in &remotes { list = list.child(row(branch, Some(tr("git_remote_badge")), cx)); }
        }
        let empty = locals.is_empty() && remotes.is_empty();
        let valid = !query.is_empty() && !exists && !query.contains(char::is_whitespace);
        div().w(px(320.)).p(px(popup::INSET)).flex().flex_col().gap_1()
            .child(div().p_1().child(Input::new(&self.branch_query).small().cleanable(true).aria_label(tr("git_branch_search"))))
            .when(empty, |el| el.child(div().px_2().py_1().text_sm().text_color(theme::muted()).child(tr("git_no_branch"))))
            .when(!empty, |el| el.child(list))
            .when(valid, |el| {
                let name = query.clone();
                el.child(popup::separator()).child(popup::row("git-branch-create", false).disabled(busy)
                    .child(div().w_full().flex().items_center().gap(px(10.))
                        .child(chrome::small_icon(IconName::Plus, 14., theme::accent()))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).child(tr("git_branch_create").replace("{n}", &query))))
                    .on_click(cx.listener(move |this, _, window, cx| this.create_branch(name.clone(), window, cx))))
            })
            .when_some(self.busy.clone().filter(|_| self.picker), |el, what| el.child(div().px_2().pb_1().text_xs().text_color(theme::muted()).child(what)))
            .when_some(self.branch_error.clone(), |el, error| el.child(div().mx_2().mb_1().pt_1().border_t_1().border_color(theme::border())
                .text_size(px(11.)).text_color(theme::danger()).whitespace_normal().child(error)))
            .into_any_element()
    }

    // ── Desenho ──

    fn render_header(&self, entry: &Entry, file: usize, work: bool, cx: &mut Context<Self>) -> AnyElement {
        let (tag, tag_color) = code_tag(&entry.code);
        let patch = entry.patch.ok();
        let path = entry.path.clone();
        let (fold, pick, drop) = (path.clone(), path.clone(), path.clone());
        let id = |what: &str| SharedString::from(format!("git-{what}-{path}"));
        let busy = self.busy.is_some();
        let toggle = cx.listener(move |this: &mut Self, _, _, cx| {
            let review = if work { &mut this.work } else { match this.opened.as_mut() { Some(o) => &mut o.review, None => return } };
            let Some(index) = review.files.iter().position(|e| e.path == fold) else { return };
            review.files[index].folded = !review.files[index].folded;
            review.refresh(index);
            cx.notify();
        });
        div().h(px(FILE_H)).w_full().flex_none().flex().items_center().gap_2().px_3().bg(theme::hover().opacity(0.5))
            .when(file > 0, |el| el.border_t_1().border_color(theme::border()))
            .when(work, |el| el.child(Checkbox::new(id("pick")).checked(self.chosen.contains(&entry.path))
                .accessibility_label(tr("git_include").replace("{path}", &entry.path))
                .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                    if *checked { this.chosen.insert(pick.clone()); } else { this.chosen.remove(&pick); }
                    cx.notify();
                }))))
            .child(div().id(id("fold")).flex_1().min_w_0().h_full().flex().items_center().gap_2().cursor_pointer().on_click(toggle)
                .child(chrome::small_icon(if entry.folded { IconName::ChevronRight } else { IconName::ChevronDown }, 13., theme::muted().opacity(0.7)))
                .child(div().w(px(12.)).flex_shrink_0().font_family(theme::MONO).text_size(px(11.)).text_color(tag_color).child(tag))
                .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(12.)).text_color(theme::muted()).child(entry.path.clone()))
                .when_some(patch, |el, p| el
                    .child(div().flex_shrink_0().font_family(theme::MONO).text_size(px(11.)).text_color(theme::success()).child(format!("+{}", p.added)))
                    .child(div().flex_shrink_0().font_family(theme::MONO).text_size(px(11.)).text_color(theme::removed()).child(format!("−{}", p.removed)))))
            .when(work, |el| el.child(chrome::icon_button(id("discard"), IconName::Undo2, tr("git_discard_hint"), cx).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| this.ask_discard(drop.clone(), window, cx)))))
            .into_any_element()
    }

    fn render_row(&mut self, work: bool, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let review = if work { &self.work } else { match self.opened.as_ref() { Some(o) => &o.review, None => return div().into_any_element() } };
        let Some(row) = review.rows.get(ix).copied() else { return div().into_any_element() };
        match row {
            Row::Header(file) => {
                let entry = &review.files[file];
                // Emprestado: o cabeçalho usa `cx.listener`, e a entrada é lida antes.
                let entry = Entry { path: entry.path.clone(), code: entry.code.clone(), folded: entry.folded, patch: clone_remote(&entry.patch) };
                self.render_header(&entry, file, work, cx)
            }
            Row::Body(file, n) => match review.files[file].patch.ok().and_then(|p| p.body.get(n)) {
                Some(Body::Notice(text)) => notice_row(text.clone(), theme::faint()),
                Some(Body::Hunk(text)) => hunk_row(text),
                Some(Body::Line { kind: Kind::Meta, text, .. }) => notice_row(text.clone(), theme::faint()),
                Some(Body::Line { kind, old, new, text }) => line_row(*kind, *old, *new, text.clone(), false),
                None => div().into_any_element(),
            },
            Row::Status(file) => match &review.files[file].patch.value {
                None => notice_row(tr("git_diff_loading").into(), theme::faint()),
                Some(Err(reason)) => notice_row(reason.clone().into(), theme::warning()),
                Some(Ok(_)) => notice_row(tr("git_diff_empty").into(), theme::faint()),
            },
        }
    }

    /// Faixa de resumo do Zeron: quantos arquivos, +/−, e o aviso de patch cortado.
    fn summary(&self, label: String, review: &Review) -> Div {
        let (added, removed) = review.totals();
        div().h(px(38.)).flex_none().flex().items_center().gap(px(10.)).px_4().border_b_1().border_color(theme::border())
            .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme::muted()).child(label))
            .child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::success()).child(format!("+{added}")))
            .child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::removed()).child(format!("−{removed}")))
            .child(div().flex_1())
            .when(review.truncated, |el| el.child(div().flex_none().px(px(6.)).py(px(2.)).rounded(px(4.)).bg(theme::warning().opacity(0.08))
                .text_size(px(10.)).text_color(theme::warning().opacity(0.75)).child(tr("git_partial"))))
    }

    fn diff_list(&self, work: bool, cx: &mut Context<Self>) -> AnyElement {
        let state = if work { self.work.list.clone() } else { match self.opened.as_ref() { Some(o) => o.review.list.clone(), None => return div().into_any_element() } };
        div().relative().flex_1().min_h_0().overflow_hidden()
            .child(list(state, cx.processor(move |this: &mut Self, ix: usize, _, cx| this.render_row(work, ix, cx))).size_full())
            .into_any_element()
    }

    fn render_changes(&self, repo: &Repo, cx: &mut Context<Self>) -> AnyElement {
        let count = repo.files.len();
        let body = if count == 0 {
            div().flex_1().flex().items_center().justify_center().text_size(px(12.)).text_color(theme::faint()).child(tr("git_clean")).into_any_element()
        } else {
            let label = if count == 1 { tr("git_uncommitted_1") } else { tr("git_uncommitted").replace("{n}", &count.to_string()) };
            let all: Vec<String> = repo.files.iter().map(|(p, _)| p.clone()).collect();
            let picked = self.picked().len();
            div().flex_1().min_h_0().flex().flex_col()
                .child(self.summary(label, &self.work)
                    .child(div().text_size(px(11.)).text_color(theme::faint()).child(tr("git_picked").replace("{n}", &picked.to_string()).replace("{total}", &count.to_string())))
                    .child(Button::new("git-pick-all").ghost().xsmall().label(tr("git_all"))
                        .on_click(cx.listener(move |this, _, _, cx| { this.chosen = all.iter().cloned().collect(); cx.notify(); })))
                    .child(Button::new("git-pick-none").ghost().xsmall().label(tr("git_none"))
                        .on_click(cx.listener(|this, _, _, cx| { this.chosen.clear(); cx.notify(); })))
                    .child(Button::new("git-discard-all").ghost().xsmall().icon(IconName::Undo2).label(tr("git_discard_all"))
                        .text_color(theme::danger()).loading(self.busy.as_deref() == Some("discard-all")).disabled(self.busy.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.ask_discard_all(window, cx)))))
                .child(self.diff_list(true, cx))
                .into_any_element()
        };
        div().size_full().flex().flex_col().child(body).child(self.render_commit_bar(cx)).into_any_element()
    }

    /// Barra de commit do Hangar (o Zeron não tem): mensagem, amend, Commit e Commit e push, colada embaixo do diff.
    fn render_commit_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let can = self.can_commit(cx);
        let committing = self.busy.as_deref() == Some("commit");
        div().flex_none().flex().gap_3().p_3().border_t_1().border_color(theme::border())
            .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                .child(div().id("git-message").child(Textarea::new(&self.message).aria_label(tr("git_message")).disabled(self.busy.is_some())))
                .child(Checkbox::new("git-amend").label(tr("git_amend")).checked(self.amend)
                    .on_change(cx.listener(|this, checked: &bool, window, cx| this.set_amend(*checked, window, cx)))))
            .child(div().w(px(170.)).flex_shrink_0().flex().flex_col().gap_2()
                .when(!self.amend, |el| el.child(Button::new("git-commit-push").small().primary().w_full().label(tr("git_commit_push")).disabled(!can)
                    .on_click(cx.listener(|this, _, window, cx| this.commit(true, window, cx)))))
                .child(Button::new("git-commit").small().w_full().label(if committing { tr("git_committing") } else { tr("git_commit") }).disabled(!can)
                    .on_click(cx.listener(|this, _, window, cx| this.commit(false, window, cx)))))
            .into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let commits = match &self.log.value {
            None => return popup::skeleton("git-log-loading", 8).into_any_element(),
            Some(Err(reason)) => return div().id("git-log-failed").p_4().flex().flex_col().items_start().gap_3().role(Role::Alert)
                .child(div().text_sm().text_color(theme::warning()).whitespace_normal().child(reason.clone()))
                .child(Button::new("git-log-retry").small().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| this.load_log(window, cx))))
                .into_any_element(),
            Some(Ok(list)) if list.is_empty() => return div().size_full().flex().items_center().justify_center().text_size(px(12.))
                .text_color(theme::faint()).child(tr("git_no_commits")).into_any_element(),
            Some(Ok(list)) => list.clone(),
        };
        let remotes = self.repo.ok().map(|r| r.remotes.clone()).unwrap_or_default();
        let more = commits.len() >= self.log_limit && self.log_limit < LOG_MAX;
        let loading = self.log.loading;
        let count = commits.len() + more as usize;
        let head = div().h(px(24.)).flex_none().flex().items_center().gap_3().px_3().border_b_1().border_color(theme::border())
            .text_size(px(9.5)).text_color(theme::faint())
            .child(div().flex_1().child(tr("git_col_commit"))).child(div().w(px(AUTHOR_W)).child(tr("git_col_author")))
            .child(div().w(px(DATE_W)).child(tr("git_col_date"))).child(div().w(px(SHA_W)).child("SHA"));
        let rows = uniform_list("git-log", count, cx.processor(move |_: &mut Self, range: std::ops::Range<usize>, _, cx| {
            range.map(|n| match commits.get(n) {
                Some(commit) => {
                    let open = commit.clone();
                    let refs = commit.refs.iter().take(2).map(|r| ref_badge(r, &remotes)).collect::<Vec<_>>();
                    let extra = commit.refs.len().saturating_sub(2);
                    div().id(SharedString::from(format!("git-commit-{}", commit.hash))).h(px(COMMIT_H)).w_full().flex().items_center().gap_3().px_3()
                        .cursor_pointer().hover(|el| el.bg(theme::hover()))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_commit(open.clone(), window, cx)))
                        .child(div().flex_1().min_w_0().flex().items_center().gap_2()
                            .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme::text())
                                .child(if commit.subject.is_empty() { tr("git_no_subject") } else { commit.subject.clone() }))
                            .children(refs)
                            .when(extra > 0, |el| el.child(div().text_size(px(10.)).text_color(theme::faint()).child(format!("+{extra}"))))
                            .when(commit.local, |el| el.child(unpushed_badge())))
                        .child(div().w(px(AUTHOR_W)).flex_shrink_0().truncate().text_size(px(11.)).text_color(theme::muted()).child(commit.author.clone()))
                        .child(div().w(px(DATE_W)).flex_shrink_0().text_size(px(11.)).text_color(theme::muted()).child(date(commit.ts)))
                        .child(div().w(px(SHA_W)).flex_shrink_0().font_family(theme::MONO).text_size(px(10.5)).text_color(theme::faint()).child(commit.short.clone()))
                        .into_any_element()
                }
                None => div().h(px(COMMIT_H)).w_full().flex().items_center().justify_center()
                    .child(Button::new("git-log-more").small().outline().loading(loading).disabled(loading)
                        .label(if loading { tr("git_loading") } else { tr("git_more") })
                        .on_click(cx.listener(|this, _, window, cx| this.more_log(window, cx))))
                    .into_any_element(),
            }).collect::<Vec<_>>()
        })).flex_1();
        div().size_full().flex().flex_col().child(head).child(rows).into_any_element()
    }

    fn render_commit(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(opened) = self.opened.as_ref() else { return div().into_any_element() };
        let commit = &opened.commit;
        let meta = div().flex_none().flex().flex_col().gap_1().px_4().py_3().border_b_1().border_color(theme::border())
            .child(div().flex().items_center().gap_2()
                .child(div().px(px(6.)).rounded(px(4.)).bg(theme::hover()).font_family(theme::MONO).text_size(px(10.5)).text_color(theme::muted()).child(commit.short.clone()))
                .child(div().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(commit.subject.clone()))
                .when(commit.local, |el| el.child(unpushed_badge())))
            .when(!commit.body.trim().is_empty(), |el| el.child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(commit.body.trim().to_owned())))
            .child(div().text_size(px(11.)).text_color(theme::faint()).child(format!("{} · {} · {}", commit.author, date(commit.ts), commit.hash)));
        let body = match &opened.state.value {
            None => popup::skeleton("git-commit-loading", 6).into_any_element(),
            Some(Err(reason)) => div().id("git-commit-failed").p_4().role(Role::Alert).text_sm().text_color(theme::warning()).whitespace_normal()
                .child(reason.clone()).into_any_element(),
            Some(Ok(())) if opened.review.files.is_empty() => div().flex_1().flex().items_center().justify_center().text_size(px(12.))
                .text_color(theme::faint()).child(tr("git_empty_commit")).into_any_element(),
            Some(Ok(())) => {
                let n = opened.review.files.len();
                let label = if n == 1 { tr("git_commit_files_1") } else { tr("git_commit_files").replace("{n}", &n.to_string()) };
                div().flex_1().min_h_0().flex().flex_col().child(self.summary(label, &opened.review)).child(self.diff_list(false, cx)).into_any_element()
            }
        };
        div().size_full().flex().flex_col().child(meta).child(body).into_any_element()
    }
}

fn clone_remote(remote: &Remote<Patch>) -> Remote<Patch> {
    Remote { value: remote.value.clone(), loading: remote.loading, seq: remote.seq }
}

impl GitPanel {
    /// Seletor de branch do Zeron: chip com o nome e a seta, lista com busca num popover.
    fn branch_picker(&self, max_w: f32, cx: &mut Context<Self>) -> Popover {
        let repo = self.repo.ok();
        let branch = repo.and_then(|r| r.current.clone());
        let dirty = repo.is_some_and(|r| r.dirty);
        let panel = cx.entity();
        let chip = Button::new("git-branch").ghost().small().disabled(repo.is_none())
            .child(div().min_w_0().flex().items_center().gap_1()
                .child(chrome::small_icon(IconName::GitBranch, 14., theme::muted()))
                .child(div().max_w(px(max_w)).truncate().font_family(theme::MONO).text_size(px(12.)).child(branch.unwrap_or_else(|| "—".into())))
                .when(dirty, |el| el.child(div().text_color(theme::warning()).child("*")))
                .child(chrome::small_icon(IconName::ChevronDown, 12., theme::faint())));
        Popover::new("git-branch-picker").anchor(Anchor::TopLeft).open(self.picker)
            .on_open_change({ let panel = panel.clone(); move |open, window, cx| panel.update(cx, |this, cx| this.open_picker(*open, window, cx)) })
            .trigger(chip)
            .content(move |_, _, cx| panel.update(cx, |this, cx| this.render_picker(cx)))
    }

    /// Commits a trazer (pull) ou a enviar (push); só com upstream e acima de zero.
    fn sync_count(&self, kind: Sync) -> Option<i64> {
        match kind { Sync::Fetch => None, Sync::Pull => self.ahead_behind.1, Sync::Push => self.ahead_behind.0 }.filter(|n| *n > 0)
    }

    /// Fetch, Pull e Push: no diálogo com rótulo e o contador dentro; na aba estreita só o ícone, contador no canto.
    fn sync_buttons(&self, compact: bool, cx: &mut Context<Self>) -> Div {
        let disabled = self.busy.is_some() || self.repo.ok().is_none();
        let mut row = div().flex_shrink_0().flex().items_center().gap(px(if compact { 2. } else { 6. }));
        for kind in [Sync::Fetch, Sync::Pull, Sync::Push] {
            let id = SharedString::from(format!("git-{}{}", if compact { "side-" } else { "" }, kind.key()));
            let running = self.busy.as_deref() == Some(kind.key());
            let count = self.sync_count(kind);
            let color = if kind == Sync::Pull { theme::warning() } else { theme::accent() };
            let tip = match count {
                Some(n) => format!("{} · {}", kind.hint(), tr(if kind == Sync::Pull { "git_behind" } else { "git_ahead" }).replace("{n}", &n.to_string())),
                None => kind.hint(),
            };
            let click = cx.listener(move |this, _, window, cx| this.sync(kind, window, cx));
            row = row.child(if compact {
                div().relative().flex_shrink_0()
                    .child(chrome::icon_button(id, kind.icon(), tip, cx).loading(running).disabled(disabled).on_click(click))
                    .when_some(count.filter(|_| !running), |el, n| el.child(div().absolute().top(px(-3.)).right(px(-4.)).h(px(14.)).min_w(px(14.))
                        .px(px(3.)).flex().items_center().justify_center().rounded_full().bg(color).font_family(theme::MONO).text_size(px(9.5))
                        .text_color(theme::background()).child(n.to_string())))
                    .into_any_element()
            } else {
                Button::new(id).small().outline().icon(kind.icon()).label(kind.label()).tooltip(tip).loading(running).disabled(disabled)
                    .when_some(count, |button, n| button.child(div().px(px(5.)).rounded_full().bg(color.opacity(0.14)).font_family(theme::MONO)
                        .text_size(px(11.)).text_color(color).child(n.to_string())))
                    .on_click(click)
                    .into_any_element()
            });
        }
        row
    }

    /// A aba Git do painel direito: branch, arquivos alterados com a marca do commit e a barra de commit empilhada. O diff
    /// não cabe na largura do painel; ele e o histórico abrem no diálogo grande.
    fn render_compact(&mut self, expand: Rc<dyn Fn(&mut Window, &mut App)>, cx: &mut Context<Self>) -> AnyElement {
        let open = expand.clone();
        let header = div().flex_shrink_0().flex().items_center().gap_1().px_3().pb_2()
            .child(div().flex_1().min_w_0().flex().child(self.branch_picker(150., cx)))
            .child(self.sync_buttons(true, cx))
            .child(chrome::icon_button("git-side-reload", IconName::RefreshCw, tr("git_reload"), cx).disabled(self.repo.loading)
                .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))))
            .child(chrome::icon_button("git-side-expand", IconName::Maximize, tr("git_expand"), cx)
                .on_click(move |_, window, cx| open(window, cx)));
        let note = |text: String| div().flex_1().flex().items_center().justify_center().px_4().text_size(px(12.)).text_color(theme::faint())
            .whitespace_normal().child(text).into_any_element();
        let files = self.repo.ok().map(|r| r.files.clone());
        let body = match &self.repo.value {
            None => popup::skeleton("git-side-loading", 5).into_any_element(),
            Some(Err(reason)) => div().id("git-side-failed").p_4().flex().flex_col().items_start().gap_3().role(Role::Alert)
                .child(div().text_sm().text_color(theme::warning()).whitespace_normal().child(reason.clone()))
                .child(Button::new("git-side-retry").small().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| this.load(window, cx))))
                .into_any_element(),
            Some(Ok(repo)) if repo.files.is_empty() => note(tr("git_clean")),
            Some(Ok(repo)) => {
                let count = repo.files.len();
                let label = if count == 1 { tr("git_uncommitted_1") } else { tr("git_uncommitted").replace("{n}", &count.to_string()) };
                let all: Vec<String> = repo.files.iter().map(|(p, _)| p.clone()).collect();
                let busy = self.busy.is_some();
                let rows = repo.files.iter().map(|(path, code)| {
                    let (tag, tag_color) = code_tag(code);
                    let counts = self.work.files.iter().find(|e| &e.path == path).and_then(|e| e.patch.ok()).map(|p| (p.added, p.removed));
                    let (name, dir) = match path.rsplit_once('/') { Some((dir, name)) => (name.to_owned(), Some(dir.to_owned())), None => (path.clone(), None) };
                    let (pick, drop) = (path.clone(), path.clone());
                    let open = expand.clone();
                    let id = |what: &str| SharedString::from(format!("git-side-{what}-{path}"));
                    div().h(px(30.)).w_full().flex_shrink_0().flex().items_center().gap_2().px_3().hover(|el| el.bg(theme::hover()))
                        .child(Checkbox::new(id("pick")).checked(self.chosen.contains(path))
                            .accessibility_label(tr("git_include").replace("{path}", path))
                            .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                                if *checked { this.chosen.insert(pick.clone()); } else { this.chosen.remove(&pick); }
                                cx.notify();
                            })))
                        .child(div().id(id("open")).flex_1().min_w_0().h_full().flex().items_center().gap_2().cursor_pointer()
                            .tooltip({ let full = path.clone(); move |window, cx| gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx) })
                            .on_click(move |_, window, cx| open(window, cx))
                            .child(div().w(px(12.)).flex_shrink_0().font_family(theme::MONO).text_size(px(11.)).text_color(tag_color).child(tag))
                            .child(div().min_w_0().truncate().text_size(px(12.)).text_color(theme::text()).child(name))
                            .when_some(dir, |el, dir| el.child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(theme::faint()).child(dir)))
                            .when_some(counts, |el, (a, r)| el
                                .child(div().ml_auto().flex_shrink_0().font_family(theme::MONO).text_size(px(10.5)).text_color(theme::success()).child(format!("+{a}")))
                                .child(div().flex_shrink_0().font_family(theme::MONO).text_size(px(10.5)).text_color(theme::removed()).child(format!("−{r}")))))
                        .child(chrome::icon_button(id("discard"), IconName::Undo2, tr("git_discard_hint"), cx).xsmall().disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| this.ask_discard(drop.clone(), window, cx))))
                }).collect::<Vec<_>>();
                div().flex_1().min_h_0().flex().flex_col()
                    .child(div().flex_shrink_0().flex().items_center().gap_1().px_3().pb_1().text_size(px(11.)).text_color(theme::faint())
                        .child(div().flex_1().min_w_0().truncate().child(format!("{label} · {}",
                            tr("git_picked").replace("{n}", &self.picked().len().to_string()).replace("{total}", &count.to_string()))))
                        .child(Button::new("git-side-all").ghost().xsmall().label(tr("git_all"))
                            .on_click(cx.listener(move |this, _, _, cx| { this.chosen = all.iter().cloned().collect(); cx.notify(); })))
                        .child(Button::new("git-side-none").ghost().xsmall().label(tr("git_none"))
                            .on_click(cx.listener(|this, _, _, cx| { this.chosen.clear(); cx.notify(); }))))
                    .child(div().id("git-side-files").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().children(rows))
                    .into_any_element()
            }
        };
        let can = self.can_commit(cx);
        let committing = self.busy.as_deref() == Some("commit");
        let commit_bar = div().flex_shrink_0().flex().flex_col().gap_2().p_3().border_t_1().border_color(theme::border())
            .child(div().id("git-side-message").child(Textarea::new(&self.message).aria_label(tr("git_message")).disabled(self.busy.is_some())))
            .child(Checkbox::new("git-side-amend").label(tr("git_amend")).checked(self.amend)
                .on_change(cx.listener(|this, checked: &bool, window, cx| this.set_amend(*checked, window, cx))))
            .child(div().flex().gap_2()
                .child(Button::new("git-side-commit").small().flex_1().label(if committing { tr("git_committing") } else { tr("git_commit") }).disabled(!can)
                    .on_click(cx.listener(|this, _, window, cx| this.commit(false, window, cx))))
                .when(!self.amend, |el| el.child(Button::new("git-side-commit-push").small().primary().flex_1().label(tr("git_commit_push")).disabled(!can)
                    .on_click(cx.listener(|this, _, window, cx| this.commit(true, window, cx))))));
        div().size_full().flex().flex_col().pt_1()
            .child(header)
            .child(body)
            .when(files.is_some_and(|f| !f.is_empty()), |el| el.child(commit_bar))
            .when_some(self.error.clone(), |el, error| el.child(div().id("git-side-error").flex_shrink_0().px_3().pb_2().role(Role::Alert)
                .text_xs().text_color(theme::danger()).whitespace_normal().child(error)))
            .when_some(self.output.clone(), |el, text| el.child(div().id("git-side-output").flex_shrink_0().mx_3().mb_2().max_h(px(72.)).overflow_y_scroll()
                .px_2().py_1().rounded(px(8.)).bg(theme::inset()).border_1().border_color(theme::border())
                .font_family(theme::MONO).text_xs().text_color(theme::muted()).whitespace_normal().child(text)))
            .into_any_element()
    }
}

impl Render for GitPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(expand) = self.expand.clone() { return self.render_compact(expand, cx); }
        let height = (f32::from(window.viewport_size().height) * 0.78).min(MAX_H);
        let repo = self.repo.ok().cloned();
        let header = div().flex().items_center().gap_2().pr(px(36.)).h(px(28.))
            .child(div().flex_shrink_0().max_w(px(260.)).truncate().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(self.title.clone()))
            .child(self.branch_picker(220., cx))
            .child(div().flex_1())
            .child(self.sync_buttons(false, cx))
            .child(chrome::icon_button("git-reload", IconName::RefreshCw, tr("git_reload"), cx).disabled(self.repo.loading)
                .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))));
        let count = |n: usize| div().ml_1().px(px(6.)).rounded_full().bg(theme::hover()).font_family(theme::MONO).text_size(px(10.)).text_color(theme::muted()).child(n.to_string());
        let changes = repo.as_ref().map(|r| r.files.len()).filter(|n| *n > 0);
        let mut tabs = vec![
            Tab::new().label(tr("git_changes")).prefix(chrome::small_icon(IconName::List, 12., theme::muted())).when_some(changes, |tab, n| tab.suffix(count(n))),
            Tab::new().label(tr("git_history")).prefix(chrome::small_icon(IconName::GitBranch, 12., theme::muted())),
        ];
        if let Some(opened) = self.opened.as_ref() {
            tabs.push(Tab::new().label(opened.commit.short.clone()).prefix(chrome::small_icon(IconName::Hash, 12., theme::muted()))
                .suffix(Button::new("git-commit-close").ghost().xsmall().icon(IconName::Close).accessibility_label(tr("git_commit_close"))
                    .on_click(cx.listener(|this, _, _, cx| { cx.stop_propagation(); this.close_commit(cx); }))));
        }
        let selected = match self.pane { Pane::Changes => 0, Pane::History => 1, Pane::Commit => 2 };
        let bar = TabBar::new("git-tabs").underline().small().selected_index(selected).children(tabs)
            .on_click(cx.listener(|this, index: &usize, window, cx| {
                let pane = match index { 0 => Pane::Changes, 1 => Pane::History, _ => Pane::Commit };
                this.set_pane(pane, window, cx);
            }));
        let body = match (&self.repo.value, self.pane) {
            (_, Pane::History) => self.render_history(cx),
            (_, Pane::Commit) => self.render_commit(cx),
            (None, _) => popup::skeleton("git-loading", 6).into_any_element(),
            (Some(Err(reason)), _) => div().id("git-failed").p_4().flex().flex_col().items_start().gap_3().role(Role::Alert)
                .child(div().text_sm().text_color(theme::warning()).whitespace_normal().child(reason.clone()))
                .child(Button::new("git-retry").small().label(tr("retry")).on_click(cx.listener(|this, _, window, cx| this.load(window, cx))))
                .into_any_element(),
            (Some(Ok(repo)), Pane::Changes) => { let repo = repo.clone(); self.render_changes(&repo, cx) }
        };
        // Rodapé: o erro e a saída do último comando; a saída tem teto e rola.
        div().h(px(height)).flex().flex_col().gap_2()
            .child(header)
            .child(div().flex_shrink_0().border_b_1().border_color(theme::border()).child(bar))
            .child(div().flex_1().min_h_0().rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset()).overflow_hidden().child(body))
            .when_some(self.error.clone(), |el, error| el.child(div().id("git-error").flex_shrink_0().role(Role::Alert).text_sm()
                .text_color(theme::danger()).whitespace_normal().child(error)))
            .when_some(self.output.clone(), |el, text| el.child(div().id("git-output").flex_shrink_0().max_h(px(72.)).overflow_y_scroll()
                .px_2().py_1().rounded(px(8.)).bg(theme::inset()).border_1().border_color(theme::border())
                .font_family(theme::MONO).text_xs().text_color(theme::muted()).whitespace_normal().child(text)))
            .into_any_element()
    }
}

impl Hangar {
    /// O git da sessão aberta, lido do disco ou pelas rotas; `expand` faz dele a lista estreita da aba.
    fn new_git_panel(&self, expand: Option<Rc<dyn Fn(&mut Window, &mut App)>>, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<GitPanel>> {
        let (Some(api), Some(session)) = (self.session_api(), self.selected.clone()) else { return None };
        let title = folder_name(&session).unwrap_or_else(|| session.name.clone());
        let runtime = self.runtime.clone();
        // Servidor nesta máquina e a pasta existe aqui: o git roda direto no disco, pela pasta real da sessão.
        let here = session.cwd.as_deref().filter(|_| api.is_loopback()).and_then(|cwd| self.local_dirs.get(cwd).cloned().flatten());
        let source = match here { Some(cwd) => Source::Local(Arc::new(cwd)), None => Source::Remote(api, session.name.clone()) };
        let panel = cx.new(|cx| GitPanel::new(source, runtime, session.name.clone(), title, expand, window, cx));
        panel.update(cx, |panel, cx| panel.load(window, cx));
        Some(panel)
    }

    /// A faixa do compositor e o "Git" do menu abrem o diálogo grande, com diff e histórico inteiros; a aba Git do
    /// painel direito fica como a vista curta.
    pub(super) fn open_git_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_git_dialog(Pane::Changes, window, cx);
    }

    /// A aba Git do painel é a vista curta, sem histórico; o histórico só existe no diálogo grande.
    pub(super) fn open_git_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_git_dialog(Pane::History, window, cx);
    }

    /// Diff e histórico no tamanho do painel expandido; ao fechar, a aba relê o que o diálogo pode ter mudado.
    fn open_git_dialog(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.new_git_panel(None, window, cx) else { return };
        if pane != Pane::Changes { panel.update(cx, |panel, cx| panel.set_pane(pane, window, cx)); }
        let side = self.side.git.as_ref().map(|(_, panel)| panel.downgrade());
        show_dialog(panel, move |window, cx| { if let Some(side) = side.as_ref() { let _ = side.update(cx, |panel, cx| panel.load(window, cx)); } },
            window, cx);
    }

    /// O painel da aba Git desta sessão, criado na primeira vez que a aba aparece.
    pub(super) fn side_git(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<GitPanel>> {
        let owner = self.session_owner()?;
        if let Some((key, panel)) = &self.side.git && key == &owner { return Some(panel.clone()); }
        let weak = cx.entity().downgrade();
        let expand: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |window, cx| { let _ = weak.update(cx, |this, cx| this.open_git_dialog(Pane::Changes, window, cx)); });
        let panel = self.new_git_panel(Some(expand), window, cx)?;
        self.side.git = Some((owner, panel.clone()));
        Some(panel)
    }
}

fn show_dialog(panel: Entity<GitPanel>, closed: impl Fn(&mut Window, &mut App) + 'static, window: &mut Window, cx: &mut App) {
    let closed = Rc::new(closed);
    window.open_dialog(cx, move |dialog, window, _| {
        let width = (f32::from(window.viewport_size().width) * 0.92).min(MAX_W);
        let closed = closed.clone();
        popup::dialog(dialog).w(px(width)).margin_top(px(24.)).on_ok(super::machines::enter_to_focused).child(panel.clone())
            .on_close(move |_, window, cx| closed(window, cx))
    });
}

/// O mesmo diálogo para uma pasta desta máquina ainda sem sessão; `closed` relê quem abriu.
pub(super) fn open_folder_git(cwd: std::path::PathBuf, title: String, runtime: Arc<Runtime>, closed: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window, cx: &mut App) {
    // Pasta escolhida pode ser subpasta do repositório.
    let root = local::toplevel(&cwd).unwrap_or(cwd);
    let panel = cx.new(|cx| GitPanel::new(Source::Local(Arc::new(root)), runtime, title.clone(), title, None, window, cx));
    panel.update(cx, |panel, cx| panel.load(window, cx));
    show_dialog(panel, closed, window, cx);
}

#[cfg(test)]
mod tests {
    use super::{Body, Kind, parse_patch, without_hints};

    #[test]
    fn refusal_keeps_the_reason_without_git_hints() {
        let text = "hint: Diverging branches can't be fast-forwarded\nhint:\nhint:   git rebase\nfatal: Not possible to fast-forward, aborting.\n";
        assert_eq!(without_hints(text), "fatal: Not possible to fast-forward, aborting.");
        assert_eq!(without_hints("hint: só dica\n"), "");
    }

    #[test]
    fn patch_numbers_lines_and_marks_new_files() {
        let files = parse_patch("diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -3,3 +3,3 @@ fn x\n ctx\n-old\n+new\n\
            diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+hi\n");
        assert_eq!(files.len(), 2);
        assert_eq!((files[0].path.as_str(), files[0].added, files[0].removed), ("a.rs", 1, 1));
        assert!(matches!(files[0].body[1], Body::Line { kind: Kind::Context, old: Some(3), new: Some(3), .. }));
        assert!(matches!(files[0].body[2], Body::Line { kind: Kind::Del, old: Some(4), new: None, .. }));
        assert!(matches!(files[0].body[3], Body::Line { kind: Kind::Add, old: None, new: Some(4), .. }));
        assert!(files[1].created && matches!(files[1].body[0], Body::Notice(_)));
    }
}
