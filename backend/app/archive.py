"""Arquivo de conversas MORTAS: navega os transcripts .jsonl no disco mesmo sem sessao tmux viva.

O registry so enxerga sessoes tmux; os jsonl antigos ficam orfaos. Aqui: listagem (projeto, preview,
data, se esta em uso por uma sessao viva) + resolucao de path VALIDADA (nunca leitura arbitraria de
disco: projeto no alfabeto do sanitize_cwd, session_id uuid, e o arquivo tem que existir dentro do
projects_dir)."""
import json
import os
import re
from pathlib import Path
from typing import Optional

from pydantic import BaseModel

from app.config import settings
from app.transcript import parse_obj

_PROJ_RE = re.compile(r"^[A-Za-z0-9-]+$")   # nomes de dir gerados por sanitize_cwd
_SID_RE = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")


class ArchiveFolder(BaseModel):
    # Nivel 1 (pastas): agregado barato — nao abre os transcripts (so 1 leitura de cabecalho por
    # pasta pro cwd real). O preview das conversas so e pago ao ENTRAR na pasta.
    project: str                 # nome do dir em projects/ (cwd sanitizado)
    cwd: Optional[str] = None    # cwd real, lido do transcript mais recente
    count: int                   # quantas conversas na pasta
    mtime: float                 # atividade mais recente


class ArchiveEntry(BaseModel):
    project: str                 # nome do dir em projects/ (cwd sanitizado)
    cwd: Optional[str] = None    # cwd real, lido de dentro do transcript
    session_id: str
    mtime: float
    preview: str                 # 1a msg de usuario (identifica "qual conversa e essa")
    live: bool = False           # em uso por uma sessao tmux viva agora


def _head_info(jsonl: Path, max_lines: int = 60) -> tuple[str, Optional[str]]:
    # (preview, cwd) lendo so o COMECO do arquivo (early-exit; transcript pode ter dezenas de MB).
    preview: str = ""
    cwd: Optional[str] = None
    try:
        with open(jsonl, encoding="utf-8", errors="replace") as fh:
            for _, line in zip(range(max_lines), fh):
                try:
                    obj = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                if cwd is None:
                    c = obj.get("cwd")
                    if isinstance(c, str) and c:
                        cwd = c
                if not preview:
                    for ev in parse_obj(obj):
                        if ev.kind == "user_msg" and ev.text:
                            preview = ev.text[:100]
                            break
                if preview and cwd:
                    break
    except OSError:
        pass
    return preview, cwd


def _folder_files(proj: Path) -> list[tuple[float, Path]]:
    files: list[tuple[float, Path]] = []
    for f in proj.glob("*.jsonl"):
        try:
            files.append((f.stat().st_mtime, f))
        except OSError:
            continue
    files.sort(key=lambda t: t[0], reverse=True)
    return files


def list_folders() -> list[ArchiveFolder]:
    # Nivel 1: as pastas (projetos) com conversa arquivada, mais recentes primeiro.
    base = Path(settings.projects_dir)
    out: list[ArchiveFolder] = []
    try:
        projdirs = [d for d in base.iterdir() if d.is_dir()]
    except OSError:
        return []
    for proj in projdirs:
        files = _folder_files(proj)
        if not files:
            continue
        _, cwd = _head_info(files[0][1])   # cwd real do transcript mais recente (1 leitura/pasta)
        out.append(ArchiveFolder(project=proj.name, cwd=cwd, count=len(files), mtime=files[0][0]))
    out.sort(key=lambda e: e.mtime, reverse=True)
    return out


def list_conversations(project: str, live_realpaths: set[str], cap: int = 100) -> list[ArchiveEntry]:
    """Nivel 2: conversas de UMA pasta, mais recentes primeiro. O preview abre cada arquivo -> o
    teto limita o custo por request. ValueError/FileNotFoundError como archive_jsonl."""
    if not _PROJ_RE.match(project):
        raise ValueError("caminho invalido")
    proj = Path(settings.projects_dir) / project
    if not proj.is_dir():
        raise FileNotFoundError(project)
    out: list[ArchiveEntry] = []
    for mt, f in _folder_files(proj)[:cap]:
        preview, cwd = _head_info(f)
        out.append(ArchiveEntry(
            project=project, cwd=cwd, session_id=f.stem, mtime=mt,
            preview=preview, live=os.path.realpath(str(f)) in live_realpaths,
        ))
    return out


def archive_jsonl(project: str, session_id: str) -> Path:
    """Path validado do transcript arquivado. ValueError = componente invalido (traversal barrado);
    FileNotFoundError = nao existe."""
    if not _PROJ_RE.match(project) or not _SID_RE.match(session_id):
        raise ValueError("caminho invalido")
    p = Path(settings.projects_dir) / project / f"{session_id}.jsonl"
    if not p.is_file():
        raise FileNotFoundError(str(p))
    return p


def archive_cwd(project: str, session_id: str) -> Optional[str]:
    """cwd real da conversa arquivada (lido do cabecalho do transcript) -- usado pra retomar (feature
    'Retomar conversa'): a sessao tmux nova precisa nascer no MESMO cwd da conversa original. Mesma
    validacao de archive_jsonl (propaga ValueError/FileNotFoundError); None = cwd nao ficou gravado
    nas primeiras linhas do transcript (conversa nao pode ser retomada)."""
    p = archive_jsonl(project, session_id)
    _, cwd = _head_info(p)
    return cwd
