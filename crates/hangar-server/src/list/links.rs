//! Campos comuns a toda linha: vida da sessão, par, encadeamento, loop, worktree, motor e conta
//! (`registry.py:1376-1431`), e a guarda de colisão de transcript (`:1219-1262`).
use super::discover_other::{Dirs, config_dir_of, env, truthy};
use super::procs::ProcessView;
use hangar_api::session::SessionRow;
use hangar_workspace::git::head_info;
use hangar_workspace::worktrees::{main_repo_of, normpath, repo_root_of, try_removed_at, worktree_paths};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use super::capped::Capped;
use super::discover::{DiscoveryProblem, Problems};

/// Linha com os padrões do `SessionInfo`.
pub fn blank_row(name: &str) -> SessionRow {
    serde_json::from_value(serde_json::json!({ "name": name })).expect("SessionRow só exige name")
}

/// `share_life.session_life` sem a parte de transferência (vem dos fatos): a chave do sidecar
/// vence; sem ela, o nascimento do terminal.
pub fn session_life(key: Option<&str>, birth: Option<u64>) -> Option<String> {
    if let Some(key) = key.filter(|k| !k.is_empty()) {
        return Some(format!("k:{key}"));
    }
    birth.filter(|b| *b > 0).map(|b| format!("t:{b}"))
}

/// `Path.resolve(strict=False)`: segue o link no que existe e junta o resto como está.
pub fn resolve_lenient(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    // O link é seguido antes do `..`, como no `resolve`; só o pedaço que não existe é lexical.
    for base in abs.ancestors() {
        if let Ok(real) = std::fs::canonicalize(base) {
            let real = without_verbatim(real);
            return match abs.strip_prefix(base) {
                Ok(rest) if !rest.as_os_str().is_empty() => normpath(&real.join(rest)),
                _ => real,
            };
        }
    }
    normpath(&abs)
}

/// O `canonicalize` do Windows devolve `\\?\C:\...`; o Python escreve `C:\...`, e a conta da sessão
/// (`claude:<pasta>`) precisa bater letra a letra com o id da credencial que ele monta.
fn without_verbatim(path: PathBuf) -> PathBuf {
    if !cfg!(windows) { return path; }
    path.to_str().and_then(verbatim_stripped).map_or(path, PathBuf::from)
}

fn verbatim_stripped(text: &str) -> Option<String> {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") { return Some(format!(r"\\{rest}")); }
    text.strip_prefix(r"\\?\").filter(|rest| rest.as_bytes().get(1) == Some(&b':')).map(str::to_owned)
}

/// `pqueue._sanitize`: nome do sidecar de vínculo (mantém o ponto, ao contrário do da sessão).
fn link_file(dir: &Path, name: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '-' }).collect();
    dir.join(format!("{safe}.json"))
}

/// JSON-objeto do sidecar; ausente, torto ou de outro tipo é "sem vínculo", como no Python. Só o
/// ausente é calado: torto ou ilegível vai ao diário, senão par, grupo ou loop somem sem motivo.
fn read_object(path: &Path, problems: &mut Problems) -> Option<Map<String, Value>> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            problems.note("list_link_unreadable", &path.to_string_lossy(), "vínculo (par, encadeamento ou loop) ilegível; tratado como ausente");
            return None;
        }
    };
    match serde_json::from_slice(&raw) {
        Ok(Value::Object(m)) => Some(m),
        _ => {
            problems.note("list_link_invalid", &path.to_string_lossy(), "vínculo (par, encadeamento ou loop) torto; tratado como ausente");
            None
        }
    }
}

struct Pair {
    peers: Vec<String>,
    /// `None` só quando o arquivo grava `task: null`; ausente é `""`.
    task: Option<String>,
    gid: String,
}

/// `PairLink.get`: legado `{"peer": x}` vira `peers`; sem membros só vale o grupo `orq`; sem `gid`
/// deriva um do conjunto, igual em todos os membros.
fn pair_of(name: &str, dirs: &Dirs, problems: &mut Problems) -> Option<Pair> {
    let data = read_object(&link_file(&dirs.claude.join(".hangar-pair"), name), problems)?;
    let raw = match data.get("peers") {
        Some(p) => p.clone(),
        None => data.get("peer").filter(|p| truthy(p)).map(|p| Value::Array(vec![p.clone()])).unwrap_or(Value::Null),
    };
    let peers: Vec<String> = raw.as_array().map(|a| a.iter().filter_map(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned).collect()).unwrap_or_default();
    if peers.is_empty() && data.get("orq") != Some(&Value::Bool(true)) {
        return None;
    }
    let task = match data.get("task") { None => Some(String::new()), Some(t) => t.as_str().map(str::to_owned) };
    let gid = data.get("gid").and_then(Value::as_str).filter(|g| !g.is_empty()).map(str::to_owned).unwrap_or_else(|| legacy_gid(name, &peers));
    Some(Pair { peers, task, gid })
}

fn legacy_gid(name: &str, peers: &[String]) -> String {
    let mut all: Vec<&str> = std::iter::once(name).chain(peers.iter().map(String::as_str)).collect();
    all.sort_unstable();
    sha1_smol::Sha1::from(all.join("\n")).digest().to_string()[..8].to_owned()
}

/// Campos do `ExternalPair`: faltando um, o Python recusa o arquivo inteiro.
const EXTERNAL_FIELDS: [&str; 8] = ["share_id", "local_session", "alias", "peer_owner", "peer_session", "peer_address", "peer_token", "created_at"];

fn external_unreadable(field: &str) {
    if crate::warn_limit::allow(None, "list_external_pairs_unreadable") {
        tracing::warn!(code = "list_external_pairs_unreadable", field, "list: external_pairs.json recusado, sem par externo");
    }
}

type ExternalRecords = Result<Vec<Map<String, Value>>, &'static str>;
static EXTERNAL_PAIRS: TailCache<ExternalRecords> = LazyLock::new(Default::default);

/// Registros de `external_pairs.json`, lidos uma vez por versão do arquivo; `Err` = o campo que
/// fez o Python recusar o arquivo inteiro.
fn external_records(path: &Path) -> Option<Arc<ExternalRecords>> {
    cached(&EXTERNAL_PAIRS, path, || {
        // Leitura que falhou não é guardada: o próximo tique tenta de novo.
        let raw = std::fs::read(path).ok()?;
        let Ok(Value::Array(records)) = serde_json::from_slice::<Value>(&raw) else { return Some(Err("json")) };
        Some(records.into_iter()
            .map(|r| match r {
                Value::Object(r) if EXTERNAL_FIELDS.iter().all(|f| r.contains_key(*f)) => Ok(r),
                _ => Err("record"),
            })
            .collect())
    })
}

/// `_pair_external`: o par de fora entre os peers da sessão. Arquivo torto vale como vazio, como
/// no Python; quem o põe de lado é o Python, aqui só se avisa.
fn pair_external(name: &str, peers: &[String], dirs: &Dirs) -> Option<Map<String, Value>> {
    let path = dirs.claude.join(".hangar-pair").join("external_pairs.json");
    let Some(records) = external_records(&path) else {
        // Ausente é o normal; existir e não ler é falha.
        if path.exists() { external_unreadable("file"); }
        return None;
    };
    let records = match &*records {
        Ok(records) => records,
        Err(field) => { external_unreadable(field); return None }
    };
    records.iter().find_map(|r| {
        let field = |k: &str| r.get(k).and_then(Value::as_str);
        if field("local_session")? != name {
            return None;
        }
        let address = format!("{}::{}", field("alias")?, field("peer_session")?);
        peers.contains(&address).then(|| {
            let mut out = Map::new();
            for (out_key, key) in [("alias", "alias"), ("owner", "peer_owner"), ("session", "peer_session")] {
                out.insert(out_key.to_owned(), r.get(key).cloned().unwrap_or(Value::Null));
            }
            out
        })
    })
}

/// Encadeamento e par (`ThenLink`, `PairLink`, `_pair_external`).
pub fn fill_links(row: &mut SessionRow, dirs: &Dirs, problems: &mut Problems) {
    row.then_target = read_object(&link_file(&dirs.claude.join(".hangar-chain"), &row.name), problems)
        .and_then(|l| l.get("target").and_then(Value::as_str).map(str::to_owned));
    match pair_of(&row.name, dirs, problems) {
        Some(pair) => {
            row.pair_external = pair_external(&row.name, &pair.peers, dirs);
            row.pair_peers = Some(pair.peers);
            row.pair_gid = Some(pair.gid);
            row.pair_task = pair.task;
        }
        None => (row.pair_external, row.pair_peers, row.pair_gid, row.pair_task) = (None, None, None, None),
    }
}

/// `_decorate_loop`: sem sidecar, nenhum badge. Devolve a falha contornada (sidecar torto), para
/// quem chama mandar ao diário.
pub fn fill_loop(row: &mut SessionRow, dirs: &Dirs) -> Option<DiscoveryProblem> {
    let mut problems = Problems::default();
    if let Some(d) = read_object(&link_file(&dirs.claude.join(".hangar-loop"), &row.name), &mut problems) {
        row.loop_status = d.get("status").and_then(Value::as_str).map(str::to_owned);
        row.loop_iter = count(d.get("iter"));
        row.loop_max = count(d.get("max_iters"));
    }
    problems.into_vec().pop()
}

/// Inteiro não negativo, ou float de valor inteiro (o pydantic aceita `3.0` num `int`).
fn count(v: Option<&Value>) -> Option<u32> {
    let f = v?.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0)?;
    u32::try_from(f as u64).ok()
}

/// Campos de uma linha de pane depois do transcript resolvido: vida, worktree, vínculos, motor e
/// conta. `provider` é o detectado no pane (`claude` quando não se reconhece); `pid_env` é o do
/// processo do agente, senão o do pane: quem declara conta e motor é o agente.
pub fn fill_pane_row(row: &mut SessionRow, provider: &str, pid_env: Option<i64>, birth: Option<u64>, procs: &dyn ProcessView, dirs: &Dirs,
                     problems: &mut Problems) {
    row.lifecycle_id = session_life(None, birth);
    // Transcript de chute (untracked) pode ser de outra sessão: não decide onde esta está.
    let jsonl = row.jsonl.clone().filter(|_| row.tracked);
    apply_location(row, locate(provider, row.cwd.as_deref(), jsonl.as_deref(), dirs, problems));
    fill_links(row, dirs, problems);
    if matches!(provider, "pi" | "omp" | "kimi" | "codex") {
        row.provider = provider.to_owned();
    }
    row.engine = pid_env.and_then(|p| env(procs, p, "CP_ENGINE"));
    row.engine_account = match (pid_env, &row.engine) {
        (Some(p), Some(_)) => env(procs, p, "CP_ENGINE_ACCOUNT"),
        _ => None,
    };
    row.conta = if let Some(engine) = &row.engine {
        if row.engine_account.is_some() { pid_env.and_then(|p| env(procs, p, "CP_ENGINE_CREDENTIAL_ID")) } else { Some(format!("chave:{engine}")) }
    } else {
        match provider {
            // Casada pelas credenciais no `cotas.py`: chega pelos fatos.
            "kimi" | "pi" | "omp" => None,
            "codex" => {
                let home = resolve_lenient(&dirs.codex_home).to_string_lossy().into_owned();
                row.codex_home = Some(home.clone());
                Some(format!("codex:{home}"))
            }
            _ => {
                let cdir = pid_env.and_then(|p| config_dir_of(procs, p)).unwrap_or_else(|| dirs.default_claude());
                Some(format!("claude:{}", resolve_lenient(&cdir).to_string_lossy()))
            }
        }
    };
}

/// Linha de sidecar: o transcript é sempre dela, então decide a worktree.
pub fn fill_location(row: &mut SessionRow, provider: &str, dirs: &Dirs, problems: &mut Problems) {
    let loc = locate(provider, row.cwd.as_deref(), row.jsonl.as_deref(), dirs, problems);
    apply_location(row, loc);
}

fn apply_location(row: &mut SessionRow, loc: Location) {
    row.branch = loc.branch;
    row.worktree = loc.worktree;
    row.worktree_path = loc.worktree_path;
    row.worktree_gone = loc.worktree_gone;
    row.git_cwd = loc.git_cwd;
}

/// 2+ linhas no mesmo transcript: só a dona (sid do cmdline = nome do arquivo, ou a única
/// `tracked`) fica com ele; as outras perdem o vínculo. Sem dona clara, todas perdem.
pub fn dedupe_collisions(rows: &mut [SessionRow], sids: &HashMap<String, Option<String>>) {
    let mut groups: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let Some(jsonl) = row.jsonl.as_deref().filter(|j| !j.is_empty()) else { continue };
        let real = resolve_lenient(Path::new(jsonl));
        match groups.iter_mut().find(|(k, _)| *k == real) {
            Some((_, members)) => members.push(i),
            None => groups.push((real, vec![i])),
        }
    }
    for (jsonl, members) in groups {
        if members.len() < 2 {
            continue;
        }
        let file = jsonl.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let base = file.strip_suffix(".jsonl").unwrap_or(&file).to_owned();
        let mut owner = members.iter().copied().find(|&i| sids.get(&rows[i].name).and_then(Option::as_deref) == Some(base.as_str()));
        if owner.is_none() {
            let tracked: Vec<usize> = members.iter().copied().filter(|&i| rows[i].tracked).collect();
            if tracked.len() == 1 {
                owner = Some(tracked[0]);
            }
        }
        for i in members {
            if Some(i) == owner {
                continue;
            }
            // Colisão dura enquanto as duas sessões vivem: uma linha por minuto basta.
            if crate::warn_limit::allow(Some(&rows[i].name), "list_collision") {
                tracing::info!(code = "list_collision", name = %rows[i].name, jsonl = %base,
                    owner = owner.map(|o| rows[o].name.as_str()).unwrap_or("none"), "transcript emprestado descartado");
            }
            rows[i].jsonl = None;
            rows[i].tracked = false;
        }
    }
}

// ── onde a sessão está (`worktrees.locate`) ──────────────────────────────────────────────────

#[derive(Debug, Default, PartialEq)]
pub struct Location {
    pub branch: Option<String>,
    pub worktree: bool,
    pub worktree_path: Option<String>,
    pub worktree_gone: bool,
    /// Raiz do repositório onde o agente trabalha, quando não é a da pasta de abertura.
    pub git_cwd: Option<String>,
}

fn is_dir(path: &str) -> bool { !path.is_empty() && Path::new(path).is_dir() }

/// Sessão que nasceu numa worktree fica nela; senão os sinais do transcript (Claude) ou dos
/// comandos (Codex) dizem para qual pasta do mesmo repositório o agente foi.
pub fn locate(provider: &str, cwd: Option<&str>, jsonl: Option<&str>, dirs: &Dirs, problems: &mut Problems) -> Location {
    let cwd = cwd.filter(|c| !c.is_empty());
    let born_in_worktree = head_info(cwd.and_then(repo_root_of).as_deref()).1;
    let mut real = None;
    if !born_in_worktree {
        // Transcript ilegível nunca derruba a lista: a sessão fica no cwd.
        real = match (provider, jsonl, cwd) {
            ("claude", Some(j), _) => claude_worktree(cwd, j, dirs, problems),
            ("codex", Some(j), Some(c)) => codex_cwd(c, j, dirs, problems),
            _ => None,
        };
    }
    let Some(real) = real.filter(|r| !r.is_empty()).or_else(|| cwd.map(str::to_owned)) else { return Location::default() };
    if !is_dir(&real) {
        // ponytail: pasta sumida que não é a de abertura vira "worktree apagada"; uma subpasta
        // comum apagada também cairia aqui, como no Python.
        let gone = Some(real.as_str()) != cwd || removed(dirs, problems).contains_key(&real);
        return Location { worktree_path: gone.then_some(real), worktree_gone: gone, ..Location::default() };
    }
    let root = repo_root_of(&real);
    let (branch, worktree) = head_info(Some(root.as_deref().unwrap_or(&real)));
    let moved = root.is_some() && root != cwd.and_then(repo_root_of);
    Location {
        branch,
        worktree,
        worktree_path: worktree.then(|| root.clone().unwrap_or_else(|| real.clone())),
        worktree_gone: false,
        git_cwd: if moved { root } else { None },
    }
}

static REMOVED: TailCache<HashMap<String, String>> = LazyLock::new(Default::default);

/// `worktrees-removidas.json`, relido só quando o arquivo muda; ausente é mapa vazio. Torto vale
/// vazio, como no Python, e não fica guardado: volta ao diário enquanto estiver torto.
fn removed(dirs: &Dirs, problems: &mut Problems) -> Arc<HashMap<String, String>> {
    let file = dirs.home.join(".hangar").join("worktrees-removidas.json");
    cached(&REMOVED, &file, || match try_removed_at(&file) {
        Ok(map) => Some(map),
        Err(_) => {
            problems.note("list_removed_worktrees_invalid", &file.to_string_lossy(), "worktrees-removidas.json ilegível ou torto; worktree apagada sai como pasta comum");
            None
        }
    }).unwrap_or_default()
}

/// Uma leitura por versão do arquivo: a lista roda a cada segundo e o transcript pode ter megas.
/// O valor sai por `Arc`: a mesma leitura serve todas as linhas do tique sem cópia.
type TailCache<T> = LazyLock<Mutex<Capped<PathBuf, ((i128, u64), Arc<T>)>>>;

fn file_key(path: &Path) -> Option<(i128, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i128).unwrap_or(0);
    Some((mtime, meta.len()))
}

fn cached<T>(cache: &TailCache<T>, path: &Path, read: impl FnOnce() -> Option<T>) -> Option<Arc<T>> {
    let key = file_key(path)?;
    if let Some((k, v)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path)
        && *k == key
    {
        return Some(v.clone());
    }
    let value = Arc::new(read()?);
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), (key, value.clone()));
    Some(value)
}

const TAIL: u64 = 256 * 1024;
const DEEP_TAIL: u64 = 8 * 1024 * 1024;

/// Linhas do fim para o começo, em blocos de `TAIL`, até `DEEP_TAIL` bytes (`reversed_lines`).
/// `visit` devolve `Break` para parar de ler: numa sessão ativa basta o último bloco.
fn reversed_lines(path: &Path, limit: u64, mut visit: impl FnMut(&[u8]) -> ControlFlow<()>) -> std::io::Result<()> {
    let mut fh = std::fs::File::open(path)?;
    // Até `limit`: o que cresceu depois do tamanho medido fica para a próxima leitura.
    let end = fh.seek(SeekFrom::End(0))?.min(limit);
    let mut pos = end;
    // Dois buffers trocados a cada bloco: nada é alocado nem zerado por bloco.
    let (mut rest, mut block): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::with_capacity(TAIL as usize));
    while pos > 0 && end - pos < DEEP_TAIL {
        let step = TAIL.min(pos);
        pos -= step;
        fh.seek(SeekFrom::Start(pos))?;
        block.clear();
        if (&mut fh).take(step).read_to_end(&mut block)? as u64 != step {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        block.extend_from_slice(&rest);
        if let Some(first) = block.iter().position(|b| *b == b'\n') {
            for line in block[first + 1..].rsplit(|b| *b == b'\n') {
                if visit(line).is_break() {
                    return Ok(());
                }
            }
            block.truncate(first);
        }
        std::mem::swap(&mut rest, &mut block);
    }
    if pos == 0 {
        let _ = visit(&rest);
    }
    Ok(())
}

static SHELL_DIR_RE: LazyLock<Regex> = LazyLock::new(|| {
    // O `\1` do Python (aspas casadas) vira três alternativas: o Rust não tem retrorreferência.
    Regex::new(r#"(?:(?:^|[\n;&|(])\s*cd|\bgit\s+-C)\s+(?:"([^\s;&|"')]+)"|'([^\s;&|"')]+)'|([^\s;&|"')]+))"#).unwrap()
});

/// (caminho, é `cd`?) de uma chamada, do último citado para o primeiro.
fn tool_paths(block: &Map<String, Value>) -> Vec<(String, bool)> {
    let Some(Value::Object(args)) = block.get("input") else { return Vec::new() };
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    if name == "Bash" && let Some(command) = args.get("command").and_then(Value::as_str) {
        let found: Vec<(String, bool)> = SHELL_DIR_RE.captures_iter(command)
            .filter_map(|c| c.get(1).or(c.get(2)).or(c.get(3)).map(|m| (m.as_str().to_owned(), true)))
            .collect();
        return found.into_iter().rev().collect();
    }
    let key = match name { "Edit" | "MultiEdit" | "Write" => "file_path", "NotebookEdit" => "notebook_path", _ => return Vec::new() };
    args.get(key).and_then(Value::as_str).map(|t| vec![(t.to_owned(), false)]).unwrap_or_default()
}

type ClaudeTail = (Option<String>, Vec<(String, bool, Option<String>)>);
type Hit = (String, bool, Option<String>);

/// Chamadas que bastam para decidir a pasta: a leitura para na linha que alcança esse número.
const HITS_ENOUGH: usize = 20;
/// Teto de frequência: a sessão ativa escreve no transcript a cada poucos segundos.
const TAIL_RECHECK: std::time::Duration = std::time::Duration::from_secs(10);

/// Uma leitura do transcript: os achados por linha (da mais recente para a mais antiga) e até
/// onde o arquivo foi lido, para a próxima ler só o que cresceu.
#[derive(Clone)]
struct TailScan {
    /// O que as linhas leem, montado uma vez por leitura.
    value: Arc<ClaudeTail>,
    checked: std::time::Instant,
    key: (i128, u64),
    /// Fim lido quando o arquivo terminava em `\n`; sem isso, a próxima relê do fim para trás.
    consumed: Option<u64>,
    last: Option<String>,
    lines: Vec<Vec<Hit>>,
}

impl TailScan {
    fn new(checked: std::time::Instant, key: (i128, u64), consumed: Option<u64>, last: Option<String>, lines: Vec<Vec<Hit>>) -> Self {
        let value = Arc::new((last.clone(), lines.iter().flatten().cloned().collect()));
        Self { value, checked, key, consumed, last, lines }
    }
}

static CLAUDE_TAILS: LazyLock<Mutex<Capped<PathBuf, TailScan>>> = LazyLock::new(Default::default);

/// Uma linha do transcript na leitura de trás para a frente: guarda o último `cwd` e as pastas das
/// chamadas; `Break` quando já há chamadas bastantes.
fn scan_line(raw: &[u8], last: &mut Option<String>, lines: &mut Vec<Vec<Hit>>, count: &mut usize) -> ControlFlow<()> {
    let has_tool = TOOL_USE.find(raw).is_some();
    // Achado o último `cwd`, só interessa linha com chamada: o resto pode ser imagem de megas.
    if !has_tool && (last.is_some() || CWD_KEY.find(raw).is_none()) {
        return ControlFlow::Continue(());
    }
    let Ok(Value::Object(line)) = serde_json::from_slice::<Value>(raw) else { return ControlFlow::Continue(()) };
    let cwd = line.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned);
    if last.is_none() {
        *last = cwd.clone();
    }
    if has_tool && let Some(Value::Array(content)) = line.get("message").and_then(|m| m.get("content")) {
        let mut hits = Vec::new();
        for block in content.iter().rev().filter_map(Value::as_object) {
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                hits.extend(tool_paths(block).into_iter().map(|(p, cd)| (p, cd, cwd.clone())));
            }
        }
        if !hits.is_empty() {
            *count += hits.len();
            lines.push(hits);
        }
    }
    if *count >= HITS_ENOUGH { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
}

/// Fim de arquivo terminado em `\n` (o Claude grava linha inteira): dali a próxima leitura segue.
fn ends_line(fh: &mut std::fs::File, end: u64) -> std::io::Result<Option<u64>> {
    if end == 0 {
        return Ok(Some(0));
    }
    fh.seek(SeekFrom::Start(end - 1))?;
    let mut b = [0u8; 1];
    fh.read_exact(&mut b)?;
    Ok((b[0] == b'\n').then_some(end))
}

/// Do zero: de trás para a frente até `DEEP_TAIL`.
fn scan_full(jsonl: &Path, key: (i128, u64), now: std::time::Instant) -> std::io::Result<TailScan> {
    let (mut last, mut lines, mut count) = (None, Vec::new(), 0);
    reversed_lines(jsonl, key.1, |raw| scan_line(raw, &mut last, &mut lines, &mut count))?;
    let mut fh = std::fs::File::open(jsonl)?;
    let consumed = ends_line(&mut fh, key.1)?;
    Ok(TailScan::new(now, key, consumed, last, lines))
}

/// Só o que cresceu desde `from`; os achados antigos completam os novos, na mesma regra de parada.
// ponytail: um achado antigo pode estar além dos 8 MB do fim novo, onde a leitura do zero não
// chegaria; continua sendo um sinal da mesma conversa.
fn scan_growth(jsonl: &Path, old: &TailScan, from: u64, key: (i128, u64), now: std::time::Instant) -> std::io::Result<TailScan> {
    let mut fh = std::fs::File::open(jsonl)?;
    // Arquivo trocado por outro maior: onde a leitura parou já não é fim de linha.
    if ends_line(&mut fh, from)? != Some(from) {
        return scan_full(jsonl, key, now);
    }
    fh.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::new();
    (&mut fh).take(key.1 - from).read_to_end(&mut buf)?;
    let (mut last, mut lines, mut count) = (None, Vec::new(), 0);
    for raw in buf.rsplit(|b| *b == b'\n') {
        if scan_line(raw, &mut last, &mut lines, &mut count).is_break() {
            break;
        }
    }
    for group in &old.lines {
        if count >= HITS_ENOUGH {
            break;
        }
        count += group.len();
        lines.push(group.clone());
    }
    let consumed = ends_line(&mut fh, key.1)?;
    Ok(TailScan::new(now, key, consumed, last.or_else(|| old.last.clone()), lines))
}

/// (último `cwd`, caminhos citados pelas ferramentas com o `cwd` da linha), do mais recente para o
/// mais antigo.
fn claude_tail(jsonl: &Path, problems: &mut Problems) -> Option<Arc<ClaudeTail>> { claude_tail_at(jsonl, std::time::Instant::now(), problems) }

fn claude_tail_at(jsonl: &Path, now: std::time::Instant, problems: &mut Problems) -> Option<Arc<ClaudeTail>> {
    {
        let mut tails = CLAUDE_TAILS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = tails.get(jsonl)
            && now.saturating_duration_since(old.checked) < TAIL_RECHECK
        {
            return Some(old.value.clone());
        }
    }
    let key = file_key(jsonl)?;
    // Só a versão que mudou paga a cópia dos achados antigos.
    let old = {
        let mut tails = CLAUDE_TAILS.lock().unwrap_or_else(|e| e.into_inner());
        match tails.get_mut(jsonl) {
            Some(old) if old.key == key => {
                old.checked = now;
                return Some(old.value.clone());
            }
            other => other.cloned(),
        }
    };
    let scan = match &old {
        Some(old) if old.consumed.is_some_and(|c| c <= key.1 && key.1 - c <= DEEP_TAIL) =>
            scan_growth(jsonl, old, old.consumed.unwrap_or_default(), key, now),
        _ => scan_full(jsonl, key, now),
    };
    let scan = match scan {
        Ok(scan) => scan,
        // Apagado ou trocado entre o `stat` e a leitura: corrida, a próxima rodada vê.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return old.map(|o| o.value),
        Err(_) => {
            // Transcript ilegível nunca derruba a lista: fica a última leitura boa, ou o cwd.
            problems.note("list_transcript_unreadable", &jsonl.to_string_lossy(), "transcript ilegível; worktree pela última leitura boa ou pelo cwd");
            return old.map(|o| o.value);
        }
    };
    let value = scan.value.clone();
    CLAUDE_TAILS.lock().unwrap_or_else(|e| e.into_inner()).insert(jsonl.to_path_buf(), scan);
    Some(value)
}

// Duas buscas por linha em até 8 MB de transcript: o `windows().any()` comparava byte a byte.
static TOOL_USE: LazyLock<memchr::memmem::Finder<'static>> = LazyLock::new(|| memchr::memmem::Finder::new(b"\"tool_use\""));
static CWD_KEY: LazyLock<memchr::memmem::Finder<'static>> = LazyLock::new(|| memchr::memmem::Finder::new(b"\"cwd\""));
static CALL: LazyLock<memchr::memmem::Finder<'static>> = LazyLock::new(|| memchr::memmem::Finder::new(b"_call"));

/// Criar ou remover worktree muda o mtime de `.git/worktrees`; o `git worktree move` só reescreve o
/// `gitdir` lá dentro, e o teto cobre esse caso.
/// Curto também porque o `git worktree add` cria a pasta antes de escrever o `gitdir` nela.
const WORKTREES_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(5);

type WorktreeList = (Option<std::time::SystemTime>, std::time::Instant, Arc<Vec<String>>);
static WORKTREES: LazyLock<Mutex<Capped<String, WorktreeList>>> = LazyLock::new(Default::default);

/// `worktree_paths` do repositório principal, relido só quando a pasta de administração muda.
fn worktrees_of(main: &str) -> Arc<Vec<String>> {
    let stamp = std::fs::metadata(Path::new(main).join(".git").join("worktrees")).and_then(|m| m.modified()).ok();
    if let Some((s, at, paths)) = WORKTREES.lock().unwrap_or_else(|e| e.into_inner()).get(main)
        && *s == stamp
        && at.elapsed() < WORKTREES_MAX_AGE
    {
        return paths.clone();
    }
    let paths = Arc::new(worktree_paths(main));
    WORKTREES.lock().unwrap_or_else(|e| e.into_inner()).insert(main.to_owned(), (stamp, std::time::Instant::now(), paths.clone()));
    paths
}

/// (principal, worktrees removidas, todas as pastas) do repositório que contém `path`.
fn repo_candidates(path: &str, dirs: &Dirs, problems: &mut Problems) -> Option<(String, Vec<String>, Vec<String>)> {
    let main = main_repo_of(&repo_root_of(path)?);
    let gone: Vec<String> = removed(dirs, problems).iter().filter(|(_, v)| **v == main).map(|(k, _)| k.clone()).collect();
    let mut all = vec![main.clone()];
    all.extend(worktrees_of(&main).iter().cloned());
    all.extend(gone.iter().cloned());
    Some((main, gone, all))
}

/// Separadores do caminho: no Windows o transcript e o rollout trazem `\\` e `/` misturados.
const SEPS: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };

/// `rest` começa num separador: `path` está dentro de `base`.
fn below(path: &str, base: &str) -> bool {
    path.strip_prefix(base.trim_end_matches(SEPS)).is_some_and(|rest| rest.starts_with(SEPS))
}

fn owner(path: &str, candidates: &[String]) -> Option<String> {
    candidates.iter()
        .filter(|c| path == c.as_str() || below(path, c))
        .fold(None::<&String>, |best, c| if best.is_none_or(|b| c.len() > b.len()) { Some(c) } else { best })
        .cloned()
}

/// Pasta sumida só conta se for deste repositório, de uma worktree removida dele ou de uma irmã
/// no padrão do Hangar (`<repo>-<x>`).
fn of_this_repo(path: &str, main: &str, gone: &[String]) -> bool {
    let mut own = vec![main.to_owned()];
    own.extend(gone.iter().cloned());
    if owner(path, &own).is_some() {
        return true;
    }
    let trimmed = main.trim_end_matches(SEPS);
    let (parent, name) = trimmed.rsplit_once(SEPS).unwrap_or(("", trimmed));
    path.strip_prefix(parent.trim_end_matches(SEPS)).and_then(|rest| rest.strip_prefix(SEPS))
        .is_some_and(|rest| rest.split(SEPS).next().unwrap_or("").starts_with(&format!("{name}-")))
}

fn expand_user(path: &str, dirs: &Dirs) -> String {
    match path.strip_prefix('~') {
        Some("") => dirs.home.to_string_lossy().into_owned(),
        Some(rest) if rest.starts_with(SEPS) => format!("{}{rest}", dirs.home.to_string_lossy()),
        _ => path.to_owned(),
    }
}

/// `os.path.normpath(os.path.join(base, p))`.
fn join_norm(base: &str, p: &str) -> String {
    normpath(&Path::new(base).join(p)).to_string_lossy().into_owned()
}

/// A pasta do mesmo repositório onde o Claude trabalha: `cd X`/`git -C X` e o arquivo editado.
/// Nada na principal tira a sessão da worktree.
fn claude_worktree(cwd: Option<&str>, jsonl: &str, dirs: &Dirs, problems: &mut Problems) -> Option<String> {
    let tail = claude_tail(Path::new(jsonl), problems)?;
    let (last, hits) = &*tail;
    let last = last.clone();
    let Some(base) = last.clone().or_else(|| cwd.map(str::to_owned)) else { return last };
    let Some((main, gone, candidates)) = repo_candidates(&base, dirs, problems) else { return last };
    let home = owner(&base, &candidates);
    for (raw, is_cd, line_cwd) in hits {
        let is_cd = *is_cd;
        if let Some(lc) = &line_cwd
            && owner(lc, &candidates) != home
        {
            break; // chamada anterior à última troca de `cwd` (EnterWorktree/ExitWorktree)
        }
        let p = join_norm(line_cwd.as_deref().unwrap_or(&base), &expand_user(raw, dirs));
        if Path::new(&p).exists() {
            if let Some(o) = owner(&p, &candidates).filter(|o| *o != main) {
                return Some(o);
            }
        } else if is_cd && (Path::new(raw).has_root() || raw.starts_with('~')) && of_this_repo(&p, &main, &gone) {
            // Pasta absoluta que sumiu: a worktree foi removida. `cd -` e `cd $W` não dizem nada.
            return Some(owner(&p, &gone).unwrap_or(p));
        }
    }
    last
}

/// Início de caminho absoluto no texto JSON dos argumentos; no Windows também `C:\\` (a barra
/// vem escapada, e o achado é desescapado) e `C:/`. O patch segue só com `/`, como no Python.
const ABS: &str = if cfg!(windows) { r"(?:/|[A-Za-z]:(?:\\\\|/))" } else { "/" };
static WORKDIR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r#""?workdir"?\s*:\s*"({ABS}[^"]+)""#)).unwrap());
static CD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r#""?cmd"?\s*:\s*"\s*cd\s+({ABS}[^\s&;"]+)"#)).unwrap());
static PATCH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\*\*\* (?:Add|Update|Delete) File: (/[^\s\\"]+)"#).unwrap());
static CODEX_PATHS: TailCache<Vec<(String, bool)>> = LazyLock::new(Default::default);

/// Últimos `TAIL` bytes e se a primeira linha deles vem cortada (arquivo maior que isso). Quem lê
/// percorre as linhas no próprio buffer: copiar cada uma custava o arquivo inteiro por leitura.
fn tail_bytes(path: &Path) -> std::io::Result<(Vec<u8>, bool)> {
    let mut fh = std::fs::File::open(path)?;
    let size = fh.seek(SeekFrom::End(0))?;
    fh.seek(SeekFrom::Start(size.saturating_sub(TAIL)))?;
    let mut buf = Vec::with_capacity(size.min(TAIL) as usize);
    fh.read_to_end(&mut buf)?;
    Ok((buf, size > TAIL))
}

/// (caminho, é pasta?) dos comandos do Codex, da chamada mais recente para a mais antiga: `cd`
/// vence `workdir`, e os dois vencem o arquivo do patch.
fn codex_paths(rollout: &Path, problems: &mut Problems) -> Option<Arc<Vec<(String, bool)>>> {
    cached(&CODEX_PATHS, rollout, || {
        let mut out = Vec::new();
        let (buf, cut) = match tail_bytes(rollout) {
            Ok(read) => read,
            Err(_) => {
                problems.note("list_rollout_unreadable", &rollout.to_string_lossy(), "rollout do Codex ilegível; worktree fica no cwd");
                return None;
            }
        };
        let mut lines = buf.split(|b| *b == b'\n');
        if cut {
            lines.next();
        }
        for raw in lines.rev() {
            if CALL.find(raw).is_none() {
                continue;
            }
            let Ok(Value::Object(line)) = serde_json::from_slice::<Value>(raw) else { continue };
            let Some(Value::Object(payload)) = line.get("payload") else { continue };
            if !matches!(payload.get("type").and_then(Value::as_str), Some("function_call" | "custom_tool_call")) {
                continue;
            }
            let text = [payload.get("arguments"), payload.get("input")].into_iter().flatten()
                .find(|v| truthy(v));
            let Some(Value::String(text)) = text else { continue };
            for (rx, is_dir) in [(&*CD_RE, true), (&*WORKDIR_RE, true), (&*PATCH_RE, false)] {
                let found: Vec<String> = rx.captures_iter(text)
                    .map(|c| if cfg!(windows) && is_dir { c[1].replace(r"\\", r"\") } else { c[1].to_owned() }).collect();
                out.extend(found.into_iter().rev().map(|p| (p, is_dir)));
            }
            if out.len() >= 50 {
                break;
            }
        }
        Some(out)
    })
}

/// A worktree (ou a principal) do MESMO repositório onde o último comando do Codex rodou.
fn codex_cwd(cwd: &str, rollout: &str, dirs: &Dirs, problems: &mut Problems) -> Option<String> {
    let (main, gone, candidates) = repo_candidates(cwd, dirs, problems)?;
    for (p, is_dir) in codex_paths(Path::new(rollout), problems)?.iter() {
        if Path::new(p).exists() {
            if let Some(o) = owner(p, &candidates) {
                return Some(o);
            }
        } else if *is_dir && of_this_repo(p, &main, &gone) {
            // Pasta que sumiu: volta como está para o `locate` marcar "apagada".
            return Some(owner(p, &gone).unwrap_or_else(|| p.clone()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_dir_regex_matches_python_quotes() {
        let paths = |cmd: &str| {
            let mut block = Map::new();
            block.insert("name".into(), "Bash".into());
            block.insert("input".into(), serde_json::json!({ "command": cmd }));
            tool_paths(&block).into_iter().map(|(p, _)| p).collect::<Vec<_>>()
        };
        assert_eq!(paths("cd /a && git -C '/b' status; cd \"/c\""), ["/c", "/b", "/a"]);
        // Aspa sem par não casa, como o `\1` do Python.
        assert_eq!(paths("cd \"/x y\""), Vec::<String>::new());
        assert_eq!(paths("echo cd /nao"), Vec::<String>::new());
    }

    #[test]
    fn verbatim_prefix_is_written_like_python() {
        assert_eq!(verbatim_stripped(r"\\?\C:\Users\u\.claude-x").as_deref(), Some(r"C:\Users\u\.claude-x"));
        assert_eq!(verbatim_stripped(r"\\?\UNC\srv\share\d").as_deref(), Some(r"\\srv\share\d"));
        // Volume sem letra não tem forma curta: fica como veio.
        assert_eq!(verbatim_stripped(r"\\?\Volume{abc}\d"), None);
        assert_eq!(verbatim_stripped(r"C:\Users\u"), None);
    }

    #[cfg(windows)]
    #[test]
    fn resolved_account_dir_has_no_verbatim_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_lenient(&tmp.path().join("ainda-nao-existe")).to_string_lossy().into_owned();
        assert!(!resolved.starts_with(r"\\?\"), "{resolved}");
        assert!(resolved.ends_with(r"\ainda-nao-existe"), "{resolved}");
    }

    fn dirs_at(home: &Path) -> Dirs {
        Dirs { home: home.to_path_buf(), claude: home.join(".claude"), codex_home: home.join(".codex"),
            pi_sessions: home.join(".pi"), omp_config: home.join(".omp"), omp_agent: home.join(".omp/agent"),
            kimi_home: home.join(".kimi-code") }
    }

    #[test]
    fn broken_links_reach_the_diary() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        for (sub, body) in [(".hangar-pair", "{"), (".hangar-chain", "[1]"), (".hangar-loop", "{")] {
            std::fs::create_dir_all(dirs.claude.join(sub)).unwrap();
            std::fs::write(dirs.claude.join(sub).join("s.json"), body).unwrap();
        }
        let mut row = blank_row("s");
        let mut problems = Problems::default();
        fill_links(&mut row, &dirs, &mut problems);
        assert_eq!((&row.pair_peers, &row.then_target), (&None, &None),"torto continua valendo ausente, como no Python");
        assert_eq!(problems.into_vec().iter().map(|p| p.code).collect::<Vec<_>>(), ["list_link_invalid", "list_link_invalid"]);
        assert_eq!(fill_loop(&mut row, &dirs).map(|p| p.code), Some("list_link_invalid"));
        // Ausente é o normal: nada vai ao diário.
        let mut problems = Problems::default();
        fill_links(&mut blank_row("outra"), &dirs, &mut problems);
        assert!(problems.into_vec().is_empty());
    }

    #[test]
    fn broken_removed_worktrees_is_reported_and_not_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = dirs_at(tmp.path());
        let file = tmp.path().join(".hangar/worktrees-removidas.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "{").unwrap();
        let mut problems = Problems::default();
        assert!(removed(&dirs, &mut problems).is_empty());
        assert_eq!(problems.into_vec().iter().map(|p| p.code).collect::<Vec<_>>(), ["list_removed_worktrees_invalid"]);
        std::fs::write(&file, r#"{"/r-wt": "/r"}"#).unwrap();
        assert_eq!(removed(&dirs, &mut Problems::default()).get("/r-wt").map(String::as_str), Some("/r"));
    }

    #[cfg(windows)]
    #[test]
    fn owner_accepts_backslash_on_windows() {
        let c = vec![r"C:\r".to_owned(), r"C:\r\wt".to_owned()];
        assert_eq!(owner(r"C:\r\wt\x", &c).as_deref(), Some(r"C:\r\wt"));
        assert!(of_this_repo(r"C:\r-novo\x", r"C:\r", &[]));
        let text = r#"{"workdir":"C:\\r\\wt"}"#;
        assert_eq!(WORKDIR_RE.captures(text).map(|c| c[1].replace(r"\\", r"\")).as_deref(), Some(r"C:\r\wt"));
    }

    #[test]
    fn owner_takes_longest_and_sibling_rule() {
        let c = vec!["/r".to_owned(), "/r/wt".to_owned()];
        assert_eq!(owner("/r/wt/x", &c).as_deref(), Some("/r/wt"));
        assert_eq!(owner("/rx", &c), None);
        assert!(of_this_repo("/r-feat/a", "/r", &[]));
        assert!(!of_this_repo("/outro/a", "/r", &[]));
    }

    #[test]
    fn reversed_lines_crosses_blocks_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        // Linhas de tamanho variado: alguma cruza a borda de 256 KB entre dois blocos.
        let lines: Vec<String> = (0..9000).map(|i| format!("{i}:{}", "x".repeat(i % 97))).collect();
        std::fs::write(&path, lines.join("\n")).unwrap();
        let mut seen = Vec::new();
        reversed_lines(&path, u64::MAX, |l| { seen.push(String::from_utf8(l.to_vec()).unwrap()); ControlFlow::Continue(()) }).unwrap();
        assert_eq!(seen, lines.iter().rev().cloned().collect::<Vec<_>>());
        let mut n = 0;
        reversed_lines(&path, u64::MAX, |_| { n += 1; if n == 3 { ControlFlow::Break(()) } else { ControlFlow::Continue(()) } }).unwrap();
        assert_eq!(n, 3);
    }

    fn set_mtime(path: &Path, at: std::time::SystemTime) {
        // Também recebe pasta. No Windows o mtime pede FILE_WRITE_ATTRIBUTES, e pasta só abre com
        // FILE_FLAG_BACKUP_SEMANTICS; no Linux pasta não abre para escrita.
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::File::options().access_mode(0x100).custom_flags(0x0200_0000).open(path).unwrap()
        };
        #[cfg(not(windows))]
        let file = std::fs::File::open(path).unwrap();
        file.set_modified(at).unwrap();
    }

    #[test]
    fn claude_tail_waits_then_reads_only_growth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let line = |cwd: &str, to: &str| format!(
            r#"{{"cwd":"{cwd}","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{"command":"cd {to}"}}}}]}}}}"#) + "\n";
        let first = line("/a", "/x1");
        std::fs::write(&path, &first).unwrap();
        let t0 = std::time::Instant::now();
        let paths = |v: Arc<ClaudeTail>| (v.0.clone(), v.1.iter().map(|h| h.0.clone()).collect::<Vec<_>>());
        assert_eq!(paths(claude_tail_at(&path, t0, &mut Problems::default()).unwrap()), (Some("/a".into()), vec!["/x1".into()]));
        // O começo muda sem mudar de tamanho (prova de que não é relido) e uma linha nova chega.
        std::fs::write(&path, first.replace("/x1", "/y1") + &line("/b", "/x2")).unwrap();
        let soon = t0 + std::time::Duration::from_secs(1);
        assert_eq!(paths(claude_tail_at(&path, soon, &mut Problems::default()).unwrap()), (Some("/a".into()), vec!["/x1".into()]), "dentro do teto reusa");
        let later = t0 + TAIL_RECHECK + std::time::Duration::from_secs(1);
        assert_eq!(paths(claude_tail_at(&path, later, &mut Problems::default()).unwrap()), (Some("/b".into()), vec!["/x2".into(), "/x1".into()]),
            "só o que cresceu é lido");
        // Encolheu: lido do zero.
        std::fs::write(&path, line("/c", "/x3")).unwrap();
        let again = later + TAIL_RECHECK + std::time::Duration::from_secs(1);
        assert_eq!(paths(claude_tail_at(&path, again, &mut Problems::default()).unwrap()), (Some("/c".into()), vec!["/x3".into()]));
        // Trocado por outro maior cujo byte no ponto lido não é fim de linha: lido do zero.
        std::fs::write(&path, line("/dddd", "/x4") + &line("/e", "/x5")).unwrap();
        let last = again + TAIL_RECHECK + std::time::Duration::from_secs(1);
        assert_eq!(paths(claude_tail_at(&path, last, &mut Problems::default()).unwrap()), (Some("/e".into()), vec!["/x5".into(), "/x4".into()]));
    }

    #[test]
    fn worktree_list_follows_admin_dir() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("r");
        let admin = main.join(".git/worktrees");
        let add = |name: &str| {
            std::fs::create_dir_all(admin.join(name)).unwrap();
            std::fs::write(admin.join(name).join("gitdir"), format!("{}/{name}/.git\n", dir.path().display())).unwrap();
        };
        add("a");
        let main_s = main.to_str().unwrap();
        let wt = |n: &str| dir.path().join(n).to_string_lossy().into_owned();
        assert_eq!(*worktrees_of(main_s), [wt("a")]);
        // `gitdir` reescrito sem a pasta de administração mudar: a lista guardada vale.
        std::fs::write(admin.join("a/gitdir"), format!("{}/z/.git\n", dir.path().display())).unwrap();
        assert_eq!(*worktrees_of(main_s), [wt("a")], "pasta de administração igual, nada relido");
        add("b");
        set_mtime(&admin, std::time::SystemTime::now() + std::time::Duration::from_secs(5));
        assert_eq!(*worktrees_of(main_s), [wt("z"), wt("b")]);
    }

    #[test]
    fn session_life_prefers_key() {
        assert_eq!(session_life(Some("k1"), Some(5)).as_deref(), Some("k:k1"));
        assert_eq!(session_life(None, Some(5)).as_deref(), Some("t:5"));
        assert_eq!(session_life(Some(""), None), None);
    }
}
