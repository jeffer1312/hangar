"""Contexto estendido pela configuração oficial, respeitando o teto nativo de cada modelo."""
import asyncio
import json
import tomllib

from app import runtime_config
from app.codex_arquivos import exclusivo, gravar, json_bytes, json_obj, ler

CONTEXTO = 1_000_000
COMPACTACAO = 900_000


def ler_opcoes(servico) -> dict:
    raw = ler(servico.codex_home / "config.toml")
    config = tomllib.loads(raw.decode()) if raw else {}
    modelos = []
    try:
        catalogo = json.loads((servico.codex_home / "models_cache.json").read_text())
        for model in catalogo.get("models", []):
            if model.get("visibility", "list") != "list":
                continue
            base, limite = model.get("context_window"), model.get("max_context_window")
            if isinstance(base, int) and isinstance(limite, int) and limite > base:
                modelos.append({"model": model["slug"], "default": base, "max": limite})
    except (OSError, ValueError, KeyError, TypeError):
        pass
    atual = config.get("model_context_window")
    # Sem valor no config.toml o Hangar abre o Codex com 1M, salvo quando a pessoa desligou.
    estendido = (atual >= CONTEXTO if isinstance(atual, int)
                 else atual is None and runtime_config.get("codex_extended_context") is not False)
    return {"contexto_estendido": estendido,
            "contexto_configurado": atual, "compactacao": config.get("model_auto_compact_token_limit"),
            "modelos": modelos}


async def salvar_opcoes(servico, habilitado: bool) -> dict:
    async with exclusivo(servico.codex_home / ".hangar-integracao.lock"):
        servico.raiz.mkdir(parents=True, exist_ok=True, mode=0o700)
        path = servico.raiz / "contexto-anterior.json"

        def preparar(config):
            atual = config.get("model_context_window")
            compactacao = config.get("model_auto_compact_token_limit")
            estendido = isinstance(atual, int) and atual >= CONTEXTO
            registro = json_obj(path)
            if habilitado:
                if estendido and compactacao == COMPACTACAO:
                    return [], lambda: None
                # Guarda antes de gravar a config: uma queda não perde o valor a restaurar.
                if not estendido:
                    registro = {"anterior": atual, "compactacao_anterior": compactacao}
                elif "compactacao_anterior" not in registro:
                    registro = {**registro, "compactacao_anterior": compactacao}
                gravar(path, json_bytes(registro), ler(path))
                querido = CONTEXTO
                compactacao_querida = COMPACTACAO
            else:
                if not estendido:
                    return [], lambda: None
                querido = registro.get("anterior")
                if isinstance(querido, int) and querido >= CONTEXTO:
                    querido = None
                compactacao_querida = (registro.get("compactacao_anterior")
                                      if compactacao == COMPACTACAO else compactacao)
            edits = [{"keyPath": chave, "value": valor, "mergeStrategy": "replace"}
                     for chave, valor in (("model_context_window", querido),
                                          ("model_auto_compact_token_limit", compactacao_querida))]
            return edits, lambda: None

        await servico._editar_config(preparar)
        await asyncio.to_thread(runtime_config.aplicar, {"codex_extended_context": habilitado})
    return ler_opcoes(servico)
