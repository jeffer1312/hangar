"""montar-pacote.py: o pacote que sai tem exatamente os arquivos do manifesto, com o sha certo."""
import hashlib
import importlib.machinery
import importlib.util
import json
import os
import re
import subprocess
import sys
import tarfile
from pathlib import Path

import pytest

# Nome com hífen: spec_from_file_location não acha loader por sufixo sem o SourceFileLoader.
_path = Path(__file__).with_name("montar-pacote.py")
spec = importlib.util.spec_from_file_location(
    "montar_pacote", _path, loader=importlib.machinery.SourceFileLoader("montar_pacote", str(_path)))
mp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mp)

REPO = Path(__file__).resolve().parents[1]


def _git(*args: str) -> str:
    return subprocess.run(["git", "-C", str(REPO), *args], check=True, capture_output=True, text=True).stdout.strip()


def _fake_python(tmp: Path, os_name: str) -> Path:
    """Arquivo no formato do python-build-standalone install_only: raiz `python/`, link no Linux."""
    src = tmp / "pysrc"
    if os_name == "linux-x86_64":
        (src / "python/bin").mkdir(parents=True)
        # O "Python embutido" do teste é o deste processo: o build compila com ele.
        (src / "python/bin/python3.14").write_text(f'#!/bin/sh\nexec "{sys.executable}" "$@"\n')
        (src / "python/bin/python3.14").chmod(0o755)
        os.symlink("python3.14", src / "python/bin/python3")
        (src / "python/lib/python3.14/site-packages").mkdir(parents=True)
        (src / "python/lib/python3.14/site-packages/README.txt").write_text("sp")
        (src / "python/share/terminfo/x").mkdir(parents=True)
        (src / "python/share/terminfo/x/xterm").write_text("ti")
        (src / "python/include/python3.14").mkdir(parents=True)
        (src / "python/include/python3.14/Python.h").write_text("h")
    else:
        (src / "python/Lib/site-packages").mkdir(parents=True)
        (src / "python/python.exe").write_bytes(b"MZ fake")
        (src / "python/python314.dll").write_bytes(b"MZ dll")
        (src / "python/python3.dll").write_bytes(b"MZ dll3")
        (src / "python/Scripts").mkdir()
        for exe in ("pip.exe", "pip3.exe", "pip3.14.exe"):
            (src / "python/Scripts" / exe).write_bytes(b"MZ pip")
    # O pip que o standalone traz: pacote, dist-info, executáveis e o wheel do ensurepip.
    lib = "python/lib/python3.14" if os_name == "linux-x86_64" else "python/Lib"
    for f in ("site-packages/pip/__init__.py", "site-packages/pip/_internal/main.py",
              "site-packages/pip-26.2.2.dist-info/METADATA", "ensurepip/_bundled/pip-26.2.2-py3-none-any.whl"):
        (src / lib / f).parent.mkdir(parents=True, exist_ok=True)
        (src / lib / f).write_text("pip")
    (src / lib / "ensurepip/__init__.py").write_text("")
    if os_name == "linux-x86_64":
        (src / "python/bin/pip").write_text("#!/bin/sh\n")
        (src / "python/bin/pip").chmod(0o755)
        os.symlink("pip", src / "python/bin/pip3")
    arc = tmp / "py.tar.gz"
    with tarfile.open(arc, "w:gz") as t:
        t.add(src / "python", arcname="python")
    return arc


def _fake_bins(tmp: Path, os_name: str) -> Path:
    exe = ".exe" if os_name.startswith("windows") else ""
    d = tmp / "bins"
    d.mkdir()
    # Nomes do artefato `server-<sistema>` do server.yml.
    for b in ("hangar-server", "hangar-cano"):
        p = d / f"{b}-{os_name}{exe}"
        p.write_bytes(f"bin {b}".encode())
        p.chmod(0o755)
    return d


@pytest.mark.parametrize("os_name", ["linux-x86_64", "windows-x86_64"])
def test_pacote_tem_exatamente_os_arquivos_do_manifesto(tmp_path, monkeypatch, os_name):
    calls = []

    def fake_install(site_packages, os_arg, py_version, workdir):
        calls.append((site_packages, os_arg, py_version))
        (site_packages / "fastapi").mkdir()
        (site_packages / "fastapi/__init__.py").write_text("")

    monkeypatch.setattr(mp, "install_deps", fake_install)
    out = tmp_path / "out"
    rc = mp.main(["--os", os_name, "--bin-dir", str(_fake_bins(tmp_path, os_name)),
                  "--python-archive", str(_fake_python(tmp_path, os_name)), "--out", str(out),
                  "--channel", "latest"])
    assert rc == 0

    version = (REPO / "VERSION").read_text().strip() + "." + _git("rev-list", "--count", "HEAD")
    assert re.fullmatch(r"\d+\.\d+\.\d+\.\d+", version)
    archive = out / f"hangar-{version}-{os_name}.tar.gz"
    assert archive.is_file()

    members = {}
    with tarfile.open(archive) as t:
        for m in t.getmembers():
            assert m.isreg(), f"{m.name}: só arquivo comum"
            name = m.name.removeprefix("./")
            assert not name.startswith("/") and ".." not in name.split("/") and "\\" not in name
            members[name] = (t.extractfile(m).read(), m.mode)

    manifest = json.loads(members.pop("manifest.json")[0])
    assert manifest["files"] == {n: hashlib.sha256(b).hexdigest() for n, (b, _) in members.items()}
    assert manifest["version"] == version
    assert manifest["os"] == os_name
    assert manifest["channel"] == "latest"
    assert manifest["commit"] == _git("rev-parse", "HEAD")
    rust_protocol = re.search(r"^pub const INTERNAL_PROTOCOL: u32 = (\d+);",
                              (REPO / "crates/hangar-server/src/lib.rs").read_text(), re.M).group(1)
    assert manifest["protocol"] == int(rust_protocol)
    api = re.search(r"^pub const CLIENT_API: u32 = (\d+);",
                    (REPO / "crates/hangar-api/src/lib.rs").read_text(), re.M).group(1)
    assert manifest["api"] == int(api)
    # Mesma conta da `dist-commits` do ci.yml.
    trees = _git("rev-parse", "HEAD:frontend", "HEAD:packages", "HEAD:messages", "HEAD:project.inlang",
                 "HEAD:package.json", "HEAD:package-lock.json") + "\n"
    assert manifest["dist_key"] == hashlib.sha256(trees.encode()).hexdigest()

    exe = ".exe" if os_name.startswith("windows") else ""
    for b in ("hangar-server", "hangar-cano"):
        data, mode = members[f"bin/{b}{exe}"]
        assert data == f"bin {b}".encode() and mode & 0o111
    for must in ("VERSION", "backend/app/main.py", "backend/hooks", "resources/", "skills/", "plugins/hangar/",
                 "messages/pt.json", "docs/atualizacoes/README.md", "shell/preview_fmt.cjs",
                 "scripts/hangar-send", "scripts/hangar-preview"):
        assert any(n == must or n.startswith(must.rstrip("/") + "/") for n in members), must
    for name in members:
        assert not name.startswith(("crates/", "frontend/", "backend/tests/", ".git")), name
        assert "node_modules" not in name, name
        if "__pycache__" in name:
            # Só o .pyc de um fonte que está no pacote.
            assert name.endswith(f".{sys.implementation.cache_tag}.pyc"), name
            pasta, arq = name.split("/__pycache__/")
            assert f"{pasta}/{arq.split('.')[0]}.py" in members, name
    # Compilado no build, sem conferir o fonte (unchecked-hash): o Python nunca grava nem regrava
    # __pycache__ dentro da pasta da versão.
    fontes = [n for n in members if n.endswith(".py") and n.startswith(("backend/", "scripts/", "python/"))]
    assert fontes
    for fonte in fontes:
        pyc = importlib.util.cache_from_source(fonte)
        assert pyc in members, pyc
        data = members[pyc][0]
        assert data[:4] == importlib.util.MAGIC_NUMBER and int.from_bytes(data[4:8], "little") == 0b01, pyc
        assert not re.search(r"(^|/)(test_[^/]*\.py|[^/]*\.test\.[a-z]+)$", name), name
    for dev in ("scripts/verificar-local", "scripts/cargo-pinned", "scripts/test_montar_pacote.py",
                "scripts/montar-pacote.py", "scripts/prova-parte4.py", "scripts/hooks/pre-push"):
        assert dev not in members, dev

    sp = "python/lib/python3.14/site-packages" if os_name == "linux-x86_64" else "python/Lib/site-packages"
    assert f"{sp}/fastapi/__init__.py" in members
    # O pacote nunca instala nada em tempo de execução: o pip fica fora.
    pip = [n for n in members if re.search(
        r"^python/(bin|Scripts)/pip[^/]*$|/site-packages/pip(/|-[^/]*\.dist-info/)|/ensurepip/_bundled/pip-", n)]
    assert pip == []
    assert calls[0][1] == os_name and calls[0][2] == "3.14"
    assert calls[0][0].as_posix().endswith(sp)
    if os_name == "linux-x86_64":
        # O link do Python vira cópia: o extrator recusa link.
        assert members["python/bin/python3"][0] == members["python/bin/python3.14"][0]
        # Cabeçalhos C, man e terminfo não servem ao backend e estouravam o teto do manifesto.
        assert not any(n.startswith(("python/include/", "python/share/")) for n in members)

    entry = json.loads((out / f"package-entry-{os_name}.json").read_text())
    assert entry == {
        "version": version, "asset": archive.name, "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
        "protocol": manifest["protocol"], "api": manifest["api"], "commit": manifest["commit"],
        "dist_key": manifest["dist_key"],
    }


def test_manifesto_acima_do_teto_do_extrator_falha_no_build(tmp_path, monkeypatch):
    monkeypatch.setattr(mp, "install_deps", lambda *a: None)
    # O teto real (4 MiB, MANIFEST_MAX do crates/hangar-package) num valor que o manifesto deste repo passa.
    monkeypatch.setattr(mp, "MANIFEST_MAX", 1000)
    out = tmp_path / "out"
    with pytest.raises(SystemExit, match="manifest.json"):
        mp.main(["--os", "linux-x86_64", "--bin-dir", str(_fake_bins(tmp_path, "linux-x86_64")),
                 "--python-archive", str(_fake_python(tmp_path, "linux-x86_64")), "--out", str(out),
                 "--channel", "latest"])
    assert not list(out.glob("*.tar.gz"))


def test_link_na_arvore_recusa(tmp_path):
    (tmp_path / "a").write_text("x")
    os.symlink("a", tmp_path / "b")
    with pytest.raises(SystemExit, match="b"):
        mp.file_shas(tmp_path)


@pytest.mark.parametrize("bad", [b"a:b", b"a\\b", b"\xffnome"])
def test_caminho_que_o_extrator_recusa_falha_no_build(tmp_path, bad):
    # Mesma regra do is_safe_relative do crates/hangar-package: sem `:`, sem `\`, nome UTF-8.
    (tmp_path / "ok").mkdir()
    (tmp_path / "ok/bom.txt").write_text("x")
    culpado = os.fsencode(tmp_path / "ok") + b"/" + bad
    with open(culpado, "wb") as f:
        f.write(b"x")
    with pytest.raises(SystemExit) as e:
        mp.file_shas(tmp_path)
    assert repr("ok/" + os.fsdecode(bad)) in str(e.value)


def test_link_do_python_para_fora_recusa(tmp_path):
    root = tmp_path / "root"
    (root / "python/bin").mkdir(parents=True)
    (tmp_path / "fora").write_text("x")
    os.symlink(tmp_path / "fora", root / "python/bin/python3")
    with pytest.raises(SystemExit, match="python3"):
        mp.replace_links(root / "python")
