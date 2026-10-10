"""Contratos de sucesso e efeitos em repositórios sintéticos iguais nos dois leitores."""
import json
import os
import subprocess
import shutil
import time
from pathlib import Path

import pytest
from app import filetree, filesearch, git_ops, transcript

_BINARY = Path(os.environ.get("CP_WORKSPACE_CONTRACT_BIN", Path(__file__).parents[2] / "crates/target/debug/examples/workspace_contract"))


def rust(operation, **arguments):
    result = subprocess.run([str(_BINARY)], input=json.dumps({"op": operation, "args": arguments},
        default=os.fspath, ensure_ascii=False) + "\n", text=True, encoding="utf-8", capture_output=True, check=True)
    return json.loads(result.stdout)


# Trava do git some no meio da cópia (manutenção em segundo plano) e derruba o copytree; não é estado
# comparado pelos testes.
_SEM_TRAVAS = shutil.ignore_patterns("*.lock", "tmp_*")


@pytest.fixture
def repo(tmp_path, monkeypatch):
    monkeypatch.setenv("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+00:00")
    monkeypatch.setenv("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+00:00")
    for args in (("init", "-b", "main"), ("config", "user.name", "Teste"), ("config", "user.email", "teste@example.invalid"),
                 ("config", "maintenance.auto", "false"), ("config", "gc.auto", "0")):
        git_ops._run(str(tmp_path), *args)
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub/ação.txt").write_text("primeira\nlinha\n", encoding="utf-8")
    (tmp_path / "outro.txt").write_text("outro\n", encoding="utf-8")
    git_ops._run(str(tmp_path), "add", ".")
    git_ops._run(str(tmp_path), "commit", "-m", "Inicial")
    (tmp_path / "sub/ação.txt").write_text("primeira\nlinha mudada\n", encoding="utf-8")
    (tmp_path / "novo.txt").write_text("novo\n", encoding="utf-8")
    return tmp_path


@pytest.mark.parametrize("operation,args", [
    ("head_info", {}), ("branch_of", {}), ("git_summary", {}), ("git_diffstat", {}),
    ("list_branches", {}), ("changed_files", {}), ("git_log", {"n":50,"grep":None}),
    ("last_commit_message", {}), ("folder_status", {}), ("sequencer_state", {}),
    ("file_diff", {"path":"sub/ação.txt"}), ("file_diff", {"path":"novo.txt"}),
    ("path_diff", {"path":"sub/ação.txt", "escopo":"nao_commitado"}),
    ("path_diff", {"path":"sub/ação.txt", "escopo":"branch"}),
])
def test_git_read_contract(repo, operation, args):
    expected = getattr(git_ops, operation)(str(repo), **args)
    assert rust(operation, cwd=repo, **args) == {"ok":True, "result":json.loads(json.dumps(expected))}


@pytest.mark.parametrize("operation,args", [
    ("list_dir", {"path":None,"so_modificados":False}),
    ("list_dir", {"path":"sub","so_modificados":True}),
    ("read_file", {"path":"sub/ação.txt"}),
    ("search", {"q":"ação", "mode":"names"}),
    ("search", {"q":"primeira", "mode":"contents"}),
    ("resolver", {"caminhos":["ação.txt","sub/ação.txt","ausente"], "suffix":True}),
])
def test_file_read_contract(repo, operation, args):
    module = filetree if operation in {"list_dir","read_file"} else filesearch
    expected = getattr(module, operation)(str(repo), **args)
    assert rust(operation, cwd=repo, **args) == {"ok":True,"result":expected}


def test_commit_revision_contract(repo):
    sha = git_ops._run(str(repo), "rev-parse", "HEAD").stdout.strip()
    for operation, args in (("commit_files", {}), ("commit_diff", {}), ("commit_file_diff", {"path":"sub/ação.txt"}), ("diff_vs_worktree", {}), ("branches_containing", {})):
        assert rust(operation, cwd=repo, sha=sha, **args) == {"ok":True, "result":getattr(git_ops, operation)(str(repo), sha, **args)}


def test_citation_contract_uses_longest_needle_and_newest_cwd(repo):
    log = repo / "fixture.jsonl"
    log.write_text("\n".join(json.dumps(line, ensure_ascii=False) for line in [
        {"cwd":str(repo), "text":"sub/ação.txt"},
        {"cwd":str(repo / "sub"), "text":"ação.txt"},
        {"cwd":str(repo), "text":"sub/ação.txt"},
    ]), encoding="utf-8")
    args = {"jsonl":log,"needles":["sub/ação.txt", "ação.txt"]}
    assert rust("citation_cwds", **args) == {"ok":True,"result":transcript.citation_cwds(log, args["needles"])}


@pytest.mark.parametrize("operation,args", [
    ("commit", {"message":"Selecionado", "paths":["sub/ação.txt"], "amend":False,"new_branch":None}),
    ("commit", {"message":"Reescrito", "paths":[],"amend":True,"new_branch":None}),
    ("commit", {"message":"Nova branch", "paths":["novo.txt"],"amend":False,"new_branch":"nova"}),
    ("discard_file", {"path":"sub/ação.txt"}), ("discard_file", {"path":"novo.txt"}),
    ("switch_branch", {"branch":"feature"}),
    ("create_branch_at", {"name":"nova","sha":None,"switch_after":False}),
    ("create_tag", {"name":"marca","sha":None,"message":None}),
    ("create_tag", {"name":"marca","sha":None,"message":"   "}),
    ("git_action", {"action":"stash"}), ("git_action", {"action":"stash-pop"}),
    ("git_action", {"action":"fetch"}),
    ("reset_to", {"sha":"$HEAD", "mode":"soft"}),
    ("reset_to", {"sha":"$HEAD", "mode":"mixed"}),
    ("reset_to", {"sha":"$HEAD", "mode":"hard"}),
])
def test_mutation_contract_and_disk_effects(repo, operation, args):
    git_ops._run(str(repo), "branch", "feature")
    copy = repo.parent / (repo.name + "-rust")
    shutil.copytree(repo, copy, ignore=_SEM_TRAVAS)
    args = {key: git_ops._run(str(repo), "rev-parse", "HEAD").stdout.strip() if value == "$HEAD" else value
            for key, value in args.items()}
    expected = getattr(git_ops, operation)(str(repo), **args)
    assert rust(operation, cwd=copy, **args) == {"ok":True,"result":expected}
    for git_args in (("status", "--porcelain", "-z"), ("ls-files", "--stage"), ("show-ref",), ("log", "--all", "--format=%H %s")):
        assert git_ops._run(str(repo), *git_args).stdout == git_ops._run(str(copy), *git_args).stdout
    files = lambda directory: {str(path.relative_to(directory)):path.read_bytes() for path in directory.rglob("*")
                               if path.is_file() and ".git" not in path.relative_to(directory).parts}
    assert files(repo) == files(copy)


def test_resolver_preserves_first_git_candidate_for_ambiguous_suffix(repo):
    (repo / "a").mkdir()
    (repo / "b").mkdir()
    (repo / "a/nome.txt").write_text("primeiro", encoding="utf-8")
    (repo / "b/nome.txt").write_text("segundo", encoding="utf-8")
    args = {"caminhos":["nome.txt"], "suffix":True}
    assert rust("resolver", cwd=repo, **args) == {"ok":True,"result":filesearch.resolver(str(repo), **args)}


@pytest.mark.skipif(os.name == "nt", reason="Aspas não são permitidas em nomes de arquivo no Windows")
def test_numstat_preserves_git_escaped_names(repo):
    path = 'ação "citada".txt'
    (repo / path).write_text("original\n", encoding="utf-8")
    git_ops._run(str(repo), "add", "--", path)
    git_ops._run(str(repo), "commit", "-m", "Nome citado")
    (repo / path).write_text("original\nlinha nova\n", encoding="utf-8")
    assert rust("changed_files", cwd=repo) == {"ok":True,"result":git_ops.changed_files(str(repo))}


@pytest.mark.skipif(os.name == "nt", reason="Barra invertida é separador no Windows")
def test_posix_listing_keeps_literal_backslash_and_readable_path(repo):
    path = "nota\\ação.txt"
    (repo / path).write_text("literal", encoding="utf-8")
    listing = rust("list_dir", cwd=repo, path=None, so_modificados=False)["result"]
    entry = next(e for e in listing["entries"] if e["name"] == path)
    assert entry["path"] == path
    assert rust("read_file", cwd=repo, path=entry["path"])["result"]["text"] == "literal"


@pytest.mark.skipif(os.name == "nt", reason="Instrumentação do executor POSIX")
def test_selected_commit_does_not_rescan_the_repo_for_each_path(repo, monkeypatch):
    shim = repo.parent / "git-shim"
    shim.mkdir()
    counts = shim / "status-count"
    actual_git = shutil.which("git")
    wrapper = shim / "git"
    wrapper.write_text('#!/bin/sh\nfor arg in "$@"; do if [ "$arg" = status ]; then echo status >> "$HANGAR_STATUS_COUNT"; fi; done\nexec "$HANGAR_REAL_GIT" "$@"\n', encoding="utf-8")
    wrapper.chmod(0o755)
    paths = [f"seleção-{n}.txt" for n in range(8)]
    for path in paths:
        (repo / path).write_text("selecionado", encoding="utf-8")
    monkeypatch.setenv("PATH", str(shim) + os.pathsep + os.environ["PATH"])
    monkeypatch.setenv("HANGAR_REAL_GIT", actual_git)
    monkeypatch.setenv("HANGAR_STATUS_COUNT", str(counts))
    assert rust("commit", cwd=repo, message="Seleção", paths=paths, amend=False, new_branch=None)["ok"]
    assert len(counts.read_text().splitlines()) <= 2


def test_executor_death_ends_the_command_and_its_child(tmp_path):
    root_file = tmp_path / "root-pid"
    child_file = tmp_path / "child-pid"
    env = {**os.environ, "HANGAR_TEST_ROOT_PID":str(root_file), "HANGAR_TEST_CHILD_PID":str(child_file)}
    worker = subprocess.Popen([str(_BINARY), "--process-fixture"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    pids = []
    def alive(pid):
        if os.name == "nt":
            import ctypes
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.OpenProcess.restype = ctypes.c_void_p
            handle = kernel.OpenProcess(0x1000, False, pid)
            if not handle:
                return False
            exit_code = ctypes.c_ulong()
            try:
                return bool(kernel.GetExitCodeProcess(ctypes.c_void_p(handle), ctypes.byref(exit_code))) and exit_code.value == 259
            finally:
                kernel.CloseHandle(ctypes.c_void_p(handle))
        if os.name != "nt":
            stat = Path(f"/proc/{pid}/stat")
            if not stat.exists() or stat.read_text().split(")", 1)[1].split()[0] == "Z":
                return False
        try:
            os.kill(pid, 0)
            return True
        except OSError:
            return False
    def wait_pid(path, deadline):
        # A fixture cria o arquivo antes de escrever o número: existir não basta.
        while time.monotonic() < deadline:
            if path.exists() and (text := path.read_text().strip()):
                return int(text)
            time.sleep(.01)
        raise AssertionError("A fixture não iniciou o descendente")
    try:
        deadline = time.monotonic() + 5
        pids = [wait_pid(root_file, deadline), wait_pid(child_file, deadline)]
        worker.kill()
        worker.wait(timeout=5)
        deadline = time.monotonic() + 3
        while any(alive(pid) for pid in pids) and time.monotonic() < deadline:
            time.sleep(.01)
        assert not any(alive(pid) for pid in pids), "O comando sobreviveu ao executor"
    finally:
        if worker.poll() is None:
            worker.kill()
            worker.wait(timeout=5)
        if os.name != "nt":
            for pid in pids:
                if alive(pid):
                    try:
                        os.kill(pid, 9)
                    except OSError:
                        pass


def _worktree_pair(repo):
    # Dois repositórios iguais lado a lado: um para cada lado, efeitos comparados por nome relativo.
    git_ops._run(str(repo), "add", ".")
    git_ops._run(str(repo), "commit", "-m", "Base")
    git_ops._run(str(repo), "branch", "outra")
    (repo / ".gitignore").write_text(".env\n", encoding="utf-8")
    git_ops._run(str(repo), "add", ".gitignore")
    git_ops._run(str(repo), "commit", "-m", "Ignora")
    (repo / ".env").write_text("SEGREDO=1\n", encoding="utf-8")
    sides = {}
    for side in ("py", "rs"):
        root = repo.parent / f"{repo.name}-{side}"
        root.mkdir()
        shutil.copytree(repo, root / "repo", ignore=_SEM_TRAVAS)
        sides[side] = root
    return sides


def _worktree_effects(root):
    target = root / "repo-wt"
    config = git_ops._run(str(root / "repo"), "config", "--get-regexp", r"^branch\.")
    return {
        "files": sorted(str(p.relative_to(target)) for p in target.rglob("*")
                        if p.is_file() and ".git" not in p.relative_to(target).parts) if target.exists() else None,
        "env": (target / ".env").read_text(encoding="utf-8") if (target / ".env").exists() else None,
        "branches": git_ops._run(str(root / "repo"), "branch", "--format=%(refname:short) %(upstream)").stdout,
        "config": config.stdout,
    }


def _both(sides, operation, **args):
    try:
        expected = {"ok": True, "result": json.loads(json.dumps(getattr(git_ops, operation)(
            str(sides["py"] / "repo"), allowed_root=sides["py"], **args)))}
    except git_ops.GitError as e:
        expected = {"ok": False, "error": {"status": e.status, "detail": e.detail}}
    actual = rust(operation, cwd=sides["rs"] / "repo", allowed_root=sides["rs"], **args)
    if actual["ok"] and actual["result"][0].startswith(str(sides["rs"])):
        actual["result"][0] = str(sides["py"]) + actual["result"][0][len(str(sides["rs"])):]
    return expected, actual


@pytest.mark.parametrize("args", [
    {"branch": "nova", "name": "wt", "new_branch": True, "base": None},
    {"branch": "nova", "name": "wt", "new_branch": True, "base": "outra"},
    {"branch": "outra", "name": "wt", "new_branch": False, "base": None},
    {"branch": "outra", "name": "wt", "new_branch": True, "base": None},
    {"branch": "-x", "name": "wt", "new_branch": True, "base": None},
    {"branch": "nova ", "name": "wt", "new_branch": True, "base": None},
    {"branch": "nova", "name": "wt", "new_branch": True, "base": "ausente"},
])
def test_create_worktree_new_branch_and_base_contract(repo, args):
    sides = _worktree_pair(repo)
    expected, actual = _both(sides, "create_worktree", **args)
    assert actual == expected
    assert _worktree_effects(sides["rs"]) == _worktree_effects(sides["py"])


def test_create_worktree_new_branch_from_remote_base(repo):
    sides = _worktree_pair(repo)
    for root in sides.values():
        upstream = root / "upstream"
        git_ops._run(str(root / "repo"), "clone", "--bare", "-q", str(root / "repo"), str(upstream))
        git_ops._run(str(root / "repo"), "remote", "add", "origin", str(upstream))
        git_ops._run(str(upstream), "branch", "so-remota", "main")
        git_ops._run(str(root / "repo"), "fetch", "-q", "origin")
    expected, actual = _both(sides, "create_worktree", branch="nova", name="wt", new_branch=True, base="so-remota")
    assert expected["ok"] and actual == expected
    assert _worktree_effects(sides["rs"]) == _worktree_effects(sides["py"])


@pytest.mark.parametrize("force", [False, True])
def test_remove_dirty_worktree_force_contract(repo, force):
    sides = _worktree_pair(repo)
    for root in sides.values():
        git_ops._run(str(root / "repo"), "worktree", "add", "-q", str(root / "repo-wt"), "outra")
        (root / "repo-wt" / "sujo.txt").write_text("não versionado\n", encoding="utf-8")
    try:
        expected = {"ok": True, "result": git_ops.remove_worktree(str(sides["py"] / "repo"), str(sides["py"] / "repo-wt"), force=force)}
    except git_ops.GitError as e:
        expected = {"ok": False, "error": {"status": e.status, "detail": e.detail.replace(str(sides["py"]), "<raiz>")}}
    actual = rust("remove_worktree", cwd=sides["rs"] / "repo", path=sides["rs"] / "repo-wt", force=force)
    if not actual["ok"]:
        actual["error"]["detail"] = actual["error"]["detail"].replace(str(sides["rs"]), "<raiz>")
    assert actual == expected
    assert (sides["rs"] / "repo-wt").exists() == (sides["py"] / "repo-wt").exists() == (not force)


def test_citations_read_rows_in_memory_instead_of_the_file(repo):
    (repo / "docs").mkdir()
    (repo / "docs/nota.md").write_text("nota\n", encoding="utf-8")
    rows = [json.dumps(line, ensure_ascii=False).encode() + b"\n" for line in [
        {"cwd": str(repo), "text": f"veja {repo}/docs/nota.md e docs/nota.md"},
        {"cwd": str(repo / "docs"), "text": "nota.md citada"},
    ]]
    # O arquivo diz outra coisa: só as linhas em memória podem produzir o resultado.
    log = repo / "fixture.jsonl"
    log.write_text(json.dumps({"cwd": "/nada", "text": "vazio"}) + "\n", encoding="utf-8")
    text = [row.decode() for row in rows]
    assert rust("citation_cwds", jsonl=log, needles=["docs/nota.md", "nota.md"], rows=text) == {
        "ok": True, "result": transcript.citation_cwds(log, ["docs/nota.md", "nota.md"], rows=rows)}
    assert rust("cited_elsewhere", jsonl=log, path="nota.md", rows=text) == {
        "ok": True, "result": json.loads(json.dumps(transcript.cited_elsewhere(log, "nota.md", rows=rows)))}
    from app import api
    other = repo.parent / "outra-pasta"
    other.mkdir()
    expected = api._cited_elsewhere(str(log), str(other), "nota.md", [str(repo)], siblings=False, rows=rows)
    assert expected == str(repo / "docs/nota.md")
    assert rust("find_elsewhere", jsonl=log, cwd=other, path="nota.md", worked=[str(repo)],
                siblings=False, rows=text) == {"ok": True, "result": expected}
