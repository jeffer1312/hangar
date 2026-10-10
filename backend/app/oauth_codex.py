"""Login OAuth do ChatGPT (o do Codex) espalhado pros CLIs que o aceitam: Codex, Pi e omp.

Os três fazem o MESMO login e guardam o resultado cada um no seu formato. Quem faz o fluxo de
dispositivo, guarda o cofre (`~/.hangar/auth/openai-codex.json`, 0600) e escreve nos stores é o
Rust; sem ele não há fluxo Python e a importação e a propagação respondem `accounts_need_rust_server`.
Daqui sai só a leitura do cofre e do login do Codex que a saúde dos harnesses usa.
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

from app import atomico
from app.agentes_sync import _codex_dir

PROVEDOR = "openai-codex"


def cofre() -> Path:
    return Path.home() / ".hangar" / "auth" / "openai-codex.json"


def _omp_db(home: Path | None) -> Path:
    from app import omp_dirs
    # `home` explícito é semente de teste: raiz fixa, sem perfil nem variável de ambiente.
    # Estrito: a saúde dos harnesses GRAVA credencial nele; perfil inválido falha, não cai na raiz errada.
    return omp_dirs.agent_dir(home=home, env={} if home else None, estrito=True) / "agent.db"


@dataclass
class Tokens:
    access: str
    refresh: str
    id_token: str
    expires_ms: int
    account_id: str
    plano: str = ""


def _gravar_json(alvo: Path, dados: dict) -> None:
    """tmp+rename, e o tmp já NASCE 0600: token em arquivo 0644 por um instante é vazamento."""
    tmp = alvo.with_name(f"{alvo.name}.{os.getpid()}.hangar.tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        fh.write(json.dumps(dados, indent=2) + "\n")
    atomico.substituir(tmp, alvo)


def ler_cofre() -> Tokens | None:
    try:
        d = json.loads(cofre().read_text(encoding="utf-8"))
        return Tokens(**{k: d[k] for k in Tokens.__dataclass_fields__ if k in d})
    except (OSError, ValueError, TypeError):
        return None


def _codex_tem_login(home: Path | None) -> bool:
    try:
        d = json.loads((_codex_dir(home) / "auth.json").read_text(encoding="utf-8"))
        return bool((d.get("tokens") or {}).get("refresh_token"))
    except (OSError, ValueError, AttributeError):
        return False


def importar_do_codex() -> Tokens | None:
    """Quem já logou pelo `codex login` não precisa logar de novo: o Rust monta o cofre do auth.json dele."""
    from app.account_bridge import request_device
    request_device("import")
    return ler_cofre()


def propagar() -> dict[str, dict]:
    from app.account_bridge import request_device
    return request_device("propagate")
