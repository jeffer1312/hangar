"""Liga/desliga o resumo do pensamento do Claude Code (`showThinkingSummaries`).

Por que NAO mora no runtime-config.json como o resto da tela de Servidor: quem le esta chave e o
proprio `claude`, no `settings.json`, no instante em que a sessao nasce. Guardar uma copia aqui
daria dois valores que divergem no minuto em que alguem editar o arquivo a mao ou rodar `/config`.
Entao a verdade e o arquivo dele, e este modulo so sabe ler e escrever essa chave.

Medido em 29/08/2026 (claude 2.1.246, opus-5): sem a chave, o CLI pede o pensamento com
`display: "omitted"` e a API devolve o bloco CIFRADO — `thinking: ""` mais a assinatura. Com ela,
vem o resumo em texto, e ele cai no `.jsonl` como qualquer bloco. Vale so pra sessao NOVA: a forma
de exibicao e decidida na largada.

Escreve no settings.json COMPARTILHADO (`~/.claude`), nao no da conta: `contas._espelhar_do_principal`
copia as chaves do principal pra cada conta na proxima abertura dela, entao gravar aqui e o unico
jeito de a escolha valer pra todas sem escrever em N arquivos que podem estar sendo lidos agora.
"""
import json
import logging
import os
import uuid
from pathlib import Path

from app import atomico
from app.contas import compartilhado

CHAVE = "showThinkingSummaries"

_log = logging.getLogger("hangar.pensamento")


def _arquivo() -> Path:
    return compartilhado() / "settings.json"


def _ler() -> dict | None:
    """Conteudo do settings.json, ou None quando ele nao da pra ler.

    None e "nao mexa": um arquivo ilegivel reescrito daqui apagaria hooks, permissions e statusLine
    do usuario. Ausente e diferente — e `{}`, e a primeira gravacao o cria.
    """
    try:
        bruto = _arquivo().read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        return {}
    except OSError as e:
        _log.warning("settings.json nao pode ser lido: %s", e)
        return None
    if not bruto:
        return {}
    try:
        d = json.loads(bruto)
    except ValueError as e:
        _log.warning("settings.json esta com JSON invalido — nao mexido: %s", e)
        return None
    return d if isinstance(d, dict) else None


def ler() -> bool:
    """Valor efetivo. Chave ausente = desligado (o padrao do proprio Claude Code)."""
    d = _ler()
    return bool(d.get(CHAVE)) if d else False


def definido() -> bool:
    """A chave esta escrita no arquivo? (a tela usa pra marcar a linha como editada)"""
    d = _ler()
    return d is not None and CHAVE in d


def gravar(valor: bool) -> None:
    """Grava a chave preservando todo o resto do arquivo.

    tmp+replace com pid+uuid no nome, como o `contas._espelhar_do_principal`: um `claude` vivo lendo
    o settings.json no meio da escrita pegaria JSON truncado, e o nome fixo ainda deixaria duas
    gravacoes simultaneas se atropelarem.
    """
    d = _ler()
    if d is None:
        raise RuntimeError("settings.json ilegivel — nao mexi nele")
    if bool(d.get(CHAVE)) == valor and CHAVE in d:
        return
    d[CHAVE] = valor
    destino = _arquivo()
    destino.parent.mkdir(parents=True, exist_ok=True)
    tmp = destino.with_name(f"{destino.name}.hangar-novo.{os.getpid()}.{uuid.uuid4().hex[:8]}")
    try:
        tmp.write_text(json.dumps(d, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        atomico.substituir(tmp, destino)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
