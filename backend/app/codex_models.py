"""Adaptador dos modelos Codex no Rust; cache, validação e processos pertencem ao servidor."""
from pathlib import Path
from app import dictation_bridge, diag
from app.codex_appserver import CodexAusente, CodexIndisponivel, CodexRecusado, CodexRespostaInvalida


class CodexLimitado(CodexIndisponivel):
    """O catálogo da conta respondeu com limitação de uso."""


def _call(operation: str, payload: dict) -> dict:
    try:
        return dictation_bridge.request(operation, payload)
    except dictation_bridge.BridgeError as error:
        if error.code == "dictation_cli_missing":
            raise CodexAusente(error.detail) from None
        if error.code == "dictation_catalog_rate_limited":
            raise CodexLimitado(error.detail) from None
        if error.code == "dictation_catalog_refused":
            raise CodexRecusado(error.detail) from None
        if operation in ("parse_models", "raw_model"):
            raise CodexRespostaInvalida(error.detail) from None
        if error.status == 400:
            raise ValueError(error.detail) from None
        raise CodexIndisponivel(error.detail) from None


def invalidar(codex_home: str | Path | None = None) -> None:
    try:
        _call("invalidate_models", {"home": str(codex_home) if codex_home is not None else None})
    except CodexIndisponivel:
        # Sem servidor não há cache local; a assinatura da credencial invalida a próxima leitura.
        diag.registrar("dictation.catalog_invalidation", "aviso", codigo="dictation_rust_unavailable")


def parse(result: dict) -> list[dict]:
    return _call("parse_models", {"provider": "codex", "catalog": result})["models"]


def listar(fresco: bool = False, *, codex_home: str | Path | None = None) -> list[dict]:
    return _call("catalog_models", {"provider": "codex", "home": str(codex_home) if codex_home is not None else None,
                                   "fresh": fresco})["models"]


def checar_escolha(model: str | None, effort: str | None, *, codex_home: str | Path | None = None,
                   service_tier: str | None = None) -> None:
    models = listar(codex_home=codex_home) if model is not None else []
    _call("validate_model", {"models": models, "model": model, "effort": effort, "service_tier": service_tier})


def raw_model(codex_home: str | Path, effective_config: dict, slug: str, version: str) -> dict:
    return _call("raw_model", {"home": str(codex_home), "config": effective_config, "model": slug, "version": version})["model"]
