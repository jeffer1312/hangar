//! A vista "citados" da aba Arquivos, como a do web: todo arquivo que a conversa citou, inclusive fora da pasta da
//! sessão. Sessão desta máquina confere no disco; de outra máquina, pelo `files/resolver` do backend, de uma vez.
//! O que não existe não aparece.
use super::*;

/// Teto de caminhos conferidos: os mais recentes primeiro.
const MAX: usize = 300;
/// Linhas da seção fechada em "mais N" e o teto dela aberta (a busca também mostra até o teto).
const SHORT: usize = 5;
const LONG: usize = 40;

#[derive(Clone)]
struct Cited { raw: String, real: String, relative: Option<String> }

#[derive(Default)]
pub(in crate::app) struct CitedView {
    list: Option<Result<Vec<Cited>, String>>,
    /// Geração da árvore e quantos eventos a conversa tinha na última leitura: só relê quando um dos dois muda.
    read_for: Option<(u64, usize)>,
}

impl CitedView {
    /// Outra sessão: a lista da anterior some.
    pub(in crate::app) fn reset(&mut self) { (self.list, self.read_for) = (None, None); }
}

/// No disco: absoluto (ou `~/`) como está, relativo pela pasta da sessão e depois pelas pastas dos `cd` da conversa;
/// só arquivo comum, fora de `.git`.
fn resolve_local(root: &Path, paths: Vec<String>, dirs: Vec<String>) -> Vec<Cited> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let home = home_dir();
    let usable = |p: PathBuf| std::fs::canonicalize(p).ok().filter(|p| p.is_file() && !p.components().any(|c| c.as_os_str() == ".git"));
    let mut seen = HashSet::new();
    paths.into_iter().filter_map(|raw| {
        let real = match raw.strip_prefix("~/") {
            Some(rest) => usable(home.as_ref()?.join(rest)),
            None if Path::new(&raw).is_absolute() => usable(PathBuf::from(&raw)),
            None => std::iter::once(root.clone()).chain(dirs.iter().map(PathBuf::from)).find_map(|dir| usable(dir.join(&raw))),
        }?;
        let relative = real.strip_prefix(&root).ok().map(|p| p.to_string_lossy().into_owned());
        let real = real.to_string_lossy().into_owned();
        seen.insert(real.clone()).then_some(Cited { raw, real, relative })
    }).collect()
}

impl Hangar {
    /// Os caminhos citados, dos mais recentes, e as pastas dos `cd` da conversa.
    fn cited_paths(&self) -> (Vec<String>, Vec<String>) {
        fn collect(value: &Value, out: &mut Vec<String>, dirs: &mut Vec<String>) {
            match value {
                Value::String(text) => {
                    for reference in composer::code_references(text) {
                        // Pedaço relativo pode ser o fim de um absoluto com espaço que o leitor cortou ("…/Área de trabalho/x.sql").
                        // Só quando o pedaço vem logo depois de um espaço: é o único jeito de o leitor ter cortado ali.
                        let cut = !reference.path.starts_with(['/', '~']) && text[..reference.start].ends_with(' ');
                        let whole = if cut { composer::spaced_paths(text, &reference.path) } else { Vec::new() };
                        for path in whole.into_iter().chain(std::iter::once(reference.path)) {
                            if !out.contains(&path) { out.push(path); }
                        }
                    }
                    for dir in composer::cd_dirs(text) { if !dirs.contains(&dir) { dirs.push(dir); } }
                }
                Value::Array(items) => for item in items { collect(item, out, dirs); },
                Value::Object(items) => for item in items.values() { collect(item, out, dirs); },
                _ => {},
            }
        }
        let (mut out, mut dirs) = (Vec::new(), Vec::new());
        for event in self.chat.events.iter().rev() {
            collect(&json!([event.text, event.tool_input, event.result]), &mut out, &mut dirs);
            if out.len() >= MAX { break; }
        }
        out.truncate(MAX);
        (out, dirs)
    }

    fn cited_sync(&mut self, cx: &mut Context<Self>) {
        let stamp = (self.tree.generation, self.chat.events.len());
        if self.tree.cited.read_for == Some(stamp) { return; }
        // Espera a árvore saber se a pasta é desta máquina; antes disso não dá para escolher entre disco e backend.
        let Some(source) = self.tree.source.as_ref() else { return };
        let local = source.root().map(Path::to_path_buf);
        let (Some(api), Some(session)) = (self.session_api(), self.selected.as_ref()) else { return };
        self.tree.cited.read_for = Some(stamp);
        let (paths, dirs) = self.cited_paths();
        if paths.is_empty() { self.tree.cited.list = Some(Ok(Vec::new())); return; }
        let name = session.name.clone();
        let job = self.runtime.spawn(async move {
            if let Some(root) = local {
                return tokio::task::spawn_blocking(move || resolve_local(&root, paths, dirs)).await.map_err(|_| Failure::local("invalid_response"));
            }
            let value = api.act(&name, &["files", "resolver"], Some(json!({"caminhos": paths.clone()})), false, 30).await?;
            // Resposta sem o mapa `ok` não é "nada citado": é resposta que o app não entende.
            let found = value.get("ok").filter(|ok| ok.is_object()).ok_or_else(|| Failure::local("invalid_response"))?;
            Ok({
                let mut seen = HashSet::new();
                paths.into_iter().filter_map(|raw| {
                    let entry = found.get(&raw)?;
                    let real = entry.get("real").and_then(Value::as_str).unwrap_or(&raw).to_owned();
                    let relative = entry.get("relativo").and_then(Value::as_str).map(str::to_owned);
                    seen.insert(real.clone()).then_some(Cited { raw, real, relative })
                }).collect::<Vec<_>>()
            })
        });
        cx.spawn(async move |this, cx| {
            // Tarefa que morreu vira erro na vista, não "carregando" para sempre.
            let result = job.await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
            let _ = this.update(cx, |this, cx| {
                if this.tree.cited.read_for != Some(stamp) { return; }
                this.tree.cited.list = Some(result.map_err(|error| Self::fetch_failure(&error)));
                this.redraw(Area::Side, cx);
            });
        }).detach();
    }

    /// Os citados como seção no topo da aba, recolhível: as primeiras e "mais N". Com a busca, os que casam. Lista vazia
    /// ou ainda lendo não ocupa lugar; falha aparece no lugar das linhas.
    pub(super) fn render_cited_section(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.cited_sync(cx);
        let query = self.tree.query.to_lowercase();
        let (rows, failed) = match &self.tree.cited.list {
            None => return None,
            Some(Err(reason)) => (Vec::new(), Some(reason.clone())),
            Some(Ok(list)) => (list.iter().filter(|c| query.is_empty() || c.relative.as_deref().unwrap_or(&c.real).to_lowercase().contains(&query))
                .cloned().collect::<Vec<_>>(), None),
        };
        if rows.is_empty() && failed.is_none() { return None; }
        let open = self.tree.cited_open;
        let limit = if self.tree.cited_more || !query.is_empty() { LONG } else { SHORT };
        let hidden = rows.len().saturating_sub(limit);
        let header = div().id("tree-cited-header").h(px(28.)).px(px(14.)).flex().items_center().gap(px(6.)).cursor_pointer()
            .role(Role::Button).aria_expanded(open).aria_label(tr("tree_cited_section"))
            .hover(|el| el.bg(theme::hover()))
            .on_click(cx.listener(|this, _, _, cx| { this.tree.cited_open = !this.tree.cited_open; this.redraw(Area::Side, cx); }))
            .child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 11., theme::faint()))
            .child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr("tree_cited_section").to_uppercase()))
            .child(div().text_size(px(11.5)).text_color(theme::faint()).child(rows.len().to_string()));
        let more = (open && (hidden > 0 || (self.tree.cited_more && rows.len() > SHORT && query.is_empty()))).then(|| {
            let label = if hidden > 0 { tr("tree_more").replace("{n}", &hidden.to_string()) } else { tr("tree_less") };
            div().id("tree-cited-more").h(px(24.)).pl(px(36.)).flex().items_center().text_size(px(12.)).text_color(theme::faint())
                .cursor_pointer().hover(|el| el.text_color(theme::text()))
                .on_click(cx.listener(|this, _, _, cx| { this.tree.cited_more = !this.tree.cited_more; this.redraw(Area::Side, cx); }))
                .child(label)
        });
        Some(div().flex_shrink_0().flex().flex_col().pb_1().border_b_1().border_color(theme::border())
            .child(header)
            .when(open, |el| el
                .when_some(failed, |el, reason| el.child(div().px_4().py_1().text_xs().text_color(theme::warning()).whitespace_normal().child(reason)))
                .children(rows.into_iter().take(limit).map(|item| self.cited_row(item, cx)))
                .children(more))
            .into_any_element())
    }

    fn cited_row(&self, item: Cited, cx: &mut Context<Self>) -> AnyElement {
        let shown = item.relative.clone().unwrap_or_else(|| abbreviate(&item.real));
        let (folder, name) = shown.rsplit_once('/').map_or((String::new(), shown.clone()), |(f, n)| (f.to_owned(), n.to_owned()));
        let open = item.relative.clone().unwrap_or(item.raw.clone());
        let local = self.tree.source.as_ref().is_some_and(FileSource::is_local);
        let target = ops::ItemTarget { session: item.relative.clone(), open: open.clone(), disk: local.then(|| PathBuf::from(&item.real)),
            full: item.real.clone(), dir: false };
        div().h(px(ROW_H)).w_full().px_1().child(div().id(SharedString::from(format!("tree-cited-{}", item.real)))
            .role(Role::ListBoxOption).aria_label(shown.clone())
            .size_full().pl(px(30.)).pr_2().flex().items_center().gap(px(8.)).rounded(px(6.)).cursor_pointer()
            .hover(|el| el.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, window, cx| this.open_file(open.clone(), None, window, cx)))
            .context_menu(item_menu(cx.entity().downgrade(), target))
            .child(crate::fileicons::tree_icon(&name, false, false))
            .child(div().flex_none().max_w(relative(0.6)).truncate().text_size(px(13.)).text_color(theme::text()).child(name))
            .child(div().flex_1().min_w_0().truncate().text_right().text_size(px(11.)).text_color(theme::faint()).child(folder)))
            .into_any_element()
    }
}

/// `/home/<usuário>/x` vira `~/x`, como a pasta mostrada no web.
fn abbreviate(path: &str) -> String {
    match home_dir().map(|h| h.to_string_lossy().into_owned()) {
        Some(home) if path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}
