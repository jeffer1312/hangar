//! Pastas do omp resolvidas só pelo home e pelo ambiente, sem criar nada.
use regex::Regex;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

static OMP_PROFILE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9._-]{0,63}$").unwrap());
static WINDOWS_RESERVED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[0-9]|LPT[0-9])(?:\.|$)").unwrap());

/// Forma aceita pelo `omp_dirs` do Python, sem nome reservado do Windows.
pub fn valid_omp_profile(p: &str) -> bool {
    p != "." && p != ".." && !p.ends_with('.') && OMP_PROFILE_RE.is_match(p) && !WINDOWS_RESERVED.is_match(p)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmpDirectories {
    pub config_root: PathBuf,
    pub agent_dir: PathBuf,
    pub data_root: PathBuf,
}

fn var<'a>(env: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    // No Windows o ambiente não diferencia caixa, como o `os.environ` do Python.
    if cfg!(windows) {
        env.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    } else {
        env.get(name).map(String::as_str)
    }
}

fn lexical(value: &OsStr) -> Result<PathBuf, &'static str> {
    if value.as_encoded_bytes().contains(&0) {
        return Err("omp_directory_invalid");
    }
    let mut path = PathBuf::new();
    let (mut rooted, mut depth) = (false, 0usize);
    for component in Path::new(value).components() {
        match component {
            Component::Prefix(prefix) => path.push(prefix.as_os_str()),
            Component::RootDir => {
                path.push(component.as_os_str());
                rooted = true;
            }
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => {
                path.pop();
                depth -= 1;
            }
            Component::ParentDir if rooted => {}
            Component::ParentDir => path.push(".."),
            Component::Normal(part) => {
                path.push(part);
                depth += 1;
            }
        }
    }
    if path.as_os_str().is_empty() {
        path.push(".");
    }
    Ok(path)
}

/// Concatena com o separador antes de normalizar: parte absoluta não substitui a anterior.
fn join(first: &OsStr, rest: &[&str]) -> Result<PathBuf, &'static str> {
    let mut text = OsString::from(first);
    for part in rest {
        text.push(std::path::MAIN_SEPARATOR_STR);
        text.push(part);
    }
    lexical(&text)
}

fn absolute(home: &Path, value: &OsStr) -> Result<PathBuf, &'static str> {
    let path = Path::new(value);
    if cfg!(windows)
        && matches!(path.components().next(), Some(Component::Prefix(_)))
        && !path.is_absolute()
    {
        return Err("omp_directory_invalid");
    }
    lexical(home.join(path).as_os_str())
}

fn profile(value: Option<&str>) -> Result<Option<String>, &'static str> {
    let profile = value.unwrap_or("").trim();
    if profile.is_empty() || profile == "default" {
        return Ok(None);
    }
    if !valid_omp_profile(profile) {
        return Err("omp_profile_invalid");
    }
    Ok(Some(profile.into()))
}

/// Resolução lexical do omp, sem criar diretórios; só a pasta de dados do XDG é conferida no
/// disco, como no `omp_plugin_sync` do Python.
pub fn omp_directories(
    home: &Path,
    env: &BTreeMap<String, String>,
) -> Result<OmpDirectories, &'static str> {
    let name = var(env, "PI_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .unwrap_or(".omp");
    if cfg!(windows) && matches!(Path::new(name).components().next(), Some(Component::Prefix(_))) {
        return Err("omp_directory_invalid");
    }
    let base = join(home.as_os_str(), &[name])?;
    let selected = if var(env, "OMP_PROFILE").is_some() {
        var(env, "OMP_PROFILE")
    } else {
        var(env, "PI_PROFILE")
    };
    let profile = profile(selected)?;
    let config_root = match &profile {
        Some(profile) => join(base.as_os_str(), &["profiles", profile])?,
        None => base.clone(),
    };
    let default_agent = join(config_root.as_os_str(), &["agent"])?;
    let mut custom = var(env, "PI_CODING_AGENT_DIR");
    if profile.is_some() {
        custom = None;
    } else if let Ok(Some(legacy)) = self::profile(var(env, "PI_PROFILE")) {
        let legacy = join(base.as_os_str(), &["profiles", &legacy, "agent"])?;
        if custom.is_some_and(|value| OsStr::new(value) == legacy.as_os_str()) {
            custom = None;
        }
    }
    let agent_dir = match custom.filter(|value| !value.is_empty()) {
        Some(value) => absolute(home, OsStr::new(value))?,
        None => default_agent.clone(),
    };
    #[allow(unused_mut)]
    let mut data_root = config_root.clone();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if agent_dir == default_agent
        && let Some(xdg) = var(env, "XDG_DATA_HOME").filter(|v| !v.is_empty())
    {
        let mut candidate = join(OsStr::new(xdg), &["omp"])?;
        if let Some(profile) = &profile {
            candidate = join(candidate.as_os_str(), &["profiles", profile])?;
        }
        let effective = absolute(home, candidate.as_os_str())?;
        if effective.exists() {
            data_root = effective;
        }
    }
    Ok(OmpDirectories {
        config_root,
        agent_dir,
        data_root,
    })
}
