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


def list_archive(live_realpaths: set[str], per_project: int = 30,
                 total_cap: int = 300) -> list[ArchiveEntry]:
    # Mais recentes primeiro, com teto por projeto e global (uma maquina antiga acumula milhares de
    # jsonl; o preview abre cada arquivo -> os tetos limitam o custo por request).
    base = Path(settings.projects_dir)
    out: list[ArchiveEntry] = []
    try:
        projdirs = [d for d in base.iterdir() if d.is_dir()]
    except OSError:
        return []
    for proj in projdirs:
        files: list[tuple[float, Path]] = []
        for f in proj.glob("*.jsonl"):
            try:
                files.append((f.stat().st_mtime, f))
            except OSError:
                continue
        files.sort(key=lambda t: t[0], reverse=True)
        for mt, f in files[:per_project]:
            preview, cwd = _head_info(f)
            out.append(ArchiveEntry(
                project=proj.name, cwd=cwd, session_id=f.stem, mtime=mt,
                preview=preview, live=os.path.realpath(str(f)) in live_realpaths,
            ))
    out.sort(key=lambda e: e.mtime, reverse=True)
    return out[:total_cap]


def archive_jsonl(project: str, session_id: str) -> Path:
    """Path validado do transcript arquivado. ValueError = componente invalido (traversal barrado);
    FileNotFoundError = nao existe."""
    if not _PROJ_RE.match(project) or not _SID_RE.match(session_id):
        raise ValueError("caminho invalido")
    p = Path(settings.projects_dir) / project / f"{session_id}.jsonl"
    if not p.is_file():
        raise FileNotFoundError(str(p))
    return p
