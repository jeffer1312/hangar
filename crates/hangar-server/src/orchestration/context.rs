//! O nome localiza a vida; somente a identidade durável liga o time à sessão.
use super::{kind, runs, text};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
pub static WRITE_LOCK: Mutex<()> = Mutex::new(());
#[derive(Clone, Debug)]
pub struct Context {
    pub gid: String,
    pub grouped: bool,
    pub prefix: String,
    pub path: PathBuf,
}
pub fn identity(home: &Path, name: &str, mux: &str) -> Result<String, String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err("nome de sessão inválido".into());
    }
    let claude = runs::json(
        &home
            .join(".hangar/claude-headless")
            .join(format!("{name}.json")),
    )
    .unwrap_or_default();
    let codex = runs::json(
        &home
            .join(".hangar/codex-sessions")
            .join(format!("{name}.json")),
    )
    .unwrap_or_default();
    for (p, c) in [("claude", &claude), ("codex", &codex)] {
        if !text(&c["key"]).is_empty() {
            return Ok(format!("{p}:{}", text(&c["key"])));
        }
    }
    if let Ok(out) = super::bounded(
        mux,
        &[
            "display-message",
            "-p",
            "-t",
            &format!("={name}:"),
            "#{pid}\t#{session_id}\t#{session_created}",
        ],
        None,
        Duration::from_secs(5),
    ) {
        if out.status.success() {
            let out = String::from_utf8_lossy(&out.stdout);
            let f: Vec<_> = out.trim().split('\t').collect();
            if f.len() == 3
                && f[0].chars().all(|c| c.is_ascii_digit())
                && f[1]
                    .strip_prefix('$')
                    .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
                && f[2].chars().all(|c| c.is_ascii_digit())
            {
                if let Some(start) = f[0].parse::<u32>().ok().and_then(process_start) {
                    return Ok(tmux_identity(f[0],start,f[1],f[2]));
                }
            }
        }
    }
    if !text(&codex["thread_id"]).is_empty() {
        return Ok(format!(
            "codex-thread:{}:{}",
            text(&codex["codex_home"]),
            text(&codex["thread_id"])
        ));
    }
    Err("não foi possível identificar o contexto atual da sessão".into())
}
pub fn tmux_identity(pid:&str,started:f64,session:&str,created:&str)->String {
    format!("tmux:{pid}:{}:{session}:{created}",crate::transcript::pyjson::float_repr(started))
}

fn process_start(pid: u32) -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let ticks = raw[raw.rfind(')')? + 1..]
            .split_whitespace()
            .nth(19)?
            .parse::<f64>()
            .ok()?;
        let boot = fs::read_to_string("/proc/stat")
            .ok()?
            .lines()
            .find_map(|l| l.strip_prefix("btime ").and_then(|s| s.parse::<f64>().ok()))?;
        Some(boot + ticks / unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, FILETIME},
            System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let (mut creation, mut exit, mut kernel, mut user): (
                FILETIME,
                FILETIME,
                FILETIME,
                FILETIME,
            ) = std::mem::zeroed();
            let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            CloseHandle(handle);
            if ok != 0 {
                let ticks =
                    (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
                Some(filetime_seconds(ticks))
            } else {
                None
            }
        }
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = pid;
        None
    }
}
pub fn filetime_seconds(ticks: u64) -> f64 {
    (i128::from(ticks) - 116444736000000000i128) as f64 / 10000000.
}
pub fn draft(home: &Path, name: &str, mux: &str) -> Result<String, String> {
    Ok(format!(
        "draft-{}",
        &super::sha256(&identity(home, name, mux)?)[..24]
    ))
}
pub fn mark(content: &str, tag: &str) -> Option<String> {
    let start = format!("<!-- hangar-orq-{tag}: ");
    content.lines().find_map(|l| {
        l.strip_prefix(&start)?
            .strip_suffix(" -->")
            .filter(|s| {
                runs::valid_id(s)
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            })
            .map(str::to_owned)
    })
}
pub fn link(pair_dir: &Path, name: &str) -> Option<Value> {
    let value = runs::json(&pair_dir.join(format!("{name}.json"))).ok()?;
    crate::groups::model::Sidecar::parse(name, value.as_object()?)
        .map(|sidecar| sidecar.to_json(name))
}

pub fn real_group(home: &Path, pair_dir: &Path, gid: &str, orchestration: bool) -> bool {
    let linked = fs::read_dir(pair_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|d| d.path().extension().is_some_and(|x| x == "json"))
        .any(|e| {
            link(
                pair_dir,
                &e.path().file_stem().unwrap_or_default().to_string_lossy(),
            )
            .is_some_and(|v| v["gid"] == gid && (!orchestration || v["orq"] == true))
        });
    linked || runs::group_phase(home, gid).as_deref() == Some("live")
}
pub fn ended(home: &Path, pair_dir: &Path, gid: &str) -> bool {
    let phase = runs::group_phase(home, gid);
    if matches!(phase.as_deref(), Some("live" | "unknown"))
        || real_group(home, pair_dir, gid, false)
    {
        return false;
    }
    phase.as_deref() == Some("ended")
        || fs::read_dir(
            std::env::var_os("HANGAR_PAIR_ARCHIVE")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".hangar/pair-arquivo")),
        )
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!("regras-{gid}-"))
        })
}
pub fn active_gid(home: &Path, name: &str, mux: &str) -> Result<Option<String>, String> {
    let dirs = match fs::read_dir(runs::root(home)) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let current = match identity(home, name, mux) {
        Ok(i) => i,
        Err(_) => return Ok(None),
    };
    let mut found = std::collections::BTreeSet::new();
    for d in dirs {
        let d = d.map_err(|e| e.to_string())?.path();
        let Some((cfg, gid, false)) = runs::auto_run(&d, true).map_err(|e| e.to_string())? else {
            continue;
        };
        let state = super::state::read(&d)?;
        let mut names = std::collections::HashSet::from([text(&state["arbiter"]).to_owned()]);
        for n in super::array(&state["open"]) {
            if let Some(row) = state["roles"].get(n.to_string()) {
                if let Some(o) = row.as_object() {
                    names.extend(o.values().filter_map(|v| v.as_str().map(str::to_owned)))
                }
            }
        }
        let mut ids = std::collections::HashMap::from([(
            text(&cfg["arbiter"]).to_owned(),
            cfg["arbiter_identity"].clone(),
        )]);
        for e in runs::lines(&d.join("eventos.jsonl"), true).map_err(|e| e.to_string())? {
            if kind(&e) == "task_inicio" {
                for role in ["executor", "par"] {
                    let member = text(&e[role]);
                    ids.insert(member.into(), e["session_identities"][member].clone());
                }
            } else if kind(&e) == "sessao_trocada" {
                ids.remove(text(&e["de"]));
                ids.insert(text(&e["para"]).into(), e["session_identity"].clone());
            }
        }
        if names.contains(name) && ids.get(name).is_some_and(|i| i == &current) {
            found.insert(gid);
        }
    }
    if found.len() > 1 {
        Err("a sessão está registrada em mais de uma execução viva".into())
    } else {
        Ok(found.into_iter().next())
    }
}
pub fn resolve(home: &Path, pair_dir: &Path, name: &str, mux: &str) -> Result<Context, String> {
    let prefix = crate::list::discover_other::sanitize_session_name(name);
    let prefix = if prefix.is_empty() {
        "hangar".into()
    } else {
        prefix
    };
    let linked = link(pair_dir, name).and_then(|l| l["gid"].as_str().map(str::to_owned));
    let gid = match linked {
        Some(gid) => Some(gid),
        None => active_gid(home, name, mux)?,
    };
    if let Some(gid) = gid {
        let path = pair_dir.join(format!("regras-{gid}.md"));
        let (content, _) = super::read(&path).map_err(|e| e.to_string())?;
        return Ok(Context {
            gid,
            grouped: true,
            prefix: mark(&content, "prefix").unwrap_or(prefix),
            path,
        });
    }
    let gid = draft(home, name, mux)?;
    let path = pair_dir.join(format!("regras-{gid}.md"));
    let (content, _) = super::read(&path).map_err(|e| e.to_string())?;
    if let Some(target) = mark(&content, "group") {
        let target_path = pair_dir.join(format!("regras-{target}.md"));
        let (target_text, _) = super::read(&target_path).map_err(|e| e.to_string())?;
        if !target_text.is_empty() && real_group(home, pair_dir, &target, false) {
            return Ok(Context {
                gid: target,
                grouped: true,
                prefix: mark(&target_text, "prefix").unwrap_or(prefix),
                path: target_path,
            });
        }
        if !ended(home, pair_dir, &target) {
            return Err("não foi possível confirmar o encerramento do grupo deste trabalho".into());
        }
    }
    Ok(Context {
        gid,
        grouped: false,
        prefix: mark(&content, "prefix").unwrap_or(prefix),
        path,
    })
}
pub fn associate(
    home: &Path,
    pair: &Path,
    name: &str,
    gid: &str,
    expected: f64,
    mux: &str,
) -> Result<Value, String> {
    if !real_group(home, pair, gid, true) {
        return Err("grupo de orquestração de destino não existe".into());
    }
    let current = resolve(home, pair, name, mux)?;
    if current.grouped {
        if current.gid != gid {
            return Err("o time já pertence a outro grupo".into());
        }
        let (_, mtime) = super::read(&current.path).map_err(|e| e.to_string())?;
        if (mtime - expected).abs() > 1e-6 {
            return Err("o time mudou desde a leitura — recarregue".into());
        }
        return Ok(reply(&current, mtime));
    }
    let (content, _) = super::read(&current.path).map_err(|e| e.to_string())?;
    if content.is_empty() || mark(&content, "group").is_some() {
        return Err("configure o time deste trabalho antes de associá-lo".into());
    }
    promote(home, pair, name, gid, Some(expected), mux)?;
    let next = resolve(home, pair, name, mux)?;
    let (_, mtime) = super::read(&next.path).map_err(|e| e.to_string())?;
    Ok(reply(&next, mtime))
}
pub fn write(
    home: &Path,
    pair_dir: &Path,
    context: &Context,
    content: &str,
    mtime: f64,
) -> Result<f64, String> {
    let (current, _) = super::read(&context.path).map_err(|e| e.to_string())?;
    let mut content = content.to_owned();
    if let Some(target) = mark(&current, "group") {
        if context.grouped || !ended(home, pair_dir, &target) {
            return Err("o contrato mudou desde a leitura — recarregue".into());
        }
        content = content
            .lines()
            .filter(|l| mark(l, "group").is_none())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_start_matches('\n')
            .to_owned();
    }
    if !context.grouped && mark(&content, "prefix").is_none() {
        content = format!(
            "<!-- hangar-orq-prefix: {} -->\n\n{content}",
            context.prefix
        )
    }
    super::write(&context.path, &content, Some(mtime))
}
pub fn promote(
    home: &Path,
    pair_dir: &Path,
    name: &str,
    gid: &str,
    mtime: Option<f64>,
    mux: &str,
) -> Result<(), String> {
    if !runs::valid_id(gid) || gid.starts_with("draft-") || gid == "padrao" {
        return Err("grupo de destino inválido".into());
    }
    let source = pair_dir.join(format!("regras-{}.md", draft(home, name, mux)?));
    let (content, current) = super::read(&source).map_err(|e| e.to_string())?;
    if mtime.is_some_and(|t| (t - current).abs() > 1e-6) {
        return Err("o arquivo mudou desde a leitura — recarregue".into());
    }
    if let Some(target) = mark(&content, "group") {
        return if target == gid || ended(home, pair_dir, &target) {
            Ok(())
        } else {
            Err("o time já pertence a outro grupo".into())
        };
    }
    if content.is_empty() {
        return Ok(());
    }
    let dest = pair_dir.join(format!("regras-{gid}.md"));
    promote_source(&source, &dest, &content, current, mtime.unwrap_or(current))
        .map_err(|e| e.message)
}

pub struct PromotionFailure {
    pub changed: bool,
    pub message: String,
}
/// A associação pela ponte e a promoção de um grupo usam a mesma escrita e restauração.
pub fn promote_source(
    source: &Path,
    dest: &Path,
    content: &str,
    current: f64,
    expected: f64,
) -> Result<(), PromotionFailure> {
    promote_source_with(source,dest,content,current,expected,super::write)
}

/// O escritor recebe a versão esperada; a restauração nunca substitui uma edição concorrente.
pub fn promote_source_with(
    source:&Path,dest:&Path,content:&str,current:f64,expected:f64,
    write:impl FnOnce(&Path,&str,Option<f64>)->Result<f64,String>,
)->Result<(),PromotionFailure>{
    let invalid = |message: String| PromotionFailure {
        changed: false,
        message,
    };
    let changed = |message: String| PromotionFailure {
        changed: true,
        message,
    };
    if (current - expected).abs() > 1e-6 {
        return Err(changed("o time mudou desde a leitura — recarregue".into()));
    }
    let (existing, _) = super::read(dest).map_err(|e| changed(e.to_string()))?;
    if !existing.is_empty() && existing != content {
        return Err(invalid(
            "o grupo já possui outro contrato; associe o time antes de escrevê-lo".into(),
        ));
    }
    let moved = existing.is_empty();
    if moved {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| changed(e.to_string()))?;
        }
        fs::rename(source, dest).map_err(|e| changed(e.to_string()))?;
    }
    let gid = dest.file_stem().unwrap_or_default().to_string_lossy();
    let gid = gid.strip_prefix("regras-").unwrap_or(&gid);
    if let Err(error) = write(
        source,
        &format!("<!-- hangar-orq-group: {gid} -->\n"),
        Some(if moved { 0. } else { current }),
    ) {
        let restoration = if moved {
            match fs::hard_link(dest, source) {
                Ok(()) => "origem restaurada sem substituir outro arquivo".into(),
                Err(e) => format!("origem não restaurada: {e}"),
            }
        } else {
            "não necessária".into()
        };
        return Err(invalid(format!(
            "a promoção não foi concluída ({error}); {restoration}. Confira {} e {} antes de associar novamente",
            source.display(),
            dest.display()
        )));
    }
    Ok(())
}

pub fn reply(ctx: &Context, mtime: f64) -> Value {
    json!({"ok":true,"gid":ctx.gid,"grouped":ctx.grouped,"session_prefix":ctx.prefix,"arquivo":ctx.path,"mtime":mtime})
}
