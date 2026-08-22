"""Apelido de credencial — o nome que a PESSOA deu, no lugar do nome que o disco impôs.

Sem isto o nome de uma conta é sempre derivado: `~/.claude-jefferson` vira "jefferson", `~/.claude`
vira "default", e uma chave de API herda a chave do arquivo do provedor — foi assim que a faixa de
cota acabou mostrando uma conta chamada "apikey", que não diz nada a ninguém.

Guardado num JSON só, na pasta COMPARTILHADA (`contas.compartilhado()`, sempre o `~/.claude` real):
o apelido é do app, não da conta. Guardá-lo dentro de cada pasta faria a conta apagada levar o
apelido junto e, pior, dependeria de gravar dentro de uma pasta que pode estar em uso por um CLI.

Chave do mapa = o `id` da credencial (`claude:<path>` / `chave:<nome>`), o mesmo id que a lista
unificada e a faixa de cota usam. Nunca o rótulo: rótulo é o que muda.
"""
import json
import logging
import os
import threading
from pathlib import Path

from app import atomico, contas

_log = logging.getLogger("claude_pocket.apelidos")

_ARQUIVO = ".claude-pocket-apelidos.json"
_MAX = 40                      # cabe na faixa do rodapé sem virar reticências
_lock = threading.Lock()


def _caminho() -> Path:
    return contas.compartilhado() / _ARQUIVO


def ler() -> dict[str, str]:
    """{id: apelido}. Arquivo ausente, ilegível ou do tipo errado = mapa vazio.

    Um apelido é enfeite: nunca pode derrubar a listagem de contas (mesma régua do
    `statusline.read()` exigindo dict).
    """
    try:
        bruto = json.loads(_caminho().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    if not isinstance(bruto, dict):
        return {}
    return {k: v.strip() for k, v in bruto.items()
            if isinstance(k, str) and isinstance(v, str) and v.strip()}


def de(id_credencial: str, natural: str) -> str:
    """O nome a mostrar: o apelido, se houver; senão o nome que veio do disco."""
    return ler().get(id_credencial) or natural


def definir(id_credencial: str, apelido: str | None) -> dict[str, str]:
    """Grava (ou apaga, com vazio/None) o apelido e devolve o mapa novo.

    tmp+rename com o pid no nome do temporário: o mesmo cuidado do sidecar de statusline — dois
    pedidos simultâneos com nome fixo promovem bytes entrelaçados no rename.
    """
    limpo = (apelido or "").strip()[:_MAX]
    with _lock:
        atual = ler()
        if limpo:
            atual[id_credencial] = limpo
        else:
            atual.pop(id_credencial, None)
        alvo = _caminho()
        tmp = alvo.with_suffix(f".tmp{os.getpid()}")
        try:
            alvo.parent.mkdir(parents=True, exist_ok=True)
            tmp.write_text(json.dumps(atual, ensure_ascii=False, indent=1), encoding="utf-8")
            atomico.substituir(tmp, alvo)
        except OSError as e:
            tmp.unlink(missing_ok=True)
            _log.warning("apelidos: nao deu pra gravar %s: %r", alvo, e)
            raise
        return atual
