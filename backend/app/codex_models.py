"""Catálogo de modelos do Codex: a fonte da lista na tela de ABERTURA, onde ainda não há sessão.

Por que não é como nenhum dos outros três: o `~/.codex/config.toml` guarda só o modelo escolhido
(`model = "..."`), nunca a lista — então não há o caminho do Kimi. E não existe `codex
--list-models`, então também não há o caminho do Pi. O que existe é o `model/list` do app-server, e
medido em 30/08/2026 (codex-cli 0.151.0) ele responde no modo **stdio**, sem `--listen`, sem thread
aberta e sem sessão viva, em 0,78s. É a mesma fonte que a folha da sessão viva usa
(`CodexAdapter.list_models`), só que por um processo efêmero em vez do app-server do pane.

Os `efforts` de cada modelo vêm daqui e não de uma lista no código porque **variam por modelo**
(medido: `gpt-5.6-sol` aceita `ultra`, `gpt-5.6-luna` não; `gpt-5.5` também não aceita `max`) — a
mesma lição que o Pi já tinha ensinado.

Cache pelo motivo do pi_catalog: é subprocess, e a lista muda de mês em mês.
"""
import time

from app import codex_appserver

# Reexportado porque a rota captura este erro por aqui, e ele não muda de significado no caminho
# do catálogo: "não achei o codex" continua sendo outra conversa que "o codex falhou".
CodexAusente = codex_appserver.CodexAusente

_TTL = 600.0
_cache: tuple[float, list[dict]] | None = None


def parse(result: dict) -> list[dict]:
    """A resposta do `model/list` no formato da tela. Estoura se não sobrar modelo nenhum."""
    out: list[dict] = []
    for m in result.get("data") or []:
        # `hidden` é o provedor dizendo "não ofereça este": oferecer faria a sessão nascer num id
        # que o plano do usuário não atende, e a falha só apareceria no primeiro turno.
        if not isinstance(m, dict) or m.get("hidden") or not m.get("model"):
            continue
        out.append({
            "id": m["model"],
            "name": m.get("displayName") or m["model"],
            "desc": m.get("description") or "",
            "efforts": [e.get("reasoningEffort") for e in (m.get("supportedReasoningEfforts") or [])
                        if isinstance(e, dict) and e.get("reasoningEffort")],
            # `default_effort` é o mesmo campo que o catálogo do Kimi já manda — a tela lê os dois
            # pelo mesmo `ModelOption`. O `isDefault` do provedor NÃO entra: quem decide o padrão
            # desta máquina é o `model` do `~/.codex/config.toml`, e mostrar o outro como "padrão"
            # apontaria pro modelo errado.
            "default_effort": m.get("defaultReasoningEffort"),
        })
    if not out:
        # Zero modelo com rc=0 é falha do provedor (login vencido, versão que mudou o schema), não
        # "seu plano não tem modelo". Levanta pra virar o 502 que a rota já sabe dar, e o caller
        # NÃO cacheia: senão o erro duraria 10 min depois de o Codex voltar.
        raise RuntimeError("codex app-server nao devolveu modelo nenhum em model/list")
    return out


def listar(fresco: bool = False) -> list[dict]:
    global _cache
    if _cache and not fresco and time.monotonic() - _cache[0] < _TTL:
        return _cache[1]
    modelos = parse(codex_appserver.perguntar("model/list"))
    _cache = (time.monotonic(), modelos)
    return modelos


def checar_escolha(model: str | None, effort: str | None) -> None:
    """Recusa (ValueError) modelo fora do catálogo, ou nível que AQUELE modelo não lista.

    `model_args` só valida a FORMA do nível — não pode ter lista fechada, porque os níveis variam
    por modelo. Sem esta checagem, `--effort ultra` num `gpt-5.5` nasce a sessão e o binário
    **descarta o nível calado** (medido em 30/08/2026: ele não morre, segue com o dele), ou seja, o
    app reportaria sucesso sobre uma escolha que não valeu. Mesma doutrina do `engine_model_set`:
    recusar aqui em vez de deixar a falha aparecer só no turno.

    Nível sem modelo não é checável (o modelo então é o do `~/.codex/config.toml`, que este
    catálogo não diz qual é) e passa.
    """
    if model is None:
        return
    for m in listar():
        if m["id"] == model:
            if effort is not None and effort not in m["efforts"]:
                raise ValueError(f"nivel fora do suporte de {model}: {effort!r} "
                                 f"(use um de {', '.join(m['efforts']) or 'nenhum'})")
            return
    raise ValueError(f"modelo fora do catalogo do Codex: {model}")
