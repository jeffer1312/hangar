#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use serde_json::json;

#[test]
fn gitlab_detail_waits_for_the_pipeline_even_when_gitlab_says_mergeable() {
    const CHILD: &str = "HANGAR_REVIEWS_PIPELINE_CHILD";
    const CASES: &[(&str, &str, bool)] = &[
        ("running", "mergeable", true),
        ("pending", "mergeable", true),
        ("success", "mergeable", true),
        ("running", "ci_still_running", true),
        ("running", "conflict", true),
        ("running", "mergeable", false),
    ];

    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let known = hangar_reviews::known(&[], &[], Vec::new(), false);
            for (index, &(pipeline, status, allowed)) in CASES.iter().enumerate() {
                let detail = hangar_reviews::detail(&known, "gitlab.example.test", "team/app", index as u64 + 1).await.unwrap();
                assert_eq!(detail.summary.pipeline.as_deref(), Some(pipeline));
                assert_eq!(detail.merge.can_merge, pipeline == "success", "pipeline {pipeline}, estado {status}");
                if pipeline != "success" {
                    assert!(detail.merge.blockers.contains(&"pipeline_running"));
                    assert_eq!(detail.merge.can_auto, status != "conflict" && allowed);
                }
                if status == "conflict" { assert!(detail.merge.blockers.contains(&"conflict")); }
                if !allowed { assert!(detail.merge.blockers.contains(&"not_allowed")); }
            }
        });
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let mut script = String::from("#!/bin/sh\nset -eu\ncase \"$*\" in\n  'auth status --hostname gitlab.example.test') exit 0 ;;\n  'api --hostname gitlab.example.test user') printf '%s' '{\"username\":\"ana\"}' ;;\n");
    for (index, &(pipeline, status, allowed)) in CASES.iter().enumerate() {
        let number = index + 1;
        let mr = json!({
            "iid": number, "state": "opened", "title": "Revisão", "source_branch": "feature", "target_branch": "main",
            "project_id": 1, "source_project_id": 1, "sha": "abc", "author": {"username": "ana"},
            "detailed_merge_status": status, "user": {"can_merge": allowed},
            "head_pipeline": {"id": 1, "project_id": 1, "status": pipeline}
        });
        script.push_str(&format!("  *'projects/team%2Fapp/merge_requests/{number}?include_diverged_commits_count=true') printf '%s' '{mr}' ;;\n"));
    }
    script.push_str("  *'/approvals') printf '%s' '{\"approved_by\":[],\"approvals_left\":0}' ;;\n  *'/discussions?per_page=100'|*'/diffs?per_page=100'|*'/jobs?per_page=100'|*'/statuses?per_page=100') printf '%s' '[]' ;;\n  *) printf '%s\\n' \"chamada inesperada: $*\" >&2; exit 1 ;;\nesac\n");
    let cli = temp.path().join("glab");
    std::fs::write(&cli, script).unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();

    // O processo filho isola PATH, login e caches sem alterar o ambiente dos outros testes.
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "gitlab_detail_waits_for_the_pipeline_even_when_gitlab_says_mergeable", "--nocapture"])
        .env_clear().env(CHILD, "1").env("HOME", temp.path()).env("XDG_CONFIG_HOME", temp.path())
        .env("PATH", temp.path()).env("TZ", "UTC").env("LC_ALL", "C").current_dir(temp.path())
        .output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
