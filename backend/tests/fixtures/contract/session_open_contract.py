"""Contrato da abertura de sessão: executa o Python do commit fixo e compara com as fixtures.

Uso, de backend/:
    uv run python tests/fixtures/contract/session_open_contract.py generate [cenário ...]
    uv run python tests/fixtures/contract/session_open_contract.py diff --side python [cenário ...]
    uv run python tests/fixtures/contract/session_open_contract.py diff --side rust [cenário ...]

Cada cenário roda num processo filho isolado (HOME, XDG_RUNTIME_DIR e TMPDIR temporários) contra a
árvore `backend/app` exportada de SOURCE_COMMIT por `git archive`. Nada é lançado: tmux, systemd-run,
agentes e o cano são interceptados na borda do subprocess e só gravados; sinal e rede para fora são
recusados e gravados. Ver session-open/README.md.
"""
from __future__ import annotations

import argparse
import copy
import fcntl
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "session-open"
REPO = HERE.parents[3]
SOURCE_COMMIT = "601bc62a0"
# Exportado do commit: o código que roda e os caminhos relativos ao repositório que ele lê.
EXPORT_PATHS = ("backend/app", "backend/tests/fixtures/pane_idle.txt", "plugins", "skills", "resources",
                "scripts")
RUNS = Path("/tmp/hso")
TOKEN = "hso-token"
FAKE_PID = 9_000_000           # acima do pid_max do kernel: nunca é um processo real
GENERATED_BY = "uv run python tests/fixtures/contract/session_open_contract.py generate {name}"
# Programas que o Python procura no PATH: existem só para o `shutil.which`; nunca rodam.
FAKE_PROGRAMS = ("claude", "codex", "pi", "omp", "kimi", "kimi-code", "hangar-codex-tui",
                 "hangar-engine", "hangar-conta", "bwrap", "systemd-run", "systemctl", "tmux")
TMUX_QUERIES = {"has-session", "list-sessions", "list-panes", "display-message", "display",
                "show-options", "capture-pane", "show-buffer", "show-environment", "list-clients"}
# Ruído do shell e do tmux no ambiente do pane, fora do contrato.
SHELL_ENV_NOISE = {"PWD", "OLDPWD", "SHLVL", "_"}


class RustSideNotImplemented(AssertionError):
    pass


# ── Raiz do cenário e exportação do código ───────────────────────────────────────────────────

def root_for(name: str) -> Path:
    # Raiz curta e fixa por cenário: os hashes que o Python deriva do caminho (trust do Kimi, pasta
    # de projeto do Claude) saem iguais nos dois lados, e o socket do cano cabe no limite do kernel.
    return RUNS / hashlib.sha256(name.encode()).hexdigest()[:6]


def export_source(commit: str = SOURCE_COMMIT) -> Path:
    """`git archive` do commit fixo em /tmp/hso/src-<commit>; reaproveitado quando completo."""
    target = RUNS / f"src-{commit}"
    done = target / ".hso-complete"
    RUNS.mkdir(mode=0o700, exist_ok=True)
    with open(RUNS / f"src-{commit}.lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if done.exists():
            return target
        archive = subprocess.run(["git", "-C", str(REPO), "archive", "--format=tar", commit, *EXPORT_PATHS],
                                 capture_output=True, env=_git_env())
        if archive.returncode != 0:
            # Clone raso (o checkout do CI) não tem o commit: o lado Python não roda, e diz por quê.
            raise RuntimeError(f"lado Python indisponível: commit {commit} fora deste clone "
                               f"(git fetch --unshallow?): {archive.stderr.decode(errors='replace').strip()}")
        with tempfile.TemporaryDirectory(dir=RUNS) as tmp:
            subprocess.run(["tar", "-x", "-C", tmp], input=archive.stdout, check=True)
            (Path(tmp) / ".hso-complete").write_text(commit + "\n")
            if target.exists():
                shutil.rmtree(target)
            os.rename(tmp, target)
            os.mkdir(tmp)   # o TemporaryDirectory apaga o que achar ali
    return target


def _git_env() -> dict:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1")
    return env


def base_env(root: Path) -> dict:
    return {
        "HOME": f"{root}/home", "PATH": f"{root}/bin:/usr/bin:/bin", "LANG": "C.UTF-8", "TZ": "UTC",
        "USER": "hso", "LOGNAME": "hso", "SHELL": f"{root}/bin/sh-login", "TERM": "xterm-256color",
        "XDG_RUNTIME_DIR": f"{root}/run", "TMPDIR": f"{root}/tmp", "CP_AUTH_TOKEN": TOKEN,
        "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "hso", "GIT_AUTHOR_EMAIL": "hso@example.invalid",
        "GIT_COMMITTER_NAME": "hso", "GIT_COMMITTER_EMAIL": "hso@example.invalid",
        "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
    }


def subst(value, root: Path):
    """`<ROOT>` do cenário vira o caminho real da raiz; `<ROOT_SANITIZED>`, a pasta de projeto dela."""
    if isinstance(value, str):
        return value.replace("<ROOT_SANITIZED>", re.sub(r"[^A-Za-z0-9]", "-", str(root))).replace("<ROOT>", str(root))
    if isinstance(value, list):
        return [subst(v, root) for v in value]
    if isinstance(value, dict):
        return {subst(k, root): subst(v, root) for k, v in value.items()}
    return value


def _claim_root(root: Path) -> None:
    """Só apaga a raiz que este harness criou (marcador); qualquer outra coisa ali é recusada."""
    RUNS.mkdir(mode=0o700, exist_ok=True)
    if RUNS.stat().st_uid != os.getuid():
        raise RuntimeError(f"{RUNS} não é deste usuário")
    if root.exists():
        if not (root / ".hso-owned").is_file():
            raise RuntimeError(f"{root} existe sem o marcador do harness; não apago")
        shutil.rmtree(root)
    root.mkdir(mode=0o700)
    (root / ".hso-owned").write_text("session-open harness\n")


def materialize(scenario: dict, root: Path) -> None:
    for sub in ("home", "bin", "run", "tmp", "work", ".harness"):
        (root / sub).mkdir(mode=0o700, exist_ok=True)
    (root / "run" / "wayland-1").touch()   # o pane recebe WAYLAND_DISPLAY como numa sessão gráfica
    for prog in FAKE_PROGRAMS + ("sh-login",):
        p = root / "bin" / prog
        p.write_text("#!/bin/sh\necho 'session-open harness: este programa nunca roda' >&2\nexit 97\n")
        p.chmod(0o755)
    state = subst(scenario.get("state", {}), root)
    for path, content in state.get("files", {}).items():
        p = Path(path)
        p.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, dict) and "symlink" in content:
            p.symlink_to(content["symlink"])
        elif isinstance(content, (dict, list)):
            p.write_text(json.dumps(content, ensure_ascii=False, indent=2), encoding="utf-8")
        else:
            p.write_text(content, encoding="utf-8")
    for d in state.get("dirs", []):
        Path(d).mkdir(parents=True, exist_ok=True)
    for repo in state.get("git_repos", []):
        _make_repo(Path(repo["path"]), repo, base_env(root))


def _make_repo(path: Path, spec: dict, env: dict) -> None:
    path.mkdir(parents=True, exist_ok=True)
    run = lambda *a: subprocess.run(["git", "-C", str(path), *a], check=True, env=env, capture_output=True)
    run("init", "-q", "-b", spec.get("branch", "main"))
    for rel, content in spec.get("files", {}).items():
        (path / rel).parent.mkdir(parents=True, exist_ok=True)
        (path / rel).write_text(content)
    run("add", "-A")
    run("commit", "-q", "-m", "init")
    for b in spec.get("branches", []):
        run("branch", b)
    if spec.get("gitignore"):
        (path / ".gitignore").write_text(spec["gitignore"])
    for rel, content in spec.get("ignored", {}).items():
        (path / rel).write_text(content)


# ── Instantâneo de arquivos ──────────────────────────────────────────────────────────────────

_SKIP_PARTS = {".harness", "bin", "__pycache__"}
_GIT_SKIP = {"objects", "logs", "hooks", "index", "ORIG_HEAD", "COMMIT_EDITMSG", "FETCH_HEAD", "description",
             "info", "packed-refs"}


def snapshot(root: Path) -> dict:
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        rel = Path(dirpath).relative_to(root)
        if rel.parts and rel.parts[0] in _SKIP_PARTS:
            dirnames[:] = []
            continue
        if ".git" in rel.parts:
            # Internos do git (objetos, reflog com hora, índice binário) ficam fora em qualquer nível.
            if any(p in _GIT_SKIP for p in rel.parts[rel.parts.index(".git") + 1:]):
                dirnames[:] = []
                continue
        if rel.parts[:4] == ("home", ".hangar", "logs", "diario"):
            continue
        for d in list(dirnames):
            full = Path(dirpath, d)
            if full.is_symlink():
                out[str(full)] = {"symlink": os.readlink(full)}
                dirnames.remove(d)
        for f in filenames:
            full = Path(dirpath, f)
            if ".git" in rel.parts and f in _GIT_SKIP:
                continue
            if full.is_symlink():
                out[str(full)] = {"symlink": os.readlink(full)}
            elif full.is_socket() or f == ".hso-owned":
                continue
            else:
                data = full.read_bytes()
                out[str(full)] = _content(full, data)
    return out


def _content(path: Path, data: bytes):
    if len(data) > 256 * 1024:
        return {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
    if path.suffix == ".json" or text.lstrip().startswith(("{", "[")):
        try:
            return {"json": json.loads(text)}
        except ValueError:
            pass
    if path.suffix == ".jsonl":
        try:
            return {"jsonl": [json.loads(line) for line in text.splitlines() if line.strip()]}
        except ValueError:
            pass
    return {"text": text}


def file_changes(before: dict, after: dict) -> dict:
    changes = {}
    for path in sorted(set(before) | set(after)):
        if path not in after:
            changes[path] = {"deleted": True}
        elif path not in before:
            changes[path] = {"created": after[path]}
        elif before[path] != after[path]:
            changes[path] = {"modified": after[path]}
    return changes


def read_diary(root: Path) -> list:
    rows = []
    for f in sorted((root / "home" / ".hangar" / "logs" / "diario").glob("*.jsonl")):
        rows += [json.loads(line) for line in f.read_text(encoding="utf-8").splitlines() if line.strip()]
    return rows


# ── Lado Python ──────────────────────────────────────────────────────────────────────────────

def run_python_side(scenario: dict) -> dict:
    name = scenario["name"]
    root = root_for(name)
    src = export_source()
    RUNS.mkdir(mode=0o700, exist_ok=True)
    with open(RUNS / f"{root.name}.lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        _claim_root(root)
        try:
            materialize(scenario, root)
            before = snapshot(root)
            (root / ".harness" / "scenario.json").write_text(json.dumps(scenario, ensure_ascii=False))
            out = root / ".harness" / "out.json"
            proc = subprocess.run(
                [sys.executable, "-I", str(Path(__file__).resolve()), "--child",
                 str(root / ".harness" / "scenario.json"), str(out), str(src / "backend")],
                env=base_env(root), cwd=root / "work", capture_output=True, text=True, timeout=180)
            if proc.returncode != 0 or not out.exists():
                raise RuntimeError(f"cenário {name}: filho saiu {proc.returncode}\n{proc.stderr[-6000:]}")
            raw = json.loads(out.read_text(encoding="utf-8"))
            raw["files"] = file_changes(before, snapshot(root))
            raw["diary"] = read_diary(root)
            raw["_paths"] = {"root": str(root), "repo": str(src), "python": sys.executable}
            return raw
        finally:
            shutil.rmtree(root, ignore_errors=True)


def _rust_binary() -> Path:
    """O exemplo `session_open_contract` do `hangar-server` (lado Rust do harness)."""
    if os.environ.get("HSO_RUST_BIN"):
        return Path(os.environ["HSO_RUST_BIN"])
    target = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "crates" / "target")
    binary = target / "debug" / "examples" / "session_open_contract"
    if not binary.is_file():
        raise RuntimeError(f"lado Rust sem binário em {binary}: compile com `cargo build -p hangar-server "
                           "--example session_open_contract` (ou aponte HSO_RUST_BIN)")
    return binary


_ARCHIVE_RESUME = re.compile(r"^/api/archive/[^/]+/[^/]+/resume$")


def run_rust_side(scenario: dict) -> dict:
    """A abertura do Rust sobre o mesmo cenário montado: o exemplo `session_open_contract` prepara o que
    o Rust lê do disco (antes do instantâneo), roda os passos e devolve o mesmo dicionário cru. O que
    ele ainda não atende (outros provedores, sem terminal, reaberturas) falha dizendo a rota."""
    name = scenario["name"]
    route = " ".join(f"{s['method']} {s['path']}" for s in scenario["steps"] if "method" in s)
    if any(s.get("method") != "POST" or not (s.get("path") == "/api/sessions" or _ARCHIVE_RESUME.match(s.get("path", "")))
           for s in scenario["steps"]):
        raise RustSideNotImplemented(f"Rust side not implemented for {route} (scenario {name})")
    binary = _rust_binary()
    root = root_for(name)
    RUNS.mkdir(mode=0o700, exist_ok=True)
    with open(RUNS / f"{root.name}.lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        _claim_root(root)
        try:
            materialize(scenario, root)
            scenario_file = root / ".harness" / "scenario.json"
            scenario_file.write_text(json.dumps(scenario, ensure_ascii=False))
            env = base_env(root)
            subprocess.run([str(binary), "prepare", str(scenario_file), str(root), str(REPO)], env=env, check=True,
                           capture_output=True, timeout=60)
            before = snapshot(root)
            out = root / ".harness" / "out.json"
            proc = subprocess.run([str(binary), "run", str(scenario_file), str(root), str(REPO), str(out)], env=env,
                                  cwd=root / "work", capture_output=True, text=True, timeout=180)
            if proc.returncode != 0 or not out.exists():
                raise RuntimeError(f"cenário {name}: lado Rust saiu {proc.returncode}\n{proc.stderr[-6000:]}")
            raw = json.loads(out.read_text(encoding="utf-8"))
            if raw.get("not_ours"):
                raise RustSideNotImplemented(f"Rust side not implemented for {route} (scenario {name}: not Claude with terminal)")
            raw["files"] = file_changes(before, snapshot(root))
            # A reconciliação da conta grava no arquivo do diário antes da criação.
            raw["diary"] = read_diary(root) + raw["diary"]
            raw["_paths"] = {"root": str(root), "repo": str(REPO), "python": sys.executable}
            return raw
        finally:
            shutil.rmtree(root, ignore_errors=True)


SIDES = {"python": run_python_side, "rust": run_rust_side}


# ── Normalização ─────────────────────────────────────────────────────────────────────────────

_UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
_HEX32 = re.compile(r"(?<![0-9a-f])[0-9a-f]{32}(?![0-9a-f])")
_ISO = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?")
_TIME_KEYS = {"ts", "created", "created_at", "createdAt", "updated", "updated_at", "trustedAt", "since",
              "started", "launched_at", "inicio", "mtime", "lastSeen", "last_used"}
_PID_KEYS = {"pid", "ppid", "pid_backend", "pane_pid", "cano_pid"}
# Do processo e da máquina (diag.recursos), nunca do pedido.
_DIARY_DROP = {"ts", "backend", "pid_backend", "inicio_backend", "req", "origem",
               "threads_backend", "memoria_total_mb", "memoria_disponivel_mb", "recursos_erro"}


class _Ids:
    """Mapa estável de valor sorteado para `<kind:N>`, na ordem da primeira aparição."""

    def __init__(self, keep: set[str]):
        self.keep, self.map, self.count = keep, {}, {}

    def __call__(self, kind: str, value: str) -> str:
        if value in self.keep:
            return value
        if value not in self.map:
            self.count[kind] = self.count.get(kind, 0) + 1
            self.map[value] = f"<{kind}:{self.count[kind]}>"
        return self.map[value]


def _scenario_literals(scenario: dict) -> set[str]:
    text = json.dumps(scenario)
    return set(_UUID.findall(text)) | set(_HEX32.findall(text))


def normalize(raw: dict, scenario: dict) -> dict:
    paths = raw.get("_paths", {})
    root, repo, python = paths.get("root"), paths.get("repo"), paths.get("python")
    sanitized_root = re.sub(r"[^A-Za-z0-9]", "-", root) if root else None
    ids = _Ids(_scenario_literals(scenario))
    pids = _Ids(set())

    def text(s: str) -> str:
        if python:
            s = s.replace(python, "<PYTHON>")
        if repo:
            s = s.replace(repo, "<REPO>")
        if root:
            s = s.replace(root, "<ROOT>")
        if sanitized_root:
            s = s.replace(sanitized_root, "<ROOT_SANITIZED>")
        s = re.sub(r"(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])", "<sha256>", s)
        s = _UUID.sub(lambda m: ids("uuid", m.group(0)), s)
        for full in _HEX32.findall(s):
            label = ids("hex", full)
            s = s.replace(full, label)
        # Prefixos de chave (`cano-<16 hex>`) seguem a chave inteira já vista.
        for value, label in list(ids.map.items()):
            if len(value) == 32 and value[:16] in s:
                s = s.replace(value[:16], label + "[:16]")
        s = re.sub(r"(cano-(?:<hex:\d+>\[:16\]|[0-9a-f]{16}))-[0-9a-f]{4}\.sock", r"\1-<rand4>.sock", s)
        s = re.sub(r"(<hex:\d+>)\.[0-9a-f]{64}", r"\1.<hmac>", s)
        s = _ISO.sub("<iso>", s)
        s = re.sub(r"\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z", "<iso-file>", s)
        return s

    def walk(value, key=None):
        if isinstance(value, dict):
            return {text(k) if isinstance(k, str) else k: walk(v, k) for k, v in value.items()}
        if isinstance(value, list):
            return [walk(v, key) for v in value]
        if isinstance(value, bool) or value is None:
            return value
        if key in _TIME_KEYS and isinstance(value, (int, float, str)):
            return "<time>"
        if key in _PID_KEYS and isinstance(value, int):
            return value if value >= FAKE_PID else pids("pid", str(value))
        if key == "ms" and isinstance(value, (int, float)):
            return "<ms>"
        if key == "operacao" and isinstance(value, str):
            return ids("op", value)
        if isinstance(value, str):
            return text(value)
        return value

    diary = []
    for row in raw.get("diary", []):
        diary.append({k: v for k, v in row.items() if k not in _DIARY_DROP})
    ordered = {k: raw.get(k) for k in ("steps", "subprocesses", "probes", "rust_bridge", "runtime_requests",
                                       "signals", "network", "tmux_sessions")}
    files = raw.get("files", {})
    ordered["records"] = {p: v for p, v in files.items() if "/home/.hangar/" in p}
    ordered["files"] = {p: v for p, v in files.items() if "/home/.hangar/" not in p}
    ordered["diary"] = diary
    walk(ordered)   # 1ª passada: numera cada id pela primeira aparição; a 2ª já acha os prefixos
    return walk(ordered)


# ── Comparação ───────────────────────────────────────────────────────────────────────────────

def compare(expected, actual, path: str = "$") -> list[str]:
    """Igualdade exata depois da normalização; devolve cada diferença com o caminho."""
    if type(expected) is not type(actual):
        return [f"{path}: esperado {json.dumps(expected, ensure_ascii=False)[:300]}, "
                f"veio {json.dumps(actual, ensure_ascii=False)[:300]}"]
    if isinstance(expected, dict):
        diffs = []
        for k in sorted(set(expected) | set(actual), key=str):
            if k not in actual:
                diffs.append(f"{path}.{k}: faltou")
            elif k not in expected:
                diffs.append(f"{path}.{k}: sobrou {json.dumps(actual[k], ensure_ascii=False)[:300]}")
            else:
                diffs += compare(expected[k], actual[k], f"{path}.{k}")
        return diffs
    if isinstance(expected, list):
        diffs = [f"{path}: {len(expected)} itens esperados, vieram {len(actual)}"] if len(expected) != len(actual) else []
        for i, (e, a) in enumerate(zip(expected, actual)):
            diffs += compare(e, a, f"{path}[{i}]")
        return diffs
    return [] if expected == actual else [f"{path}: esperado {expected!r}, veio {actual!r}"]


# ── Fixtures ─────────────────────────────────────────────────────────────────────────────────

def fixture_path(name: str) -> Path:
    return FIXTURES / f"{name}.json"


def build_fixture(scenario: dict, result: dict) -> dict:
    return {"_meta": {"source_commit": SOURCE_COMMIT,
                      "generated_by": GENERATED_BY.format(name=scenario["name"]),
                      "note": "gerada executando o Python; nunca editar à mão"},
            "scenario": scenario, "result": result}


def _parallel(fn, items):
    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(max_workers=min(6, os.cpu_count() or 2)) as pool:
        return list(pool.map(fn, items))


def generate(scenarios: list[dict]) -> list[tuple[str, str | None]]:
    FIXTURES.mkdir(exist_ok=True)

    def one(sc):
        try:
            result = normalize(run_python_side(sc), sc)
        except Exception as exc:   # noqa: BLE001 — cada cenário falha sozinho e o erro sai no fim
            return sc["name"], f"{type(exc).__name__}: {exc}"
        fixture_path(sc["name"]).write_text(
            json.dumps(build_fixture(sc, result), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        return sc["name"], None
    return _parallel(one, scenarios)


# ── Diferenças esperadas do lado Rust ───────────────────────────────────────────────────────
# Cada conserto do `suspeitas.md` (D-011) ou decisão (D-xxx) que muda o que a fixture prende tem uma
# regra: ela desfaz o conserto na saída crua do Rust (ou tira da fixture o que é só do Python) para o
# resto ser comparado por igualdade. Regra que se aplica e não acha o conserto é diferença.

# `terminal::NOT_INHERITED` e os segredos do servidor: o que o S-63 tira do pane com `env -u`.
_NOT_INHERITED = {
    "CLAUDE_CODE_SUBAGENT_MODEL", "CLAUDE_CODE_ENABLE_FUNCTION_HOOKS", "HANGAR_PLUGIN_URL", "HANGAR_PLUGIN_TOKEN",
    "HANGAR_CLAUDE_SETTINGS", "HANGAR_JEV", "TYPESAFE_API_KEY", "JEV_ENDPOINT", "JEV_MODEL", "JEV_TEXTO_BASE_URL",
    "JEV_TEXTO_API_KEY", "JEV_TEXTO_MODELO", "JEV_TEXTO_CMD", "JEV_AUX_BASE_URL", "JEV_AUX_API_KEY", "JEV_AUX_MODELO",
    "JEV_AUX_ESFORCO", "CP_ENGINE", "CP_ENGINE_ACCOUNT", "CP_ENGINE_CREDENTIAL_ID", "CP_ENGINE_ACCOUNT_BASE_URL",
    "CP_ENGINE_SERVICE_TIER", "CLAUDE_CODE_MAX_CONTEXT_TOKENS", "HANGAR_ORQ_READ_ONLY", "HANGAR_CANO_KEY",
    "HANGAR_CANO_OWNER", "CP_PI_SESSION", "OMP_PROFILE", "CP_AUTH_TOKEN", "HANGAR_INTERNAL_SECRET", "HANGAR_RUNTIME_INSTANCE",
    "ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_CUSTOM_HEADERS", "ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL", "ANTHROPIC_DEFAULT_FABLE_MODEL", "CLAUDE_CODE_EXTRA_BODY",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW", "CLAUDE_CODE_MAX_OUTPUT_TOKENS", "CLAUDE_CODE_DISABLE_BUNDLED_SKILLS",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS", "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING", "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
    "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING", "ENABLE_TOOL_SEARCH", "DISABLE_PROMPT_CACHING",
}


def _new_sessions(raw: dict) -> list[dict]:
    return [p for p in raw.get("subprocesses", []) if p.get("prog") == "tmux" and p.get("argv", [])[:1] == ["new-session"]]


def _rule_s63(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """S-63: o pane sai sem o que outra sessão deixou no servidor tmux (`env -u` de cada variável)."""
    found = False

    def undo(command: str) -> str:
        nonlocal found
        words, out, i = command.split(" "), [], 0
        while i < len(words):
            if words[i] == "-u" and i + 1 < len(words) and words[i + 1] in _NOT_INHERITED:
                found, i = True, i + 2
                continue
            out.append(words[i])
            i += 1
        text = " ".join(out)
        rest = text.removeprefix("exec env ")
        return "exec " + rest if rest != text and not rest.startswith("-u ") else text
    created = _new_sessions(raw)
    for name in {p["argv"][p["argv"].index("-s") + 1] for p in created}:
        if name in raw.get("tmux_sessions", {}):
            raw["tmux_sessions"][name]["command"] = undo(raw["tmux_sessions"][name]["command"])
    # O conserto conta pelo comando do `new-session`.
    found = False
    for p in created:
        p["argv"][-1] = undo(p["argv"][-1])
    return found if created else None


def _rule_no_default_shell(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """O comando vai em `/bin/sh -c` separados: o tmux roda direto, sem o `default-shell` do usuário."""
    created, found = _new_sessions(raw), False
    for p in created:
        if p["argv"][-3:-1] == ["/bin/sh", "-c"]:
            del p["argv"][-3:-1]
            found = True
    return found if created else None


def _rule_session_key(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """Matriz, item 2 (D-012): `CP_SESSION_KEY` em toda sessão, também no Claude com terminal."""
    created = _new_sessions(raw)
    found = False
    for p in created:
        argv = p["argv"]
        at = next((i for i in range(len(argv) - 1) if argv[i] == "-e" and argv[i + 1].startswith("CP_SESSION_KEY=")), None)
        if at is not None:
            del argv[at:at + 2]
            found = True
    for s in raw.get("tmux_sessions", {}).values():
        s.get("env", {}).pop("CP_SESSION_KEY", None)
    return found if created else None


def _rule_display_env(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """`display_env`: o pane nasce com todas as variáveis de tela, não só o `WAYLAND_DISPLAY` (o
    Computer Use recusa sem `HYPRLAND_INSTANCE_SIGNATURE`); no harness aparece o `XDG_RUNTIME_DIR`."""
    created = _new_sessions(raw)
    found = False
    for p in created:
        argv, kept = p["argv"], []
        i = 0
        while i < len(argv):
            if argv[i] == "-e" and i + 1 < len(argv) and argv[i + 1].split("=", 1)[0] in (
                    "DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "XDG_RUNTIME_DIR"):
                found, i = True, i + 2
                continue
            kept.append(argv[i])
            i += 1
        p["argv"] = kept
    for s in raw.get("tmux_sessions", {}).values():
        for key in ("DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "XDG_RUNTIME_DIR"):
            s.get("env", {}).pop(key, None)
    return found if created else None


def _rule_spec_record(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """Task 3 (D-012, item 3): a sessão nascida deixa o spec em `~/.hangar/session-specs/<key>.json`."""
    created = [p for p in _new_sessions(raw) if p.get("rc") == 0]
    records = [p for p in raw.get("files", {}) if "/home/.hangar/session-specs/" in p]
    for p in records:
        del raw["files"][p]
    return bool(records) if created else None


def _rule_s15_diary(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """S-15/S-35: a recusa do lançamento vai ao diário com a etapa, o código e o stderr resumido."""
    refused = [e for e in raw.get("diary", []) if e.get("evento") == "sessao.criar_recusada" and e.get("nivel") == "erro"]
    for e in refused:
        e["detalhe"] = e.pop("codigo", None)
        e.pop("etapa", None)
    return all(e["detalhe"] for e in refused) if refused else None


def _rule_only_python(expected: dict, raw: dict, scenario: dict) -> None:
    """Do Python e não da abertura: o `api.servidor` do middleware HTTP dele (a rota é do Rust), as
    consultas ao tmux dele (`mux.comando`), a sonda `claude --version` (o Rust lê o `plugin-dir` que o
    servidor publica, como o terminal), o PATH do shell de login (Task 3: o do servidor), o `bwrap`
    provado duas vezes (o Rust prova uma) e o nome da exceção no fim do diário (o Rust põe o código)."""
    expected["diary"] = [e for e in expected.get("diary", []) if e.get("evento") not in ("api.servidor", "mux.comando")]
    expected["probes"] = [p for p in expected.get("probes", []) if p.get("prog") not in ("claude", "sh-login")]
    bwrap = [p for p in expected["probes"] if p.get("prog") == "bwrap"]
    if len(bwrap) == 2 and bwrap[0] == bwrap[1]:
        expected["probes"].remove(bwrap[1])
    for side in (expected, raw):
        for e in side.get("diary", []):
            if e.get("evento") == "sessao.criar.fim":
                e.pop("erro_tipo", None)
                e.pop("causa_tipo", None)
    return None


# Recusas que o Python dava dentro do `registry.create`, com a trava da conta já tomada; o Rust as dá
# na resolução.
_REFUSED_BEFORE_LOCK = ("sessao sem terminal so vale para provider claude ou codex", "Nome de perfil OMP inválido",
                        "permission_mode: use um de Ask for approval, Approve for me, Full Access")


def _rule_name_before_lock(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """Ordem: o nome inválido ou em uso (e o que o Python recusava dentro do `registry.create`) é
    recusado antes de tocar a conta; o Python já tinha tomado a trava dela e deixava o arquivo vazio."""
    detail = (raw.get("steps") or [{}])[0].get("body", {}).get("detail")
    msg = str(detail.get("msg", "")) if isinstance(detail, dict) else ""
    refused = isinstance(detail, dict) and (detail.get("code") == "erro_nome_em_uso" or msg.startswith("nome invalido")
                                            or msg in _REFUSED_BEFORE_LOCK)
    if not refused:
        return None
    locks = [p for p in expected.get("records", {}) if "/home/.hangar/account-locks/" in p]
    for p in locks:
        del expected["records"][p]
    return not any("/home/.hangar/account-locks/" in p for p in raw.get("files", {}))


def _rule_s40(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """S-40: o prompt inicial do Claude com terminal é entregue: o posicional no fim do comando."""
    prompt = next((st.get("json", {}).get("initial_prompt") for st in scenario["steps"] if "json" in st), None)
    created = [p for p in _new_sessions(raw) if "CP_PROVIDER=claude" in p["argv"]]
    if not prompt or not created:
        return None
    tail = (" -- " if prompt.startswith("-") else " ") + shlex.quote(prompt)
    found = False
    for p in created:
        if p["argv"][-1].endswith(tail):
            p["argv"][-1], found = p["argv"][-1][:-len(tail)], True
    for s in raw.get("tmux_sessions", {}).values():
        if s.get("env", {}).get("CP_PROVIDER") == "claude":
            s["command"] = s["command"].removesuffix(tail)
    return found


_CODEX_POLICY = re.compile(r" --approval-policy \S+ --sandbox \S+")


def _rule_codex_policy(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """Matriz, item 17 (S-05): o Codex com terminal nasce com `--approval-policy`/`--sandbox` do modo
    do spec; o Python deixava o lançador cair no padrão dele (Full Access) sem dizer."""
    created = [p for p in _new_sessions(raw) if "CP_PROVIDER=codex" in p["argv"]]
    found = bool(created) and all(_CODEX_POLICY.search(p["argv"][-1]) for p in created)
    for p in created:
        p["argv"][-1] = _CODEX_POLICY.sub("", p["argv"][-1], count=1)
    for s in raw.get("tmux_sessions", {}).values():
        if s.get("env", {}).get("CP_PROVIDER") == "codex":
            s["command"] = _CODEX_POLICY.sub("", s["command"], count=1)
    return found if created else None


def _rule_codex_login_lock(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """A conta Codex lida pelo Rust (login e pasta) cria o arquivo da trava dela, vazio; o Python a lia
    pela ponte do Rust, que no harness é falsa. Só conta na recusa, quando a criação não chegou à trava."""
    if not any(st.get("json", {}).get("provider") == "codex" for st in scenario["steps"]):
        return None
    if all(st.get("status", 200) < 400 for st in raw.get("steps", [])):
        return None
    locks = [p for p in raw.get("files", {}) if "/home/.hangar/account-locks/" in p]
    for p in locks:
        del raw["files"][p]
    return True if locks else None


def _headless_request(scenario: dict) -> dict | None:
    body = next((st.get("json") for st in scenario["steps"] if "json" in st), None) or {}
    return body if body.get("headless") is True and body.get("provider", "claude") == "claude" else None


def _rule_s40_headless(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """S-40 sem terminal: o Python recusava o prompt inicial; o Rust cria a sessão com ele já na fila
    durável (antes de responder) e a acorda. A recusa da fixture não tem o que comparar: a regra só
    confere a criação, a fila e o acordar."""
    body = _headless_request(scenario)
    if not body or not body.get("initial_prompt") or body.get("read_only"):
        return None
    status = (raw.get("steps") or [{}])[0].get("status")
    woke = any(r.get("op") == "claude_headless.acordar" for r in raw.get("runtime_requests", []))
    queued = any("/.hangar-queue/runtime/" in path and body["initial_prompt"] in json.dumps(change, ensure_ascii=False)
                 for path, change in (raw.get("files") or {}).items())
    raw["_replaced"] = True
    return status == 200 and woke and queued


def _rule_d001_inherit(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """D-001: `inherit` é campo novo do pedido; o Python de 601bc62a0 recusava o corpo com 422. A recusa
    da fixture não tem o que comparar: a regra só confere que o Rust abriu."""
    body = next((st.get("json") for st in scenario["steps"] if "json" in st), None) or {}
    if "inherit" not in body or (expected.get("steps") or [{}])[0].get("status") != 422:
        return None
    raw["_replaced"] = True
    return (raw.get("steps") or [{}])[0].get("status") == 200


def _rule_headless_read_only(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """Só leitura sem terminal é recusado na resolução, antes de provar o `bwrap` e de tocar a conta (o
    Python provava e travava antes de recusar); a frase deixa de falar do prompt inicial (S-40)."""
    body = _headless_request(scenario)
    if not body or not body.get("read_only"):
        return None
    detail = (raw.get("steps") or [{}])[0].get("body", {}).get("detail")
    if not isinstance(detail, dict) or detail.get("msg") != "sessao sem terminal nao aceita read_only":
        return False
    detail["msg"] = "sessao sem terminal nao aceita read_only nem prompt inicial"
    expected["probes"] = [p for p in expected.get("probes", []) if p.get("prog") != "bwrap"]
    for p in [p for p in expected.get("records", {}) if "/home/.hangar/account-locks/" in p]:
        del expected["records"][p]
    return not any(p.get("prog") == "bwrap" for p in raw.get("probes", []))


def _created_agent(raw: dict) -> list[tuple[dict, str]]:
    """Os `new-session` de Pi, omp e Kimi, com o provedor."""
    out = []
    for p in _new_sessions(raw):
        provider = next((a.split("=", 1)[1] for a in p["argv"] if a.startswith("CP_PROVIDER=")), None)
        if provider in ("pi", "omp", "kimi"):
            out.append((p, provider))
    return out


def _rule_skills_sync(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """Matriz, item 16, com a T-016: Pi e Kimi abrem com as skills sincronizadas (`skill_bridge.py` com
    o Python da venv do backend); o Python refazia a ponte no próprio processo. O omp fica de fora,
    como lá (RET-078): a ponte nele aparece como subprocesso a mais."""
    if not any(prov in ("pi", "kimi") for _, prov in _created_agent(raw)):
        return None
    def bridge(p: dict) -> bool:
        return str(p.get("prog", "")).startswith("python") and str((p.get("argv") or [""])[0]).endswith("skill_bridge.py")
    synced = [p for p in raw.get("subprocesses", []) if bridge(p)]
    raw["subprocesses"] = [p for p in raw.get("subprocesses", []) if not bridge(p)]
    return bool(synced)


def _rule_pi_session_env(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """S-58/S-73: `CP_PI_SESSION` (Pi e omp) e `OMP_PROFILE` vão por `-e`, também no Windows e no Pi
    aberto pelo app; o Python punha `env CP_PI_SESSION=… OMP_PROFILE=…` no comando do omp."""
    created = [(p, prov) for p, prov in _created_agent(raw) if prov in ("pi", "omp")]
    found = True
    for p, provider in created:
        argv, kept, moved = p["argv"], [], []
        i = 0
        while i < len(argv):
            if argv[i] == "-e" and i + 1 < len(argv) and argv[i + 1].split("=", 1)[0] in ("CP_PI_SESSION", "OMP_PROFILE"):
                moved.append(argv[i + 1])
                i += 2
                continue
            kept.append(argv[i])
            i += 1
        sid = next((m.split("=", 1)[1] for m in moved if m.startswith("CP_PI_SESSION=")), None)
        command = kept[-1]
        found = found and bool(sid) and (f"--session-id {sid}" in command or f"_{sid}.jsonl" in command)
        if provider == "omp" and moved:
            words = command.split(" ")
            at = words.index("omp")
            command = " ".join(words[:at] + ["env", *moved] + words[at:])
        kept[-1] = command
        p["argv"] = kept
        name = argv[argv.index("-s") + 1]
        session = raw.get("tmux_sessions", {}).get(name)
        if session:
            session["command"] = command
            for m in moved:
                session.get("env", {}).pop(m.split("=", 1)[0], None)
    return found if created else None


def _archive_step(scenario: dict) -> dict | None:
    return next((st for st in scenario["steps"] if _ARCHIVE_RESUME.match(st.get("path", ""))), None)


def _rule_archive_headless(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """Matriz, itens 21 e 24 (S-08): o retomar do Arquivo passa pela resolução da criação e segue o padrão
    sem terminal do servidor; o Python abria sempre com terminal. A sessão nasce sem terminal na conversa
    retomada (o sidecar com o id dela) e é acordada; o pane da fixture não tem o que comparar."""
    step = _archive_step(scenario)
    got = (raw.get("steps") or [{}])[0]
    if step is None or (expected.get("steps") or [{}])[0].get("status") != 200 or got.get("body", {}).get("headless") is not True:
        return None
    sid = step["path"].rsplit("/", 2)[-2]
    name = got["body"]["name"]
    sidecar = next((change for path, change in (raw.get("files") or {}).items()
                    if path.endswith(f"/home/.hangar/claude-headless/{name}.json")), None)
    raw["_replaced"] = True
    woke = any(r.get("op") == "claude_headless.acordar" for r in raw.get("runtime_requests", []))
    return got.get("status") == 200 and woke and sid in json.dumps(sidecar or {})


_RESUME_MODE = re.compile(r" --permission-mode \S+")


def _rule_resume_mode(expected: dict, raw: dict, scenario: dict) -> bool | None:
    """Matriz, item 17: o modo de permissão vai explícito também no `claude --resume` (o Python o deixava
    de fora e a sessão voltava no padrão da conta)."""
    created = [p for p in _new_sessions(raw) if "CP_PROVIDER=claude" in p["argv"] and " --resume " in p["argv"][-1]]
    if not created:
        return None
    found = all(_RESUME_MODE.search(p["argv"][-1]) for p in created)
    for p in created:
        p["argv"][-1] = _RESUME_MODE.sub("", p["argv"][-1], count=1)
    for s in raw.get("tmux_sessions", {}).values():
        if " --resume " in s.get("command", ""):
            s["command"] = _RESUME_MODE.sub("", s["command"], count=1)
    return found


RUST_DIFFERENCES = (
    ("matriz itens 21 e 24 (Arquivo segue o padrão sem terminal)", _rule_archive_headless),
    ("matriz item 17 (modo explícito no --resume)", _rule_resume_mode),
    ("S-40", _rule_s40),
    ("matriz item 17 (política explícita do Codex)", _rule_codex_policy),
    ("trava da conta Codex lida pelo Rust", _rule_codex_login_lock),
    ("S-40 (sem terminal)", _rule_s40_headless),
    ("D-001 (campo inherit)", _rule_d001_inherit),
    ("só leitura sem terminal recusado antes da conta", _rule_headless_read_only),
    ("ordem: nome antes da trava da conta", _rule_name_before_lock),
    ("S-63", _rule_s63),
    ("sem o default-shell do usuário", _rule_no_default_shell),
    ("S-58/S-73 (CP_PI_SESSION e OMP_PROFILE por -e)", _rule_pi_session_env),
    ("matriz item 16 (skills sincronizadas)", _rule_skills_sync),
    ("matriz item 2 (CP_SESSION_KEY)", _rule_session_key),
    ("variáveis de tela (display_env)", _rule_display_env),
    ("registro do spec (Task 3)", _rule_spec_record),
    ("S-15/S-35 (diário do lançamento)", _rule_s15_diary),
    ("só do Python", _rule_only_python),
)


def apply_rust_differences(expected: dict, raw: dict, scenario: dict) -> tuple[dict, dict, list[str]]:
    """Fixture (normalizada) e saída crua do Rust com as diferenças esperadas desfeitas."""
    expected, raw = copy.deepcopy(expected), copy.deepcopy(raw)
    missing = []
    for rule, fn in RUST_DIFFERENCES:
        if fn(expected, raw, scenario) is False:
            missing.append(f"$: a diferença esperada '{rule}' não apareceu na saída do Rust")
    return expected, raw, missing


def diff(side: str, names: list[str] | None = None) -> dict[str, list[str]]:
    """Roda cada fixture do lado pedido a partir do mesmo cenário gravado e compara."""
    fixtures = [json.loads(f.read_text(encoding="utf-8")) for f in sorted(FIXTURES.glob("*.json"))]
    chosen = [fx for fx in fixtures if not names or fx["scenario"]["name"] in names]
    if names and len(chosen) != len(set(names)):
        missing = set(names) - {fx["scenario"]["name"] for fx in chosen}
        raise SystemExit(f"fixture inexistente: {sorted(missing)}")

    def one(fx):
        sc = fx["scenario"]
        if side == "python":
            return sc["name"], compare(fx["result"], normalize(run_python_side(sc), sc))
        try:
            raw = run_rust_side(sc)
        except RustSideNotImplemented as exc:
            return sc["name"], [f"$: {exc}"]
        expected, raw, missing = apply_rust_differences(fx["result"], raw, sc)
        if raw.pop("_replaced", False):
            return sc["name"], missing
        return sc["name"], missing + compare(expected, normalize(raw, sc))
    return dict(_parallel(one, chosen))


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    g = sub.add_parser("generate")
    g.add_argument("--group", help="só os cenários cujo grupo começa com isto (a, b, c, d)")
    g.add_argument("names", nargs="*")
    d = sub.add_parser("diff")
    d.add_argument("--side", choices=sorted(SIDES), default="python")
    d.add_argument("names", nargs="*")
    r = sub.add_parser("raw")
    r.add_argument("name")
    args = ap.parse_args(argv)
    sys.path.insert(0, str(HERE))
    from session_open_scenarios import SCENARIOS
    by_name = {s["name"]: s for s in SCENARIOS}
    if len(by_name) != len(SCENARIOS):
        raise SystemExit("nome de cenário repetido")
    if args.cmd == "generate":
        chosen = [by_name[n] for n in args.names] if args.names else SCENARIOS
        if args.group:
            chosen = [s for s in chosen if s["group"].startswith(args.group)]
        elif not args.names:
            # Geração completa: fixture de cenário que saiu da lista sai junto.
            for stale in set(FIXTURES.glob("*.json")) - {fixture_path(n) for n in by_name}:
                stale.unlink()
                print("removida", stale.relative_to(REPO))
        failed = 0
        for n, error in generate(chosen):
            print(f"FALHOU {n}: {error}" if error else f"gravada {fixture_path(n).relative_to(REPO)}")
            failed += bool(error)
        return 1 if failed else 0
    if args.cmd == "raw":
        sc = by_name[args.name]
        print(json.dumps(normalize(run_python_side(sc), sc), ensure_ascii=False, indent=2))
        return 0
    failures = 0
    for name, diffs in diff(args.side, args.names or None).items():
        print(("ok    " if not diffs else "DIFERE ") + name)
        for line in diffs:
            print("   ", line)
        failures += bool(diffs)
    return 1 if failures else 0


# ══ Processo filho: o Python do commit fixo, com as bordas interceptadas ═════════════════════

def child_main(scenario_path: str, out_path: str, backend: str) -> None:
    import asyncio
    import io
    import signal
    import socket
    import threading

    scenario = json.loads(Path(scenario_path).read_text(encoding="utf-8"))
    root = Path(os.environ["HOME"]).parent
    scenario = {**scenario, "state": subst(scenario.get("state", {}), root),
                "steps": subst(scenario["steps"], root), "fakes": subst(scenario.get("fakes", {}), root)}
    fakes = scenario.get("fakes", {})
    base = dict(os.environ)
    rec: dict = {"steps": [], "subprocesses": [], "probes": [], "rust_bridge": [], "runtime_requests": [],
                 "signals": [], "network": []}
    lock = threading.Lock()
    tmux_lock = threading.RLock()

    def env_delta(env) -> dict:
        if env is None:
            return {}
        env = dict(env)
        added = {k: v for k, v in sorted(env.items()) if base.get(k) != v and k not in SHELL_ENV_NOISE}
        removed = sorted(k for k in base if k not in env)
        return {"set": added, "unset": removed} if removed else {"set": added}

    # ── processos falsos: um /proc de mentira em disco, lido pelo procinfo de verdade ──
    procs: dict[int, dict] = {}
    next_pid = [FAKE_PID + 1000]
    proc_dir = root / ".harness" / "proc"

    def add_proc(argv, environ, ppid=1, alive=True, pid=None, label=None, unreadable=False) -> int:
        if pid is None:
            pid, next_pid[0] = next_pid[0], next_pid[0] + 1
        procs[pid] = {"argv": list(argv), "label": label or (os.path.basename(argv[0]) if argv else "?"),
                      "alive": alive}
        if alive:
            d = proc_dir / str(pid)
            (d / "fd").mkdir(parents=True, exist_ok=True)
            (d / "cmdline").write_bytes(b"".join(a.encode() + b"\0" for a in argv))
            env = d / "environ"
            env.write_bytes(b"".join(f"{k}={v}".encode() + b"\0" for k, v in environ.items()))
            comm = os.path.basename(argv[0])[:15] if argv else "sh"
            (d / "stat").write_text(f"{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 100 0 0\n")
            if unreadable:
                env.chmod(0)
        return pid

    def kill_proc(pid) -> None:
        if pid in procs:
            procs[pid]["alive"] = False
            shutil.rmtree(proc_dir / str(pid), ignore_errors=True)

    for p in scenario["state"].get("processes", []):
        env = {**base, **p.get("env", {})}
        for k in p.get("unset", []):
            env.pop(k, None)
        add_proc(p["argv"], env, p.get("ppid", 1), p.get("alive", True), p["pid"], p.get("label"),
                 p.get("environ_unreadable", False))

    # ── tmux falso ──
    tmux_sessions: dict[str, dict] = {}
    pane_seq = [0]

    def new_pane(command: str, env: dict, unreadable: bool = False) -> dict:
        argv = _pane_argv(command)
        unset = _pane_unset(command)
        environ = {**base, **env}
        for k in unset:
            environ.pop(k, None)
        pid = add_proc(argv, environ, unreadable=unreadable)
        pane = {"id": f"%{pane_seq[0]}", "pid": pid}
        pane_seq[0] += 1
        return pane

    for s in scenario["state"].get("tmux_sessions", []):
        pane = new_pane(s["command"], s.get("env", {}), s.get("environ_unreadable", False))
        tmux_sessions[s["name"]] = {"cwd": s["cwd"], "env": s.get("env", {}), "command": s["command"],
                                    "panes": [pane], "options": dict(s.get("options", {})),
                                    "screen": s.get("screen")}

    # Tela de Claude ocioso (captura real do repositório) para todo pane sem tela própria no cenário.
    idle_text = (Path(backend) / "tests" / "fixtures" / "pane_idle.txt").read_text(encoding="utf-8")

    def target_session(t: str) -> str | None:
        t = t.lstrip("=")
        if t.startswith("%"):
            return next((n for n, s in tmux_sessions.items() if any(p["id"] == t for p in s["panes"])), None)
        name = t.split(":")[0].split(".")[0]
        return name if name in tmux_sessions else None

    def fmt(template: str, name: str, pane: dict) -> str:
        s = tmux_sessions[name]
        proc = procs.get(pane["pid"], {})
        values = {"session_name": name, "pane_current_path": s["cwd"], "session_path": s["cwd"],
                  "pane_active": "1", "pane_pid": str(pane["pid"]), "pane_id": pane["id"],
                  "session_created": "1760000000", "window_index": "0", "pane_index": "0",
                  "pane_dead": "0", "session_id": "$0", "session_attached": "0",
                  "pane_current_command": os.path.basename(proc.get("argv", ["sh"])[0])}
        def sub(m):
            key = m.group(1)
            if key in values:
                return values[key]
            if key.startswith("@"):
                return s["options"].get(key, "")
            return s["env"].get(key, "")
        return re.sub(r"#\{([^}]+)\}", sub, template)

    def opt(args, flag):
        return args[args.index(flag) + 1] if flag in args else None

    def tmux(args: list[str]) -> tuple[int, str, str]:
        cmd = args[0] if args else ""
        if cmd == "has-session":
            name = target_session(opt(args, "-t") or "")
            return (0, "", "") if name else (1, "", f"can't find session: {(opt(args, '-t') or '').lstrip('=')}\n")
        if cmd == "new-session":
            name = opt(args, "-s")
            force = fakes.get("tmux_new_session")
            if force:
                return force.get("rc", 1), force.get("stdout", ""), force.get("stderr", "")
            if name in tmux_sessions:
                return 1, "", f"duplicate session: {name}\n"
            env, i = {}, 0
            while i < len(args):
                if args[i] == "-e":
                    k, _, v = args[i + 1].partition("=")
                    env[k] = v
                    i += 2
                else:
                    i += 1
            command = args[-1]
            tmux_sessions[name] = {"cwd": opt(args, "-c") or "", "env": env, "command": command,
                                   "panes": [new_pane(command, env)], "options": {}, "screen": None}
            out = ""
            if "-P" in args:
                out = fmt(opt(args, "-F") or "#{session_name}:", name, tmux_sessions[name]["panes"][0]) + "\n"
            return 0, out, ""
        if cmd in ("list-sessions", "ls"):
            f = opt(args, "-F") or "#{session_name}"
            lines = [fmt(f, n, s["panes"][0]) for n, s in tmux_sessions.items()]
            return (0, "\n".join(lines) + "\n", "") if lines else (1, "", "no server running on <sock>\n")
        if cmd == "list-panes":
            f = opt(args, "-F") or "#{pane_id}"
            if "-a" in args:
                if not tmux_sessions:
                    return 1, "", "no server running on <sock>\n"
                lines = [fmt(f, n, p) for n, s in tmux_sessions.items() for p in s["panes"]]
            else:
                name = target_session(opt(args, "-t") or "")
                if name is None:
                    return 1, "", "can't find session\n"
                lines = [fmt(f, name, p) for p in tmux_sessions[name]["panes"]]
            return 0, "\n".join(lines) + ("\n" if lines else ""), ""
        if cmd in ("display-message", "display"):
            name = target_session(opt(args, "-t") or "")
            if name is None:
                return 1, "", "can't find session\n"
            return 0, fmt(args[-1], name, tmux_sessions[name]["panes"][0]) + "\n", ""
        if cmd == "show-options":
            name = target_session(opt(args, "-t") or "")
            value = tmux_sessions[name]["options"].get(args[-1]) if name else None
            return (0, value + "\n", "") if value is not None else (1, "", "invalid option\n")
        if cmd == "set-option":
            name = target_session(opt(args, "-t") or "")
            if name is None:
                return 1, "", "can't find session\n"
            tmux_sessions[name]["options"][args[-2]] = args[-1]
            return 0, "", ""
        if cmd == "capture-pane":
            name = target_session(opt(args, "-t") or "")
            if name is None:
                return 1, "", "can't find pane\n"
            return 0, tmux_sessions[name].get("screen") or idle_text, ""
        if cmd == "kill-session":
            name = target_session(opt(args, "-t") or "")
            if name is None:
                return 1, "", "can't find session\n"
            for p in tmux_sessions.pop(name)["panes"]:
                kill_proc(p["pid"])
            return 0, "", ""
        if cmd == "rename-session":
            old = target_session(opt(args, "-t") or "")
            if old is None:
                return 1, "", "can't find session\n"
            tmux_sessions[args[-1]] = tmux_sessions.pop(old)
            return 0, "", ""
        return 0, "", ""

    # ── borda do subprocess ──
    real_popen = subprocess.Popen

    def rule_for(prog: str, argv: list[str]):
        for r in fakes.get("programs", []) + DEFAULT_PROGRAM_RULES:
            if r["prog"] == prog and argv[:len(r.get("args", []))] == r.get("args", []):
                return r
        return None

    def handle(args, env, cwd) -> tuple[int, str, str]:
        argv = [str(a) for a in args]
        scoped = False
        if os.path.basename(argv[0]) == "systemd-run":
            # Escopo transiente real nunca: só o comando de dentro é atendido.
            scoped = True
            argv = argv[argv.index("--") + 1:] if "--" in argv else []
            if not argv:
                return 0, "", ""
        prog = os.path.basename(argv[0])
        if prog == "true":
            return 0, "", ""
        entry = {"prog": prog, "argv": argv[1:], "cwd": str(cwd) if cwd else None, "env": env_delta(env)}
        if scoped:
            entry["scope"] = "systemd-run --user --scope --collect -q"
        if prog == "tmux":
            with tmux_lock:
                rc, out, err = tmux(argv[1:])
            entry["rc"] = rc
            if argv[1] not in TMUX_QUERIES:
                entry.pop("env")
                with lock:
                    rec["subprocesses"].append(entry)
            return rc, out, err
        r = rule_for(prog, argv[1:])
        rc, out, err = (r.get("rc", 0), r.get("stdout", ""), r.get("stderr", "")) if r else (0, "", "")
        if prog == "sh-login":
            out = base["PATH"] + "\n"
        entry["rc"] = rc
        probe = bool(r) or prog == "sh-login"
        if probe:
            entry = {k: entry[k] for k in ("prog", "argv", "rc")}
        with lock:
            (rec["probes"] if probe else rec["subprocesses"]).append(entry)
        return rc, out, err

    class FakePopen(real_popen):
        _child_created = False

        def __new__(cls, args, *a, **kw):
            seq = [args] if isinstance(args, (str, bytes)) else list(args)
            if kw.get("shell"):
                seq = ["/bin/sh", "-c", args]
            if os.path.basename(str(seq[0])) == "git":
                cwd = str(kw.get("cwd") or os.getcwd())
                inside = cwd.startswith(str(root)) or any(str(x).startswith(str(root)) for x in seq)
                if not inside:
                    raise PermissionError(f"session-open harness: git fora da raiz {seq}")
                return real_popen(args, *a, **kw)
            return object.__new__(cls)

        def __init__(self, args, bufsize=-1, executable=None, stdin=None, stdout=None, stderr=None,
                     preexec_fn=None, close_fds=True, shell=False, cwd=None, env=None, universal_newlines=None,
                     startupinfo=None, creationflags=0, restore_signals=True, start_new_session=False,
                     pass_fds=(), *, encoding=None, errors=None, text=None, **kw):
            seq = ["/bin/sh", "-c", args] if shell else ([args] if isinstance(args, (str, bytes)) else list(args))
            self.args = args
            self.pid = next_pid[0]
            next_pid[0] += 1
            self.returncode, out, err = handle(seq, env if env is not None else os.environ, cwd)
            self._text = bool(text or universal_newlines or encoding or errors)
            self._out = out if self._text else out.encode()
            self._err = err if self._text else err.encode()
            mk = (lambda v: io.StringIO(v)) if self._text else (lambda v: io.BytesIO(v))
            self.stdout = mk(self._out) if stdout == subprocess.PIPE else None
            self.stderr = mk(self._err) if stderr == subprocess.PIPE else None
            self.stdin = None

        def communicate(self, input=None, timeout=None):
            return (self._out if self.stdout is not None else None, self._err if self.stderr is not None else None)

        def wait(self, timeout=None):
            return self.returncode

        def poll(self):
            return self.returncode

        def kill(self):
            pass

        terminate = kill

        def send_signal(self, sig):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def __del__(self):
            pass

    subprocess.Popen = FakePopen

    class FakeAsyncProcess:
        def __init__(self, pid):
            self.pid, self.returncode, self.stdin, self.stdout, self.stderr = pid, None, None, None, None

        async def wait(self):
            await asyncio.Event().wait()

        async def communicate(self, input=None):
            await asyncio.Event().wait()

        def kill(self):
            pass

        terminate = kill

        def send_signal(self, sig):
            pass

    async def fake_exec(program, *args, cwd=None, env=None, **kw):
        argv = [str(program), *map(str, args)]
        scoped = os.path.basename(argv[0]) == "systemd-run"
        inner = argv[argv.index("--") + 1:] if scoped and "--" in argv else argv
        entry = {"prog": os.path.basename(inner[0]), "argv": inner[1:], "cwd": str(cwd) if cwd else None,
                 "env": env_delta(env if env is not None else os.environ), "launch": "asyncio.create_subprocess_exec"}
        if scoped:
            entry["scope"] = "systemd-run --user --scope --collect -q"
        pid = add_proc(inner, env or os.environ, label=entry["prog"])
        with lock:
            rec["subprocesses"].append(entry)
        return FakeAsyncProcess(pid)

    async def fake_shell(cmd, **kw):
        return await fake_exec("/bin/sh", "-c", cmd, **kw)

    asyncio.create_subprocess_exec = fake_exec
    asyncio.create_subprocess_shell = fake_shell
    import asyncio.subprocess as asp
    asp.create_subprocess_exec = fake_exec
    asp.create_subprocess_shell = fake_shell

    def refuse(*a, **kw):
        raise PermissionError("session-open harness: execução direta proibida")
    for fn in ("system", "popen", "execv", "execve", "execvp", "execvpe", "spawnv", "spawnve", "posix_spawn",
               "posix_spawnp", "fork", "forkpty"):
        if hasattr(os, fn):
            setattr(os, fn, refuse)

    # ── sinais: só nos processos falsos, e gravados ──
    real_kill = os.kill

    def fake_kill(pid, sig):
        if pid == os.getpid():
            return real_kill(pid, sig)
        p = procs.get(abs(pid))
        if sig == 0:
            if p is None:
                return real_kill(pid, 0)   # só pergunta se existe
            if p["alive"]:
                return None
            raise ProcessLookupError(3, "No such process")
        with lock:
            rec["signals"].append({"target": p["label"] if p else "real-process-refused",
                                   "signal": signal.Signals(sig).name})
        if p is None:
            raise PermissionError(1, "session-open harness: sinal a processo real recusado")
        kill_proc(abs(pid))

    os.kill = fake_kill
    os.killpg = fake_kill

    # /proc/<pid falso> lê do /proc de mentira; o resto do /proc é o de verdade.
    import builtins
    proc_re = re.compile(r"^/proc/(\d+)(/.*)?$")

    def redirect(path):
        if isinstance(path, (str, os.PathLike)):
            m = proc_re.match(os.fspath(path))
            if m and int(m.group(1)) in procs:
                return str(proc_dir / m.group(1)) + (m.group(2) or "")
        return path

    def wrap(fn):
        def inner(*a, **kw):
            if a:
                a = (redirect(a[0]), *a[1:])
            return fn(*a, **kw)
        return inner

    real_listdir = os.listdir

    def fake_listdir(path="."):
        out = real_listdir(redirect(path))
        if not isinstance(path, int) and os.fspath(path) == "/proc":
            out = out + [str(pid) for pid, p in procs.items() if p["alive"]]
        return out

    builtins.open = io.open = wrap(io.open)
    os.stat, os.lstat, os.scandir = wrap(os.stat), wrap(os.lstat), wrap(os.scandir)
    os.listdir = fake_listdir

    # ── rede: nada sai ──
    def fake_connect(self, address):
        entry = {"family": self.family.name, "address": str(address)}
        with lock:
            # Uma linha por endereço: quantas vezes o Python insiste depende do relógio.
            if entry not in rec["network"]:
                rec["network"].append(entry)
        raise ConnectionRefusedError(111, "session-open harness: conexão recusada")

    socket.socket.connect = fake_connect
    socket.socket.connect_ex = lambda self, a: (fake_connect(self, a), 111)[1]

    # ── o código do commit fixo ──
    sys.path.insert(0, backend)
    from fastapi.testclient import TestClient
    import app as app_pkg
    if not Path(app_pkg.__file__).resolve().is_relative_to(Path(backend).resolve()):
        raise SystemExit(f"app importado de {app_pkg.__file__}, não de {backend}")
    from app import account_bridge, share_bridge, groups_bridge, list_bridge
    from app import claude_customizations, api
    from app.adapters.claude_headless import adapter as hl_adapter

    # Pontes para o Rust: gravadas; respondem o que o cenário mandar. Sem resposta dada, as de estado
    # respondem ok vazio e a do catálogo (dictation) responde como com o Rust fora.
    replies = fakes.get("rust_bridge", {})
    from app import dictation_bridge

    def bridge(kind, make_error, default):
        def call(op, *args, **kw):
            payload = kw if kw else (args[0] if args else {})
            with lock:
                rec["rust_bridge"].append({"bridge": kind, "op": op, "args": payload})
            reply = replies.get(f"{kind}.{op}", default)
            if "error" in reply:
                e = reply["error"]
                raise make_error(e.get("status", 503), e["code"], e.get("detail", ""))
            return copy.deepcopy(reply["result"])
        return call

    ok = {"result": {}}
    share_bridge.call = bridge("share", lambda s, c, d: share_bridge.ShareBridgeError(c, d), ok)
    groups_bridge.call = bridge("groups", lambda s, c, d: groups_bridge.GroupsBridgeError(c, d), ok)
    list_bridge._request = bridge("list", lambda s, c, d: list_bridge.ListBridgeError(c, d), ok)
    claude_customizations._request = bridge("claude_customizations", claude_customizations.CustomizationsError, ok)
    dictation_bridge.request = bridge("dictation", dictation_bridge.BridgeError, {"error": {
        "status": 503, "code": "dictation_rust_unavailable",
        "detail": "O servidor Rust de organização está indisponível."}})

    def quotas(cached_only=True, **kw):
        with lock:
            rec["rust_bridge"].append({"bridge": "accounts", "op": "quotas", "args": {"cached_only": cached_only}})
        return copy.deepcopy(fakes.get("quotas"))

    account_bridge.request_quotas = quotas

    # O tempo de execução (cano, processo sem terminal, app-server do Codex) é do Rust/coordenador:
    # o pedido fica gravado e nada sobe.
    def runtime(kind):
        def call(self_or_name, *args, **kw):
            name = self_or_name if isinstance(self_or_name, str) else (args[0] if args else kw.get("name"))
            with lock:
                rec["runtime_requests"].append({"op": kind, "name": name,
                                                **{k: v for k, v in kw.items() if k != "name"}})
        return call

    hl_adapter.ClaudeHeadlessAdapter.acordar = runtime("claude_headless.acordar")
    _ensure = runtime("claude_headless.ensure_running")

    async def ensure_running(self, name, **kw):
        _ensure(self, name, **{k: v for k, v in kw.items() if k in ("esperar_pronta", "so_reconectar")})
        return None
    hl_adapter.ClaudeHeadlessAdapter.ensure_running = ensure_running
    _reload = runtime("claude_headless.recarregar")

    async def recarregar(self, name, **kw):
        _reload(self, name)
    hl_adapter.ClaudeHeadlessAdapter.recarregar = recarregar
    _stop = runtime("claude_headless.parar")

    async def parar(self, name, *a, **kw):
        _stop(self, name)
    hl_adapter.ClaudeHeadlessAdapter.parar = parar
    _perm = runtime("claude_headless.set_permission_mode")

    async def set_permission_mode(self, name, mode):
        # Sem processo vivo o cano não responde: é o ramo que reabre em bypass.
        _perm(self, name, mode=mode)
        raise RuntimeError("session-open harness: sem processo sem terminal vivo")
    hl_adapter.ClaudeHeadlessAdapter.set_permission_mode = set_permission_mode
    _warm = runtime("codex.aquecer_sem_terminal")

    async def aquecer(name):
        _warm(name)
    api._aquecer_codex_sem_terminal = aquecer

    client = TestClient(api.app, raise_server_exceptions=True)
    headers = {"Authorization": f"Bearer {TOKEN}"}
    for step in scenario["steps"]:
        if "call" in step:
            rec["steps"].append(_child_call(step, rec))
            continue
        req = {"method": step["method"], "path": step["path"]}
        try:
            r = client.request(step["method"], step["path"], headers=headers,
                               **({"json": step["json"]} if "json" in step else {}),
                               **({"params": step["params"]} if "params" in step else {}))
            try:
                body = r.json()
            except ValueError:
                body = r.text
            rec["steps"].append({**req, "status": r.status_code, "body": body})
        except Exception as exc:   # noqa: BLE001 — a exceção não tratada é o comportamento gravado
            rec["steps"].append({**req, "status": 500, "exception": f"{type(exc).__name__}: {exc}"})
    rec["tmux_sessions"] = {n: {"cwd": s["cwd"], "env": s["env"], "command": s["command"]}
                            for n, s in sorted(tmux_sessions.items())}
    Path(out_path).write_text(json.dumps(rec, ensure_ascii=False, default=str), encoding="utf-8")


def _child_call(step: dict, rec: dict) -> dict:
    """Chamada direta ao Python: só `launch_headless`, que hoje é do Rust (`runtime/claude_launch.rs`,
    conferido contra estas fixtures nos testes dele); o Python não lança mais."""
    if step["call"] != "launch_headless":
        raise SystemExit(f"chamada desconhecida {step['call']}")
    return {"call": step["call"], "name": step["name"], "exception": "o lançamento do Claude sem terminal é do Rust"}


def _pane_argv(command: str) -> list[str]:
    """argv do processo do pane a partir do comando do `new-session` (`exec [env -u X ]cmd...`)."""
    try:
        words = shlex.split(command)
    except ValueError:
        return ["sh", "-c", command]
    if words[:1] == ["exec"]:
        words = words[1:]
    while words[:2] and words[0] == "env" and len(words) > 2 and words[1] == "-u":
        words = words[3:]
    return words or ["sh"]


def _pane_unset(command: str) -> list[str]:
    try:
        words = shlex.split(command)
    except ValueError:
        return []
    if words[:1] == ["exec"]:
        words = words[1:]
    unset = []
    while len(words) > 2 and words[0] == "env" and words[1] == "-u":
        unset.append(words[2])
        words = words[3:]
    return unset


DEFAULT_PROGRAM_RULES = [
    {"prog": "claude", "args": ["--version"], "stdout": "2.1.300 (Claude Code)\n"},
    {"prog": "claude", "args": ["--help"], "stdout": "Usage: claude --plugin-dir <path>\n"},
]


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--child":
        child_main(*sys.argv[2:5])
    else:
        raise SystemExit(main())
