import json

import pytest

from app import git_ops, worktrees
from app.git_ops import GitError
from app.models import SessionInfo


def _repo(path):
    path.mkdir(parents=True, exist_ok=True)
    d = str(path)
    for args in (["init", "-q", "-b", "main"], ["config", "user.email", "t@t"],
                 ["config", "user.name", "t"], ["commit", "-q", "--allow-empty", "-m", "init"]):
        git_ops._run(d, *args)
    return d


def _wt(main, path, branch):
    assert git_ops._run(main, "worktree", "add", "-b", branch, str(path)).returncode == 0
    return str(path)


@pytest.fixture(autouse=True)
def _isolated_removed(tmp_path, monkeypatch):
    monkeypatch.setattr(worktrees, "REMOVED_FILE", tmp_path / "removidas.json")


def test_roots_and_worktree_list(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    (tmp_path / "repo-x" / "sub").mkdir()
    assert worktrees.repo_root_of(str(tmp_path / "repo-x" / "sub")) == wt
    assert worktrees.main_repo_of(wt) == main
    assert worktrees.main_repo_of(main) == main
    assert worktrees.worktree_paths(main) == [wt]


def test_claude_cwd_reads_last_line_with_cwd(tmp_path):
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text("\n".join([json.dumps({"cwd": "/a"}), json.dumps({"cwd": "/b"}),
                            json.dumps({"type": "summary"})]) + "\n")
    assert worktrees.claude_cwd(str(f)) == "/b"


def test_claude_cwd_without_cwd_is_none(tmp_path):
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text(json.dumps({"type": "summary"}) + "\n")
    assert worktrees.claude_cwd(str(f)) is None


def test_locate_claude_moved_into_worktree(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    (tmp_path / "repo-x" / "sub").mkdir()
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text(json.dumps({"cwd": main}) + "\n" + json.dumps({"cwd": wt + "/sub"}) + "\n")
    loc = worktrees.locate("claude", main, str(f))
    assert (loc.branch, loc.worktree, loc.worktree_path, loc.worktree_gone) == ("x", True, wt, False)


def test_locate_claude_worktree_gone(tmp_path):
    main = _repo(tmp_path / "repo")
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text(json.dumps({"cwd": str(tmp_path / "repo-sumiu")}) + "\n")
    loc = worktrees.locate("claude", main, str(f))
    assert loc.worktree_gone and loc.worktree_path == str(tmp_path / "repo-sumiu")


def _claude_line(cwd, *tools):
    """Linha de assistente do Claude com as chamadas `(nome, input)` dadas."""
    content = [{"type": "tool_use", "name": n, "input": i} for n, i in tools]
    return json.dumps({"type": "assistant", "cwd": cwd, "message": {"content": content}})


def test_locate_claude_sibling_worktree_by_cd_without_cwd_change(tmp_path):
    # O Claude Code devolve o shell à pasta de abertura: o `cwd` do transcript nunca sai da principal.
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text(_claude_line(main, ("Bash", {"command": f"cd {wt} && git status"})) + "\n")
    loc = worktrees.locate("claude", main, str(f))
    assert (loc.branch, loc.worktree_path, loc.git_cwd) == ("x", wt, wt)


def test_locate_claude_edit_in_worktree_and_main_note_keeps_worktree(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    (tmp_path / "repo-x" / "a.py").write_text("")
    (tmp_path / "repo" / "nota.md").write_text("")
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text("\n".join([
        _claude_line(main, ("Edit", {"file_path": wt + "/a.py"})),
        _claude_line(main, ("Write", {"file_path": main + "/nota.md"})),
    ]) + "\n")
    loc = worktrees.locate("claude", main, str(f))
    assert loc.git_cwd == wt and loc.branch == "x"


def test_locate_claude_cd_to_main_to_look_keeps_the_worktree(tmp_path):
    # Consultar a principal é rotina: se o `cd` para ela contasse, o rótulo alternaria a cada comando.
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text("\n".join([
        _claude_line(main, ("Bash", {"command": f"git -C {wt} log -1"})),
        _claude_line(main, ("Bash", {"command": f"cd {wt}; ls; cd {main} && git status"})),
    ]) + "\n")
    loc = worktrees.locate("claude", main, str(f))
    assert (loc.branch, loc.worktree_path, loc.git_cwd) == ("x", wt, wt)


@pytest.mark.parametrize("provider", ["claude", "codex"])
def test_locate_session_born_in_worktree_stays_there(tmp_path, provider):
    # A criação com "Nova worktree" abre a sessão já dentro dela: os sinais do transcript não a tiram.
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    wy = _wt(main, tmp_path / "repo-y", "y")
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    if provider == "claude":
        f.write_text(_claude_line(wt, ("Bash", {"command": f"cd {wy} && ls"})) + "\n")
    else:
        call = {"type": "response_item", "payload": {"type": "function_call", "name": "exec_command",
                "arguments": json.dumps({"cmd": f"cd {main} && ls"})}}
        f.write_text(json.dumps(call) + "\n")
    loc = worktrees.locate(provider, wt, str(f))
    assert (loc.branch, loc.worktree_path, loc.git_cwd) == ("x", wt, None)


def test_locate_claude_cd_into_another_worktree_moves(tmp_path):
    main = _repo(tmp_path / "repo")
    _wt(main, tmp_path / "repo-x", "x")
    wy = _wt(main, tmp_path / "repo-y", "y")
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text("\n".join([
        _claude_line(main, ("Bash", {"command": f"cd {tmp_path / 'repo-x'} && ls"})),
        _claude_line(main, ("Bash", {"command": f"git -C {wy} status"})),
    ]) + "\n")
    assert worktrees.locate("claude", main, str(f)).git_cwd == wy


def test_locate_claude_calls_before_the_last_cwd_change_do_not_count(tmp_path):
    # ExitWorktree: o `cwd` voltou à principal; o `cd` de antes era da worktree.
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text("\n".join([
        _claude_line(wt, ("Bash", {"command": f"cd {wt} && ls"})),
        json.dumps({"type": "user", "cwd": main}),
    ]) + "\n")
    loc = worktrees.locate("claude", main, str(f))
    assert (loc.branch, loc.git_cwd) == ("main", None)


def test_locate_claude_relative_missing_cd_is_not_a_removed_worktree(tmp_path):
    main = _repo(tmp_path / "repo")
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text(_claude_line(main, ("Bash", {"command": "W=/x; cd $W && cd - && cd build"})) + "\n")
    loc = worktrees.locate("claude", main, str(f))
    assert (loc.branch, loc.worktree_gone, loc.git_cwd) == ("main", False, None)


def test_locate_claude_removed_sibling_worktree_is_gone(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    assert git_ops._run(main, "worktree", "remove", wt).returncode == 0
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text(_claude_line(main, ("Bash", {"command": f"cd {wt} && ls"})) + "\n")
    loc = worktrees.locate("claude", main, str(f))
    assert loc.worktree_gone and loc.worktree_path == wt and loc.git_cwd is None


def test_locate_codex_uses_last_workdir_of_same_repo(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    other = _repo(tmp_path / "outro")
    f = tmp_path / "rollout.jsonl"
    calls = [
        {"type": "response_item", "payload": {"type": "custom_tool_call", "name": "exec",
         "input": f'tools.exec_command({{cmd:"git status", workdir:"{wt}"}})'}},
        {"type": "response_item", "payload": {"type": "function_call", "name": "exec_command",
         "arguments": json.dumps({"cmd": f"cd {other} && ls"})}},
    ]
    f.write_text("\n".join(json.dumps(c) for c in calls) + "\n")
    loc = worktrees.locate("codex", main, str(f))
    assert loc.worktree_path == wt and loc.branch == "x"


def test_locate_without_signal_uses_opening_folder(tmp_path):
    main = _repo(tmp_path / "repo")
    loc = worktrees.locate("codex", main, None)
    assert (loc.branch, loc.worktree, loc.worktree_path) == ("main", False, None)


def test_locate_codex_cd_wins_over_workdir_in_same_call(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    f = tmp_path / "rollout.jsonl"
    call = {"type": "response_item", "payload": {"type": "function_call", "name": "exec_command",
            "arguments": json.dumps({"cmd": f"cd {wt} && ls", "workdir": main})}}
    f.write_text(json.dumps(call) + "\n")
    loc = worktrees.locate("codex", main, str(f))
    assert loc.worktree_path == wt and loc.branch == "x"


def test_locate_codex_removed_worktree_is_gone(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    assert git_ops._run(main, "worktree", "remove", wt).returncode == 0
    f = tmp_path / "rollout.jsonl"
    call = {"type": "response_item", "payload": {"type": "custom_tool_call", "name": "exec",
            "input": f'tools.exec_command({{cmd:"ls", workdir:"{wt}"}})'}}
    f.write_text(json.dumps(call) + "\n")
    loc = worktrees.locate("codex", main, str(f))
    assert loc.worktree_gone and loc.worktree_path == wt


def test_locate_codex_missing_folder_of_other_repo_does_not_count(tmp_path):
    main = _repo(tmp_path / "repo")
    f = tmp_path / "rollout.jsonl"
    call = {"type": "response_item", "payload": {"type": "function_call", "name": "exec_command",
            "arguments": json.dumps({"cmd": "ls", "workdir": str(tmp_path / "outro" / "sumiu")})}}
    f.write_text(json.dumps(call) + "\n")
    loc = worktrees.locate("codex", main, str(f))
    assert (loc.branch, loc.worktree, loc.worktree_path, loc.worktree_gone) == ("main", False, None, False)


def test_locate_codex_deleted_patch_file_does_not_count(tmp_path):
    main = _repo(tmp_path / "repo")
    f = tmp_path / "rollout.jsonl"
    call = {"type": "response_item", "payload": {"type": "custom_tool_call", "name": "apply_patch",
            "input": f"*** Begin Patch\n*** Delete File: {tmp_path / 'outro' / 'a.txt'}\n*** End Patch"}}
    f.write_text(json.dumps(call) + "\n")
    loc = worktrees.locate("codex", main, str(f))
    assert (loc.branch, loc.worktree, loc.worktree_path, loc.worktree_gone) == ("main", False, None, False)


def test_worktree_paths_with_relative_pointers(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    # Ponteiros relativos, como grava `worktree.useRelativePaths`.
    (tmp_path / "repo-x" / ".git").write_text("gitdir: ../repo/.git/worktrees/repo-x\n")
    (tmp_path / "repo" / ".git" / "worktrees" / "repo-x" / "gitdir").write_text("../../../../repo-x/.git\n")
    assert worktrees.main_repo_of(wt) == main
    assert worktrees.worktree_paths(main) == [wt]


@pytest.mark.parametrize("lines", [
    ['["function_call", "cwd"]'],
    ["42"],
    [json.dumps({"type": "response_item", "payload": ["function_call"]})],
    [json.dumps({"type": "response_item", "payload": "custom_tool_call"})],
    ['{"type": "response_item", "payload": {"type": "function_call", "arguments": "{\\"cmd\\": \\"cd /'],
])
def test_locate_survives_malformed_lines(tmp_path, lines):
    main = _repo(tmp_path / "repo")
    f = tmp_path / "d0000009-0000-4000-8000-000000000000.jsonl"
    f.write_text("\n".join(lines))
    for provider in ("codex", "claude"):
        loc = worktrees.locate(provider, main, str(f))
        assert (loc.branch, loc.worktree, loc.worktree_path, loc.worktree_gone) == ("main", False, None, False)


def test_locate_missing_transcript_uses_opening_folder(tmp_path):
    main = _repo(tmp_path / "repo")
    for provider in ("codex", "claude"):
        loc = worktrees.locate(provider, main, str(tmp_path / "d0000007-0000-4000-8000-000000000000.jsonl"))
        assert (loc.branch, loc.worktree_path, loc.worktree_gone) == ("main", None, False)


def test_locate_never_raises_on_reader_failure(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    f = tmp_path / "d0000008-0000-4000-8000-000000000000.jsonl"
    f.write_text("{}\n")

    def boom(*_):
        raise RuntimeError("transcript torto")
    monkeypatch.setattr(worktrees, "claude_cwd", boom)
    loc = worktrees.locate("claude", main, str(f))
    assert (loc.branch, loc.worktree_path, loc.worktree_gone) == ("main", None, False)


def _commit(path, name):
    (path / name).write_text(name)
    git_ops._run(str(path), "add", name)
    git_ops._run(str(path), "commit", "-q", "-m", name)


def test_status_ahead_dirty_and_ignored(tmp_path):
    main = _repo(tmp_path / "repo")
    (tmp_path / "repo" / ".gitignore").write_text(".env\nnotas.txt\nnode_modules/\n")
    git_ops._run(main, "add", ".gitignore")
    git_ops._run(main, "commit", "-q", "-m", "ignore")
    (tmp_path / "repo" / ".env").write_text("S=1")
    wt = _wt(main, tmp_path / "repo-x", "x")
    git_ops._run(wt, "config", "branch.x.hangar-base", "main")
    _commit(tmp_path / "repo-x", "a.txt")
    (tmp_path / "repo-x" / "solto.txt").write_text("?")
    (tmp_path / "repo-x" / ".env").write_text("S=1")        # cópia idêntica: não se perde nada
    (tmp_path / "repo-x" / "notas.txt").write_text("minha")  # só existe aqui
    (tmp_path / "repo-x" / "node_modules").mkdir()
    st = worktrees.status(wt, [SessionInfo(name="s1", cwd=main, worktree_path=wt)])
    assert st["branch"] == "x" and st["base"] == "main" and st["ahead"] == 1
    assert st["merged"] is False and st["dirty"] == 1
    assert st["ignored"] == ["notas.txt"]
    assert st["sessions"] == ["s1"] and st["exists"] is True and st["repo"] == main


def test_status_main_branch_is_the_main_folder_branch(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    git_ops._run(wt, "config", "branch.x.hangar-base", "main")
    git_ops._run(main, "switch", "-q", "-c", "dev")
    st = worktrees.status(wt)
    assert st["base"] == "main" and st["main_branch"] == "dev"


def test_status_merged_by_ancestor(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    _commit(tmp_path / "repo-x", "a.txt")
    git_ops._run(main, "merge", "-q", "--no-ff", "-m", "m", "x")
    assert worktrees.status(wt)["merged"] is True


def test_deleted_upstream_does_not_prove_merge(tmp_path):
    remote = _repo(tmp_path / "remote")
    git_ops._run(remote, "branch", "x")
    main = str(tmp_path / "clone")
    git_ops._run(str(tmp_path), "clone", "-q", remote, main)
    git_ops._run(main, "config", "user.email", "t@t")
    git_ops._run(main, "config", "user.name", "t")
    git_ops._run(main, "switch", "-q", "x")
    _commit(tmp_path / "clone", "a.txt")            # A alteração nunca entrou na base.
    git_ops._run(main, "switch", "-q", "main")
    assert worktrees.is_merged(main, "x", "main") is False
    git_ops._run(remote, "branch", "-D", "x")
    git_ops._run(main, "fetch", "-q", "--prune")
    assert worktrees.is_merged(main, "x", "main") is False


def test_status_missing_folder(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    import shutil
    shutil.rmtree(wt)
    st = worktrees.status(wt)
    assert st["exists"] is False and st["repo"] == main and st["branch"] == "x"


def test_status_missing_folder_inside_main_repo(tmp_path):
    import shutil
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo" / ".claude" / "worktrees" / "x", "x")
    shutil.rmtree(wt)
    st = worktrees.status(wt)   # sobe as pastas até achar o repo que ainda a lista
    assert st["exists"] is False and st["repo"] == main and st["branch"] == "x"
    listed = worktrees.list_all([main], [])[0]["worktrees"][0]
    assert listed["repo"] == main and listed["branch"] == "x"


def test_list_all_groups_by_main_repo(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    _repo(tmp_path / "sem-worktree")
    out = worktrees.list_all([main, wt, str(tmp_path / "sem-worktree")], [])
    assert [r["repo"] for r in out] == [main]
    assert [w["path"] for w in out[0]["worktrees"]] == [wt]


def test_routes_refuse_outside_root(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient
    from app import api, fs
    (tmp_path / "raiz").mkdir()
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    monkeypatch.setattr(fs, "resolve_scan_roots", lambda _s: [tmp_path / "raiz"])
    monkeypatch.setattr(api, "resolve_scan_roots", lambda _s: [tmp_path / "raiz"])
    monkeypatch.setattr(api.settings, "auth_token", "t")
    r = TestClient(api.app).get("/api/worktrees/detail", params={"path": wt},
                                headers={"Authorization": "Bearer t"})
    assert r.status_code == 403


def test_worktree_outside_root_counts_by_its_main_repo(tmp_path, monkeypatch):
    """O Codex cria worktrees fora das raízes; a lista as mostra pelo repo principal, e abrir e
    apagar seguem a mesma regra."""
    from fastapi.testclient import TestClient
    from app import api, fs
    main = _repo(tmp_path / "repo")
    wt = _merged_wt(main, tmp_path / "codex" / "repo-x", "x")
    estranho = _repo(tmp_path / "codex" / "solto")
    _claude_project(tmp_path, monkeypatch, wt)
    monkeypatch.setattr(fs, "resolve_scan_roots", lambda _s: [tmp_path / "repo"])
    monkeypatch.setattr(api, "resolve_scan_roots", lambda _s: [tmp_path / "repo"])
    monkeypatch.setattr(api.settings, "auth_token", "t")
    h = {"Authorization": "Bearer t"}
    c = TestClient(api.app)
    assert c.get("/api/worktrees/detail", params={"path": wt}, headers=h).status_code == 200
    assert c.get("/api/worktrees/detail", params={"path": estranho}, headers=h).status_code == 403
    r = c.post("/api/worktrees/delete", json={"repo": main, "path": wt}, headers=h)
    assert r.status_code == 200 and not (tmp_path / "codex" / "repo-x").exists()


def test_list_all_dedupes_main_repo_through_symlink(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    (tmp_path / "atalho").symlink_to(tmp_path / "repo")
    out = worktrees.list_all([str(tmp_path / "atalho"), main, wt], [])
    assert [r["repo"] for r in out] == [main]


def test_fresh_worktree_is_not_merged(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    assert worktrees.status(wt)["merged"] is False
    _commit(tmp_path / "repo-x", "a.txt")
    assert worktrees.status(wt)["merged"] is False
    git_ops._run(main, "merge", "-q", "--no-ff", "-m", "m", "x")
    assert worktrees.status(wt)["merged"] is True


def test_closed_skips_live_session_transcripts(tmp_path, monkeypatch):
    from app import archive
    from app.registry import sanitize_cwd
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    proj = tmp_path / "projects"
    (proj / sanitize_cwd(wt)).mkdir(parents=True)
    (proj / sanitize_cwd(wt) / "d0000001-0000-4000-8000-000000000000.jsonl").write_text("{}\n")
    (proj / sanitize_cwd(wt) / "d0000003-0000-4000-8000-000000000000.jsonl").write_text("{}\n")
    monkeypatch.setattr(archive, "_contas", lambda *_a: [(None, "", proj)])
    live = SessionInfo(name="s1", cwd=wt, jsonl=str(proj / sanitize_cwd(wt) / "d0000003-0000-4000-8000-000000000000.jsonl"))
    st = worktrees.status(wt, [live])
    assert st["sessions"] == ["s1"] and st["closed"] == 1


def test_sessions_match_through_symlink(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    (tmp_path / "link").symlink_to(tmp_path)
    s = SessionInfo(name="s1", cwd=str(tmp_path / "link" / "repo-x"))
    assert worktrees.status(wt, [s])["sessions"] == ["s1"]


def test_ignored_never_lists_folders(tmp_path):
    main = _repo(tmp_path / "repo")
    (tmp_path / "repo" / ".gitignore").write_text("cache\n")
    git_ops._run(main, "add", ".gitignore")
    git_ops._run(main, "commit", "-q", "-m", "ignore")
    wt = _wt(main, tmp_path / "repo-x", "x")
    (tmp_path / "repo-x" / "cache").mkdir()
    (tmp_path / "repo-x" / "cache" / "dado.bin").write_text("só aqui")
    assert worktrees.status(wt)["ignored"] == []


def test_status_survives_git_timeout(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    (tmp_path / "repo-x" / "solto.txt").write_text("?")
    real_run = worktrees._run

    def slow(cwd, *args, **kw):
        if args[0] == "status":
            raise git_ops.GitError(504, "git timeout")
        return real_run(cwd, *args, **kw)
    monkeypatch.setattr(worktrees, "_run", slow)
    out = worktrees.list_all([main], [])
    st = out[0]["worktrees"][0]
    assert st["dirty"] == 0 and st["degraded"] is True and st["merged"] is False


def test_merge_check_timeout_is_not_merged(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    _commit(tmp_path / "repo-x", "a.txt")
    git_ops._run(main, "merge", "--no-ff", "-m", "Integração", "x")
    st = worktrees.status(wt)
    assert st["merged"] is True and st["degraded"] is False
    real_run = worktrees._run

    def slow(cwd, *args, **kw):
        if args[:2] == ("merge-base", "--is-ancestor"):
            raise git_ops.GitError(504, "git timeout")
        return real_run(cwd, *args, **kw)
    monkeypatch.setattr(worktrees, "_run", slow)
    st = worktrees.status(wt)
    assert st["merged"] is False and st["degraded"] is True
    assert worktrees.is_merged(wt, "x", "main") is False


def test_list_all_skips_repo_outside_roots(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    (tmp_path / "outra").mkdir()
    assert worktrees.list_all([wt], [], roots=[tmp_path / "outra"]) == []
    assert [r["repo"] for r in worktrees.list_all([wt], [], roots=[tmp_path])] == [main]


def test_list_all_repo_filter_without_sessions(tmp_path):
    """Repo sem sessão nem pasta recente: o filtro `repo` basta (menu de branch, lista após criar)."""
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    out = worktrees.list_all([], [], roots=[tmp_path], repo=wt)
    assert [r["repo"] for r in out] == [main]
    assert worktrees.list_all([], [], roots=[tmp_path / "outra"], repo=main) == []


def test_tree_bytes_skips_windows_junctions(monkeypatch):
    """No Windows a junction (node_modules do pnpm) tem `is_symlink()` falso e `is_dir` verdadeiro."""
    import os
    from types import SimpleNamespace

    def entry(path, *, is_dir=False, junction=False):
        return SimpleNamespace(name=path.rsplit("/", 1)[-1], path=path,
                               is_symlink=lambda: False, is_junction=lambda: junction,
                               is_dir=lambda follow_symlinks=True: is_dir,
                               stat=lambda follow_symlinks=True: SimpleNamespace(st_size=100))

    tree = {"/w/pkg": [entry("/w/pkg/a.js"), entry("/w/pkg/link", is_dir=True, junction=True)],
            "/w/link": [entry("/w/link/big.bin")],
            "/w/pkg/link": [entry("/w/pkg/link/big.bin")]}

    class _It(list):
        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    monkeypatch.setattr(os, "scandir", lambda p: _It(tree[p]))
    skipped = [0]
    assert worktrees._tree_bytes(entry("/w/link", is_dir=True, junction=True), skipped) == 0
    assert worktrees._tree_bytes(entry("/w/pkg", is_dir=True), skipped) == 100
    assert skipped == [0]


def test_detail_on_plain_folder_is_404(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient
    from app import api, fs
    (tmp_path / "comum").mkdir()
    monkeypatch.setattr(fs, "resolve_scan_roots", lambda _s: [tmp_path])
    monkeypatch.setattr(api, "resolve_scan_roots", lambda _s: [tmp_path])
    monkeypatch.setattr(api.settings, "auth_token", "t")
    r = TestClient(api.app).get("/api/worktrees/detail", params={"path": str(tmp_path / "comum")},
                                headers={"Authorization": "Bearer t"})
    assert r.status_code == 404


def test_guest_gets_403_on_every_route(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient
    from app import api, guest_users
    monkeypatch.setattr(guest_users, "_path_override", tmp_path / "guests.json")
    guest_users._reset()
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    monkeypatch.setattr(api.settings, "auth_token", "secret")
    _, tok = guest_users.create("bia", str(tmp_path), False, True)
    h = {"Authorization": f"Bearer {tok}"}
    c = TestClient(api.app)
    try:
        assert c.get("/api/worktrees", headers=h).status_code == 403
        assert c.get("/api/worktrees/detail", params={"path": wt}, headers=h).status_code == 403
        assert c.post("/api/worktrees/fetch", json={"repo": main}, headers=h).status_code == 403
        assert c.post("/api/worktrees/delete", json={"repo": main, "path": wt},
                      headers=h).status_code == 403
        assert c.post("/api/worktrees/delete-merged", json={"repo": main}, headers=h).status_code == 403
        assert (tmp_path / "repo-x").exists()
    finally:
        guest_users._reset()


def _claude_project(tmp_path, monkeypatch, wt, sid="d0000002-0000-4000-8000-000000000000"):
    from app import archive
    from app.registry import sanitize_cwd
    base = tmp_path / "cfg" / "projects"
    (base / sanitize_cwd(wt)).mkdir(parents=True)
    (base / sanitize_cwd(wt) / f"{sid}.jsonl").write_text(json.dumps({"cwd": wt}) + "\n")
    (base / sanitize_cwd(wt) / sid).mkdir()
    monkeypatch.setattr(archive, "_contas", lambda config_dir=None: [(None, "", base)])
    return base


def _merged_wt(main, path, branch):
    """Worktree com um commit já mesclado na principal: a recém-criada não conta como mesclada."""
    wt = _wt(main, path, branch)
    _commit(path, f"{branch}.txt")
    assert git_ops._run(main, "merge", "-q", "--no-ff", "-m", "m", branch).returncode == 0
    return wt


def test_delete_clean_merged_moves_conversations_and_branch(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    wt = _merged_wt(main, tmp_path / "repo-x", "x")
    base = _claude_project(tmp_path, monkeypatch, wt)
    from app.registry import sanitize_cwd
    out = worktrees.delete(main, wt, [])
    assert out == {"removed": wt, "branch_deleted": True, "moved": 2}
    assert not (tmp_path / "repo-x").exists()
    assert (base / sanitize_cwd(main) / "d0000002-0000-4000-8000-000000000000.jsonl").exists()
    assert (base / sanitize_cwd(main) / "d0000002-0000-4000-8000-000000000000").is_dir()
    assert worktrees.removed()[wt] == main
    assert worktrees.redirect(wt) == main
    assert "x" not in git_ops.list_branches(main)["branches"]


def test_delete_refuses_open_session_and_main(tmp_path):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    with pytest.raises(GitError) as busy:
        worktrees.delete(main, wt, [SessionInfo(name="s1", cwd=main, worktree_path=wt)])
    assert busy.value.status == 409 and "s1" in busy.value.detail
    with pytest.raises(GitError) as principal:
        worktrees.delete(main, main, [])
    assert principal.value.status == 404


def test_delete_dirty_needs_confirm_and_keeps_unmerged_branch(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    _claude_project(tmp_path, monkeypatch, wt)
    _commit(tmp_path / "repo-x", "a.txt")
    (tmp_path / "repo-x" / "solto.txt").write_text("?")
    with pytest.raises(GitError) as e:
        worktrees.delete(main, wt, [])
    assert e.value.status == 409
    assert (tmp_path / "repo-x").exists()
    out = worktrees.delete(main, wt, [], confirm=True)
    assert out["branch_deleted"] is False
    assert "x" in git_ops.list_branches(main)["branches"]


def test_delete_refuses_degraded_even_confirmed(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    real_run = worktrees._run

    def slow(cwd, *args, **kw):
        if args[0] == "status":
            raise GitError(504, "git timeout")
        return real_run(cwd, *args, **kw)
    monkeypatch.setattr(worktrees, "_run", slow)
    with pytest.raises(GitError) as e:
        worktrees.delete(main, wt, [], confirm=True)
    assert e.value.status == 409
    assert (tmp_path / "repo-x").exists()


def test_delete_folder_already_gone(tmp_path, monkeypatch):
    import shutil
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    _claude_project(tmp_path, monkeypatch, wt)
    shutil.rmtree(wt)
    out = worktrees.delete(main, wt, [])
    assert out["removed"] == wt
    assert worktrees.worktree_paths(main) == []


def test_delete_does_not_overwrite_same_uuid(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    wt = _wt(main, tmp_path / "repo-x", "x")
    base = _claude_project(tmp_path, monkeypatch, wt)
    from app.registry import sanitize_cwd
    (base / sanitize_cwd(main)).mkdir()
    (base / sanitize_cwd(main) / "d0000002-0000-4000-8000-000000000000.jsonl").write_text("principal\n")
    out = worktrees.delete(main, wt, [])
    assert out["moved"] == 0
    assert (base / sanitize_cwd(main) / "d0000002-0000-4000-8000-000000000000.jsonl").read_text() == "principal\n"
    assert (base / sanitize_cwd(wt) / "d0000002-0000-4000-8000-000000000000.jsonl").exists()


def test_delete_merged_only_takes_clean(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    a = _merged_wt(main, tmp_path / "repo-a", "a")
    _merged_wt(main, tmp_path / "repo-b", "b")
    _claude_project(tmp_path, monkeypatch, a)
    (tmp_path / "repo-b" / "solto.txt").write_text("?")
    assert worktrees.delete_merged(main, []) == [a]
    assert (tmp_path / "repo-b").exists()


def test_delete_merged_confirmed_takes_only_the_listed(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    b = _merged_wt(main, tmp_path / "repo-b", "b")
    _merged_wt(main, tmp_path / "repo-c", "c")
    _claude_project(tmp_path, monkeypatch, b)
    (tmp_path / "repo-b" / "solto.txt").write_text("?")
    (tmp_path / "repo-c" / "solto.txt").write_text("?")
    assert worktrees.delete_merged(main, [], paths=[b]) == []
    assert worktrees.delete_merged(main, [], paths=[b], confirm=True, lossy=[b]) == [b]
    assert not (tmp_path / "repo-b").exists() and (tmp_path / "repo-c").exists()


def test_delete_merged_keeps_one_that_got_dirty_after_the_confirmation(tmp_path, monkeypatch):
    main = _repo(tmp_path / "repo")
    a = _merged_wt(main, tmp_path / "repo-a", "a")
    _claude_project(tmp_path, monkeypatch, a)
    (tmp_path / "repo-a" / "novo.txt").write_text("escrito depois da tela")
    assert worktrees.delete_merged(main, [], paths=[a], confirm=True, lossy=[]) == []
    assert (tmp_path / "repo-a" / "novo.txt").exists()


def test_delete_merged_confirm_requires_paths(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient
    from app import api, fs
    main = _repo(tmp_path / "repo")
    monkeypatch.setattr(fs, "resolve_scan_roots", lambda _s: [tmp_path])
    monkeypatch.setattr(api, "resolve_scan_roots", lambda _s: [tmp_path])
    monkeypatch.setattr(api.settings, "auth_token", "t")
    r = TestClient(api.app).post("/api/worktrees/delete-merged", json={"repo": main, "confirm": True},
                                 headers={"Authorization": "Bearer t"})
    assert r.status_code == 422


def test_delete_leaves_colliding_subfolder_conversations(tmp_path, monkeypatch):
    from app.registry import sanitize_cwd
    main = _repo(tmp_path / "repo")
    sub = tmp_path / "repo" / "sub"
    sub.mkdir()
    wt = _merged_wt(main, tmp_path / "repo-sub", "feat")
    assert sanitize_cwd(str(sub)) == sanitize_cwd(wt)   # `repo/sub` e `repo-sub`: mesma pasta de projeto
    base = _claude_project(tmp_path, monkeypatch, str(sub), sid="d0000004-0000-4000-8000-000000000000")
    live = base / sanitize_cwd(wt) / "d000000a-0000-4000-8000-000000000000.jsonl"
    live.write_text(json.dumps({"cwd": wt}) + "\n")
    s2 = SessionInfo(name="s2", cwd=str(sub), jsonl=str(live))
    out = worktrees.delete(main, wt, [s2])
    assert out["moved"] == 0
    assert (base / sanitize_cwd(wt) / "d0000004-0000-4000-8000-000000000000.jsonl").exists()
    assert live.exists()


def test_delete_moves_conversation_that_entered_the_worktree(tmp_path, monkeypatch):
    from app.registry import sanitize_cwd
    main = _repo(tmp_path / "repo")
    wt = _merged_wt(main, tmp_path / "repo-x", "x")
    base = _claude_project(tmp_path, monkeypatch, wt)
    # EnterWorktree: começou na principal, terminou na worktree, e o Claude guarda no projeto dela.
    (base / sanitize_cwd(wt) / "d0000005-0000-4000-8000-000000000000.jsonl").write_text(
        json.dumps({"cwd": main}) + "\n" + json.dumps({"cwd": wt}) + "\n")
    out = worktrees.delete(main, wt, [])
    assert out["moved"] == 3
    assert (base / sanitize_cwd(main) / "d0000005-0000-4000-8000-000000000000.jsonl").exists()


def test_delete_moves_conversations_of_worktree_subfolders(tmp_path, monkeypatch):
    from app.registry import sanitize_cwd
    main = _repo(tmp_path / "repo")
    wt = _merged_wt(main, tmp_path / "repo-x", "x")
    (tmp_path / "repo-x" / "sub").mkdir()
    sub = str(tmp_path / "repo-x" / "sub")
    base = _claude_project(tmp_path, monkeypatch, sub, sid="d0000004-0000-4000-8000-000000000000")
    out = worktrees.delete(main, wt, [])
    assert out["moved"] == 2
    assert (base / sanitize_cwd(main) / "d0000004-0000-4000-8000-000000000000.jsonl").exists()
    assert (base / sanitize_cwd(main) / "d0000004-0000-4000-8000-000000000000").is_dir()


def test_delete_leaves_colliding_deeper_subfolder_conversations(tmp_path, monkeypatch):
    from app.registry import sanitize_cwd
    main = _repo(tmp_path / "repo")
    deep = tmp_path / "repo" / "sub" / "deep"
    deep.mkdir(parents=True)
    wt = _merged_wt(main, tmp_path / "repo-sub", "feat")
    # `repo/sub/deep` cai no prefixo das subpastas de `repo-sub`, mas nunca esteve nela.
    assert sanitize_cwd(str(deep)).startswith(sanitize_cwd(wt) + "-")
    base = _claude_project(tmp_path, monkeypatch, str(deep), sid="d0000006-0000-4000-8000-000000000000")
    out = worktrees.delete(main, wt, [])
    assert out["moved"] == 0
    assert (base / sanitize_cwd(str(deep)) / "d0000006-0000-4000-8000-000000000000.jsonl").exists()


def test_delete_missing_folder_keeps_other_orphan(tmp_path, monkeypatch):
    import shutil
    from app import archive
    monkeypatch.setattr(archive, "_contas", lambda config_dir=None: [])
    main = _repo(tmp_path / "repo")
    a = _wt(main, tmp_path / "repo-a", "a")
    b = _wt(main, tmp_path / "repo-b", "b")
    shutil.rmtree(a)
    shutil.rmtree(b)
    assert worktrees.delete(main, a, [])["removed"] == a
    assert worktrees.worktree_paths(main) == [b]


def test_redirect_through_symlink(tmp_path):
    main = _repo(tmp_path / "repo")
    (tmp_path / "link").symlink_to(tmp_path)
    worktrees.record_removed(str(tmp_path / "repo-x"), main)
    assert worktrees.redirect(str(tmp_path / "link" / "repo-x" / "sub")) == main
    assert worktrees.redirect(str(tmp_path / "outra")) == str(tmp_path / "outra")


def test_create_invalidates_lists_even_if_client_disconnects(tmp_path, monkeypatch):
    """Cliente que desconecta durante o `git worktree add` não deixa a lista sem a pasta nova."""
    import asyncio
    import threading
    from app import api
    entered, release, invalidated = threading.Event(), threading.Event(), threading.Event()

    def slow_create(*_a, **_k):
        entered.set()
        release.wait(5)
        return str(tmp_path / "repo-x"), True

    monkeypatch.setattr(api, "_allowed_scan_root", lambda _p: str(tmp_path))
    monkeypatch.setattr(api, "create_worktree", slow_create)
    monkeypatch.setattr(api, "_invalidate_lists", invalidated.set)

    async def run():
        body = api.WorktreeCreateBody(repo=str(tmp_path / "repo"), branch="x", name="x")
        task = asyncio.create_task(api.worktrees_create(body))
        await asyncio.to_thread(entered.wait, 5)
        task.cancel()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task

    asyncio.run(run())
    assert invalidated.wait(5)
