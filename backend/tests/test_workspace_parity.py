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


@pytest.fixture
def repo(tmp_path, monkeypatch):
    monkeypatch.setenv("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+00:00")
    monkeypatch.setenv("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+00:00")
    for args in (("init", "-b", "main"), ("config", "user.name", "Teste"), ("config", "user.email", "teste@example.invalid")):
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
    shutil.copytree(repo, copy)
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
    try:
        deadline = time.monotonic() + 5
        while not child_file.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        assert child_file.exists(), "A fixture não iniciou o descendente"
        pids = [int(root_file.read_text()), int(child_file.read_text())]
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
