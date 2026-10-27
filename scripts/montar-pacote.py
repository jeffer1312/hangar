#!/usr/bin/env python3
"""Monta o pacote por versão de um sistema (plano docs/superpowers/plans/2026-10-24-pacote-por-versao.md).

    montar-pacote.py --os linux-x86_64|windows-x86_64 --bin-dir <binários> \
        --python-archive <python-build-standalone install_only .tar.gz> --out <pasta> [--channel <canal>]

Sai em <out>: `hangar-<versao>-<os>.tar.gz` (manifest.json na raiz, todo outro arquivo nele com o sha256)
e `package-entry-<os>.json` (a entrada do sistema no `package-<canal>.json`). O formato é o que o
crates/hangar-package aceita: só arquivos comuns, caminhos relativos com `/`, nenhum link.
Precisa de `git` e `uv`; roda no checkout do commit que vai ser empacotado.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.request
from fnmatch import fnmatch
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
MANIFEST = "manifest.json"
# Teto do extrator (MANIFEST_MAX em crates/hangar-package/src/lib.rs): acima dele o pacote não abre.
MANIFEST_MAX = 4 << 20
# Sistema → plataforma do `uv pip install` e sufixo dos executáveis. manylinux_2_28 = a glibc que o
# hangar-server também exige.
OSES = {"linux-x86_64": ("x86_64-manylinux_2_28", ""), "windows-x86_64": ("x86_64-pc-windows-msvc", ".exe")}
# Os instaladores vão juntos: a instalação nova roda o do `current`, em modo pacote (sem git, uv nem build).
TREES = ["backend/app", "backend/hooks", "resources", "scripts", "skills", "plugins", "shell", "messages",
         "docs/atualizacoes", "docs/novidades", "docs/tmux.conf.windows.example", "docs/tmux.conf.example",
         "VERSION", "install.sh", "install.ps1"]
# Testes, provas, medições e ferramentas de quem desenvolve: rodam do checkout (modo desenvolvedor).
EXCLUDE = [
    "*__pycache__*", "*.pyc", "*node_modules*", "*/tests/*", "*/test_*", "*/test-*", "*.test.*",
    "scripts/hooks/*", "scripts/prova-*", "scripts/medir-*", "scripts/probe-*", "scripts/RODAR-TESTE*",
    "scripts/cargo-pinned", "scripts/podar-target", "scripts/verificar-*", "scripts/ci-gates.sh",
    "scripts/check-*", "scripts/checar-*", "scripts/conferir-*", "scripts/core-front-tests.mjs",
    "scripts/hook-*.mjs", "scripts/i18n-compile.mjs", "scripts/deploy.sh", "scripts/comparar-custos.py",
    "scripts/fetch_model_prices.py", "scripts/post-merge.hook", "scripts/install-hooks.sh",
    "scripts/migrate-to-*", "scripts/montar-pacote.py",
]
# Backend de build dos pacotes do lock que só têm sdist (setup.py sem pyproject usa setuptools e wheel).
BUILD_REQS = """\
setuptools==84.2.1 --hash=sha256:ca140b45d5325a9478f880e351484f62fef15f336462e449ca0c03c873d589c2
wheel==0.48.0 --hash=sha256:3217dcc807155e45db462d7ef2431f5ddda0d7273b700d05a67b271ceb1287ab
packaging==26.3 --hash=sha256:d7193f7c8e4e93f444fde0262bf90af30e16fa0ad0ad44cb553c87339b23cd1c
"""


def die(msg: str):
    raise SystemExit(f"montar-pacote: {msg}")


def run(cmd: list, **kw) -> subprocess.CompletedProcess:
    return subprocess.run([str(c) for c in cmd], check=True, **kw)


def git(*args: str) -> bytes:
    return run(["git", "-C", REPO, *args], capture_output=True).stdout


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def const(path: str, pattern: str) -> int:
    m = re.search(pattern, (REPO / path).read_text(encoding="utf-8"), re.M)
    if not m:
        die(f"{pattern!r} não encontrado em {path}")
    return int(m.group(1))


def wanted(rel: str) -> bool:
    if rel.startswith("shell/") and not rel.endswith(".cjs"):
        return False  # de shell/ só os módulos que o hangar-preview e o folha importam
    return not any(fnmatch(rel, p) for p in EXCLUDE)


def copy_repo(root: Path):
    out = git("ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *TREES)
    for rel in sorted({r for r in out.decode().split("\0") if r}):
        src = REPO / rel
        if not wanted(rel):
            continue
        if src.is_symlink():
            die(f"{rel}: link no repositório; o pacote só leva arquivo comum")
        if not src.is_file():
            print(f"montar-pacote: aviso: {rel} está no índice e não no disco; fica fora", file=sys.stderr)
            continue
        dst = root / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)


def replace_links(tree: Path):
    """Troca cada link por uma cópia do alvo: o extrator recusa link."""
    base = tree.resolve()
    for dirpath, dirnames, filenames in os.walk(tree):
        for name in dirnames + filenames:
            p = Path(dirpath, name)
            if not p.is_symlink():
                continue
            target = p.resolve()
            if not target.is_relative_to(base) or not target.is_file():
                die(f"{p}: link para fora de {tree} ou para pasta")
            p.unlink()
            shutil.copy2(target, p)


def python_layout(root: Path, os_name: str) -> tuple[Path, str]:
    """(site-packages, versão X.Y) do Python embutido extraído em root/python."""
    py = root / "python"
    if os_name == "windows-x86_64":
        dlls = [d.name for d in py.glob("python3*.dll") if re.fullmatch(r"python3\d+\.dll", d.name)]
        sp = py / "Lib/site-packages"
        ver = f"3.{dlls[0][7:-4]}" if len(dlls) == 1 else None
    else:
        libs = [d for d in py.glob("lib/python3.*") if re.fullmatch(r"python3\.\d+", d.name)]
        sp = libs[0] / "site-packages" if len(libs) == 1 else py
        ver = libs[0].name[6:] if len(libs) == 1 else None
    if not ver or not sp.is_dir():
        die(f"{py}: não reconheço o python-build-standalone de {os_name}")
    return sp, ver


def install_deps(site_packages: Path, os_name: str, py_version: str, workdir: Path):
    """Dependências do backend/uv.lock no site-packages do Python embutido, só por wheel e com hash.

    Pacote do lock sem wheel (hoje só o http-ece) vira wheel aqui, a partir do sdist conferido pelo hash
    do lock; tem de sair `none-any`, senão não serve para outro sistema.
    """
    lock = tomllib.loads((REPO / "backend/uv.lock").read_text(encoding="utf-8"))
    sdist_only = [p for p in lock["package"] if "wheels" not in p and "sdist" in p]
    reqs = workdir / "requirements.txt"
    skip = [a for p in sdist_only for a in ("--no-emit-package", p["name"])]
    run(["uv", "export", "--frozen", "--no-dev", "--no-emit-project", "--project", REPO / "backend",
         *skip, "-o", reqs], stdout=subprocess.DEVNULL)
    lines = []
    if sdist_only:
        # O build roda num ambiente com o backend de build fixado por hash e sem rede (`--offline`).
        venv = workdir / "build-venv"
        run(["uv", "venv", "--python", sys.executable, venv], stdout=subprocess.DEVNULL)
        build_py = venv / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        (workdir / "build-requirements.txt").write_text(BUILD_REQS, encoding="utf-8")
        run(["uv", "pip", "install", "--python", build_py, "--require-hashes", "--only-binary", ":all:",
             "-r", workdir / "build-requirements.txt"])
    for p in sdist_only:
        url, want = p["sdist"]["url"], p["sdist"]["hash"].removeprefix("sha256:")
        sdist = workdir / url.rsplit("/", 1)[1]
        urllib.request.urlretrieve(url, sdist)
        if sha256(sdist) != want:
            die(f"{sdist.name}: sha256 não confere com o backend/uv.lock")
        wheels = workdir / f"wheels-{p['name']}"
        run(["uv", "build", "--wheel", "--no-build-isolation", "--offline", "--python", build_py,
             "--out-dir", wheels, sdist])
        (whl,) = wheels.glob("*.whl")
        if not whl.name.endswith("-none-any.whl"):
            die(f"{whl.name}: wheel com código nativo; não dá para montar para outro sistema")
        lines.append(f"{p['name']} @ {whl.as_uri()} --hash=sha256:{sha256(whl)}")
    with open(reqs, "a", encoding="utf-8") as f:
        f.write("".join(line + "\n" for line in lines))
    run(["uv", "pip", "install", "--target", site_packages, "--python-platform", OSES[os_name][0],
         "--python-version", py_version, "--only-binary", ":all:", "--require-hashes", "--link-mode", "copy",
         "-r", reqs])
    # Executáveis dos pacotes: o shebang aponta para a máquina que montou. O backend roda por `python -m`.
    for d in ("bin", "Scripts"):
        shutil.rmtree(site_packages / d, ignore_errors=True)


def compile_pyc(root: Path, os_name: str, py_version: str):
    """Todo .py do pacote ganha o .pyc `unchecked-hash`: o Python usa sem conferir o fonte e nunca grava
    __pycache__ na pasta da versão (hook roda o Python do pacote sem PYTHONPYCACHEPREFIX).

    Compila com o Python embutido quando ele roda aqui; senão com este, da mesma versão: o .pyc não
    depende do sistema, só do magic number, conferido contra um .pyc que o standalone já traga.
    """
    if os_name == "linux-x86_64" and sys.platform == "linux":
        py = root / "python/bin/python3"
    else:
        py = Path(sys.executable)
        if f"{sys.version_info[0]}.{sys.version_info[1]}" != py_version:
            die(f"compilar para Python {py_version} pede um {py_version} aqui, não {sys.version.split()[0]}")
        shipped = next((root / "python").rglob("*.pyc"), None)
        if shipped and shipped.read_bytes()[:4] != importlib.util.MAGIC_NUMBER:
            die(f"magic number de {shipped.name} difere do deste Python")
    run([py, "-m", "compileall", "-q", "-j", "0", "--invalidation-mode", "unchecked-hash", root])


def safe_relative(rel: str) -> bool:
    """A regra do `is_safe_relative` do crates/hangar-package, mais o nome em UTF-8 que ele exige."""
    try:
        rel.encode("utf-8")
    except UnicodeEncodeError:
        return False
    return ":" not in rel and "\\" not in rel and all(c not in ("", ".", "..") for c in rel.split("/"))


def file_shas(root: Path) -> dict[str, str]:
    files = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for name in sorted(dirnames + filenames):
            p = Path(dirpath, name)
            rel = p.relative_to(root).as_posix()
            if p.is_symlink() or not (p.is_dir() or p.is_file()):
                die(f"{rel!r}: link ou arquivo especial; o pacote só leva arquivo comum")
            if not safe_relative(rel):
                die(f"{rel!r}: caminho que o extrator recusa (`:`, `\\`, nome vazio, `.`/`..` ou não UTF-8)")
            if p.is_file() and rel != MANIFEST:
                files[rel] = sha256(p)
    return files


def frontmatter(text: str) -> tuple[dict, str]:
    """O `frontmatter` do crates/hangar-server/src/update_notes: linhas como o `str::lines` do Rust."""
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    lines = [line.removesuffix("\r") for line in lines]
    if not lines or lines[0].strip() != "---" or "---" not in lines[1:]:
        return {}, text
    end = lines.index("---", 1)
    fields = {}
    for line in lines[1:end]:
        if line.strip() and not line.lstrip().startswith("#") and ":" in line:
            key, value = line.split(":", 1)
            fields[key.strip()] = value.strip()
    return fields, "\n".join(lines[end + 1:]).strip()


def _note_files(folder: Path) -> list[tuple[str, str]]:
    names = sorted(p.name for p in folder.glob("*.md") if p.is_file() and not p.name.upper().startswith("README"))
    return [(n, (folder / n).read_text(encoding="utf-8")) for n in names]


def _stem(name: str) -> str:
    while name.endswith(".md"):  # o `trim_end_matches(".md")` do Rust
        name = name[:-3]
    return name


def notes(root: Path, os_name: str) -> dict:
    """Avisos e novidades da versão, no formato do `GET /api/update/notes`. As regras são as do
    crates/hangar-server/src/update_notes (casos comuns em fixtures.json); aviso só de passo destrutivo
    de máquina que o steps.rs roda neste sistema: comando com prova, e comando para este sistema."""
    own = "comando_windows" if os_name.startswith("windows") else "comando_posix"
    warnings = []
    for name, text in _note_files(root / "docs/atualizacoes"):
        f, body = frontmatter(text)
        commands = [f.get(k) for k in ("comando", "comando_posix", "comando_windows")]
        if any(commands) and not f.get("prova"):
            continue  # o steps.rs ignora o arquivo
        command = (f.get(own) or f.get("comando") or "").strip()
        if f.get("titulo") and f.get("destrutivo", "").lower() in ("true", "sim", "1") and f.get("tipo") == "maquina" and command:
            warnings.append({"id": f.get("id") or name.removesuffix(".md"), "title": f["titulo"], "text": body})
    kinds = {"novo": "new", "melhoria": "improvement", "correcao": "fix"}
    places = {"desktop": "desktop", "celular": "mobile", "todos": "all"}
    news = []
    for name, text in _note_files(root / "docs/novidades"):
        f, body = frontmatter(text)
        kind, platforms = kinds.get(f.get("tipo")), places.get(f.get("onde"))
        if not (f.get("titulo") and kind and platforms):
            continue
        news.append({"id": _stem(name), "title": f["titulo"], "kind": kind, "text": body, "platforms": platforms,
                     "try_it": f.get("experimente") or None,
                     "animation": None if kind == "fix" else f.get("animacao") or None})
    order = list(kinds.values())
    news.sort(key=lambda n: (order.index(n["kind"]), n["id"]))
    return {"warnings": warnings, "news": news}


def pack(root: Path, files: list[str], dest: Path):
    part = dest.with_name(dest.name + ".part")
    with tarfile.open(part, "w:gz", format=tarfile.PAX_FORMAT, compresslevel=6) as tar:
        for rel in [MANIFEST, *files]:
            p = root / rel
            st = p.stat()
            info = tarfile.TarInfo(rel)
            info.size, info.mode, info.mtime = st.st_size, st.st_mode & 0o777, int(st.st_mtime)
            with open(p, "rb") as f:
                tar.addfile(info, f)
    part.replace(dest)


def channel_of_checkout() -> str:
    branch = git("rev-parse", "--abbrev-ref", "HEAD").decode().strip()
    if branch == "HEAD":
        die("checkout sem branch: passe --channel")
    # Mesma limpeza do tag `server-<branch>` do server.yml.
    return "latest" if branch == "main" else re.sub(r"[^A-Za-z0-9._-]", "-", branch)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--os", required=True, choices=sorted(OSES))
    ap.add_argument("--bin-dir", required=True, type=Path)
    ap.add_argument("--python-archive", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--channel", help="latest (main) ou o nome da branch; padrão: o do checkout")
    a = ap.parse_args(argv)
    exe = OSES[a.os][1]

    version = (REPO / "VERSION").read_text(encoding="utf-8").strip() + "." + git(
        "rev-list", "--count", "HEAD").decode().strip()
    if not re.fullmatch(r"\d+\.\d+\.\d+\.\d+", version):
        die(f"versão {version!r} fora do formato VERSION.commits")
    protocol = const("crates/hangar-server/src/lib.rs", r"^pub const INTERNAL_PROTOCOL: u32 = (\d+);")
    if protocol != const("backend/app/rust_server.py", r"^RUST_SERVER_PROTOCOL = (\d+)"):
        die("INTERNAL_PROTOCOL (Rust) e RUST_SERVER_PROTOCOL (Python) diferentes")
    manifest = {
        "version": version,
        "commit": git("rev-parse", "HEAD").decode().strip(),
        "channel": a.channel or channel_of_checkout(),
        "protocol": protocol,
        "api": const("crates/hangar-api/src/lib.rs", r"^pub const CLIENT_API: u32 = (\d+);"),
        "os": a.os,
        # Chave da tela na release `dist-commits`: a mesma conta do ci.yml.
        "dist_key": hashlib.sha256(git("rev-parse", "HEAD:frontend", "HEAD:packages", "HEAD:messages",
                                       "HEAD:project.inlang", "HEAD:package.json",
                                       "HEAD:package-lock.json")).hexdigest(),
    }

    a.out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=a.out, prefix=".montar-") as tmp:
        work, root = Path(tmp, "work"), Path(tmp, "pkg")
        work.mkdir()
        root.mkdir()
        with tarfile.open(a.python_archive) as t:
            t.extractall(root, filter="data")
        # Cabeçalhos C, man e terminfo do standalone: o backend não usa, e só a terminfo eram 2871
        # arquivos que levavam o manifesto acima do MANIFEST_MAX.
        for d in ("include", "share"):
            shutil.rmtree(root / "python" / d, ignore_errors=True)
        replace_links(root / "python")
        site_packages, py_version = python_layout(root, a.os)
        # O pacote nunca instala nada em tempo de execução: sai o pip do standalone.
        py = root / "python"
        for p in [*site_packages.glob("pip"), *site_packages.glob("pip-*.dist-info"), *py.glob("bin/pip*"),
                  *py.glob("Scripts/pip*"), *py.rglob("ensurepip/_bundled/pip-*.whl")]:
            shutil.rmtree(p) if p.is_dir() else p.unlink()
        install_deps(site_packages, a.os, py_version, work)

        (root / "bin").mkdir()
        for name in ("hangar-server", "hangar-cano"):
            src = next((a.bin_dir / n for n in (f"{name}{exe}", f"{name}-{a.os}{exe}")
                        if (a.bin_dir / n).is_file()), None)
            if src is None:
                die(f"{name}{exe} não está em {a.bin_dir}")
            shutil.copyfile(src, root / "bin" / f"{name}{exe}")
            # O artefato do Actions perde o bit de execução.
            (root / "bin" / f"{name}{exe}").chmod(0o755)
        copy_repo(root)
        version_notes = notes(root, a.os)
        compile_pyc(root, a.os, py_version)

        files = file_shas(root)
        manifest["files"] = files
        text = json.dumps(manifest, indent=2, sort_keys=True)
        if len(text.encode()) > MANIFEST_MAX:
            die(f"{MANIFEST} com {len(text.encode())} bytes passa do teto de {MANIFEST_MAX} do extrator")
        (root / MANIFEST).write_text(text, encoding="utf-8")
        asset = a.out / f"hangar-{version}-{a.os}.tar.gz"
        pack(root, sorted(files), asset)

    entry = {k: manifest[k] for k in ("version", "protocol", "api", "commit", "dist_key")}
    entry.update(asset=asset.name, sha256=sha256(asset), notes=version_notes)
    (a.out / f"package-entry-{a.os}.json").write_text(json.dumps(entry, indent=2, sort_keys=True),
                                                      encoding="utf-8")
    print(f"{asset}: {len(files)} arquivos, {asset.stat().st_size} bytes, versão {version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
