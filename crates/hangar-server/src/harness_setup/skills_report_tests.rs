use super::*;

#[test]
fn rebuild_report_keeps_partial_success_and_reports_each_blocked_target() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/bridge-report.json")).unwrap();
    for row in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        let source = home.path().join(".claude/skills/example");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("SKILL.md"), "fixture").unwrap();
        let blocked: Vec<_> = row["blocked"].as_array().unwrap().iter().map(|v|v.as_str().unwrap()).collect();
        for (name,relative) in [("pi",".pi/agent"),("kimi",".kimi-code")] {
            let parent = home.path().join(relative);
            fs::create_dir_all(&parent).unwrap();
            if blocked.contains(&name) { fs::write(parent.join("skills-bridge"), "pessoal").unwrap(); }
        }
        let env = AccountEnvironment::from_map([
            ("HOME".into(),home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(),home.path().to_string_lossy().into_owned()),
        ].into());
        let report = rebuild_report(&env,&repo,false);
        assert_eq!(report.stats,row["stats"]);
        assert_eq!(report.errors.len(),row["failures"].as_array().unwrap().len(),"diagnóstico perdido: {blocked:?}");
        for target in blocked {
            let relative = if target == "pi" { ".pi/agent" } else { ".kimi-code" };
            let path = home.path().join(relative).join("skills-bridge");
            assert_eq!(fs::read_to_string(&path).unwrap(),"pessoal");
            let diagnostic = &report.errors[target];
            assert!(diagnostic.contains(path.to_str().unwrap()),"diagnóstico sem destino: {diagnostic}");
            assert!(diagnostic.contains("17"),"diagnóstico sem causa do sistema: {diagnostic}");
        }
    }
}
