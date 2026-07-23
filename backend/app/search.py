"""Busca de conteudo cross-session: UM `rg` capado sobre projects_dir varre todos os transcripts
.jsonl (vivos + arquivados) e devolve os trechos casados. Reusa o join live/dead do archive/registry
(realpath do jsonl no conjunto das sessoes vivas) pra a UI saber se abre o chat (viva) ou o arquivo
(morta).

SEGURANCA: o `rg` roda via subprocess com LISTA DE ARGUMENTOS (nunca shell=True, nunca string
interpolada). A query e tratada como STRING LITERAL (-F fixed-string) e passada como ARGUMENTO (-e q)
-> nunca chega a um shell nem e interpretada como regex/flag. Caps HARD impedem que uma query ampla
despeje o mundo."""
import json
import os
import re
import shutil
import subprocess
from pathlib import Path
from typing import Optional

from pydantic import BaseModel

from app.config import settings
from app.archive import _head_info
from app.transcript import parse_obj

# Caps HARD (uma query ampla nao pode despejar o mundo).
_MAX_HITS = 50            # teto global de trechos devolvidos
_MAX_PER_FILE = 3         # -m: matches por arquivo (evita 1 transcript dominar o resultado)
_SNIPPET = 180            # tamanho do trecho legivel
_MAX_Q = 200             # comprimento maximo da query (excedente e truncado)
# ponytail: rede de seguranca; transcript acima disso e raro (rg com -m ja sai cedo por arquivo).
# Sobe se um dia um transcript legitimo passar disso e some da busca.
_MAX_FILESIZE = "64M"

# Resolve o binario uma vez; "rg" puro se nao achar (Popen entao levanta FileNotFoundError -> []).
_RG = shutil.which("rg") or "rg"


class SearchHit(BaseModel):
    project: str                        # nome do dir em projects/ (cwd sanitizado)
    session_id: str                     # stem do .jsonl
    session_name: Optional[str] = None  # nome tmux se a sessao esta VIVA -> abre o chat; None = arquivo
    cwd: Optional[str] = None           # cwd real da conversa
    line: str                           # trecho legivel (texto da msg), NAO o JSON cru
    mtime: float
    live: bool = False


def _snippet(raw: str, q: str) -> str:
    """Trecho legivel de uma linha casada. A linha e uma entrada JSON do transcript -> extrai o TEXTO
    da msg (ou o resultado de ferramenta) em vez de despejar o JSON cru; janela centrada no termo pra o
    match aparecer mesmo numa msg longa. Fallback: raw truncado (linha sem texto parseavel)."""
    text = ""
    try:
        obj = json.loads(raw)
        if isinstance(obj, dict):
            parts: list[str] = []
            for ev in parse_obj(obj):
                if ev.text:
                    parts.append(ev.text)
                elif ev.result:
                    parts.append(ev.result)
            text = " ".join(parts).strip()
    except (ValueError, TypeError):
        pass
    if not text:
        text = raw.strip()
    low, ql = text.lower(), q.lower()
    i = low.find(ql)
    if i < 0 or len(text) <= _SNIPPET:
        return text[:_SNIPPET]
    start = max(0, i - 40)   # abre um pouco antes do termo pra dar contexto
    return ("…" if start else "") + text[start:start + _SNIPPET]


def search(q: str, live_names: dict[str, str], limit: int = _MAX_HITS) -> list[SearchHit]:
    """Trechos de conteudo em todos os transcripts .jsonl sob projects_dir.

    `live_names`: realpath(jsonl) -> nome tmux das sessoes VIVAS (o mesmo join que o archive faz com
    registry.list()); marca `live` e carrega `session_name` pra a UI abrir o chat (viva) ou o arquivo
    (morta). `q` blank/so-espaco -> []; truncada em _MAX_Q. Ordena por mtime desc (mais recente primeiro).
    """
    q = (q or "").strip()
    if not q:
        return []
    q = q[:_MAX_Q]
    base = Path(settings.projects_dir)
    if not base.is_dir():
        return []
    # LISTA de argumentos (sem shell). -F: q e string literal. -e q: q vai como VALOR da flag (mesmo
    # comecando com '-' nao vira flag). --no-ignore: nao pular transcript por um .gitignore/ignore.
    argv = [
        _RG, "--json", "-F", "--no-messages", "--no-ignore",
        "-m", str(_MAX_PER_FILE),
        "--max-filesize", _MAX_FILESIZE,
        "-g", "*.jsonl",
        "-e", q, "--", str(base),
    ]
    try:
        proc = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    except (OSError, ValueError):
        return []
    hits: list[SearchHit] = []
    cwd_cache: dict[str, Optional[str]] = {}  # realpath -> cwd real (1 leitura de cabecalho/arquivo)
    try:
        for raw in (proc.stdout or []):
            if len(hits) >= limit:
                break   # teto global: para de ler (a leitura por streaming nao bufferiza o mundo)
            try:
                obj = json.loads(raw)
            except (ValueError, TypeError):
                continue
            if not isinstance(obj, dict) or obj.get("type") != "match":
                continue
            data = obj.get("data") or {}
            path = ((data.get("path") or {}).get("text")) or ""
            line = ((data.get("lines") or {}).get("text")) or ""
            if not path:
                continue
            p = Path(path)
            real = os.path.realpath(path)
            if real not in cwd_cache:
                # cwd: 1o do proprio JSON casado (barato); senao cabecalho do arquivo (reuso do archive).
                cwd: Optional[str] = None
                try:
                    j = json.loads(line)
                    if isinstance(j, dict):
                        c = j.get("cwd")
                        cwd = c if isinstance(c, str) and c else None
                except (ValueError, TypeError):
                    cwd = None
                if not cwd:
                    _, cwd = _head_info(p)
                cwd_cache[real] = cwd
            name = live_names.get(real)
            try:
                mtime = os.path.getmtime(path)
            except OSError:
                mtime = 0.0
            hits.append(SearchHit(
                project=p.parent.name, session_id=p.stem, session_name=name,
                cwd=cwd_cache[real], line=_snippet(line, q), mtime=mtime, live=name is not None,
            ))
    finally:
        if proc.stdout:
            proc.stdout.close()
        proc.terminate()   # bateu o teto -> mata o rg (nao deixa varrendo o resto em vao)
        proc.wait()
    hits.sort(key=lambda h: h.mtime, reverse=True)
    return hits


# Stopwords pt/en (busca lexical "onde falei sobre X" — RAG). Curta de proposito: so o ruido comum
# que degradaria o OR de termos; palavra de conteudo fica.
_ASK_STOPWORDS = {
    # pt
    "que", "qual", "quais", "quando", "onde", "como", "por", "porque", "para", "pra", "com", "sem",
    "dos", "das", "uma", "uns", "umas", "meu", "minha", "seu", "sua", "nos", "nas", "isso", "isto",
    "aquilo", "sobre", "falei", "falamos", "conversa", "conversamos", "disse", "mencionei", "assunto",
    "ele", "ela", "eles", "elas", "voce", "vc", "eu", "tem", "ter", "foi", "era", "sao", "num", "numa",
    # en
    "the", "and", "for", "with", "was", "were", "did", "does", "what", "when", "where", "which", "who",
    "about", "said", "told", "talked", "spoke", "mentioned", "this", "that", "these", "those", "you",
    "have", "has", "had", "are", "our", "their",
}


def extract_terms(question: str, max_terms: int = 6) -> list[str]:
    """Termos da pergunta pra busca lexical: minusculiza, tira stopwords pt/en e palavras <3 chars,
    dedup preservando ordem, no maximo `max_terms`."""
    out: list[str] = []
    seen: set[str] = set()
    for w in re.findall(r"\w+", (question or "").lower(), re.UNICODE):
        if len(w) < 3 or w in _ASK_STOPWORDS or w in seen:
            continue
        seen.add(w)
        out.append(w)
        if len(out) >= max_terms:
            break
    return out


_ASK_SYSTEM = (
    "Você responde onde um assunto apareceu nas conversas do usuário, a partir de TRECHOS de "
    "transcripts. Responda curto em pt-BR: diga em QUAL(is) sessão(ões) o assunto apareceu e o "
    "contexto, citando os NOMES EXATOS das sessões como aparecem nos rótulos. Se os trechos não "
    "responderem à pergunta, diga que não encontrou. Sem preâmbulo, sem markdown."
)


def build_ask_prompt(question: str, hits: list[SearchHit]) -> str:
    """Prompt do ask-history: pergunta + trechos rotulados '[sessão NOME — viva|arquivada]: trecho'."""
    lines = [_ASK_SYSTEM, "", f"PERGUNTA: {question}", "", "Trechos das conversas:"]
    for h in hits:
        name = h.session_name or h.project or h.session_id[:8]
        estado = "viva" if h.live else "arquivada"
        lines.append(f"[sessão {name} — {estado}]: {h.line}")
    return "\n".join(lines)


def search_terms(terms: list[str], live_names: dict[str, str], cap: int = 30) -> list[SearchHit]:
    """Busca OR: roda `search` por termo, dedup por (session_id, trecho), ordena por mtime desc, cap.
    Cada termo ja e capado em `search`; o cap global segura o custo do prompt do ask-history."""
    seen: set[tuple[str, str]] = set()
    out: list[SearchHit] = []
    for t in terms:
        for h in search(t, live_names, limit=cap):
            key = (h.session_id, h.line)
            if key in seen:
                continue
            seen.add(key)
            out.append(h)
    out.sort(key=lambda h: h.mtime, reverse=True)
    return out[:cap]
