//! Coerções escalares dos modelos publicados: não estreitam inteiros para 64 bits.
use serde_json::{Value, json};

pub fn boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => match value.as_f64()? {
            0.0 => Some(false),
            1.0 => Some(true),
            _ => None,
        },
        Value::String(value) => match value.to_ascii_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
            _ => None,
        },
        _ => None,
    }
}

pub fn integer(value: &Value) -> Option<Value> {
    if let Some(b) = value.as_bool() {
        return Some(json!(u8::from(b)));
    }
    let raw = match value {
        Value::String(s) => {
            let s = s.trim();
            let bytes = s.as_bytes();
            if bytes.iter().enumerate().any(|(i, b)| {
                *b == b'_'
                    && (i == 0
                        || i + 1 == bytes.len()
                        || !bytes[i - 1].is_ascii_digit()
                        || !bytes[i + 1].is_ascii_digit())
            }) {
                return None;
            }
            s.replace('_', "")
        }
        Value::Number(n) => {
            let raw = n.to_string();
            if raw.contains(['.', 'e', 'E']) {
                let f = n.as_f64()?;
                if !f.is_finite() || f.fract() != 0.0 || f <= i64::MIN as f64 || f >= -(i64::MIN as f64) {
                    return None;
                }
                format!("{f:.0}")
            } else {
                raw
            }
        }
        _ => return None,
    };
    let (negative, raw) = if let Some(rest) = raw.strip_prefix('-') {
        (true, rest)
    } else { (false, raw.strip_prefix('+').unwrap_or(&raw)) };
    let raw = if let Some((whole, fraction)) = raw.split_once('.') {
        if fraction.is_empty() || !fraction.bytes().all(|b| b == b'0') {
            return None;
        }
        whole
    } else {
        raw
    };
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = raw.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    if value.is_string() && digits.len() + usize::from(negative) > 4300 {
        return None;
    }
    serde_json::from_str(&format!(
        "{}{digits}",
        if negative && digits != "0" { "-" } else { "" }
    ))
    .ok()
}

/// Normalização lexical do pathlib, sem expandir o usuário nem resolver `..`.
pub fn path_text(value:&str,windows:bool)->String {
    if !windows {
        let root=if value.starts_with("//")&&!value.starts_with("///"){"//"}else if value.starts_with('/') {"/"}else{""};
        let tail=value.split('/').filter(|part|!part.is_empty()&&*part!=".").collect::<Vec<_>>().join("/");
        let result=format!("{root}{tail}");return if result.is_empty(){".".into()}else{result};
    }
    let normalized=value.replace('/',"\\");
    let (drive,mut root,tail)=if normalized.starts_with("\\\\") {
        let start=if normalized.get(..8).is_some_and(|s|s.eq_ignore_ascii_case("\\\\?\\UNC\\")){8}else{2};
        match normalized[start..].find('\\').map(|i|start+i).and_then(|first|normalized[first+1..].find('\\').map(|i|first+1+i)) {
            Some(end)=>(&normalized[..end],"\\",&normalized[end+1..]),
            None=>(normalized.as_str(),"",""),
        }
    }else if normalized.starts_with('\\') {("","\\",&normalized[1..])}
    else if normalized.chars().nth(1)==Some(':') {
        let end=normalized.chars().next().unwrap().len_utf8()+1;
        if normalized[end..].starts_with('\\'){(&normalized[..end],"\\",&normalized[end+1..])}else{(&normalized[..end],"",&normalized[end..])}
    }else{("","",normalized.as_str())};
    if root.is_empty()&&drive.starts_with('\\')&&!drive.ends_with('\\') {
        let parts=drive.split('\\').collect::<Vec<_>>();
        if parts.len()==6 || parts.len()==4&&!"?.".contains(parts[2]) {root="\\";}
    }
    let parts=tail.split('\\').filter(|part|!part.is_empty()&&*part!=".").collect::<Vec<_>>();
    let relative_drive=drive.is_empty()&&root.is_empty()&&parts.first().is_some_and(|part|part.chars().nth(1)==Some(':'));
    let result=format!("{drive}{root}{}{}",if relative_drive{".\\"}else{""},parts.join("\\"));
    if result.is_empty(){".".into()}else{result}
}
pub fn path(value:&str)->std::path::PathBuf { path_text(value,cfg!(windows)).into() }
