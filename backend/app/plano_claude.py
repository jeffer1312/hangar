"""Descoberta do plano nativo associado a uma sessão Claude Code."""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any


_SLUG = re.compile(r"^[a-z0-9][a-z0-9-]{0,119}$")


@dataclass(frozen=True)
class PlanoClaude:
    nome: str
    caminho: Path
    anchor_id: str | None = None


def _ler_json(caminho: Path) -> dict[str, Any]:
    try:
        valor = json.loads(caminho.read_text(encoding="utf-8"))
        return valor if isinstance(valor, dict) else {}
    except (OSError, ValueError):
        return {}


def _config_dir_do_transcript(transcript: Path) -> Path:
    """O transcript Claude sempre fica em <config>/projects/<slug>/<id>.jsonl."""
    pais = transcript.parents
    if len(pais) >= 3 and pais[1].name == "projects":
        return pais[2]
    return Path.home() / ".claude"


def _diretorio_de_planos(transcript: Path, cwd: Path) -> Path:
    config_dir = _config_dir_do_transcript(transcript)
    configuracao: dict[str, Any] = {}
    for arquivo in (
        config_dir / "settings.json",
        config_dir / "settings.local.json",
        cwd / ".claude" / "settings.json",
        cwd / ".claude" / "settings.local.json",
    ):
        configuracao.update(_ler_json(arquivo))

    bruto = configuracao.get("plansDirectory")
    if not isinstance(bruto, str) or not bruto.strip():
        return (config_dir / "plans").resolve()
    caminho = Path(bruto).expanduser()
    return (caminho if caminho.is_absolute() else cwd / caminho).resolve()


def _caminho_de_bloco(bloco: dict[str, Any]) -> str | None:
    entrada = bloco.get("input")
    if not isinstance(entrada, dict):
        return None
    if bloco.get("name") in {"Write", "Edit"}:
        caminho = entrada.get("file_path")
        return caminho if isinstance(caminho, str) else None
    # Mantém compatibilidade com versões que venham a expor o arquivo no evento do modo plano.
    if bloco.get("name") == "ExitPlanMode":
        caminho = entrada.get("planFilePath")
        return caminho if isinstance(caminho, str) else None
    return None


def _id_bloco_assistente(evento: dict[str, Any], *, tipo: str, valor: str | None = None) -> str | None:
    """Converte um bloco do transcript no mesmo id que `transcript.parse_obj` publica."""
    uuid = evento.get("uuid")
    mensagem = evento.get("message")
    conteudo = mensagem.get("content") if isinstance(mensagem, dict) else None
    if not isinstance(uuid, str) or not isinstance(conteudo, list):
        return None
    indice = 0
    for bloco in conteudo:
        if not isinstance(bloco, dict):
            continue
        bloco_tipo = bloco.get("type")
        if bloco_tipo == "tool_use":
            if tipo == "tool_use" and bloco.get("id") == valor:
                return uuid if indice == 0 else f"{uuid}:{indice}"
            indice += 1
        elif bloco_tipo == "text":
            texto = bloco.get("text")
            if tipo == "text" and isinstance(texto, str) and texto.strip():
                return uuid if indice == 0 else f"{uuid}:{indice}"
            indice += 1
        elif bloco_tipo == "thinking" and isinstance(bloco.get("thinking"), str) and bloco["thinking"].strip():
            indice += 1
    return None


def _eh_prompt_humano(conteudo: Any) -> bool:
    if isinstance(conteudo, str):
        return bool(conteudo.strip())
    if not isinstance(conteudo, list) or not conteudo:
        return False
    return not all(isinstance(bloco, dict) and bloco.get("type") == "tool_result" for bloco in conteudo)


def descobrir(transcript: str | Path, cwd: str | Path) -> PlanoClaude | None:
    """Encontra a proposta ainda sem resposta no transcript principal da sessão."""
    arquivo = Path(transcript)
    raiz = _diretorio_de_planos(arquivo, Path(cwd))
    candidatos: dict[str, tuple[Path, str | None]] = {}
    saidas_do_modo_plano: dict[str, tuple[str, str | None]] = {}
    ultimo: tuple[Path, str | None] | None = None
    ultimo_slug: str | None = None
    slug_confirmado: tuple[str, str | None] | None = None
    aguardando_resposta: tuple[Path, str | None] | None = None
    aprovado = False

    try:
        linhas = arquivo.open(encoding="utf-8")
    except OSError:
        return None
    with linhas:
        for linha in linhas:
            try:
                evento = json.loads(linha)
            except (ValueError, TypeError):
                continue
            if not isinstance(evento, dict) or evento.get("isSidechain") is True:
                continue
            slug = evento.get("slug")
            if isinstance(slug, str) and _SLUG.fullmatch(slug):
                ultimo_slug = slug
            mensagem = evento.get("message")
            conteudo = mensagem.get("content") if isinstance(mensagem, dict) else None
            if evento.get("type") == "user" and _eh_prompt_humano(conteudo):
                aguardando_resposta = None
                ultimo = None
                slug_confirmado = None
                candidatos.clear()
                saidas_do_modo_plano.clear()
                aprovado = False
            if not isinstance(conteudo, list):
                continue
            if evento.get("type") == "assistant" and aguardando_resposta is not None:
                resposta = _id_bloco_assistente(evento, tipo="text")
                if resposta is not None:
                    ultimo = (aguardando_resposta[0], resposta)
                    aguardando_resposta = None
            for bloco in conteudo:
                if not isinstance(bloco, dict):
                    continue
                if bloco.get("type") == "tool_use":
                    identificador = bloco.get("id")
                    if bloco.get("name") in {"EnterPlanMode", "ExitPlanMode"}:
                        aprovado = False
                    if aprovado:
                        continue
                    if (bloco.get("name") == "ExitPlanMode" and isinstance(identificador, str)
                            and (ultimo_slug is not None or ultimo is not None)):
                        nome = ultimo_slug if ultimo_slug is not None else ultimo[0].stem
                        ancora = _id_bloco_assistente(evento, tipo="tool_use", valor=identificador)
                        saidas_do_modo_plano[identificador] = (
                            nome, ancora,
                        )
                        caminho = ultimo[0] if ultimo is not None else (raiz / f"{nome}.md").resolve()
                        ultimo = (caminho, ancora)
                        aguardando_resposta = None
                    bruto = _caminho_de_bloco(bloco)
                    if not bruto or not isinstance(identificador, str):
                        continue
                    caminho = Path(bruto).expanduser()
                    caminho = (caminho if caminho.is_absolute() else Path(cwd) / caminho).resolve()
                    if caminho.suffix.lower() == ".md" and caminho.is_relative_to(raiz):
                        candidatos[identificador] = (
                            caminho,
                            _id_bloco_assistente(evento, tipo="tool_use", valor=identificador),
                        )
                elif bloco.get("type") == "tool_result":
                    identificador = bloco.get("tool_use_id")
                    candidato = candidatos.pop(identificador, None)
                    if candidato is not None and bloco.get("is_error") is not True:
                        ultimo = candidato
                        aguardando_resposta = candidato
                    saida = saidas_do_modo_plano.pop(identificador, None)
                    if saida is not None and bloco.get("is_error") is not True:
                        texto = bloco.get("content")
                        # ExitPlanMode também pode apenas encaminhar o plano para um líder.
                        if isinstance(texto, str) and texto.startswith((
                            "User has approved your plan.",
                            "User has approved exiting plan mode.",
                            "User has approved the plan.",
                        )):
                            ultimo = None
                            slug_confirmado = None
                            aguardando_resposta = None
                            candidatos.clear()
                            aprovado = True
                            continue
                        slug_confirmado = saida
                        if candidato is not None:
                            caminho = candidato[0]
                        elif ultimo is not None:
                            caminho = ultimo[0]
                        else:
                            caminho = (raiz / f"{saida[0]}.md").resolve()
                        ancora = saida[1] or (ultimo[1] if ultimo is not None else None)
                        ultimo = (caminho, ancora)
                        aguardando_resposta = (caminho, ancora)

    # Uma escrita confirmada traz o caminho exato e prevalece sobre o slug genérico da sessão.
    if ultimo is not None:
        return PlanoClaude(nome=ultimo[0].stem, caminho=ultimo[0], anchor_id=ultimo[1])
    if slug_confirmado is not None:
        caminho = (raiz / f"{slug_confirmado[0]}.md").resolve()
        # A validação do slug torna a checagem redundante no caso normal, mas mantém a fronteira
        # explícita caso a construção do caminho mude no futuro.
        if caminho.is_relative_to(raiz):
            return PlanoClaude(nome=slug_confirmado[0], caminho=caminho, anchor_id=slug_confirmado[1])
    return None
