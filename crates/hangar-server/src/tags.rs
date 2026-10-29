//! Etiqueta da máquina e das contas, gravada no servidor de cada máquina: todos os clientes mostram
//! o mesmo nome, ícone e cor, e a troca chega pela lista do dono (evento `tags`).
use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::{Method, StatusCode}, response::Response};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{collections::{BTreeMap, HashMap, HashSet}, io, net::SocketAddr, path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, AtomicU64, Ordering}}};
use tokio::sync::Notify;
use crate::accounts::{environment::AccountEnvironment, storage::{CLAUDE_MARKER, CODEX_MARKER, PENDING}};
use crate::{accounts::{AccountService, quotas::other}, routes::{AppState, gate}, session_write::{detail_body, json_response}};

const MAX_ACCOUNTS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Color { Slate, Blue, Green, Lime, Amber, Orange, Pink, Violet }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MachineIcon { Laptop, Monitor, Server, House, Briefcase, Cloud }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountIcon { CreditCard, Provider, KeyRound, Wallet, UserRound, CircleUser, Contact, BadgeCheck, AtSign, Ticket, IdCard }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tag { pub name: String, pub icon: MachineIcon, pub color: Option<Color> }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountTag { pub icon: AccountIcon, pub color: Option<Color> }

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    #[serde(default)] machine: Option<Tag>,
    #[serde(default)] accounts: BTreeMap<String, AccountTag>,
}

/// Chave presente com `null` é diferente de chave ausente: a primeira apaga, a segunda não toca.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountChange { id: String, #[serde(default, deserialize_with = "present")] tag: Option<Option<AccountTag>> }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    #[serde(default, deserialize_with = "present")] machine: Option<Option<Tag>>,
    account: Option<AccountChange>,
}

impl Change {
    fn valid(&self) -> bool {
        self.machine.as_ref().and_then(Option::as_ref).is_none_or(|tag| tag.name.chars().count() <= 24)
            && self.account.as_ref().is_none_or(|a| a.tag.is_some() && !a.id.is_empty() && a.id.chars().count() <= 512)
    }
}

fn path(home: &Path) -> PathBuf { home.join(".hangar").join("tags.json") }

fn read(path: &Path) -> io::Result<Doc> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Doc::default()),
        Err(error) => Err(error),
    }
}

enum Failure { Full, Io }

fn apply(path: &Path, change: Option<Change>) -> Result<Doc, Failure> {
    let io = |_| Failure::Io;
    let Some(change) = change else { return read(path).map_err(io) };
    std::fs::create_dir_all(path.parent().ok_or(Failure::Io)?).map_err(io)?;
    let lock = std::fs::OpenOptions::new().read(true).append(true).create(true).open(path.with_extension("lock")).map_err(io)?;
    lock.lock().map_err(io)?;
    let mut doc = read(path).map_err(io)?;
    if let Some(machine) = change.machine { doc.machine = machine; }
    if let Some(AccountChange { id, tag: Some(tag) }) = change.account {
        match tag {
            None => { doc.accounts.remove(&id); }
            Some(_) if !doc.accounts.contains_key(&id) && doc.accounts.len() >= MAX_ACCOUNTS => return Err(Failure::Full),
            Some(tag) => { doc.accounts.insert(id, tag); }
        }
    }
    crate::runtime::queue::atomic_write(path, &serde_json::to_vec(&doc).map_err(|_| Failure::Io)?).map_err(io)?;
    Ok(doc)
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    json_response(status, detail_body(code, message, json!({})))
}

pub async fn handle(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !gate(&state, peer, &request).1 {
        return failure(StatusCode::FORBIDDEN, "tags_owner_only", "só o dono pode ler ou alterar as etiquetas");
    }
    let change = if request.method() == Method::PUT {
        let parsed = to_bytes(request.into_body(), 16 * 1024).await.ok()
            .and_then(|raw| serde_json::from_slice::<Change>(&raw).ok()).filter(Change::valid);
        let Some(change) = parsed else {
            return failure(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "etiqueta inválida");
        };
        Some(change)
    } else { None };
    let saving = change.is_some();
    let path = path(&state.accounts.env.home);
    match tokio::task::spawn_blocking(move || apply(&path, change)).await {
        Ok(Ok(doc)) => {
            if saving { state.list.tags.invalidate(); }
            json_response(StatusCode::OK, json!(doc))
        }
        Ok(Err(Failure::Full)) => failure(StatusCode::CONFLICT, "tags_full", "etiquetas demais; apague a de uma conta antes"),
        _ => failure(StatusCode::SERVICE_UNAVAILABLE, "tags_unavailable", "não foi possível ler ou guardar as etiquetas"),
    }
}

/// O que cada conta de `/api/credenciais` mostra: o apelido, senão o nome natural. A lista é a da
/// tela de Contas (`listed_ids`), sem login nem cota.
/// Volta o id de `/api/credenciais` (o que o editor grava) e o nome.
fn known_accounts(accounts: &AccountService) -> Result<Vec<(String, Value)>, &'static str> {
    let config = other::load(&accounts.env);
    let (listed, problems) = crate::accounts::credentials::listed_ids(accounts, &config);
    if let Some(&(_, code)) = problems.first() { return Err(code); }
    Ok(listed.into_iter().map(|l| {
        let name = config.aliases.get(&l.id).cloned().unwrap_or(l.natural);
        (l.id, name)
    }).collect())
}

/// O id como a `conta` da linha da lista o escreve: pasta que ainda não existe sob um link (HOME por
/// link sem `~/.claude`) sai resolvida pelo link, e o cliente casa a linha e a etiqueta pelo texto.
fn row_id(id: String) -> String {
    match id.split_once(':') {
        Some((kind @ ("claude" | "codex"), path)) =>
            format!("{kind}:{}", crate::list::links::resolve_lenient(std::path::Path::new(path)).to_string_lossy()),
        _ => id,
    }
}

/// O evento `tags` da lista do dono.
pub(crate) fn event(accounts: &AccountService, machine_id: &str) -> Result<Value, &'static str> {
    let doc = read(&path(&accounts.env.home)).map_err(|_| "tags_unreadable")?;
    let mut out = Map::new();
    for (id, name) in known_accounts(accounts)? {
        let name = name.as_str().map(|n| n.split('@').next().unwrap_or(n)).unwrap_or_default();
        // A etiqueta foi gravada pelo id da tela de Contas; a chave do evento é o da linha da lista.
        let tag = doc.accounts.get(&id);
        out.insert(row_id(id), json!({"name": name, "icon": tag.map_or(AccountIcon::CreditCard, |t| t.icon), "color": tag.and_then(|t| t.color)}));
    }
    Ok(json!({"machine_id": (!machine_id.is_empty()).then_some(machine_id), "machine": doc.machine, "accounts": out}))
}

type Stamp = Option<(u64, Option<std::time::SystemTime>)>;

fn stamp(path: &Path) -> Stamp { std::fs::symlink_metadata(path).ok().map(|m| (m.len(), m.modified().ok())) }

/// Dentro de uma pasta de conta, o que a torna conta.
const ACCOUNT_FILES: [&str; 5] = [CLAUDE_MARKER, CODEX_MARKER, PENDING, ".credentials.json", "projects"];

fn account_name(name: &str) -> bool { name.starts_with(".claude") || name.starts_with(".codex") }

/// O que o evento lê fora das pastas de conta: etiquetas, apelidos (nome novo e antigo), motores e
/// config do Kimi.
fn config_files(env: &AccountEnvironment) -> Vec<PathBuf> {
    let shared = env.home.join(".claude");
    let engines = env.base.get("CP_ENGINES_FILE").filter(|p| !p.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| shared.join("engines.json"));
    vec![path(&env.home), shared.join(".hangar-apelidos.json"), shared.join(".claude-pocket-apelidos.json"),
        engines, other::kimi_config(env)]
}

/// As pastas `.claude*`/`.codex*` da HOME e as padrão, que podem morar fora dela.
fn account_dirs(env: &AccountEnvironment) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&env.home).into_iter().flatten().flatten()
        .filter(|e| e.file_name().to_str().is_some_and(account_name) && e.path().is_dir())
        .map(|e| e.path()).collect();
    dirs.extend([env.claude_base.clone(), env.codex_default.clone()]);
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Reserva sem vigia: só metadados (tamanho e mtime) do que o evento lê. A mtime da HOME fica fora:
/// qualquer arquivo do usuário a mexe. O caminho entra junto: renomear a pasta de uma conta não
/// muda mtime nenhum, mas muda o id dela.
fn signature(env: &AccountEnvironment) -> Vec<(PathBuf, Stamp)> {
    let mut files = config_files(env);
    for dir in account_dirs(env) {
        files.extend(ACCOUNT_FILES.map(|name| dir.join(name)));
        files.push(dir);
    }
    files.into_iter().map(|f| { let s = stamp(&f); (f, s) }).collect()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

/// Identidade da pasta; `None` se não é pasta. Fora do Unix, só a existência.
fn dir_inode(dir: &Path) -> Option<u64> {
    let meta = std::fs::metadata(dir).ok().filter(std::fs::Metadata::is_dir)?;
    #[cfg(unix)]
    { use std::os::unix::fs::MetadataExt; Some(meta.ino()) }
    #[cfg(not(unix))]
    { let _ = meta; Some(0) }
}

fn wake(generation: &AtomicU64, changed: &Notify) {
    generation.fetch_add(1, Ordering::AcqRel);
    changed.notify_one();
}

/// O que acorda o evento: arquivos exatos e as pastas observadas (nascer ou sumir pede rearmar).
#[derive(Default)]
struct Wanted { home: PathBuf, files: HashSet<PathBuf>, dirs: HashSet<PathBuf> }

impl Wanted {
    /// `Some(rearmar)` se o caminho importa. `~/.claude.json` começa com `.claude` mas não é pasta.
    fn classify(&self, p: &Path) -> Option<bool> {
        if self.files.contains(p) { return Some(false); }
        let account = p.parent() == Some(self.home.as_path())
            && p.file_name().and_then(|n| n.to_str()).is_some_and(account_name) && p.is_dir();
        (account || self.dirs.contains(p)).then_some(true)
    }
}

/// Vigia das fontes do evento: um observador só, sem recursão. O callback só invalida e pede o
/// rearme, que roda na próxima leitura: armar de dentro do callback travaria a thread do notify.
struct Watch {
    watcher: RecommendedWatcher,
    /// Pasta armada e o inode dela: recriada, o observador da velha já foi solto pelo sistema.
    armed: HashMap<PathBuf, u64>,
    wanted: Arc<Mutex<Wanted>>,
    rescan: Arc<AtomicBool>,
}

impl Watch {
    fn start(env: &AccountEnvironment, generation: Arc<AtomicU64>, changed: Arc<Notify>) -> notify::Result<Self> {
        let wanted = Arc::new(Mutex::new(Wanted::default()));
        let rescan = Arc::new(AtomicBool::new(false));
        let (seen, again) = (wanted.clone(), rescan.clone());
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let hit = match res {
                // Fila do sistema transbordou ou o vigia falhou: o que mudou não está nos avisos.
                Ok(ev) if ev.need_rescan() => Some(true),
                Err(_) => Some(true),
                // O notify 7 assina abrir/fechar: a leitura do próprio evento o acordaria sem fim.
                Ok(ev) if ev.kind.is_access() => None,
                Ok(ev) => { let w = lock(&seen); ev.paths.iter().filter_map(|p| w.classify(p)).reduce(|a, b| a || b) }
            };
            let Some(rearm) = hit else { return };
            if rearm { again.store(true, Ordering::Release); }
            wake(&generation, &changed);
        })?;
        let mut me = Self { watcher, armed: HashMap::new(), wanted, rescan };
        me.arm(env)?;
        Ok(me)
    }

    /// Arma a HOME, as pastas dos arquivos lidos e as de conta; solta as que sumiram.
    /// shortcut: pasta ausente fora da HOME (KIMI_CODE_HOME, CP_ENGINES_FILE, CLAUDE_CONFIG_DIR) só
    /// é armada no próximo rearme; vigiar o pai dela se isso aparecer.
    fn arm(&mut self, env: &AccountEnvironment) -> notify::Result<()> {
        let files = config_files(env);
        let accounts = account_dirs(env);
        let mut dirs: Vec<PathBuf> = files.iter().filter_map(|f| f.parent().map(Path::to_path_buf)).collect();
        dirs.push(env.home.clone());
        dirs.extend(accounts.iter().cloned());
        dirs.sort();
        dirs.dedup();
        let mut wanted: HashSet<PathBuf> = files.into_iter().collect();
        for dir in &accounts { wanted.extend(ACCOUNT_FILES.map(|name| dir.join(name))); }
        *lock(&self.wanted) = Wanted { home: env.home.clone(), files: wanted, dirs: dirs.iter().cloned().collect() };
        let gone: Vec<PathBuf> = self.armed.iter()
            .filter(|(d, ino)| !dirs.contains(d) || dir_inode(d) != Some(**ino)).map(|(d, _)| d.clone()).collect();
        for d in gone {
            // O sistema já soltou a pasta apagada; o erro do `unwatch` é esse.
            let _ = self.watcher.unwatch(&d);
            self.armed.remove(&d);
        }
        for d in &dirs {
            let Some(ino) = dir_inode(d).filter(|_| !self.armed.contains_key(d)) else { continue };
            match self.watcher.watch(d, RecursiveMode::NonRecursive) {
                Ok(()) => { self.armed.insert(d.clone(), ino); }
                // Sumiu entre o stat e o watch: o aviso da pasta-mãe rearma.
                Err(e) if matches!(e.kind, notify::ErrorKind::PathNotFound) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

fn watch_unavailable(error: &notify::Error) {
    tracing::warn!(code = "rust.tags_watch_unavailable", %error, "vigia das etiquetas indisponível; vale a assinatura por tique");
}

/// Fonte do evento para o produtor da lista; ligada na subida do servidor (sem ela, a lista não o publica).
#[derive(Default)]
pub struct Feed {
    source: OnceLock<(AccountService, String)>,
    /// Chave e JSON da última montagem: geração das invalidações e, sem vigia, a assinatura.
    last: Arc<Mutex<Option<(Key, Arc<str>)>>>,
    generation: Arc<AtomicU64>,
    /// Gravação aceita ou aviso do vigia: o produtor publica na hora, sem esperar o tique.
    pub(crate) changed: Arc<Notify>,
    /// `None`: sem vigia, cada leitura confere a assinatura.
    watch: Arc<Mutex<Option<Watch>>>,
}

type Key = (u64, Option<Vec<(PathBuf, Stamp)>>);

impl Feed {
    pub fn bind(&self, accounts: AccountService, machine_id: String) {
        if self.source.set((accounts, machine_id)).is_err() { return; }
        let Some((accounts, _)) = self.source.get() else { return };
        match Watch::start(&accounts.env, self.generation.clone(), self.changed.clone()) {
            Ok(w) => *lock(&self.watch) = Some(w),
            Err(error) => watch_unavailable(&error),
        }
    }

    /// Gravação aceita: remonta e publica na hora. Duas gravações no mesmo tique de mtime e de mesmo
    /// tamanho deixariam a assinatura igual.
    pub(crate) fn invalidate(&self) { wake(&self.generation, &self.changed); }

    /// O JSON do evento, lido fora do runtime; remontado só quando a chave muda. Sem mudança volta
    /// o mesmo `Arc`: quem compara não copia o texto.
    pub(crate) async fn read(&self) -> Option<Result<Arc<str>, &'static str>> {
        let (accounts, machine_id) = self.source.get()?.clone();
        let (last, generation, watch) = (self.last.clone(), self.generation.clone(), self.watch.clone());
        let read = tokio::task::spawn_blocking(move || {
            // Rearme antes da montagem: o que nascer depois dele acorda de novo.
            let watching = {
                let mut held = lock(&watch);
                let failed = match held.as_mut() {
                    Some(w) if w.rescan.swap(false, Ordering::AcqRel) => w.arm(&accounts.env).err(),
                    _ => None,
                };
                if let Some(error) = failed {
                    watch_unavailable(&error);
                    *held = None;
                }
                held.is_some()
            };
            let key = (generation.load(Ordering::Acquire), (!watching).then(|| signature(&accounts.env)));
            let mut last = lock(&last);
            if let Some((_, json)) = last.as_ref().filter(|(seen, _)| *seen == key) { return Ok(json.clone()); }
            let json: Arc<str> = event(&accounts, &machine_id)?.to_string().into();
            *last = Some((key, json.clone()));
            Ok(json)
        });
        Some(read.await.unwrap_or(Err("tags_task_failed")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::environment::AccountEnvironment;
    use crate::list::{discover::Problems, discover_other::Dirs, procs::{ChildrenMap, ProcessView}};
    use std::{ffi::OsString, path::{Path, PathBuf}, time::Duration};

    fn service(home: &Path) -> AccountService {
        AccountService::new(AccountEnvironment::from_map([("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned())].into()))
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    // Real e sem o `\\?\` do canonicalize do Windows, como os ids saem.
    fn real_home(tmp: &tempfile::TempDir) -> PathBuf { crate::list::links::resolve_lenient(tmp.path()) }

    fn account(home: &Path, name: &str) -> PathBuf {
        let dir = home.join(name);
        write(&dir.join(crate::accounts::storage::CLAUDE_MARKER), "");
        crate::list::links::resolve_lenient(&dir)
    }

    /// Pane com `CLAUDE_CONFIG_DIR` (ou sem ele): é o que decide a `conta` da linha.
    struct Pane(Option<PathBuf>);

    impl ProcessView for Pane {
        fn children(&self, _: Duration) -> std::io::Result<Arc<ChildrenMap>> { Ok(Arc::default()) }
        fn argv(&self, _: i64) -> Vec<String> { vec!["claude".into()] }
        fn cwd(&self, _: i64) -> Option<PathBuf> { None }
        fn env_var(&self, _: i64, name: &str) -> std::io::Result<Option<OsString>> {
            Ok(self.0.clone().filter(|_| name == "CLAUDE_CONFIG_DIR").map(OsString::from))
        }
        fn start_time(&self, _: i64) -> Option<f64> { None }
    }

    /// A `conta` que a lista põe na linha de um pane Claude.
    fn row_account(home: &Path, config_dir: Option<PathBuf>) -> String {
        let dirs = Dirs { home: home.to_path_buf(), claude: home.join(".claude"), codex_home: home.join(".codex"),
            pi_sessions: home.join(".pi"), omp_config: home.join(".omp"), omp_agent: home.join(".omp/agent"),
            kimi_home: home.join(".kimi-code") };
        let mut row = crate::list::links::blank_row("s");
        crate::list::links::fill_pane_row(&mut row, "claude", Some(1), None, &Pane(config_dir), &dirs, &mut Problems::default());
        row.conta.unwrap()
    }

    fn keys(event: &Value) -> Vec<String> { event["accounts"].as_object().unwrap().keys().cloned().collect() }

    #[test]
    fn names_are_cut_at_the_at_sign_and_untagged_accounts_get_the_card() {
        let tmp = tempfile::tempdir().unwrap();
        let home = real_home(&tmp);
        let mail = account(&home, ".claude-mail");
        account(&home, ".claude-200-2");
        write(&home.join(".claude/.hangar-apelidos.json"), &json!({format!("claude:{}", mail.display()): "ana@example.com"}).to_string());
        write(&home.join(".claude/engines.json"), &json!({"kimi": {"label": "claude-200-2", "base_url": "https://x"}}).to_string());
        write(&home.join(".hangar/tags.json"), &json!({"machine": {"name": "Casa", "icon": "house", "color": "green"},
            "accounts": {"chave:kimi": {"icon": "key-round", "color": "amber"}}}).to_string());
        let tagged = event(&service(&home), "casa").unwrap();
        assert_eq!(tagged["machine_id"], "casa");
        assert_eq!(tagged["machine"], json!({"name": "Casa", "icon": "house", "color": "green"}));
        let accounts = &tagged["accounts"];
        assert_eq!(accounts[format!("claude:{}", mail.display())], json!({"name": "ana", "icon": "credit-card", "color": null}));
        assert_eq!(accounts["chave:kimi"], json!({"name": "claude-200-2", "icon": "key-round", "color": "amber"}));
        assert_eq!(event(&service(&home), "").unwrap()["machine_id"], Value::Null, "id desconhecido é null");
    }

    #[tokio::test]
    async fn without_the_watcher_the_signature_still_sees_a_new_account() {
        let tmp = tempfile::tempdir().unwrap();
        let home = real_home(&tmp);
        let feed = Feed::default();
        feed.bind(service(&home), String::new());
        *lock(&feed.watch) = None;
        let first = feed.read().await.unwrap().unwrap();
        // Assinatura igual: o evento guardado volta sem releitura.
        let key = (feed.generation.load(Ordering::Acquire), Some(signature(&service(&home).env)));
        *lock(&feed.last) = Some((key, "guardado".into()));
        assert_eq!(&*feed.read().await.unwrap().unwrap(), "guardado");
        let added = account(&home, ".claude-nova");
        let after = feed.read().await.unwrap().unwrap();
        assert_ne!(after, first);
        assert!(after.contains(&format!("claude:{}", added.display())), "conta nova fora do evento: {after}");
    }

    #[tokio::test]
    async fn a_new_account_folder_wakes_the_feed_without_a_tick() {
        let tmp = tempfile::tempdir().unwrap();
        let home = real_home(&tmp);
        let feed = Feed::default();
        feed.bind(service(&home), String::new());
        assert!(lock(&feed.watch).is_some(), "vigia não subiu");
        let first = feed.read().await.unwrap().unwrap();
        let added = account(&home, ".claude-x");
        tokio::time::timeout(Duration::from_secs(10), feed.changed.notified()).await.expect("o vigia não acordou o evento");
        let after = feed.read().await.unwrap().unwrap();
        assert_ne!(after, first);
        assert!(after.contains(&format!("claude:{}", added.display())), "conta nova fora do evento: {after}");
        // A pasta nova foi armada no rearme: mexer no marcador dela acorda de novo (com conteúdo:
        // só abrir e fechar é acesso, e acesso não acorda).
        std::fs::write(home.join(".claude-x").join(CLAUDE_MARKER), "x").unwrap();
        tokio::time::timeout(Duration::from_secs(10), feed.changed.notified()).await.expect("pasta nova sem vigia");
    }

    /// A linha e o evento usam o mesmo id: o cliente casa as duas pelo texto.
    #[cfg(unix)]
    #[test]
    fn the_row_and_the_event_name_the_same_claude_account() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().canonicalize().unwrap().join("real");
        std::fs::create_dir_all(&real).unwrap();
        // HOME por link e `~/.claude` ainda inexistente: a conta padrão sai do mesmo caminho nos dois lados.
        let home = tmp.path().join("home");
        std::os::unix::fs::symlink(&real, &home).unwrap();
        let target = account(&home, ".claude-work");
        let link = home.join(".claude-link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let keys = keys(&event(&service(&home), "").unwrap());
        let default = row_account(&home, None);
        assert!(keys.contains(&default), "conta padrão {default} fora do evento {keys:?}");
        let linked = row_account(&home, Some(link));
        assert!(keys.contains(&linked), "conta por link {linked} fora do evento {keys:?}");
        // A etiqueta gravada pelo id da tela de Contas aparece na chave da linha.
        let screen_id = format!("claude:{}", home.join(".claude").display());
        write(&real.join(".hangar/tags.json"), &json!({"accounts": {screen_id: {"icon": "wallet", "color": "blue"}}}).to_string());
        assert_eq!(event(&service(&home), "").unwrap()["accounts"][&default]["icon"], "wallet");
    }
}
