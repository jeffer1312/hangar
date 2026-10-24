use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use hangar_package::{RENAME_TRIES, RENAME_WAIT, extract, extract_with, rename_with_retry};

const SHA_HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const SHA_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const SHA_TOOL: &str = "7c9bbe5ec9b3fb774e8fa0f54247e93c34ddf8e5d16fe3073420de0ae81a262d";

fn manifest(files: &[(&str, &str)]) -> String {
    let files: serde_json::Map<String, serde_json::Value> =
        files.iter().map(|(p, s)| (p.to_string(), serde_json::Value::from(*s))).collect();
    serde_json::json!({
        "version": "0.1.0.5", "commit": "abc", "channel": "latest", "protocol": 1, "api": 1,
        "os": "linux-x86_64", "dist_key": null, "files": files,
    })
    .to_string()
}

enum Entry<'a> {
    File(&'a str, &'a [u8]),
    /// Nome gravado cru no cabeçalho, sem a checagem do `tar::Builder`.
    RawName(&'a str, &'a [u8]),
    Symlink(&'a str, &'a str),
}

fn archive(dir: &Path, entries: &[Entry]) -> PathBuf {
    let path = dir.join("pkg.tar.gz");
    let mut b = tar::Builder::new(GzEncoder::new(fs::File::create(&path).unwrap(), Compression::default()));
    for e in entries {
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o755);
        match e {
            Entry::File(name, data) => {
                h.set_size(data.len() as u64);
                h.set_cksum();
                b.append_data(&mut h, name, *data).unwrap();
            }
            Entry::RawName(name, data) => {
                h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
                h.set_size(data.len() as u64);
                h.set_cksum();
                b.append(&h, *data).unwrap();
            }
            Entry::Symlink(name, target) => {
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_size(0);
                b.append_link(&mut h, name, target).unwrap();
            }
        }
    }
    b.into_inner().unwrap().finish().unwrap();
    path
}

fn good_manifest() -> String {
    manifest(&[("bin/hangar-server", SHA_HELLO), ("docs/empty.txt", SHA_EMPTY)])
}

fn assert_no_version_dirs(versions: &Path) {
    assert!(!versions.join("0.1.0.5").exists(), "pasta final não pode nascer");
    assert!(!versions.join("0.1.0.5.tmp").exists(), ".tmp tem que sumir");
}

#[test]
fn extract_happy_path() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let m = good_manifest();
    let a = archive(
        t.path(),
        &[
            Entry::File("manifest.json", m.as_bytes()),
            Entry::File("bin/hangar-server", b"hello"),
            Entry::File("./docs/empty.txt", b""),
        ],
    );
    let dir = extract(&a, &versions).expect("extração válida");
    assert_eq!(dir, versions.join("0.1.0.5"));
    assert_eq!(fs::read(dir.join("bin/hangar-server")).unwrap(), b"hello");
    assert_eq!(fs::read(dir.join("docs/empty.txt")).unwrap(), b"");
    assert!(dir.join("manifest.json").is_file());
    assert!(!versions.join("0.1.0.5.tmp").exists());
}

#[test]
fn sha_mismatch_leaves_nothing() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let m = good_manifest();
    let a = archive(
        t.path(),
        &[
            Entry::File("manifest.json", m.as_bytes()),
            Entry::File("bin/hangar-server", b"hellx"),
            Entry::File("docs/empty.txt", b""),
        ],
    );
    let err = extract(&a, &versions).unwrap_err();
    assert_eq!(err.code(), "package_sha_mismatch");
    assert_no_version_dirs(&versions);
}

#[test]
fn missing_or_extra_file_fails() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let m = good_manifest();
    let missing = archive(t.path(), &[Entry::File("manifest.json", m.as_bytes()), Entry::File("bin/hangar-server", b"hello")]);
    assert_eq!(extract(&missing, &versions).unwrap_err().code(), "package_sha_mismatch");
    assert_no_version_dirs(&versions);

    let extra = archive(
        t.path(),
        &[
            Entry::File("manifest.json", m.as_bytes()),
            Entry::File("bin/hangar-server", b"hello"),
            Entry::File("docs/empty.txt", b""),
            Entry::File("bin/tool", b"tool"),
        ],
    );
    assert_eq!(extract(&extra, &versions).unwrap_err().code(), "package_sha_mismatch");
    assert_no_version_dirs(&versions);
}

#[test]
fn missing_manifest_fails() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let a = archive(t.path(), &[Entry::File("bin/tool", b"tool")]);
    assert_eq!(extract(&a, &versions).unwrap_err().code(), "package_manifest_invalid");
}

#[test]
fn parent_dir_entry_rejected() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let m = manifest(&[("bin/tool", SHA_TOOL)]);
    let a = archive(
        t.path(),
        &[
            Entry::File("manifest.json", m.as_bytes()),
            Entry::RawName("bin/../../escaped", b"tool"),
            Entry::File("bin/tool", b"tool"),
        ],
    );
    assert_eq!(extract(&a, &versions).unwrap_err().code(), "package_unsafe_path");
    assert_no_version_dirs(&versions);
    assert!(!versions.join("escaped").exists());
    assert!(!t.path().join("escaped").exists());
}

#[test]
fn absolute_entry_and_symlink_rejected() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let m = manifest(&[("bin/tool", SHA_TOOL)]);
    let abs = archive(t.path(), &[Entry::File("manifest.json", m.as_bytes()), Entry::RawName("/tmp/abs-escape", b"tool")]);
    assert_eq!(extract(&abs, &versions).unwrap_err().code(), "package_unsafe_path");
    assert_no_version_dirs(&versions);

    let link = archive(
        t.path(),
        &[Entry::File("manifest.json", m.as_bytes()), Entry::Symlink("bin/tool", "/etc/passwd")],
    );
    assert_eq!(extract(&link, &versions).unwrap_err().code(), "package_unsafe_path");
    assert_no_version_dirs(&versions);
}

#[test]
fn existing_version_is_reused_only_when_intact() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let m = good_manifest();
    let a = archive(
        t.path(),
        &[
            Entry::File("manifest.json", m.as_bytes()),
            Entry::File("bin/hangar-server", b"hello"),
            Entry::File("docs/empty.txt", b""),
        ],
    );
    let dir = extract(&a, &versions).unwrap();
    let mut renames = 0;
    let reused = extract_with(
        &a,
        &versions,
        &mut |_, _| {
            renames += 1;
            Ok(())
        },
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(reused, dir);
    assert_eq!(renames, 0, "versão intacta não é extraída de novo");
    assert!(!versions.join("0.1.0.5.tmp").exists());

    fs::write(dir.join("bin/hangar-server"), b"changed").unwrap();
    assert_eq!(extract(&a, &versions).unwrap_err().code(), "package_existing_differs");
    assert_eq!(fs::read(dir.join("bin/hangar-server")).unwrap(), b"changed", "versão existente não é reescrita");
}

fn good_archive(dir: &Path) -> PathBuf {
    let m = good_manifest();
    archive(
        dir,
        &[
            Entry::File("manifest.json", m.as_bytes()),
            Entry::File("bin/hangar-server", b"hello"),
            Entry::File("docs/empty.txt", b""),
        ],
    )
}

#[test]
fn existing_version_without_manifest_differs() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let a = good_archive(t.path());
    let dir = extract(&a, &versions).unwrap();
    fs::remove_file(dir.join("manifest.json")).unwrap();
    assert_eq!(extract(&a, &versions).unwrap_err().code(), "package_existing_differs");
}

#[cfg(unix)]
#[test]
fn existing_version_that_is_a_symlink_differs() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let a = good_archive(t.path());
    let dir = extract(&a, &versions).unwrap();
    // O alvo do link é uma versão intacta: seguir o link faria a conferência passar.
    let elsewhere = t.path().join("elsewhere");
    fs::rename(&dir, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
    assert_eq!(extract(&a, &versions).unwrap_err().code(), "package_existing_differs");
}

#[test]
fn oversized_manifest_rejected() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let big = format!("{}{}", good_manifest(), " ".repeat(4 << 20));
    let a = archive(
        t.path(),
        &[
            Entry::File("manifest.json", big.as_bytes()),
            Entry::File("bin/hangar-server", b"hello"),
            Entry::File("docs/empty.txt", b""),
        ],
    );
    assert_eq!(extract(&a, &versions).unwrap_err().code(), "package_manifest_invalid");
    assert_no_version_dirs(&versions);
}

// O pacote de Windows, com os .pyc, chega perto de 1 MiB de manifesto.
#[test]
fn two_mib_manifest_accepted() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let big = format!("{}{}", good_manifest(), " ".repeat(2 << 20));
    let a = archive(
        t.path(),
        &[
            Entry::File("manifest.json", big.as_bytes()),
            Entry::File("bin/hangar-server", b"hello"),
            Entry::File("docs/empty.txt", b""),
        ],
    );
    extract(&a, &versions).unwrap();
}

#[test]
fn failed_rename_leaves_nothing() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let a = good_archive(t.path());
    let err = extract_with(&a, &versions, &mut |_, _| Err(io::Error::other("injected")), &mut |_| {}).unwrap_err();
    assert_eq!(err.code(), "package_rename_failed");
    assert_no_version_dirs(&versions);
}

#[test]
fn directory_where_manifest_expects_file_fails() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let m = manifest(&[("bin", SHA_TOOL), ("bin/tool", SHA_TOOL)]);
    let a = archive(t.path(), &[Entry::File("manifest.json", m.as_bytes()), Entry::File("bin/tool", b"tool")]);
    assert_eq!(extract(&a, &versions).unwrap_err().code(), "package_sha_mismatch");
    assert_no_version_dirs(&versions);
}

#[test]
fn two_different_manifests_rejected() {
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let first = good_manifest();
    let second = first.replace("\"commit\":\"abc\"", "\"commit\":\"def\"");
    assert_ne!(first, second);
    let a = archive(
        t.path(),
        &[
            Entry::File("manifest.json", first.as_bytes()),
            Entry::File("bin/hangar-server", b"hello"),
            Entry::File("docs/empty.txt", b""),
            Entry::File("manifest.json", second.as_bytes()),
        ],
    );
    assert_eq!(extract(&a, &versions).unwrap_err().code(), "package_manifest_invalid");
    assert_no_version_dirs(&versions);
}

#[cfg(unix)]
#[test]
fn permission_denied_is_permanent_on_unix() {
    let mut calls = 0;
    let mut waits = 0;
    let err = rename_with_retry(
        Path::new("a"),
        Path::new("b"),
        &mut |_, _| {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        },
        &mut |_| waits += 1,
    )
    .unwrap_err();
    assert_eq!(err.code(), "package_rename_failed");
    assert_eq!(calls, 1);
    assert_eq!(waits, 0);
}

#[cfg(windows)]
#[test]
fn rename_retries_then_succeeds() {
    let mut calls = 0;
    let mut waits = Vec::new();
    let res = rename_with_retry(
        Path::new("a"),
        Path::new("b"),
        &mut |_, _| {
            calls += 1;
            // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
            match calls {
                1 => Err(io::Error::from_raw_os_error(5)),
                2 => Err(io::Error::from_raw_os_error(32)),
                3 => Err(io::Error::from_raw_os_error(33)),
                _ => Ok(()),
            }
        },
        &mut |d| waits.push(d),
    );
    assert!(res.is_ok());
    assert_eq!(calls, 4);
    assert_eq!(waits, vec![Duration::from_millis(500); 3]);
}

#[cfg(windows)]
#[test]
fn stale_tmp_removal_retries_on_windows() {
    use std::os::windows::fs::OpenOptionsExt;
    let t = tempfile::tempdir().unwrap();
    let versions = t.path().join("versions");
    let a = good_archive(t.path());
    let stale = versions.join("0.1.0.5.tmp");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("held.txt"), b"x").unwrap();
    // Sem compartilhamento: apagar falha com ERROR_SHARING_VIOLATION até a espera soltar o arquivo.
    let mut held = Some(fs::OpenOptions::new().read(true).share_mode(0).open(stale.join("held.txt")).unwrap());
    let mut waits = 0;
    let dir = extract_with(&a, &versions, &mut |a, b| fs::rename(a, b), &mut |_| {
        waits += 1;
        held.take();
    })
    .unwrap();
    assert!(waits >= 1);
    assert_eq!(fs::read(dir.join("bin/hangar-server")).unwrap(), b"hello");
}

#[test]
fn rename_limits() {
    assert_eq!(RENAME_TRIES, 20);
    assert_eq!(RENAME_WAIT, Duration::from_millis(500));
}

#[cfg(windows)]
#[test]
fn rename_gives_up_after_twenty_tries() {
    let mut calls = 0;
    let mut waits = 0;
    let err = rename_with_retry(
        Path::new("a"),
        Path::new("b"),
        &mut |_, _| {
            calls += 1;
            Err(io::Error::from_raw_os_error(32))
        },
        &mut |_| waits += 1,
    )
    .unwrap_err();
    assert_eq!(err.code(), "package_rename_failed");
    assert_eq!(calls, 20);
    assert_eq!(waits, 19);
}

#[test]
fn rename_does_not_retry_other_errors() {
    let mut calls = 0;
    let err = rename_with_retry(
        Path::new("a"),
        Path::new("b"),
        &mut |_, _| {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::NotFound))
        },
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code(), "package_rename_failed");
    assert_eq!(calls, 1);
}
