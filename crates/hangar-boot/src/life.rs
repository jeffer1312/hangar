//! Vida do servidor: a marca de "de pé" a cada subida e a de parada limpa. A subida que acha a marca
//! de pé sabe que a vida anterior não saiu limpa (queda, kill, falta de energia) e a conta.
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

pub const FILE: &str = "server-life.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Begin {
    /// A vida anterior não gravou a parada limpa.
    pub unclean: bool,
    pub unclean_exits: u64,
    /// O arquivo estava ilegível e recomeçou do zero (a contagem anterior se perdeu).
    pub reset: bool,
    /// A marca de pé é de outro servidor ainda vivo no mesmo HOME (binário de branch em teste):
    /// nada foi contado nem regravado.
    pub held_by: Option<u32>,
}

/// Na subida, antes de qualquer outra coisa: a marca de pé que ficou é a vida anterior que não saiu
/// limpa. Ela é contada e apagada aqui; esta vida só se marca de pé quando estiver pronta.
/// `alive(pid, nascimento)`: o processo da marca ainda é o mesmo, de pé.
pub fn begin(dir: &Path, now: &str, alive: impl Fn(u32, f64) -> bool) -> std::io::Result<Begin> {
    let (previous, reset) = read_or_reset(dir)?;
    let running = previous.get("running").and_then(Value::as_bool).unwrap_or(false);
    if running && let Some(pid) = holder(&previous, &alive) {
        return Ok(Begin { unclean: false, unclean_exits: count_of(&previous), reset: false, held_by: Some(pid) });
    }
    let unclean = running;
    let count = count_of(&previous) + u64::from(unclean);
    if unclean || reset {
        let last = if unclean { json!(now) } else { previous.get("last_unclean").cloned().unwrap_or(Value::Null) };
        write(dir, &json!({"running": false, "unclean_exits": count, "last_unclean": last}))?;
    }
    Ok(Begin { unclean, unclean_exits: count, reset, held_by: None })
}

/// O pid da marca, quando o mesmo processo (pid e nascimento) ainda está de pé. Marca sem
/// nascimento não prova nada: o pid pode ter sido reusado.
fn holder(life: &Value, alive: &impl Fn(u32, f64) -> bool) -> Option<u32> {
    let pid = u32::try_from(life.get("pid")?.as_u64()?).ok()?;
    let birth = life.get("birth")?.as_f64()?;
    alive(pid, birth).then_some(pid)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Marked,
    /// Outro servidor vivo segura a marca: ela fica com ele.
    Held(u32),
}

/// Pronta para atender: daqui em diante, sair sem `end` é uma saída não limpa. `birth` é o
/// nascimento deste processo, que a subida seguinte confere antes de contar a queda.
pub fn mark_running(dir: &Path, pid: u32, birth: Option<f64>, now: &str, alive: impl Fn(u32, f64) -> bool) -> std::io::Result<Mark> {
    let (mut life, _) = read_or_reset(dir)?;
    let running = life.get("running").and_then(Value::as_bool).unwrap_or(false);
    let ours = life.get("pid").and_then(Value::as_u64) == Some(u64::from(pid));
    if running && !ours && let Some(other) = holder(&life, &alive) {
        return Ok(Mark::Held(other));
    }
    let map = life.as_object_mut().expect("read_or_reset devolve objeto");
    map.insert("running".into(), json!(true));
    // A contagem fica sempre no arquivo, também antes da primeira saída não limpa.
    map.entry("unclean_exits").or_insert(json!(0));
    map.insert("pid".into(), json!(pid));
    match birth {
        Some(birth) => map.insert("birth".into(), json!(birth)),
        None => map.shift_remove("birth"),
    };
    map.insert("started".into(), json!(now));
    map.shift_remove("health");
    write(dir, &life).map(|()| Mark::Marked)
}

/// O endereço onde esta vida, já de pé, atende a saúde: o lançador do Windows o confere. `false`
/// quando a marca de pé não é deste `pid`.
pub fn mark_health(dir: &Path, pid: u32, address: SocketAddr) -> std::io::Result<bool> {
    let (mut life, _) = read_or_reset(dir)?;
    if !running_as(&life, pid) { return Ok(false); }
    life.as_object_mut().expect("read_or_reset devolve objeto").insert("health".into(), json!(address.to_string()));
    write(dir, &life).map(|()| true)
}

/// O endereço de saúde da vida `pid` de pé; `None` em qualquer outro caso, inclusive arquivo
/// ilegível e a versão que não o grava.
pub fn health(dir: &Path, pid: u32) -> Option<SocketAddr> {
    let life = read(dir).ok()?;
    if !running_as(&life, pid) { return None; }
    life.get("health")?.as_str()?.parse().ok()
}

fn running_as(life: &Value, pid: u32) -> bool {
    life.get("running").and_then(Value::as_bool) == Some(true) && life.get("pid").and_then(Value::as_u64) == Some(u64::from(pid))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Cleared,
    /// A marca é de outra vida (ou esta nunca se marcou): fica como está.
    OtherLife,
    /// O arquivo estava ilegível: regravado parado, e a contagem anterior se perdeu.
    Reset,
}

/// Parada limpa desta vida: só apaga a marca de pé se ela for deste `pid`.
pub fn end(dir: &Path, pid: u32) -> std::io::Result<Ended> {
    let (mut life, reset) = read_or_reset(dir)?;
    if reset {
        write(dir, &json!({"running": false, "unclean_exits": 0, "last_unclean": null}))?;
        return Ok(Ended::Reset);
    }
    if life.get("pid").and_then(Value::as_u64) != Some(u64::from(pid)) { return Ok(Ended::OtherLife); }
    life.as_object_mut().expect("read_or_reset devolve objeto").insert("running".into(), json!(false));
    write(dir, &life)?;
    Ok(Ended::Cleared)
}

fn read_or_reset(dir: &Path) -> std::io::Result<(Value, bool)> {
    match read(dir) {
        Ok(value) => Ok((value, false)),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => Ok((json!({}), true)),
        Err(error) => Err(error),
    }
}

pub fn unclean_exits(dir: &Path) -> std::io::Result<u64> {
    read(dir).map(|life| count_of(&life))
}

fn count_of(life: &Value) -> u64 {
    life.get("unclean_exits").and_then(Value::as_u64).unwrap_or(0)
}

fn read(dir: &Path) -> std::io::Result<Value> {
    match std::fs::read(dir.join(FILE)) {
        Ok(raw) => serde_json::from_slice::<Value>(&raw).ok().filter(Value::is_object)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "server-life.json ilegível")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error),
    }
}

fn write(dir: &Path, life: &Value) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(life.to_string().as_bytes())?;
    // Falta de energia logo depois não pode deixar o arquivo vazio: é justamente o caso que ele conta.
    file.as_file().sync_all()?;
    file.persist(dir.join(FILE)).map_err(|e| e.error)?;
    #[cfg(unix)]
    if let Ok(folder) = std::fs::File::open(dir) { let _ = folder.sync_all(); }
    Ok(())
}

/// As últimas `count` linhas ERROR/WARN do log, só com a hora e os campos `code=`/`panic=`: a
/// mensagem pode trazer texto de conversa.
pub fn last_errors(log: &Path, count: usize) -> String {
    let read = || -> std::io::Result<String> {
        let mut file = std::fs::File::open(log)?;
        let end = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(end.saturating_sub(65536)))?;
        let mut raw = Vec::new();
        file.read_to_end(&mut raw)?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    };
    let tail = match read() {
        Ok(tail) => tail,
        Err(error) => return format!("log ilegível: {:?}", error.kind()),
    };
    let kept: Vec<&str> = tail.lines().filter(|line| line.contains(" ERROR ") || line.contains(" WARN ")).collect();
    kept[kept.len().saturating_sub(count)..].iter().map(|line| reduce_error(line)).collect::<Vec<_>>().join(" | ")
}

static FIELD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s[a-z_][a-z0-9_.]*=").unwrap());
static KEPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\s((?:code|panic)=(?:"[^"]{0,160}"|\S{1,80}))"#).unwrap());

/// `2026-11-03T10:55:00.123Z  ERROR alvo: msg code=x detail="…"` vira `10:55:00 code=x`.
pub fn reduce_error(line: &str) -> String {
    let dated = line.get(..4).is_some_and(|year| year.bytes().all(|b| b.is_ascii_digit()));
    let head = match line.split_once(char::is_whitespace) {
        Some((_, rest)) if dated => rest.trim_start(),
        _ => line,
    };
    let kept = FIELD.find(head).map(|cut| KEPT.captures_iter(&head[cut.start()..])
        .map(|found| found[1].to_owned()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
    let when = if dated { line.get(11..19).map(|hour| format!("{hour} ")).unwrap_or_default() } else { String::new() };
    format!("{when}{kept}").trim_end().to_owned()
}
