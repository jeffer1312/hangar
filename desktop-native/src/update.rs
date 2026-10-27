//! Atualização do próprio app pela release fixa `native-latest`, reescrita pelo CI a cada push na main. Com o servidor
//! desta máquina num canal de testes (`pre_voo.alvo` fora da main), a release é a `native-<branch>` dessa branch.
//! Lê só o manifesto (pelo endereço de download, que não gasta a cota da API do GitHub), ao abrir e a cada 6 h.
//! Atualizar baixa o binário da plataforma, confere o sha256 do manifesto e troca o arquivo guardando o anterior em
//! `<exe>.old` (no Windows, `<exe>.old-<horário>` se o `.old` ainda estiver preso). O processo velho continua de pé
//! até o novo gravar o próprio pid em `<exe>.alive`: se ele morrer ou não der sinal a tempo, o anterior volta para o
//! lugar e este processo segue aberto. Nunca fica sem app.
use crate::{api::{Api, Failure}, i18n::{tr, tr_shared}, theme};
use gpui_kit::{assets::IconName, component::{button::*, notification::Notification, *}, *};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, ffi::OsString, path::{Path, PathBuf}, sync::Arc, time::Duration};
use tokio::runtime::Runtime;

mod all;
pub(crate) mod confirm;

const RELEASES: &str = "https://github.com/jeffer1312/hangar/releases/download";
/// Branch que o CI compilou (native.yml). Build local não sabe a dele: só versão mais nova o troca.
const BUILT_CHANNEL: Option<&str> = option_env!("HANGAR_NATIVE_CHANNEL");

/// O assistente de instalação escolhe o canal do pacote por ele.
pub(crate) fn built_channel() -> Option<&'static str> { BUILT_CHANNEL }
const EVERY: Duration = Duration::from_secs(6 * 3600);
/// Janela GPUI fria num disco lento leva segundos; 30 s cobre com folga sem deixar o usuário esperando à toa.
const ALIVE_WAIT: Duration = Duration::from_secs(30);
const ALIVE_ENV: &str = "HANGAR_NATIVE_ALIVE";
/// O CI sobe os binários da release em poucos segundos: a segunda tentativa já encontra o arquivo novo inteiro.
const RETRY_PAUSE: Duration = Duration::from_secs(5);
pub const CURRENT: &str = env!("HANGAR_NATIVE_RELEASE");

/// Mesma limpeza do passo de publicação do native.yml: mudar uma exige mudar a outra.
fn release_tag(channel: &str) -> String {
    if channel == "main" { return "native-latest".into(); }
    format!("native-{}", channel.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect::<String>())
}

/// Só para provas: HANGAR_NATIVE_UPDATE_URL aponta para uma release falsa local.
fn base(channel: &str) -> String {
    std::env::var("HANGAR_NATIVE_UPDATE_URL").unwrap_or_else(|_| format!("{RELEASES}/{}", release_tag(channel)))
}

/// Nome do binário cru desta plataforma na release. Plataforma sem build no CI não procura nada.
fn asset() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("Hangar-linux-x86_64"),
        ("windows", "x86_64") => Some("Hangar-windows-x86_64.exe"),
        ("macos", "aarch64") => Some("Hangar-macos-aarch64"),
        ("macos", "x86_64") => Some("Hangar-macos-x86_64"),
        _ => None,
    }
}

/// `0.1.0.2533` contra `0.1.0.2540`, número a número. Texto que não é versão nunca é "mais novo".
fn newer(remote: &str, local: &str) -> bool {
    let parse = |v: &str| v.split('.').map(str::parse::<u64>).collect::<Result<Vec<_>, _>>().ok();
    matches!((parse(remote), parse(local)), (Some(r), Some(l)) if r > l)
}

fn installed_restart_needed(mark: &Path, exe: Option<&Path>, installed: Option<&Path>, current: &str,
    built: Option<&str>, channel: &str, busy: bool, attempted: &mut Option<String>) -> bool {
    if busy || channel != "main" || built.is_some_and(|built| !built.is_empty() && built != "main") { return false; }
    let (Some(exe), Some(installed)) = (exe, installed) else { return false };
    // Não resolve o arquivo: um atalho para um build de desenvolvimento continua fora da instalação.
    #[cfg(windows)]
    let same = exe.to_string_lossy().trim_start_matches(r"\\?\").eq_ignore_ascii_case(installed.to_string_lossy().trim_start_matches(r"\\?\"));
    #[cfg(not(windows))]
    let same = exe == installed;
    if !same { return false; }
    let Some(mark) = std::fs::read(mark).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) else { return false };
    let Some(version) = mark["version"].as_str() else { return false };
    if mark["sem_build"].as_bool() == Some(true) || !newer(version, current) || attempted.as_deref() == Some(version) { return false; }
    // Relançamento que falha mantém a janela: não tenta o mesmo arquivo quebrado a cada conferência.
    *attempted = Some(version.to_owned());
    true
}

/// Versão mais nova da mesma release, ou qualquer versão de outra: trocar de canal (inclusive a volta para a main, que
/// costuma ter menos commits que a branch de teste) não pode esperar a contagem passar a do app instalado. Compara pela
/// release, não pelo nome: `a/b` e `a-b` publicam na mesma e não podem se oferecer uma à outra para sempre.
fn offered(remote: &str, channel: &str, current: &str, built: Option<&str>) -> bool {
    newer(remote, current) || (newer(remote, "0") && built.is_some_and(|built| release_tag(built) != release_tag(channel)))
}

/// Branch que o servidor segue; no pacote, a do canal do manifesto (`latest` = main). Sem o campo (servidor anterior ao
/// canal, checkout ilegível), a main, como o backend.
pub(crate) fn alvo(state: &Value) -> String {
    state["pre_voo"]["alvo"].as_str().filter(|alvo| !alvo.is_empty()).unwrap_or("main").to_owned()
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Versão da API que este app exige do servidor: a que o servidor do mesmo commit fala.
const REQUIRED_API: u32 = hangar_api::CLIENT_API;

/// Release anterior ao campo e servidor anterior à rota contam como a primeira API.
fn first_api() -> u32 { 1 }

#[derive(Deserialize)]
struct Manifest { version: String, files: HashMap<String, String>, #[serde(default = "first_api")] requires_api: u32 }

#[derive(Clone)]
struct Offer { version: String, url: String, sha256: String, channel: String, requires_api: u32 }

/// Por que a procura não achou o que oferecer: a rede falhou, ou a máquina respondeu e a branch não tem o app.
#[derive(Clone, Debug, PartialEq)]
enum CheckFail { Network(String), NoRelease(String) }

impl std::fmt::Display for CheckFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { CheckFail::Network(text) | CheckFail::NoRelease(text) => f.write_str(text) }
    }
}

async fn check(client: &reqwest::Client, channel: &str) -> Result<Option<Offer>, CheckFail> {
    let network = |e: reqwest::Error| CheckFail::Network(e.to_string());
    let missing = || if channel == "main" { Ok(None) }
        else { Err(CheckFail::NoRelease(tr("app_update_channel_missing").replace("{branch}", channel))) };
    let Some(name) = asset() else { return Ok(None) };
    let base = base(channel);
    let response = client.get(format!("{base}/native-latest.json")).send().await.map_err(network)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Sem a release da main: nada a oferecer. Sem a da branch: avisa e fica, nunca oferece a da main no lugar.
        return missing();
    }
    let manifest: Manifest = response.error_for_status().map_err(network)?.json().await.map_err(network)?;
    // Branch cujo build desta plataforma falhou: avisa como a release ausente, em vez de dizer "em dia".
    let Some(sha256) = manifest.files.get(name) else { return missing() };
    Ok(offered(&manifest.version, channel, CURRENT, BUILT_CHANNEL).then(|| Offer { version: manifest.version,
        url: format!("{base}/{name}"), sha256: sha256.to_lowercase(), channel: channel.to_owned(), requires_api: manifest.requires_api }))
}

fn modified(path: &Path) -> Option<std::time::SystemTime> { std::fs::metadata(path).and_then(|m| m.modified()).ok() }

/// O arquivo instalado mudou depois que este processo abriu. Sem uma das datas, não dá para provar que mudou.
fn replaced_since(opened: Option<std::time::SystemTime>, installed_now: Option<std::time::SystemTime>) -> bool {
    matches!((opened, installed_now), (Some(opened), Some(now)) if opened != now)
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = exe.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Onde guardar o binário atual durante a troca.
#[cfg(unix)]
fn old_path(exe: &Path) -> PathBuf { sibling(exe, ".old") }

/// No Windows, um `.old` que ainda é a imagem de um processo vivo (versão anterior que não saiu) não pode ser apagado,
/// e renomear o app por cima dele dá "acesso negado". Os restos saem quando dá; o que ficou preso cede o nome.
#[cfg(windows)]
fn old_path(exe: &Path) -> PathBuf {
    // O NTFS não diferencia maiúsculas: o nome do `current_exe` pode vir com outra caixa que a do disco.
    let prefix = format!("{}.old", exe.file_name().unwrap_or_default().to_string_lossy()).to_lowercase();
    if let Some(entries) = exe.parent().and_then(|dir| std::fs::read_dir(dir).ok()) {
        for entry in entries.flatten().filter(|entry| entry.file_name().to_string_lossy().to_lowercase().starts_with(&prefix)) {
            if let Err(error) = std::fs::remove_file(entry.path()) {
                eprintln!("resto da atualização anterior preso em {}: {error}", entry.path().display());
            }
        }
    }
    let old = sibling(exe, ".old");
    // Arquivo que nega até a leitura (apagado mas ainda aberto) também ocupa o nome.
    if matches!(old.try_exists(), Ok(false)) { return old; }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    sibling(exe, &format!(".old-{stamp}"))
}

/// Confere o sha256 e troca o binário em disco. Devolve o caminho do anterior guardado, ou a frase da falha.
fn swap(exe: &Path, bytes: &[u8], sha256: &str) -> Result<PathBuf, String> {
    use std::io::Write;
    if sha256_hex(bytes) != sha256 { return Err(tr("app_update_bad_sha")); }
    let fail = |e: std::io::Error| tr("app_update_swap_failed").replace("{reason}", &e.to_string());
    let (new, old) = (sibling(exe, ".new"), old_path(exe));
    let place = || -> Result<(), String> {
        let mut file = std::fs::File::create(&new).map_err(fail)?;
        file.write_all(bytes).and_then(|_| file.sync_all()).map_err(fail)?;
        drop(file);
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(fail)?;
            // Cópia, não rename: entre dois renames o caminho do app ficaria vazio.
            std::fs::copy(exe, &old).map_err(fail)?;
            std::fs::rename(&new, exe).map_err(fail)?;
        }
        // No Windows o executável em uso não pode ser sobrescrito, mas pode ser renomeado.
        #[cfg(windows)] {
            std::fs::rename(exe, &old).map_err(fail)?;
            if let Err(error) = std::fs::rename(&new, exe) {
                // Sem desfazer, o caminho do app fica vazio: essa falha não pode sair como "nada foi trocado".
                if let Err(back) = std::fs::rename(&old, exe) {
                    return Err(tr("app_update_rollback_failed").replace("{reason}", &format!("{error}; {back}")));
                }
                return Err(fail(error));
            }
        }
        Ok(())
    };
    // Troca que falhou não deixa o binário baixado ao lado do app.
    place().inspect_err(|_| { let _ = std::fs::remove_file(&new); })?;
    Ok(old)
}

fn rollback(exe: &Path, old: &Path) -> std::io::Result<()> {
    // No Windows o `.old` é a imagem deste processo: renomear continua valendo, sobrescrever o novo exige tirá-lo antes.
    #[cfg(windows)] { let _ = std::fs::remove_file(exe); }
    std::fs::rename(old, exe)
}

/// O novo processo prova que subiu gravando o próprio pid; pid e não "arquivo existe", para um resto antigo não enganar.
async fn alive(child: &mut std::process::Child, path: &Path, wait: Duration) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + wait;
    while tokio::time::Instant::now() < deadline {
        if std::fs::read_to_string(path).is_ok_and(|pid| pid.trim() == child.id().to_string()) { return Ok(()); }
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => return Err(format!("o processo novo saiu sem prova de vida ({status})")),
            Err(error) => { let _ = child.kill(); return Err(format!("o processo novo não pôde ser acompanhado: {error}")); }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(format!("o processo novo não deu prova de vida em {} s", wait.as_secs()))
}

fn spawn_child(exe: &Path, signal: &Path) -> Result<std::process::Child, String> {
    std::process::Command::new(exe).args(std::env::args_os().skip(1)).env(ALIVE_ENV, signal).spawn()
        .map_err(|error| format!("não abriu {}: {error}", exe.display()))
}

/// Sobe o binário que está no caminho do app e espera a prova de vida. Sem ela, a janela única volta para este processo.
async fn relaunch(exe: &Path) -> Result<(), String> {
    let signal = sibling(exe, ".alive");
    let _ = std::fs::remove_file(&signal);
    // A versão nova assume o arquivo da janela única antes de provar que subiu.
    let own_address = crate::single_instance::snapshot();
    let up = match spawn_child(exe, &signal) { Ok(mut child) => alive(&mut child, &signal, ALIVE_WAIT).await, Err(error) => Err(error) };
    let _ = std::fs::remove_file(&signal);
    if let Err(reason) = &up {
        crate::log_line(&format!("relançar o app falhou: {reason}"));
        if let Some(address) = own_address { crate::single_instance::restore(address); }
    }
    up
}

/// O motivo técnico vai junto da frase da tela: sem ele, quem lê o erro não tem o que procurar no log.
fn with_reason(text: String, reason: &str) -> String { format!("{text} ({reason})") }

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, reqwest::Error> {
    Ok(client.get(url).send().await?.error_for_status()?.bytes().await?.into())
}

/// A release é republicada a cada push: o binário trocado no meio da transferência corta o corpo, e uma segunda
/// tentativa pega o arquivo novo (o sha dele é conferido depois, contra o manifesto relido).
async fn download(client: &reqwest::Client, url: &str, pause: Duration) -> Result<Vec<u8>, String> {
    if let Ok(bytes) = fetch(client, url).await { return Ok(bytes) }
    tokio::time::sleep(pause).await;
    fetch(client, url).await.map_err(|error| tr("app_update_download_failed").replace("{reason}", &cause(error)))
}

/// A frase do reqwest leva a URL assinada inteira e esconde a causa: sai a URL, entra a cadeia de origens.
fn cause(error: reqwest::Error) -> String {
    let error = error.without_url();
    std::iter::successors(Some(&error as &dyn std::error::Error), |e| e.source()).map(ToString::to_string).collect::<Vec<_>>().join(": ")
}

/// A oferta aceita contra o manifesto relido agora: a release republicada vale se couber na API que o plano aceitou;
/// passou a exigir mais, a oferta acabou (`None`). Leitura que falha ou sem oferta mantém a aceita.
fn still_offered(accepted: Offer, read: Result<Option<Offer>, CheckFail>) -> Option<Offer> {
    match read {
        Ok(Some(fresh)) if fresh.requires_api > accepted.requires_api => None,
        Ok(Some(fresh)) => Some(fresh),
        Ok(None) | Err(_) => Some(accepted),
    }
}

/// `Ok(false)`: a oferta acabou antes da troca (a release passou a exigir mais) e nada foi mexido; sem aviso, o próximo
/// ciclo refaz o plano.
async fn install(client: reqwest::Client, exe: Option<PathBuf>, offer: Offer) -> Result<bool, String> {
    // A oferta pode ter horas e a release é republicada a cada push: o sha que vale é o do manifesto de agora.
    let channel = offer.channel.clone();
    let Some(offer) = still_offered(offer, check(&client, &channel).await) else { return Ok(false) };
    let exe = exe.ok_or_else(|| tr("app_update_swap_failed").replace("{reason}", "current_exe"))?;
    // O instalador pode ter posto esta versão no caminho do app com ele aberto: não há o que baixar nem trocar.
    let (disk, wanted) = (exe.clone(), offer.sha256.clone());
    let placed = tokio::task::spawn_blocking(move || std::fs::read(&disk).is_ok_and(|bytes| sha256_hex(&bytes) == wanted));
    if placed.await.unwrap_or(false) {
        return relaunch(&exe).await.map(|()| true).map_err(|reason| with_reason(tr("app_update_relaunch_failed"), &reason));
    }
    let bytes = download(&client, &offer.url, RETRY_PAUSE).await?;
    // O CI sobe os binários antes do manifesto: no meio da publicação o binário já é o novo e o sha ainda o antigo.
    let sha256 = if sha256_hex(&bytes) == offer.sha256 { offer.sha256 } else {
        // O binário novo pode ser de uma release que já exige mais: sem ele não há o que trocar, nem erro de checksum.
        let Some(fresh) = still_offered(offer, check(&client, &channel).await) else { return Ok(false) };
        fresh.sha256
    };
    // Dezenas de MB conferidos, gravados e copiados: fora das duas threads do runtime.
    let target = exe.clone();
    let old = tokio::task::spawn_blocking(move || swap(&target, &bytes, &sha256)).await.map_err(|e| e.to_string())??;
    let Err(reason) = relaunch(&exe).await else { return Ok(true) };
    rollback(&exe, &old).map_err(|e| tr("app_update_rollback_failed").replace("{reason}", &e.to_string()))?;
    Err(with_reason(tr("app_update_rolled_back"), &reason))
}

pub fn relaunched() -> bool { std::env::var_os(ALIVE_ENV).is_some() }

/// Chamado pelo processo novo quando a janela abriu: é a prova de vida que o antigo espera.
pub fn report_alive() {
    if let Some(path) = std::env::var_os(ALIVE_ENV) { let _ = std::fs::write(path, std::process::id().to_string()); }
}

/// Por que o "Atualizar tudo" não mexe no servidor desta máquina.
#[derive(Clone, Debug, PartialEq)]
enum Hold {
    /// A atualização alinha o disco com a branch alvo (`pre_voo.alvo`, main em servidor antigo) e arrastaria a branch.
    WorkBranch(String, String),
    /// Mudanças locais, commits não enviados ou divergência: o motor faria stash + reset e tiraria do disco o trabalho de
    /// outras sessões. É o mesmo portão da atualização automática.
    LocalChanges,
    Running,
    Missing(String),
    ChannelDraft,
}

impl Hold {
    /// Só impedimentos à execução conjunta bloqueiam também o app.
    fn stops(&self) -> bool { matches!(self, Hold::Running | Hold::Missing(_)) }

    fn text(&self) -> String {
        match self {
            Hold::WorkBranch(branch, target) => tr("app_update_hold_branch").replace("{branch}", branch).replace("{alvo}", target),
            Hold::LocalChanges => tr("app_update_hold_changes"),
            Hold::Running => tr("app_update_hold_running"),
            Hold::Missing(what) => tr("app_update_hold_missing").replace("{what}", what),
            Hold::ChannelDraft => tr("settings_channel_pending"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ServerStep { Skip, Update, Held(Hold) }

#[derive(Debug, PartialEq)]
struct Plan { server: ServerStep, app: bool }

impl Plan {
    fn visible(&self) -> bool { self.app || matches!(&self.server, ServerStep::Update) || matches!(&self.server, ServerStep::Held(h) if h.stops()) }
}

/// Servidor atrás: há atualização publicada a puxar, ou o disco não é o que está no ar. A versão do app não entra: quem
/// diz se os dois se entendem é a API. O `-dirty` sai da comparação: qualquer edição depois do boot deixaria o servidor
/// "atrás" para sempre.
fn behind(state: &Value) -> bool {
    let flag = state["atualizacao_disponivel"].as_bool() == Some(true);
    let clean = |v: &Value| v.as_str().map(|s| s.trim_end_matches("-dirty").to_owned());
    let restart = matches!((clean(&state["versoes"]["repo"]), clean(&state["versoes"]["backend"])), (Some(disk), Some(up)) if disk != up);
    flag || restart
}

/// API do servidor que a atualização alcança; ausente ou ilegível (branch sem release do servidor) é 1.
fn published_api(state: &Value) -> u32 {
    state["api_publicado"].as_u64().and_then(|api| u32::try_from(api).ok()).unwrap_or_else(first_api)
}

/// `GET /api/client-api`. 404 é servidor anterior à rota; qualquer outra falha (o 503 do modo python inclusive) é
/// desconhecido, que nunca vira 1.
fn client_api(read: Result<Value, Failure>) -> Option<u32> {
    match read {
        Ok(value) => value["api"].as_u64().and_then(|api| u32::try_from(api).ok()),
        Err(error) if error.status == Some(404) => Some(first_api()),
        Err(_) => None,
    }
}

/// Lido do mesmo `pre_voo` que o servidor usa para recusar o `iniciar`: o botão sabe antes do clique.
fn hold(state: &Value) -> Option<Hold> {
    let pre = &state["pre_voo"];
    if pre["branch_de_trabalho"].as_bool() == Some(true) {
        return Some(Hold::WorkBranch(pre["branch"].as_str().unwrap_or("?").to_owned(), pre["alvo"].as_str().unwrap_or("main").to_owned()));
    }
    let count = |key: &str| pre[key].as_u64().unwrap_or(0);
    if count("sujo") > 0 || count("ahead") > 0 || pre["divergiu"].as_bool() == Some(true) { return Some(Hold::LocalChanges); }
    if state["estado"]["fase"].as_str() == Some("rodando") { return Some(Hold::Running); }
    if pre["pode"].as_bool() == Some(false) {
        let missing: Vec<&str> = pre["faltando"].as_array().map(|list| list.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let what = if missing.is_empty() { pre["erro"].as_str().unwrap_or("?").to_owned() } else { missing.join(", ") };
        return Some(Hold::Missing(what));
    }
    None
}

/// O que o "Atualizar tudo" faz: `need` é a API que a release do app exige, `server` o `GET /api/atualizacao` do servidor
/// desta máquina (nenhum = não há servidor local) e `run` a API no ar (a do local, ou a do ativo sem local; `None` =
/// desconhecida). O botão só aparece quando um clique consegue concluir.
fn plan(need: Option<u32>, server: Option<&Value>, run: Option<u32>) -> Plan {
    let step = match server {
        Some(state) if behind(state) => hold(state).map_or(ServerStep::Update, ServerStep::Held),
        _ => ServerStep::Skip,
    };
    let Some(need) = need else { return Plan { server: step, app: false } };
    // Todo servidor fala a API 1; desconhecido não promete mais que isso.
    if install_after(need, run) {
        let stops = matches!(&step, ServerStep::Held(h) if h.stops());
        return Plan { app: !stops, server: step };
    }
    // O servidor publicado ainda não chega lá (build pendente, branch sem release): nada a oferecer, sem aviso.
    if server.is_none_or(|state| need > published_api(state)) { return Plan { server: ServerStep::Skip, app: false }; }
    Plan { app: run.is_some() && step == ServerStep::Update, server: step }
}

/// O servidor ativo não fala a API que este app exige. Desconhecida não afirma nada.
fn outdated(run: Option<u32>) -> bool { run.is_some_and(|run| REQUIRED_API > run) }

pub(crate) fn update_process_id(value: &Value) -> Option<i64> {
    (value["ok"].as_bool() == Some(true)).then(|| value["pid"].as_i64()).flatten().filter(|pid| *pid > 0)
}

pub(crate) fn restart_result_matches(state: &Value, pid: i64, request_id: Option<&str>) -> bool {
    match request_id {
        Some(id) => state["request_id"].as_str() == Some(id),
        None => state["pid"].as_i64() == Some(pid),
    }
}

/// Leituras sem resposta seguidas que encerram o acompanhamento (10 min a 2 s).
const SILENT_READS: u32 = 300;
/// Motor travado com pid vivo não prende o app para sempre.
const FOLLOW_CEILING: Duration = Duration::from_secs(30 * 60);

/// Motor respondendo nunca encerra por tempo; só o silêncio seguido ou o teto total. `step` é a leitura desta volta.
/// Desfecho lido junto com o teto vale: o servidor já terminou.
fn follow_limit(step: Option<&Follow>, silent: u32, elapsed: Duration) -> Option<&'static str> {
    if matches!(step, Some(Follow::Done(_) | Follow::NotStarted)) { return None; }
    if silent >= SILENT_READS { return Some("update_silent"); }
    (elapsed >= FOLLOW_CEILING).then_some("update_too_long")
}

/// Na retomada, servidor sem estado nenhum é o app que morreu antes do `iniciar`: não há rodada a esperar.
fn resumed_step(step: Follow) -> Follow {
    if step == Follow::Waiting { Follow::NotStarted } else { step }
}

/// O desfecho é desta rodada quando o `ts` dele vem depois do desfecho anterior ao clique. Nunca pelo pid: no Windows o
/// lançador do venv abre um filho, e o pid do estado não é o devolvido pelo `iniciar`.
fn after_baseline(ts: Option<&str>, baseline: Option<&str>) -> bool {
    let (Some(ts), Some(baseline)) = (ts, baseline) else { return baseline.is_none() };
    match (chrono::DateTime::parse_from_rfc3339(ts), chrono::DateTime::parse_from_rfc3339(baseline)) {
        (Ok(ts), Ok(baseline)) => ts > baseline,
        _ => ts != baseline,
    }
}

const INTENT: &str = "update-intent.json";
const INTENT_LOCK: &str = "update-intent.lock";

/// "Servidor e depois app", gravado antes do `iniciar`: fechar o app no meio não perde a instalação do app.
#[derive(serde::Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Intent {
    /// `Api::identity` do servidor desta máquina.
    servidor: String,
    app_version: String,
    /// `ts` do desfecho anterior ao clique: o desfecho desta rodada vem depois dele.
    #[serde(default)]
    baseline: Option<String>,
}

/// Trava ao lado da intenção: duas instâncias do app nunca acompanham a mesma atualização.
/// `Ok(None)`: outra instância segura a trava.
fn intent_lock(dir: &Path) -> std::io::Result<Option<std::fs::File>> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(INTENT_LOCK))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

fn intent_write(dir: &Path, intent: &Intent) -> std::io::Result<()> {
    let tmp = dir.join(format!("{INTENT}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(intent)?)?;
    std::fs::rename(&tmp, dir.join(INTENT))
}

fn intent_read(dir: &Path) -> Option<Intent> { serde_json::from_slice(&std::fs::read(dir.join(INTENT)).ok()?).ok() }

fn intent_delete(dir: &Path) { let _ = std::fs::remove_file(dir.join(INTENT)); }

/// Só retoma a intenção do mesmo servidor e de um app ainda mais velho que a versão pedida: o app novo não herda a dele.
fn intent_valid(intent: &Intent, identity: &str, current: &str) -> bool {
    intent.servidor == identity && newer(&intent.app_version, current)
}

/// Depois do desfecho do servidor, o app só entra com a API que exige no ar; desconhecida nunca.
fn install_after(need: u32, api: Option<u32>) -> bool { need <= first_api() || api.is_some_and(|api| need <= api) }

static RESUME_TRIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, PartialEq)]
enum Outcome { Updated, UpdatedManual, Failed(String) }

#[derive(Debug, PartialEq)]
enum Follow { Running { step: u64, total: u64, text: String }, Waiting, NotStarted, Done(Outcome) }

/// Uma leitura do estado durante a atualização do servidor. `baseline` é o `ts` do desfecho anterior: "pronto" com ele, sem
/// ter visto "rodando", é o desfecho velho, não o desta vez.
fn follow(state: &Value, saw_running: bool, baseline: Option<&str>) -> Follow {
    let text = |key: &str| state[key].as_str().unwrap_or("").to_owned();
    match state["fase"].as_str() {
        Some("rodando") => Follow::Running { step: state["passo"].as_u64().unwrap_or(0), total: state["total"].as_u64().unwrap_or(0), text: text("texto") },
        Some("pronto") if !saw_running && !after_baseline(state["ts"].as_str(), baseline) => Follow::NotStarted,
        Some("pronto") if state["ok"].as_bool() != Some(true) => {
            let reason = Some(text("erro").trim_end_matches('.').to_owned()).filter(|e| !e.is_empty()).unwrap_or_else(|| "?".into());
            Follow::Done(Outcome::Failed(reason))
        }
        Some("pronto") if state["reiniciar_manual"].as_bool() == Some(true) => Follow::Done(Outcome::UpdatedManual),
        Some("pronto") => Follow::Done(Outcome::Updated),
        _ => Follow::Waiting,
    }
}

enum Run {
    Idle,
    Searching,
    Server { step: u64, total: u64, text: String },
    Restarting,
    DesktopRestart,
    App,
    Failed(String),
}

impl Run {
    fn busy(&self) -> bool { !matches!(self, Self::Idle | Self::Failed(_)) }

    fn begin_server_search(&mut self) -> bool {
        if self.busy() { return false; }
        *self = Self::Searching;
        true
    }
}

pub struct Updater {
    runtime: Arc<Runtime>,
    client: reqwest::Client,
    /// Lido ao abrir: depois da primeira troca o Linux passa a responder "<caminho> (deleted)" para este processo.
    exe: Option<PathBuf>,
    /// Data de modificação do executável quando este processo abriu.
    exe_stamp: Option<std::time::SystemTime>,
    offer: Option<Offer>,
    /// O servidor desta máquina (endereço de loopback), dado pelo app sempre que a lista de servidores muda.
    local: Option<Api>,
    server: Option<Value>,
    /// `GET /api/client-api` do servidor desta máquina (`None` = desconhecida).
    server_api: Option<u32>,
    server_seq: u64,
    /// O servidor ativo, para o aviso de servidor desatualizado (pode ser o mesmo do `local`).
    active: Option<Api>,
    active_state: Option<Value>,
    active_api: Option<u32>,
    active_seq: u64,
    run: Run,
    /// Procura do app em andamento e o desfecho da última, para a página Sobre.
    checking: bool,
    checked: Option<Result<(), CheckFail>>,
    /// Branch da última procura que terminou: o canal do servidor mudar depois dela pede outra.
    checked_channel: Option<String>,
    /// Último `pre_voo.alvo` lido do servidor desta máquina: leitura que falha não muda o canal.
    known_channel: Option<String>,
    channel_blocked: Option<String>,
    /// O assistente de instalação está rodando: nem procura nem troca o app no meio (spec "Destacado do app").
    suspended: bool,
    /// Trava da intenção que este processo acompanha; com ela, só ele apaga o arquivo.
    intent_lock: Option<std::fs::File>,
    all_servers: Vec<crate::app::ServerEntry>,
    survey: all::Survey,
    batch: Option<all::Batch>,
    all_running: bool,
}

/// Linha "Canal de testes" da página Sobre: a main não mostra nada.
pub fn test_channel(branch: &str) -> Option<String> {
    (!branch.is_empty() && branch != "main").then(|| tr("about_test_channel").replace("{branch}", branch))
}

/// Canal do app na página Sobre: a branch do build e, quando a procura já segue outra, a próxima.
pub fn app_channel(built: Option<&str>, following: &str) -> Vec<String> {
    let built = built.filter(|built| !built.is_empty()).unwrap_or("main");
    let next = (release_tag(following) != release_tag(built)).then(|| tr("about_following_channel").replace("{branch}", following));
    test_channel(built).into_iter().chain(next).collect()
}

/// O que a página Sobre mostra na linha do app.
pub enum AppCheck { Never, Checking, UpToDate, Available(String), Failed(String), NoRelease(String) }

pub struct Handle(pub Entity<Updater>);
impl Global for Handle {}

/// Cria o indicador e começa a procurar. A barra do topo o encontra pelo `Handle` global.
pub fn start(runtime: Arc<Runtime>, cx: &mut App) {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(300)).user_agent(concat!("hangar-native/", env!("HANGAR_NATIVE_RELEASE")))
        .build().unwrap_or_default();
    let exe = std::env::current_exe().ok();
    let exe_stamp = exe.as_deref().and_then(modified);
    let entity = cx.new(|_| Updater { runtime, client, exe, exe_stamp, offer: None, local: None, server: None,
        server_api: None, server_seq: 0, active: None, active_state: None, active_api: None, active_seq: 0, run: Run::Idle, checking: false, checked: None, checked_channel: None, known_channel: None, channel_blocked: None, suspended: false, intent_lock: None, all_servers: Vec::new(), survey: all::Survey::default(), batch: None, all_running: false });
    let weak = entity.downgrade();
    cx.spawn(async move |cx| loop {
        let Ok(()) = weak.update(cx, |this, cx| {
            this.refresh_server(cx);
            this.refresh_active(cx);
            this.check_app(cx);
            this.refresh_machine_updates(cx);
        }) else { return };
        cx.background_executor().timer(EVERY).await;
    }).detach();
    cx.observe(&entity, |entity, cx| {
        if entity.read(cx).survey.pending && !entity.read(cx).busy() { entity.update(cx, |this, cx| this.refresh_machine_updates(cx)); }
    }).detach();
    cx.set_global(Handle(entity));
}

fn read_state(runtime: &Runtime, api: &Api, query: &'static [(&'static str, &'static str)], seconds: u64) -> tokio::task::JoinHandle<Result<Value, Failure>> {
    let api = api.clone();
    runtime.spawn(async move { api.server_read(&["atualizacao"], query, seconds).await })
}

fn read_api(runtime: &Runtime, api: &Api) -> tokio::task::JoinHandle<Option<u32>> {
    let api = api.clone();
    runtime.spawn(async move { client_api(api.server_read(&["client-api"], &[], 20).await) })
}

impl Updater {
    pub fn set_channel_blocked(&mut self, address: Option<String>, cx: &mut Context<Self>) {
        if self.channel_blocked == address { return; }
        self.channel_blocked = address;
        cx.notify();
    }

    pub fn is_channel_blocked(&self) -> bool {
        self.local.as_ref().is_some_and(|api| self.channel_blocked.as_deref() == Some(api.identity().as_str()))
    }

    /// Branch que o app segue: a última lida do servidor desta máquina. Antes da primeira leitura, a branch em que este app
    /// foi compilado (servidor fora do ar não troca de canal); sem servidor local, a main.
    fn channel(&self) -> String {
        if self.local.is_none() { return "main".into(); }
        self.known_channel.clone().unwrap_or_else(|| BUILT_CHANNEL.unwrap_or("main").to_owned())
    }

    /// Todo estado lido do servidor desta máquina passa por aqui: canal trocado fora deste app (celular, web, `.env`)
    /// refaz a procura da release, e a oferta da branch anterior deixa de valer.
    fn receive_server(&mut self, state: Option<Value>, cx: &mut Context<Self>) {
        if let Some(state) = &state { self.known_channel = Some(alvo(state)); }
        self.server = state;
        if self.checked_channel.as_ref().is_some_and(|checked| *checked != self.channel()) { self.check_app(cx); }
    }

    /// O canal de testes do servidor mudou pela tela: relê o estado e, com ele, a release do app.
    pub fn refresh(&mut self, cx: &mut Context<Self>) { self.refresh_server(cx) }

    /// A lista de servidores ou o servidor ativo mudou: relê o estado dos dois.
    pub fn set_servers(&mut self, local: Option<Api>, active: Option<Api>, cx: &mut Context<Self>) {
        if self.local.as_ref().map(Api::identity) != local.as_ref().map(Api::identity) { self.known_channel = None; }
        self.local = local;
        self.active = active;
        self.resume(cx);
        self.refresh_server(cx);
        self.refresh_active(cx);
    }

    /// Ao abrir, com o servidor desta máquina conhecido: a intenção deixada por um app fechado no meio da atualização do
    /// servidor volta a ser acompanhada, e o desfecho dela segue a mesma regra do clique.
    fn resume(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.local.clone() else { return };
        if self.busy() || self.suspended || RESUME_TRIED.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
        let Some(dir) = crate::appearance::dir() else { return };
        // Outra instância segura a trava: ela decide.
        let lock = match intent_lock(&dir) {
            Ok(Some(lock)) => lock,
            Ok(None) => return,
            Err(error) => return crate::log_line(&format!("trava da intenção de atualização: {error}")),
        };
        let Some(intent) = intent_read(&dir) else { return };
        if !intent_valid(&intent, &api.identity(), CURRENT) { return intent_delete(&dir); }
        self.intent_lock = Some(lock);
        self.run = Run::Server { step: 0, total: 0, text: String::new() };
        cx.notify();
        cx.spawn(async move |this, cx| {
            // Os avisos do desfecho saem numa janela.
            let Some(handle) = cx.update(|cx| cx.windows().first().copied()) else {
                let _ = this.update(cx, |this, cx| { this.run = Run::Idle; this.intent_lock = None; cx.notify(); });
                return;
            };
            let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx|
                this.follow_server(api, false, intent.baseline, true, window, cx)); });
        }).detach();
    }

    /// Grava a intenção sob a trava antes do `iniciar`. Sem app a instalar depois não há o que retomar. `Err` = outra
    /// instância acompanha uma atualização; qualquer outra falha segue sem intenção, como a gravação que falha.
    fn begin_intent(&mut self, api: &Api, baseline: Option<String>) -> Result<(), ()> {
        let (Some(offer), Some(dir)) = (self.offer.as_ref(), crate::appearance::dir()) else { return Ok(()) };
        let lock = match intent_lock(&dir) {
            Ok(Some(lock)) => lock,
            Ok(None) => return Err(()),
            Err(error) => { crate::log_line(&format!("trava da intenção de atualização: {error}")); return Ok(()) }
        };
        let intent = Intent { servidor: api.identity(), app_version: offer.version.clone(), baseline };
        match intent_write(&dir, &intent) {
            Ok(()) => self.intent_lock = Some(lock),
            Err(error) => crate::log_line(&format!("intenção de atualização não gravada: {error}")),
        }
        Ok(())
    }

    /// Desfecho de qualquer tipo: a intenção deste processo sai do disco e a trava é solta.
    fn end_intent(&mut self) {
        if let Some(_lock) = self.intent_lock.take() {
            if let Some(dir) = crate::appearance::dir() { intent_delete(&dir); }
        }
    }

    /// Procura versão nova do app agora. Falha não vira aviso na tela: fica no stderr e na linha da página Sobre.
    pub fn check_app(&mut self, cx: &mut Context<Self>) {
        if self.checking || self.suspended { return; }
        self.checking = true;
        cx.notify();
        let (client, channel) = (self.client.clone(), self.channel());
        let task = { let channel = channel.clone(); self.runtime.spawn(async move { check(&client, &channel).await }) };
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(CheckFail::Network(e.to_string())));
            let _ = this.update(cx, |this, cx| {
                this.checking = false;
                // O canal mudou durante a procura: a resposta é de outra release.
                if this.channel() != channel { return this.check_app(cx); }
                // `Failed` também: o "Tentar de novo" instala a oferta guardada.
                let idle = !this.busy();
                match result {
                    Ok(found) => {
                        if idle { this.offer = found; }
                        this.checked = Some(Ok(()));
                    }
                    Err(error) => {
                        eprintln!("procura de atualização do app falhou: {error}");
                        // A oferta de outro canal não vale mais: o Atualizar instalaria o app da branch errada.
                        if idle { this.offer.take_if(|offer| offer.channel != channel); }
                        this.checked = Some(Err(error));
                    }
                }
                this.checked_channel = Some(channel);
                cx.notify();
            });
        }).detach();
    }

    pub fn app_check(&self) -> AppCheck {
        match (&self.offer, self.checking, &self.checked) {
            (Some(offer), false, _) => AppCheck::Available(offer.version.clone()),
            (_, true, _) => AppCheck::Checking,
            (None, _, Some(Ok(()))) => AppCheck::UpToDate,
            (None, _, Some(Err(CheckFail::Network(error)))) => AppCheck::Failed(error.clone()),
            (None, _, Some(Err(CheckFail::NoRelease(text)))) => AppCheck::NoRelease(text.clone()),
            (None, _, None) => AppCheck::Never,
        }
    }

    /// Mesma ação do botão do topo: atualiza o servidor desta máquina se estiver atrás e depois o app.
    pub fn start_update(&mut self, window: &mut Window, cx: &mut Context<Self>) { if !self.suspended { self.run(window, cx) } }

    /// Ligado pelo assistente enquanto o script roda; ao desligar, a procura que ficou para trás sai agora.
    pub fn set_suspended(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.suspended == on { return; }
        self.suspended = on;
        if !on { self.check_app(cx); self.resume(cx); }
        cx.notify();
    }

    pub fn is_busy(&self) -> bool { self.busy() }

    pub(crate) fn installed_update_ready(&self, attempted: &mut Option<String>) -> bool {
        if !cfg!(any(target_os = "linux", windows)) { return false; }
        let Some(home) = std::env::home_dir() else { return false };
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let installed = crate::app::setup::installed_app_target(cfg!(windows), &home, local.as_deref());
        // Relançar o mesmo arquivo que já está rodando não atualiza nada: um build local mais velho que a release
        // reabriria para sempre.
        if !replaced_since(self.exe_stamp, installed.as_deref().and_then(modified)) { return false; }
        installed_restart_needed(&home.join(".hangar/native/release.json"), self.exe.as_deref(), installed.as_deref(), CURRENT,
            BUILT_CHANNEL, &self.channel(), self.restart_held(), attempted)
    }

    /// Atualizando ou com o assistente aberto: o reinício por troca externa espera.
    pub(crate) fn restart_held(&self) -> bool { self.busy() || self.suspended }

    pub fn restart_desktop(&mut self, cx: &mut Context<Self>) -> Result<tokio::task::JoinHandle<Result<(), String>>, String> {
        if self.busy() { return Err(tr("app_restart_busy")); }
        let exe = self.exe.clone().ok_or_else(|| tr("app_restart_failed"))?;
        self.run = Run::DesktopRestart;
        cx.notify();
        Ok(self.runtime.spawn(async move { relaunch(&exe).await }))
    }

    pub fn finish_desktop_restart(&mut self, cx: &mut Context<Self>) {
        // Falha manual não é falha de instalação: nunca oferece download no botão de tentar de novo.
        if matches!(self.run, Run::DesktopRestart) { self.run = Run::Idle; cx.notify(); }
    }

    pub fn channel_lines(&self) -> Vec<String> { app_channel(BUILT_CHANNEL, &self.channel()) }

    pub fn server_outdated(&self) -> bool { outdated(self.active_api) }

    /// Versão do servidor ativo e a deste app, para a explicação do aviso.
    pub fn outdated_versions(&self) -> (String, String) {
        let running = self.active_state.as_ref().and_then(|s| s["versao_legivel"]["backend"].as_str()).unwrap_or("?").to_owned();
        (running, CURRENT.to_owned())
    }

    fn refresh_active(&mut self, cx: &mut Context<Self>) {
        self.active_seq += 1;
        let seq = self.active_seq;
        let Some(api) = self.active.clone() else { (self.active_state, self.active_api) = (None, None); cx.notify(); return };
        let (task, api_task) = (read_state(&self.runtime, &api, &[], 20), read_api(&self.runtime, &api));
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(Failure::local(e.to_string())));
            let running = api_task.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if this.active_seq != seq { return; }
                // Sem resposta não há o que afirmar sobre a versão: o aviso some.
                this.active_state = result.ok();
                this.active_api = running;
                cx.notify();
            });
        }).detach();
    }

    fn busy(&self) -> bool { self.run.busy() || self.all_running }

    fn plan(&self) -> Plan {
        let need = self.offer.as_ref().map(|o| o.requires_api);
        // Sem servidor local, quem precisa entender o app novo é o servidor ativo.
        let run = if self.local.is_some() { self.server_api } else { self.active_api };
        if self.is_channel_blocked() { return Plan { server: ServerStep::Held(Hold::ChannelDraft), app: plan(need, None, run).app }; }
        plan(need, self.local.as_ref().and(self.server.as_ref()), run)
    }

    fn refresh_server(&mut self, cx: &mut Context<Self>) {
        if self.busy() { return; }
        self.server_seq += 1;
        let seq = self.server_seq;
        let Some(api) = self.local.clone() else { (self.server, self.server_api) = (None, None); cx.notify(); return };
        let (task, api_task) = (read_state(&self.runtime, &api, &[], 20), read_api(&self.runtime, &api));
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(Failure::local(e.to_string())));
            let running = api_task.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if this.server_seq != seq { return; }
                this.server_api = running;
                // Servidor local fora do ar: sem ele o botão cuida só do app.
                this.receive_server(result.map_err(|e| eprintln!("estado do servidor desta máquina: {}", e.detail)).ok(), cx);
                cx.notify();
            });
        }).detach();
    }

    fn fail(&mut self, reason: String, cx: &mut Context<Self>) {
        self.local_batch_status(all::Status::Failed(reason.clone()), cx);
        self.end_intent();
        self.run = Run::Failed(reason);
        cx.notify();
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.batch.is_some() { return self.open_batch(window, cx); }
        if self.busy() { return; }
        // Só o modal do servidor mostra avisos e novidades: com ele, busca no remoto e lê a rota antes de abrir.
        let modal = self.update_all_count() >= 2 || matches!(self.plan().server, ServerStep::Update);
        let Some(api) = self.local.clone().filter(|_| modal) else { return self.confirm(confirm::Notes::default(), window, cx) };
        self.run = Run::Searching;
        cx.notify();
        confirm::load_then_open(api, &self.runtime, window, cx, |this, state, notes, window, cx| {
            this.run = Run::Idle;
            if state.is_some() { this.receive_server(state, cx); }
            cx.notify();
            this.confirm(notes, window, cx);
        });
    }

    fn confirm(&mut self, notes: confirm::Notes, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_all_count() >= 2 { self.confirm_all(notes, window, cx) } else { self.run_local(notes, window, cx) }
    }

    fn run_local(&mut self, notes: confirm::Notes, window: &mut Window, cx: &mut Context<Self>) {
        let plan = self.plan();
        match &plan.server {
            ServerStep::Held(hold) if hold.stops() => self.fail(hold.text(), cx),
            ServerStep::Update => {
                let this = cx.entity().downgrade();
                let (title, desc) = if plan.app { ("app_update_confirm_title", "app_update_confirm_desc") }
                    else { ("app_update_confirm_server_title", "app_update_confirm_server_desc") };
                confirm::open(window, cx, tr(title), tr(desc), notes, None,
                    move |window, cx| { let _ = this.update(cx, |this, cx| this.search_server(window, cx)); });
            }
            _ if plan.app => self.confirm_channel(window, cx),
            _ => {}
        }
    }

    /// O canal pode ter mudado no servidor depois da última leitura (até 6 h): relê antes de instalar o app.
    fn confirm_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.local.clone() else { return self.install_app(window, cx) };
        self.run = Run::Searching;
        cx.notify();
        let task = read_state(&self.runtime, &api, &[], 20);
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(Failure::local(e.to_string())));
            let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx| {
                this.run = Run::Idle;
                // Sem resposta vale o último canal lido; a oferta de outro canal é barrada no `install_app`.
                if let Ok(state) = result { this.receive_server(Some(state), cx); }
                this.install_app(window, cx);
            }); });
        }).detach();
    }

    /// No clique a versão vem da rede: o `origin/main` do servidor só é renovado a cada 30 min, e a release do app sai logo
    /// depois do push. Com o estado novo, o plano é refeito antes de pedir qualquer coisa.
    fn search_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.local.clone() else { return };
        if !self.run.begin_server_search() {
            window.push_notification(Notification::error(tr("app_restart_busy")), cx);
            return;
        }
        // Atualização só desta máquina: a linha dela na tela de Servidores reconsulta quando terminar.
        // Na rodada de todas, as linhas ficam com o desfecho.
        if self.batch.is_none() { self.survey.pending = true; }
        cx.notify();
        let task = read_state(&self.runtime, &api, &[("procurar", "1")], 150);
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(Failure::local(e.to_string())));
            let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx| match result {
                Err(error) => this.fail(tr("update_failed").replace("{reason}", error.detail.trim_end_matches('.')), cx),
                Ok(state) => {
                    this.run = Run::Idle;
                    this.local_batch_snapshot(&state, cx);
                    this.receive_server(Some(state), cx);
                    let plan = if this.batch.is_some() { this.batch_plan() } else { this.plan() };
                    match &plan.server {
                        ServerStep::Update => this.start_server(api, window, cx),
                        ServerStep::Held(hold) if this.batch.is_some() && hold.stops() => { this.end_intent(); cx.notify(); }
                        ServerStep::Held(hold) if hold.stops() => this.fail(hold.text(), cx),
                        _ if plan.app => this.install_app(window, cx),
                        _ => cx.notify(),
                    }
                }
            }); });
        }).detach();
    }

    fn start_server(&mut self, api: Api, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_channel_blocked() { self.fail(tr("settings_channel_pending"), cx); return; }
        let baseline = self.server.as_ref().and_then(|s| s["estado"]["ts"].as_str()).map(str::to_owned);
        if self.begin_intent(&api, baseline.clone()).is_err() { return self.fail(tr("app_restart_busy"), cx); }
        self.run = Run::Server { step: 0, total: 0, text: String::new() };
        self.local_batch_status(all::Status::Starting, cx);
        cx.notify();
        let task = { let api = api.clone(); self.runtime.spawn(async move { api.server_post(&["atualizacao", "iniciar"], 30).await }) };
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(Failure::local(e.to_string())));
            let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx| match result {
                // O servidor grava "rodando" antes de responder: o próximo "pronto" é o desfecho.
                Ok(value) if update_process_id(&value).is_some() => this.follow_server(api, true, baseline, false, window, cx),
                Ok(_) => this.fail(tr("invalid_response"), cx),
                Err(error) if error.uncertain => this.follow_server(api, false, baseline, false, window, cx),
                // Recusa que o estado lido não previa: relê e decide pela mesma regra (branch → só o app).
                Err(error) if error.status == Some(409) => this.recheck_after_refusal(api, error.detail, window, cx),
                Err(error) => this.fail(tr("update_refused").replace("{reason}", &error.detail), cx),
            }); });
        }).detach();
    }

    fn recheck_after_refusal(&mut self, api: Api, detail: String, window: &mut Window, cx: &mut Context<Self>) {
        let task = read_state(&self.runtime, &api, &[], 20);
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(Failure::local(e.to_string())));
            let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx| {
                // Recusado, o servidor não roda nada desta vez: não há o que retomar.
                this.end_intent();
                this.run = Run::Idle;
                this.receive_server(result.ok(), cx);
                match this.plan() {
                    Plan { server: ServerStep::Held(hold), app: true } if !hold.stops() => this.install_app(window, cx),
                    _ => this.fail(tr("update_refused").replace("{reason}", &detail), cx),
                }
            }); });
        }).detach();
    }

    /// Lê o estado a cada 2 s até o desfecho, sem depender do servidor ativo nem da conexão da janela; a queda durante o
    /// reinício é esperada. Encerra pelo `follow_limit`. `resumed`: retomada ao abrir, em que o desfecho anterior à
    /// intenção só a descarta, sem aviso.
    fn follow_server(&mut self, api: Api, saw_running: bool, baseline: Option<String>, resumed: bool, window: &mut Window,
        cx: &mut Context<Self>) {
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let (mut saw, mut silent, started) = (saw_running, 0, std::time::Instant::now());
            loop {
                let Ok(task) = this.update(cx, |this, _| read_state(&this.runtime, &api, &[], 10)) else { return };
                let read = task.await.unwrap_or_else(|e| Err(Failure::local(e.to_string())));
                let _ = this.update(cx, |this, cx| this.local_batch_read(&read, baseline.as_deref(), cx));
                silent = if read.is_ok() { 0 } else { silent + 1 };
                let step = match &read {
                    Ok(value) => {
                        let combined = this.update(cx, |this, _| this.batch.is_some()).unwrap_or(false);
                        let step = follow(&value["estado"], saw && !combined, baseline.as_deref());
                        let step = if combined && step == Follow::NotStarted { Follow::Waiting } else { step };
                        Some(if resumed { resumed_step(step) } else { step })
                    }
                    Err(error) if matches!(error.status, Some(401 | 403)) => Some(Follow::Done(Outcome::Failed(tr("auth_error")))),
                    Err(_) => None,
                };
                if let Some(key) = follow_limit(step.as_ref(), silent, started.elapsed()) {
                    let _ = this.update(cx, |this, cx| this.fail(tr(key), cx));
                    return;
                }
                if matches!(step, Some(Follow::Running { .. })) { saw = true; }
                let finished = handle.update(cx, |_, window, cx| this.update(cx, |this, cx| {
                    if let Ok(value) = &read { this.receive_server(Some(value.clone()), cx); }
                    match step {
                        Some(Follow::Running { step, total, text }) => { this.run = Run::Server { step, total, text }; cx.notify(); false }
                        None if saw => { this.run = Run::Restarting; cx.notify(); false }
                        None | Some(Follow::Waiting) => false,
                        Some(Follow::NotStarted) if resumed => { this.end_intent(); this.run = Run::Idle; cx.notify(); true }
                        Some(Follow::NotStarted) => { this.fail(tr("update_not_started"), cx); true }
                        Some(Follow::Done(outcome)) => { this.server_done(outcome, window, cx); true }
                    }
                }).unwrap_or(true)).unwrap_or(true);
                if finished { return; }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }).detach();
    }

    /// A atualização pode ter parado num servidor mais velho que o app exige (manifesto mudou no meio, plataforma sem
    /// build): relê a API no ar e a oferta antes de instalar.
    fn server_done(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        let manual = matches!(outcome, Outcome::UpdatedManual);
        if let Outcome::Failed(reason) = outcome { return self.fail(tr("update_failed").replace("{reason}", &reason), cx); }
        let Some(api) = self.local.clone() else { return self.server_updated(manual, None, window, cx) };
        self.run = Run::Searching;
        cx.notify();
        let (client, channel) = (self.client.clone(), self.channel());
        let task = self.runtime.spawn(async move {
            let (running, found) = tokio::join!(api.server_read(&["client-api"], &[], 20), check(&client, &channel));
            (client_api(running), found)
        });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let (running, found) = task.await.unwrap_or_else(|e| (None, Err(CheckFail::Network(e.to_string()))));
            let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx| {
                this.run = Run::Idle;
                // Leitura que falha mantém a oferta guardada; o `install` relê o manifesto de qualquer jeito.
                if let Ok(found) = found { this.offer = found; }
                this.server_updated(manual, running, window, cx);
            }); });
        }).detach();
    }

    fn server_updated(&mut self, manual: bool, running: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(offer) = self.offer.clone() {
            if install_after(offer.requires_api, running) { return self.install_app(window, cx); }
            // O servidor atualizou; o app espera, calado, que o próximo ciclo refaça o plano com a API real.
            crate::log_line(&format!("update: app {} exige a API {} e o servidor está na {:?}; o app fica para o próximo ciclo",
                offer.version, offer.requires_api, running));
        }
        let version = self.server.as_ref().and_then(|s| s["versao_legivel"]["backend"].as_str()).unwrap_or("?").to_owned();
        let text = if manual { tr("update_done_manual") } else { tr("update_done").replace("{version}", &version) };
        window.push_notification(Notification::success(text).id::<Updater>(), cx);
        confirm::show_pending(false, window, cx);
        self.end_intent();
        cx.notify();
        self.refresh_server(cx);
        self.refresh_active(cx);
    }

    fn install_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A troca encerra este processo; as outras máquinas precisam terminar de ser acompanhadas antes.
        if self.defer_batch_app(cx) { return; }
        let Some(offer) = self.offer.clone() else { self.end_intent(); self.run = Run::Idle; cx.notify(); return };
        // Canal trocado depois da procura: a oferta é da branch anterior; procura de novo em vez de instalá-la.
        if offer.channel != self.channel() {
            self.end_intent();
            self.offer = None;
            self.run = Run::Idle;
            return self.check_app(cx);
        }
        self.run = Run::App;
        cx.notify();
        let task = self.runtime.spawn(install(self.client.clone(), self.exe.clone(), offer));
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| match result {
                Ok(true) => { this.end_intent(); cx.quit() }
                Ok(false) => {
                    this.end_intent();
                    crate::log_line("a release do app passou a exigir mais que a oferta aceita: nada foi trocado");
                    this.offer = None;
                    this.run = Run::Idle;
                    this.check_app(cx);
                    cx.notify();
                }
                Err(reason) => {
                    // O motivo não cabe na barra: vai no aviso e fica no tooltip do "Tentar de novo".
                    let _ = handle.update(cx, |_, window, cx| window.push_notification(Notification::error(reason.clone()).id::<Updater>(), cx));
                    this.fail(reason, cx);
                }
            });
        }).detach();
    }

    /// Rótulo curto e explicação do botão parado, conforme o plano.
    fn idle_texts(&self, plan: &Plan) -> (String, String) {
        if !plan.visible() && self.idle_visible(plan) {
            return (tr_shared("update_all_button", &[("n", &self.update_all_count().to_string())]), tr_shared("update_all_title", &[]));
        }
        let version = self.offer.as_ref().map(|o| o.version.clone()).unwrap_or_default();
        let tip = match (&plan.server, plan.app) {
            (ServerStep::Held(hold), _) if hold.stops() => hold.text(),
            (ServerStep::Update, true) => tr("app_update_all_tip").replace("{version}", &version),
            (ServerStep::Update, false) => tr("app_update_server_tip"),
            (ServerStep::Held(hold), true) => format!("{} {}", tr("app_update_available").replace("{version}", &version), hold.text()),
            _ => tr("app_update_available").replace("{version}", &version),
        };
        (tr("app_update_now"), tip)
    }
}

impl Render for Updater {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.all_running { return self.render_batch_button(cx); }
        let plan = self.plan();
        let (id, label, tip, color) = match &self.run {
            Run::Idle if !self.idle_visible(&plan) => return div().into_any_element(),
            Run::Idle => { let (label, tip) = self.idle_texts(&plan); ("topbar-update", label, tip, theme::accent()) }
            Run::Searching => ("topbar-update", tr("app_update_searching"), tr("app_update_searching"), theme::accent()),
            Run::Server { step, total, text } if *total > 0 => ("topbar-update",
                tr("app_update_server_step").replace("{step}", &step.to_string()).replace("{total}", &total.to_string()),
                tr("update_step").replace("{step}", &step.to_string()).replace("{total}", &total.to_string()).replace("{text}", text), theme::accent()),
            Run::Server { .. } => ("topbar-update", tr("update_running"), tr("update_running"), theme::accent()),
            Run::Restarting => ("topbar-update", tr("app_update_server_restarting"), tr("update_restarting"), theme::accent()),
            Run::DesktopRestart => ("topbar-update", tr("app_restarting"), tr("app_restarting"), theme::accent()),
            Run::App => ("topbar-update", tr("app_update_running"), tr("app_update_running"), theme::accent()),
            Run::Failed(reason) => ("topbar-update-retry", tr("app_update_retry"), reason.clone(), theme::danger()),
        };
        Button::new(id).ghost().small().h(px(26.)).px(px(10.)).rounded_full().border_1().border_color(color)
            .disabled(self.busy())
            .child(div().flex().items_center().gap(px(6.)).text_size(px(12.5))
                .child(Icon::new(IconName::Download).size(px(14.)).text_color(color))
                .child(label))
            .accessibility_label(tip.clone()).tooltip(tip)
            .on_click(cx.listener(|this, _, window, cx| this.run(window, cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn relaunch_spawn_failure_keeps_the_reason() {
        let missing = std::env::temp_dir().join(format!("hangar-missing-exe-{}", std::process::id()));
        let error = spawn_child(&missing, &missing.with_extension("alive")).unwrap_err();
        assert!(error.contains(&missing.display().to_string()), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn relaunch_names_why_the_child_gave_no_sign_of_life() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let signal = std::env::temp_dir().join(format!("hangar-alive-test-{}", std::process::id()));
        let mut exited = std::process::Command::new("sh").args(["-c", "exit 3"]).spawn().unwrap();
        let error = runtime.block_on(alive(&mut exited, &signal, Duration::from_secs(5))).unwrap_err();
        assert!(error.contains("saiu") && error.contains('3'), "{error}");
        let mut silent = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let error = runtime.block_on(alive(&mut silent, &signal, Duration::from_millis(300))).unwrap_err();
        assert!(error.contains("prova de vida"), "{error}");
        assert!(silent.try_wait().unwrap().is_some(), "o filho sem sinal é encerrado");
    }

    #[test]
    fn restart_and_installation_share_the_same_busy_gate() {
        assert!(!Run::Idle.busy());
        assert!(!Run::Failed("installation failed".into()).busy());
        for run in [Run::Searching, Run::Server { step: 0, total: 0, text: String::new() },
            Run::Restarting, Run::DesktopRestart, Run::App] {
            assert!(run.busy());
        }
    }

    #[test]
    fn pending_update_confirmation_cannot_overwrite_a_desktop_restart() {
        let mut run = Run::DesktopRestart;
        assert!(!run.begin_server_search());
        assert!(matches!(run, Run::DesktopRestart));
        let mut run = Run::Idle;
        assert!(run.begin_server_search());
        assert!(matches!(run, Run::Searching));
        assert!(!run.begin_server_search());
        let mut run = Run::Failed("failed".into());
        assert!(run.begin_server_search());
    }

    #[test]
    fn about_shows_test_channel_only_off_main() {
        assert_eq!(test_channel("main"), None);
        assert_eq!(test_channel(""), None);
        assert!(test_channel("feat/x").is_some_and(|text| text.contains("feat/x")));
        assert!(app_channel(None, "main").is_empty());
        assert!(app_channel(Some("main"), "main").is_empty());
        assert!(app_channel(Some(""), "main").is_empty());
        let same = app_channel(Some("feat/x"), "feat/x");
        assert_eq!(same.len(), 1);
        assert!(same[0].contains("feat/x"));
        // `a/b` e `a-b` publicam na mesma release: não é outro canal.
        assert_eq!(app_channel(Some("feat/x"), "feat-x").len(), 1);
        let back = app_channel(Some("feat/x"), "main");
        assert!(back[0].contains("feat/x") && back[1].contains("main"));
        let local = app_channel(None, "feat/y");
        assert_eq!(local.len(), 1);
        assert!(local[0].contains("feat/y"));
    }

    #[test]
    fn update_channel_draft_holds_only_its_local_server_update() {
        let local = Api::new("http://127.0.0.1:8765", "synthetic-token").unwrap();
        let mut updater = Updater { runtime: Arc::new(Runtime::new().unwrap()), client: reqwest::Client::new(), exe: None, exe_stamp: None,
            offer: Some(Offer { version: "9999.0.0.0".into(), url: String::new(), sha256: String::new(), channel: "main".into(), requires_api: 2 }),
            local: Some(local.clone()), server: Some(server(serde_json::json!({"atualizacao_disponivel": true}))), server_api: Some(2),
            server_seq: 0, active: None, active_state: None, active_api: None, active_seq: 0, run: Run::Idle, checking: false, checked: None,
            checked_channel: None, known_channel: None, channel_blocked: Some(local.identity()), suspended: false, intent_lock: None, all_servers: Vec::new(), survey: all::Survey::default(), batch: None, all_running: false };
        assert_eq!(updater.plan(), Plan { server: ServerStep::Held(Hold::ChannelDraft), app: true });
        assert!(!Hold::ChannelDraft.stops());
        updater.channel_blocked = Some("http://other-machine:8765".into());
        assert!(updater.plan().app);
        assert_eq!(updater.plan().server, ServerStep::Update);
        updater.channel_blocked = None;
        assert_eq!(updater.plan().server, ServerStep::Update);
        updater.channel_blocked = Some(local.identity());
        updater.channel_blocked = Some(local.identity());
        updater.server_api = Some(1);
        assert!(!updater.plan().app, "servidor preso pelo canal não recebe app que exige mais que ele");
        updater.server_api = Some(2);
        updater.offer = None;
        assert!(!updater.plan().app);
        assert!(!updater.plan().visible());
    }

    #[test]
    fn remote_only_uses_the_active_server_api() {
        let mut updater = Updater { runtime: Arc::new(Runtime::new().unwrap()), client: reqwest::Client::new(), exe: None, exe_stamp: None,
            offer: Some(Offer { version: "9999.0.0.0".into(), url: String::new(), sha256: String::new(), channel: "main".into(), requires_api: REQUIRED_API }),
            local: None, server: None, server_api: None, server_seq: 0, active: None, active_state: None, active_api: Some(REQUIRED_API), active_seq: 0,
            run: Run::Idle, checking: false, checked: None, checked_channel: None, known_channel: None, channel_blocked: None, suspended: false, intent_lock: None, all_servers: Vec::new(), survey: all::Survey::default(), batch: None, all_running: false };
        assert!(updater.plan().app);
        assert!(!updater.server_outdated());
        updater.active_api = Some(REQUIRED_API - 1);
        assert!(!updater.plan().app);
        assert!(updater.server_outdated());
    }

    #[test]
    fn newer_compares_number_by_number() {
        assert!(newer("0.1.0.2540", "0.1.0.2533"));
        assert!(newer("0.1.0.10", "0.1.0.9"));
        assert!(newer("0.2.0.1", "0.1.0.9999"));
        assert!(newer("0.1.0.1", "0.1.0"), "build sem git recebe a release");
        assert!(!newer("0.1.0.2533", "0.1.0.2533"));
        assert!(!newer("0.1.0.2500", "0.1.0.2533"));
        assert!(!newer("lixo", "0.1.0.1"));
        assert!(!newer("0.1.0.1", "2026.09.27-abc"));
    }

    #[test]
    fn installed_restart_requires_a_newer_mark_and_an_idle_installed_stable_app() {
        let dir = intent_dir("installed-restart");
        std::fs::create_dir_all(&dir).unwrap();
        let mark = dir.join("release.json");
        let installed = dir.join("hangar-native");
        let development = dir.join("target/debug/hangar-native");
        std::fs::write(&mark, r#"{"version":"0.1.0.110","files":{},"requires_api":1}"#).unwrap();
        assert!(installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "main", false, &mut None));
        assert!(!installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "main", true, &mut None));
        assert!(!installed_restart_needed(&mark, Some(&development), Some(&installed), APP, Some("main"), "main", false, &mut None));
        assert!(!installed_restart_needed(&mark, None, Some(&installed), APP, Some("main"), "main", false, &mut None));
        assert!(!installed_restart_needed(&mark, Some(&installed), None, APP, Some("main"), "main", false, &mut None));
        assert!(!installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("beta"), "main", false, &mut None));
        assert!(!installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "beta", false, &mut None));
        assert!(installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, None, "main", false, &mut None));
        assert!(!installed_restart_needed(&mark, Some(&installed), Some(&installed), "0.1.0.110", Some("main"), "main", false, &mut None), "o processo novo não reinicia de novo");
        // Build local mais velho que a release no caminho instalado: sem o arquivo ter mudado, nada a relançar.
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert!(!replaced_since(Some(t0), Some(t0)), "o mesmo arquivo que está rodando não relança");
        assert!(replaced_since(Some(t0), Some(t0 + Duration::from_secs(1))), "arquivo trocado relança");
        assert!(!replaced_since(None, Some(t0)) && !replaced_since(Some(t0), None), "sem data não relança");
        assert!(!installed_restart_needed(&mark, Some(&installed), Some(&installed), "0.1.0.120", Some("main"), "main", false, &mut None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installed_restart_attempts_each_version_once_but_waits_while_busy() {
        let dir = intent_dir("installed-restart-once");
        std::fs::create_dir_all(&dir).unwrap();
        let mark = dir.join("release.json");
        let installed = dir.join("hangar-native");
        std::fs::write(&mark, r#"{"version":"0.1.0.110"}"#).unwrap();
        let mut attempted = None;
        assert!(!installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "main", true, &mut attempted));
        assert_eq!(attempted, None, "ocupado apenas adia a tentativa");
        assert!(installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "main", false, &mut attempted));
        assert!(!installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "main", false, &mut attempted), "falha não relança a mesma versão indefinidamente");
        std::fs::write(&mark, r#"{"version":"0.1.0.120"}"#).unwrap();
        assert!(installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "main", false, &mut attempted));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installed_restart_ignores_missing_incomplete_or_unsupported_marks() {
        let dir = intent_dir("installed-restart-marks");
        std::fs::create_dir_all(&dir).unwrap();
        let mark = dir.join("release.json");
        let installed = dir.join("hangar-native");
        assert!(!installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "main", false, &mut None));
        for text in ["", "{", "{}", r#"{"version":110}"#, r#"{"version":"invalid"}"#,
            r#"{"sem_build":true}"#, r#"{"version":"0.1.0.110","sem_build":true}"#] {
            std::fs::write(&mark, text).unwrap();
            assert!(!installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "main", false, &mut None), "{text}");
        }
        std::fs::write(&mark, r#"{"version":"0.1.0.110"}"#).unwrap();
        assert!(installed_restart_needed(&mark, Some(&installed), Some(&installed), APP, Some("main"), "main", false, &mut None), "a próxima leitura retoma depois da gravação completa");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn release_tag_matches_the_workflow_cleanup() {
        assert_eq!(release_tag("main"), "native-latest");
        assert_eq!(release_tag("hangar-server-parte1"), "native-hangar-server-parte1");
        assert_eq!(release_tag("feat/native-test-channel"), "native-feat-native-test-channel");
        assert_eq!(release_tag("a+b_c.d"), "native-a-b_c.d");
    }

    #[test]
    fn offered_takes_a_newer_version_or_any_version_of_another_channel() {
        assert!(offered("0.1.0.110", "main", APP, Some("main")));
        assert!(!offered("0.1.0.90", "main", APP, Some("main")), "mesma branch, versão velha");
        assert!(offered("0.1.0.90", "main", APP, Some("hangar-server-parte1")), "canal desligado volta para a main");
        assert!(offered("0.1.0.90", "hangar-server-parte1", APP, Some("main")), "canal ligado troca mesmo com contagem menor");
        assert!(!offered("0.1.0.100", "hangar-server-parte1", APP, Some("hangar-server-parte1")));
        assert!(!offered("0.1.0.90", "hangar-server-parte1", APP, None), "build local: só a versão decide");
        assert!(!offered("0.1.0.90", "feat/x", APP, Some("feat-x")), "mesma release com outro nome não troca");
        assert!(!offered("lixo", "main", APP, Some("hangar-server-parte1")), "texto que não é versão nunca é oferecido");
    }

    #[test]
    fn channel_follows_the_last_state_read_from_the_local_server() {
        let mut updater = Updater { runtime: Arc::new(Runtime::new().unwrap()), client: reqwest::Client::new(), exe: None, exe_stamp: None, offer: None,
            local: None, server: None, server_api: None, server_seq: 0, active: None, active_state: None, active_api: None, active_seq: 0,
            run: Run::Idle, checking: false, checked: None, checked_channel: None, known_channel: Some("hangar-server-parte1".into()),
            channel_blocked: None, suspended: false, intent_lock: None, all_servers: Vec::new(), survey: all::Survey::default(), batch: None, all_running: false };
        assert_eq!(updater.channel(), "main", "sem servidor local não há canal");
        updater.local = Some(Api::new("http://127.0.0.1:8765", "synthetic-token").unwrap());
        assert_eq!(updater.channel(), "hangar-server-parte1", "leitura que falhou mantém o último canal");
        updater.known_channel = None;
        assert_eq!(updater.channel(), BUILT_CHANNEL.unwrap_or("main"), "antes da primeira leitura, o canal do build");
    }

    #[test]
    fn alvo_defaults_to_main_like_the_backend() {
        assert_eq!(alvo(&server(serde_json::json!({"pre_voo": {"alvo": "hangar-server-parte1"}}))), "hangar-server-parte1");
        assert_eq!(alvo(&server(serde_json::json!({}))), "main", "o server() de teste não traz alvo");
        assert_eq!(alvo(&serde_json::json!({"pre_voo": {"pode": false, "erro": "nao_e_repo"}})), "main");
        assert_eq!(alvo(&serde_json::json!({"versoes": {}})), "main", "servidor anterior ao canal");
    }

    /// O `pre_voo` do `package_body` (crates/hangar-server/src/update/http.rs): o canal do manifesto vira a branch dele.
    #[test]
    fn package_channel_picks_its_release_and_old_servers_keep_the_branch() {
        let package = |channel: &str| serde_json::json!({"modo": "pacote", "pre_voo": {"pode": true, "branch": channel, "alvo": channel,
            "branch_de_trabalho": false, "sujo": 0, "ahead": 0}});
        assert_eq!(release_tag(&alvo(&package("beta"))), "native-beta");
        assert_eq!(release_tag(&alvo(&package("main"))), "native-latest", "canal latest = main");
        let dev = serde_json::json!({"modo": "desenvolvedor", "pre_voo": {"branch": "feat-x", "alvo": "feat-x"}});
        assert_eq!(release_tag(&alvo(&dev)), "native-feat-x", "modo desenvolvedor segue a branch do checkout");
        assert_eq!(release_tag(&alvo(&serde_json::json!({"versoes": {}}))), "native-latest", "servidor sem o campo: a main, como antes");
    }

    fn server(extra: Value) -> Value {
        let mut base = serde_json::json!({
            "atualizacao_disponivel": false,
            "versoes": {"repo": "dist-latest-3-gabc", "backend": "dist-latest-3-gabc"},
            "versao_legivel": {"backend": "0.1.0.100"},
            "pre_voo": {"pode": true, "faltando": [], "branch": "main", "branch_de_trabalho": false, "sujo": 0, "ahead": 0, "divergiu": false},
            "estado": {"fase": "pronto", "ts": "t0"},
        });
        fn merge(into: &mut Value, from: Value) {
            match (into, from) {
                (Value::Object(a), Value::Object(b)) => for (k, v) in b { merge(a.entry(k).or_insert(Value::Null), v) },
                (slot, v) => *slot = v,
            }
        }
        merge(&mut base, extra);
        base
    }

    const APP: &str = "0.1.0.100";

    const SKIP_NO_APP: Plan = Plan { server: ServerStep::Skip, app: false };

    #[test]
    fn plan_without_an_offer_updates_only_a_behind_server() {
        let behind = server(serde_json::json!({"atualizacao_disponivel": true}));
        let only = plan(None, Some(&behind), Some(2));
        assert_eq!(only, Plan { server: ServerStep::Update, app: false });
        assert!(only.visible(), "servidor atrás sem app novo ainda mostra o botão");
        assert!(!plan(None, Some(&server(serde_json::json!({}))), Some(2)).visible(), "nada a fazer: botão some");
    }

    #[test]
    fn plan_a_compatible_app_goes_alone_or_after_a_behind_server() {
        let current = server(serde_json::json!({}));
        assert_eq!(plan(Some(2), Some(&current), Some(2)), Plan { server: ServerStep::Skip, app: true });
        // Contagem de versão não é mais motivo para mexer no servidor.
        let older = server(serde_json::json!({"versao_legivel": {"backend": "0.0.0.1"}}));
        assert_eq!(plan(Some(2), Some(&older), Some(2)), Plan { server: ServerStep::Skip, app: true });
        let behind = server(serde_json::json!({"atualizacao_disponivel": true}));
        assert_eq!(plan(Some(2), Some(&behind), Some(3)), Plan { server: ServerStep::Update, app: true });
    }

    #[test]
    fn plan_an_app_needing_a_published_newer_server_updates_the_server_first() {
        let behind = server(serde_json::json!({"atualizacao_disponivel": true, "api_publicado": 3}));
        assert_eq!(plan(Some(3), Some(&behind), Some(2)), Plan { server: ServerStep::Update, app: true });
        let current = server(serde_json::json!({"api_publicado": 3}));
        assert!(!plan(Some(3), Some(&current), Some(2)).visible(), "sem atualização do servidor o app não cabe");
        let held = server(serde_json::json!({"atualizacao_disponivel": true, "api_publicado": 3, "pre_voo": {"sujo": 1}}));
        assert!(!plan(Some(3), Some(&held), Some(2)).app, "servidor preso não chega à api do app");
    }

    #[test]
    fn plan_hides_an_app_whose_server_is_not_published_yet() {
        let behind = server(serde_json::json!({"atualizacao_disponivel": true, "api_publicado": 2}));
        let p = plan(Some(3), Some(&behind), Some(2));
        assert_eq!(p, SKIP_NO_APP);
        assert!(!p.visible());
    }

    #[test]
    fn plan_on_a_branch_without_a_server_release_hides_an_app_needing_more() {
        // Sem `api_publicado` o servidor publicado conta como 1.
        let behind = server(serde_json::json!({"atualizacao_disponivel": true}));
        assert_eq!(plan(Some(2), Some(&behind), Some(1)), SKIP_NO_APP);
        assert_eq!(plan(Some(2), Some(&server(serde_json::json!({"api_publicado": "x"}))), Some(1)), SKIP_NO_APP, "ilegível = 1");
    }

    #[test]
    fn plan_remote_only_offers_only_what_the_active_server_runs() {
        assert_eq!(plan(Some(2), None, Some(2)), Plan { server: ServerStep::Skip, app: true });
        assert_eq!(plan(Some(3), None, Some(2)), SKIP_NO_APP);
        assert_eq!(plan(Some(2), None, None), SKIP_NO_APP);
    }

    #[test]
    fn plan_with_an_unknown_server_api_offers_only_an_app_needing_1() {
        let behind = server(serde_json::json!({"atualizacao_disponivel": true, "api_publicado": 3}));
        assert_eq!(plan(Some(2), Some(&behind), None), Plan { server: ServerStep::Update, app: false });
        assert_eq!(plan(Some(1), Some(&behind), None), Plan { server: ServerStep::Update, app: true });
        assert_eq!(plan(Some(1), None, None), Plan { server: ServerStep::Skip, app: true });
    }

    #[test]
    fn an_offer_republished_needing_more_api_expires() {
        let offer = |sha: &str, api: u32| Offer { version: "1".into(), url: String::new(), sha256: sha.into(), channel: "main".into(), requires_api: api };
        let sha = |found: Option<Offer>| found.map(|o| o.sha256);
        assert_eq!(sha(still_offered(offer("a", 2), Ok(Some(offer("b", 2))))), Some("b".into()), "republicada na mesma API: vale a nova");
        assert_eq!(sha(still_offered(offer("a", 2), Ok(Some(offer("b", 3))))), None, "passou a exigir mais: a oferta acabou");
        assert_eq!(sha(still_offered(offer("a", 2), Err(CheckFail::Network("x".into())))), Some("a".into()), "leitura que falha");
        assert_eq!(sha(still_offered(offer("a", 2), Ok(None))), Some("a".into()));
    }

    #[test]
    fn manifest_without_requires_api_needs_api_1() {
        let old: Manifest = serde_json::from_value(serde_json::json!({"version": "0.1.0.1", "files": {}})).unwrap();
        assert_eq!(old.requires_api, 1);
        let new: Manifest = serde_json::from_value(serde_json::json!({"version": "0.1.0.1", "files": {}, "requires_api": 4})).unwrap();
        assert_eq!(new.requires_api, 4);
    }

    #[test]
    fn client_api_reads_404_as_1_and_any_other_failure_as_unknown() {
        let failed = |status: Option<u16>| Err(Failure { status, ..Failure::local("x") });
        assert_eq!(client_api(Ok(serde_json::json!({"api": 2}))), Some(2));
        assert_eq!(client_api(failed(Some(404))), Some(1), "servidor anterior à rota");
        assert_eq!(client_api(failed(Some(503))), None, "modo python");
        assert_eq!(client_api(failed(None)), None, "rede");
        assert_eq!(client_api(Ok(serde_json::json!({}))), None, "resposta sem o campo");
    }

    #[test]
    fn plan_ignores_dirty_but_not_a_pending_restart() {
        let dirty = server(serde_json::json!({"versoes": {"repo": "dist-latest-3-gabc-dirty"}}));
        assert_eq!(plan(None, Some(&dirty), Some(2)).server, ServerStep::Skip);
        let restart = server(serde_json::json!({"versoes": {"repo": "dist-latest-4-gdef"}}));
        assert_eq!(plan(None, Some(&restart), Some(2)).server, ServerStep::Update);
    }

    #[test]
    fn plan_leaves_a_work_branch_or_local_changes_alone_and_updates_the_app() {
        let branch = server(serde_json::json!({"atualizacao_disponivel": true, "pre_voo": {"branch": "feat-x", "branch_de_trabalho": true}}));
        assert_eq!(plan(Some(2), Some(&branch), Some(2)),
            Plan { server: ServerStep::Held(Hold::WorkBranch("feat-x".into(), "main".into())), app: true });
        let target = server(serde_json::json!({"atualizacao_disponivel": true, "pre_voo": {"branch": "outra", "alvo": "teste", "branch_de_trabalho": true}}));
        assert_eq!(plan(Some(2), Some(&target), Some(2)).server,
            ServerStep::Held(Hold::WorkBranch("outra".into(), "teste".into())));
        for extra in [serde_json::json!({"sujo": 2}), serde_json::json!({"ahead": 1}), serde_json::json!({"divergiu": true})] {
            let s = server(serde_json::json!({"atualizacao_disponivel": true, "pre_voo": extra}));
            assert_eq!(plan(Some(2), Some(&s), Some(2)), Plan { server: ServerStep::Held(Hold::LocalChanges), app: true });
        }
        assert!(!plan(None, Some(&branch), Some(2)).visible(), "sem app novo e servidor bloqueado: nada a fazer");
    }

    #[test]
    fn plan_stops_everything_while_missing_tools_or_already_running() {
        let missing = server(serde_json::json!({"atualizacao_disponivel": true, "pre_voo": {"pode": false, "faltando": ["npm", "uv"]}}));
        let p = plan(Some(2), Some(&missing), Some(2));
        assert_eq!(p, Plan { server: ServerStep::Held(Hold::Missing("npm, uv".into())), app: false });
        assert!(p.visible(), "mostra o motivo");
        let running = server(serde_json::json!({"atualizacao_disponivel": true, "estado": {"fase": "rodando"}}));
        assert_eq!(plan(Some(2), Some(&running), Some(2)), Plan { server: ServerStep::Held(Hold::Running), app: false });
        let reaching = server(serde_json::json!({"atualizacao_disponivel": true, "api_publicado": 3, "estado": {"fase": "rodando"}}));
        assert!(plan(Some(3), Some(&reaching), Some(2)).visible(), "o motivo aparece também para o app que exige o servidor novo");
    }

    #[test]
    fn outdated_only_when_the_active_server_api_is_below_what_the_app_requires() {
        assert!(outdated(Some(REQUIRED_API - 1)));
        assert!(!outdated(Some(REQUIRED_API)), "igual");
        assert!(!outdated(Some(REQUIRED_API + 1)), "servidor à frente é o normal");
        assert!(!outdated(None), "desconhecido não acende o aviso");
    }

    #[test]
    fn follow_reads_each_outcome() {
        let state = |v: Value| v;
        assert_eq!(follow(&state(serde_json::json!({"fase": "rodando", "passo": 4, "total": 5, "texto": "deps"})), false, Some("t0")),
            Follow::Running { step: 4, total: 5, text: "deps".into() });
        assert_eq!(follow(&state(serde_json::json!({"fase": "pronto", "ok": true, "ts": "t0"})), false, Some("t0")), Follow::NotStarted,
            "o desfecho antigo não é o desta vez");
        assert_eq!(follow(&state(serde_json::json!({"fase": "pronto", "ok": true, "ts": "t1"})), true, Some("t0")), Follow::Done(Outcome::Updated));
        assert_eq!(follow(&state(serde_json::json!({"fase": "pronto", "ok": true, "reiniciar_manual": true, "ts": "t1"})), true, Some("t0")),
            Follow::Done(Outcome::UpdatedManual));
        assert_eq!(follow(&state(serde_json::json!({"fase": "pronto", "ok": false, "erro": "npm ci falhou.", "ts": "t1"})), true, Some("t0")),
            Follow::Done(Outcome::Failed("npm ci falhou".into())));
        assert_eq!(follow(&Value::Null, true, Some("t0")), Follow::Waiting);
    }

    #[test]
    fn follow_never_gives_up_on_a_live_engine_but_keeps_two_ceilings() {
        let every = Duration::from_secs(2);
        // Leituras respondidas zeram o silêncio: 400 `rodando` seguidos (13 min) não encerram.
        let running = Follow::Running { step: 1, total: 5, text: String::new() };
        for read in 1..=400 { assert_eq!(follow_limit(Some(&running), 0, every * read), None, "leitura {read}"); }
        assert_eq!(follow_limit(None, 299, every * 299), None);
        assert_eq!(follow_limit(None, 300, every * 300), Some("update_silent"), "300 leituras sem resposta seguidas");
        let ceiling = Duration::from_secs(30 * 60);
        assert_eq!(follow_limit(Some(&running), 0, ceiling), Some("update_too_long"), "motor travado com pid vivo");
        // O desfecho lido junto com o teto vale: o servidor já atualizou.
        assert_eq!(follow_limit(Some(&Follow::Done(Outcome::Updated)), 0, ceiling), None);
        assert_eq!(follow_limit(Some(&Follow::NotStarted), 0, ceiling), None);
    }

    #[test]
    fn a_resumed_follow_without_any_server_state_drops_the_intent() {
        // O app morreu entre gravar a intenção e o `iniciar`, e o servidor nunca teve estado.
        assert_eq!(resumed_step(follow(&Value::Null, false, Some("t0"))), Follow::NotStarted);
        assert_eq!(resumed_step(Follow::Done(Outcome::Updated)), Follow::Done(Outcome::Updated));
    }

    #[test]
    fn only_another_holder_of_the_intent_lock_blocks_the_update() {
        let dir = intent_dir("lock-error");
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        // Um arquivo no lugar da pasta: criar a trava falha, e isso não é outra instância.
        std::fs::write(&dir, b"").unwrap();
        assert!(intent_lock(&dir).is_err());
        let _ = std::fs::remove_file(&dir);
    }

    fn intent_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-intent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn sample_intent() -> Intent {
        Intent { servidor: "http://127.0.0.1:8765".into(), app_version: "0.1.0.200".into(), baseline: Some("t0".into()) }
    }

    #[test]
    fn intent_is_written_resumed_and_deleted_under_one_lock() {
        let dir = intent_dir("cycle");
        let lock = intent_lock(&dir).unwrap().expect("primeira trava");
        assert!(intent_lock(&dir).unwrap().is_none(), "segunda instância não retoma");
        // Gravada antes do `iniciar`.
        intent_write(&dir, &sample_intent()).unwrap();
        let read = intent_read(&dir).expect("retomada lê a intenção");
        assert_eq!(read, sample_intent());
        let ours = serde_json::json!({"fase": "pronto", "ok": true, "ts": "t1"});
        assert_eq!(follow(&ours, false, read.baseline.as_deref()), Follow::Done(Outcome::Updated));
        assert!(install_after(2, Some(2)), "mesmo desfecho com a API suficiente: instala o app");
        intent_delete(&dir);
        assert_eq!(intent_read(&dir), None);
        drop(lock);
        assert!(intent_lock(&dir).unwrap().is_some(), "a trava sai com quem a segurava");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn intent_resumes_only_for_the_same_server_and_an_app_still_older() {
        let intent = sample_intent();
        assert!(intent_valid(&intent, "http://127.0.0.1:8765", "0.1.0.100"));
        assert!(!intent_valid(&intent, "http://127.0.0.1:9000", "0.1.0.100"), "identity diferente");
        assert!(!intent_valid(&intent, "http://127.0.0.1:8765", "0.1.0.200"), "o app novo não herda a dele");
        assert!(!intent_valid(&intent, "http://127.0.0.1:8765", "0.1.0.300"));
        // Desfecho velho (ts da intenção, sem rodando visto) não é o desta rodada.
        let old = serde_json::json!({"fase": "pronto", "ok": true, "ts": "t0"});
        assert_eq!(follow(&old, false, intent.baseline.as_deref()), Follow::NotStarted);
    }

    #[test]
    fn a_resumed_round_is_the_outcome_after_the_intent_whatever_the_pid() {
        // No Windows o lançador do venv abre um filho: o pid do estado nunca é o devolvido pelo `iniciar`.
        let intent: Intent = serde_json::from_value(serde_json::json!({"servidor": "s", "app_version": "0.1.0.200",
            "pid": 111, "baseline": "2026-10-19T10:00:00-03:00"})).unwrap();
        let after = serde_json::json!({"fase": "pronto", "ok": true, "pid": 222, "ts": "2026-10-19T10:05:00-03:00"});
        assert_eq!(follow(&after, false, intent.baseline.as_deref()), Follow::Done(Outcome::Updated));
        assert!(install_after(2, Some(2)), "releitura da API passa: instala");
        let before = serde_json::json!({"fase": "pronto", "ok": true, "pid": 222, "ts": "2026-10-19T09:59:00-03:00"});
        assert_eq!(follow(&before, false, intent.baseline.as_deref()), Follow::NotStarted, "desfecho anterior à intenção");
    }

    #[test]
    fn the_app_installs_after_the_server_only_with_the_api_it_requires() {
        assert!(install_after(2, Some(2)));
        assert!(install_after(2, Some(3)));
        assert!(!install_after(3, Some(2)), "servidor parou num binário mais velho");
        assert!(!install_after(2, None), "desconhecida não basta para quem exige mais que a 1");
        assert!(install_after(1, None), "a API 1 todo servidor atende, mesmo com a leitura falhando");
    }

    #[test]
    fn update_process_id_requires_an_accepted_positive_integer() {
        for response in [serde_json::json!({}), serde_json::json!({"ok": false, "pid": 42}),
            serde_json::json!({"ok": true, "pid": 0}), serde_json::json!({"ok": true, "pid": 1.5})] {
            assert_eq!(update_process_id(&response), None, "{response}");
        }
        assert_eq!(update_process_id(&serde_json::json!({"ok": true, "pid": 42})), Some(42));
        assert!(restart_result_matches(&serde_json::json!({"pid": 88, "request_id": "a"}), 42, Some("a")));
        assert!(!restart_result_matches(&serde_json::json!({"pid": 42, "request_id": "old"}), 42, Some("a")));
        assert!(restart_result_matches(&serde_json::json!({"pid": 42}), 42, None));
    }

    #[test]
    fn an_update_capped_at_the_last_published_build_is_a_normal_update() {
        // Servidor antigo ainda manda o campo: build pendente no CI não é falha nem aviso para quem atualiza.
        let state = serde_json::json!({"fase": "pronto", "ok": true, "ts": "t1", "waiting_for_server_build": true, "commit_para": "991bb4f123456789"});
        assert_eq!(follow(&state, true, Some("t0")), Follow::Done(Outcome::Updated));
        let manual = serde_json::json!({"fase": "pronto", "ok": true, "ts": "t1", "reiniciar_manual": true, "waiting_for_server_build": true});
        assert_eq!(follow(&manual, true, Some("t0")), Follow::Done(Outcome::UpdatedManual));
    }

    #[test]
    fn failed_swap_leaves_no_download_behind() {
        let dir = std::env::temp_dir().join(format!("hangar-swap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // O app não existe: a troca falha depois de gravar o `.new`.
        let exe = dir.join("app");
        assert!(swap(&exe, b"abc", &sha256_hex(b"abc")).is_err());
        assert!(!sibling(&exe, ".new").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Release falsa: a conexão `i` anuncia 100 bytes e entrega só `sent[i]`, como o binário trocado no meio da
    /// transferência. Atende uma conexão por item e fecha a porta.
    async fn cut_release(sent: Vec<usize>) -> (String, Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/Hangar-linux-x86_64", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            for size in sent {
                let (mut socket, _) = listener.accept().await.unwrap();
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = socket.read(&mut [0u8; 4096]).await;
                let mut reply = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n".to_vec();
                reply.extend(std::iter::repeat_n(7u8, size));
                socket.write_all(&reply).await.unwrap();
            }
        });
        (url, hits)
    }

    #[tokio::test]
    async fn download_tries_again_when_the_body_is_cut() {
        // Os 100 bytes só vêm na segunda conexão.
        let (url, _) = cut_release(vec![10, 100]).await;
        let bytes = download(&reqwest::Client::new(), &url, Duration::ZERO).await.unwrap();
        assert_eq!(bytes, vec![7u8; 100]);
    }

    #[tokio::test]
    async fn download_failure_names_the_cause_without_the_url() {
        let (url, hits) = cut_release(vec![10, 10]).await;
        let error = download(&reqwest::Client::new(), &url, Duration::ZERO).await.unwrap_err();
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(!error.contains("127.0.0.1") && !error.contains("Hangar-linux-x86_64"), "{error}");
        assert_ne!(error, tr("app_update_download_failed").replace("{reason}", ""));
    }

    #[test]
    fn sha256_matches_known_digest() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
