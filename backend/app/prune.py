"""Poda periodica de sidecars de sessao morta.

O app espalha sidecars por sessao em `<config>/.hangar-*`: statusline, estado,
preview, fila, bilhetes pane->sessao e catalogo de modelos do Pi. Nada disso sumia quando a
sessao morria (medido 18/08/2026: status 183, state 178, preview 142, fila 139 entradas,
pi 5,7 MB — e o backend ativo acumula mais; a mais antiga de 01/07/2026).

Por que nao e so arrumacao: sidecar de sessao morta MENTE. Dois arquivos velhos com nome de
pane reaproveitado reportavam um executor rodando outro modelo com custo 4,5x maior — o
perigo e ler dado velho achando que e o de agora.

Criterio CONSERVADOR e escrito (nada de "limpa tudo ao subir"):

1. A chave e o session-id (session_key() do jsonl), o nome da sessao (fila) ou o pane_id
   (bilhete) — NUNCA o nome do pane como identidade de conteudo. Sidecar cuja chave pertence
   a uma sessao viva NUNCA e apagado, por mais velho que seja.
2. Idade minima de _MIN_AGE: sessao morta RECENTE preserva os sidecars de proposito — foi
   lendo sidecar de execucao morta que o achado do custo errado foi descoberto, e as leituras
   ja tem tetos proprios (statusline 1 dia, preview 10 min) que impedem dado velho de mentir
   na tela. Depois de _MIN_AGE, sessao inexistente = lixo.

Onde roda: no lifespan do backend (app/api.py), primeira varredura no boot e depois a cada
_INTERVALO. O registry e polled (nao tem evento de "sessao sumiu"), entao a poda no registry
deixaria lixo de sessao que morreu fora da vista dele; e so no startup deixaria o backend de
longa duracao (unit systemd do usuario) acumulando para sempre. Periodica cobre os dois.
"""

import logging
import re
import time
from pathlib import Path

from app import tmux
from app.config import _backend_config_base, list_config_dirs

_log = logging.getLogger("claude_pocket.prune")

# Idade minima (s) de um sidecar orfao para entrar na poda. 7 dias: preserva a materia-prima
# de diagnostico de execucao morta recente (o caso t1/t2 da Task foi achado lendo sidecar de
# sessao morta) sem deixar o lixo acumular.
_MIN_AGE = 7 * 86400.0
# Intervalo (s) entre varreduras periodicas: 1x/dia basta — a leitura ja recusa sidecar velho
# (teto de 1 dia na statusline, 10 min na preview), entao poda e higiene, nao seguranca.
_INTERVALO = 24 * 3600.0

# Keyed pela chave de sessao (session_key do jsonl): statusline, estado, preview, askq e o
# catalogo de modelos do Pi (subdir de .hangar-pi).
_STEM_KEYED = (".hangar-status", ".hangar-state", ".hangar-preview",
               ".hangar-askq", ".hangar-pi/models")
# Keyed pelo NOME da sessao (sanitizado): a fila sobrevive ao /clear de proposito (o
# session-id muda no /clear), entao o nome e a chave certa dela.
_NOME_KEYED = ".hangar-queue"
# Keyed pelo pane_id (bilhete pane->sessao do Pi e do Kimi). O tmux REUSA %pane_id, entao o
# bilhete nunca decide quem e a sessao (isso e o frescor de pi_session_file) — a poda so tira
# bilhete de pane que NAO existe mais; pane vivo nunca tem bilhete podado.
_PANE_KEYED = (".hangar-pi", ".hangar-kimi")

# Sobra do `tmp+rename`: o processo morreu (kill -9, OOM, VM travando) entre o write e o rename e
# ninguem recolhe — nenhum `except` roda num kill -9, e o proximo render escreve com OUTRO pid no
# nome. Medido em 23/08/2026 nesta maquina: 31 arquivos, o mais antigo de 29/07, e um deles com o
# conteudo `{"text":` — nove bytes, a escrita cortada no meio, que e exatamente o que o tmp+rename
# existe pra nunca promover.
#
# Nao ha leitor: os quatro publicadores (preview_hook, state_hook, kimi_state_hook, os dois
# statusline em js/ts e o cp-state.ts) escrevem no tmp e renomeiam; quem consome le so o `.json`
# final. Entao a chave de sessao nao entra na conta aqui — o criterio conservador do resto do
# arquivo existe pra nao apagar sidecar que alguem AINDA le, e nao e o caso.
#
# Idade curta de proposito (1h, contra os 7 dias do resto): o unico risco e apagar o tmp de uma
# escrita EM VOO, e essa vive milissegundos. Sete dias adiariam a limpeza sem comprar seguranca
# nenhuma. Casa `.tmp`, `.tmp.<pid>` e `.tmp<pid>` — as quatro formas que os publicadores usam —,
# ancorado no FIM do nome pra nunca pegar um `<chave>.json`/`<nome>.jsonl` de verdade.
_MIN_AGE_TMP = 3600.0
_TMP_RE = re.compile(r"\.tmp\.?\d*$")


def _config_bases() -> list[Path]:
    # Mesmas bases dos outros leitores (statusline/hook_state): todos os config dirs das
    # contas + o base do backend.
    try:
        return list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
    except OSError:
        return [_backend_config_base()]


def _pane_ids_vivos() -> set[str]:
    """Pane_ids vivos, ou set() se nao der para saber.

    Cuidado: tmux.list_panes_all() devolve {} quando o comando falha (rc != 0) — ele NAO
    levanta. Entao o except abaixo so cobre o tmux que levanta (ex: import ausente); o tmux
    que responde {} NAO passa por ele e devolve set() igual. Quem distingue "set() = nao
    ha pane" de "set() = nao sei" e o guard de _podar, que pula a familia quando o
    conjunto esta vazio (ver la).
    """
    out: set[str] = set()
    try:
        for panes in tmux.list_panes_all().values():
            for p in panes:
                pid = p.get("pane_id") or ""
                if pid:
                    out.add(pid.lstrip("%"))
    except Exception:  # noqa: BLE001 — tmux que LEVANTA: nao poda pane-keyed, nunca derruba a poda
        _log.warning("prune: tmux levantou, bilhetes pane->sessao ficam", exc_info=True)
    return out


def _podar_dir(d: Path, chaves_vivas: set[str], agora: float, pattern: str = "*.json") -> int:
    """Apaga de UM diretorio os arquivos cuja chave nao esta viva E que passaram de _MIN_AGE.

    Falha-soft: arquivo que some no meio (sessao encerrando) ou sem stat nao derruba a
    varredura. O `.tmp` meio-escrito nao casa este glob (suffix diferente) — quem o recolhe e o
    `_podar_tmp`, por outro criterio.
    """
    if not d.is_dir():
        return 0
    n = 0
    try:
        for f in d.glob(pattern):
            try:
                if f.is_file() and f.stem not in chaves_vivas and agora - f.stat().st_mtime >= _MIN_AGE:
                    f.unlink()
                    n += 1
            except OSError:
                continue
    except OSError:
        pass
    return n


def _podar_tmp(d: Path, agora: float) -> int:
    """Sobra de `tmp+rename` em UM diretorio, por IDADE so — ver _MIN_AGE_TMP.

    Varre tambem os subdiretorios de primeiro nivel: o catalogo de modelos do Pi mora em
    `.hangar-pi/models`, e um `.tmp` la e tao orfao quanto os outros.
    """
    if not d.is_dir():
        return 0
    n = 0
    try:
        alvos = [d, *(x for x in d.iterdir() if x.is_dir())]
    except OSError:
        return 0
    for alvo in alvos:
        try:
            for f in alvo.iterdir():
                try:
                    if (f.is_file() and _TMP_RE.search(f.name)
                            and agora - f.stat().st_mtime >= _MIN_AGE_TMP):
                        f.unlink()
                        n += 1
                except OSError:
                    continue
        except OSError:
            continue
    return n


def _podar(bases: list[Path], chaves_stem: set[str], chaves_nome: set[str],
           chaves_pane: set[str], agora: float) -> dict[str, int]:
    """Varre as bases e devolve {subdir: quantos apagou}. Separada de prune_sidecars para o
    teste rodar com tmp_path, sem tocar nos config dirs reais do usuario."""
    # Conjunto de chaves VAZIO = "nao sei quem esta vivo", NUNCA "nada esta vivo". Sem este
    # guard o criterio conservador virava varredura por IDADE PURA: tmux fora do ar faz
    # list_panes_all devolver {} com rc!=0 (SEM levantar — o except nem dispara), e
    # registry.list() devolve [] junto; a 1a varredura roda no BOOT, quando e comum nao
    # haver sessao nenhuma ainda. Pular custa um ciclo de lixo; nao pular custa sidecar de
    # sessao viva.
    if not chaves_stem:
        _log.warning("prune: sem chave de sessao viva — %s ficam", ", ".join(_STEM_KEYED))
    if not chaves_nome:
        _log.warning("prune: sem nome de sessao viva — %s fica", _NOME_KEYED)
    if not chaves_pane:
        _log.warning("prune: sem pane vivo (tmux fora?) — %s ficam", ", ".join(_PANE_KEYED))
    apagados: dict[str, int] = {}
    for base in bases:
        # FORA dos guards de chave acima, e de proposito: `.tmp` orfao nao tem dono vivo pra
        # proteger — "nao sei quem esta vivo" nao muda em nada o fato de ninguem ler aquele
        # arquivo. Por dir de sidecar, seja ele keyed por stem, nome, pane ou por nada (o
        # `.hangar-active`, que a poda normal nem visita, tambem acumulava).
        try:
            for d in sorted(base.glob(".hangar-*")):
                n = _podar_tmp(d, agora)
                if n:
                    apagados[f"{d.name} (.tmp)"] = apagados.get(f"{d.name} (.tmp)", 0) + n
        except OSError:
            pass
        if chaves_stem:
            for sub in _STEM_KEYED:
                apagados[sub] = apagados.get(sub, 0) + _podar_dir(base / sub, chaves_stem, agora)
        if chaves_nome:
            apagados[_NOME_KEYED] = (apagados.get(_NOME_KEYED, 0)
                                     + _podar_dir(base / _NOME_KEYED, chaves_nome, agora, "*.jsonl"))
        if chaves_pane:
            for sub in _PANE_KEYED:
                apagados[sub] = (apagados.get(sub, 0)
                                 + _podar_dir(base / sub, chaves_pane, agora))
    return apagados


def prune_sidecars(infos=None, agora: float | None = None,
                   bases: list[Path] | None = None) -> dict[str, int]:
    """Poda de sidecars orfaos em todos os config dirs. Devolve {subdir: apagados}.

    `infos`: snapshot de SessionInfo (registry.list()) — sem ele, resolve ao vivo.
    `agora`/`bases`: injecao de teste. Com bases injetadas, NUNCA toca o disco do usuario.
    """
    if infos is None:
        # Import local: registry importa pqueue/adapters no boot; aqui o prune so precisa da
        # listagem, e o import local evita qualquer ciclo futuro (padrao do resto do app).
        from app.registry import SessionRegistry

        infos = SessionRegistry().list()
    if agora is None:
        agora = time.time()
    if bases is None:
        bases = _config_bases()

    from app.models import session_key
    from app.pqueue import _sanitize

    chaves_stem = {session_key(i.jsonl) for i in infos if i.jsonl}
    chaves_nome = {_sanitize(i.name) for i in infos}
    chaves_pane = _pane_ids_vivos()
    apagados = _podar(bases, chaves_stem, chaves_nome, chaves_pane, agora)
    for sub, n in apagados.items():
        if n:
            _log.info("prune: %d sidecar(s) de sessao morta removidos em %s", n, sub)
    return apagados


async def prune_loop() -> None:
    """Loop periodico do lifespan: varre na subida e depois a cada _INTERVALO.

    O primeiro disparo e imediato (backend que reinicia todo dia — o caso comum — nao espera
    24h); o intervalo cobre o backend de longa duracao. A varredura roda no thread (registry
    faz fork de tmux + /proc). Falha de UMA rodada nao mata o loop: loga e segue.
    """
    import asyncio

    while True:
        try:
            await asyncio.to_thread(prune_sidecars)
        except Exception:  # noqa: BLE001
            _log.exception("prune: varredura falhou; proxima em %ds", _INTERVALO)
        await asyncio.sleep(_INTERVALO)
