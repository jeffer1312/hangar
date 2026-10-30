"""Captura resgate e recusa antes de efeitos destrutivos em repositórios isolados."""
import argparse, json, os, re, subprocess, sys, tempfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--reference", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
reference, output = args.reference.resolve(), args.output.resolve()
commit = subprocess.check_output(["git", "-C", str(reference), "rev-parse", "HEAD"], text=True).strip()
with tempfile.TemporaryDirectory(prefix="hangar-rescue-contract-") as scratch:
    root = Path(scratch)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=os.devnull, GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
        GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
    os.chdir(root)
    sys.path.insert(0, str(reference / "backend"))
    from app import atualizar as app
    original_git = app._git
    rows = []
    for name in ["clean", "dirty", "dirty-untracked", "ahead", "uncertain", "work-branch",
                 "branch-failed", "stash-failed", "verify-failed"]:
        repo = root / name
        repo.mkdir()
        app.REPO = repo
        os.environ["CLAUDE_CONFIG_DIR"] = str(repo / "claude")
        def git(*argv):
            return subprocess.run(["git", "-C", str(repo), *argv], text=True, capture_output=True)
        for argv in [("init", "-b", "main"), ("config", "user.name", "Fixture"),
                     ("config", "user.email", "fixture@example.invalid")]:
            assert git(*argv).returncode == 0
        (repo / "a.txt").write_text("original\n")
        assert git("add", ".").returncode == 0
        assert git("commit", "-m", "original").returncode == 0
        pre = {"sujo": 0, "ahead": 0, "ahead_incerto": False, "branch_de_trabalho": False}
        if name in ["dirty", "dirty-untracked", "stash-failed"]:
            (repo / "a.txt").write_text("alterado\n")
            pre["sujo"] = 1
        if name == "dirty-untracked":
            (repo / "solto.txt").write_text("solto\n")
        if name in ["ahead", "branch-failed", "verify-failed"]:
            pre["ahead"] = 1
        if name == "uncertain":
            pre["ahead_incerto"] = True
        if name == "work-branch":
            pre["branch_de_trabalho"] = True
        calls = []
        def recorded(*argv, **kwargs):
            calls.append({"args": list(argv), "timeout": kwargs.get("timeout")})
            fail = ((name == "branch-failed" and argv[0] == "branch")
                    or (name == "stash-failed" and argv[0] == "stash")
                    or (name == "verify-failed" and argv[:2] == ("rev-parse", "--verify")))
            if fail:
                return subprocess.CompletedProcess(argv, 1, "", "fixture-git-failed")
            return original_git(*argv, **kwargs)
        app._git = recorded
        try:
            result = {"value": app.resguardar(pre)}
        except Exception as error:
            result = {"error": str(error), "type": type(error).__name__}
        refs = git("for-each-ref", "--format=%(refname:short)", "refs/heads/resgate/").stdout.splitlines()
        head = git("rev-parse", "HEAD").stdout.strip()
        stash = git("stash", "list", "--format=%s").stdout.splitlines()
        def stashed(path, untracked=False):
            found = git("show", ("stash@{0}^3:" if untracked else "stash@{0}:") + path)
            return found.stdout if found.returncode == 0 else None
        row = {"name": name, "pre": pre, "result": result, "calls": calls,
               "refs": [{"name": ref, "at_head": git("rev-parse", ref).stdout.strip() == head} for ref in refs],
               "file": (repo / "a.txt").read_text(), "untracked_exists": (repo / "solto.txt").exists(),
               "stash": stash, "stashed_file": stashed("a.txt"), "stashed_untracked": stashed("solto.txt", True)}
        def normalize(value):
            if isinstance(value, str):
                return re.sub(r"resgate/\d{4}-\d{2}-\d{2}-\d{4}", "resgate/<MINUTE>", value)
            if isinstance(value, list):
                return [normalize(item) for item in value]
            if isinstance(value, dict):
                return {key: normalize(item) for key, item in value.items()}
            return value
        rows.append(normalize(row))
output.write_text(json.dumps({"source_commit": commit,
    "command": "python backend/tests/fixtures/contract/update-git/generate-rescue.py --reference <checkout> --output <fixture>",
    "normalization": "Somente o minuto gerado no nome resgate/YYYY-MM-DD-HHMM; refs conferidas contra HEAD real.",
    "cases": rows}, ensure_ascii=False, indent=2) + "\n")
print(f"{len(rows)} casos capturados")
