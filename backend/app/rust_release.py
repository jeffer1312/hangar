# backend/app/rust_release.py
"""Baixa o hangar-server e o hangar-cano da release do checkout para ~/.hangar/bin/.

Na main é a `server-latest`; noutra branch, a `server-<branch>` que o server.yml publica.

Um módulo só para os dois instaladores e o botão Atualizar. Sem os binários o Python atende
sozinho, então nada aqui levanta: cada falha vira aviso e linha no diário.
Uso: python -m app.rust_release [--never-fail]   (sai 0 no lugar, 1 falhou, 2 sem build)
"""
from __future__ import annotations

import hashlib
import http.client
import json
import logging
import os
import platform
import re
import secrets
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

from app import atomico, diag

_log = logging.getLogger("hangar.rust_release")

REPO = Path(__file__).resolve().parents[2]
RELEASES_URL = "https://github.com/jeffer1312/hangar/releases/download"
MAIN_TAG = "server-latest"
NAMES = ("hangar-server", "hangar-cano")
_EVENT = "hangar_server.baixar"

# Constante de módulo pelo motivo do atualizar.py: o teste troca a decisão sem mexer no os.name.
_E_WINDOWS = os.name == "nt"

# O passo de atualização morre aos 600 s: manifesto + dois binários só cabem com prazo total por
# download, porque o timeout do urllib vale por operação de socket e não pelo conjunto.
_MANIFEST_DEADLINE = 30.0
_ASSET_DEADLINE = 200.0
_SOCKET_TIMEOUT = 15.0
_MAX_OLD = 100

# Corpo cortado no meio (IncompleteRead) é HTTPException, não OSError.
_DOWNLOAD_ERRORS = (OSError, http.client.HTTPException, ValueError)


def platform_key() -> str | None:
    machine = platform.machine().lower()
    if sys.platform.startswith("linux") and machine in ("x86_64", "amd64"):
        return "linux-x86_64"
    if sys.platform == "win32" and machine in ("amd64", "x86_64"):
        return "windows-x86_64"
    if sys.platform == "darwin" and machine == "arm64":
        return "macos-aarch64"
    return None


def release_tag() -> str:
    """Release da branch do checkout; main e master usam a `server-latest`.

    Fora delas, cair na `server-latest` é desvio e vai ao diário: senão a máquina roda o binário
    da main achando que roda o da branch.
    """
    try:
        p = subprocess.run(["git", "rev-parse", "--abbrev-ref", "HEAD"], cwd=REPO, capture_output=True,
                           text=True, timeout=5, encoding="utf-8", errors="replace",
                           creationflags=0x08000000 if _E_WINDOWS else 0)   # CREATE_NO_WINDOW
    except Exception as e:                           # noqa: BLE001 — sem branch, a da main serve
        diag.registrar(_EVENT, "aviso", codigo="branch_ilegivel", **diag.erro_campos(e))
        return MAIN_TAG
    branch = p.stdout.strip()
    if p.returncode != 0 or not branch:
        diag.registrar(_EVENT, "aviso", codigo="branch_ilegivel", retorno=p.returncode)
        return MAIN_TAG
    if branch in ("main", "master"):
        return MAIN_TAG
    if branch == "HEAD":
        diag.registrar(_EVENT, "aviso", codigo="head_solto")
        return MAIN_TAG
    # Mesma limpeza do passo de publicação do .github/workflows/server.yml.
    tag = "server-" + re.sub(r"[^A-Za-z0-9._-]", "-", branch)
    if tag == MAIN_TAG:
        # O server.yml recusa publicar esta branch, que sobrescreveria a release da main.
        diag.registrar(_EVENT, "erro", codigo="branch_colide_com_main", tag=tag)
    return tag


def bin_dir() -> Path:
    return Path.home() / ".hangar" / "bin"


def _get(url: str, deadline: float) -> bytes:
    """Lê em blocos até `deadline` segundos; passou disso é falha comum (TimeoutError)."""
    limit = time.monotonic() + deadline
    chunks = []
    with urllib.request.urlopen(url, timeout=_SOCKET_TIMEOUT) as r:
        # read1 devolve o que chegou; read(n) seguraria até n bytes e o prazo nunca seria olhado.
        while block := r.read1(1 << 16):
            chunks.append(block)
            if time.monotonic() > limit:
                raise TimeoutError(f"passou de {deadline:g} s")
    return b"".join(chunks)


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _already_there(target: Path, sha: str) -> bool:
    try:
        return target.is_file() and _sha256_file(target) == sha
    except OSError:
        return False                 # ilegível: baixa de novo em vez de dar o binário por bom


def _free(path: Path) -> bool:
    try:
        os.lstat(path)
    except FileNotFoundError:
        return True
    except OSError:
        return False                 # nega até a leitura (apagado mas ainda aberto): ocupa o nome
    return False


def _sweep_old(target: Path) -> None:
    """Apaga os `<nome>.old*` de trocas anteriores, sem olhar a caixa (o NTFS não diferencia)."""
    prefix = f"{target.name}.old".lower()
    try:
        entries = list(target.parent.iterdir())
    except OSError:
        return
    for entry in entries:
        if entry.name.lower().startswith(prefix):
            try:
                entry.unlink()
            except OSError:
                pass                 # ainda é a imagem de um processo vivo: sai numa próxima rodada


def _old_name(target: Path) -> Path:
    """`<nome>.old`, ou `<nome>.old-N` quando um anterior ainda preso ocupa o nome."""
    candidate = target.with_name(f"{target.name}.old")
    for n in range(1, _MAX_OLD + 1):
        if _free(candidate):
            return candidate
        candidate = target.with_name(f"{target.name}.old-{n}")
    raise OSError(f"sem nome livre para {target.name}.old")


def _install(data: bytes, target: Path) -> None:
    tmp = target.with_name(f".{target.name}.{secrets.token_hex(4)}.tmp")
    try:
        tmp.write_bytes(data)
        tmp.chmod(0o755)
        if _E_WINDOWS and not _free(target):
            # Exe em uso não pode ser sobrescrito nem apagado, mas pode ser renomeado: o processo
            # vivo segue no nome velho e o próximo a subir pega o novo.
            old = _old_name(target)
            target.rename(old)
            try:
                atomico.substituir(tmp, target)
            except OSError:
                old.rename(target)   # sem desfazer, o caminho do binário ficaria vazio
                raise
        else:
            atomico.substituir(tmp, target)
    finally:
        tmp.unlink(missing_ok=True)


def _fetch_one(url: str, files: object, plat: str, name: str, target: Path,
               commit: str, tag: str | None) -> str | None:
    entry = files.get(f"{plat}/{name}") if isinstance(files, dict) else None
    if not isinstance(entry, dict) or not isinstance(entry.get("name"), str) \
            or not isinstance(entry.get("sha256"), str):
        diag.registrar("hangar_server.baixar", "erro", etapa="manifesto", detalhe=name)
        return f"{name}: a release não traz build para {plat}"
    sha = entry["sha256"].lower()
    if _already_there(target, sha):
        return None
    try:
        data = _get(f"{url}/{entry['name']}", _ASSET_DEADLINE)
    except _DOWNLOAD_ERRORS as e:
        diag.registrar("hangar_server.baixar", "erro", etapa="download", detalhe=name, **diag.erro_campos(e))
        return f"{name}: o download falhou ({e})"
    if hashlib.sha256(data).hexdigest() != sha:
        diag.registrar("hangar_server.baixar", "erro", etapa="sha256", detalhe=name)
        return f"{name}: o sha256 não confere com a release; o binário anterior ficou"
    try:
        _install(data, target)
    except OSError as e:
        diag.registrar("hangar_server.baixar", "erro", etapa="gravar", detalhe=name, **diag.erro_campos(e))
        return f"{name}: não consegui gravar em {target.parent} ({e})"
    diag.registrar("hangar_server.baixar", codigo="trocado", detalhe=name, commit=commit, tag=tag)
    return None


def fetch(base_url: str | None = None, dest: Path | None = None) -> list[str] | None:
    """Põe em `dest` os binários desta máquina, conferidos pelo sha256 do manifesto.

    `None` = a release não tem build para esta máquina; `[]` = tudo no lugar; senão, os avisos.
    Nunca levanta: quem chama (instalador, botão Atualizar) segue sem os binários.
    """
    plat = platform_key()
    if plat is None:
        diag.registrar("hangar_server.baixar", "aviso", codigo="sem_build")
        return None
    # `tag` None = URL dada por quem chama: sem release da main para onde recuar.
    url, tag = base_url or os.environ.get("HANGAR_SERVER_RELEASE_URL"), None
    if not url:
        tag = release_tag()
        url = f"{RELEASES_URL}/{tag}"
    url = url.rstrip("/")
    dest = dest or bin_dir()
    ext = ".exe" if plat.startswith("windows") else ""
    branch_tag = None   # preenchido quando a branch não tem release e caímos na da main
    try:
        try:
            manifest = json.loads(_get(f"{url}/server-latest.json", _MANIFEST_DEADLINE))
        except urllib.error.HTTPError as e:
            if e.code != 404 or tag in (None, MAIN_TAG):
                raise
            # O server.yml só publica a branch quando crates/ muda: sem release própria, vale a da main.
            diag.registrar(_EVENT, "aviso", codigo="sem_release_da_branch", tag=tag)
            branch_tag, tag, url = tag, MAIN_TAG, f"{RELEASES_URL}/{MAIN_TAG}"
            manifest = json.loads(_get(f"{url}/server-latest.json", _MANIFEST_DEADLINE))
        files = manifest["files"]
        # Só vai ao diário, para diagnosticar versão do Python diferente da do binário.
        commit = str(manifest.get("commit", ""))[:40]
        dest.mkdir(parents=True, exist_ok=True)
    except (*_DOWNLOAD_ERRORS, KeyError, TypeError) as e:
        diag.registrar("hangar_server.baixar", "erro", etapa="manifesto", tag=tag, **diag.erro_campos(e))
        return [f"binários Rust não baixados: não consegui ler o manifesto da release ({e})"]
    avisos, kept, installed = [], [], []
    for name in NAMES:
        target = dest / f"{name}{ext}"
        _sweep_old(target)
        # Sem a release da branch, o binário instalado pode ser o dela: o da main só preenche falta.
        if branch_tag and not _free(target):
            kept.append(name)
            continue
        if aviso := _fetch_one(url, files, plat, name, target, commit, tag):
            avisos.append(aviso)
        elif branch_tag:
            installed.append(name)
    if kept:
        avisos.append(f"release {branch_tag} ausente; mantive os binários instalados ({', '.join(kept)})")
    if installed:
        avisos.append(f"release {branch_tag} ausente; instalei os da main ({', '.join(installed)})")
    for aviso in avisos:
        _log.warning(aviso)
    return avisos


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        # Console do Windows com pipe é cp1252: caminho ou erro fora dela não pode derrubar o passo.
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure:
            reconfigure(errors="replace")
    avisos = fetch()
    if avisos is None:
        print("binários Rust: a release não tem build para esta máquina; o Python atende sozinho")
        code = 2
    elif avisos:
        for aviso in avisos:
            print(aviso)
        code = 1
    else:
        print(f"binários Rust em {bin_dir()}")
        code = 0
    # O passo de atualização não pode parar a atualização inteira por um extra.
    return 0 if "--never-fail" in argv else code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
