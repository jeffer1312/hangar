//! Resolução compartilhada pelo renderizador e pela impressão das instruções.
use crate::accounts::environment::AccountEnvironment;
use std::path::{Path, PathBuf};

pub fn imported(line: &str, base: &Path, env: &AccountEnvironment) -> Option<PathBuf> {
    let value = crate::transcript::py::strip(line).strip_prefix('@')?;
    if !value
        .chars()
        .next()
        .is_some_and(|ch| !crate::transcript::py::is_space(ch))
    {
        return None;
    }
    let path = expand_user(value, env);
    let path = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    let path = normalize(&path);
    path.is_file().then_some(path)
}

fn normalize(path: &Path) -> PathBuf {
    let result: PathBuf = path
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        if bytes.starts_with(b"//") && !bytes.starts_with(b"///") {
            let mut prefix = std::ffi::OsString::from("/");
            prefix.push(result);
            return prefix.into();
        }
    }
    result
}

#[cfg(unix)]
fn expand_user(value: &str, env: &AccountEnvironment) -> PathBuf {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    if !value.starts_with('~') {
        return value.into();
    }
    let end = value.find('/').unwrap_or(value.len());
    let name = &value[1..end];
    let home = if name.is_empty() {
        env.base
            .get("HOME")
            .map(PathBuf::from)
            .or_else(|| user_record(None).map(|(_, home)| home))
    } else {
        user_record(Some(name)).map(|(_, home)| home)
    };
    let Some(home) = home else {
        return value.into();
    };
    let bytes = home.as_os_str().as_bytes();
    let end_home = bytes
        .iter()
        .rposition(|byte| *byte != b'/')
        .map_or(0, |index| index + 1);
    let mut expanded = bytes[..end_home].to_vec();
    expanded.extend_from_slice(value[end..].as_bytes());
    if expanded.is_empty() {
        expanded.push(b'/');
    }
    std::ffi::OsString::from_vec(expanded).into()
}

#[cfg(unix)]
fn user_record(name: Option<&str>) -> Option<(std::ffi::OsString, PathBuf)> {
    use std::{
        ffi::{CStr, CString, OsStr},
        os::unix::ffi::OsStrExt,
    };
    let name = name.map(CString::new).transpose().ok()?;
    let mut size = 1024usize;
    loop {
        let mut buffer = vec![0u8; size];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // Os ponteiros do registro só vivem enquanto o buffer desta chamada está alocado.
        let code = unsafe {
            match &name {
                Some(name) => libc::getpwnam_r(
                    name.as_ptr(),
                    entry.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                ),
                None => libc::getpwuid_r(
                    libc::getuid(),
                    entry.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                ),
            }
        };
        if code == libc::ERANGE {
            size = size.checked_mul(2)?;
            continue;
        }
        if code != 0 || result.is_null() {
            return None;
        }
        if unsafe { (*result).pw_name.is_null() || (*result).pw_dir.is_null() } {
            return None;
        }
        let (name, home) = unsafe {
            (
                CStr::from_ptr((*result).pw_name),
                CStr::from_ptr((*result).pw_dir),
            )
        };
        return Some((
            OsStr::from_bytes(name.to_bytes()).to_owned(),
            PathBuf::from(OsStr::from_bytes(home.to_bytes())),
        ));
    }
}

#[cfg(windows)]
fn expand_user(value: &str, env: &AccountEnvironment) -> PathBuf {
    if !value.starts_with('~') {
        return value.into();
    }
    let end = value.find(['/', '\\']).unwrap_or(value.len());
    let mut home = match env.base.get("USERPROFILE") {
        Some(home) => home.clone(),
        None => match env.base.get("HOMEPATH") {
            Some(path) => Path::new(env.base.get("HOMEDRIVE").map(String::as_str).unwrap_or(""))
                .join(path)
                .to_string_lossy()
                .into_owned(),
            None => return value.into(),
        },
    };
    let name = &value[1..end];
    if !name.is_empty() && env.base.get("USERNAME").map(String::as_str) != Some(name) {
        let split = home.rfind(['/', '\\']).map_or(0, |index| index + 1);
        if env.base.get("USERNAME").map(String::as_str) != Some(&home[split..]) {
            return value.into();
        }
        home.truncate(split);
        home.push_str(name);
    }
    home.push_str(&value[end..]);
    home.into()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(48))]
        #[test]
        fn arbitrary_import_line_does_not_panic(line in ".{0,128}") {
            let home = tempfile::tempdir().unwrap();
            let env = AccountEnvironment::from_map([("HOME".into(),home.path().to_string_lossy().into_owned())].into());
            let _ = imported(&line,home.path(),&env);
        }
    }

    #[test]
    fn imported_instruction_paths_match_original() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../backend/tests/fixtures/contract/codex-integration/instruction-imports.json"
        ))
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let base = home.path().join("project");
        std::fs::create_dir_all(base.join("directory")).unwrap();
        for file in fixture["files"].as_array().unwrap() {
            let file = home.path().join(file.as_str().unwrap());
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "Importado\n").unwrap();
        }
        let env = AccountEnvironment::from_map(
            [("HOME".into(), home.path().to_string_lossy().into_owned())].into(),
        );
        let (account, account_home) = user_record(None).expect("conta Unix da prova ausente");
        let target = home.path().join("home.md");
        let source_parts: Vec<_> = account_home.components().collect();
        let target_parts: Vec<_> = target.components().collect();
        let common = source_parts
            .iter()
            .zip(&target_parts)
            .take_while(|(a, b)| a == b)
            .count();
        let mut relative = PathBuf::new();
        for _ in common..source_parts.len() {
            relative.push("..");
        }
        for part in &target_parts[common..] {
            relative.push(part);
        }
        let named = format!("@~{}/{}", account.to_string_lossy(), relative.display());
        let mut differences = Vec::new();
        for row in fixture["cases"].as_array().unwrap() {
            let line = row["line"]
                .as_str()
                .unwrap()
                .replace("$ROOT", home.path().to_str().unwrap())
                .replace("$NAMED_IMPORT", &named);
            let actual =
                json!(imported(&line, &base, &env).map(|path| path.to_string_lossy().into_owned()));
            let expected = row["result"].as_str().map(|value| {
                value
                    .replace("$ROOT", home.path().to_str().unwrap())
                    .replace("$ACCOUNT_HOME", account_home.to_str().unwrap())
                    .replace("$RELATIVE", relative.to_str().unwrap())
            });
            if actual != json!(expected) {
                differences.push((row["line"].clone(), actual, json!(expected)));
            }
        }
        assert!(
            differences.is_empty(),
            "imports divergentes: {differences:?}"
        );
    }
}
