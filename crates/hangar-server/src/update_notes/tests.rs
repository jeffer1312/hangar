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
    let notes = collect(&tmp.path().join("docs"), "origin/main", &applied).await.unwrap();
    assert_eq!(notes["target"], "origin/main");
    assert_eq!(notes["news"], json!([
        {"id": "2026-02-02-nova", "title": "T novo", "kind": "new", "text": "Corpo novo.", "platforms": "mobile",
         "try_it": "aperte ↑", "animation": "historico-compositor"},
        {"id": "2026-02-01-conserto", "title": "T correcao", "kind": "fix", "text": "Corpo correcao.", "platforms": "desktop",
         "try_it": null, "animation": null},
    ]));
    assert_eq!(notes["warnings"], json!([{"id": "2026-02-01-apaga", "title": "Apaga algo", "text": "Vai apagar."}]));

    std::fs::write(&applied, r#"["2026-02-01-apaga"]"#).unwrap();
    let notes = collect(tmp.path(), "origin/main", &applied).await.unwrap();
    assert_eq!(notes["warnings"], json!([]));
}

#[tokio::test]
async fn not_a_repo_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(collect(tmp.path(), "origin/main", &tmp.path().join("x.json")).await.is_err());
}

#[test]
fn update_branch_follows_env_then_dotenv_then_main() {
    assert_eq!(update_branch(Some("feat/x".into()), Some("CP_UPDATE_BRANCH=outra")), "feat/x");
    assert_eq!(update_branch(None, Some("CP_X=1\nCP_UPDATE_BRANCH=\"feat/y\"\n")), "feat/y");
    assert_eq!(update_branch(Some(" ".into()), Some("CP_UPDATE_BRANCH=")), "main");
    assert_eq!(update_branch(Some("-x".into()), None), "main");
    assert_eq!(update_branch(Some("a..b".into()), None), "main");
}
