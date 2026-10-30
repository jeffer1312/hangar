//! Seleção explícita e identidade de origem dos plugins adotados do Claude.
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

pub type Origin = (String, String);
#[derive(Debug)]
pub struct Error {
    pub kind: &'static str,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}
impl std::error::Error for Error {}
fn error(kind: &'static str, message: impl Into<String>) -> Error {
    Error {
        kind,
        message: message.into(),
    }
}
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}
fn object(value: &Value) -> Result<&serde_json::Map<String, Value>, Error> {
    value.as_object().ok_or_else(|| {
        error(
            "AttributeError",
            format!("'{}' object has no attribute 'get'", type_name(value)),
        )
    })
}

pub fn desired(settings: &Value) -> Result<BTreeSet<String>, Error> {
    let Some(enabled) = object(settings)?.get("enabledPlugins") else {
        return Ok(BTreeSet::new());
    };
    let invalid = || {
        error(
            "ValueError",
            "enabledPlugins inválido; instalações existentes preservadas",
        )
    };
    let enabled = enabled.as_object().ok_or_else(invalid)?;
    let mut result = BTreeSet::new();
    for (id, enabled) in enabled {
        let valid_part = |part: &str| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        };
        if !id
            .split_once('@')
            .is_some_and(|(name, market)| valid_part(name) && valid_part(market))
            || !enabled.is_boolean()
        {
            return Err(invalid());
        }
        if enabled == &Value::Bool(true) {
            result.insert(id.clone());
        }
    }
    Ok(result)
}
pub fn origin(data: &Value, claude: bool, base: &Path) -> Result<Option<Origin>, Error> {
    let source = if claude {
        object(data)?.get("source").unwrap_or(&Value::Null)
    } else {
        data
    };
    let Some(source) = source.as_object() else {
        return Ok(None);
    };
    let kind = source
        .get(if claude { "source" } else { "source_type" })
        .and_then(Value::as_str)
        .unwrap_or("");
    let github;
    let location = if kind == "github" {
        let repo = match source.get("repo") {
            None => "",
            Some(Value::String(repo)) => repo,
            Some(value) => {
                return Err(error(
                    "TypeError",
                    format!(
                        "can only concatenate str (not \"{}\") to str",
                        type_name(value)
                    ),
                ));
            }
        };
        github = Value::String(format!("https://github.com/{repo}"));
        &github
    } else if claude {
        source
            .get("url")
            .filter(|value| crate::orchestration::truthy(value))
            .or_else(|| source.get("path"))
            .unwrap_or(&Value::Null)
    } else {
        source.get("source").unwrap_or(&Value::Null)
    };
    let Some(location) = location.as_str().filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    if matches!(kind, "directory" | "local") {
        let path = Path::new(location);
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            base.join(path)
        };
        let path = super::plugins::resolved(&path).map_err(|message| error("OSError", message))?;
        let path = path.to_string_lossy().into_owned();
        #[cfg(windows)]
        let path = path.replace('/', "\\").to_lowercase();
        return Ok(Some((
            "local".into(),
            path.strip_prefix("\\\\?\\").unwrap_or(&path).into(),
        )));
    }
    if !matches!(kind, "git" | "github") {
        return Ok(None);
    }
    let (host, repo) = if let Some((host, repo)) = location
        .strip_prefix("git@")
        .and_then(|value| value.split_once(':'))
    {
        (host.to_owned(), repo.to_owned())
    } else {
        url_parts(location)?
    };
    if host.is_empty() {
        return Ok(None);
    }
    let repo = repo.trim_end_matches('/');
    Ok(Some((
        "git".into(),
        format!(
            "{}/{}",
            host.to_lowercase(),
            repo.strip_suffix(".git").unwrap_or(repo)
        ),
    )))
}
pub fn identity(
    id: &str,
    expected: &Origin,
    markets: &Value,
    inventory: &Value,
    base: &Path,
) -> Result<String, Error> {
    let (name, market) = id.rsplit_once('@').ok_or_else(|| {
        error(
            "ValueError",
            "not enough values to unpack (expected 2, got 1)",
        )
    })?;
    let markets = object(markets)?;
    let inventory = object(inventory)?;
    if let Some(data) = markets.get(market) {
        if origin(data, false, base)?.as_ref() != Some(expected) {
            return Err(error("ValueError", "Marketplace homônimo de outra origem"));
        }
        if inventory.contains_key(id) {
            return Ok(id.into());
        }
    }
    let mut alias = None;
    for installed in inventory.keys() {
        let (candidate, market) = installed.rsplit_once('@').unwrap_or((installed, ""));
        if candidate != name {
            continue;
        }
        if market.is_empty() && !installed.contains('@') {
            return Err(error("IndexError", "list index out of range"));
        }
        if origin(markets.get(market).unwrap_or(&Value::Null), false, base)?.as_ref()
            == Some(expected)
        {
            if alias.is_some() {
                return Err(error(
                    "ValueError",
                    "Mais de uma identidade nativa para o plugin",
                ));
            }
            alias = Some(installed.clone());
        }
    }
    Ok(alias.unwrap_or_else(|| id.into()))
}

#[path = "plugin_selection_url.rs"]
mod url;
use url::url_parts;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(64))]
        #[test]
        fn arbitrary_plugin_identifier_and_origin_do_not_panic(value in ".{0,256}") {
            let base = tempfile::tempdir_in(crate::list::links::without_verbatim(std::env::temp_dir().canonicalize().unwrap())).unwrap();
            let _ = desired(&json!({"enabledPlugins":{value.clone():true}}));
            let _ = origin(&json!({"source_type":"git","source":value}),false,base.path());
        }
    }
}
