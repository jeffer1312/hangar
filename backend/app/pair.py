"""Leitura do arquivo de grupo (feature "trabalhando juntas"). Quem grava é o Rust; aqui só a lista
do modo `python`/convidado, o `orq` e o bastão leem. Dir irmão ".hangar-pair", um JSON por MEMBRO,
keyed pelo NOME:

    {"peers": ["outra", "mais-uma"], "task": "...", "gid": "ab12cd34"}

peers = os OUTROS membros do grupo. gid = id estável do grupo — nomeia o arquivo de CONTRATO.
Formato legado {"peer": "x"} (1:1) é lido como {"peers": ["x"]}."""
import hashlib
import json
import re
from pathlib import Path

from app.config import settings


def _sanitize(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]", "-", name)


def _pair_dir() -> Path:
    d = Path(settings.projects_dir).parent / ".hangar-pair"
    d.mkdir(parents=True, exist_ok=True)
    return d


def _gid_legado(name: str, peers: list[str]) -> str:
    """gid derivado do CONJUNTO de membros, pra sidecar escrito antes de o gid existir (par 1:1).

    Todo membro deriva do mesmo conjunto ordenado, então os sidecars de um mesmo grupo caem no mesmo
    valor sem combinarem nada — que é o que a lista do app precisa pra agrupá-los. Não é gravado:
    a leitura roda a cada poll da lista."""
    return hashlib.sha1("\n".join(sorted([name, *peers])).encode("utf-8")).hexdigest()[:8]


class PairLink:
    """Sidecar de UM membro (<nome>.json). get() normaliza o formato legado 1:1."""

    def __init__(self, name: str):
        self.name = name
        self.path = _pair_dir() / f"{_sanitize(name)}.json"

    def get(self) -> dict | None:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, ValueError):
            return None
        if not isinstance(data, dict):
            return None
        if "peers" not in data and data.get("peer"):
            data["peers"] = [data["peer"]]
        if not isinstance(data.get("peers", []), list):
            return None
        peers = [p for p in (data.get("peers") or []) if p]
        # Sem peers só o grupo de orquestração existe: o árbitro do `orquestrar-auto` nasce sozinho
        # e o time chega depois, pelo orquestrador.
        if not peers and data.get("orq") is not True:
            return None
        harness = data.get("harness")
        return {"peers": peers, "task": data.get("task", ""),
                "gid": data.get("gid") or _gid_legado(self.name, peers),
                "harness": harness if isinstance(harness, dict) else {},
                "orq": data.get("orq") is True, "dona": fed.get("owner") if isinstance(fed := data.get("fed"), dict) and fed.get("local") is False else None}


def _arquivo_dir() -> Path:
    # O cofre (~/.hangar), não o config dir de uma conta — mesma razão do orq.raiz_padrao().
    return Path.home() / ".hangar" / "pair-arquivo"


def contract_path_for(name: str) -> Path | None:
    """Arquivo de CONTRATO do grupo de `name` (markdown, keyed pelo gid). None = sem grupo."""
    link = PairLink(name).get()
    if not link or not link.get("gid"):
        return None
    return _pair_dir() / f"grupo-{link['gid']}.md"
