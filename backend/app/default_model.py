"""Guarda e devolve o `"model"` do settings.json do Claude Code.

Existe por causa de UMA medicao (31/07/2026, claude 2.1.220): numa sessao de MOTOR o picker do
`/model` so lista os 4 aliases (todos apontando pro mesmo `ANTHROPIC_MODEL`), entao a unica forma de
trocar pra outro modelo do provedor e o comando com argumento — `/model kimi-for-coding`. E ele
responde:

    Set model to kimi-for-coding and saved as your default for new sessions

ou seja, grava `"model": "kimi-for-coding"` no settings.json GLOBAL. A troca era pra valer so na
sessao de motor; do jeito que sai da caixa ela vaza pra toda sessao nova — inclusive as da conta
Anthropic, que nasceriam pedindo um modelo que a API da Anthropic nao conhece.

A sessao viva ja aplicou o modelo em memoria (medido: a statusline troca na hora e nao volta), entao
regravar o valor anterior depois do comando desfaz o vazamento sem desfazer a troca.

Blindagem igual a do hook_installer: arquivo quebrado/estranho a mao NAO e tocado — perder a config
do usuario e pior que deixar o default trocado (que ele ve na tela e conserta).
"""
import json
import logging
import os
import tempfile
import time
from pathlib import Path
from typing import Any

_log = logging.getLogger(__name__)

# A escrita do Claude Code NAO e sincrona com a linha de resultado no terminal: medido, o
# settings.json so muda ~0.8s depois do Enter. Repor antes disso e um no-op, e o valor vazado
# aterrissa DEPOIS — foi o que aconteceu na primeira medicao ao vivo (o terminal ja mostrava o
# resultado, o arquivo ainda tinha o valor velho).
_PRAZO_ESCRITA = 3.0
_INTERVALO = 0.15
# Segunda conferencia depois de repor: barata, e cobre a escrita que aterrissa logo apos.
_RECONFERE_APOS = 0.6

_AUSENTE = object()  # distingue "a chave nao existia" de "existia com valor None"


def _arquivo(config_dir: Path | None) -> Path:
    return (config_dir or Path.home() / ".claude") / "settings.json"


def _ler(path: Path) -> dict | None:
    try:
        bruto = path.read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        return {}
    except OSError as e:
        _log.warning("settings.json (%s) nao pode ser lido: %s", path, e)
        return None
    if not bruto:
        return {}
    try:
        d = json.loads(bruto)
    except ValueError as e:
        # Loga igual ao ramo do OSError acima: nao mexer no arquivo e a decisao CERTA, mas ela faz o
        # undo virar no-op — o id do motor fica como default global e ninguem descobre por que uma
        # sessao nova nasceu pedindo um modelo que a Anthropic nao conhece. Uma linha de log e o
        # unico rastro que sobra.
        _log.warning("settings.json (%s) esta com JSON invalido — nao mexido, e o default de modelo "
                     "pode ter ficado apontando pro motor: %s", path, e)
        return None
    return d if isinstance(d, dict) else None


def snapshot(config_dir: Path | None) -> Any:
    """Valor atual de settings.json["model"], ou o sentinela de ausente/ilegivel.

    Devolve `None` (nao o sentinela) quando o arquivo esta ilegivel: restore() ve isso e nao mexe.
    """
    d = _ler(_arquivo(config_dir))
    if d is None:
        return None
    return d.get("model", _AUSENTE)


def restore(config_dir: Path | None, antes: Any) -> bool:
    """Regrava o `"model"` anterior (ou o remove, se nao existia). True = mexeu no arquivo.

    `antes is None` = nao ha snapshot confiavel -> nao faz nada. Preserva todo o resto do arquivo:
    hooks, permissions, statusLine — a escrita e do arquivo inteiro, entao um dict parcial apagaria
    a config do usuario.
    """
    if antes is None:
        return False
    path = _arquivo(config_dir)
    d = _ler(path)
    if d is None:
        return False
    atual = d.get("model", _AUSENTE)
    if atual == antes:
        return False  # o comando nao mexeu (ou ja foi restaurado)
    if antes is _AUSENTE:
        d.pop("model", None)
    else:
        d["model"] = antes
    # tmp + replace: um corte no meio nao pode deixar o settings.json pela metade — seria a config
    # inteira do usuario perdida por causa de um undo.
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(d, fh, indent=2, ensure_ascii=False)
            fh.write("\n")
        os.replace(tmp, path)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
    return True


def restore_quando_aterrissar(config_dir: Path | None, antes: Any) -> bool:
    """Espera a escrita do Claude Code chegar ao disco e SO ENTAO repoe o valor anterior.

    Repor cedo demais nao repoe nada: o `restore()` ve o valor ainda inalterado, decide que nao ha
    o que desfazer, e o id do motor aterrissa no arquivo segundos depois — vazado, calado. Por isso
    aqui sondamos ate o valor MUDAR (ou o prazo estourar) e reconferimos uma vez depois de repor.

    Prazo estourado sem mudanca = o comando nao gravou nada; repor e um no-op e tudo bem.
    """
    if antes is None:
        return False
    fim = time.monotonic() + _PRAZO_ESCRITA
    while snapshot(config_dir) == antes and time.monotonic() < fim:
        time.sleep(_INTERVALO)
    mexeu = restore(config_dir, antes)
    time.sleep(_RECONFERE_APOS)
    return restore(config_dir, antes) or mexeu
