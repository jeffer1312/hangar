//! Troca somente caminhos temporários dos contratos; conteúdo e escapes pessoais ficam intactos.
use serde_json::Value;
use std::path::Path;

pub fn text(input: &str, root: &Path, marker: &str) -> String {
    let root = root.to_string_lossy();
    let mut result = input.to_owned();
    for form in [format!(r"\\?\{}", root), root.to_string(), root.replace('\\', "/")] {
        result = result.replace(&form.replace('\\', "\\\\"), marker).replace(&form, marker);
    }
    // No POSIX, a barra invertida pertence ao nome, não separa componentes.
    if !(root.as_bytes().get(1) == Some(&b':') || root.starts_with(r"\\")) { return result; }
    let pattern = regex::Regex::new(&format!(r#"{}(?:[\\/][^\r\n"'<>]*)?"#, regex::escape(marker))).unwrap();
    pattern.replace_all(&result, |capture: &regex::Captures| {
        capture[0].replace("\\\\", "/").replace('\\', "/")
    }).into_owned()
}
pub fn value(input: &Value, root: &Path, marker: &str) -> Value {
    hangar_server::config_sync::paths::map_strings(input, &|s| text(s, root, marker))
}

#[test]
fn temporary_paths_do_not_rewrite_other_backslashes() {
    let root = Path::new(r"C:\fixture\home");
    assert_eq!(text(r"C:\fixture\home\dir/file", root, "$ROOT"), "$ROOT/dir/file");
    assert_eq!(text(r"personal\escape", root, "$ROOT"), r"personal\escape");
    assert_eq!(text(r"/fixture/home/re\po", Path::new("/fixture/home"), "$ROOT"), r"$ROOT/re\po");
    let input = serde_json::json!({"path":r"C:\fixture\home\dir", "content":r"personal\escape"});
    assert_eq!(value(&input,root,"$ROOT"),serde_json::json!({"path":"$ROOT/dir","content":r"personal\escape"}));
}
