"""Dados do usuário (`.env`, `peers.json`, `projects.json`) moram em `~/.hangar/config/`, fora da
pasta da versão. Máquina ainda não migrada os tem na `backend/`: vale o lugar novo quando o arquivo
está lá; senão, o antigo. No pacote é sempre o novo: a `backend/` é da versão, que a atualização apaga.
Quem grava grava no caminho devolvido, então fora do pacote nunca cria a pasta nova."""
from pathlib import Path

_BACKEND = Path(__file__).resolve().parent.parent


def path(name: str) -> Path:
    new = Path.home() / ".hangar" / "config" / name
    return new if new.exists() or (_BACKEND.parent / "manifest.json").exists() or (Path.home() / ".hangar" / "current").exists() else _BACKEND / name
