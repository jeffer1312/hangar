use super::*;
use std::process::Command;

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn news(tipo: &str, onde: &str, extra: &str) -> String {
    format!("---\ntitulo: T {tipo}\ntipo: {tipo}\nonde: {onde}\n{extra}---\n\nCorpo {tipo}.\n")
}

/// HEAD antigo com uma novidade já instalada; `origin/main` um commit à frente com as novas.
fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git_in(dir, &["init", "-q", "-b", "main"]);
    write(dir, "docs/novidades/README.md", "formato");
    write(dir, "docs/novidades/2026-01-01-velha.md", &news("novo", "todos", ""));
    git_in(dir, &["add", "."]);
    git_in(dir, &["commit", "-qm", "base"]);
    let base = git_in(dir, &["rev-parse", "HEAD"]);
    write(dir, "docs/novidades/2026-02-01-conserto.md", &news("correcao", "desktop", "animacao: x\n"));
    write(dir, "docs/novidades/2026-02-02-nova.md", &news("novo", "celular", "experimente: aperte ↑\nanimacao: historico-compositor\n"));
    write(dir, "docs/novidades/2026-02-03-ruim.md", &news("outro", "todos", ""));
    write(dir, "docs/atualizacoes/2026-02-01-apaga.md", "---\ntitulo: Apaga algo\ndestrutivo: Sim\n---\n\nVai apagar.\n");
    write(dir, "docs/atualizacoes/2026-02-02-leve.md", "---\ntitulo: Leve\n---\n\nNada.\n");
    git_in(dir, &["add", "."]);
    git_in(dir, &["commit", "-qm", "alvo"]);
    let target = git_in(dir, &["rev-parse", "HEAD"]);
    git_in(dir, &["update-ref", "refs/remotes/origin/main", &target]);
    git_in(dir, &["reset", "-q", "--hard", &base]);
    tmp
}

#[tokio::test]
async fn news_only_from_target_and_destructive_warning_until_applied() {
    let tmp = repo();
    let applied = tmp.path().join("aplicados.json");
    let notes = collect(&tmp.path().join("docs"), "origin/main", tmp.path()).await.unwrap();
    assert_eq!(notes["target"], "origin/main");
    assert_eq!(notes["news"], json!([
        {"id": "2026-02-02-nova", "title": "T novo", "kind": "new", "text": "Corpo novo.", "platforms": "mobile",
         "try_it": "aperte ↑", "animation": "historico-compositor"},
        {"id": "2026-02-01-conserto", "title": "T correcao", "kind": "fix", "text": "Corpo correcao.", "platforms": "desktop",
         "try_it": null, "animation": null},
    ]));
    assert_eq!(notes["warnings"], json!([{"id": "2026-02-01-apaga", "title": "Apaga algo", "text": "Vai apagar."}]));

    std::fs::write(&applied, r#"["2026-02-01-apaga"]"#).unwrap();
    let notes = collect(tmp.path(), "origin/main", tmp.path()).await.unwrap();
    assert_eq!(notes["warnings"], json!([]));
}

/// Registro ilegível: não se sabe o que já rodou; erro (503), nunca todos os avisos de novo.
#[tokio::test]
async fn unreadable_registry_is_an_error() {
    let tmp = repo();
    std::fs::write(tmp.path().join("aplicados.json"), "[").unwrap();
    assert!(collect(tmp.path(), "origin/main", tmp.path()).await.is_err());
}

#[tokio::test]
async fn not_a_repo_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(collect(tmp.path(), "origin/main", tmp.path()).await.is_err());
}

/// Os mesmos casos do `scripts/test_montar_pacote.py`: o índice do pacote, montado em Python, traz as
/// notas que esta leitura daria.
#[test]
fn package_fixtures_match_the_python_builder() {
    let cases: Value = serde_json::from_str(include_str!("fixtures.json")).unwrap();
    let files = |dir: &str| {
        let mut files: Vec<(String, String)> = cases[dir].as_object().unwrap().iter()
            .map(|(name, text)| (name.clone(), text.as_str().unwrap().to_owned())).filter(|(name, _)| is_note_file(name)).collect();
        files.sort();
        files
    };
    let news = order_news(files("novidades").iter().filter_map(|(name, text)| {
        let id = name.trim_end_matches(".md");
        news_item(id, text).map(|(rank, item)| (rank, id.to_owned(), item))
    }).collect());
    assert_eq!(json!(news), cases["notes"]["news"]);
    for (os, windows) in [("linux-x86_64", false), ("windows-x86_64", true)] {
        let warnings: Vec<Value> = files("atualizacoes").iter().filter_map(|(name, text)| package_warning(name, text, windows)).map(|w| w.1).collect();
        assert_eq!(json!(warnings), cases["notes"][os], "{os}");
    }
}

#[test]
fn update_branch_follows_env_then_dotenv_then_main() {
    assert_eq!(update_branch(Some("feat/x".into()), Some("CP_UPDATE_BRANCH=outra")), "feat/x");
    assert_eq!(update_branch(None, Some("CP_X=1\nCP_UPDATE_BRANCH=\"feat/y\"\n")), "feat/y");
    assert_eq!(update_branch(Some(" ".into()), Some("CP_UPDATE_BRANCH=")), "main");
    assert_eq!(update_branch(Some("-x".into()), None), "main");
    assert_eq!(update_branch(Some("a..b".into()), None), "main");
}
