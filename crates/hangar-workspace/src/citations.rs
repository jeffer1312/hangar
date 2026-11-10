//! As citações autorizam o arquivo; coincidência com um sufixo não autoriza outro caminho.
use crate::{Result, error, real};
use serde_json::{Value, json};
use std::{
    cell::OnceCell,
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

/// Linhas da conversa: o arquivo, ou as já em memória (histórico de uma transferência).
#[derive(Clone, Copy)]
pub enum Transcript<'a> {
    File(&'a Path),
    Rows(&'a [String]),
}

/// Uma linha da conversa. O `cwd` dela só sai do JSON para quem achou algo, uma vez.
struct Line<'a> {
    text: &'a str,
    cwd: OnceCell<Option<String>>,
}
impl<'a> Line<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, cwd: OnceCell::new() }
    }
    fn cwd(&self) -> Option<&str> {
        self.cwd
            .get_or_init(|| {
                serde_json::from_str::<Value>(self.text)
                    .ok()
                    .and_then(|v| v.get("cwd").and_then(Value::as_str).map(str::to_owned))
            })
            .as_deref()
    }
}

const KEEP_ABSOLUTE: usize = 50;
const KEEP_RELATIVE: usize = 20;

/// O que uma pergunta já achou até certo ponto da conversa; cada tipo de pergunta usa as suas listas.
#[derive(Clone, Default)]
struct Found {
    seen: bool,
    folders: Vec<String>,
    absolute: Vec<String>,
    relative: Vec<String>,
}
impl Found {
    /// Só os mais recentes entram na resposta: o resto não precisa ficar guardado.
    fn trim(&mut self) {
        for (list, limit) in [(&mut self.absolute, KEEP_ABSOLUTE), (&mut self.relative, KEEP_RELATIVE)] {
            let mut kept = recent(std::mem::take(list), limit);
            kept.reverse();
            *list = kept;
        }
    }
}

/// Os `limit` mais recentes sem repetição, do mais novo ao mais antigo.
fn recent(values: Vec<String>, limit: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .rev()
        .filter(|s| seen.insert(s.clone()))
        .take(limit)
        .collect()
}

enum Kind {
    /// `longer`: os outros caminhos pedidos que começam por este. Onde um deles casa, este não conta.
    Cited { escaped: String, longer: Vec<String> },
    /// `relative`: só existe para nome sem pasta.
    Mentions { name: String, suffix: String, relative: Option<regex::Regex> },
}

/// Uma pergunta à conversa: anda linha a linha, e o que achou (`found`) continua de onde parou. A `key` diz
/// quando duas perguntas são a mesma.
struct Query {
    key: String,
    kind: Kind,
    found: Found,
}
impl Query {
    /// O texto que a linha precisa conter para esta pergunta achar algo nela.
    fn needle(&self) -> &str {
        match &self.kind {
            Kind::Cited { escaped, .. } => escaped,
            Kind::Mentions { name, .. } => name,
        }
    }
    fn line(&mut self, line: &Line<'_>) {
        let Query { kind, found, .. } = self;
        let text = line.text;
        match kind {
            Kind::Cited { escaped, longer } => {
                let mut from = 0;
                loop {
                    let Some(at) = text[from..].find(escaped.as_str()).map(|at| from + at) else {
                        return;
                    };
                    if !longer.iter().any(|other| text[at..].starts_with(other.as_str())) {
                        break;
                    }
                    from = at + text[at..].chars().next().map_or(1, char::len_utf8);
                }
                found.seen = true;
                if let Some(cwd) = line.cwd().filter(|s| !s.is_empty()) {
                    found.folders.retain(|s| s != cwd);
                    found.folders.push(cwd.to_owned());
                }
            }
            Kind::Mentions { name, suffix, relative } => {
                if !text.contains(name.as_str()) {
                    return;
                }
                for (start, c) in text.char_indices() {
                    if c != '/' || !beginning(&text[..start]) {
                        continue;
                    }
                    let rest = &text[start..];
                    let allowed = rest
                        .char_indices()
                        .find(|(_, c)| "\\\n\"`'<>|*?".contains(*c))
                        .map(|(p, _)| p)
                        .unwrap_or(rest.len());
                    let rest = &rest[..allowed];
                    for (index, _) in rest.match_indices(suffix.as_str()) {
                        if index == 0 {
                            continue;
                        }
                        let end = index + suffix.len();
                        if rest[..index].chars().count() > 401 {
                            break;
                        }
                        if end_boundary(&rest[end..]) {
                            found.absolute.push(rest[..end].into());
                            break;
                        }
                    }
                }
                if let Some(relative) = relative {
                    for hit in relative.find_iter(text) {
                        let prefix = &text[..hit.start()];
                        if prefix
                            .chars()
                            .last()
                            .is_some_and(|c| c.is_alphanumeric() || "_./-".contains(c))
                        {
                            continue;
                        }
                        if end_boundary(&text[hit.end()..]) {
                            found.relative.push(hit.as_str().to_owned());
                        }
                    }
                }
                if found.absolute.len() + found.relative.len() > 1024 {
                    found.trim();
                }
            }
        }
    }
}

const ANCHOR: u64 = 64;
// ponytail: ao encher, esvazia tudo; passando de 64 conversas com citação ao mesmo tempo, despejar uma por vez.
const MAX_FILES: usize = 64;
const MAX_QUERIES: usize = 512;

#[cfg(unix)]
type FileId = (u64, u64);
#[cfg(not(unix))]
type FileId = ();
fn file_id(meta: &std::fs::Metadata) -> FileId {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
    }
}

/// O que já foi lido de uma conversa. Ela só cresce por acréscimo: o trecho até `checked` vale enquanto o arquivo
/// for o mesmo e os bytes logo antes desse ponto forem os de `anchor`. Trocou: `generation` nova, e quem lia a
/// anterior não grava mais aqui.
struct Slot {
    generation: u64,
    id: FileId,
    checked: u64,
    anchor: Vec<u8>,
    /// Por pergunta: até onde ela leu (sempre fim de linha completa) e o que achou até ali.
    queries: HashMap<String, (u64, Found)>,
}
#[derive(Default)]
struct Scanned {
    generations: u64,
    files: HashMap<PathBuf, Slot>,
}
static SCANNED: LazyLock<Mutex<Scanned>> = LazyLock::new(Mutex::default);

#[cfg(test)]
thread_local! {
    /// Bytes de arquivo lidos por `scan` nesta thread.
    static READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn anchor_at(mut file: &File, end: u64) -> Option<Vec<u8>> {
    let len = end.min(ANCHOR);
    let mut bytes = vec![0; len as usize];
    file.seek(SeekFrom::Start(end - len)).ok()?;
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

/// Põe em cada pergunta o que ela já tinha achado neste arquivo e devolve de onde cada uma continua.
/// Sem geração (`None`) nada é guardado depois.
fn resume(path: &Path, file: &File, queries: &mut [Query]) -> (Option<u64>, Vec<u64>) {
    let meta = match file.metadata() {
        Ok(meta) => meta,
        Err(error) => {
            tracing::warn!(code = "citations_transcript_unreadable", kind = ?error.kind(), "conversa sem metadados; lida inteira, sem guardar");
            return (None, vec![0; queries.len()]);
        }
    };
    let id = file_id(&meta);
    let mut scanned = SCANNED.lock().unwrap_or_else(|e| e.into_inner());
    let valid = scanned.files.get(path).is_some_and(|slot| {
        slot.id == id
            && meta.len() >= slot.checked
            && anchor_at(file, slot.checked).is_some_and(|anchor| anchor == slot.anchor)
    });
    if !valid {
        if !scanned.files.contains_key(path) && scanned.files.len() >= MAX_FILES {
            scanned.files.clear();
        }
        scanned.generations += 1;
        let slot = Slot {
            generation: scanned.generations,
            id,
            checked: 0,
            anchor: Vec::new(),
            queries: HashMap::new(),
        };
        scanned.files.insert(path.to_owned(), slot);
    }
    let slot = &scanned.files[path];
    let starts = queries
        .iter_mut()
        .map(|query| match slot.queries.get(&query.key) {
            Some((offset, found)) => {
                query.found = found.clone();
                *offset
            }
            None => 0,
        })
        .collect();
    (Some(slot.generation), starts)
}

fn store(path: &Path, generation: u64, end: u64, anchor: Vec<u8>, queries: &[Query]) {
    let mut scanned = SCANNED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(slot) = scanned.files.get_mut(path).filter(|slot| slot.generation == generation) else {
        return;
    };
    if end > slot.checked {
        slot.checked = end;
        slot.anchor = anchor;
    }
    for query in queries {
        // Outro pedido pode ter ido mais longe com a mesma pergunta: fica o dele.
        if slot.queries.get(&query.key).is_some_and(|(offset, _)| *offset >= end) {
            continue;
        }
        if slot.queries.len() >= MAX_QUERIES && !slot.queries.contains_key(&query.key) {
            slot.queries.clear();
        }
        slot.queries.insert(query.key.clone(), (end, query.found.clone()));
    }
}

/// Bytes lidos numa passada antes de valer a pena montar o padrão. Nos testes vale desde a primeira linha,
/// para as conversas pequenas deles passarem pelo atalho.
const PREFILTER_AFTER: u64 = if cfg!(test) { 0 } else { 256 * 1024 };

/// Uma procura só, nos bytes da linha, pelo texto de todas as perguntas: a linha sem nenhum não muda a resposta
/// de nenhuma, e deixa de custar a conversão para texto e uma procura por pergunta. `None` (sem atalho) se o
/// padrão não couber ou se algum texto tiver U+FFFD: a conversão põe esse caractere no lugar de byte inválido, e
/// só aí o texto da linha poderia ter o que os bytes dela não têm.
fn any_needle(queries: &[Query]) -> Option<regex::bytes::Regex> {
    if queries.iter().any(|query| query.needle().contains('\u{FFFD}')) {
        return None;
    }
    let needles = queries.iter().map(|query| regex::escape(query.needle())).collect::<Vec<_>>();
    // Sem o erro no log: ele pode trazer o padrão, que é feito dos caminhos procurados.
    regex::bytes::Regex::new(&needles.join("|"))
        .inspect_err(|_| tracing::warn!(code = "citations_needle_pattern_failed", queries = queries.len(), "padrão das perguntas não coube; a leitura segue sem o atalho"))
        .ok()
}

fn feed(queries: &mut [Query], starts: &[u64], any: Option<&regex::bytes::Regex>, at: u64, bytes: &[u8]) {
    if any.is_some_and(|any| !any.is_match(bytes)) {
        return;
    }
    let text = String::from_utf8_lossy(bytes);
    let line = Line::new(&text);
    for (query, start) in queries.iter_mut().zip(starts) {
        if *start <= at {
            query.line(&line);
        }
    }
}

/// Passa a conversa pelas perguntas. No arquivo, cada pergunta lê só o que entrou depois da última vez dela.
fn scan(source: Transcript<'_>, mut queries: Vec<Query>) -> Vec<Query> {
    let path = match source {
        Transcript::Rows(rows) => {
            for row in rows {
                let line = Line::new(row);
                queries.iter_mut().for_each(|query| query.line(&line));
            }
            return queries;
        }
        Transcript::File(path) => path,
    };
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            // Conversa que ainda não existe é resposta vazia, não defeito.
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(code = "citations_transcript_unreadable", kind = ?error.kind(), "conversa não abriu; nenhuma citação lida");
            }
            return queries;
        }
    };
    let (generation, starts) = resume(path, &file, &mut queries);
    let Some(from) = starts.iter().copied().min() else {
        return queries;
    };
    let mut reader = BufReader::with_capacity(1 << 20, &file);
    if let Err(error) = reader.seek(SeekFrom::Start(from)) {
        tracing::warn!(code = "citations_transcript_unreadable", kind = ?error.kind(), "conversa não posicionou; nenhuma citação lida");
        return queries;
    }
    let mut end = from;
    let mut buffer = Vec::new();
    // O padrão só é montado depois de `PREFILTER_AFTER` bytes lidos: no trecho curto que a conversa
    // cresceu entre dois pedidos, montá-lo custa mais do que ele poupa.
    let any = OnceCell::new();
    // `partial`: a última linha ainda sem `\n` (escrita em curso).
    let partial = loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => break false,
            // O ponto guardado abaixo é o fim da última linha lida inteira: a próxima leitura retoma no
            // trecho que falhou, sem pular nada.
            Err(error) => {
                tracing::warn!(code = "citations_transcript_read_failed", kind = ?error.kind(), at = end, "leitura da conversa falhou no meio; a resposta vai só até ali");
                break false;
            }
            Ok(read) => {
                #[cfg(test)]
                READ.with(|total| total.set(total.get() + read as u64));
                if buffer.last() != Some(&b'\n') {
                    break true;
                }
                let any = (end - from >= PREFILTER_AFTER).then(|| any.get_or_init(|| any_needle(&queries)).as_ref()).flatten();
                feed(&mut queries, &starts, any, end, &buffer);
                end += read as u64;
            }
        }
    };
    drop(reader);
    if let Some(generation) = generation
        && starts.iter().any(|start| *start < end)
        && let Some(anchor) = anchor_at(&file, end)
    {
        queries.iter_mut().for_each(|query| query.found.trim());
        store(path, generation, end, anchor, &queries);
    }
    // A linha incompleta entra só nesta resposta: o ponto guardado fica antes dela, e ela é relida inteira.
    if partial {
        let any = any.get_or_init(|| any_needle(&queries)).as_ref();
        feed(&mut queries, &starts, any, end, &buffer);
    }
    queries
}

pub fn cwds(jsonl: Transcript<'_>, needles: &[String]) -> Value {
    ask(jsonl, needles, &[]).0
}

/// Uma leitura da conversa para tudo o que o pedido pergunta: as pastas de quem citou cada caminho de `cited`
/// (`cwds`) e, na ordem pedida, as menções de cada caminho de `mentioned` (`elsewhere`).
fn ask(jsonl: Transcript<'_>, cited: &[String], mentioned: &[String]) -> (Value, Vec<Value>) {
    // A linha é JSON cru: caminho do Windows aparece com a contrabarra dobrada (`C:\\Users\\x.png`). Procura-se a
    // forma escapada de cada caminho e o achado volta com o caminho original.
    let mut wanted = cited
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| (json_escaped(s), s.clone()))
        .collect::<Vec<_>>();
    wanted.sort();
    wanted.dedup();
    let mut queries = wanted
        .iter()
        .map(|(escaped, _)| {
            // Na mesma posição vale o caminho mais longo: os que começam por este o escondem ali.
            let longer = wanted
                .iter()
                .filter(|(other, _)| other.len() > escaped.len() && other.starts_with(escaped.as_str()))
                .map(|(other, _)| other.clone())
                .collect::<Vec<_>>();
            Query {
                key: format!("cited\0{escaped}\0{}", longer.join("\0")),
                kind: Kind::Cited { escaped: escaped.clone(), longer },
                found: Found::default(),
            }
        })
        .collect::<Vec<_>>();
    // `None`: caminho que não se procura (vazio ou com `..`).
    let asked = mentioned
        .iter()
        .map(|path| mentions(path).map(|query| queries.push(query)).is_some())
        .collect::<Vec<_>>();
    let mut answers = scan(jsonl, queries).into_iter();
    let mut map = serde_json::Map::new();
    for ((_, needle), query) in wanted.iter().zip(answers.by_ref()) {
        if query.found.seen {
            let mut list = query.found.folders;
            list.reverse();
            map.insert(needle.clone(), json!(list));
        }
    }
    let mentions = asked
        .into_iter()
        .map(|asked| {
            let found = asked.then(|| answers.next()).flatten().map(|query| query.found).unwrap_or_default();
            let absolute = recent(found.absolute, KEEP_ABSOLUTE)
                .into_iter()
                .filter(|p| Path::new(p).is_file())
                .collect::<Vec<_>>();
            let relative = recent(found.relative, KEEP_RELATIVE)
                .into_iter()
                .filter(|p| !p.split('/').any(|s| s == ".."))
                .collect::<Vec<_>>();
            json!([absolute, relative])
        })
        .collect();
    (Value::Object(map), mentions)
}
fn json_escaped(text: &str) -> String {
    let quoted = serde_json::to_string(text).unwrap_or_default();
    quoted.get(1..quoted.len().saturating_sub(1)).unwrap_or_default().to_owned()
}
fn end_boundary(rest: &str) -> bool {
    let mut cs = rest.chars();
    match cs.next() {
        Some(c) if c.is_alphanumeric() || c == '_' || c == '-' => false,
        Some('.') => !cs.next().is_some_and(|c| c.is_alphanumeric() || c == '_'),
        _ => true,
    }
}
fn beginning(prefix: &str) -> bool {
    prefix.is_empty()
        || prefix.ends_with("\\n")
        || prefix.ends_with("\\t")
        || prefix
            .chars()
            .last()
            .is_some_and(|c| c.is_whitespace() || "\"`'([=:,".contains(c))
}
/// A pergunta pelas menções de `path`; `None` para caminho vazio ou com `..`.
fn mentions(path: &str) -> Option<Query> {
    let tail = path
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_matches('/')
        .to_owned();
    if tail.is_empty() || tail.split('/').any(|p| p == "..") {
        return None;
    }
    let name = tail.rsplit('/').next().unwrap().to_owned();
    let relative = (!tail.contains('/'))
        .then(|| regex::Regex::new(&format!(r"(?:[\p{{L}}\p{{N}}_.-]+/)+{}", regex::escape(&name))).ok())
        .flatten();
    Some(Query {
        key: format!("mentions\0{tail}"),
        kind: Kind::Mentions { suffix: format!("/{tail}"), name, relative },
        found: Found::default(),
    })
}
pub fn elsewhere(jsonl: Transcript<'_>, path: &str) -> Value {
    ask(jsonl, &[], std::slice::from_ref(&path.to_owned())).1.pop().unwrap_or_else(|| json!([[], []]))
}
/// `elsewhere` de vários caminhos numa leitura só da conversa, na ordem pedida.
pub fn elsewhere_many(jsonl: Transcript<'_>, paths: &[String]) -> Vec<Value> {
    ask(jsonl, &[], paths).1
}
pub fn find_elsewhere(
    jsonl: Transcript<'_>,
    cwd: &Path,
    path: &str,
    worked: &[String],
    siblings: bool,
) -> Option<PathBuf> {
    locate(&elsewhere(jsonl, path), cwd, path, worked, siblings)
}
/// O arquivo que as menções de `path` (resposta de `elsewhere`) apontam, procurado nas pastas da sessão.
pub fn locate(result: &Value, cwd: &Path, path: &str, worked: &[String], siblings: bool) -> Option<PathBuf> {
    if let Some(p) = result[0].as_array()?.first().and_then(Value::as_str) {
        return Some(PathBuf::from(p));
    }
    let rel = path.replace('\\', "/").trim_start_matches("./").to_owned();
    if rel.split('/').any(|p| p == "..") {
        return None;
    }
    let relatives = if rel.contains('/') {
        vec![rel]
    } else {
        result[1]
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let mut folders = worked
        .iter()
        .map(|p| real(Path::new(p)))
        .collect::<Vec<_>>();
    let root = real(cwd);
    if !folders.contains(&root) {
        folders.push(root.clone());
    }
    if siblings
        && let Some(parent) = root.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        let mut extra = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !folders.contains(p))
            .collect::<Vec<_>>();
        extra.sort();
        folders.extend(extra.into_iter().take(200));
    }
    for rel in relatives {
        for folder in &folders {
            let candidate = real(&folder.join(&rel));
            if candidate != *folder && candidate.starts_with(folder) && candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
pub fn resolve(cwd: &Path, jsonl: Transcript<'_>, path: &str, write: bool) -> Result<PathBuf> {
    if path.is_empty() || path.contains('\0') {
        return Err(error(400, "invalid path"));
    }
    let expanded = crate::expand_home(path);
    let relative = !expanded.is_absolute() && !path.replace('\\', "/").split('/').any(|p| p == "..");
    let cited = cwds(jsonl, &[path.to_owned()]);
    let Some(bases) = cited.get(path).and_then(Value::as_array) else {
        return Err(crate::WorkspaceError {
            status: 403,
            detail: json!({"code":"erro_arquivo_nao_citado","params":{},"msg":"file not referenced in this conversation"}),
            code: None,
        });
    };
    let target = if expanded.is_absolute() {
        real(&expanded)
    } else {
        if !relative {
            return Err(crate::WorkspaceError {
                status: 403,
                detail: json!({"code":"erro_caminho_fora_cwd","params":{},"msg":"path escapes session cwd"}),
                code: None,
            });
        }
        let worked = bases
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut folders = worked
            .iter()
            .map(|s| real(Path::new(s)))
            .collect::<Vec<_>>();
        folders.push(real(cwd));
        folders
            .iter()
            .find_map(|base| {
                let target = real(&base.join(&expanded));
                (target != *base && target.starts_with(base) && target.is_file()).then_some(target)
            })
            // Só quem não resolveu nas pastas da sessão pergunta pelas menções. Caminho real antes da trava
            // do `.git`: um atalho citado não pode escapar dela.
            .or_else(|| find_elsewhere(jsonl, cwd, path, &worked, !write).map(|p| real(&p)))
            .ok_or_else(not_found)?
    };
    if !target.is_file() {
        return Err(not_found());
    }
    if target.components().any(|p| p.as_os_str() == ".git") {
        return Err(crate::file_error(
            403,
            "erro_arq_area_do_git",
            "área interna do git",
        ));
    }
    Ok(target)
}
fn not_found() -> crate::WorkspaceError {
    crate::WorkspaceError {
        status: 404,
        detail: json!({"code":"erro_arquivo_nao_encontrado","params":{},"msg":"file not found"}),
        code: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{READ, Transcript, cwds, elsewhere, elsewhere_many};
    use serde_json::{Value, json};
    use std::{io::Write, path::Path};

    #[test]
    fn windows_path_is_found_with_its_backslashes_escaped() {
        let path = r"C:\Users\Admin\Temp\shot 1.png".to_owned();
        let line = json!({"cwd": r"C:\Sistemas\web", "message": {"content": format!("Print: {path}")}}).to_string();
        assert!(line.contains(r"C:\\Users"), "o JSON da conversa dobra a contrabarra");
        let rows = [line];
        assert_eq!(cwds(Transcript::Rows(&rows), &[path.clone(), "/tmp/a.png".into()]), json!({path: [r"C:\Sistemas\web"]}));
    }

    fn row(cwd: &str, text: &str) -> String {
        json!({"cwd": cwd, "message": {"content": text}}).to_string() + "\n"
    }
    fn append(path: &Path, text: &str) {
        let mut file = std::fs::OpenOptions::new().append(true).create(true).open(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }
    fn rows(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path).unwrap().split_inclusive('\n').map(str::to_owned).collect()
    }
    /// A resposta pelo arquivo tem de ser a de quem lê a conversa inteira de novo.
    fn cited(path: &Path, needles: &[String]) -> Value {
        let found = cwds(Transcript::File(path), needles);
        assert_eq!(found, cwds(Transcript::Rows(&rows(path)), needles), "diferente da leitura completa");
        found
    }
    fn read_by(run: impl FnOnce()) -> u64 {
        READ.with(|total| total.set(0));
        run();
        READ.with(|total| total.get())
    }

    #[test]
    fn citation_appended_later_is_found_reading_only_the_new_lines() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("c.jsonl");
        let needles = ["/tmp/a.png".to_owned(), "/tmp/b.png".to_owned()];
        let first = row("/proj/a", "veja /tmp/a.png");
        append(&jsonl, &first);
        let whole = read_by(|| assert_eq!(cited(&jsonl, &needles), json!({"/tmp/a.png": ["/proj/a"]})));
        assert_eq!(whole, first.len() as u64);
        let second = row("/proj/b", "e /tmp/b.png, de novo /tmp/a.png");
        append(&jsonl, &second);
        let grown = read_by(|| {
            assert_eq!(cited(&jsonl, &needles), json!({"/tmp/a.png": ["/proj/b", "/proj/a"], "/tmp/b.png": ["/proj/b"]}));
        });
        assert_eq!(grown, second.len() as u64, "releu o que já tinha lido");
        assert_eq!(read_by(|| drop(cited(&jsonl, &needles))), 0, "sem linha nova, nada a ler");
    }

    #[test]
    fn replaced_or_truncated_conversation_is_read_again_from_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("c.jsonl");
        let needles = ["/tmp/old.png".to_owned(), "/tmp/new.png".to_owned()];
        append(&jsonl, &row("/proj/a", "veja /tmp/old.png"));
        assert_eq!(cited(&jsonl, &needles), json!({"/tmp/old.png": ["/proj/a"]}));
        // Regravado no lugar e maior que antes: o mesmo arquivo, com outro conteúdo.
        std::fs::write(&jsonl, row("/proj/b", "agora só /tmp/new.png, com texto de sobra para passar do tamanho de antes")).unwrap();
        assert_eq!(cited(&jsonl, &needles), json!({"/tmp/new.png": ["/proj/b"]}), "citação do conteúdo antigo sobreviveu");
        std::fs::write(&jsonl, row("/p", "nada")).unwrap();
        assert_eq!(cited(&jsonl, &needles), json!({}), "arquivo truncado");
        let other = dir.path().join("other.jsonl");
        std::fs::write(&other, row("/proj/c", "volta /tmp/old.png")).unwrap();
        std::fs::rename(&other, &jsonl).unwrap();
        assert_eq!(cited(&jsonl, &needles), json!({"/tmp/old.png": ["/proj/c"]}), "arquivo trocado");
    }

    #[test]
    fn line_still_being_written_counts_now_and_is_read_whole_later() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("c.jsonl");
        let needles = ["/tmp/a.png".to_owned(), "/tmp/early.png".to_owned(), "/tmp/shot.png".to_owned()];
        let second = row("/proj/b", "ref /tmp/early.png e print /tmp/shot.png pronto");
        // O corte cai no meio de um caminho.
        let cut = second.find("shot.png").unwrap() + 3;
        append(&jsonl, &row("/proj/a", "veja /tmp/a.png"));
        append(&jsonl, &second[..cut]);
        assert_eq!(cited(&jsonl, &needles), json!({"/tmp/a.png": ["/proj/a"], "/tmp/early.png": []}));
        append(&jsonl, &second[cut..]);
        assert_eq!(
            cited(&jsonl, &needles),
            json!({"/tmp/a.png": ["/proj/a"], "/tmp/early.png": ["/proj/b"], "/tmp/shot.png": ["/proj/b"]}),
            "a linha completada perdeu o começo"
        );
    }

    #[test]
    fn many_questions_take_one_reading_and_answer_like_one_each() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("c.jsonl");
        let text = row("/proj/a", "veja src/a.rs e docs/b.md") + &row("/proj/b", "depois web/src/a.rs, c.png e docs/b.md");
        append(&jsonl, &text);
        let paths = ["a.rs".to_owned(), "docs/b.md".to_owned(), "../fora.rs".to_owned(), "c.png".to_owned(), "nada.txt".to_owned()];
        let lines = rows(&jsonl);
        let alone: Vec<Value> = paths.iter().map(|path| elsewhere(Transcript::Rows(&lines), path)).collect();
        assert_eq!(alone[0][1], json!(["web/src/a.rs", "src/a.rs"]));
        let read = read_by(|| assert_eq!(elsewhere_many(Transcript::File(&jsonl), &paths), alone));
        assert_eq!(read, text.len() as u64, "cada pergunta leu a conversa de novo");
        assert_eq!(elsewhere_many(Transcript::Rows(&lines), &paths), alone);
    }

    #[test]
    fn bytes_that_are_not_text_do_not_change_what_a_full_reading_finds() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("c.jsonl");
        // Um byte inválido no meio de um caminho: no texto da linha ele vira U+FFFD.
        let mut bytes = br#"{"cwd":"/proj","m":"veja /tmp/a"#.to_vec();
        bytes.push(0xFF);
        bytes.extend_from_slice(b".png e /tmp/b.png\"}\n");
        std::fs::write(&jsonl, &bytes).unwrap();
        let lines = [String::from_utf8_lossy(&bytes).into_owned()];
        for needles in [
            vec!["/tmp/a.png".to_owned(), "/tmp/b.png".to_owned()],
            vec!["/tmp/a\u{FFFD}.png".to_owned(), "/tmp/b.png".to_owned()],
            vec!["/tmp/a\u{FFFD}.png".to_owned()],
        ] {
            let found = cwds(Transcript::File(&jsonl), &needles);
            assert_eq!(found, cwds(Transcript::Rows(&lines), &needles), "{needles:?}");
            assert_eq!(found.get("/tmp/a.png"), None);
        }
        assert_eq!(cwds(Transcript::File(&jsonl), &["/tmp/a\u{FFFD}.png".to_owned()]), json!({"/tmp/a\u{FFFD}.png": ["/proj"]}));
    }

    /// Números repetíveis para o ensaio abaixo.
    struct Dice(u64);
    impl Dice {
        fn roll(&mut self, sides: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % sides
        }
        fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
            from[self.roll(from.len())]
        }
    }

    #[test]
    fn any_order_of_questions_on_a_growing_conversation_matches_a_full_reading() {
        // Caminhos que começam um pelo outro, com espaço, acento e contrabarra, citados e nunca citados.
        let paths = [
            "/tmp/a.png", "/tmp/a.png.bak", "/tmp/a", "/tmp/dir/a.png", r"C:\Users\Ana\shot 1.png", r"C:\Users\Ana",
            "docs/ação.md", "ação.md", "src/app.rs", "app.rs", "web/src/app.rs", "/var/log/nunca.txt", "x", "ab",
        ];
        let filler = ["veja", "print em", "depois de", "e também", "```", "\\n", "(", "nada aqui", "/", "./"];
        let mut dice = Dice(7);
        for case in 0..200 {
            let dir = tempfile::tempdir().unwrap();
            let jsonl = dir.path().join("c.jsonl");
            for step in 0..6 {
                let mut text = String::new();
                for _ in 0..dice.roll(12) {
                    let words: Vec<&str> = (0..1 + dice.roll(6))
                        .map(|_| if dice.roll(3) == 0 { dice.pick(&paths) } else { dice.pick(&filler) })
                        .collect();
                    text += &row(dice.pick(&["/proj/a", "/proj/b", ""]), &words.join(dice.pick(&[" ", "", ": "])));
                }
                match dice.roll(12) {
                    // Regravado no lugar: tudo o que estava guardado deixa de valer.
                    0 => std::fs::write(&jsonl, &text).unwrap(),
                    // Linha ainda sendo escrita.
                    1 => append(&jsonl, text.trim_end_matches('\n')),
                    _ => append(&jsonl, &text),
                }
                for _ in 0..1 + dice.roll(3) {
                    let asked: Vec<String> = (0..1 + dice.roll(4)).map(|_| dice.pick(&paths).to_owned()).collect();
                    let lines = rows(&jsonl);
                    assert_eq!(
                        cwds(Transcript::File(&jsonl), &asked),
                        cwds(Transcript::Rows(&lines), &asked),
                        "caso {case}, passo {step}: {asked:?}"
                    );
                    assert_eq!(
                        elsewhere_many(Transcript::File(&jsonl), &asked),
                        elsewhere_many(Transcript::Rows(&lines), &asked),
                        "menções, caso {case}, passo {step}: {asked:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn mentions_match_a_full_scan_as_the_conversation_grows() {
        let dir = tempfile::tempdir().unwrap();
        let (jsonl, shot) = (dir.path().join("c.jsonl"), dir.path().join("shots").join("final.png"));
        std::fs::create_dir_all(shot.parent().unwrap()).unwrap();
        std::fs::write(&shot, b"png").unwrap();
        // Mais menções do que a resposta leva: o que fica guardado é cortado no caminho.
        append(&jsonl, &(0..1100).map(|i| row("/proj", &format!("rascunho em pasta{i}/final.png"))).collect::<String>());
        for round in 0..3 {
            append(&jsonl, &row("/proj", &format!("print em {} e em shots/final.png, rodada {round}", shot.display())));
            let found = elsewhere(Transcript::File(&jsonl), "final.png");
            assert_eq!(found, elsewhere(Transcript::Rows(&rows(&jsonl)), "final.png"), "rodada {round}");
            // No Windows o caminho absoluto leva contrabarra, e este casamento só olha `/`.
            if cfg!(unix) {
                assert_eq!(found[0], json!([shot.to_string_lossy()]));
            }
            assert_eq!(found[1][0], "shots/final.png");
        }
    }
}
