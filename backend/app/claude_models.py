"""Adaptador do catálogo Claude no Rust, sem processo ou catálogo de reserva em Python."""
from pathlib import Path
from app import dictation_bridge


class ClaudeIndisponivel(RuntimeError):
    """O catálogo não pôde responder ao pedido."""


class ClaudeAusente(ClaudeIndisponivel):
    """O executável Claude não está disponível no servidor."""


def _call(operation: str, payload: dict) -> dict:
    try:
        return dictation_bridge.request(operation, payload)
    except dictation_bridge.BridgeError as error:
        kind = ClaudeAusente if error.code == "dictation_cli_missing" else ClaudeIndisponivel
        raise kind(error.detail) from None


def listar(config_dir: str | Path | None = None, timeout: float = 30.0) -> list[dict]:
    return _call("catalog_models", {"provider": "claude", "home": str(config_dir) if config_dir is not None else None,
                                   "timeout": timeout})["raw"]


def para_tela(modelos: list[dict], atual: str | None = None) -> list[dict]:
    return _call("parse_models", {"provider": "claude", "catalog": {"models": modelos}, "current": atual})["models"]
