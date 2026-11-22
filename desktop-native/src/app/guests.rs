//! Configurações › Convidados. A conta fica numa máquina do dono, que deriva as chaves e cifra o cofre, para o convidado
//! entrar pelo Connect sem o serviço ver a senha. O acesso em cada máquina nasce antes, com o token que este app já tem
//! dela: sem acesso, a conta não teria o que guardar.
use super::*;
use super::sync::{decrypt_json, derive_keys, secure_setup, sync_field};
use super::create::{Root, Scan, choice, crumbs, scan_of};
use super::device::Remote;
use super::server_config::chip;
use super::servers::{ServerEntry, norm};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui_kit::component::switch::Switch;
use hangar_connect_client::{normalize_name as normalize_user, valid_guest_user};
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Máquina deste app que pode receber o convidado. `connect_url`: o host do Connect dela, que vai ao cofre do convidado
/// no lugar do endereço salvo.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Machine { id: String, label: String, address: String, token: String, connect_url: Option<String>, lan_url: Option<String> }

impl Machine {
    fn from_entry(entry: &ServerEntry) -> Self {
        let address = entry.address.trim().trim_end_matches('/').to_owned();
        // Salva pelo próprio host do Connect: não há outro endereço para o cofre.
        let connect_url = if connect_host(&address) { None } else {
            entry.connect.as_ref().map(|c| c.url.trim().trim_end_matches('/').to_owned()).filter(|url| !url.is_empty())
        };
        let label = if entry.label.is_empty() { super::servers::default_label(&entry.address) } else { entry.label.clone() };
        Self { id: entry.id.clone(), label, address, token: entry.token.clone(), connect_url,
            lan_url: entry.lan.as_ref().map(|lan| lan.url.clone()).filter(|url| !url.is_empty()) }
    }

    /// O endereço que o cofre do convidado guarda: o host do Connect quando a máquina tem, senão o salvo.
    fn base_url(&self) -> &str { self.connect_url.as_deref().unwrap_or(&self.address) }

    fn api(&self) -> Result<Api, Failure> { Api::new(&self.address, &self.token) }
}

fn host_of(url: &str) -> Option<String> { url::Url::parse(url.trim()).ok()?.host_str().map(str::to_ascii_lowercase) }

fn connect_host(address: &str) -> bool { host_of(address).is_some_and(|host| host.ends_with(".hangar.dev.br")) }

/// A máquina de um acesso gravado: pelo id deste app ou, no acesso criado por outro app, pelo endereço que a conta guardou.
fn resolve<'a>(machines: &'a [Machine], id: &str, base_url: Option<&str>) -> Option<&'a Machine> {
    machines.iter().find(|m| m.id == id).or_else(|| {
        let wanted = norm(base_url?);
        machines.iter().find(|m| norm(&m.address) == wanted || m.connect_url.as_deref().is_some_and(|url| norm(url) == wanted))
    })
}

/// Um acesso da conta. `token` só existe no acesso recém-criado ou trazido da conta antiga: a leitura nunca o devolve, e
/// sem ele a máquina que guarda a conta mantém o que já tinha.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct GuestServer {
    server_id: String,
    #[serde(default)] label: String,
    #[serde(default)] base_url: Option<String>,
    guest_id: String,
    root: String,
    #[serde(skip)] token: Option<String>,
}

/// Linha de `GET /api/guest-accounts`. `name_taken`: o serviço do Connect disse que o nome já é de outra conta.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct GuestAccount {
    user: String,
    sees_owner: bool,
    owner_sees: bool,
    #[serde(default)] name_taken: bool,
    #[serde(default)] servers: Vec<GuestServer>,
}

/// `password: None` na edição mantém a senha gravada.
struct Draft { user: String, password: Option<String>, sees_owner: bool, owner_sees: bool, servers: Vec<(String, String)> }

/// O `admin_blob` da conta antiga, cifrado com a chave da sincronização do dono: guarda a senha do convidado.
#[derive(Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LegacyAdmin { user: String, password: String, sees_owner: bool, owner_sees: bool, servers: Vec<LegacyAccess> }

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LegacyAccess { server_id: String, guest_id: String, token: String, root: String }

// À mão pra a senha nunca ir parar num log ou numa mensagem de teste.
impl std::fmt::Debug for LegacyAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyAdmin").field("user", &self.user).field("sees_owner", &self.sees_owner)
            .field("owner_sees", &self.owner_sees).field("servers", &self.servers.len()).finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct UrlOnly { #[serde(default)] url: String }

/// Servidor do cofre de sincronização do dono, como o web guarda. `base_url` vazio é o próprio hub.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OwnerServer {
    id: String,
    #[serde(default)] label: String,
    #[serde(default)] base_url: String,
    #[serde(default)] invite: bool,
    #[serde(default)] connect: Option<UrlOnly>,
}

trait GuestOps {
    async fn create(&self, machine: &Machine, draft: &Draft, root: &str) -> Result<(String, String), Failure>;
    async fn update(&self, machine: &Machine, id: &str, draft: &Draft, root: &str) -> Result<(), Failure>;
    async fn delete(&self, machine: &Machine, id: &str) -> Result<(), Failure>;
    async fn store(&self, body: Value) -> Result<(), Failure>;
    async fn drop_account(&self, user: &str) -> Result<(), Failure>;
}

fn describe(error: &Failure) -> String { Hangar::setting_failure(error) }

fn server_text(label: &str, error: &str) -> String { tr_shared("convidados_falha_servidor", &[("servidor", label), ("erro", error)]) }

fn away_text(label: &str) -> String { server_text(label, &tr_shared("convidados_servidor_fora_da_lista", &[])) }

fn store_text(label: &str, error: &Failure) -> String {
    match error.status {
        Some(409) => tr_shared("convidados_usuario_em_uso", &[]),
        // Nome ou senha recusados: o motivo do servidor diz qual.
        Some(400) => describe(error),
        Some(401 | 403) => tr_shared("erro_so_dono", &[]),
        _ => tr_shared("guests_store_failed", &[("servidor", label)]),
    }
}

/// Como o servidor (`valid_user` em hangar-guest-accounts): o `.` ou `@` dos clientes e, no arquivo, só ASCII minúsculo de 3 a 64.
fn valid_user(user: &str) -> bool {
    (3..=64).contains(&user.len())
        && user.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._@+-".contains(&b))
        && valid_guest_user(user)
}

/// Nome que serve no Connect para a conta antiga `old`: o mesmo, se já serve; senão o antigo com `@<dono>`.
fn suggest_user(old: &str, owner: &str) -> String {
    let user = normalize_user(old);
    if valid_user(&user) { return user; }
    let clean = |text: &str| normalize_user(text).chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._+-".contains(*c)).collect::<String>();
    let mut tail = clean(owner);
    if tail.is_empty() { tail = "hangar".into(); }
    tail.truncate(32);
    let mut head = clean(&user);
    if head.is_empty() { head = "convidado".into(); }
    head.truncate(63 - tail.len());
    format!("{head}@{tail}")
}

fn validate(draft: &Draft, editing: bool, guests: &[GuestAccount]) -> Option<&'static str> {
    let name = draft.user.as_str();
    if name.is_empty() { return Some("convidados_usuario_obrigatorio"); }
    // Recusa antes de tocar nas máquinas: depois deixaria acesso criado sem conta.
    if !valid_user(name) { return Some("guests_user_invalid"); }
    if !editing && guests.iter().any(|g| g.user == name) { return Some("convidados_usuario_em_uso"); }
    match &draft.password {
        None if !editing => return Some("guests_password_required"),
        Some(password) if password.chars().count() < 8 => return Some("sync_password_min"),
        _ => {}
    }
    if draft.servers.is_empty() { return Some("convidados_servidor_obrigatorio"); }
    if draft.servers.iter().any(|(_, root)| root.is_empty()) { return Some("convidados_pasta_obrigatoria"); }
    None
}

/// `name_refused`: a máquina que guarda a conta recusou o nome (409); a conta nova não existe.
pub(super) struct SaveOutcome { saved: GuestAccount, errors: Vec<String>, store_failed: bool, name_refused: bool }

fn gone(error: &Failure) -> bool { error.status == Some(404) }

fn shown(old: Option<&GuestServer>, id: &str) -> String { old.map(|o| o.label.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| id.to_owned()) }

/// O que uma máquina deixa na conta depois de mexer nela, e a falha dela.
type Step = (Option<GuestServer>, Option<String>);

/// Uma máquina marcada: cria, altera ou, se sumiu de lá, recria o acesso. Na falha, o acesso antigo fica na conta.
async fn mark_one(ops: &impl GuestOps, machines: &[Machine], draft: &Draft, id: &str, root: &str, old: Option<&GuestServer>) -> Step {
    // Sem a máquina na lista não dá pra agir nela.
    let Some(machine) = resolve(machines, id, old.and_then(|o| o.base_url.as_deref())) else { return (old.cloned(), Some(away_text(&shown(old, id)))) };
    let entry = |guest_id: String, token: Option<String>| GuestServer {
        server_id: id.to_owned(), label: machine.label.clone(), base_url: Some(machine.base_url().to_owned()), guest_id, root: root.to_owned(), token,
    };
    let result = match old {
        None => ops.create(machine, draft, root).await.map(|(guest, token)| entry(guest, Some(token))),
        Some(old) => match ops.update(machine, &old.guest_id, draft, root).await {
            Ok(()) => Ok(entry(old.guest_id.clone(), old.token.clone())),
            // Recria em vez de ficar com um id morto.
            Err(error) if gone(&error) => ops.create(machine, draft, root).await.map(|(guest, token)| entry(guest, Some(token))),
            Err(error) => Err(error),
        },
    };
    match result {
        Ok(done) => (Some(done), None),
        Err(error) => (old.cloned(), Some(server_text(&machine.label, &describe(&error)))),
    }
}

/// Uma máquina desmarcada: tira o acesso. Na falha ele fica na conta, para tirar de novo.
async fn unmark_one(ops: &impl GuestOps, machines: &[Machine], old: &GuestServer) -> Step {
    let Some(machine) = resolve(machines, &old.server_id, old.base_url.as_deref()) else {
        return (Some(old.clone()), Some(away_text(&shown(Some(old), &old.server_id))));
    };
    match ops.delete(machine, &old.guest_id).await {
        Err(error) if !gone(&error) => (Some(old.clone()), Some(server_text(&machine.label, &describe(&error)))),
        _ => (None, None),
    }
}

/// Cria, altera e tira o acesso em todas as máquinas ao mesmo tempo e só depois grava a conta. Falha numa máquina volta
/// com o nome dela, e a entrada antiga fica na conta para tentar de novo.
async fn save_guest(ops: &impl GuestOps, machines: &[Machine], store_label: &str, draft: &Draft, prev: Option<&GuestAccount>) -> SaveOutcome {
    let before = |id: &str| prev.and_then(|p| p.servers.iter().find(|s| s.server_id == id));
    let marked = draft.servers.iter().map(|(id, root)| mark_one(ops, machines, draft, id, root, before(id)));
    let unmarked = prev.map_or(&[][..], |p| p.servers.as_slice()).iter()
        .filter(|old| !draft.servers.iter().any(|(id, _)| *id == old.server_id))
        .map(|old| unmark_one(ops, machines, old));
    let (marked, unmarked) = futures::future::join(futures::future::join_all(marked), futures::future::join_all(unmarked)).await;
    let (mut kept, mut errors) = (Vec::new(), Vec::new());
    for (entry, error) in marked.into_iter().chain(unmarked) {
        kept.extend(entry);
        errors.extend(error);
    }
    let saved = GuestAccount { user: draft.user.clone(), sees_owner: draft.sees_owner, owner_sees: draft.owner_sees, name_taken: false, servers: kept };
    let servers: Vec<Value> = saved.servers.iter().map(|s| {
        let mut row = json!({"server_id": s.server_id, "label": s.label, "guest_id": s.guest_id, "root": s.root});
        if let Some(url) = &s.base_url { row["base_url"] = json!(url); }
        if let Some(token) = &s.token { row["token"] = json!(token); }
        row
    }).collect();
    let mut body = json!({"user": saved.user, "sees_owner": saved.sees_owner, "owner_sees": saved.owner_sees, "servers": servers});
    if let Some(password) = &draft.password { body["password"] = json!(password); }
    if let Err(error) = ops.store(body).await {
        errors.push(store_text(store_label, &error));
        // Os acessos já mudaram: `saved` volta pro formulário e salvar de novo repete com ele, sem duplicar.
        return SaveOutcome { saved, errors, store_failed: true, name_refused: error.status == Some(409) };
    }
    SaveOutcome { saved, errors, store_failed: false, name_refused: false }
}

/// Desfaz os acessos de uma conta que a máquina não gravou (nome recusado). Volta uma linha por máquina que ficou com o
/// acesso, para o dono tirar à mão; o que já tinha saído (404) conta como tirado.
async fn drop_accesses(ops: &impl GuestOps, machines: &[Machine], guest: &GuestAccount) -> Vec<String> {
    let left = &|server: &str, error: &str| tr_shared("guests_access_left", &[("servidor", server), ("usuario", &guest.user), ("erro", error)]);
    let drops = guest.servers.iter().map(|access| async move {
        let Some(machine) = resolve(machines, &access.server_id, access.base_url.as_deref()) else {
            return Some(left(access.server_id.as_str(), tr_shared("convidados_servidor_fora_da_lista", &[]).as_str()));
        };
        match ops.delete(machine, &access.guest_id).await {
            Err(error) if !gone(&error) => Some(left(machine.label.as_str(), describe(&error).as_str())),
            _ => None,
        }
    });
    futures::future::join_all(drops).await.into_iter().flatten().collect()
}

/// Tira o acesso de todas as máquinas e só apaga a conta quando todas largaram e a gravação foi aceita.
async fn remove_guest(ops: &impl GuestOps, machines: &[Machine], store_label: &str, guest: &GuestAccount) -> Vec<String> {
    let draft = Draft { user: guest.user.clone(), password: None, sees_owner: guest.sees_owner, owner_sees: guest.owner_sees, servers: Vec::new() };
    let out = save_guest(ops, machines, store_label, &draft, Some(guest)).await;
    let mut errors = out.errors;
    if out.saved.servers.is_empty() && !out.store_failed && let Err(error) = ops.drop_account(&guest.user).await {
        errors.push(store_text(store_label, &error));
    }
    errors
}

/// Conta nova com a mesma senha e os mesmos acessos da antiga: `prev` aponta os acessos que já existem em cada máquina,
/// então nada é criado de novo. A máquina do cofre do dono é achada na lista deste app pelo id ou pelo endereço; `hub` é
/// o host da máquina que guarda o cofre antigo (o endereço vazio no cofre é ela). `Err`: as máquinas que faltam.
fn legacy_plan(admin: &LegacyAdmin, owners: &[OwnerServer], mine: &[Machine], hub: Option<&str>, user: &str) -> Result<(Draft, GuestAccount), Vec<String>> {
    let (mut missing, mut found) = (Vec::new(), Vec::new());
    for access in &admin.servers {
        let owner = owners.iter().find(|s| s.id == access.server_id);
        let hosts: Vec<String> = owner.into_iter().flat_map(|o| {
            let saved = if o.base_url.is_empty() { hub.map(str::to_owned) } else { host_of(&o.base_url) };
            [saved, o.connect.as_ref().and_then(|c| host_of(&c.url))]
        }).flatten().collect();
        let local = mine.iter().find(|m| m.id == access.server_id).or_else(|| mine.iter().find(|m| {
            [Some(m.address.as_str()), m.connect_url.as_deref(), m.lan_url.as_deref()].into_iter().flatten().filter_map(host_of).any(|h| hosts.contains(&h))
        }));
        match local {
            Some(machine) => found.push((access, machine)),
            None => missing.push(owner.map(|o| o.label.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| access.server_id.clone())),
        }
    }
    if !missing.is_empty() { return Err(missing); }
    let name = normalize_user(user);
    let draft = Draft { user: name.clone(), password: Some(admin.password.clone()), sees_owner: admin.sees_owner, owner_sees: admin.owner_sees,
        servers: found.iter().map(|(access, machine)| (machine.id.clone(), access.root.clone())).collect() };
    let prev = GuestAccount { user: name, sees_owner: admin.sees_owner, owner_sees: admin.owner_sees, name_taken: false,
        servers: found.iter().map(|(access, machine)| GuestServer { server_id: machine.id.clone(), label: machine.label.clone(),
            base_url: Some(machine.base_url().to_owned()), guest_id: access.guest_id.clone(), root: access.root.clone(),
            token: Some(access.token.clone()) }).collect() };
    Ok((draft, prev))
}

/// `path` dentro de `root` (ou a própria raiz).
fn inside(root: &str, path: &str) -> bool {
    path.strip_prefix(root.trim_end_matches('/')).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Onde o seletor abre: na pasta já escolhida, pela raiz mais funda que a contém; sem ela, no topo da primeira raiz.
fn start_at(roots: &[Root], chosen: &str) -> Option<(Root, String)> {
    let holder = roots.iter().filter(|r| !chosen.is_empty() && inside(&r.path, chosen)).max_by_key(|r| r.path.len());
    match holder {
        Some(root) => Some((root.clone(), chosen.to_owned())),
        None => roots.first().map(|root| (root.clone(), root.path.clone())),
    }
}

fn begin<T>(remote: &mut Remote<T>) -> u64 {
    (remote.seq, remote.loading) = (ticket(), true);
    remote.seq
}

/// `apis`: uma por máquina, montada uma vez, pra o criar e o recriar da mesma máquina usarem as mesmas conexões.
struct Live { store: Api, apis: HashMap<String, Result<Api, Failure>> }

impl Live {
    fn new(store: Api, machines: &[Machine]) -> Self {
        Self { store, apis: machines.iter().map(|m| (m.id.clone(), m.api())).collect() }
    }

    fn api(&self, machine: &Machine) -> Result<Api, Failure> {
        self.apis.get(&machine.id).cloned().unwrap_or_else(|| machine.api())
    }
}

impl GuestOps for Live {
    async fn create(&self, machine: &Machine, draft: &Draft, root: &str) -> Result<(String, String), Failure> {
        let body = json!({"name": draft.user, "root": root, "sees_owner": draft.sees_owner, "owner_sees": draft.owner_sees});
        let value = self.api(machine)?.server_send(reqwest::Method::POST, &["guests"], Some(body), 30).await?;
        let field = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
        field("id").zip(field("token")).ok_or_else(|| Failure::local("invalid_response"))
    }

    async fn update(&self, machine: &Machine, id: &str, draft: &Draft, root: &str) -> Result<(), Failure> {
        let body = json!({"root": root, "sees_owner": draft.sees_owner, "owner_sees": draft.owner_sees});
        self.api(machine)?.server_send(reqwest::Method::POST, &["guests", id], Some(body), 30).await.map(|_| ())
    }

    async fn delete(&self, machine: &Machine, id: &str) -> Result<(), Failure> {
        self.api(machine)?.server_post(&["guests", id, "delete"], 30).await.map(|_| ())
    }

    // A máquina deriva as chaves (600 mil voltas) antes de responder.
    async fn store(&self, body: Value) -> Result<(), Failure> {
        self.store.server_send(reqwest::Method::POST, &["guest-accounts"], Some(body), 90).await.map(|_| ())
    }

    async fn drop_account(&self, user: &str) -> Result<(), Failure> {
        match self.store.server_post(&["guest-accounts", user, "delete"], 30).await {
            Err(error) if error.status != Some(404) => Err(error),
            _ => Ok(()),
        }
    }
}

async fn load_accounts(store: &Api) -> Result<Vec<GuestAccount>, Failure> {
    let rows = store.server_read(&["guest-accounts"], &[], 15).await?;
    serde_json::from_value(rows).map_err(|_| Failure::local("invalid_response"))
}

/// Login de sincronização do dono numa máquina, com a lista das contas antigas: só vive enquanto a página está aberta.
#[derive(Clone)]
pub(super) struct LegacySession { hub: Api, cookie: String, owners: Vec<OwnerServer>, guests: Vec<LegacyAdmin> }

/// Entra no hub como o web: prelogin → chaves → login (cookie) → cofre do dono → contas antigas.
async fn open_legacy(hub: &Api, user: String, password: String) -> Result<LegacySession, String> {
    let (pre, _) = hub.hub(reqwest::Method::GET, &["sync", "prelogin"], &[("user", user.as_str())], None, None, 15).await.map_err(|e| describe(&e))?;
    let salt = pre.get("salt").and_then(Value::as_str).and_then(|s| STANDARD.decode(s).ok());
    let iterations = pre.get("iterations").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
    let (Some(salt), Some(iterations)) = (salt, iterations) else { return Err(tr("invalid_response")) };
    // PBKDF2 com 600 mil voltas: fora das threads do runtime.
    let keys = tokio::task::spawn_blocking(move || derive_keys(&password, &salt, iterations)).await.ok().flatten()
        .ok_or_else(|| tr("invalid_response"))?;
    let login = hub.hub(reqwest::Method::POST, &["sync", "login"], &[], Some(json!({"user": user, "auth_hash": keys.auth_hash})), None, 15).await;
    let cookie = match login {
        Ok((_, Some(cookie))) => cookie,
        Ok((_, None)) => return Err(tr("invalid_response")),
        Err(error) if error.status == Some(401) => return Err(tr_shared("sync_credenciais_invalidas", &[])),
        Err(error) if error.status == Some(429) => return Err(tr_shared("sync_muitas_tentativas", &[])),
        Err(error) => return Err(describe(&error)),
    };
    let read = async {
        let (vault, _) = hub.hub(reqwest::Method::GET, &["sync", "vault"], &[], None, Some(&cookie), 15).await.map_err(|e| describe(&e))?;
        let owners: Vec<OwnerServer> = match vault.get("enc_blob") {
            None | Some(Value::Null) => Vec::new(),
            Some(blob) => decrypt_json(&keys.enc_key, blob).ok_or_else(|| tr("invalid_response"))?,
        };
        let (rows, _) = hub.hub(reqwest::Method::GET, &["sync", "guests"], &[], None, Some(&cookie), 15).await.map_err(|e| describe(&e))?;
        let guests = rows.as_array().ok_or_else(|| tr("invalid_response"))?.iter()
            .map(|row| row.get("admin_blob").and_then(|blob| decrypt_json(&keys.enc_key, blob)).ok_or_else(|| tr("invalid_response")))
            .collect::<Result<Vec<LegacyAdmin>, String>>()?;
        Ok::<_, String>((owners, guests))
    }.await;
    match read {
        // Convite fica só no aparelho que resgatou: não é servidor do dono.
        Ok((owners, guests)) => Ok(LegacySession { hub: hub.clone(), cookie, owners: owners.into_iter().filter(|s| !s.invite).collect(), guests }),
        Err(error) => {
            close_legacy(hub, &cookie).await;
            Err(error)
        }
    }
}

/// A sessão do hub dá escrita no cofre: sai assim que não serve mais.
async fn close_legacy(hub: &Api, cookie: &str) {
    if let Err(error) = hub.hub(reqwest::Method::POST, &["sync", "logout"], &[], None, Some(cookie), 10).await {
        let code = error.code.clone().or_else(|| error.status.map(|status| status.to_string())).unwrap_or_else(|| "failed".into());
        crate::log_line(&format!("convidados: saída do hub não confirmada codigo={code}"));
    }
}

struct LoginForm { user: Entity<InputState>, password: Entity<InputState>, _subscriptions: Vec<Subscription> }

struct GuestForm {
    editing: Option<GuestAccount>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    sees_owner: bool,
    owner_sees: bool,
    /// Máquina marcada = tem pasta aqui (vazia até escolher), pelo id deste app; a ordem é a da marcação.
    roots: Vec<(String, String)>,
    /// O seletor de pasta aberto; um por vez.
    browser: Option<FolderBrowser>,
    error: Option<String>,
}

/// Navega pelas pastas de uma máquina marcada: raízes → subpastas → escolher, ou criar uma nova na pasta atual.
struct FolderBrowser {
    server: String,
    roots: Remote<Vec<Root>>,
    root: Option<Root>,
    dir: String,
    scan: Remote<Scan>,
    /// Campo do nome da pasta nova; a assinatura cria no Enter.
    naming: Option<(Entity<InputState>, Subscription)>,
    /// Número da criação em voo.
    making: Option<u64>,
    make_error: Option<String>,
}

impl FolderBrowser {
    fn new(server: String) -> Self {
        Self { server, roots: Remote::default(), root: None, dir: String::new(), scan: Remote::default(), naming: None, making: None,
            make_error: None }
    }
}

/// Máquina com a Sincronização registrada: é nela que o cofre antigo pode ter contas de convidado.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LegacyHub { server: String, label: String, user: String }

/// A tela de uma máquina com contas antigas: entrada da sincronização, a lista delas ou o nome novo de uma.
struct LegacyView {
    hub: LegacyHub,
    login: Option<LoginForm>,
    session: Option<LegacySession>,
    renaming: Option<(LegacyAdmin, Entity<InputState>)>,
    errors: Vec<String>,
}

#[derive(Default)]
pub(in crate::app) struct Guests {
    list: Option<Result<Vec<GuestAccount>, String>>,
    /// Pedido em voo; a resposta só vale se for deste número.
    waiting: Option<u64>,
    form: Option<GuestForm>,
    failures: Vec<String>,
    /// Convidado salvo, removido ou trazido.
    notice: Option<String>,
    legacy_seq: u64,
    legacy_hubs: Vec<LegacyHub>,
    legacy: Option<LegacyView>,
}

impl Guests {
    fn browser(&mut self) -> Option<&mut FolderBrowser> { self.form.as_mut()?.browser.as_mut() }

    /// Aplica o resultado de um Salvar: com falha o formulário fica aberto e salvar de novo repete só o que faltou.
    fn apply_saved(&mut self, SaveOutcome { saved, errors, store_failed, name_refused }: SaveOutcome) {
        self.failures = errors;
        // A conta não existe: os acessos já foram desfeitos e o formulário continua de conta nova.
        if name_refused { return; }
        if !store_failed && let Some(Ok(list)) = self.list.as_mut() {
            list.retain(|g| g.user != saved.user);
            list.push(saved.clone());
        }
        if let Some(form) = self.form.as_mut() { form.editing = Some(saved); }
        if self.failures.is_empty() {
            self.form = None;
            self.notice = Some(tr_shared("convidados_salvo", &[]));
        }
    }
}

pub(super) enum GuestsReply {
    Loaded(u64, Result<Vec<GuestAccount>, Failure>),
    Legacy(u64, Vec<LegacyHub>),
    Saved(u64, SaveOutcome),
    Removed(u64, Vec<String>),
    LegacyOpened(u64, Result<LegacySession, String>),
    /// `Ok((antigo, novo))`; `Err((falhas, conta nova gravada))`.
    Brought(u64, Result<(String, String), (Vec<String>, bool)>),
    FolderRoots(u64, Result<Vec<Root>, String>),
    FolderScan(u64, Result<Scan, String>),
    /// Caminho da pasta criada.
    FolderMade(u64, Result<String, String>),
}

// Número global: a página zera o estado ao sair e o seletor de pasta nasce de novo a cada abertura, e uma resposta
// antiga não pode casar com o pedido novo.
fn ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Hangar {
    fn guests_send_later(&self) -> impl Fn(GuestsReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move {
                let payload = Payload::Sync(super::sync::SyncReply::Guests(reply));
                let _ = tx.send(Envelope { connection, selection: None, payload }).await;
            })
        }
    }

    /// As máquinas desta pessoa que podem receber o convidado: ligadas, sem convite e sem par externo.
    fn guest_machines(&self) -> Vec<Machine> {
        self.servers.iter().filter(|s| !s.disabled && !s.invite && !s.ephemeral).map(Machine::from_entry).collect()
    }

    fn guest_machine_api(&self, id: &str) -> Result<Api, String> {
        let machines = self.guest_machines();
        let machine = machines.iter().find(|m| m.id == id).ok_or_else(|| tr_shared("convidados_servidor_fora_da_lista", &[]))?;
        machine.api().map_err(|error| Hangar::failure(&error))
    }

    /// Página aberta: a lista da máquina escolhida e as máquinas com contas antigas.
    pub(super) fn guests_opened(&mut self, cx: &mut Context<Self>) {
        self.guests_page_left();
        self.guests_reload(cx);
        self.guests_find_legacy(cx);
    }

    /// A sessão da sincronização do dono (cookie e chave) sai junto com a página.
    pub(super) fn guests_page_left(&mut self) {
        if let Some(session) = self.guests.legacy.as_mut().and_then(|view| view.session.take()) {
            self.runtime.spawn(async move { close_legacy(&session.hub, &session.cookie).await });
        }
        self.guests = Guests::default();
    }

    fn guests_reload(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        let seq = ticket();
        (self.guests.waiting, self.guests.list) = (Some(seq), None);
        let done = self.guests_send_later();
        self.runtime.spawn(async move { done(GuestsReply::Loaded(seq, load_accounts(&store).await)).await });
        cx.notify();
    }

    fn guests_find_legacy(&mut self, cx: &mut Context<Self>) {
        let machines = self.guest_machines();
        self.guests.legacy_seq = ticket();
        let (seq, done) = (self.guests.legacy_seq, self.guests_send_later());
        self.runtime.spawn(async move {
            let reads = machines.into_iter().map(|machine| async move {
                let setup = machine.api().ok()?.server_read(&["sync", "setup"], &[], 10).await.ok()?;
                let flag = |key: &str| setup.get(key).and_then(Value::as_bool) == Some(true);
                let user = setup.get("user").and_then(Value::as_str).filter(|u| !u.is_empty())?.to_owned();
                (flag("enabled") && flag("registered")).then(|| LegacyHub { server: machine.id.clone(), label: machine.label.clone(), user })
            });
            let hubs = futures::future::join_all(reads).await.into_iter().flatten().collect();
            done(GuestsReply::Legacy(seq, hubs)).await
        });
        cx.notify();
    }

    fn guests_open(&mut self, guest: Option<GuestAccount>, window: &mut Window, cx: &mut Context<Self>) {
        let user = cx.new(|cx| InputState::new(window, cx).default_value(guest.as_ref().map(|g| g.user.clone()).unwrap_or_default()));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let machines = self.guest_machines();
        // Acesso de máquina que não está neste app fica fora das marcações; o Salvar o mantém na conta.
        let roots = guest.iter().flat_map(|g| &g.servers)
            .filter_map(|s| resolve(&machines, &s.server_id, s.base_url.as_deref()).map(|m| (m.id.clone(), s.root.clone()))).collect();
        let guests = &mut self.guests;
        guests.form = Some(GuestForm { sees_owner: guest.as_ref().is_some_and(|g| g.sees_owner),
            owner_sees: guest.as_ref().is_none_or(|g| g.owner_sees), editing: guest, user, password, roots, browser: None, error: None });
        (guests.failures, guests.notice, guests.legacy) = (Vec::new(), None, None);
        cx.notify();
    }

    fn guests_toggle(&mut self, id: String, on: bool, cx: &mut Context<Self>) {
        let Some(form) = self.guests.form.as_mut() else { return };
        if !on {
            form.roots.retain(|(server, _)| *server != id);
            if form.browser.as_ref().is_some_and(|b| b.server == id) { form.browser = None; }
        } else if !form.roots.iter().any(|(server, _)| *server == id) {
            form.roots.push((id.clone(), String::new()));
            // Marcar já abre as pastas dela: sem pasta a máquina não salva.
            self.guests_browse(id, cx);
        }
        cx.notify();
    }

    fn guests_browse(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(form) = self.guests.form.as_mut() else { return };
        form.browser = Some(FolderBrowser::new(id));
        self.guests_load_roots(cx);
    }

    /// A API da máquina do seletor de pasta aberto, o começo de todo pedido dele. `None`: o seletor fechou.
    fn guests_browser_api(&self) -> Option<Result<Api, String>> {
        let server = self.guests.form.as_ref()?.browser.as_ref()?.server.clone();
        Some(self.guest_machine_api(&server))
    }

    fn guests_load_roots(&mut self, cx: &mut Context<Self>) {
        let (done, Some(api)) = (self.guests_send_later(), self.guests_browser_api()) else { return };
        let Some(browser) = self.guests.browser() else { return };
        let seq = begin(&mut browser.roots);
        match api {
            Err(error) => { browser.roots.finish(seq, Err(error)); }
            Ok(api) => { self.runtime.spawn(async move {
                let roots = api.server_read(&["fs", "roots"], &[], 15).await.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value::<Vec<Root>>(v).map_err(|_| tr("invalid_response")));
                done(GuestsReply::FolderRoots(seq, roots)).await
            }); }
        }
        cx.notify();
    }

    fn guests_pick_root(&mut self, root: Root, cx: &mut Context<Self>) {
        let Some(browser) = self.guests.browser() else { return };
        let path = root.path.clone();
        browser.root = Some(root);
        self.guests_scan(path, cx);
    }

    fn guests_scan(&mut self, path: String, cx: &mut Context<Self>) {
        let (done, Some(api)) = (self.guests_send_later(), self.guests_browser_api()) else { return };
        let Some(browser) = self.guests.browser() else { return };
        let Some(root) = browser.root.as_ref().map(|r| r.path.clone()) else { return };
        (browser.dir, browser.make_error) = (path.clone(), None);
        let seq = begin(&mut browser.scan);
        match api {
            Err(error) => { browser.scan.finish(seq, Err(error)); }
            Ok(api) => { self.runtime.spawn(async move {
                let query: Vec<(&str, &str)> = if path == root { vec![("root", root.as_str())] } else { vec![("root", root.as_str()), ("path", path.as_str())] };
                done(GuestsReply::FolderScan(seq, scan_of(api.server_read(&["fs", "scan"], &query, 15).await))).await
            }); }
        }
        cx.notify();
    }

    fn guests_pick_folder(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(form) = self.guests.form.as_mut() else { return };
        let Some(browser) = form.browser.take() else { return };
        if let Some((_, root)) = form.roots.iter_mut().find(|(id, _)| *id == browser.server) { *root = path; }
        cx.notify();
    }

    fn guests_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe_in(&input, window, |this: &mut Hangar, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) { this.guests_make_folder(cx); }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        let Some(browser) = self.guests.browser() else { return };
        (browser.naming, browser.make_error) = (Some((input, subscription)), None);
        cx.notify();
    }

    fn guests_make_folder(&mut self, cx: &mut Context<Self>) {
        let (done, Some(api)) = (self.guests_send_later(), self.guests_browser_api()) else { return };
        let Some(browser) = self.guests.browser() else { return };
        let (Some((input, _)), Some(root), None) = (browser.naming.as_ref(), browser.root.as_ref(), browser.making) else { return };
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() { return; }
        let root = root.path.clone();
        let path = (browser.dir != root).then(|| browser.dir.clone());
        let failed = |error: &str| tr_shared("arquivo_criar_pasta_erro", &[("erro", error)]);
        match api {
            Err(error) => browser.make_error = Some(failed(&error)),
            Ok(api) => {
                let seq = ticket();
                (browser.making, browser.make_error) = (Some(seq), None);
                self.runtime.spawn(async move {
                    let body = json!({"root": root, "path": path, "name": name});
                    let made = api.server_send(reqwest::Method::POST, &["fs", "mkdir"], Some(body), 15).await
                        .map_err(|e| Hangar::fetch_failure(&e))
                        .and_then(|v| v.get("path").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| tr("invalid_response")));
                    done(GuestsReply::FolderMade(seq, made)).await
                });
            }
        }
        cx.notify();
    }

    /// Respostas do seletor de pasta: não passam pelo `waiting`, que é do Salvar e da lista.
    fn receive_folder(&mut self, reply: GuestsReply, cx: &mut Context<Self>) {
        let Some(form) = self.guests.form.as_mut() else { return };
        let Some(browser) = form.browser.as_mut() else { return };
        match reply {
            GuestsReply::FolderRoots(seq, result) => {
                if !browser.roots.finish(seq, result) { return; }
                let chosen = form.roots.iter().find(|(id, _)| *id == browser.server).map(|(_, p)| p.as_str()).unwrap_or_default();
                if let Some((root, dir)) = start_at(browser.roots.ok().map_or(&[][..], Vec::as_slice), chosen) {
                    browser.root = Some(root);
                    self.guests_scan(dir, cx);
                }
            }
            GuestsReply::FolderScan(seq, result) => { browser.scan.finish(seq, result); }
            GuestsReply::FolderMade(seq, result) => {
                if browser.making != Some(seq) { return; }
                browser.making = None;
                match result {
                    // Entra na pasta nova: "Usar esta pasta" já a escolhe.
                    Ok(path) => { browser.naming = None; self.guests_scan(path, cx); }
                    Err(error) => browser.make_error = Some(tr_shared("arquivo_criar_pasta_erro", &[("erro", &error)])),
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// O rascunho do formulário: cada máquina marcada leva o id que o acesso já tem na conta (que pode ser de outro app).
    fn guests_draft(&self, form: &GuestForm, cx: &App) -> Draft {
        let machines = self.guest_machines();
        let prev = form.editing.as_ref().map_or(&[][..], |g| g.servers.as_slice());
        let mut servers: Vec<(String, String)> = form.roots.iter().map(|(machine, root)| {
            let recorded = prev.iter().find(|s| resolve(&machines, &s.server_id, s.base_url.as_deref()).is_some_and(|m| m.id == *machine));
            (recorded.map_or_else(|| machine.clone(), |s| s.server_id.clone()), root.clone())
        }).collect();
        // Acesso numa máquina que este app não tem: segue na conta como está, e o Salvar diz que ela saiu da lista.
        servers.extend(prev.iter().filter(|s| resolve(&machines, &s.server_id, s.base_url.as_deref()).is_none()).map(|s| (s.server_id.clone(), s.root.clone())));
        let password = form.password.read(cx).value().to_string();
        Draft { user: normalize_user(&form.user.read(cx).value()), password: (!password.is_empty()).then_some(password),
            sees_owner: form.sees_owner, owner_sees: form.owner_sees, servers }
    }

    fn guests_submit(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.api.clone() else { return };
        if self.guests.waiting.is_some() { return; }
        let Some(form) = self.guests.form.as_ref() else { return };
        let draft = self.guests_draft(form, cx);
        let list = self.guests.list.as_ref().and_then(|l| l.as_ref().ok()).map_or(&[][..], Vec::as_slice);
        let error = validate(&draft, form.editing.is_some(), list).map(|key| tr_shared(key, &[]));
        let prev = form.editing.clone();
        let (machines, label) = (self.guest_machines(), self.server_label(cx));
        let guests = &mut self.guests;
        let Some(form) = guests.form.as_mut() else { return };
        form.error = error;
        if form.error.is_some() { cx.notify(); return; }
        let seq = ticket();
        (guests.waiting, guests.notice) = (Some(seq), None);
        let done = self.guests_send_later();
        self.runtime.spawn(async move {
            let live = Live::new(store, &machines);
            let mut outcome = save_guest(&live, &machines, &label, &draft, prev.as_ref()).await;
            if outcome.name_refused && prev.is_none() {
                // Nome recusado na conta nova: os acessos que acabaram de nascer saem, e o que não sai fica dito.
                let left = drop_accesses(&live, &machines, &outcome.saved).await;
                outcome.errors.extend(left);
                outcome.saved.servers.clear();
            }
            done(GuestsReply::Saved(seq, outcome)).await
        });
        cx.notify();
    }

    fn guests_confirm_remove(&mut self, guest: GuestAccount, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let text = tr_shared("convidados_remover_aviso", &[("usuario", &guest.user)]);
        chrome::confirm_alert(window, cx, tr_shared("convidados_remover", &[]), text, tr_shared("convidados_remover", &[]),
            ButtonVariant::Danger, move |_, cx| {
                let guest = guest.clone();
                let _ = this.update(cx, |this, cx| this.guests_remove(guest, cx));
                true
            });
    }

    fn guests_remove(&mut self, guest: GuestAccount, cx: &mut Context<Self>) {
        let Some(store) = self.api.clone() else { return };
        if self.guests.waiting.is_some() { return; }
        let (machines, label) = (self.guest_machines(), self.server_label(cx));
        let seq = ticket();
        (self.guests.waiting, self.guests.notice) = (Some(seq), None);
        let done = self.guests_send_later();
        self.runtime.spawn(async move {
            let errors = remove_guest(&Live::new(store, &machines), &machines, &label, &guest).await;
            done(GuestsReply::Removed(seq, errors)).await
        });
        cx.notify();
    }

    fn guests_open_legacy(&mut self, hub: LegacyHub, window: &mut Window, cx: &mut Context<Self>) {
        let user = cx.new(|cx| InputState::new(window, cx).default_value(hub.user.clone()));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let subscriptions = [&user, &password].map(|input| cx.subscribe_in(input, window,
            |this: &mut Hangar, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) { this.guests_legacy_login(window, cx); }
            }));
        password.update(cx, |state, cx| state.focus(window, cx));
        let guests = &mut self.guests;
        guests.legacy = Some(LegacyView { hub, login: Some(LoginForm { user, password, _subscriptions: subscriptions.into() }), session: None,
            renaming: None, errors: Vec::new() });
        (guests.form, guests.notice) = (None, None);
        cx.notify();
    }

    fn guests_close_legacy(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.guests.legacy.take().and_then(|view| view.session) {
            self.runtime.spawn(async move { close_legacy(&session.hub, &session.cookie).await });
        }
        cx.notify();
    }

    fn guests_legacy_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.guests.waiting.is_some() { return; }
        let Some(view) = self.guests.legacy.as_ref() else { return };
        let Some(form) = view.login.as_ref() else { return };
        let (user, password) = (form.user.read(cx).value().trim().to_string(), form.password.read(cx).value().to_string());
        let machines = self.guest_machines();
        let hub = machines.iter().find(|m| m.id == view.hub.server).cloned();
        let error = match &hub {
            None => Some(tr_shared("convidados_servidor_fora_da_lista", &[])),
            // A senha derivada vale como senha do dono: nunca sai em http.
            Some(machine) if !secure_setup(&machine.address) => Some(tr("sync_config_https")),
            _ if user.is_empty() => Some(tr_shared("convidados_usuario_obrigatorio", &[])),
            _ if password.is_empty() => Some(tr_shared("guests_password_required", &[])),
            _ => None,
        };
        let api = hub.as_ref().map(Machine::api);
        let Some(view) = self.guests.legacy.as_mut() else { return };
        if let Some(error) = error { view.errors = vec![error]; cx.notify(); return; }
        let api = match api { Some(Ok(api)) => api, Some(Err(error)) => { view.errors = vec![Hangar::failure(&error)]; cx.notify(); return; } None => return };
        view.errors.clear();
        // A senha sai do campo assim que o pedido parte.
        if let Some(form) = view.login.as_ref() { form.password.update(cx, |state, cx| state.set_value("", window, cx)); }
        let seq = ticket();
        self.guests.waiting = Some(seq);
        let done = self.guests_send_later();
        self.runtime.spawn(async move { done(GuestsReply::LegacyOpened(seq, open_legacy(&api, user, password).await)).await });
        cx.notify();
    }

    /// Nome sem `.` nem `@` não serve no Connect: a pessoa escolhe o novo antes de qualquer acesso mudar.
    fn guests_pick_legacy(&mut self, admin: LegacyAdmin, window: &mut Window, cx: &mut Context<Self>) {
        if valid_user(&normalize_user(&admin.user)) {
            let name = admin.user.clone();
            return self.guests_bring(admin, name, cx);
        }
        let owner = self.guests.legacy.as_ref().map(|v| v.hub.user.clone()).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(suggest_user(&admin.user, &owner)));
        input.update(cx, |state, cx| state.focus(window, cx));
        if let Some(view) = self.guests.legacy.as_mut() { (view.renaming, view.errors) = (Some((admin, input)), Vec::new()); }
        cx.notify();
    }

    fn guests_bring(&mut self, admin: LegacyAdmin, name: String, cx: &mut Context<Self>) {
        let Some(store) = self.api.clone() else { return };
        if self.guests.waiting.is_some() { return; }
        let (machines, label) = (self.guest_machines(), self.server_label(cx));
        let Some(view) = self.guests.legacy.as_mut() else { return };
        let Some(session) = view.session.clone() else { return };
        let hub_host = machines.iter().find(|m| m.id == view.hub.server).and_then(|m| host_of(&m.address));
        if !valid_user(&normalize_user(&name)) { view.errors = vec![tr_shared("guests_user_invalid", &[])]; cx.notify(); return; }
        let (draft, prev) = match legacy_plan(&admin, &session.owners, &machines, hub_host.as_deref(), &name) {
            Ok(plan) => plan,
            Err(missing) => {
                view.errors = vec![tr_shared("guests_legacy_missing", &[("servidores", &missing.join(", "))])];
                cx.notify();
                return;
            }
        };
        view.errors.clear();
        let seq = ticket();
        (self.guests.waiting, self.guests.notice) = (Some(seq), None);
        let done = self.guests_send_later();
        self.runtime.spawn(async move {
            let out = save_guest(&Live::new(store, &machines), &machines, &label, &draft, Some(&prev)).await;
            let result = if !out.errors.is_empty() { Err((out.errors, !out.store_failed)) } else {
                // A antiga só sai depois que a nova foi guardada inteira; já tinha saído (404) conta como apagada.
                match session.hub.hub(reqwest::Method::POST, &["sync", "guests", admin.user.as_str(), "delete"], &[], None, Some(&session.cookie), 30).await {
                    Err(error) if error.status != Some(404) => Err((vec![describe(&error)], true)),
                    _ => Ok((admin.user.clone(), draft.user.clone())),
                }
            };
            done(GuestsReply::Brought(seq, result)).await
        });
        cx.notify();
    }

    /// Lista lida: 404 é servidor sem as contas pelo Connect, e 401/403 é token que não é o do dono.
    fn guests_loaded(&mut self, result: Result<Vec<GuestAccount>, Failure>) {
        self.guests.list = Some(result.map_err(|error| match error.status {
            Some(404) => tr_shared("guests_update_server", &[]),
            Some(401 | 403) => tr_shared("erro_so_dono", &[]),
            _ => tr_shared("convidados_erro", &[("erro", &describe(&error))]),
        }));
    }

    pub(super) fn receive_guests(&mut self, reply: GuestsReply, _window: &mut Window, cx: &mut Context<Self>) {
        let seq = match &reply {
            GuestsReply::Legacy(seq, hubs) => {
                if *seq == self.guests.legacy_seq { self.guests.legacy_hubs = hubs.clone(); cx.notify(); }
                return;
            }
            GuestsReply::Loaded(seq, _) | GuestsReply::Saved(seq, _) | GuestsReply::Removed(seq, _) | GuestsReply::LegacyOpened(seq, _)
                | GuestsReply::Brought(seq, _) => *seq,
            GuestsReply::FolderRoots(..) | GuestsReply::FolderScan(..) | GuestsReply::FolderMade(..) => return self.receive_folder(reply, cx),
        };
        if self.guests.waiting != Some(seq) { return; }
        self.guests.waiting = None;
        match reply {
            GuestsReply::Loaded(_, list) => self.guests_loaded(list),
            GuestsReply::Saved(_, outcome) => self.guests.apply_saved(outcome),
            GuestsReply::Removed(_, errors) => {
                if errors.is_empty() { self.guests.notice = Some(tr_shared("guests_removed", &[])); }
                self.guests.failures = errors;
                self.guests_reload(cx);
            }
            GuestsReply::LegacyOpened(_, result) => if let Some(view) = self.guests.legacy.as_mut() {
                match result {
                    Ok(session) => (view.session, view.login, view.errors) = (Some(session), None, Vec::new()),
                    Err(error) => view.errors = vec![error],
                }
            },
            GuestsReply::Brought(_, Ok((old, new))) => {
                if let Some(view) = self.guests.legacy.as_mut() {
                    view.renaming = None;
                    if let Some(session) = view.session.as_mut() { session.guests.retain(|g| g.user != old); }
                }
                self.guests.notice = Some(tr_shared("guests_legacy_brought", &[("usuario", &new)]));
                self.guests_reload(cx);
            }
            GuestsReply::Brought(_, Err((errors, stored))) => {
                if let Some(view) = self.guests.legacy.as_mut() { view.errors = errors; }
                // A conta nova pode ter sido guardada e só a antiga não ter saído: tentar de novo atualiza os mesmos acessos.
                if stored { self.guests_reload(cx); }
            }
            GuestsReply::Legacy(..) | GuestsReply::FolderRoots(..) | GuestsReply::FolderScan(..) | GuestsReply::FolderMade(..) => {}
        }
        cx.notify();
    }

    /// A pasta de uma máquina marcada: a escolhida (ou o aviso de que falta) e o seletor, quando aberto.
    fn render_guest_folder(&self, machine: &Machine, path: &str, browser: Option<&FolderBrowser>, busy: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let id = machine.id.clone();
        let label = tr_shared("convidados_pasta", &[("servidor", &machine.label)]);
        let summary = div().flex().items_center().gap_2()
            .child(div().flex_1().min_w_0().text_sm().truncate().map(|el| if path.is_empty() {
                el.text_color(theme::muted()).child(tr_shared("convidados_pasta_nenhuma", &[]))
            } else { el.font_family(theme::MONO).child(path.to_owned()) }))
            .when(browser.is_none(), |el| el.child(Button::new(SharedString::from(format!("guests-browse-{}", machine.id))).outline().small()
                .icon(IconName::FolderOpen).disabled(busy)
                .label(tr_shared(if path.is_empty() { "convidados_escolher_pasta" } else { "convidados_trocar_pasta" }, &[]))
                .on_click(cx.listener(move |this, _, _, cx| this.guests_browse(id.clone(), cx)))));
        div().id(SharedString::from(format!("guests-folder-{}", machine.id))).role(Role::Group).aria_label(label.clone())
            .ml(px(24.)).flex().flex_col().gap(px(6.))
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
            .child(summary)
            .when_some(browser, |el, browser| el.child(Self::render_folder_browser(browser, path, busy, cx)))
    }

    fn render_folder_browser(browser: &FolderBrowser, chosen: &str, busy: bool, cx: &mut Context<Self>) -> Div {
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        let loading = |id: &'static str| div().id(id).role(Role::Status).text_sm().text_color(theme::muted()).child(tr_shared("comum_carregando", &[]));
        let alert = |id: &'static str, text: String| div().id(id).role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(text);
        let retry = |id: &'static str| Button::new(id).outline().small().label(tr_shared("lista_tentar_novamente", &[])).disabled(busy);
        let frame = div().flex().flex_col().gap_2().p_2().rounded(px(8.)).border_1().border_color(theme::border());
        // Fechar no meio do mkdir descartaria a resposta: a pasta nasceria sem ninguém saber.
        let close = Button::new("guests-browse-close").ghost().small().label(tr_shared("comum_cancelar", &[])).disabled(busy || browser.making.is_some())
            .on_click(cx.listener(|this, _, _, cx| { if let Some(form) = this.guests.form.as_mut() { form.browser = None; } cx.notify(); }));
        let roots = match browser.roots.value.as_ref() {
            _ if browser.roots.loading => return frame.child(loading("guests-roots-loading")).child(div().child(close)),
            None => return frame.child(div().child(close)),
            Some(Err(error)) => return frame.child(alert("guests-roots-error", format!("{} {error}", tr_shared("arquivo_carregar_raizes_erro", &[]))))
                .child(div().flex().gap_2().child(retry("guests-roots-retry").on_click(cx.listener(|this, _, _, cx| this.guests_load_roots(cx)))).child(close)),
            Some(Ok(list)) if list.is_empty() => return frame.child(muted(tr_shared("arquivo_sem_raizes", &[]))).child(div().child(close)),
            Some(Ok(list)) => list,
        };
        let Some(root) = browser.root.as_ref() else { return frame.child(div().child(close)) };
        let chips = div().id("guests-roots").role(Role::Group).aria_label(tr_shared("arquivo_raizes_aria", &[])).flex().flex_wrap().gap(px(6.))
            .children(roots.iter().map(|r| {
                let pick = r.clone();
                choice(SharedString::from(format!("guests-root-{}", r.path)), r.path == root.path, cx).small().rounded_full()
                    .label(r.name.clone()).tooltip(r.path.clone()).disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.guests_pick_root(pick.clone(), cx)))
            }));
        let trail = div().id("guests-crumbs").aria_label(tr_shared("arquivo_caminho_aria", &[])).flex().flex_wrap().items_center().gap(px(2.))
            .children(crumbs(root, &browser.dir).into_iter().enumerate().map(|(n, (text, path))| div().flex().items_center().gap(px(2.))
                .when(n > 0, |el| el.child(div().text_color(theme::faint()).text_size(px(12.)).child("/")))
                .child(Button::new(SharedString::from(format!("guests-crumb-{path}"))).ghost().xsmall().label(text).disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.guests_scan(path.clone(), cx))))));
        let rows = match browser.scan.value.as_ref() {
            _ if browser.scan.loading => loading("guests-scan-loading").into_any_element(),
            None => div().into_any_element(),
            Some(Err(error)) => {
                let dir = browser.dir.clone();
                div().flex().flex_col().gap_2().child(alert("guests-scan-error", error.clone()))
                    .child(div().child(retry("guests-scan-retry").on_click(cx.listener(move |this, _, _, cx| this.guests_scan(dir.clone(), cx)))))
                    .into_any_element()
            }
            Some(Ok(scan)) if scan.error.is_some() => muted(scan.error.clone().unwrap_or_default()).into_any_element(),
            Some(Ok(scan)) if scan.entries.is_empty() => muted(tr_shared("arquivo_sem_subpastas", &[])).into_any_element(),
            // ponytail: lista simples com rolagem; pasta com milhares de subpastas pede a lista virtual do `create.rs`.
            Some(Ok(scan)) => div().id("guests-folders").max_h(px(240.)).overflow_y_scroll().flex().flex_col().gap(px(2.))
                .children(scan.entries.iter().map(|entry| {
                    let (pick, open) = (entry.path.clone(), entry.path.clone());
                    let on = entry.path == chosen;
                    div().flex().items_center().gap(px(4.))
                        .child(choice(SharedString::from(format!("guests-pick-{}", entry.path)), on, cx).small().flex_1().min_w_0().justify_start()
                            .icon(IconName::Folder).label(entry.name.clone()).tooltip(entry.path.clone()).selected(on).disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.guests_pick_folder(pick.clone(), cx))))
                        .child(Button::new(SharedString::from(format!("guests-open-{}", entry.path))).ghost().small().icon(IconName::ChevronRight)
                            .accessibility_label(tr_shared("arquivo_abrir", &[("nome", &entry.name)])).disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.guests_scan(open.clone(), cx))))
                })).into_any_element(),
        };
        let making = browser.making.is_some();
        let naming = browser.naming.as_ref().map(|(input, _)| {
            let ready = !input.read(cx).value().trim().is_empty();
            div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().child(Input::new(input).small().disabled(making || busy).aria_label(tr_shared("arquivo_nova_pasta_nome", &[]))))
                .child(Button::new("guests-mkdir").primary().small().label(tr_shared("arquivo_criar_pasta", &[])).loading(making)
                    .disabled(!ready || making || busy).on_click(cx.listener(|this, _, _, cx| this.guests_make_folder(cx))))
                .child(Button::new("guests-mkdir-cancel").ghost().small().label(tr_shared("comum_cancelar", &[])).disabled(making)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(b) = this.guests.browser() { (b.naming, b.make_error) = (None, None); }
                        cx.notify();
                    })))
        });
        let dir = browser.dir.clone();
        // Durante a leitura `dir` já é a pasta nova e `scan` ainda é a anterior.
        let readable = !browser.scan.loading && browser.scan.ok().is_some_and(|scan| scan.error.is_none());
        frame.child(chips).child(trail).child(rows)
            .children(naming)
            .when_some(browser.make_error.clone(), |el, error| el.child(alert("guests-mkdir-error", error)))
            .child(div().flex().flex_wrap().items_center().gap_2()
                .child(Button::new("guests-use-folder").outline().small().icon(IconName::FolderOpen).label(tr_shared("arquivo_usar_pasta", &[]))
                    .disabled(busy || !readable).on_click(cx.listener(move |this, _, _, cx| this.guests_pick_folder(dir.clone(), cx))))
                .when(browser.naming.is_none(), |el| el.child(Button::new("guests-new-folder").ghost().small().icon(IconName::Plus)
                    .label(tr_shared("arquivo_nova_pasta", &[])).disabled(busy || !readable)
                    .on_click(cx.listener(|this, _, window, cx| this.guests_new_folder(window, cx)))))
                .child(div().flex_1())
                .child(close))
    }

    /// Linha da lista: nome em cima, resumo embaixo e a seta; a linha inteira abre.
    fn guest_row(id: SharedString, title: String, summary: String, danger: bool, busy: bool, on_click: impl Fn(&mut Hangar, &mut Window, &mut Context<Hangar>) + 'static,
        cx: &mut Context<Self>) -> Stateful<Div> {
        div().id(id).role(Role::Button).aria_label(title.clone()).flex().items_center().gap_3().px_3().py_2().rounded(px(8.)).border_1()
            .border_color(theme::border())
            .when(!busy, |el| el.cursor_pointer().hover(|el| el.bg(theme::hover()))
                .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx))))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                .child(div().text_sm().truncate().text_color(if danger { theme::danger() } else { theme::muted() }).child(summary)))
            .child(Icon::new(IconName::ChevronRight).size(px(14.)).text_color(theme::faint()))
    }

    fn render_guest_form(&self, form: &GuestForm, busy: bool, cx: &mut Context<Self>) -> Div {
        let alert = |id: &'static str, text: String| div().id(id).role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(text);
        let machines = self.guest_machines();
        let mut servers = div().flex().flex_col().gap_2().p_3().rounded(px(8.)).border_1().border_color(theme::border())
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(tr_shared("convidados_servidores", &[])));
        for machine in &machines {
            let root = form.roots.iter().find(|(id, _)| *id == machine.id).map(|(_, path)| path.as_str());
            let id = machine.id.clone();
            let browser = form.browser.as_ref().filter(|b| b.server == machine.id);
            servers = servers.child(Checkbox::new(SharedString::from(format!("guests-server-{}", machine.id))).small()
                .label(machine.label.clone()).checked(root.is_some()).disabled(busy)
                .on_click(cx.listener(move |this, on: &bool, _, cx| this.guests_toggle(id.clone(), *on, cx))))
                .when_some(root, |el, path| el.child(self.render_guest_folder(machine, path, browser, busy, cx)));
        }
        let toggle = |id: &'static str, label: String, on: bool, set: fn(&mut GuestForm, bool), cx: &mut Context<Self>| {
            div().flex().items_center().justify_between().gap_3().py_2().border_b_1().border_color(theme::border())
                .child(div().text_sm().child(label.clone()))
                .child(Switch::new(id).checked(on).disabled(busy).accessibility_label(label)
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        if let Some(form) = this.guests.form.as_mut() { set(form, *on); }
                        cx.notify();
                    })))
        };
        let editing = form.editing.clone();
        let heading = tr_shared(if editing.is_some() { "guests_edit_title" } else { "convidados_adicionar" }, &[]);
        div().flex().flex_col().gap_3()
            .child(div().child(Button::new("guests-back").ghost().small().icon(IconName::ChevronLeft).label(tr_shared("convidados_titulo", &[]))
                .disabled(busy).on_click(cx.listener(|this, _, _, cx| { this.guests.form = None; this.guests.failures.clear(); cx.notify(); }))))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(heading))
            // O nome é a chave da conta e do login dela: editar não o troca.
            .child(sync_field(tr("login_usuario"), &form.user, busy || editing.is_some()))
            .child(sync_field(tr_shared(if editing.is_some() { "guests_password_keep" } else { "login_senha" }, &[]), &form.password, busy))
            .child(servers)
            .child(div().flex().flex_col()
                .child(toggle("guests-sees-owner", tr_shared("convidados_ve_minhas", &[]), form.sees_owner, |f, on| f.sees_owner = on, cx))
                .child(toggle("guests-owner-sees", tr_shared("convidados_eu_vejo", &[]), form.owner_sees, |f, on| f.owner_sees = on, cx)))
            .when_some(form.error.clone(), |el, error| el.child(alert("guests-form-error", error)))
            .child(div().flex().items_center().gap_2()
                .child(Button::new("guests-save").primary().disabled(busy).loading(busy)
                    .label(tr_shared(if busy { "convidados_salvando" } else { "convidados_salvar" }, &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.guests_submit(cx))))
                .child(div().flex_1())
                .when_some(editing, |el, guest| el.child(Button::new("guests-remove").ghost().text_color(theme::danger()).label(tr_shared("convidados_remover", &[]))
                    .disabled(busy).on_click(cx.listener(move |this, _, window, cx| this.guests_confirm_remove(guest.clone(), window, cx))))))
    }

    fn render_legacy(&self, view: &LegacyView, busy: bool, cx: &mut Context<Self>) -> Div {
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        let mut page = div().flex().flex_col().gap_3()
            .child(div().child(Button::new("guests-legacy-back").ghost().small().icon(IconName::ChevronLeft).label(tr_shared("convidados_titulo", &[]))
                .disabled(busy).on_click(cx.listener(|this, _, _, cx| this.guests_close_legacy(cx)))))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(tr_shared("guests_legacy_title", &[])));
        let errors = view.errors.iter().enumerate().map(|(n, text)| div().id(SharedString::from(format!("guests-legacy-error-{n}"))).role(Role::Alert)
            .text_sm().text_color(theme::danger()).whitespace_normal().child(text.clone())).collect::<Vec<_>>();
        if let Some(form) = &view.login {
            return page.child(muted(tr_shared("guests_legacy_login_help", &[("servidor", &view.hub.label)])))
                .child(sync_field(tr("login_usuario"), &form.user, busy))
                .child(sync_field(tr("login_senha"), &form.password, busy))
                .children(errors)
                .child(div().child(Button::new("guests-legacy-login").primary().label(tr_shared(if busy { "login_entrando" } else { "login_entrar" }, &[]))
                    .disabled(busy).loading(busy).on_click(cx.listener(|this, _, window, cx| this.guests_legacy_login(window, cx)))));
        }
        let Some(session) = &view.session else { return page.children(errors) };
        if let Some((admin, input)) = &view.renaming {
            let typed = normalize_user(&input.read(cx).value());
            let help = tr_shared("guests_legacy_rename_help", &[("antigo", &admin.user), ("novo", &typed)]);
            let (admin, name_input) = (admin.clone(), input.clone());
            return page.child(muted(help))
                .child(sync_field(tr_shared("guests_legacy_new_name", &[]), input, busy))
                .children(errors)
                .child(div().flex().gap_2()
                    .child(Button::new("guests-legacy-bring-renamed").primary().label(tr_shared(if busy { "convidados_salvando" } else { "guests_legacy_bring" }, &[]))
                        .disabled(busy || typed.is_empty()).loading(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let name = name_input.read(cx).value().to_string();
                            this.guests_bring(admin.clone(), name, cx);
                        })))
                    .child(Button::new("guests-legacy-rename-cancel").outline().label(tr_shared("comum_cancelar", &[])).disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(view) = this.guests.legacy.as_mut() { (view.renaming, view.errors) = (None, Vec::new()); }
                            cx.notify();
                        }))));
        }
        if session.guests.is_empty() { page = page.child(muted(tr_shared("guests_legacy_empty", &[]))); }
        for admin in &session.guests {
            let pick = admin.clone();
            page = page.child(div().flex().items_center().gap_3().px_3().py_2().rounded(px(8.)).border_1().border_color(theme::border())
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(admin.user.clone()))
                    .child(div().text_sm().text_color(theme::muted()).child(tr_shared("guests_legacy_account", &[]))))
                .child(Button::new(SharedString::from(format!("guests-legacy-bring-{}", admin.user))).outline().small()
                    .label(tr_shared("guests_legacy_bring", &[])).disabled(busy).loading(busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.guests_pick_legacy(pick.clone(), window, cx)))));
        }
        page.children(errors)
    }

    pub(super) fn render_guests_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let title = div().flex().items_center().gap_2()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr_shared("convidados_titulo", &[])))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()));
        let mut page = div().flex().flex_col().gap_4().child(title);
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        if self.api.is_none() { return page.child(muted(tr("settings_offline"))).into_any_element(); }
        // Convite é máquina de outra pessoa: não guarda convidado deste dono.
        if self.active_invite() { return page.child(muted(tr_shared("guests_no_server", &[]))).into_any_element(); }
        let busy = self.guests.waiting.is_some();
        for (n, text) in self.guests.failures.iter().enumerate() {
            page = page.child(div().id(SharedString::from(format!("guests-failure-{n}"))).role(Role::Alert).text_sm().text_color(theme::danger())
                .whitespace_normal().child(text.clone()));
        }
        if let Some(view) = &self.guests.legacy { return page.child(self.render_legacy(view, busy, cx)).into_any_element(); }
        if let Some(form) = &self.guests.form { return page.child(self.render_guest_form(form, busy, cx)).into_any_element(); }
        page = page.child(muted(tr_shared("guests_connect_description", &[])));
        if let Some(notice) = self.guests.notice.clone() {
            page = page.child(div().id("guests-notice").role(Role::Status).text_sm().font_weight(FontWeight::SEMIBOLD).child(notice));
        }
        let list = match &self.guests.list {
            None => return page.child(div().id("guests-loading").role(Role::Status).text_sm().text_color(theme::muted()).child(tr_shared("comum_carregando", &[])))
                .into_any_element(),
            Some(Err(error)) => return page.child(div().id("guests-load-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal()
                    .child(error.clone()))
                .child(div().child(Button::new("guests-retry").outline().small().label(tr_shared("lista_tentar_novamente", &[])).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.guests_reload(cx)))))
                .into_any_element(),
            Some(Ok(list)) => list.clone(),
        };
        let mut rows = div().flex().flex_col().gap_2();
        if list.is_empty() { rows = rows.child(muted(tr_shared("convidados_vazio", &[]))); }
        for guest in list {
            let count = guest.servers.len().to_string();
            let summary = if guest.name_taken { tr_shared("guests_name_taken", &[]) } else {
                format!("{} · {}", tr_shared("convidados_resumo", &[("servidores", &count)]),
                    tr_shared(if guest.sees_owner { "guests_sees_mine" } else { "guests_not_sees_mine" }, &[]))
            };
            let (id, title, taken) = (SharedString::from(format!("guests-row-{}", guest.user)), guest.user.clone(), guest.name_taken);
            rows = rows.child(Self::guest_row(id, title, summary, taken, busy, move |this, window, cx| this.guests_open(Some(guest.clone()), window, cx), cx));
        }
        page = page.child(rows).child(div().child(Button::new("guests-add").primary().icon(IconName::Plus).label(tr_shared("convidados_adicionar", &[]))
            .disabled(busy).on_click(cx.listener(|this, _, window, cx| this.guests_open(None, window, cx)))));
        if !self.guests.legacy_hubs.is_empty() {
            let mut legacy = div().flex().flex_col().gap_2().pt_2()
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr_shared("guests_legacy_title", &[])));
            for hub in self.guests.legacy_hubs.clone() {
                let summary = tr_shared("guests_legacy_server_help", &[("usuario", &hub.user)]);
                let (id, title) = (SharedString::from(format!("guests-legacy-{}", hub.server)), hub.label.clone());
                legacy = legacy.child(Self::guest_row(id, title, summary, false, busy, move |this, window, cx| this.guests_open_legacy(hub.clone(), window, cx), cx));
            }
            page = page.child(legacy);
        }
        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` do gpui_kit, que o `super::*` traz, esconderia o `#[test]` da linguagem.
    use super::{Draft, Failure, GuestAccount, GuestOps, GuestServer, LegacyAccess, LegacyAdmin, Machine, OwnerServer, Root, STANDARD, decrypt_json,
        derive_keys, legacy_plan, remove_guest, resolve, save_guest, start_at, suggest_user, valid_user, validate};
    use super::super::sync::encrypt_json;
    use base64::Engine as _;
    use crate::i18n::tr_shared;
    use serde_json::{Value, json};
    use std::cell::RefCell;

    #[test]
    fn keys_match_the_web_derivation() {
        // Vetor calculado com o `deriveKeys` do web (WebCrypto no Node) pra mesma senha, sal e voltas.
        let keys = derive_keys("senha-teste", b"0123456789abcdef", 1000).unwrap();
        assert_eq!(keys.auth_hash, WEB_AUTH_HASH);
        let blob = encrypt_json(&keys.enc_key, &json!({"a": 1})).unwrap();
        assert_eq!(decrypt_json::<Value>(&keys.enc_key, &blob).unwrap(), json!({"a": 1}));
        assert_eq!(STANDARD.decode(blob["iv"].as_str().unwrap()).unwrap().len(), 12);
        // Cifrado pelo web com a chave `cp-enc` desta senha.
        assert_eq!(decrypt_json::<Value>(&keys.enc_key, &json!({"iv": WEB_IV, "data": WEB_DATA})).unwrap(), json!({"web": true}));
        assert!(derive_keys("x", b"s", 0).is_none());
    }

    const WEB_AUTH_HASH: &str = "fewlsDVCTvVcrbrLvNFlw10sAtj5G+YyqW3dt+XxYeg=";
    const WEB_IV: &str = "JQty6M7zR3xv37QM";
    const WEB_DATA: &str = "Uud7Id6F3q4oKV0LhFLyMIYDqTe3tsd8pZ5QPQ==";

    #[test]
    fn legacy_admin_reads_the_web_shape() {
        let web = json!({"user": "ana", "password": "12345678", "seesOwner": false, "ownerSees": true,
            "servers": [{"serverId": "srv-1", "guestId": "g1", "token": "t", "root": "/home/ana"}]});
        let admin: LegacyAdmin = serde_json::from_value(web).unwrap();
        assert_eq!(admin.servers, vec![LegacyAccess { server_id: "srv-1".into(), guest_id: "g1".into(), token: "t".into(), root: "/home/ana".into() }]);
        assert!(!format!("{admin:?}").contains("12345678"), "Debug mostrou a senha");
    }

    #[test]
    fn account_list_reads_the_server_shape_without_tokens() {
        let rows = json!([{"user": "ana.silva", "sees_owner": true, "owner_sees": false, "name_taken": true,
            "servers": [{"server_id": "a", "label": "A", "base_url": "https://a.jeff.hangar.dev.br", "guest_id": "g1", "root": "/srv"}]}]);
        let list: Vec<GuestAccount> = serde_json::from_value(rows).unwrap();
        assert!(list[0].name_taken && list[0].sees_owner && !list[0].owner_sees);
        assert_eq!(list[0].servers[0].token, None);
        assert_eq!(list[0].servers[0].base_url.as_deref(), Some("https://a.jeff.hangar.dev.br"));
    }

    fn draft(user: &str, password: Option<&str>, servers: &[(&str, &str)]) -> Draft {
        Draft { user: user.into(), password: password.map(Into::into), sees_owner: false, owner_sees: true,
            servers: servers.iter().map(|(a, b)| ((*a).into(), (*b).into())).collect() }
    }

    fn account(user: &str, servers: Vec<GuestServer>) -> GuestAccount {
        GuestAccount { user: user.into(), sees_owner: false, owner_sees: true, name_taken: false, servers }
    }

    #[test]
    fn guest_names_need_a_dot_or_an_at_like_the_server() {
        assert!(valid_user("ana.silva") && valid_user("ana@casa") && valid_user("lhais87.oliveira"));
        assert!(!valid_user("ana"), "sem ponto nem @ é nome de conta do Connect");
        assert!(!valid_user("Ana.Silva") && !valid_user("a.") && !valid_user("ana silva.x"));
        assert_eq!(suggest_user("lhais87.oliveira", "jeff"), "lhais87.oliveira");
        assert_eq!(suggest_user("Ana", "jeffersonfelizardo"), "ana@jeffersonfelizardo");
        assert_eq!(suggest_user("José!", ""), "jos@hangar");
    }

    #[test]
    fn validation_order_and_the_kept_password() {
        let existing = [account("bia.x", vec![])];
        let check = |d: Draft, editing| validate(&d, editing, &existing);
        assert_eq!(check(draft("", Some("12345678"), &[("s", "/")]), false), Some("convidados_usuario_obrigatorio"));
        assert_eq!(check(draft("ana", Some("12345678"), &[("s", "/")]), false), Some("guests_user_invalid"));
        assert_eq!(check(draft("bia.x", Some("12345678"), &[("s", "/")]), false), Some("convidados_usuario_em_uso"));
        assert_eq!(check(draft("ana.x", None, &[("s", "/")]), false), Some("guests_password_required"));
        // Na edição, senha vazia mantém a gravada.
        assert_eq!(check(draft("bia.x", None, &[("s", "/")]), true), None);
        assert_eq!(check(draft("ana.x", Some("1234567"), &[("s", "/")]), false), Some("sync_password_min"));
        assert_eq!(check(draft("ana.x", Some("12345678"), &[]), false), Some("convidados_servidor_obrigatorio"));
        assert_eq!(check(draft("ana.x", Some("12345678"), &[("s", "")]), false), Some("convidados_pasta_obrigatoria"));
    }

    /// Máquinas de mentira: `fail` diz que status cada operação devolve por máquina.
    #[derive(Default)]
    struct Fake { fail: Vec<(&'static str, &'static str, u16)>, store_fails: Option<u16>, calls: RefCell<Vec<String>>, stored: RefCell<Option<Value>> }

    fn refused(status: u16) -> Failure { Failure { status: Some(status), detail: "boom".into(), retry_after: None, uncertain: false, code: None } }

    impl Fake {
        fn run(&self, op: &str, server: &str) -> Result<(), Failure> {
            self.calls.borrow_mut().push(format!("{op} {server}"));
            match self.fail.iter().find(|(o, s, _)| *o == op && *s == server) {
                Some((_, _, status)) => Err(refused(*status)),
                None => Ok(()),
            }
        }
    }

    impl GuestOps for Fake {
        async fn create(&self, machine: &Machine, _: &Draft, _: &str) -> Result<(String, String), Failure> {
            self.run("create", &machine.id).map(|_| (format!("new-{}", machine.id), format!("tok-{}", machine.id)))
        }
        async fn update(&self, machine: &Machine, _: &str, _: &Draft, _: &str) -> Result<(), Failure> { self.run("update", &machine.id) }
        async fn delete(&self, machine: &Machine, _: &str) -> Result<(), Failure> { self.run("delete", &machine.id) }
        async fn store(&self, body: Value) -> Result<(), Failure> {
            *self.stored.borrow_mut() = Some(body);
            self.store_fails.map_or(Ok(()), |status| Err(refused(status)))
        }
        async fn drop_account(&self, user: &str) -> Result<(), Failure> { self.run("drop", user) }
    }

    fn machine(id: &str, address: &str, connect: Option<&str>) -> Machine {
        Machine { id: id.into(), label: id.to_uppercase(), address: address.into(), token: format!("owner-{id}"), connect_url: connect.map(Into::into), lan_url: None }
    }

    fn machines() -> Vec<Machine> {
        vec![machine("a", "http://100.64.0.2:8765", Some("https://a.jeff.hangar.dev.br")), machine("b", "https://b.ts.net", None)]
    }

    fn access(id: &str, root: &str) -> GuestServer {
        GuestServer { server_id: id.into(), label: id.to_uppercase(), base_url: None, guest_id: format!("old-{id}"), root: root.into(), token: None }
    }

    #[test]
    fn save_creates_updates_recreates_and_sends_the_connect_host() {
        let fake = Fake { fail: vec![("update", "a", 404)], ..Fake::default() };
        let old = account("ana.x", vec![access("a", "/old"), access("b", "/b")]);
        let out = futures::executor::block_on(save_guest(&fake, &machines(), "Casa", &draft("ana.x", None, &[("a", "/new"), ("b", "/b2")]), Some(&old)));
        assert!(out.errors.is_empty() && !out.store_failed);
        // `a` sumiu na máquina e foi recriado; o cofre leva o host do Connect dela, e `b` o endereço salvo.
        assert_eq!(fake.stored.borrow().clone().unwrap(), json!({"user": "ana.x", "sees_owner": false, "owner_sees": true, "servers": [
            {"server_id": "a", "label": "A", "base_url": "https://a.jeff.hangar.dev.br", "guest_id": "new-a", "root": "/new", "token": "tok-a"},
            {"server_id": "b", "label": "B", "base_url": "https://b.ts.net", "guest_id": "old-b", "root": "/b2"}]}));
        // Conta nova leva a senha; a edição sem senha não a manda.
        futures::executor::block_on(save_guest(&fake, &machines(), "Casa", &draft("bia.x", Some("12345678"), &[("b", "/b")]), None));
        assert_eq!(fake.stored.borrow().as_ref().unwrap()["password"], json!("12345678"));
    }

    #[test]
    fn access_made_by_another_app_is_found_by_its_address() {
        let ms = machines();
        assert_eq!(resolve(&ms, "expo-id", Some("https://a.jeff.hangar.dev.br/")).map(|m| m.id.as_str()), Some("a"));
        assert_eq!(resolve(&ms, "b", None).map(|m| m.id.as_str()), Some("b"));
        assert!(resolve(&ms, "x", Some("https://c.ts.net")).is_none());
        let fake = Fake::default();
        let old = account("ana.x", vec![GuestServer { base_url: Some("https://a.jeff.hangar.dev.br".into()), ..access("expo-id", "/r") }]);
        let out = futures::executor::block_on(save_guest(&fake, &ms, "Casa", &draft("ana.x", None, &[("expo-id", "/r2")]), Some(&old)));
        // O id gravado pelo outro app fica: ele continua achando a própria máquina.
        assert_eq!(out.saved.servers[0].server_id, "expo-id");
        assert_eq!(fake.calls.borrow().as_slice(), ["update a"]);
    }

    #[test]
    fn save_keeps_entries_it_could_not_touch() {
        let fake = Fake { fail: vec![("delete", "b", 500)], store_fails: Some(500), ..Fake::default() };
        let old = account("ana.x", vec![access("a", "/a"), access("b", "/b"), access("gone", "/g")]);
        let out = futures::executor::block_on(save_guest(&fake, &machines(), "Casa", &draft("ana.x", None, &[]), Some(&old)));
        assert_eq!(out.saved.servers, vec![access("b", "/b"), access("gone", "/g")]);
        assert!(out.store_failed && !out.name_refused);
        assert_eq!(out.errors.len(), 3);
        assert_eq!(out.errors[2], tr_shared("guests_store_failed", &[("servidor", "Casa")]));
    }

    #[test]
    fn refused_name_is_its_own_outcome() {
        let fake = Fake { store_fails: Some(409), ..Fake::default() };
        let out = futures::executor::block_on(save_guest(&fake, &machines(), "Casa", &draft("ana.x", Some("12345678"), &[("a", "/a")]), None));
        assert!(out.store_failed && out.name_refused);
        assert_eq!(out.errors, vec![tr_shared("convidados_usuario_em_uso", &[])]);
    }

    #[test]
    fn remove_deletes_the_account_only_when_everything_let_go() {
        let fake = Fake { fail: vec![("delete", "a", 404)], ..Fake::default() };
        let errors = futures::executor::block_on(remove_guest(&fake, &machines(), "Casa", &account("ana.x", vec![access("a", "/a")])));
        assert!(errors.is_empty());
        assert!(fake.calls.borrow().contains(&"drop ana.x".to_string()));
        let fake = Fake { fail: vec![("delete", "a", 500)], ..Fake::default() };
        let errors = futures::executor::block_on(remove_guest(&fake, &machines(), "Casa", &account("ana.x", vec![access("a", "/a")])));
        assert_eq!(errors.len(), 1);
        assert!(!fake.calls.borrow().contains(&"drop ana.x".to_string()));
    }

    #[test]
    fn legacy_plan_keeps_the_password_and_the_accesses() {
        let owners: Vec<OwnerServer> = serde_json::from_value(json!([{"id": "srv-hub", "label": "Hub", "baseUrl": ""},
            {"id": "srv-b", "label": "B", "baseUrl": "https://b.ts.net"}, {"id": "srv-c", "label": "Escritório", "baseUrl": "https://c.ts.net"}])).unwrap();
        let admin = |servers: &[(&str, &str)]| LegacyAdmin { user: "ana".into(), password: "senha-dela".into(), sees_owner: true, owner_sees: false,
            servers: servers.iter().map(|(id, root)| LegacyAccess { server_id: (*id).into(), guest_id: format!("g-{id}"), token: format!("t-{id}"), root: (*root).into() }).collect() };
        let ms = machines();
        // O endereço vazio no cofre é a máquina que guarda o cofre antigo.
        let (draft, prev) = legacy_plan(&admin(&[("srv-hub", "/h"), ("srv-b", "/b")]), &owners, &ms, Some("100.64.0.2"), "ana@jeff").unwrap();
        assert_eq!(draft.user, "ana@jeff");
        assert_eq!(draft.password.as_deref(), Some("senha-dela"));
        assert_eq!(draft.servers, vec![("a".to_string(), "/h".to_string()), ("b".to_string(), "/b".to_string())]);
        assert_eq!(prev.servers[1].token.as_deref(), Some("t-srv-b"));
        assert_eq!(prev.servers[0].base_url.as_deref(), Some("https://a.jeff.hangar.dev.br"));
        assert_eq!(legacy_plan(&admin(&[("srv-c", "/c")]), &owners, &ms, None, "ana@jeff").err(), Some(vec!["Escritório".to_string()]));
    }

    #[test]
    fn browser_opens_at_the_saved_folder() {
        let root = |path: &str| Root { name: path.into(), path: path.into() };
        let roots = [root("/home/ana"), root("/home/ana/pessoal"), root("/srv")];
        let at = |chosen: &str| start_at(&roots, chosen).map(|(r, dir)| (r.path, dir));
        // A raiz mais funda que contém a pasta; "/home/anabela" não está dentro de "/home/ana".
        assert_eq!(at("/home/ana/pessoal/hangar"), Some(("/home/ana/pessoal".into(), "/home/ana/pessoal/hangar".into())));
        assert_eq!(at("/srv"), Some(("/srv".into(), "/srv".into())));
        assert_eq!(at("/home/anabela"), Some(("/home/ana".into(), "/home/ana".into())));
        assert_eq!(at(""), Some(("/home/ana".into(), "/home/ana".into())));
        assert!(start_at(&[], "/x").is_none());
    }
}
