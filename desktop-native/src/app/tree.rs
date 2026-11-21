//! Árvore de arquivos da sessão no painel direito, no molde do `files/` do Zeron (MIT, ver `LICENSE-ZERON`):
//! busca por nome ou conteúdo no topo, pastas com seta, ícone por tipo, o nome na cor do estado no git e o clique abrindo o arquivo
//! no visor. Servidor nesta máquina lê o disco e acompanha as mudanças com um vigia; servidor de fora, pelas rotas.
mod cited;
mod ops;
mod source;

use super::*;
use super::panes::Area;
use std::path::Path;
use source::{FileSource, Listing};
use ops::item_menu;
pub(super) use source::{read_local, resolve};

const ROW_H: f32 = 26.;
const INDENT: f32 = 14.;
/// Espera o disco sossegar antes de reler: um `git checkout` ou um build disparam centenas de avisos seguidos.
const SETTLE: Duration = Duration::from_millis(300);

enum Dir { Loading, Ready(Listing), Failed(String) }

#[derive(Clone)]
enum RowKind { Entry { dir: bool, mark: Option<char> }, Loading, Empty, Failed(String), Truncated }

#[derive(Clone)]
struct Row { path: String, name: String, depth: usize, kind: RowKind }

#[derive(Clone, Copy, PartialEq)]
enum SearchMode { Names, Contents }

#[derive(Clone)]
struct SearchHit { path: String, line: Option<u32>, text: Option<String> }

pub(super) struct Tree {
    pub open: bool,
    owner: Option<SessionOwner>,
    generation: u64,
    source: Option<FileSource>,
    /// Outra pasta desta máquina escolhida no painel; vale só para a sessão em que foi escolhida.
    custom_root: Option<(Option<SessionOwner>, PathBuf)>,
    root_error: Option<String>,
    picking: Option<Failure>,
    dirs: HashMap<String, Dir>,
    expanded: HashSet<String>,
    selected: Option<String>,
    /// Arquivo que o visor pediu para mostrar na árvore; espera a origem e as pastas de cima serem lidas.
    reveal: Option<String>,
    rows: Vec<Row>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    search: Entity<InputState>,
    query: String,
    search_mode: SearchMode,
    search_generation: u64,
    /// `None` enquanto a busca roda.
    results: Option<Result<(Vec<SearchHit>, bool), String>>,
    active: usize,
    search_task: Option<Task<()>>,
    reloading: bool,
    reload_again: bool,
    watcher: Option<notify::RecommendedWatcher>,
    watched: HashSet<PathBuf>,
    watch_error: bool,
    _watch_task: Option<Task<()>>,
    cited: cited::CitedView,
    /// Seção dos citados aberta, e com a lista inteira em vez das primeiras.
    cited_open: bool,
    cited_more: bool,
    /// O tipo escolhido nos chips da seção; volta para todos ao trocar de sessão.
    cited_kind: cited::CitedKind,
    /// O campo do diálogo de nome (renomear, novo arquivo ou pasta) enquanto ele está aberto.
    name_dialog: Option<Subscription>,
}

impl Tree {
    pub fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(activity::web("arq_buscar")));
        cx.subscribe(&search, |this: &mut Hangar, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.tree_search_changed(cx),
            _ => {}
        }).detach();
        Self { open: false, owner: None, generation: 0, source: None, custom_root: None, root_error: None, picking: None, dirs: HashMap::new(), expanded: HashSet::new(),
            selected: None, reveal: None, rows: Vec::new(), scroll: UniformListScrollHandle::new(), focus: cx.focus_handle(), search,
            query: String::new(), search_mode: SearchMode::Names, search_generation: 0, results: None, active: 0, search_task: None, reloading: false, reload_again: false,
            watcher: None, watched: HashSet::new(), watch_error: false, _watch_task: None, cited: Default::default(),
            cited_open: true, cited_more: false, cited_kind: Default::default(), name_dialog: None }
    }

    /// Linhas à vista: a raiz e, dentro de cada pasta aberta, o que ela tem, com a linha de estado quando falta conteúdo.
    fn rebuild(&mut self) {
        fn walk(tree: &Tree, dir: &str, depth: usize, out: &mut Vec<Row>) {
            let status = |kind: RowKind| Row { path: format!("{dir}\u{0}"), name: String::new(), depth, kind };
            match tree.dirs.get(dir) {
                None | Some(Dir::Loading) => out.push(status(RowKind::Loading)),
                Some(Dir::Failed(reason)) => out.push(status(RowKind::Failed(reason.clone()))),
                Some(Dir::Ready(listing)) if listing.entries.is_empty() => out.push(status(RowKind::Empty)),
                Some(Dir::Ready(listing)) => {
                    for entry in &listing.entries {
                        out.push(Row { path: entry.path.clone(), name: entry.name.clone(), depth,
                            kind: RowKind::Entry { dir: entry.dir, mark: entry.mark } });
                        if entry.dir && tree.expanded.contains(&entry.path) { walk(tree, &entry.path, depth + 1, out); }
                    }
                    if listing.truncated { out.push(status(RowKind::Truncated)); }
                }
            }
        }
        let mut rows = Vec::new();
        walk(self, "", 0, &mut rows);
        self.rows = rows;
    }

    fn entry(&self, path: &str) -> Option<(bool, bool)> {
        self.rows.iter().find_map(|row| match row.kind {
            RowKind::Entry { dir, .. } if row.path == path => Some((dir, self.expanded.contains(path))),
            _ => None,
        })
    }

    /// Pastas lidas agora: a raiz e as abertas.
    fn loaded_dirs(&self) -> Vec<String> {
        let mut dirs = vec![String::new()];
        dirs.extend(self.expanded.iter().filter(|dir| self.dirs.contains_key(*dir)).cloned());
        dirs
    }

    pub fn local_root(&self, owner: &Option<SessionOwner>) -> Option<PathBuf> {
        self.source.as_ref().filter(|_| &self.owner == owner).and_then(FileSource::root).map(Path::to_path_buf)
    }

    fn custom_root(&self, owner: &Option<SessionOwner>) -> Option<&Path> {
        self.custom_root.as_ref().filter(|(of, _)| of == owner).map(|(_, root)| root.as_path())
    }

    /// Fora da pasta da sessão o visor recebe o caminho inteiro: assim a gravação vai pela rota de arquivo de fora, que
    /// recusa o que não foi citado, e nunca por cima do arquivo de mesmo nome na pasta da sessão.
    fn file_path(&self, path: String) -> String {
        match self.custom_root(&self.owner) { Some(root) => root.join(&path).to_string_lossy().into_owned(), None => path }
    }
}

/// A pasta do usuário, para os caminhos com `~/`; no Windows não há `HOME`.
pub(super) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

/// As pastas acima de um caminho relativo, da raiz para dentro: `a/b/c.md` dá `a` e `a/b`.
fn parent_dirs(path: &str) -> Vec<String> {
    path.match_indices('/').map(|(ix, _)| path[..ix].to_owned()).filter(|dir| !dir.is_empty()).collect()
}

fn mark_color(mark: char) -> Hsla {
    match mark { '?' | 'A' => theme::success(), 'R' => theme::accent(), 'D' | 'U' => theme::danger(), _ => theme::warning() }
}

impl Hangar {
    /// A aba Arquivos entrou ou saiu de cena. O foco só vai para a árvore quando veio de um gesto (`window`).
    pub(super) fn show_tree(&mut self, open: bool, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if let (true, Some(window)) = (open, window) { self.tree.focus.focus(window, cx); }
        if self.tree.open == open { return; }
        self.tree.open = open;
        if !open { self.tree_stop(); self.tree.reveal = None; }
        cx.notify();
    }

    /// Solta o vigia e as leituras em voo; a próxima abertura relê do zero.
    fn tree_stop(&mut self) {
        let tree = &mut self.tree;
        tree.owner = None;
        tree.generation += 1;
        tree.search_generation += 1;
        tree.results = None;
        (tree.watcher, tree._watch_task, tree.search_task) = (None, None, None);
        tree.watched.clear();
        tree.cited.reset();
        tree.cited_kind = Default::default();
        (tree.source, tree.picking, tree.reloading, tree.reload_again, tree.watch_error) = (None, None, false, false, false);
        tree.dirs.clear();
        tree.expanded.clear();
        tree.rows.clear();
        tree.selected = None;
    }

    /// Chamado no desenho do painel: sessão trocada recomeça a árvore e escolhe de novo de onde ler.
    fn tree_sync(&mut self, cx: &mut Context<Self>) {
        let owner = self.session_owner();
        if !self.tree.open || self.tree.owner == owner { return; }
        self.tree_stop();
        self.tree.owner = owner;
        let (Some(api), Some(session)) = (self.session_api(), self.selected.as_ref()) else { return };
        let (name, cwd, generation) = (session.name.clone(), session.cwd.clone(), self.tree.generation);
        let custom = self.tree.custom_root(&self.tree.owner).map(Path::to_path_buf);
        let pick = self.runtime.spawn(async move { match custom {
            Some(root) => FileSource::Local { root },
            None => FileSource::pick(api, name, cwd).await,
        } });
        cx.spawn(async move |this, cx| {
            let Ok(source) = pick.await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.tree.generation != generation { return; }
                this.tree.source = Some(source);
                this.tree_reload(cx);
                this.tree_begin_search(cx);
            });
        }).detach();
    }

    /// Relê a raiz e as pastas abertas (e, no disco, as marcas do git e o que o vigia acompanha).
    fn tree_reload(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.tree.source.clone() else { return };
        if self.tree.reloading { self.tree.reload_again = true; return; }
        self.tree.reloading = true;
        let dirs = self.tree.loaded_dirs();
        self.tree_fetch(source, dirs, true, cx);
    }

    fn tree_fetch(&mut self, source: FileSource, dirs: Vec<String>, reload: bool, cx: &mut Context<Self>) {
        for dir in &dirs { self.tree.dirs.entry(dir.clone()).or_insert(Dir::Loading); }
        self.tree.rebuild();
        let generation = self.tree.generation;
        let job = self.runtime.spawn(async move {
            let listings = source.list(dirs).await;
            let watch = match source.root().map(Path::to_path_buf) {
                Some(root) => tokio::task::spawn_blocking(move || FileSource::watch_dirs(&root)).await.ok(),
                None => None,
            };
            (listings, watch)
        });
        cx.spawn(async move |this, cx| {
            let Ok((listings, watch)) = job.await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.tree.generation != generation { return; }
                for (dir, listing) in listings {
                    // Pasta que sumiu do disco fecha em vez de ficar aberta com erro.
                    if dir.is_empty() || this.tree.expanded.contains(&dir) {
                        this.tree.dirs.insert(dir, match listing { Ok(listing) => Dir::Ready(listing), Err(error) => Dir::Failed(Self::fetch_failure(&error)) });
                    }
                }
                if let Some(dirs) = watch { this.tree_watch(dirs, cx); }
                this.tree.rebuild();
                this.tree_apply_reveal(cx);
                this.redraw(Area::Side, cx);
                if reload {
                    this.tree.reloading = false;
                    if std::mem::take(&mut this.tree.reload_again) { this.tree_reload(cx); }
                }
            });
        }).detach();
    }

    /// Acompanha exatamente estas pastas; o primeiro pedido também liga o vigia e a fila que relê a árvore.
    fn tree_watch(&mut self, dirs: Vec<PathBuf>, cx: &mut Context<Self>) {
        use notify::Watcher as _;
        if self.tree.watcher.is_none() {
            let (tx, rx) = async_channel::bounded::<()>(1);
            let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok_and(|event| !matches!(event.kind, notify::EventKind::Access(_))) { let _ = tx.try_send(()); }
            });
            let Ok(watcher) = watcher else { self.tree.watch_error = true; return };
            self.tree.watcher = Some(watcher);
            let generation = self.tree.generation;
            self.tree._watch_task = Some(cx.spawn(async move |this, cx| {
                while rx.recv().await.is_ok() {
                    cx.background_executor().timer(SETTLE).await;
                    while rx.try_recv().is_ok() {}
                    let alive = this.update(cx, |this, cx| {
                        if this.tree.generation == generation { this.tree_reload(cx); }
                    });
                    if alive.is_err() { return; }
                }
            }));
        }
        let wanted: HashSet<PathBuf> = dirs.into_iter().collect();
        let Some(watcher) = self.tree.watcher.as_mut() else { return };
        for gone in self.tree.watched.difference(&wanted) { let _ = watcher.unwatch(gone); }
        let mut failed = false;
        for new in wanted.difference(&self.tree.watched) {
            failed |= watcher.watch(new, notify::RecursiveMode::NonRecursive).is_err();
        }
        self.tree.watched = wanted;
        self.tree.watch_error = failed;
    }

    fn tree_toggle_dir(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.tree.expanded.remove(&path) {
            self.tree.expanded.insert(path.clone());
            if !matches!(self.tree.dirs.get(&path), Some(Dir::Ready(_))) {
                self.tree.dirs.remove(&path);
                if let Some(source) = self.tree.source.clone() { self.tree_fetch(source, vec![path], false, cx); }
            }
        }
        self.tree.rebuild();
    }

    fn tree_activate(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.tree.selected = Some(path.clone());
        match self.tree.entry(&path) {
            Some((true, _)) => self.tree_toggle_dir(path, cx),
            Some((false, _)) => self.open_file(self.tree.file_path(path), None, window, cx),
            None => {}
        }
        self.redraw(Area::Side, cx);
    }

    /// "Abrir pasta" do visor: abre as pastas até o arquivo e o marca, assim que a árvore tiver de onde ler.
    pub(super) fn tree_reveal_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.tree.reveal = Some(path);
        self.tree_apply_reveal(cx);
    }

    pub(super) fn tree_focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tree.search.update(cx, |input, cx| input.focus(window, cx));
    }

    pub(super) fn find_project_files(&mut self, contents: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection_dialog || window.has_active_dialog(cx) || self.open_read_only()
            || !self.selected.as_ref().is_some_and(|session| session.readable()) { return; }
        self.file_add(window, cx);
        let mode = if contents { SearchMode::Contents } else { SearchMode::Names };
        self.tree_search_mode(mode, window, cx);
        let owner = self.session_owner();
        // O campo precisa estar montado antes de receber o foco.
        cx.on_next_frame(window, move |this, window, cx| {
            if this.session_owner() != owner || !this.tree.open || !this.side.open
                || this.tree.search_mode != mode || window.has_active_dialog(cx) { return; }
            this.tree_focus_search(window, cx);
            this.tree.search.update(cx, |input, cx| input.select_all(window, cx));
        });
        self.redraw(Area::Side, cx);
    }

    fn tree_apply_reveal(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(source)) = (self.tree.reveal.clone(), self.tree.source.clone()) else { return };
        let parents = parent_dirs(&path);
        self.tree.expanded.extend(parents.iter().cloned());
        let unread: Vec<String> = parents.iter().filter(|dir| !self.tree.dirs.contains_key(*dir)).cloned().collect();
        if !unread.is_empty() { self.tree_fetch(source, unread, false, cx); return; }
        // Pasta que falhou também encerra: a linha de erro fica à vista no lugar do arquivo.
        if std::iter::once("").chain(parents.iter().map(String::as_str)).any(|dir| matches!(self.tree.dirs.get(dir), None | Some(Dir::Loading))) { return; }
        self.tree.reveal = None;
        self.tree.selected = Some(path);
        self.tree.rebuild();
        self.tree_reveal();
        self.redraw(Area::Side, cx);
    }

    fn tree_reveal(&self) {
        if let Some(ix) = self.tree.selected.as_ref().and_then(|path| self.tree.rows.iter().position(|row| &row.path == path)) {
            self.tree.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
    }

    /// Setas andam, Enter abre, → entra na pasta e ← volta à pasta de cima, como o Zeron.
    fn tree_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let entries: Vec<&Row> = self.tree.rows.iter().filter(|row| matches!(row.kind, RowKind::Entry { .. })).collect();
        let at = self.tree.selected.as_ref().and_then(|path| entries.iter().position(|row| &row.path == path));
        let selected = self.tree.selected.clone();
        match event.keystroke.key.as_str() {
            "up" | "down" => {
                let next = match (at, event.keystroke.key.as_str()) {
                    (None, _) => 0,
                    (Some(ix), "up") => ix.saturating_sub(1),
                    (Some(ix), _) => (ix + 1).min(entries.len().saturating_sub(1)),
                };
                self.tree.selected = entries.get(next).map(|row| row.path.clone());
            }
            "left" => if let Some(path) = selected {
                if self.tree.entry(&path) == Some((true, true)) { self.tree_toggle_dir(path, cx); }
                else if let Some((parent, _)) = path.rsplit_once('/') { self.tree.selected = Some(parent.to_owned()); }
            },
            "right" => if let Some(path) = selected.filter(|path| self.tree.entry(path) == Some((true, false))) { self.tree_toggle_dir(path, cx); },
            "enter" | "space" => if let Some(path) = selected { self.tree_activate(path, window, cx); },
            "f2" => if let Some(path) = selected.filter(|_| self.tree_writable()) {
                let dir = self.tree.entry(&path).is_some_and(|(dir, _)| dir);
                let target = self.tree_item(&path, dir);
                if target.session.is_some() { self.tree_rename_dialog(target, window, cx); }
            },
            "delete" => if let Some(path) = selected.filter(|_| self.tree_writable()) {
                let dir = self.tree.entry(&path).is_some_and(|(dir, _)| dir);
                let target = self.tree_item(&path, dir);
                if target.session.is_some() { self.tree_confirm_trash(target, window, cx); }
            },
            _ => return,
        }
        cx.stop_propagation();
        self.tree_reveal();
        self.redraw(Area::Side, cx);
    }

    fn tree_collapse_all(&mut self, cx: &mut Context<Self>) {
        self.tree.expanded.clear();
        self.tree.dirs.retain(|dir, _| dir.is_empty());
        self.tree.rebuild();
        self.redraw(Area::Side, cx);
    }

    fn tree_search_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.tree.search.read(cx).value().trim().to_owned();
        if query == self.tree.query { return; }
        self.tree.query = query;
        self.tree_begin_search(cx);
    }

    fn tree_search_mode(&mut self, mode: SearchMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.tree.search_mode == mode { return; }
        self.tree.search_mode = mode;
        let placeholder = if mode == SearchMode::Contents { tr("tree_search_contents_placeholder") } else { activity::web("arq_buscar") };
        self.tree.search.update(cx, |input, cx| { input.set_placeholder(placeholder, window, cx); input.focus(window, cx); });
        self.tree_begin_search(cx);
    }

    fn tree_begin_search(&mut self, cx: &mut Context<Self>) {
        self.tree.search_generation += 1;
        self.tree.active = 0;
        self.tree.results = None;
        self.tree.search_task = None;
        let query = self.tree.query.clone();
        let Some(source) = self.tree.source.clone().filter(|_| !query.is_empty()) else { self.redraw(Area::Side, cx); return };
        let (Some(api), Some(session)) = (self.session_api(), self.selected.as_ref()) else { return };
        let (name, mode, generation) = (session.name.clone(), self.tree.search_mode, self.tree.search_generation);
        let runtime = self.runtime.clone();
        self.tree.search_task = Some(cx.spawn(async move |this, cx| {
            // Espera a digitação parar: cada tecla não vira uma busca.
            cx.background_executor().timer(Duration::from_millis(180)).await;
            let result = runtime.spawn(async move {
                if mode == SearchMode::Names {
                    return source.search(query).await.map(|(paths, truncated)| (paths.into_iter()
                        .map(|path| SearchHit { path, line: None, text: None }).collect(), truncated));
                }
                // Reutiliza a busca do servidor, que ignora binários e respeita o gitignore.
                let value = api.read(&name, &["files", "search"], &[("q", query.as_str()), ("mode", "contents")], 30).await?;
                let hits = value.get("hits").and_then(Value::as_array).ok_or_else(|| Failure::local("invalid_response"))?;
                let hits = hits.iter().map(|hit| Ok(SearchHit {
                    path: hit.get("path").and_then(Value::as_str).ok_or_else(|| Failure::local("invalid_response"))?.to_owned(),
                    line: Some(hit.get("line").and_then(Value::as_u64).and_then(|line| u32::try_from(line).ok()).filter(|line| *line > 0)
                        .ok_or_else(|| Failure::local("invalid_response"))?),
                    text: Some(hit.get("text").and_then(Value::as_str).ok_or_else(|| Failure::local("invalid_response"))?.to_owned()),
                })).collect::<Result<Vec<_>, Failure>>()?;
                Ok((hits, value.get("truncated").and_then(Value::as_bool).unwrap_or(false)))
            }).await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
            let _ = this.update(cx, |this, cx| {
                if generation != this.tree.search_generation { return; }
                this.tree.results = Some(result.map_err(|error| Self::fetch_failure(&error)));
                this.redraw(Area::Side, cx);
            });
        }));
        self.redraw(Area::Side, cx);
    }

    fn tree_step_result(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.tree.results.as_ref().and_then(|r| r.as_ref().ok()).map_or(0, |(hits, _)| hits.len());
        if count == 0 { return; }
        self.tree.active = (self.tree.active as isize + delta).clamp(0, count as isize - 1) as usize;
        self.tree.scroll.scroll_to_item(self.tree.active, ScrollStrategy::Nearest);
        self.redraw(Area::Side, cx);
    }

    fn tree_open_result(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = self.tree.results.as_ref().and_then(|r| r.as_ref().ok()).and_then(|(hits, _)| hits.get(ix)).cloned() else { return };
        self.tree.active = ix;
        self.open_file(self.tree.file_path(hit.path), hit.line, window, cx);
    }

    /// Mostra outra pasta desta máquina no painel; a sessão e o agente continuam na pasta deles.
    fn tree_choose_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let chosen = prompt.await;
            let _ = this.update(cx, |this, cx| match chosen {
                Ok(Ok(None)) => {}
                Ok(Ok(Some(paths))) if !paths.is_empty() => this.tree_set_root(paths.into_iter().next(), cx),
                Ok(Err(error)) => { this.tree.root_error = Some(error.to_string()); this.redraw(Area::Side, cx); }
                _ => { this.tree.root_error = Some(tr("picker_failed")); this.redraw(Area::Side, cx); }
            });
        }).detach();
    }

    fn tree_set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.tree.root_error = None;
        let owner = self.session_owner();
        self.tree.custom_root = match root.map(|root| std::fs::canonicalize(&root).map_err(|error| format!("{}: {error}", root.display()))) {
            Some(Ok(root)) => Some((owner, root)),
            Some(Err(error)) => { self.tree.root_error = Some(error); return self.redraw(Area::Side, cx); }
            None => None,
        };
        // Sem dono, o próximo desenho escolhe de novo de onde ler.
        self.tree_stop();
        self.redraw(Area::Side, cx);
    }

    fn tree_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let row = self.tree.rows.get(ix)?.clone();
        let pad = 8. + row.depth as f32 * INDENT;
        let status = |text: String, color: Hsla| div().pl(px(pad + INDENT + 4.)).pr_2().h_full().flex().items_center()
            .text_size(px(11.)).text_color(color).child(div().truncate().child(text));
        let content = match row.kind {
            RowKind::Loading => status(activity::web("arq_carregando"), theme::faint()).into_any_element(),
            RowKind::Empty => status(tr("tree_empty_folder"), theme::faint()).into_any_element(),
            RowKind::Truncated => status(activity::web("arq_pasta_grande"), theme::faint()).into_any_element(),
            RowKind::Failed(reason) => status(reason, theme::danger()).into_any_element(),
            RowKind::Entry { dir, mark } => {
                let selected = self.tree.selected.as_deref() == Some(row.path.as_str());
                let open = dir && self.tree.expanded.contains(&row.path);
                let path = row.path.clone();
                let menu = item_menu(cx.entity().downgrade(), self.tree_item(&row.path, dir));
                let color = mark.map(mark_color).unwrap_or(if selected { theme::text() } else { theme::text().opacity(0.86) });
                div().id(SharedString::from(format!("tree-{}", row.path))).role(Role::TreeItem).aria_label(row.name.clone())
                    .aria_selected(selected).when(dir, |el| el.aria_expanded(open))
                    .h_full().pl(px(pad)).pr_2().flex().items_center().gap(px(4.)).rounded(px(6.)).cursor_pointer()
                    .map(|el| if selected { el.bg(theme::accent_dim()) } else { el.hover(|el| el.bg(theme::hover())) })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.tree.focus.focus(window, cx);
                        this.tree_activate(path.clone(), window, cx);
                    }))
                    .child(div().size(px(14.)).flex_none().flex().items_center().justify_center()
                        .when(dir, |el| el.child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 11., theme::faint()))))
                    .child(crate::fileicons::tree_icon(&row.name, dir, open))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(color).child(row.name.clone()))
                    // A letra repete a cor: o estado não depende só dela.
                    .when_some(mark, |el, mark| el.child(div().flex_none().font_family(theme::MONO).text_size(px(10.)).text_color(mark_color(mark))
                        .child(if mark == '?' { 'U' } else if mark == 'U' { 'C' } else { mark }.to_string())))
                    .context_menu(menu)
                    .into_any_element()
            }
        };
        // Guias de recuo desenhadas na própria linha, para a lista virtual emendar mesmo com o pai fora da tela.
        Some(div().relative().h(px(ROW_H)).w_full().px_1()
            .children((0..row.depth).map(|level| div().absolute().top_0().bottom_0().left(px(4. + 8. + 7. + level as f32 * INDENT)).w(px(1.)).bg(theme::border())))
            .child(content).into_any_element())
    }

    fn tree_result_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (hits, _) = self.tree.results.as_ref()?.as_ref().ok()?;
        let hit = hits.get(ix)?.clone();
        let path = hit.path.clone();
        let (folder, name) = path.rsplit_once('/').map_or(("", path.as_str()), |(folder, name)| (folder, name));
        let active = ix == self.tree.active;
        let label = hit.line.map_or_else(|| path.clone(), |line| format!("{path}:{line}"));
        let header = div().flex().items_center().gap_1().min_w_0()
            .child(crate::fileicons::tree_icon(name, false, false))
            .child(div().flex_none().max_w(relative(0.6)).truncate().text_size(px(12.)).text_color(theme::text()).child(name.to_owned()))
            .when_some(hit.line, |el, line| el.child(div().flex_none().text_xs().text_color(theme::muted()).child(format!(":{line}"))))
            .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(theme::faint()).child(folder.to_owned()));
        Some(div().id(SharedString::from(format!("tree-hit-{label}"))).role(Role::ListBoxOption).aria_selected(active).aria_label(label)
            .h(px(if hit.text.is_some() { ROW_H * 2. } else { ROW_H })).mx_1().px_2().flex().flex_col().justify_center().gap_1().rounded(px(6.)).cursor_pointer()
            .map(|el| if active { el.bg(theme::accent_dim()) } else { el.hover(|el| el.bg(theme::hover())) })
            .on_click(cx.listener(move |this, _, window, cx| this.tree_open_result(ix, window, cx)))
            .child(header)
            .when_some(hit.text, |el, text| el.child(div().min_w_0().truncate().font_family(theme::MONO).text_xs().text_color(theme::muted()).child(text.trim().to_owned())))
            .into_any_element())
    }

    /// O corpo do painel no modo Arquivos: a busca, os citados na conversa e, embaixo, a pasta (ou os resultados).
    pub(super) fn render_tree(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.tree_sync(cx);
        let mode = self.tree.search_mode;
        let toggle = |id: &'static str, label: String, child: AnyElement, on: bool, to: SearchMode, cx: &mut Context<Self>| Button::new(id)
            .ghost().xsmall().selected(on).tooltip(label.clone()).accessibility_label(label).child(child)
            .on_click(cx.listener(move |this, _, window, cx| this.tree_search_mode(to, window, cx)));
        let modes = div().flex().items_center().gap(px(2.))
            .child(toggle("tree-search-names", activity::web("arq_modo_nomes"),
                div().text_size(px(11.5)).font_weight(FontWeight::MEDIUM).child("Aa").into_any_element(), mode == SearchMode::Names, SearchMode::Names, cx))
            .child(toggle("tree-search-contents", activity::web("arq_modo_conteudo"),
                chrome::small_icon(IconName::Type, 13., theme::muted()).into_any_element(), mode == SearchMode::Contents, SearchMode::Contents, cx));
        let searching = !self.tree.query.is_empty();
        let search = div().flex().items_center().px_3().pt(px(10.)).pb(px(6.))
            .child(div().id("tree-search").flex_1().min_w_0()
                .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.tree_step_result(-1, cx)))
                .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.tree_step_result(1, cx)))
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    if this.tree.query.is_empty() { cx.propagate(); return; }
                    this.tree.search.update(cx, |input, cx| input.set_value("", window, cx));
                }))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "enter" && !this.tree.query.is_empty() { this.tree_open_result(this.tree.active, window, cx); }
                }))
                .child(Input::new(&self.tree.search).small().cleanable(true).aria_label(if mode == SearchMode::Contents {
                    tr("tree_search_contents_placeholder")
                } else { activity::web("arq_buscar") })
                    .prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))
                    .suffix(modes)));
        let cited = self.render_cited_section(cx);
        let custom = self.tree.custom_root(&self.session_owner()).map(|root| root.display().to_string());
        let root_bar = (custom.is_some() || self.tree.root_error.is_some()).then(|| div().flex().flex_col().gap_1().px_3().pb_2()
            .when_some(custom.clone(), |el, root| el.child(div().flex().items_center().gap_2()
                .child(chrome::small_icon(IconName::Folder, 14., theme::accent()))
                .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme::muted()).child(root))
                .child(Button::new("tree-session-folder").ghost().small().icon(IconName::ArrowLeft).label(tr("tree_session_folder"))
                    .on_click(cx.listener(|this, _, _, cx| this.tree_set_root(None, cx))))))
            .when_some(self.tree.root_error.clone(), |el, error| el.child(div().text_xs().text_color(theme::warning()).child(error))));
        // A pasta como cabeçalho de seção, com as ações dela; o ramo é o do git da sessão.
        let branch = custom.is_none().then(|| self.selected.as_ref().and_then(|s| s.branch.clone())).flatten();
        let writable = self.tree_root_writable();
        let project = div().flex().items_center().gap(px(2.)).h(px(30.)).pl(px(14.)).pr_2().mt_1()
            .child(div().min_w(px(48.)).flex_shrink(1.).truncate().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::muted()).child(self.tree_root_label().to_uppercase()))
            .when_some(branch, |el, branch| el.child(div().ml(px(6.)).min_w_0().flex_shrink(8.).truncate().font_family(theme::MONO)
                .text_size(px(11.)).text_color(theme::faint()).child(branch)))
            .child(div().flex_1())
            .when(writable, |el| el
                .child(chrome::icon_button("tree-new-file", IconName::FilePlus, tr("tree_new_file"), cx)
                    .on_click(cx.listener(|this, _, window, cx| this.tree_create_dialog(None, false, window, cx))))
                .child(chrome::icon_button("tree-new-folder", IconName::Folder, tr("tree_new_folder"), cx)
                    .on_click(cx.listener(|this, _, window, cx| this.tree_create_dialog(None, true, window, cx)))))
            .child(chrome::icon_button("tree-collapse", IconName::ChevronsDownUp, activity::web("arq_recolher_tudo"), cx)
                .disabled(self.tree.expanded.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.tree_collapse_all(cx))))
            .child(chrome::icon_button("tree-reload", IconName::RefreshCw, activity::web("arq_recarregar"), cx)
                .disabled(self.tree.source.is_none())
                .on_click(cx.listener(|this, _, _, cx| { this.tree.cited.reset(); this.tree_reload(cx); this.tree_begin_search(cx); })))
            .when(self.session_api().is_some_and(|api| api.is_loopback()), |el| el
                .child(chrome::icon_button("tree-other-folder", IconName::FolderOpen, tr("tree_other_folder"), cx)
                    .on_click(cx.listener(|this, _, window, cx| this.tree_choose_root(window, cx)))));
        let note = |text: String, color: Hsla| div().px_4().py_2().text_xs().text_color(color).child(text).into_any_element();
        let body = if searching {
            match &self.tree.results {
                None => note(activity::web("arq_carregando"), theme::muted()),
                Some(Err(reason)) => note(reason.clone(), theme::warning()),
                Some(Ok((hits, _))) if hits.is_empty() => note(activity::web(if mode == SearchMode::Contents { "arq_sem_conteudo" } else { "arq_sem_nome" }), theme::muted()),
                Some(Ok((hits, truncated))) => div().flex_1().min_h_0().flex().flex_col()
                    .child(uniform_list(if mode == SearchMode::Contents { "tree-content-results" } else { "tree-results" }, hits.len(), cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range.filter_map(|ix| this.tree_result_row(ix, cx)).collect::<Vec<_>>()
                    })).track_scroll(&self.tree.scroll).flex_1())
                    .when(*truncated, |el| el.child(note(activity::web("arq_primeiros_200"), theme::faint())))
                    .into_any_element(),
            }
        } else if self.tree.source.is_none() {
            note(activity::web("arq_carregando"), theme::muted())
        } else {
            div().id("tree").flex_1().min_h_0().track_focus(&self.tree.focus).role(Role::Tree).aria_label(activity::web("arq_aba"))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.tree_key(event, window, cx)))
                .child(uniform_list("tree-rows", self.tree.rows.len(), cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range.filter_map(|ix| this.tree_row(ix, cx)).collect::<Vec<_>>()
                })).track_scroll(&self.tree.scroll).size_full())
                .into_any_element()
        };
        // Só a falha aparece: ler do disco ou pelo servidor é o normal e não ocupa linha.
        let failed = self.tree.source.as_ref().is_some_and(|s| s.is_local()) && self.tree.watch_error;
        div().size_full().flex().flex_col().pt_1()
            .child(search)
            .children(cited)
            .child(project)
            .children(root_bar)
            .child(body)
            .when(failed, |el| el.child(div().flex_shrink_0().px_4().py(px(6.)).border_t_1().border_color(theme::border())
                .text_size(px(11.)).text_color(theme::warning()).child(tr("tree_watch_failed"))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parent_dirs_walk_from_the_root_inward() {
        assert_eq!(super::parent_dirs("docs/decisoes/a.md"), ["docs", "docs/decisoes"]);
        assert!(super::parent_dirs("README.md").is_empty());
    }
}
