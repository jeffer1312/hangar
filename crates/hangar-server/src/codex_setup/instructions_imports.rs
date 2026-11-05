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
pub(crate) fn user_record(name: Option<&str>) -> Option<(std::ffi::OsString, PathBuf)> {
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

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(48))]
        #[test]
        fn arbitrary_import_line_does_not_panic(line in ".{0,128}") {
            let home = tempfile::tempdir().unwrap();
            let env = AccountEnvironment::from_map([("HOME".into(),home.path().to_string_lossy().into_owned())].into());
            let _ = imported(&line,home.path(),&env);
        }
    }
}
