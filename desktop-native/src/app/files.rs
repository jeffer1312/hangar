//! Arquivos sobre a conversa, no molde do visor do Zeron (`files/preview.rs`, MIT, ver `LICENSE-ZERON`): abas com o
//! ícone do tipo, trilha do caminho, Markdown com o estilo da conversa, código com cores e imagem. Cada aba conserva
//! seu rascunho e seu pedido.
use super::*;
use gpui_kit::component::input::{Editor, EditorState, Position, RopeExt};
mod navigation;
mod reader;
mod reader_images;
mod lifecycle;
pub(crate) mod grammars;

actions!(file_view, [CloseFile, NextFile, PreviousFile, SaveFile, FindFile, GoToFileLine]);

/// Lado maior da imagem decodificada: cabe numa tela grande sem guardar o original inteiro na memória.
const PICTURE_SIDE: u32 = 4096;
const UNREADABLE_PICTURE: &str = "file_image_unreadable";

pub(super) struct Files {
    owner: Option<SessionOwner>,
    hidden: bool,
    tabs: Vec<FileTab>,
    active: usize,
    serial: u64,
    /// O visor toma a janela: a lista de sessões e o painel direito saem enquanto ele está à vista.
    expanded: bool,
    viewport_width: std::rc::Rc<std::cell::Cell<f32>>,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    _focus_lost: Subscription,
    preview_find: navigation::PreviewFind,
    _refresh_task: Task<()>,
}

struct FileTab {
    id: u64,
    path: String,
    line: Option<u32>,
    content: Option<Result<Document, String>>,
    /// Imagem: mostrada, não editada.
    picture: Option<Picture>,
    reader: reader::ReaderSettings,
    pending_anchor: Option<String>,
}

impl FileTab {
    /// Imagem já à vista: só ela vai para o visor com zoom.
    fn zoomable(&self) -> bool { matches!(self.picture, Some(Picture::Disk(_) | Picture::Ready(_))) }
}

#[derive(serde::Deserialize)]
pub(super) struct Content { path: String, text: String, truncated: bool, digest: Option<String>, #[serde(skip)] external: bool }
impl Content {
    fn editable(&self) -> bool { !self.truncated && self.digest.as_ref().is_some_and(|digest| !digest.is_empty()) }
}
pub(super) enum FileReply {
    Read(u64, Result<Content, Failure>),
    Saved(u64, String, Result<Value, Failure>),
    Picture(u64, Result<Arc<RenderImage>, Failure>),
    Link(u64, String),
    ReaderImageReady(u64),
    Refresh(u64, u64, bool, Result<Content, Failure>),
}

/// Da raiz, direto do disco (servidor nesta máquina); fora dela, pela rota de arquivo citado, que só serve o que a
/// conversa mencionou.
enum Picture { Disk(PathBuf), Loading, Ready(Arc<RenderImage>), Failed(String), Audio, Video }

struct Document {
    editor: Entity<EditorState>,
    base: Content,
    saving: bool,
    dirty: bool,
    saved: Option<Instant>,
    error: Option<String>,
    poll_error: Option<String>,
    markdown: Option<Entity<TextViewState>>,
    reader_images: reader_images::ReaderImages,
    _changed: Subscription,
    _cursor: Subscription,
    _markdown_changed: Option<Subscription>,
    cursor: Position,
    read_seq: u64,
    checking: bool,
    reloading: bool,
    disk_changed: bool,
    close_after_save: bool,
}

impl Document {
    fn editable(&self) -> bool { self.base.editable() }
    fn dirty(&self) -> bool { self.dirty }
}

fn file_failure(error: &Failure) -> String {
    if error.detail.starts_with("erro_arq_") {
        return Hangar::fetch_failure(error);
    }
    match error.status {
        Some(404) => activity::web("erro_arq_inexistente"),
        Some(409) => activity::web("erro_arq_mudou_no_disco"),
        Some(413) => activity::web("erro_arq_grande_demais"),
        Some(415) => activity::web("erro_arq_binario"),
        _ => Hangar::fetch_failure(error),
    }
}

pub(super) fn file_language(path: &str) -> &'static str {
    let file = std::path::Path::new(path);
    if file.file_name().and_then(|s| s.to_str()).is_some_and(|name| matches!(name, "Makefile" | "makefile" | "GNUmakefile")) { return "make"; }
    let extension = file.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "py" | "pyi" => "python",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" | "mdx" => "markdown",
        "sh" | "bash" | "zsh" => "bash",
        "css" => "css",
        "html" | "htm" => "html",
        "vue" => "html",
        "svelte" => "svelte",
        "sql" => "sql",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "java" => "java",
        "cs" => "csharp",
        "kt" | "kts" => "kotlin",
        "go" => "go",
        "rb" => "ruby",
        "lua" => "lua",
        "swift" => "swift",
        "php" => "php",
        "pas" | "pp" | "dpr" | "dpk" | "lpr" | "inc" | "dfm" | "lfm" | "fmx" => "pascal",
        "dart" => "dart",
        "diff" | "patch" => "diff",
        "mk" => "make",
        "zig" => "zig",
        "scala" | "sc" => "scala",
        "ex" | "exs" => "elixir",
        _ => "text",
    }
}

impl Files {
    pub fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        grammars::register();
        let focus = cx.focus_handle();
        let lost = cx.on_focus_lost(window, |this, window, cx| this.files_focus_lost(window, cx));
        Self { owner: None, hidden: false, tabs: Vec::new(), active: 0, serial: 0, expanded: false,
            viewport_width: std::rc::Rc::new(std::cell::Cell::new(1000.)), focus, return_focus: None, _focus_lost: lost, preview_find: navigation::PreviewFind::new(window, cx),
            _refresh_task: lifecycle::watch_files(window, cx) }
    }
}

async fn read_file(api: Api, name: String, mut path: String, candidates: Vec<String>, local: Option<PathBuf>, here: bool) -> Result<Content, Failure> {
    // Servidor nesta máquina (a árvore já provou): caminho da raiz sai do disco, com as mesmas travas.
    if let Some(root) = local.filter(|root| !path.starts_with('/') && !std::path::Path::new(&path).is_absolute() && root.join(&path).exists()) {
        return tokio::task::spawn_blocking(move || super::tree::read_local(&root, &path).map(|read|
            Content { path, text: read.text, truncated: read.truncated, digest: read.digest, external: false }))
            .await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
    }
    // Citado fora da raiz, com o servidor nesta máquina: também do disco, o próprio caminho ou o primeiro candidato que existe.
    // A rota de arquivo citado fica para a sessão de fora.
    // Resolvido inteiro antes de olhar `.git`: symlink não escapa da regra, e o arquivo lido é o de verdade.
    let outside = |path: &str| path.strip_prefix("~/").and_then(|rest| Some(super::tree::home_dir()?.join(rest)))
        .or_else(|| std::path::Path::new(path).is_absolute().then(|| PathBuf::from(path)))
        .and_then(|file| std::fs::canonicalize(file).ok())
        .filter(|file| file.is_file() && !file.components().any(|c| c.as_os_str() == ".git"));
    let found = if here { std::iter::once(&path).chain(&candidates).find_map(|p| outside(p).map(|file| (p.clone(), file))) } else { None };
    // O caminho como foi citado vai no conteúdo: é ele que a rota de gravação confere no transcript.
    if let Some((cited, file)) = found {
        path = cited;
        let (Some(folder), Some(file_name)) = (file.parent().map(std::path::Path::to_path_buf), file.file_name().map(|n| n.to_string_lossy().into_owned()))
            else { return Err(Failure::local("file_missing")) };
        return tokio::task::spawn_blocking(move || {
            let folder = std::fs::canonicalize(&folder).map_err(|_| Failure::local("file_missing"))?;
            super::tree::read_local(&folder, &file_name).map(|read|
                Content { path, text: read.text, truncated: read.truncated, digest: read.digest, external: true })
        }).await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
    }
    let mut resolved = api.act(&name, &["files", "resolver"], Some(json!({"caminhos": [&path]})), false, 30).await?;
    if resolved.get("ok").and_then(|v| v.get(&path)).is_none() && !candidates.is_empty() {
        resolved = api.act(&name, &["files", "resolver"], Some(json!({"caminhos": candidates})), false, 30).await?;
        if let Some(found) = candidates.into_iter().find(|candidate| resolved.get("ok").and_then(|v| v.get(candidate)).is_some()) { path = found; }
    }
    let entry = resolved.get("ok").and_then(|v| v.get(&path)).ok_or_else(|| Failure::local("file_missing"))?;
    let (route, requested) = match entry.get("relativo") {
        Some(Value::String(relative)) => (["files", "read"], relative.as_str()),
        Some(Value::Null) => (["file", "text"], path.as_str()),
        _ => return Err(Failure::local("invalid_response")),
    };
    let mut content: Content = serde_json::from_value(api.read(&name, &route, &[("path", requested)], 30).await?)
        .map_err(|_| Failure::local("invalid_response"))?;
    content.external = route[0] == "file";
    Ok(content)
}

impl Hangar {
    pub(super) fn files_visible(&self) -> bool {
        self.files.owner.is_some() && self.files.owner == self.session_owner() && !self.files.tabs.is_empty() && !self.files.hidden
            && (self.settings.is_none() || self.settings_live())
    }

    /// A imagem da aba vai para o visor da conversa, que tem o zoom; a aba só a mostra encaixada.
    fn zoom_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(key), Some(tab)) = (self.selected_key(), self.files.tabs.get(self.files.active)) else { return };
        if !tab.zoomable() { return; }
        let source = Source::Cited(tab.path.clone());
        self.open_image(key, vec![source], 0, window, cx);
    }

    /// Clique ou Ctrl + roda na imagem abrem o visor com zoom; a roda sozinha continua livre.
    fn picture_view(image: ImageSource, cx: &mut Context<Self>) -> AnyElement {
        div().size_full().p_4().flex().items_center().justify_center()
            .child(div().id("file-picture").max_w_full().max_h_full().flex().cursor_pointer()
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(tr("file_zoom")).build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| this.zoom_picture(window, cx)))
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                    if event.modifiers.control { cx.stop_propagation(); this.zoom_picture(window, cx); }
                }))
                .child(img(image).max_w_full().max_h_full().object_fit(ObjectFit::Contain)))
            .into_any_element()
    }

    pub(super) fn open_file(&mut self, path: String, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        // Arquivos da sessão da outra pessoa são recusados pelo servidor dela.
        if self.open_read_only() { return; }
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return };
        if self.guard_file_owner_change(path.clone(), line, window, cx) { return; }
        if !self.files_visible() {
            let owner = self.session_owner();
            if self.files.owner != owner { for tab in std::mem::take(&mut self.files.tabs) { release(tab, window, cx); } }
            self.files.owner = owner;
            self.files.hidden = false;
            self.files.return_focus = window.focused(cx);
        }
        if let Some(ix) = self.files.tabs.iter().position(|tab| tab.path == path) {
            self.files.active = ix;
            self.files.tabs[ix].line = line;
            self.focus_file(window, cx);
            return;
        }
        self.files.serial += 1;
        let id = self.files.serial;
        let local = self.tree.local_root(&self.session_owner());
        // A leitura de texto recusa binário: imagem vem do disco ou da rota de arquivo citado; áudio toca no player e
        // vídeo abre no programa do sistema.
        let picture = if crate::audio::is_audio(&path) { Some(Picture::Audio) }
            else if crate::audio::is_video(&path) { Some(Picture::Video) }
            else { is_image(&path).then(|| local.as_ref().and_then(|root| super::tree::resolve(root, &path).ok())
                .map_or(Picture::Loading, Picture::Disk)) };
        let fetch_picture = matches!(picture, Some(Picture::Loading));
        let is_picture = picture.is_some();
        let mut reader = reader::ReaderSettings::default();
        if file_language(&path) != "markdown" { reader.set_mode(reader::ReaderMode::Source); }
        self.files.tabs.push(FileTab { id, path: path.clone(), line, content: None, picture, reader, pending_anchor: None });
        self.files.active = self.files.tabs.len() - 1;
        self.focus_file(window, cx);
        let (connection, selection, tx) = (self.connection, None, self.tx.clone());
        if fetch_picture {
            let (api, name, path, tx) = (api.clone(), key.name.clone(), path.clone(), tx.clone());
            self.runtime.spawn(async move {
                let result = api.fetch(&name, &Source::Cited(path)).await.and_then(|bytes|
                    crate::media::decode(&bytes, PICTURE_SIDE, PICTURE_SIDE, None).ok_or_else(|| Failure::local(UNREADABLE_PICTURE)));
                let _ = tx.send(Envelope { connection, selection, payload: Payload::FileView(FileReply::Picture(id, result)) }).await;
            });
        }
        if is_picture { return; }
        // Outros lugares onde o mesmo nome aparece na conversa, se o caminho citado não existir (nome solto, caminho
        // abreviado com "…"): absolutos que terminam nele, inclusive com espaço, e o nome ou o relativo dentro das pastas
        // dos `cd`. A leitura tenta o citado primeiro.
        let mut candidates = Vec::new();
        {
            fn collect(value: &Value, name: &str, out: &mut Vec<String>, dirs: &mut Vec<String>, relatives: &mut Vec<String>) {
                match value {
                    Value::String(text) => {
                        let tail = format!("/{name}");
                        for reference in composer::code_references(text) {
                            if !reference.path.ends_with(&tail) { continue; }
                            let list = if reference.path.starts_with(['/', '~']) { &mut *out } else { &mut *relatives };
                            if !list.contains(&reference.path) { list.push(reference.path); }
                        }
                        for path in composer::spaced_paths(text, &tail) { if !out.contains(&path) { out.push(path); } }
                        for dir in composer::cd_dirs(text) { if !dirs.contains(&dir) { dirs.push(dir); } }
                    }
                    Value::Array(items) => for item in items { collect(item, name, out, dirs, relatives); },
                    Value::Object(items) => for item in items.values() { collect(item, name, out, dirs, relatives); },
                    _ => {},
                }
            }
            let name = composer::basename(&path).to_owned();
            let (mut dirs, mut relatives) = (Vec::new(), vec![name.clone()]);
            for event in self.chat.events.iter().rev() {
                collect(&json!([event.text, event.tool_input, event.result]), &name, &mut candidates, &mut dirs, &mut relatives);
            }
            candidates.retain(|c| *c != path);
            // Os absolutos não podem tomar o teto inteiro: as pastas dos `cd` também precisam de vez.
            candidates.truncate(30);
            for dir in &dirs {
                for relative in &relatives { candidates.push(format!("{dir}/{relative}")); }
            }
            // ponytail: teto fixo; sessão de fora confere todos no backend, um a um.
            candidates.truncate(60);
        }
        let cwd = self.selected.as_ref().and_then(|s| s.cwd.clone());
        self.runtime.spawn(async move {
            // Servidor nesta máquina, mesmo com a aba Arquivos nunca aberta: loopback e a pasta da sessão existe aqui.
            // Túnel para outra máquina também é loopback, e a pasta dela não existe neste disco.
            let here = local.is_some() || (api.is_loopback() && cwd.is_some_and(|cwd| std::path::Path::new(&cwd).is_dir()));
            let result = read_file(api, key.name, path, candidates, local, here).await;
            let _ = tx.send(Envelope { connection, selection, payload: Payload::FileView(FileReply::Read(id, result)) }).await;
        });
    }

    pub(super) fn receive_file_view(&mut self, reply: FileReply, window: &mut Window, cx: &mut Context<Self>) {
        let (id, result) = match reply {
            FileReply::Read(id, result) => (id, result),
            FileReply::Saved(id, text, result) => { self.file_saved(id, text, result, window, cx); return; }
            FileReply::Refresh(id, seq, reload, result) => { self.file_refreshed(id, seq, reload, result, window, cx); return; }
            FileReply::Link(id, url) => { self.navigate_reader_link(id, &url, window, cx); return; }
            FileReply::ReaderImageReady(id) => {
                if let Some(view) = self.files.tabs.iter().find(|tab| tab.id == id).and_then(|tab| tab.content.as_ref()?.as_ref().ok()?.markdown.as_ref()) {
                    view.update(cx, |view, cx| view.invalidate_inline_layout(cx));
                }
                cx.notify();
                return;
            }
            FileReply::Picture(id, result) => {
                let slot = self.files.tabs.iter_mut().find(|tab| tab.id == id).and_then(|tab| tab.picture.as_mut());
                match (slot, result) {
                    (Some(slot), Ok(image)) => *slot = Picture::Ready(image),
                    (Some(slot), Err(error)) => *slot = Picture::Failed(if error.detail == UNREADABLE_PICTURE { tr(UNREADABLE_PICTURE) } else { file_failure(&error) }),
                    // A aba fechou enquanto a imagem chegava.
                    (None, Ok(image)) => cx.drop_image(image, Some(window)),
                    (None, Err(_)) => {}
                }
                cx.notify();
                return;
            }
        };
        let Some(ix) = self.files.tabs.iter().position(|tab| tab.id == id) else { return };
        let path = &self.files.tabs[ix].path;
        self.files.tabs[ix].content = Some(result.map(|content| {
            let editor = cx.new(|cx| EditorState::new(window, cx).language(file_language(path)).default_value(content.text.clone()).soft_wrap(true));
            let changed = cx.subscribe_in(&editor, window, move |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(tab) = this.files.tabs.iter_mut().find(|tab| tab.id == id) {
                        if let Some(Ok(doc)) = &mut tab.content {
                            doc.dirty = doc.editor.read(cx).value().as_ref() != doc.base.text;
                            doc.saved = None;
                            if let Some(view) = &doc.markdown {
                                let text = doc.editor.read(cx).value().to_string();
                                view.update(cx, |view, cx| view.set_text(&text, cx));
                            }
                        }
                    }
                    cx.notify();
                }
            });
            let cursor = cx.observe(&editor, move |this: &mut Self, editor, cx| {
                let position = editor.read(cx).cursor_position();
                if let Some(Ok(doc)) = this.files.tabs.iter_mut().find(|tab| tab.id == id).and_then(|tab| tab.content.as_mut()) {
                    if doc.cursor != position { doc.cursor = position; cx.notify(); }
                }
            });
            let markdown = (file_language(path) == "markdown").then(|| cx.new(|cx| TextViewState::markdown(&content.text, cx).scrollable(true)));
            let markdown_changed = markdown.as_ref().map(|view| {
                let weak = cx.entity().downgrade();
                view.read(cx).list_state().set_scroll_handler(move |_, _, cx| { let _ = weak.update(cx, |_, cx| cx.notify()); });
                cx.observe_in(view, window, move |this: &mut Self, _, window, cx| {
                    this.reveal_pending_anchor(id, window, cx);
                    cx.notify();
                })
            });
            let doc = Document { editor, base: content, saving: false, dirty: false, saved: None, error: None, markdown, _changed: changed,
                reader_images: Default::default(), _cursor: cursor, _markdown_changed: markdown_changed, cursor: Position::new(0, 0), poll_error: None,
                read_seq: 0, checking: false, reloading: false, disk_changed: false, close_after_save: false };
            doc.editor.update(cx, |state, cx| state.set_readonly(!doc.editable(), cx));
            doc
        }).map_err(|error| match error.status {
            Some(415) => activity::web("erro_arq_binario"),
            Some(404) => activity::web("erro_arq_inexistente"),
            None if error.detail == "file_missing" => activity::web("erro_arq_inexistente"),
            _ => file_failure(&error),
        }));
        // A leitura não toma o foco de outra aba, diálogo ou campo aberto enquanto esperava.
        if ix == self.files.active && self.files.focus.contains_focused(window, cx) { self.focus_file(window, cx); }
        cx.notify();
    }

    fn save_file(&mut self, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return };
        let tab = &mut self.files.tabs[self.files.active];
        let Some(Ok(doc)) = &mut tab.content else { return };
        if doc.saving || doc.reloading || !doc.editable() || !doc.dirty() { return; }
        doc.read_seq = doc.read_seq.wrapping_add(1);
        doc.checking = false;
        let text = doc.editor.read(cx).value().to_string();
        let body = json!({"path": doc.base.path, "text": text, "digest": doc.base.digest});
        let route = if doc.base.external { ["file", "text"] } else { ["files", "write"] };
        (doc.saving, doc.saved, doc.error) = (true, None, None);
        // A resposta não pode apagar uma edição feita depois do envio.
        doc.editor.update(cx, |state, cx| state.set_readonly(true, cx));
        let (id, connection, selection, tx) = (tab.id, self.connection, None, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &route, Some(body), false, 30).await;
            let _ = tx.send(Envelope { connection, selection, payload: Payload::FileView(FileReply::Saved(id, text, result)) }).await;
        });
        cx.notify();
    }

    fn file_saved(&mut self, id: u64, text: String, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.files.tabs.iter_mut().find(|tab| tab.id == id) else { return };
        let Some(Ok(doc)) = &mut tab.content else { return };
        doc.saving = false;
        doc.editor.update(cx, |state, cx| state.set_readonly(!doc.editable(), cx));
        match result.and_then(|value| value.get("digest").and_then(Value::as_str).filter(|s| !s.is_empty())
            .map(str::to_owned).ok_or_else(|| Failure::local("invalid_response"))) {
            Ok(digest) => {
                (doc.base.text, doc.base.digest) = (text, Some(digest));
                doc.dirty = false;
                doc.disk_changed = false;
                doc.poll_error = None;
                let saved = Instant::now();
                doc.saved = Some(saved);
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(tab) = this.files.tabs.iter_mut().find(|tab| tab.id == id) {
                            if let Some(Ok(doc)) = &mut tab.content {
                                if doc.saved == Some(saved) { doc.saved = None; cx.notify(); }
                            }
                        }
                    });
                }).detach();
            }
            Err(error) => { doc.error = Some(file_failure(&error)); doc.close_after_save = false; }
        }
        if doc.close_after_save && !doc.dirty { self.finish_close_file(id, window, cx); }
        cx.notify();
    }

    fn discard_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.request_discard_file(window, cx) { return; }
        self.finish_discard_file(window, cx);
    }

    fn finish_discard_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let Some(Ok(doc)) = &mut self.files.tabs[self.files.active].content else { return };
        if doc.saving || doc.reloading { return; }
        doc.editor.update(cx, |state, cx| state.set_value(doc.base.text.clone(), window, cx));
        if let Some(markdown) = &doc.markdown { markdown.update(cx, |state, cx| state.set_text(&doc.base.text, cx)); }
        doc.dirty = false;
        (doc.error, doc.saved) = (None, None);
        cx.notify();
    }

    fn set_reader_mode(&mut self, mode: reader::ReaderMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.files.preview_find.open { self.close_preview_find(window, cx); }
        self.files.tabs[self.files.active].reader.set_mode(mode);
        self.focus_file(window, cx);
        cx.notify();
    }

    fn focus_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.files.preview_find.target.is_some_and(|id| {
            let tab = &self.files.tabs[self.files.active];
            tab.id != id || !tab.reader.mode.reading() || tab.line.is_some()
        }) {
            self.close_preview_find(window, cx);
        }
        let tab = &mut self.files.tabs[self.files.active];
        if tab.line.is_some() { tab.reader.set_mode(reader::ReaderMode::Source); }
        if tab.reader.mode.reading() {
            self.files.focus.focus(window, cx);
            cx.notify();
            return;
        }
        let id = tab.id;
        if let Some(Ok(doc)) = &tab.content {
            let line = tab.line.take();
            let row = doc.editor.update(cx, |state, cx| {
                if let Some(line) = line {
                    let row = line.saturating_sub(1).min(state.text().lines_len().saturating_sub(1) as u32);
                    state.set_cursor_position(Position::new(row, 0), window, cx);
                    Some(row)
                }
                else { state.focus(window, cx); None }
            });
            if let Some(row) = row {
                // O editor novo só tem medida depois do primeiro desenho.
                cx.on_next_frame(window, move |_, window, cx| cx.on_next_frame(window,
                    move |this, window, cx| this.reveal_file_line(id, row, window, cx)));
            }
        } else { self.files.focus.focus(window, cx); }
        cx.notify();
    }

    fn reveal_file_line(&mut self, id: u64, row: u32, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let tab = &self.files.tabs[self.files.active];
        if tab.id != id { return; }
        if let Some(Ok(doc)) = &tab.content {
            doc.editor.update(cx, |state, cx| {
                let position = Position::new(row, 0);
                if state.focus_handle(cx).is_focused(window) && state.cursor_position() == position {
                    state.set_cursor_position(position, window, cx);
                }
            });
        }
    }

    fn files_focus_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        window.focus_lost_restore_target(cx).unwrap_or_else(|| self.root_focus.clone()).focus(window, cx);
    }

    fn step_file(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let n = self.files.tabs.len();
        self.files.active = (self.files.active + if forward { 1 } else { n - 1 }) % n;
        self.focus_file(window, cx);
    }

    fn close_file(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.request_close_file(id, window, cx) { return; }
        self.finish_close_file(id, window, cx);
    }

    fn finish_close_file(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let restore = self.files_visible() && self.files.focus.contains_focused(window, cx);
        if self.files.preview_find.target == Some(id) { self.close_preview_find(window, cx); }
        let Some(ix) = self.files.tabs.iter().position(|tab| tab.id == id) else { return };
        self.stop_audio(&format!("file:{id}"));
        release(self.files.tabs.remove(ix), window, cx);
        if self.files.tabs.is_empty() { if restore { self.restore_file_focus(window, cx); } }
        else {
            if ix < self.files.active { self.files.active -= 1; }
            self.files.active = self.files.active.min(self.files.tabs.len() - 1);
            if restore { self.focus_file(window, cx); }
        }
        cx.notify();
    }

    fn restore_file_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files.return_focus.take().filter(|focus| self.root_focus.contains(focus, window))
            .unwrap_or_else(|| self.root_focus.clone()).focus(window, cx);
    }

    pub(super) fn files_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.files_visible() { return false; }
        if self.files.preview_find.open { self.close_preview_find(window, cx); return true; }
        if let Some(Ok(doc)) = &self.files.tabs[self.files.active].content {
            if doc.editor.read(cx).search_session().open {
                doc.editor.update(cx, |state, cx| { state.close_search(cx); state.focus(window, cx); });
                return true;
            }
        }
        self.files.hidden = true;
        self.restore_file_focus(window, cx);
        cx.notify();
        true
    }

    pub(super) fn files_expanded(&self) -> bool { self.files.expanded && self.files_visible() }

    /// O arquivo volta ao painel e a lista de sessões reaparece.
    pub(super) fn restore_files(&mut self) { self.files.expanded = false; }

    /// O "+" do Zeron abre outro arquivo: aqui, a busca da aba Arquivos do painel.
    pub(super) fn file_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files.expanded = false;
        if !self.side.open { self.toggle_side(cx); }
        self.choose_side_tab(crate::appearance::SideTab::Files, window, cx);
        self.tree_focus_search(window, cx);
    }

    /// Abrir pasta: a árvore do painel abre as pastas até o arquivo e o deixa marcado.
    fn file_reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.files.tabs[self.files.active].path.clone();
        self.files.expanded = false;
        if !self.side.open { self.toggle_side(cx); }
        self.choose_side_tab(crate::appearance::SideTab::Files, window, cx);
        self.tree_reveal_path(path, cx);
    }

    /// Do disco quando a sessão é desta máquina; senão pela rota de arquivo citado.
    fn play_file_audio(&mut self, id: u64, path: String, cx: &mut Context<Self>) {
        let key = format!("file:{id}");
        if let Some(real) = self.file_on_disk(&path) {
            return self.toggle_audio(key, &path, async move {
                tokio::task::spawn_blocking(move || {
                    // O arquivo inteiro vai para a memória antes de decodificar.
                    if std::fs::metadata(&real).map_err(|e| e.to_string())?.len() > 200 * 1024 * 1024 {
                        return Err("arquivo maior que 200 MB".to_owned());
                    }
                    std::fs::read(&real).map_err(|e| e.to_string())
                }).await.map_err(|e| e.to_string())?
            }, cx);
        }
        let (Some(api), Some(session)) = (self.session_api(), self.selected_key()) else { return };
        let uploads = self.uploads_for(&session);
        let name = path.clone();
        self.toggle_audio(key, &name, async move {
            uploads.fetch(&api, &session.name, &Source::Cited(path)).await.map_err(|error| Self::fetch_failure(&error))
        }, cx);
    }

    fn open_file_video(&mut self, cx: &mut Context<Self>) {
        let path = self.files.tabs[self.files.active].path.clone();
        if let Some(real) = self.file_on_disk(&path) { cx.open_with_system(&real); return; }
        let name = composer::basename(&path).to_owned();
        self.keep_file(Source::Cited(path), name, true, cx);
    }

    fn file_reveal_system(&mut self, cx: &mut Context<Self>) {
        let path = self.files.tabs[self.files.active].path.clone();
        match self.file_on_disk(&path) {
            Some(real) => cx.reveal_path(&real),
            None => if let Some(key) = self.selected_key() {
                self.action_feedback.insert(key, (tr("file_not_on_disk"), true));
                cx.notify();
            },
        }
    }

    pub(super) fn render_file_view(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.files_visible() { return None; }
        let tab = &self.files.tabs[self.files.active];
        let name = composer::basename(&tab.path).to_owned();
        let tabs = div().id("file-tabs").role(Role::TabList).aria_label(tr("file_tabs")).min_w_0().flex().items_center().gap_1().overflow_x_scroll()
            .children(self.files.tabs.iter().enumerate().map(|(ix, tab)| {
                let (id, active) = (tab.id, ix == self.files.active);
                let mark = match &tab.content {
                    Some(Ok(doc)) if doc.error.is_some() => Some(true),
                    Some(Ok(doc)) if doc.dirty() => Some(false),
                    _ => None,
                };
                let tip = mark.map_or(tab.path.clone(), |failed| format!("{} · {}", tab.path,
                    activity::web(if failed { "arq_falhou_salvar" } else { "arq_nao_salvo" })));
                let group = SharedString::from(format!("file-tab-{id}"));
                let name = composer::basename(&tab.path).to_owned();
                div().id(("file-tab", id)).group(group.clone()).role(Role::Tab).aria_selected(active).aria_label(tab.path.clone())
                    .h(px(28.)).pl(px(8.)).pr(px(2.)).flex().items_center().gap(px(6.)).flex_shrink_0().rounded(px(6.)).cursor_pointer()
                    .map(|el| if active { el.bg(theme::elevated()) } else { el.hover(|el| el.bg(theme::hover())) })
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(ix) = this.files.tabs.iter().position(|t| t.id == id) { this.files.active = ix; this.focus_file(window, cx); }
                    }))
                    .child(crate::fileicons::tree_icon(&name, false, false))
                    .child(div().max_w(px(120.)).truncate().text_size(px(12.)).text_color(if active { theme::text() } else { theme::muted() }).child(name))
                    .when_some(mark, |el, failed| el.child(div().size(px(6.)).flex_shrink_0().rounded_full()
                        .bg(if failed { theme::danger() } else { theme::accent() })))
                    // Como no Zeron, o X aparece na aba ativa e na que está sob o ponteiro.
                    .child(div().opacity(if active { 1. } else { 0. }).group_hover(group, |s| s.opacity(1.))
                        .child(Button::new(("file-close", id)).ghost().xsmall().icon(IconName::Close).accessibility_label(tr("file_close"))
                            .tooltip(tr("file_close")).on_click(cx.listener(move |this, _, window, cx| this.close_file(id, window, cx)))))
            }));
        let expanded = self.files.expanded;
        let strip = div().h(px(40.)).flex_shrink_0().flex().items_center().gap_1().px_2()
            .child(tabs)
            .child(chrome::icon_button("file-add", IconName::Plus, tr("file_add"), cx)
                .on_click(cx.listener(|this, _, window, cx| this.file_add(window, cx))))
            .child(div().flex_1())
            .child(chrome::icon_button("file-expand", if expanded { IconName::Minimize } else { IconName::Maximize },
                    tr(if expanded { "file_restore" } else { "file_expand" }), cx).selected(expanded)
                .on_click(cx.listener(|this, _, _, cx| { this.files.expanded = !this.files.expanded; cx.notify(); })))
            .child(Button::new("file-back").ghost().small().label(tr("file_back"))
                .on_click(cx.listener(|this, _, window, cx| { this.files_escape(window, cx); })));
        let parts = trail(self.selected.as_ref().and_then(|s| s.cwd.as_deref()), &tab.path);
        let last = parts.len().saturating_sub(1);
        let full = tab.path.clone();
        let crumbs = div().id("file-trail").min_w_0().flex_1().flex().items_center().overflow_hidden()
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx))
            .children(parts.into_iter().enumerate().flat_map(|(ix, part)| [
                (ix > 0).then(|| div().mx(px(4.)).flex_shrink_0().text_size(px(11.)).text_color(theme::faint()).child("›").into_any_element()),
                Some(div().min_w_0().truncate().text_size(px(11.))
                    .text_color(if ix == last { theme::muted() } else { theme::faint() }).child(part).into_any_element()),
            ]).flatten());
        let doc = tab.content.as_ref().and_then(|result| result.as_ref().ok());
        let toolbar = div().min_h_8().flex_shrink_0().flex().flex_wrap().items_center().gap_1().px_2().py_1().border_t_1().border_b_1().border_color(theme::border())
            .child(div().pl_1().flex_shrink_0().child(crate::fileicons::tree_icon(&name, false, false)))
            .child(crumbs)
            .when_some(doc, |el, doc| el
                .child(Button::new("file-find").ghost().xsmall().icon(IconName::Search).label(tr("file_find"))
                    .tooltip(tr("file_find_shortcut")).on_click(cx.listener(|this, _, window, cx| { this.find_in_file(window, cx); })))
                .when(!tab.reader.mode.reading(), |el| {
                    let position = doc.editor.read(cx).cursor_position();
                    el.child(Button::new("file-goto-line").ghost().xsmall()
                        .label(tr("file_position").replace("{line}", &(position.line + 1).to_string()).replace("{column}", &(position.character + 1).to_string()))
                        .tooltip(tr("file_goto_shortcut")).on_click(cx.listener(|this, _, window, cx| this.open_file_line(window, cx))))
                })
                .child(chrome::icon_button("file-reload", IconName::RefreshCw, tr("file_reload"), cx).disabled(doc.saving || doc.reloading)
                    .on_click(cx.listener(|this, _, window, cx| this.reload_file(window, cx))))
                .when(doc.saved.is_some(), |el| el.child(div().px_1().text_xs().text_color(theme::success()).child(tr("file_saved"))))
                .when(doc.editable() && doc.dirty(), |el| el
                    .child(Button::new("file-discard").ghost().xsmall().label(tr("file_discard")).disabled(doc.saving || doc.reloading)
                        .on_click(cx.listener(|this, _, window, cx| this.discard_file(window, cx))))
                    .child(Button::new("file-save").primary().xsmall().label(tr(if doc.saving { "file_saving" } else { "file_save" }))
                        .tooltip(tr("file_save_shortcut")).disabled(doc.saving || doc.reloading)
                        .on_click(cx.listener(|this, _, _, cx| this.save_file(cx)))))
                .when(doc.markdown.is_some(), |el| el.child(self.reader_toolbar(tab, cx))))
            .when(tab.zoomable(), |el| el
                .child(chrome::icon_button("file-zoom", IconName::Plus, tr("file_zoom"), cx)
                    .on_click(cx.listener(|this, _, window, cx| this.zoom_picture(window, cx)))))
            // Arquivo citado fora da raiz não está na árvore da sessão.
            .child(chrome::icon_button("file-reveal", IconName::FolderOpen, tr("file_reveal"), cx).disabled(tab.path.starts_with(['/', '~']))
                .on_click(cx.listener(|this, _, window, cx| this.file_reveal(window, cx))))
            .child(chrome::icon_button("file-reveal-system", IconName::ExternalLink, tr("file_reveal_system"), cx)
                .disabled(!self.session_on_disk())
                .on_click(cx.listener(|this, _, _, cx| this.file_reveal_system(cx))));
        let state = |text: String, color: Hsla| div().size_full().flex().items_center().justify_center().p_4().text_sm().text_color(color)
            .child(text).into_any_element();
        let content = match (&tab.picture, &tab.content) {
            (Some(Picture::Disk(path)), _) => Self::picture_view(path.clone().into(), cx),
            (Some(Picture::Ready(image)), _) => Self::picture_view(image.clone().into(), cx),
            (Some(Picture::Audio), _) => {
                let (id, path) = (tab.id, tab.path.clone());
                div().size_full().flex().items_center().justify_center()
                    .child(self.audio_controls(&format!("file:{id}"), move |this, cx| this.play_file_audio(id, path.clone(), cx), cx))
                    .into_any_element()
            }
            (Some(Picture::Video), _) => div().size_full().flex().flex_col().items_center().justify_center().gap_3().p_4()
                .child(div().text_sm().text_color(theme::muted()).child(tr("file_video_external")))
                .child(Button::new("file-video-open").outline().small().label(tr("file_video_open"))
                    .on_click(cx.listener(|this, _, _, cx| this.open_file_video(cx))))
                .into_any_element(),
            (Some(Picture::Loading), _) | (None, None) => state(tr("file_loading"), theme::faint()),
            (Some(Picture::Failed(error)), _) | (None, Some(Err(error))) => state(error.clone(), theme::danger()),
            (None, Some(Ok(doc))) if tab.reader.mode.reading() && doc.markdown.is_some() =>
                self.render_reader(tab, doc, self.files.viewport_width.get(), cx),
            (None, Some(Ok(doc))) => Self::reader_source(tab, doc).into_any_element(),
        };
        let viewport_width = self.files.viewport_width.clone();
        let owner = cx.entity().downgrade();
        let measure = canvas(move |bounds, _, cx| {
            let width = f32::from(bounds.size.width);
            if (viewport_width.get() - width).abs() > 1. {
                viewport_width.set(width);
                cx.defer(move |cx| { let _ = owner.update(cx, |_, cx| cx.notify()); });
            }
        }, |_, _, _, _| {}).absolute().inset_0();
        Some(div().id("file-viewer").absolute().inset_0().occlude().flex().flex_col().min_h_0().bg(theme::surface())
            .border_1().border_color(theme::border()).rounded_lg().overflow_hidden()
            .key_context("FileViewer").track_focus(&self.files.focus)
            .on_action(cx.listener(|this, _: &CloseFile, window, cx| {
                if let Some(tab) = this.files.tabs.get(this.files.active) { this.close_file(tab.id, window, cx); }
            }))
            .on_action(cx.listener(|this, _: &NextFile, window, cx| this.step_file(true, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousFile, window, cx| this.step_file(false, window, cx)))
            .on_action(cx.listener(|this, _: &SaveFile, _, cx| this.save_file(cx)))
            .on_action(cx.listener(|this, _: &FindFile, window, cx| { this.find_in_file(window, cx); }))
            .on_action(cx.listener(|this, _: &GoToFileLine, window, cx| this.open_file_line(window, cx)))
            .child(measure)
            .child(strip)
            .child(toolbar)
            .children(doc.filter(|doc| doc.disk_changed).map(|_| div().id("file-disk-changed").role(Role::Alert)
                .flex_shrink_0().px_3().py_2().text_sm().text_color(theme::warning()).child(tr("file_disk_changed"))))
            .children(doc.and_then(|doc| doc.poll_error.as_ref()).map(|error| div().id("file-poll-error").role(Role::Alert)
                .flex_shrink_0().px_3().py_2().text_sm().text_color(theme::danger()).child(error.clone())))
            .children(doc.and_then(|doc| doc.error.as_ref()).filter(|_| tab.reader.mode.reading()).map(|error| div().id("file-preview-error").role(Role::Alert)
                .flex_shrink_0().px_3().py_2().text_sm().text_color(theme::danger()).child(error.clone())))
            .children(self.render_preview_find(cx))
            .child(div().flex_1().min_h_0().overflow_hidden().child(content)).into_any_element())
    }
}

/// Imagem decodificada aqui sai da memória de vídeo junto com a aba.
fn release(tab: FileTab, window: &mut Window, cx: &mut App) {
    if let Some(Picture::Ready(image)) = tab.picture { cx.drop_image(image, Some(window)); }
    if let Some(Ok(doc)) = tab.content { doc.reader_images.release(window, cx); }
}

/// A trilha do Zeron: a pasta da sessão e as partes do caminho. Caminho absoluto (arquivo citado fora da raiz) já diz
/// de onde vem e não leva a pasta da sessão.
fn trail(root: Option<&str>, path: &str) -> Vec<String> {
    let mut parts = Vec::new();
    if !path.starts_with('/') && !path.starts_with('~') {
        parts.extend(root.and_then(|root| std::path::Path::new(root).file_name()).map(|name| name.to_string_lossy().into_owned()));
    }
    parts.extend(path.split('/').filter(|part| !part.is_empty() && *part != ".").map(str::to_owned));
    parts
}

/// Destino de um link da prévia de Markdown dentro da sessão: relativo à pasta do documento, sem sair da raiz. Web,
/// âncora e caminho absoluto ficam de fora.
fn link_target(document: &str, url: &str) -> Option<String> {
    let windows = document.contains('\\');
    if url.starts_with(['#', '/', '\\']) || url.contains(':') || (!windows && url.contains('\\')) { return None; }
    let target = url.split(['#', '?']).next().filter(|target| !target.is_empty())?;
    let mut decoded = Vec::new();
    let mut bytes = target.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' { decoded.push(byte); continue; }
        let hex = [bytes.next()?, bytes.next()?];
        decoded.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
    }
    let target = String::from_utf8(decoded).ok()?;
    if target.starts_with(['/', '\\']) || target.contains(':') || (!windows && target.contains('\\')) || target.chars().any(char::is_control) { return None; }
    let document = document.replace('\\', "/");
    let target = target.replace('\\', "/");
    let mut parts: Vec<&str> = document.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            // O primeiro pedaço vazio é a raiz de um caminho absoluto: dali não se sobe.
            ".." => { if parts.last().is_none_or(|last| last.is_empty() || last.ends_with(':')) { return None; } parts.pop(); }
            part => parts.push(part),
        }
    }
    Some(parts.join(if windows { "\\" } else { "/" }))
}

fn is_image(path: &str) -> bool {
    let extension = std::path::Path::new(path).extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_language_maps_compiled_grammars_and_plain_text() {
        for (path, expected) in [
            ("main.rs", "rust"), ("app.ts", "typescript"), ("view.tsx", "tsx"),
            ("app.js", "javascript"), ("script.py", "python"), ("data.json", "json"),
            ("config.toml", "toml"), ("config.yml", "yaml"), ("README.md", "markdown"),
            ("run.sh", "bash"), ("app.css", "css"), ("index.html", "html"),
            ("App.svelte", "svelte"), ("query.sql", "sql"), ("main.c", "c"),
            ("header.h", "c"), ("main.cc", "cpp"), ("main.cpp", "cpp"),
            ("header.hpp", "cpp"), ("Main.java", "java"), ("Program.cs", "csharp"),
            ("Main.kt", "kotlin"), ("Main.kts", "kotlin"), ("main.go", "go"),
            ("main.rb", "ruby"), ("main.lua", "lua"), ("Main.swift", "swift"),
            ("index.php", "php"), ("README.markdown", "markdown"),
            ("run.zsh", "bash"), ("App.vue", "html"),
            ("sample.unknown", "text"), ("config.jsonc", "json"),
            ("notes.mdx", "markdown"), ("theme.scss", "text"),
            ("types.pyi", "python"), ("main.pas", "pascal"), ("Projeto.DPR", "pascal"),
            ("Form1.dfm", "pascal"), ("main.dart", "dart"), ("fix.patch", "diff"),
            ("Makefile", "make"), ("rules.mk", "make"), ("main.zig", "zig"),
            ("App.scala", "scala"), ("app.ex", "elixir"),
        ] {
            assert_eq!(super::file_language(path), expected, "{path}");
        }
    }

    #[test]
    fn every_mapped_language_has_a_grammar_with_color_rules() {
        use gpui_kit::component::highlighter::LanguageRegistry;
        super::grammars::register();
        let mut broken = Vec::new();
        for name in [
            "a.rs", "a.ts", "a.tsx", "a.js", "a.py", "a.json", "a.toml", "a.yml", "a.md", "a.sh", "a.css", "a.html",
            "a.svelte", "a.sql", "a.c", "a.cpp", "a.java", "a.cs", "a.kt", "a.go", "a.rb", "a.lua", "a.swift", "a.php",
            "a.pas", "a.dart", "a.diff", "a.mk", "a.zig", "a.scala", "a.ex",
        ].map(super::file_language) {
            match LanguageRegistry::singleton().language(name).and_then(|config| Some((config.language?, config.highlights))) {
                None => broken.push(format!("{name}: sem gramática")),
                Some((_, highlights)) if highlights.trim().is_empty() => broken.push(format!("{name}: sem regras de cor")),
                Some((language, highlights)) => if let Err(error) = tree_sitter::Query::new(&language, &highlights) {
                    broken.push(format!("{name}: regra quebrada ({error})"));
                },
            }
        }
        assert!(broken.is_empty(), "{broken:#?}");
    }

    #[test]
    fn trail_starts_at_the_session_folder_unless_the_path_is_absolute() {
        assert_eq!(super::trail(Some("/home/j/hangar"), "docs/a.md"), ["hangar", "docs", "a.md"]);
        assert_eq!(super::trail(Some("/home/j/hangar/"), "./mobile/app.json"), ["hangar", "mobile", "app.json"]);
        assert_eq!(super::trail(Some("/home/j/hangar"), "/etc/hosts"), ["etc", "hosts"]);
        assert_eq!(super::trail(None, "a.md"), ["a.md"]);
    }

    #[test]
    fn markdown_links_resolve_inside_the_session_only() {
        let target = |url| super::link_target("docs/guide/intro.md", url);
        assert_eq!(target("setup.md").as_deref(), Some("docs/guide/setup.md"));
        assert_eq!(target("../../README.md#topo").as_deref(), Some("README.md"));
        assert_eq!(target("./img/a%20b.png").as_deref(), Some("docs/guide/img/a b.png"));
        assert_eq!(target("../../../etc/passwd"), None);
        for outside in ["https://x.dev/a.md", "#topo", "/etc/hosts", "mailto:a@b.c", "a%2"] { assert_eq!(target(outside), None, "{outside}"); }
        assert_eq!(super::link_target("/tmp/notes/a.md", "../b.md").as_deref(), Some("/tmp/b.md"));
        assert_eq!(super::link_target("/a.md", "../b.md"), None);
    }

    #[test]
    fn editing_requires_a_complete_read_and_digest() {
        let mut content: super::Content = serde_json::from_value(serde_json::json!({
            "path": "empty.txt", "text": "", "truncated": false, "digest": "read-digest"
        })).unwrap();
        assert!(content.editable());
        content.truncated = true;
        assert!(!content.editable());
        content.truncated = false;
        content.digest = None;
        assert!(!content.editable());
        content.digest = Some(String::new());
        assert!(!content.editable());
    }
}
