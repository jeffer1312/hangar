import asyncio
import contextvars
import json
import logging
import os
import re
import sys
import time
import traceback
import uuid
from pathlib import Path
from app import atomico, diag, guest_users, list_bridge, plugin_bridge, share_store
from app import registry as registry_mod
from app.adapters import CLAUDE_HEADLESS, chave_de, get_adapter
from app.adapters.preview_push import PushPreviewSource, fonte_ferramenta, fonte_pensamento
from app.difusor import Difusor
from app.pqueue import PromptQueue, _transcript_start_ts, committed_user_lines
from app.preview import PreviewBroker, _norm
from app.models import PreviewEvent, session_key
from app.live_rate import live_snapshot
from app.stats import Accumulator as StatsAccumulator
from app.registry import SessionRegistry
from app.share_guest_api import guest_safe
from app.askquestion import read_pending_askq


# O front deriva o uso de contexto do 2º par do segmento 💬 (`<in>/<out> <usado>/<janela>`) e, com
# só um par, mostra "medição indisponível" — corretamente, porque ler in/out como contexto daria
# 100% falso. O que faltava era saber POR QUE o par some: o statusline é produzido pelo script do
# usuário a partir do payload do Claude Code, que às vezes não traz context_window. Este log grava o
# statusline CRU nesses momentos, pra a causa sair de medição e não de chute.
_CTX_PAIR_RE = re.compile(r"([\d.,]+)\s*[kKmM]?\s*/\s*([\d.,]+)\s*[kKmM]?")
# Par ROTULADO "ctx 97k/1M": como a statusline do Pi e a do Kimi Code escrevem o contexto — o
# par do turno vem grudado em letras ("251kin/10kout") ou nem existe (o stdin do Kimi nao traz
# in/out), entao a regra dos >=2 pares descartaria o unico par existente. Com o rotulo na frente
# nao ha ambiguidade (mesma regra do parseStatusLine do front).
_CTX_LABELED_RE = re.compile(r"\bctx\s*[\d.,]+\s*[kKmM]?\s*/\s*[\d.,]+\s*[kKmM]?")


def context_pairs(status_line: str | None) -> int:
    """Quantos pares numéricos há no segmento 💬 do statusline (>=2 => há métrica de contexto).

    Um par rotulado "ctx x/y" CONTA como métrica (retorna 2): sem isto toda sessao Pi/Kimi
    caia no log de "sem métrica" com o contexto certo na tela — ruido que escondia o caso real."""
    if not status_line:
        return 0
    seg = re.search(r"💬([^│]*)", status_line)
    if not seg:
        return 0
    n = len(_CTX_PAIR_RE.findall(seg.group(1)))
    return max(n, 2) if _CTX_LABELED_RE.search(seg.group(1)) else n


def preview_is_committed(preview: str, committed: str) -> bool:
    """O texto do preview já é o bloco que caiu no .jsonl? (regra PURA, testável isolada do stream.)

    DOIS casos, não um:
      1. preview ⊆ commitado — no gap entre blocos o pane ainda mostra o bloco já gravado.
      2. commitado é PREFIXO do preview — o extract_assistant_text não cortou o chrome (verbo de
         status do Claude Code fora do _TOOL_VERBS, ex. "Making 1 scratchpad edit…") e ele grudou
         no fim da prosa já gravada.

    O caso 2 faltava: sem ele a bolha DUPLICAVA e ficava piscando (o preview repetia a mensagem
    anterior + a linha de status). Ele é o que sobrevive ao vocabulário do TUI mudar de novo — a
    lista de verbos é calibração best-effort, esta regra não depende dela.

    Piso de 16 chars: um trecho curto casa por acidente com qualquer coisa.
    """
    n = _norm(preview)
    if len(n) < 16 or not committed:
        return False
    return n in committed or n.startswith(committed)


# Linhas que o proprio TUI acrescenta a QUALQUER AskUserQuestion. Nunca vem no payload do hook, entao
# nao podem contar como "opcao a mais" na checagem de frescor abaixo.
# Linhas que o TUI acrescenta a TODA pergunta e que nunca estao no payload do hook. Guardadas SEM
# pontuacao final e comparadas assim (ver `_sem_ponto`): o mesmo item aparece "Type something." na
# escolha unica e "Type something" na multipla — medido 28/08/2026 —, e a versao sem ponto passava
# batido, virava "opcao no pane fora do sidecar" e derrubava o stepper nativo de TODA multipla
# escolha. Guardar as duas formas na lista resolveria hoje e quebraria no proximo ponto que sumir.
_TUI_EXTRAS = frozenset({"Type something", "Chat about this"})

# Caixinha que o TUI desenha antes do rotulo numa pergunta de MULTIPLA ESCOLHA: "[ ] Alfa",
# "[x] Alfa". Ela e desenho, nao rotulo — o payload do hook traz so "Alfa".
# Medido 28/08/2026: sem tirar isto, `first_opts <= pane_opts` NUNCA casava numa multiSelect (o
# pane trazia `["[ ] Alfa", "[ ] Bravo", …]` contra `{"Alfa", "Bravo", …}` do sidecar) e o stepper
# nativo simplesmente nao abria — toda multipla escolha caia no OptionButtons cru. As linhas
# extras do TUI vinham com caixinha tambem (`[ ] Type something.`), entao nem o _TUI_EXTRAS as
# removia. So a COMPARACAO tira a caixinha: a lista que vai pro OptionButtons continua com ela,
# porque ali a caixinha e o unico jeito de ver o que ja esta marcado.
_CAIXA_MULTI = re.compile(r"^\[.?\]\s*")


def _sem_caixa(rotulo: str) -> str:
    return _CAIXA_MULTI.sub("", rotulo)


def _normaliza_extra(rotulo: str) -> str:
    """Tira a caixinha e o ponto final, que e o que separa as linhas do TUI de uma opcao de verdade.
    So serve pra COMPARAR com `_TUI_EXTRAS`: o rotulo que vai pra tela nao passa por aqui."""
    return _sem_caixa(rotulo).rstrip(".")


def _ask_question_event(state_json: str, jsonl: str) -> dict | None:
    """Retorna o evento SSE ask_question que abre o stepper nativo do AskUserQuestion, ou None.
    Dispara em awaiting_input + sidecar do hook cujas opcoes batem com o menu corrente do pane —
    qualquer numero de perguntas, inclusive uma."""
    try:
        obj = json.loads(state_json)
    except (json.JSONDecodeError, ValueError):
        return None
    if obj.get("state") != "awaiting_input":
        return None
    payload = read_pending_askq(jsonl)
    if payload is None:
        return None
    # Pergunta UNICA tambem abre o stepper. Havia aqui um gate `len(questions) < 2 -> None`, com o
    # argumento de que o TUI submete direto no Enter (sem tela de Review) e o OptionButtons bastava.
    # O que ele custava: o OptionButtons le o picker do PANE, e o pane so tem ROTULO — a `description`
    # de cada opcao, que e onde mora a explicacao da escolha, sumia da tela. Como o padrao e perguntar
    # uma coisa por vez, isso valia pra quase toda pergunta. answer_questions ja trata pergunta unica
    # (terminal_input.py:463, com guard de malha fechada justamente porque ali o Enter ja submete).
    # `has_preview` sobrevive porque decide a ESTRATEGIA DE CASAMENTO logo abaixo, nao mais se emite.
    has_preview = any(o.preview for q in payload.questions for o in q.options)
    # NAO depende de `overlay`: is_overlay e fragil p/ AskUserQuestion — o rodape de navegacao sai das
    # ultimas 8 linhas do pane (linhas em branco no fim) -> overlay=False -> o stepper NUNCA abria e caia
    # no OptionButtons. Freshness pelo SIDECAR x menu atual: o sidecar nao e limpo se respondido pela TUI
    # (so no /answer + kill), entao confere que as opcoes da 1a pergunta batem com as do menu corrente
    # (classify) -> sidecar velho sobre OUTRO prompt (ex: permissao) nao abre o stepper.
    # Freshness: sem preview, igualdade exata (sidecar ⊆ pane) — protecao original contra sidecar
    # STALE abrir o stepper sobre OUTRO prompt (ex: menu de permissao). COM preview, a label do pane
    # vem truncada pelo wrap da coluna ("System no topo (igual aos" vs "...igual aos irmãos)") ->
    # relaxa pra prefixo NUMA DIRECAO SO (opcao do pane e prefixo da label completa; o inverso
    # deixaria label curta "Yes" casar com "Yes, and bypass permissions" = cross-wire de permissao)
    # + contagem igual de opcoes. Falhou -> degrada pro OptionButtons (= hoje), sem regressao.
    first_opts = {o.label for o in payload.questions[0].options}
    # As linhas que o TUI acrescenta saem da conta nos DOIS ramos, nao so no de baixo. No ramo COM
    # preview a comparacao e por CONTAGEM IGUAL, entao mante-las ali reprovava 100% das perguntas com
    # preview — exatamente o caminho que existe pra nao perder o preview. Bug anterior a este trecho:
    # o teste do ramo de preview usava um pane fabricado sem as extras e nunca o exercitou.
    pane_opts = {_sem_caixa(o) for o in (obj.get("options") or [])
                 if _normaliza_extra(o) not in _TUI_EXTRAS}
    if not first_opts or not pane_opts:
        return None
    if not has_preview:
        if not first_opts <= pane_opts:
            # Este return era o UNICO dos tres sem log, e era justamente por onde a multipla
            # escolha saia — a degradacao mais comum era tambem a mais calada. Mesmo motivo dos
            # logs irmaos abaixo: sem isto o unico sintoma e "a tela ficou mais pobre".
            _log.info("askq: rotulo do sidecar fora do menu, degrada p/ OptionButtons "
                      "sidecar=%s pane=%s", sorted(first_opts), sorted(pane_opts))
            return None
        # Subset sozinho nao basta. Um sidecar STALE cujos rotulos por acaso APARECEM num menu maior
        # (ex: {Sim, Nao} contra um menu [Cancelar, Sim, Nao]) passava — e como answer_questions
        # submete POR INDICE, com o menu real em outra ordem o Enter cai na linha errada. Entao
        # nenhuma opcao REAL do pane pode faltar no sidecar. _TUI_EXTRAS sai da conta porque o TUI a
        # acrescenta a toda pergunta e ela nunca esta no payload do hook.
        # Se o Claude Code renomear essas linhas, o casamento passa a reprovar e a sessao degrada pro
        # OptionButtons — o comportamento antigo, nunca resposta na linha errada.
        #
        # O log e o que torna essa degradacao VISIVEL, e o sinal esta na FREQUENCIA: uma linha
        # ocasional e sidecar velho, que e o esperado; a mesma linha em TODA pergunta significa que
        # as linhas do TUI mudaram de texto e ninguem mais ve descricao de opcao. Sem isto o unico
        # sintoma seria "a tela ficou mais pobre", sem ninguem saber por que.
        # `_log` e definido adiante no modulo; resolve em tempo de chamada.
        if extras := pane_opts - first_opts:
            _log.info("askq: opcao no pane fora do sidecar, degrada p/ OptionButtons extras=%s", extras)
            return None
    else:
        def _match(lbl: str) -> bool:
            return any(s and lbl.startswith(s) for s in pane_opts)
        # Mesmo motivo do log do ramo de cima, que aqui faltava: reprovar CALADO deixa como unico
        # sintoma "a tela ficou mais pobre" — a folha nativa com descricao e preview vira lista de
        # rotulo truncado, e nao ha por onde comecar a investigar. Diagnostico de 25/08/2026 numa
        # maquina Windows passou por isto: as opcoes tinham preview, entao este era o ramo, e o log
        # nao tinha uma linha sequer sobre a pergunta. Loga os dois lados porque a causa e a
        # DIFERENCA entre eles (rotulo do pane truncado num ponto que nao e prefixo, ou contagem
        # diferente de opcoes).
        if len(first_opts) != len(pane_opts) or not all(_match(l) for l in first_opts):
            _log.info("askq: sidecar com preview nao casa o menu, degrada p/ OptionButtons "
                      "sidecar=%s pane=%s", sorted(first_opts), sorted(pane_opts))
            return None
    return {"event": "ask_question", "data": json.dumps(payload.model_dump(), ensure_ascii=False)}

# Stateless (so projects_dir) — usado pelo watcher pra detectar troca de jsonl (ex: /clear abre um
# transcript novo, mas a conexao SSE foi bindada no antigo).
_registry = SessionRegistry()

# Instancia stateless pro stream de lista (separada do _registry do jsonl_watcher pra clareza).
_list_registry = SessionRegistry()

_log = logging.getLogger("hangar.sse")

# "Abrir o navegador embutido" vindo do AGENTE (POST /api/sessions/<nome>/nav, via CLI
# hangar-preview open). É um MARCADOR por sessão {url, ts}, não uma fila: cada conexão SSE (a do
# chat da sessão e a da lista) o entrega UMA vez e ele fica, até o desktop confirmar que criou o
# view (DELETE /nav) ou vencer o prazo. Antes era `pop` pelo primeiro stream que passasse — o
# celular lendo a mesma sessão comia o evento e o desktop nunca via. Gravado em disco porque
# reiniciar o backend perdia o pedido.
_NAV_TTL_S = 600.0
_NAV_MARCADORES: dict[str, dict] = {}
# Disco é lido UMA vez por processo (na primeira consulta). "Dict vazio" é o estado normal —
# marcador é evento raro — e reler a cada poll de cada SSE seria I/O síncrono no loop.
_nav_carregado = False
# Monitores de estado compartilhados entre as conexoes de um mesmo chat (ver _monitor_de).
_ESTADOS = Difusor()


def _nav_arquivo() -> Path:
    # Sublinhado como o `_srv.json`: a pasta é a dos sidecars de navegador (um por sessão, com
    # `chave`), e quem a varre (CLI, `/navegador`) pula os arquivos de processo.
    return Path.home() / ".hangar" / "nav" / "_pendentes.json"


def _nav_carregar() -> None:
    global _nav_carregado
    if _nav_carregado:
        return
    _nav_carregado = True
    try:
        d = json.loads(_nav_arquivo().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return
    if isinstance(d, dict):
        _NAV_MARCADORES.update({k: v for k, v in d.items()
                                if isinstance(v, dict) and isinstance(v.get("url"), str) and isinstance(v.get("ts"), (int, float))})


def _nav_gravar() -> None:
    try:
        arq = _nav_arquivo()
        arq.parent.mkdir(parents=True, exist_ok=True)
        tmp = arq.with_suffix(f".{os.getpid()}.tmp")
        tmp.write_text(json.dumps(_NAV_MARCADORES), encoding="utf-8")
        os.chmod(tmp, 0o600)   # a url pode levar o token do dono
        atomico.substituir(tmp, arq)
    except OSError as e:
        _log.warning("nav: marcador nao gravado em disco: %s", e)


def nav_pendente(name: str, url: str) -> None:
    # Só a ÚLTIMA url por sessão: é o que a UI abre.
    _nav_carregar()
    _NAV_MARCADORES[name] = {"url": url, "ts": time.time()}
    _nav_gravar()


def nav_confirmar(name: str) -> None:
    """O desktop criou o view: o marcador cumpriu o papel."""
    _nav_carregar()
    if _NAV_MARCADORES.pop(name, None) is not None:
        _nav_gravar()


def nav_vivos() -> dict[str, dict]:
    """Marcadores dentro do prazo; os vencidos saem no caminho."""
    _nav_carregar()
    agora = time.time()
    vencidos = [n for n, m in _NAV_MARCADORES.items() if agora - float(m.get("ts", 0)) > _NAV_TTL_S]
    for n in vencidos:
        _NAV_MARCADORES.pop(n, None)
    if vencidos:
        _nav_gravar()
    return _NAV_MARCADORES


def nav_novos(vistos: dict[str, float], name: str | None = None) -> list[tuple[str, dict]]:
    """O que esta conexão ainda não entregou: marcador cujo `ts` difere do que ela já mandou.
    `name` restringe à sessão de um chat; None é a lista, que vê todas."""
    saida = []
    for n, m in nav_vivos().items():
        if name is not None and n != name:
            continue
        ts = m.get("ts", 0)
        if vistos.get(n) == ts:
            continue
        vistos[n] = ts
        saida.append((n, m))
    return saida


async def _cached_list():
    # Um snapshot só no processo: o do api (`_guardar_snap`, single-flight e invalidado na criação
    # e no rename). Dois caches eram duas varreduras de /proc + tmux por segundo pro mesmo dado.
    from app import api  # api importa este módulo
    snap = api._list_snap["snap"]
    if snap is not None and time.monotonic() - snap[0] < api._LIST_TTL:
        return snap[1]
    return await asyncio.to_thread(api._guardar_snap)


# Reducao ESTAVEL da statusline pro dedup da lista: modelo, contexto em baldes de 5%, ⚡5h% e 📅7d%.
# Relogio (⏱) e custo ficam DE FORA — mudam a cada captura e re-emitiriam a lista inteira a toa.
# Espelha o parse do front (frontend/src/lib/statusline.ts), so o subset que o sig precisa.
_ST_MODEL = re.compile(r"🤖\s*([^(│]+)")
# O esforco mora no parentese DEPOIS do modelo (`(high✦)`); sem ele no sig, trocar so o esforco
# nao re-emitia a lista e o painel ficava com o valor velho.
_ST_EFFORT = re.compile(r"🤖[^(│]*\(([^)│]*)\)")
_ST_5H = re.compile(r"⚡[^│]*?(\d+)\s*%")
_ST_7D = re.compile(r"📅[^│]*?(\d+)\s*%")
_ST_PAIR = re.compile(r"([\d.,]+)\s*([kKmM])?\s*/\s*([\d.,]+)\s*([kKmM])?")
_ST_LABELED = re.compile(r"\bctx\s*([\d.,]+)\s*([kKmM])?\s*/\s*([\d.,]+)\s*([kKmM])?")


def _status_sig(s):
    if not s:
        return None
    ctx = None
    seg = re.search(r"💬([^│]*)", s)
    if seg:
        def _num(x, unit):
            mult = {"k": 1e3, "m": 1e6}.get((unit or "").lower(), 1.0)
            try:
                return float(x.replace(",", "")) * mult
            except ValueError:
                return 0.0
        # O par ROTULADO "ctx x/y" (Pi, Kimi Code) vence: sem ele a regra dos >=2 pares
        # descartava o unico par dessas linhas e o sig nunca via o contexto mudar.
        rotulado = _ST_LABELED.search(seg.group(1))
        pairs = _ST_PAIR.findall(seg.group(1))
        # >=2 pares (Claude): o 1o e in/out do turno; o ULTIMO e uso/janela (regra do front).
        alvo = rotulado.groups() if rotulado else (pairs[-1] if len(pairs) >= 2 else None)
        if alvo:
            u, uu, t, tu = alvo
            total = _num(t, tu)
            if total > 0:
                ctx = round(_num(u, uu) / total * 20)  # baldes de 5% (round: 4.9999… nao vira 4)
    return (
        m.group(1).strip() if (m := _ST_MODEL.search(s)) else None,
        ctx,
        m.group(1) if (m := _ST_5H.search(s)) else None,
        m.group(1) if (m := _ST_7D.search(s)) else None,
        m.group(1).strip() if (m := _ST_EFFORT.search(s)) else None,
    )


def _context_sig(ctx) -> int | None:
    """Contexto em baldes de 5%, como o da statusline: cada resposta muda o uso em alguns tokens."""
    if not isinstance(ctx, dict) or not ctx.get("window"):
        return None
    return int(ctx.get("used", 0) * 20 // ctx["window"])


def _list_sig(infos) -> str:
    # Dedup IGNORA last_activity: e o mtime do jsonl (float sub-segundo) que muda a CADA escrita de uma
    # sessao ativa -> sem isto a lista inteira re-emitia a cada poll sem nada visivel mudar = flicker.
    # Re-emite so em mudanca de membership/state/cwd/tracked/jsonl/question/stalled/limited/
    # limit_reset/then_target/status_line-reduzida/presenca-de-label/loop/engine. Sem o engine aqui,
    # resumir um pane cujo motor sumiu do engines.json (kimi -> None) nao reemite a lista e o chip
    # ⚙ kimi fica preso, calado. A `conta` entra pela MESMA razao: numa sessao Pi ela e a
    # credencial do modelo ESCOLHIDO (trocar de kimi-coding pro Codex muda a conta sem mexer em
    # mais nada), e sem isto a pilula de cota fica desenhando a cota da conta anterior.
    # Sem o plan_name aqui, trocar do plano A pro B com o mesmo 9/17 nao re-emite e o chip fica
    # preso no plano errado — mesmo bug do engine. plan_hidden pela MESMA razao: escolher "nenhum"
    # zera todos os outros campos de plano, mas a lista precisa re-emitir pro painel continuar
    # montado (com o seletor que desfaz a escolha) em vez de sumir.
    # plan_tasks/plan_task/plan_task_total/plan_complete tambem entram: um step desmarcado na
    # Task 1 e outro marcado na Task 2 no mesmo write pode deixar done/total liquidos (e plan_name)
    # identicos e ainda assim mudar a distribuicao por Task — sem isto a barra segmentada e a
    # Task atual ficam com o snapshot velho ate outra coisa qualquer mudar a sig.
    return json.dumps(
        [(i.name, i.cwd, getattr(i, "branch", None), getattr(i, "git_cwd", None),
          getattr(i, "worktree_gone", False), getattr(i, "git_dirty", None),
          getattr(i, "git_ahead", None), getattr(i, "git_behind", None),
          getattr(i, "git_added", None), getattr(i, "git_removed", None),
          i.state, i.tracked, getattr(i, "headless", False), i.jsonl, i.question, i.stalled, i.limited,
          getattr(i, "lifecycle_id", None), getattr(i, "transfer_id", None),
          getattr(i, "transfer_phase", None),
          getattr(i, "last_reply", None), getattr(i, "last_reply_at", None),
          getattr(i, "pending_questions", 0),
          i.limit_reset, i.then_target, _status_sig(getattr(i, "status_line", None)),
          _context_sig(getattr(i, "context", None)), getattr(i, "model", None),
          (getattr(i, "label", None) if getattr(i, "provider", None) == "codex" and not i.tracked
           else bool(getattr(i, "label", None))),
          getattr(i, "startup_steps", []),
          getattr(i, "loop_status", None), getattr(i, "loop_iter", None),
          getattr(i, "engine", None), getattr(i, "conta", None), getattr(i, "codex_service_tier", None),
          getattr(i, "plan_name", None), getattr(i, "plan_done", None),
          getattr(i, "plan_total", None),
          getattr(i, "plan_task", None), getattr(i, "plan_task_total", None),
          getattr(i, "plan_complete", None),
          tuple(map(tuple, getattr(i, "plan_tasks", None) or [])),
          getattr(i, "plan_hidden", None),
          # `problema` entra pelo mesmo motivo do engine: ele aparece e some sem mexer em mais
          # nada (o hook e aprovado na TUI e a sessao passa a ter marcador), e sem isto o aviso na
          # tela ficaria preso ate outra coisa qualquer mudar a assinatura.
          getattr(i, "problema", None),
          # Uma sessao Pi/omp/kimi nasce classificada como `claude` e so vira o provider dela
          # quando a extensao publica o bilhete do pane. Sem o provider aqui, essa virada so
          # re-emite a lista se o `jsonl` mudar junto — e a lista fica com o glifo errado (e sem
          # os chips de provider, que so aparecem quando ela mistura harnesses) ate outra coisa
          # qualquer mudar a assinatura.
          getattr(i, "provider", None),
          # Ligar/desligar o compartilhamento não mexe em mais nada da sessão: sem isto o selo 🔗
          # não aparece nem some até outra coisa mudar a assinatura.
          getattr(i, "shared", False),
          # O dono da sessão (convidado) é gravado depois da criação, e some ao apagar o convidado:
          # sem isto o rótulo e a visibilidade ficam velhos até outra coisa mudar a assinatura.
          getattr(i, "owner", None),
          # Sucessão do árbitro muda só este campo na linha do orquestrador.
          getattr(i, "orq_arbiter", None),
          # Entrar, sair ou renomear grupo muda só estes campos: sem eles o selo do par fica velho
          # no companheiro que ficou até outra coisa mudar a assinatura.
          getattr(i, "pair_peers", None), getattr(i, "pair_gid", None),
          getattr(i, "pair_task", None), getattr(i, "pair_external", None))
         for i in infos],
        ensure_ascii=False,
    )


def _shortcuts_snapshot() -> str | None:
    from app import shortcut_terminals
    return json.dumps(shortcut_terminals.list_all(), ensure_ascii=False)


class _ListRefresher:
    """UM refresher em background (single-flight, compartilhado por TODAS as conexoes da lista) que
    produz o snapshot de list_with_state no ritmo dele. Desenho (decisao do jefferson): a conexao SSE
    e PRIORIDADE ABSOLUTA e NUNCA espera trabalho — ela so LE o ultimo snapshot pronto e emite quando
    a versao muda; o custo de raspar/decorar mora AQUI, uma vez, nao M×conexoes. Decoracao que falha
    (git 2s pendurado etc.) e LOGADA (warning — isolamento sim, silencio nao) mantendo o snapshot
    anterior (stale > morto): refresher travado = clientes seguem pingando e vendo a lista velha, nunca
    desconectam. O ref-count para o refresher quando a ultima conexao sai (nao raspa tmux com zero clientes)."""

    def __init__(self, poll: float = 1.5):
        self.poll = poll
        self.data: str | None = None
        self.shortcuts_data: str | None = None
        self._sc_task: asyncio.Task | None = None
        self._sc_started = 0.0
        self._sc_failing = False
        self.sig: str | None = None
        self.version = 0
        self.errored = False
        self.latest: tuple[float, list] | None = None
        self._task: asyncio.Task | None = None
        self._refs = 0
        self._loop = None
        self._cond: asyncio.Condition | None = None

    def _ensure(self):
        # (Re)inicia o refresher se nao ha task viva NESTE event loop. O bind por-loop e o que deixa o
        # singleton sobreviver aos asyncio.run() dos testes (cada um e um loop novo).
        loop = asyncio.get_running_loop()
        if self._task is None or self._task.done() or self._loop is not loop:
            self._loop = loop
            self._cond = asyncio.Condition()
            self.data = None
            self.shortcuts_data = None
            self._sc_task = None
            self._sc_failing = False
            self.sig = None
            self.version = 0
            self.errored = False
            self.latest = None
            self._refs = 0
            # O produtor atende todas as conexões, não pertence ao primeiro assinante.
            context = contextvars.copy_context()
            context.run(diag.req_atual.set, "")
            self._task = asyncio.create_task(self._run(), context=context)

    def _launch_shortcuts(self) -> None:
        # A lista nunca espera terminal de atalho: a leitura roda ao lado da lista, no máximo uma em
        # voo (thread não cancela; travada, só segura o valor anterior em vez de acumular threads).
        if self._sc_task is None:
            self._sc_task = asyncio.create_task(asyncio.to_thread(_shortcuts_snapshot))
            self._sc_started = time.monotonic()
        elif not self._sc_task.done() and time.monotonic() - self._sc_started > 5 and not self._sc_failing:
            self._sc_failing = True
            _log.warning("terminais de atalho: leitura travada; mantem a anterior")

    def _harvest_shortcuts(self) -> str | None:
        task = self._sc_task
        if task is None or not task.done():
            return self.shortcuts_data
        self._sc_task = None
        if task.cancelled():
            return self.shortcuts_data
        try:
            value = task.result()
        except Exception:
            # Uma vez por queda, não a cada ciclo.
            if not self._sc_failing:
                _log.warning("terminais de atalho: leitura falhou; mantem a anterior", exc_info=True)
            self._sc_failing = True
            return self.shortcuts_data
        self._sc_failing = False
        return value

    async def _produce(self) -> tuple[list, bool]:
        """A lista decorada e se foi o Python que a produziu. Com o Rust dono o retrato é dele
        (`list.snapshot`) e a descoberta Python não roda; o modo é relido a cada tique, porque a
        desistência do Rust passa tudo ao Python."""
        if await registry_mod.rust_owns_list_async():
            return await asyncio.to_thread(list_bridge.snapshot), False
        registry_mod.PYTHON_DISCOVERY["refresher"] += 1
        snap = [i.model_copy() for i in await _cached_list()]
        return await _list_registry.list_with_state(snap), True

    async def _run(self):
        while True:
            self._launch_shortcuts()
            try:
                started = time.monotonic()
                infos, ours = await self._produce()
                sig = _list_sig(infos)
                # Serializar a lista inteira só quando vai ser publicada (a sig decide, como antes).
                data = (json.dumps([i.model_dump(mode="json") for i in infos], ensure_ascii=False)
                        if sig != self.sig or self.errored else None)
            except Exception:
                # Decoracao/raspagem falhou -> MANTEM o snapshot anterior (stale > morto), nunca derruba
                # a conexao. Loga (padrao da casa) E sinaliza 'list_error' UMA vez (na transicao) pras
                # conexoes — lista vazia por falha e indistinguivel de zero sessoes; o front distingue
                # "erro" de "offline". Ciclo bom seguinte re-emite 'sessions' e limpa o erro no front.
                _log.warning("refresher da lista falhou; mantem snapshot anterior", exc_info=True)
                if not self.errored:
                    # No diário só na TRANSIÇÃO, igual ao evento: um refresher quebrado a cada poll
                    # encheria o arquivo do dia com a mesma linha.
                    # SÓ o tipo da exceção no diário, não a mensagem dela. Este caminho embrulha
                    # `list_with_state`, que carrega o texto da pergunta pendente do
                    # AskUserQuestion — um erro de serialização ali ecoaria conversa dentro de um
                    # arquivo que promete não guardar nenhuma. A mensagem inteira vai no log local
                    # (o `_log.warning` acima, com traceback), que não é o arquivo que se envia.
                    # Tipo + `arquivo:linha` da moldura mais interna: zero conteúdo, e responde
                    # ONDE — só o tipo deixava 290 `RuntimeError` num diário sem pista nenhuma.
                    _tipo, _exc, _tb = sys.exc_info()
                    quadro = traceback.extract_tb(_tb)[-1] if _tb else None
                    onde = f" @ {Path(quadro.filename).name}:{quadro.lineno}" if quadro else ""
                    diag.registrar("lista.refresher_falhou", "erro",
                                   detalhe=f"{type(_exc).__name__}{onde}",
                                   codigo=getattr(_exc, "code", None))
                    async with self._cond:
                        self.errored = True
                        self.version += 1
                        self._cond.notify_all()
                await asyncio.sleep(self.poll)
                continue
            # A cada tique, mesmo sem mudança na sig: o /api/sessions serve daqui campos que a sig
            # ignora (last_activity, statusline inteira). Lista já decorada não é mais escrita.
            # Idade conta do início do tique: lista iniciada antes de uma invalidação não vale.
            # Só a do Python: a sombra compara o Rust com ela, e o `/api/sessions` pede o retrato
            # do Rust direto a ele.
            self.latest = (started, infos) if ours else None
            shortcuts = self._harvest_shortcuts()
            # sucesso: emite se a sig mudou OU se estava em erro (pra o front LIMPAR o list_error).
            # `data`/`sig` só andam quando a assinatura da lista muda: gravar `data` numa mudança que
            # é só dos terminais reemitiria `sessions` por um `last_activity` que a assinatura ignora.
            if data is not None or shortcuts != self.shortcuts_data:
                if self.errored:
                    diag.registrar("lista.recuperada", quantidade=len(infos))
                async with self._cond:
                    if data is not None:
                        self.errored = False
                        self.sig = sig
                        self.data = data
                    self.shortcuts_data = shortcuts
                    self.version += 1
                    self._cond.notify_all()
            await asyncio.sleep(self.poll)

    def acquire(self) -> asyncio.Condition:
        self._ensure()
        self._refs += 1
        return self._cond

    def release(self):
        self._refs -= 1
        if self._refs <= 0 and self._task is not None:
            self._task.cancel()
            self._task = None
            if self._sc_task is not None:
                self._sc_task.cancel()
                self._sc_task = None
            self._refs = 0


_list_refresher = _ListRefresher()


def recent_list(max_age: float) -> list | None:
    """Última lista decorada do refresher, se viva e com no máximo `max_age` s. Só leitura."""
    latest = getattr(_list_refresher, "latest", None)
    if latest is None or time.monotonic() - latest[0] > max_age or latest[0] < _list_invalidated_at:
        return None
    return latest[1]


# Criação/rename: a lista do refresher ainda não tem a sessão nova; /api/sessions recalcula.
_list_invalidated_at = 0.0


def invalidate_recent_list() -> None:
    global _list_invalidated_at
    _list_invalidated_at = time.monotonic()


async def list_events(ping_secs: float = 8.0, only=None, viewer=None, token=None):
    """SSE da LISTA de sessoes. Conexao = PRIORIDADE ABSOLUTA, zero trabalho: um reader que so LE o
    snapshot compartilhado (produzido pelo _ListRefresher unico) e emite quando a versao muda, + um
    ping em timer FIXO por conexao (incondicional). Refresher travado nao afeta a conexao — o ping
    segue e o front ve a lista velha (stale > desconectado).

    `only` = conexao de convidado: ve so a sessao compartilhada, nao recebe os pedidos de navegador
    do dono (`nav`) e nao conta como app do dono aberto. Aceita o nome ou o `Guest` do token:
    renomear a sessao muda o registro, e o stream aberto tem que acompanhar (`token` o relê).
    `viewer` = convidado com login proprio (ou None = dono): a lista passa pelo mesmo filtro da
    rota `/api/sessions`."""
    queue: asyncio.Queue = asyncio.Queue()
    cond = _list_refresher.acquire()
    started = time.monotonic()
    diag.registrar("sse.lista_abriu")
    if only is None and viewer is None:
        plugin_bridge.app_entrou()

    async def reader():
        last_version, last_data, last_shortcuts, was_error = -1, None, None, False
        while True:
            async with cond:
                await cond.wait_for(lambda: _list_refresher.version != last_version)
                last_version = _list_refresher.version
                errored = _list_refresher.errored
                data = _list_refresher.data
                shortcuts = _list_refresher.shortcuts_data
            if errored:
                await queue.put(("list_error", "{}"))   # falha do refresher — front distingue de offline
                was_error = True
                continue
            if data is not None:
                try:
                    if only is not None:
                        # `only` em str é só o nome (usos internos). Com `token`, o registro é
                        # relido a cada envio: sessão ligada ao token depois da abertura aparece.
                        guest = None if isinstance(only, str) else (
                            (token and share_store.lookup_token(token)) or only)
                        sees = (lambda n: n == only) if guest is None else guest.sees
                        data = json.dumps([guest_safe(x, guest) for x in json.loads(data)
                                           if sees(x.get("name"))], ensure_ascii=False)
                    if guest_users.has_claims() or viewer is not None:
                        itens = await asyncio.to_thread(guest_users.filter_visible, viewer,
                                                        json.loads(data), lambda x: x.get("name"))
                        data = json.dumps(itens, ensure_ascii=False)
                except Exception:
                    # Sem isto o reader morre calado e o cliente fica com a lista congelada e só ping.
                    _log.exception("sse: recorte da lista falhou")
                    await queue.put(("list_error", "{}"))
                    was_error = True
                    continue
                # Compara o que sairia: versão só dos terminais de atalho não reenvia `sessions`,
                # mas renomear a sessão do convidado muda o recorte sem mudar o dado da lista.
                if data != last_data or was_error:
                    last_data, was_error = data, False
                    await queue.put(("sessions", data))
            # Terminal de atalho não entra no stream do convidado.
            if only is None and viewer is None and shortcuts is not None and shortcuts != last_shortcuts:
                last_shortcuts = shortcuts
                await queue.put(("shortcut_terminals", shortcuts))

    async def ping_loop():
        while True:
            await asyncio.sleep(ping_secs)
            await queue.put(("ping", "{}"))

    async def nav_pump():
        # A lista e o unico stream que o desktop mantem aberto o tempo todo: e por aqui que "abrir
        # navegador" chega com a sessao FORA da tela, e o shell cria o view escondido.
        vistos: dict[str, float] = {}
        try:
            while True:
                await asyncio.sleep(1.0)
                for nome, marc in nav_novos(vistos):
                    queue.put_nowait(("nav", json.dumps({"name": nome, "url": marc["url"]})))
        except asyncio.CancelledError:
            raise
        except Exception:
            _log.exception("sse: nav_pump da lista morreu")

    tasks = [asyncio.create_task(reader()), asyncio.create_task(ping_loop())]
    if only is None and viewer is None:
        tasks.append(asyncio.create_task(nav_pump()))
    try:
        while True:
            event, data = await queue.get()
            yield {"event": event, "data": data}
    finally:
        for t in tasks:
            t.cancel()
        _list_refresher.release()
        if only is None and viewer is None:
            plugin_bridge.app_saiu()
        diag.registrar("sse.lista_fechou", ms=int((time.monotonic() - started) * 1000))


def _confirm_codex_queue(name: str, jsonl: str) -> None:
    from app import runtime_coordinator
    from app.runtime_adapter import run_sync
    coordinator = runtime_coordinator.current()
    if coordinator is not None and coordinator.managed_runtime(name):
        try:
            run_sync(lambda: coordinator.op(name, {"kind":"confirm"}, uuid.uuid4().hex), coordinator.loop)
        except runtime_coordinator.TransferInProgress:
            pass        # posse passando ao Rust: a confirmação fica para a próxima rodada, o stream segue
        return
    queue = PromptQueue(name)
    if not any(r.get("delivered") and not r.get("confirmed") for r in queue.load()):
        return
    from app.conversation_history import confirmation_options
    confirmation = confirmation_options(name, jsonl, "codex")
    committed = committed_user_lines(jsonl, "codex", **confirmation)
    start = _transcript_start_ts(jsonl)
    if committed is not None and start is not None:
        # RPC aceito não prova escrita no rollout; ausência nunca autoriza reenvio.
        queue.reconcile_delivered(committed, start, time.time(), grace=0, confirm_only=True)


# Com o Rust de pé, estes saem do hub: os quatro primeiros do `Monitor` (Claude com terminal), os
# seis do feed do Claude e do Codex sem terminal, e a faixa dos mods, que o hub junta das duas fontes.
_RUST_STATE_EVENTS = ("state", "preview", "ask_question", "suggest", "pensamento", "ferramenta", "plugin_ui")
# O hub pinga o canal a cada 10 s: três calados = conexão morta.
_RUST_CHANNEL_IDLE_S = 30.0
_RUST_CHANNEL_LINE = 1 << 20


class RustStateChannelError(Exception):
    """O canal privado do estado no Rust não abriu ou caiu: o stream fecha e o cliente reconecta."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def _estado_do_rust(provider: str, name: str) -> bool:
    """Com o Rust esperado ou de pé (`pending`/`rust`), o hub é dono do estado ao vivo em qualquer
    porta e nada disso sobe aqui: Claude com terminal (o `Monitor`) e Claude e Codex sem terminal
    que o Rust atende (o feed do runtime)."""
    if provider not in ("claude", CLAUDE_HEADLESS, "codex"):
        return False
    from app import runtime_coordinator
    owner = runtime_coordinator.current()
    if owner is None or owner.mode not in ("pending", "rust"):
        return False
    if provider == "claude":
        return True
    if provider == CLAUDE_HEADLESS:
        return owner.rust_owns("claude", True)
    return _codex_headless(name) and owner.rust_owns("codex", True)


async def _estado_do_rust_async(provider: str, name: str) -> bool:
    """O Codex lê o arquivo da sessão: fora do laço de eventos. Os outros não leem disco."""
    if provider == "codex":
        return await asyncio.to_thread(_estado_do_rust, provider, name)
    return _estado_do_rust(provider, name)


def _codex_headless(name: str) -> bool:
    from app.adapters.codex import sessions as codex_sessions
    return bool((codex_sessions.load(name) or {}).get("headless"))


async def _canal_do_estado(name: str):
    """`(evento, dado)` do canal privado do hub (`/__hangar_server/state/{name}/events`), para quem
    entrou pelas portas do Python. HTTP/1.0: o corpo vem cru até o fim, sem pedaços."""
    from urllib.parse import quote
    endpoint = list_bridge.endpoint()
    if endpoint is None:
        raise RustStateChannelError("state_channel_off")
    address, secret = endpoint
    host, port = address.rsplit(":", 1)
    try:
        reader, writer = await asyncio.wait_for(
            asyncio.open_connection(host.strip("[]"), int(port), limit=_RUST_CHANNEL_LINE), 5)
    except (OSError, TimeoutError) as e:
        raise RustStateChannelError("state_channel_unavailable") from e
    try:
        writer.write((f"GET /__hangar_server/state/{quote(name, safe='')}/events HTTP/1.0\r\n"
                      f"host: {address}\r\nx-hangar-internal: {secret}\r\n\r\n").encode())
        await writer.drain()
        status = (await asyncio.wait_for(reader.readline(), 10)).split()
        if len(status) < 2 or status[1] != b"200":
            raise RustStateChannelError(
                f"state_channel_status:{status[1].decode(errors='replace')[:3] if len(status) > 1 else 'none'}")
        while (await asyncio.wait_for(reader.readline(), 10)) not in (b"\r\n", b"\n", b""):
            pass
        event, data = None, []
        while True:
            line = await asyncio.wait_for(reader.readline(), _RUST_CHANNEL_IDLE_S)
            if not line:
                raise RustStateChannelError("state_channel_closed")
            line = line.decode("utf-8").rstrip("\r\n")
            if line.startswith("event:"):
                event = line[6:].strip()
            elif line.startswith("data:"):
                data.append(line[5:].removeprefix(" "))
            elif not line:
                if event in _RUST_STATE_EVENTS:
                    yield event, "\n".join(data)
                event, data = None, []
    except TimeoutError as e:
        raise RustStateChannelError("state_channel_timeout") from e
    except (OSError, ValueError, UnicodeDecodeError) as e:
        raise RustStateChannelError("state_channel_read") from e
    finally:
        writer.close()


def _info_event(name: str, provider: str, jsonl: str | None) -> dict:
    """`info` da conexão interna: o mesmo JSON da rota /internal/sessions/{name}/info."""
    from app.internal_api import info_payload   # internal_api importa este módulo
    return {"event": "info",
            "data": json.dumps(info_payload(name, provider, jsonl), ensure_ascii=False)}


async def merged_events(name: str, jsonl: str, provider: str = "claude",
                        start_offset: int | None = None, count_app: bool = True,
                        side: bool = False):
    # side=True: conexão interna do hangar-server (internal_api.side_events). Abre com `info` e o
    # repete no lugar do `reset`. A conversa o hangar-server lê do arquivo; aqui o transcript só é
    # seguido para a supressão da prévia e a baixa da fila do Codex.
    # count_app=False: conexao de convidado. Contar como app do dono aberto calaria as push dele.
    # provider: default "claude" preserva o comportamento de hoje pros callers que ainda nao passam
    # (api.py so passa quando uma tarefa futura ligar o seletor de provider no endpoint).
    # Sessão Claude sem terminal: provider continua "claude" pro front, mas o adapter (monitor,
    # prévia, entrada) é outro — a chave interna resolve pelo sidecar.
    provider = chave_de(name, provider)
    current_provider = provider    # atualizado no __reprovider__ (ver jsonl_watcher)
    current_jsonl = jsonl          # atualizado no __reset__ (ex: /clear abre novo transcript)
    # Ancora de hook do estado: o monitor le o marcador do sid VIVO (a closure acompanha o rebind
    # do /clear, que troca o current_jsonl -> sid novo).
    # transcript_get so vai pro adapter que o aceita (Kimi e orq): fecha sobre `current_jsonl` pelo
    # mesmo motivo do sid_get — o /clear troca o transcript, e um caminho congelado leria o mtime do
    # arquivo da sessao anterior.
    def _monitor_de(prov):
        adap = get_adapter(prov)
        kw = {"transcript_get": lambda: current_jsonl} if prov in ("kimi", "orq") else {}
        # Um monitor por (sessao, provider, transcript), compartilhado entre as conexoes abertas
        # nesse chat (desktop + celular = um capture-pane, nao dois). O transcript entra na chave
        # porque o monitor fecha sobre o sid VIVO da conexao que o criou: apos um /clear, quem
        # continua nele e recriado com a chave nova (ver __reset__) em vez de herdar a closure de
        # uma conexao que pode ja ter ido embora.
        return _ESTADOS.ouvir((name, prov, current_jsonl), lambda: adap.state_monitor(
            name, sid_get=lambda: session_key(current_jsonl) if current_jsonl else None, **kw))

    # Claude com terminal e o Rust de pé: o Python não observa o pane nem lê a prévia, em nenhuma
    # porta. A conexão interna é o próprio hub; quem entrou pelo Python lê o canal privado dele.
    rust_state = await _estado_do_rust_async(provider, name)
    pqueue = PromptQueue(name)
    # Fonte do preview ao vivo ramifica por provider: Claude nao tem push (o app-server manda os
    # deltas, o TUI do Claude nao) -> continua no PreviewBroker (poll do pane). Codex nao tem pane
    # -> PushPreviewSource, alimentado por push do CodexAdapter.state_monitor. Mesma interface
    # publica (get/subscribe) -> o resto do pump (preview_pump/_enqueue_preview/_already_committed)
    # fica IGUAL pras duas fontes. Pi tambem e pane -> mesmo PreviewBroker, mas o provider VAI
    # JUNTO: o chrome que fecha o bloco em voo e outro (caixa do composer), ver preview.py.
    # stem_get: chave do sidecar de previa (o agente publica o texto em voo por conta propria — hoje
    # so a extensao do Pi). Fecha sobre `current_jsonl` pelo mesmo motivo do monitor: o /clear troca
    # o transcript, e um stem congelado leria o marcador da sessao anterior.
    # orq: sem pane nem texto em voo; a fonte de push fica vazia e nada raspa o tmux.
    def _broker_de(prov):
        return (PushPreviewSource.get(name) if prov in ("codex", CLAUDE_HEADLESS, "orq")
                else PreviewBroker.get(name, prov,
                                       lambda: session_key(current_jsonl) if current_jsonl else None))

    broker = None if rust_state else _broker_de(provider)
    # Inicio da sessao atual: poda entradas de fila pre-/clear no live SSE (mesma regra do history).
    # `or 0.0`: aqui start_ts so PODA bolha de sessao anterior. Transcript ilegivel -> nao corta
    # nada, que e o fallback seguro deste lado (o perigoso e no reconcile, ver _transcript_start_ts).
    start_ts = _transcript_start_ts(jsonl) or 0.0
    queue: asyncio.Queue = asyncio.Queue()
    # Slot coalescido do preview: NUNCA entra na FIFO compartilhada (firehose atrasaria o assistant_msg
    # autoritativo — head-of-line). Mantemos so o ULTIMO texto + um unico marcador pendente na fila;
    # frames intermediarios sao descartados (full-replace, o ultimo vence). Sem await entre as
    # escritas do dict -> consistente no loop asyncio single-thread, sem lock.
    preview_slot = {"text": "", "pending": False, "md": False, "full": False}
    # Texto da ULTIMA msg de assistente que já caiu no .jsonl (normalizado). Fonte de verdade pra
    # suprimir preview JÁ COMMITADO: no gap entre blocos (durante tool-calls) o pane ainda mostra o
    # bloco que já foi gravado -> sem isto, vira bolha duplicada. Atualizado pelo tail_pump.
    committed = {"text": ""}

    def _already_committed(text: str) -> bool:
        return preview_is_committed(text, committed["text"])

    async def pump(kind, agen):
        try:
            async for item in agen:
                # model_dump_json (not model_dump): the SSE `data:` field must be a
                # JSON string for the browser's JSON.parse(e.data). A raw dict gets
                # str()'d by sse-starlette into Python repr (None/single quotes) = invalid JSON.
                # Clientes antigos ignoram o evento novo, sem desenhar a baixa como mensagem.
                event = "queue_confirmed" if kind == "message" and item.queued_confirmed else kind
                await queue.put((event, item.model_dump_json()))
        except Exception as exc:  # surface, never swallow
            diag.registrar("sse.pump_falhou", "erro", sessao=name, provider=current_provider,
                           etapa=kind, erro_tipo=type(exc).__name__)
            await queue.put(("__error__", exc))

    async def ping_loop():
        # Heartbeat VISIVEL pro cliente (a cada 10s). Diferente do ping interno do sse_starlette,
        # que vai como COMENTARIO (': ping') e o EventSource ignora -> o front nao consegue observar.
        # Este vai como evento real 'ping' pra alimentar o watchdog de liveness do front: numa
        # conexao half-open (mobile troca de rede / app no background), sem isto o front congela no
        # ultimo estado pq nada chega e o onerror nao dispara. O ping faz o front detectar e reconectar.
        # O primeiro sai na hora: o front dá 10s pro primeiro quadro, e esperar o estado ou o
        # transcript deixaria uma sessão lenta de ler parecendo conexão presa.
        while True:
            await queue.put(("ping", "{}"))
            await asyncio.sleep(10)

    async def nav_pump():
        # Entrega o marcador "abrir navegador" DESTA sessao uma vez por conexao (ver nav_novos).
        # Poll de 1s basta: e evento raro e humano, nao canal quente.
        vistos: dict[str, float] = {}
        try:
            while True:
                await asyncio.sleep(1.0)
                for _, marc in nav_novos(vistos, name):
                    queue.put_nowait(("nav", json.dumps({"url": marc["url"]})))
        except asyncio.CancelledError:
            raise
        except Exception:
            # Feature, não núcleo (mesmo trato do stats_pump): morrer calado deixaria "abrir
            # navegador" mudo nesta conexão sem rastro nenhum.
            _log.exception("sse: nav_pump da sessão %s morreu", name)

    def _enqueue_preview(text: str, md: bool = False, full: bool = False):
        # Atualiza o slot e enfileira UM marcador 'preview' por vez (drop-old). Sem await entre as
        # escritas -> consistente no loop single-thread.
        preview_slot["text"] = text
        preview_slot["md"] = md
        preview_slot["full"] = full
        if not preview_slot["pending"]:
            preview_slot["pending"] = True
            queue.put_nowait(("preview", None))

    async def tail_pump(path: str, start_offset: int | None = None):
        # Transcript do .jsonl (msgs canonicas). Alem de emitir, RASTREIA a ultima msg de assistente
        # em `committed` -> fonte de verdade pra suprimir preview duplicado. E quando um bloco commita
        # que e exatamente o que o preview mostra, LIMPA o preview na hora (sem esperar o broker mudar).
        # Recebe o path (em vez de fechar sobre um tailer fixo) pra poder ser recriado no rebind do /clear.
        try:
            from app.conversation_history import session_transfer, verify_boundary, live_event
            record = await asyncio.to_thread(session_transfer, name, path, current_provider)
            if record:
                offset = await asyncio.to_thread(verify_boundary, record, path)
                start_offset = max(offset, start_offset or 0)
            async for ev in get_adapter(current_provider).transcript_stream(path, start_offset):
                if record:
                    ev = await asyncio.to_thread(live_event, record, path, ev)
                    if ev is None:
                        continue
                if current_provider == "codex" and ev.kind == "user_msg":
                    await asyncio.to_thread(_confirm_codex_queue, name, path)
                if ev.kind == "assistant_msg" and ev.text:
                    committed["text"] = _norm(ev.text)
                    # Com o estado no hub, quem limpa a prévia gravada é ele.
                    if not rust_state and _already_committed(preview_slot["text"]):
                        _enqueue_preview("")
                # 3o item da tupla = `id:` do SSE (None nos demais eventos). So o transcript ganha
                # id: e o unico stream com posicao retomavel. state/preview/ping NAO podem ter id --
                # o browser guarda o ULTIMO id visto, entao um ping carimbado sobrescreveria a
                # posicao real do transcript e a retomada pularia mensagens.
                #
                # O id carrega o STEM do jsonl junto com o offset ("<uuid>:<byte>"). Offset puro era
                # inseguro: apos um /clear o transcript e OUTRO arquivo, e um offset antigo que por
                # acaso coubesse no tamanho do novo passava na validacao, dava seek no meio dele e
                # PULAVA calado todo o inicio da conversa nova (parse_line engole a linha parcial).
                # Com o stem, id de outro transcript simplesmente nao e honrado.
                if side:
                    continue
                ev_id = f"{session_key(path)}:{ev.offset}" if ev.offset is not None else None
                await queue.put(("message", ev.model_dump_json(), ev_id))
        except asyncio.CancelledError:
            raise  # rebind do watcher cancela este task de proposito -> nao reportar como erro
        except Exception as exc:  # surface, never swallow
            diag.registrar("sse.pump_falhou", "erro", sessao=name, provider=current_provider,
                           etapa="transcript", erro_tipo=type(exc).__name__)
            await queue.put(("__error__", exc))

    async def stats_pump(path: str):
        # Faixa de estatísticas (turnos/steps/tokens/tempos) — fold incremental do MESMO arquivo
        # do transcript, IO no threadpool. FEATURE, não núcleo: diferente dos outros pumps, erro
        # aqui NUNCA derruba o stream (regra do incidente 2026-07-23) — loga e a faixa some.
        acc = StatsAccumulator.compartilhado(current_provider, path)
        if acc is None:
            return                           # provider sem fold -> sem faixa
        try:
            last = None
            while True:
                snap = await asyncio.to_thread(acc.collect)
                if snap:
                    # Medida do stream (adapter sem terminal ou plugin) vence a reserva do transcript.
                    snap.update(live_snapshot(name, session_key(path), acc.last_call_ts()))
                if snap and snap != last:
                    last = snap
                    await queue.put(("stats", json.dumps(snap)))
                await asyncio.sleep(1.0)   # 1 s: o tok/s "agora" é ao vivo
        except asyncio.CancelledError:
            raise                            # rebind do /clear cancela de propósito
        except Exception:
            _log.exception("sse: stats_pump falhou name=%s (faixa desligada)", name)
        finally:
            acc.soltar()

    async def jsonl_watcher():
        # Detecta /clear (e qualquer troca de transcript): o claude abre um .jsonl NOVO, mas o tailer foi
        # bindado no antigo -> nada novo chegaria ate o EventSource reconectar (o usuario tinha que sair e
        # voltar). Aqui, vigia o jsonl ATIVO desta sessao e, quando diverge do bindado, sinaliza reset.
        # IMPORTANTE: usa a MESMA resolucao do endpoint /events (registry.list -> resolve()): cmdline
        # --session-id, depois fd aberto, depois btime, depois newest-by-mtime. Espelhar o endpoint
        # garante que o watcher dispare exatamente quando um reconnect mudaria de transcript.
        # E vigia o PROVIDER junto, porque ele tambem muda debaixo de um stream ja aberto: uma
        # sessao Pi/Kimi recem-criada leva ~15s ate a extensao publicar o bilhete do pane, e nesse
        # meio-tempo o registry a classifica como "claude" e resolve um caminho no layout do Claude,
        # que nunca vai existir. Medido 21/08/2026: `sse: abriu name=hangar provider=claude
        # jsonl=a05ee4a8-….jsonl` as 16:01:14, com o .jsonl do Pi nascendo as 16:01:31 noutro
        # diretorio. Rebindar SO o arquivo nao bastava — o adapter (parser, monitor, previa) e
        # escolhido na abertura, entao o tailer lia o arquivo certo com o parser errado e o chat
        # ficava mudo ate o usuario sair e voltar (era o unico jeito de abrir um stream novo).
        current = jsonl
        current_prov = provider
        # Codex: trocar de modo (`/modo-execucao`) troca o dono do estado.
        current_headless = await asyncio.to_thread(_codex_headless, name) if provider == "codex" else None
        pending = None       # candidato a nova resolucao, aguardando confirmar persistencia
        pending_n = 0
        falhou = False
        while True:
            await asyncio.sleep(2)
            try:
                viva = next((s for s in await _cached_list() if s.name == name), None)
            except Exception as exc:
                if not falhou:
                    diag.registrar("sse.resolucao_falhou", "erro", sessao=name,
                                   provider=current_prov, erro_tipo=type(exc).__name__)
                falhou = True
                viva = None
            else:
                if falhou:
                    diag.registrar("sse.resolucao_recuperada", sessao=name, provider=current_prov)
                falhou = False
            live = viva.jsonl if viva else None
            live_prov = chave_de(name, viva.provider) if viva and viva.provider else current_prov
            live_headless = bool(getattr(viva, "headless", False)) if viva else current_headless
            flipped = (live_prov == "codex" == current_prov and current_headless is not None
                       and live_headless != current_headless)
            current_headless = live_headless
            if live and (live_prov != current_prov or flipped):
                # Troca de provider NAO espera os 2 polls do jsonl: ela nao oscila como a resolucao
                # por mtime — e a sessao terminando de se identificar. Segurar aqui e deixar o chat
                # mudo mais tempo, sem nada em troca.
                current_prov = live_prov
                current = live
                pending = None
                pending_n = 0
                queue.put_nowait(("__reprovider__", (live_prov, live)))
                continue
            if not live or live == current:
                pending = None
                pending_n = 0
                continue
            # Mudou: exige PERSISTIR por >=2 polls antes de resetar. Filtra flips transitorios (a
            # resolucao oscila quando o processo com --session-id some por 1 ciclo) que limpavam o chat.
            pending_n = pending_n + 1 if live == pending else 1
            pending = live
            if pending_n >= 2:
                current = live
                pending = None
                pending_n = 0
                queue.put_nowait(("__reset__", live))

    async def preview_pump(fonte):
        # Assina a fonte COMPARTILHADA da sessao (1 broker pra N conexoes: PreviewBroker faz 1 loop
        # de capture do pane; PushPreviewSource so guarda o ultimo push, sem loop). Coalesce (slot +
        # 1 marcador). SUPRIME texto JA COMMITADO no .jsonl (gap entre blocos) -> manda "" pra nao
        # duplicar. Fail-loud como os outros pumps.
        try:
            # A fonte emite o TRIO: `md` diz se aquele texto e markdown cru (sidecar do agente) ou
            # ja pintado pela TUI (pane); `full` diz se e incremental (seguro sem teto — ver
            # PreviewEvent). Vem junto de proposito — ver o docstring do subscribe().
            async for text, md, full in fonte.subscribe():
                _enqueue_preview("" if _already_committed(text) else text, md, full)
        except Exception as exc:  # surface, never swallow
            diag.registrar("sse.pump_falhou", "erro", sessao=name, provider=current_provider,
                           etapa="previa", erro_tipo=type(exc).__name__)
            await queue.put(("__error__", exc))

    async def rust_state_pump():
        # Estado, prévia, pergunta e sugestão do `Monitor` do Rust, como o hub os publicou.
        try:
            async for event, data in _canal_do_estado(name):
                await queue.put(("__rust__", (event, data)))
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # surface, never swallow
            diag.registrar("sse.estado_rust_falhou", "erro", sessao=name, provider=current_provider,
                           codigo=getattr(exc, "code", type(exc).__name__))
            await queue.put(("__error__", exc))

    def _fontes_do_estado(prov, rust):
        """Tarefas do estado ao vivo por chave: as do Python, o canal do hub, ou nada (conexão interna
        de sessão do Rust, que é o próprio hub). `rust` vem de quem montou o `broker`: reler o modo
        aqui deixaria os dois discordarem se ele mudasse no meio. Pensamento e ferramenta em voo do
        Claude e do Codex sem terminal também são do hub; nos outros, o Python os produz."""
        if rust:
            fontes = {} if side else {"rust": asyncio.create_task(rust_state_pump())}
        else:
            fontes = {"state": asyncio.create_task(pump("state", _monitor_de(prov))),
                      "preview": asyncio.create_task(preview_pump(broker))}
        if not (rust and prov in ("codex", CLAUDE_HEADLESS)):
            fontes["pensamento"] = asyncio.create_task(em_voo_pump("pensamento", fonte_pensamento(name)))
            fontes["ferramenta"] = asyncio.create_task(em_voo_pump("ferramenta", fonte_ferramenta(name)))
        return fontes

    def _segue_transcript(prov, rust):
        """A conexão interna de sessão do Rust só seguia o transcript para a prévia, que é do hub; a do
        Codex continua, porque o `user_msg` confirma a fila dele."""
        return not (side and rust and prov != "codex")

    em_voo_slots = {"pensamento": {"text": "", "pending": False},
                    "ferramenta": {"text": "", "pending": False}}

    async def em_voo_pump(evento: str, fonte):
        # Pensamento e ferramenta em voo (só o Claude sem terminal publica). Mesmo slot coalescido
        # da prévia: rajada de deltas não pode atrasar o transcript na fila compartilhada. Sessão de
        # outro provider só fica esperando uma fonte que nunca muda.
        slot = em_voo_slots[evento]
        try:
            async for text, _md, _full in fonte.subscribe():
                slot["text"] = text
                if not slot["pending"]:
                    slot["pending"] = True
                    queue.put_nowait((evento, None))
        except Exception as exc:  # surface, never swallow
            diag.registrar("sse.pump_falhou", "erro", sessao=name, provider=current_provider,
                           etapa=evento, erro_tipo=type(exc).__name__)
            await queue.put(("__error__", exc))

    async def band_pump():
        # Faixa e painéis dos mods: fonte própria. Na carona do `state` ela só saía quando o estado
        # mudava, e o mod que relê com a sessão parada ficava velho na tela.
        vista = 0
        try:
            while True:
                atual = await plugin_bridge.esperar_faixa(name, vista, 30)
                if atual != vista:
                    vista = atual
                    await queue.put(("plugin_ui", None))
        except Exception as exc:  # surface, never swallow
            diag.registrar("sse.pump_falhou", "erro", sessao=name, provider=current_provider,
                           etapa="faixa", erro_tipo=type(exc).__name__)
            await queue.put(("__error__", exc))

    async def toast_pump():
        # Avisos dos mods: cada conexão começa do zero e recebe os que ainda não venceram; o app
        # descarta pelo id o que já mostrou. Sai sem id de SSE, que é o cursor do transcript.
        seen = 0
        try:
            while True:
                seen, fresh = await plugin_bridge.wait_toasts(name, seen, 30)
                for toast in fresh:
                    await queue.put(("plugin_toast", json.dumps(toast, ensure_ascii=False)))
        except Exception as exc:  # surface, never swallow
            diag.registrar("sse.pump_falhou", "erro", sessao=name, provider=current_provider,
                           etapa="toast", erro_tipo=type(exc).__name__)
            await queue.put(("__error__", exc))

    sugestao_emitida = ""          # ultima sugestao que saiu; so a mudanca vira evento
    ask_q_emitted = False          # impede reemissao enquanto o mesmo prompt permanece na tela
    codex_question_emitted = ""
    ultimo_estado = None           # ultimo `state` emitido; None ate o primeiro tick
    _fantasma_logado = {"v": False}
    prev_deliverable = False     # init False -> 1o estado entregavel pos-(re)connect tambem dispara 1
                                 # drain (recovery de restart/reconexao com pendencia)
    drain_tasks: set = set()     # drains fire-and-forget; NAO entram em `tasks` (nao cancelar no disconnect)

    def drain_done(task):
        drain_tasks.discard(task)
        if not task.cancelled() and (exc := task.exception()) is not None:
            from app.runtime_coordinator import failure_reason
            reason = failure_reason(exc)
            diag.registrar("sse.fila_falhou", "erro", sessao=name, provider=current_provider,
                           erro_tipo=reason["codigo"], detalhe=reason["detalhe"])

    # start_offset so vale pro tail INICIAL (veio do Last-Event-ID desta conexao). O rebind do
    # /clear abaixo recria sem ele: o transcript e outro arquivo, o offset antigo nao significa nada.
    if provider == "codex":
        # Confirma antigas antes de o follow republicar a fila na reconexão.
        await asyncio.to_thread(_confirm_codex_queue, name, jsonl)
    # A conexão interna só seguia o transcript para a supressão da prévia, que agora é do hub.
    tail_task = asyncio.create_task(tail_pump(jsonl, start_offset)) if _segue_transcript(provider, rust_state) else None
    stats_task = asyncio.create_task(stats_pump(jsonl))
    # Nomeadas porque sao refeitas quando o provider muda no meio do stream (__reprovider__): cada
    # uma carrega o adapter antigo dentro de si (parser do transcript, fold das estatisticas,
    # monitor de estado, fonte da previa) e trocar so uma deixaria o stream meio num provider e
    # meio no outro.
    state_tasks = _fontes_do_estado(provider, rust_state)
    tasks = [
        *([tail_task] if tail_task else []),
        stats_task,
        # Fila duravel: user_msg sinteticos (id "queued-") pras msgs enfileiradas. O front faz o
        # dedup cruzado (queued- vs real) por texto.
        asyncio.create_task(pump("message", pqueue.follow(min_ts=start_ts, emit_confirmed=True))),
        *state_tasks.values(),
        asyncio.create_task(ping_loop()),
        asyncio.create_task(jsonl_watcher()),
        asyncio.create_task(band_pump()),
    ]
    # Aviso de mod pode trazer texto sensível do dono: convidado não recebe. O marcador do navegador
    # também não: a url da página de rascunho leva o token do dono.
    if count_app:
        tasks.append(asyncio.create_task(toast_pump()))
        tasks.append(asyncio.create_task(nav_pump()))
    # NUCLEO (conexao): instrumentacao do CICLO DE VIDA do stream. O sintoma relatado é "a conversa
    # para e só volta fechando/abrindo o app", e o log de acesso do uvicorn só mostra a conexão
    # FECHANDO — sem duração, sem motivo, sem quanto foi entregue. Sem isso a causa (queda de rede
    # do celular / iOS suspendendo / erro num pump / cancelamento) é indistinguível e vira chute.
    _t0 = time.monotonic()
    _sent = {"message": 0, "state": 0, "preview": 0, "ping": 0, "other": 0}
    _why = "cliente desconectou"
    motivo_diag = "cliente_desconectou"
    _last_ctx_warn = {"sl": None}
    _log.info("sse: abriu name=%s provider=%s jsonl=%s", name, provider, Path(jsonl).name if jsonl else None)
    diag.registrar("sse.abriu", sessao=name, provider=provider,
                   etapa="retomada" if start_offset is not None else "inicio")
    if count_app:
        plugin_bridge.app_entrou()
    try:
        if side:
            # Dentro do try: quem desconecta já aqui ainda passa pelo finally (tarefas, app_saiu).
            yield _info_event(name, current_provider, current_jsonl)
        while True:
            # Só o tail_pump enfileira o 3o item (o offset -> `id:` do SSE); os demais produtores
            # continuam mandando pares. Desempacota tolerante em vez de tocar em todos eles.
            item = await queue.get()
            event, data = item[0], item[1]
            ev_id = item[2] if len(item) > 2 else None
            if event == "__error__":
                _why = f"erro no pump: {type(data).__name__}: {data}"
                motivo_diag = "falha_pump"
                raise data
            if event == "__rust__":
                # Do hub (`Monitor` ou feed do runtime): já saiu com sugestão, pergunta, problema e entrega resolvidos.
                rust_event, rust_data = data
                _sent[rust_event if rust_event in _sent else "other"] += 1
                yield {"event": rust_event, "data": rust_data}
                continue
            if event == "__reprovider__":
                # A sessao terminou de se identificar (ex: nasceu como "claude" e e Pi). Refaz TUDO
                # que depende do adapter — transcript, estatisticas, estado e previa — e manda
                # `reset` pro front recarregar o history pelo caminho certo. Sem o reset, o que ja
                # estava no arquivo antes desta troca nunca apareceria: o tailer novo entra pelo
                # TAIL, e o history que o front leu foi lido do provider errado.
                novo_prov, novo_jsonl = data
                diag.registrar("sse.reiniciou", sessao=name, provider=novo_prov, etapa="troca_provider")
                _log.info("sse: provider mudou name=%s %s -> %s jsonl=%s",
                          name, current_provider, novo_prov,
                          Path(novo_jsonl).name if novo_jsonl else None)
                for t in (tail_task, stats_task, *state_tasks.values()):
                    if t is not None:
                        tasks.remove(t)
                        t.cancel()
                current_provider = novo_prov
                current_jsonl = novo_jsonl
                committed["text"] = ""
                rust_state = await _estado_do_rust_async(novo_prov, name)
                broker = None if rust_state else _broker_de(novo_prov)
                # A prévia do provider anterior sai; na conexão interna de sessão do Rust quem
                # limpa é o hub, e uma prévia do Python ali seria descartada como vazamento.
                if not (side and rust_state):
                    _enqueue_preview("")
                if broker is not None:
                    broker.reset()
                ask_q_emitted = False
                tail_task = asyncio.create_task(tail_pump(novo_jsonl)) if _segue_transcript(novo_prov, rust_state) else None
                stats_task = asyncio.create_task(stats_pump(novo_jsonl))
                state_tasks = _fontes_do_estado(novo_prov, rust_state)
                tasks += [t for t in (tail_task, stats_task, *state_tasks.values()) if t is not None]
                yield (_info_event(name, current_provider, current_jsonl) if side
                       else {"event": "reset", "data": "{}"})
                continue
            if event == "__reset__":
                diag.registrar("sse.reiniciou", sessao=name, provider=current_provider,
                               etapa="troca_transcript")
                # Troca de transcript (ex: /clear). Re-binda o tailer no jsonl novo, zera o estado de
                # suppress/preview, e manda 'reset' pro front recarregar o history do zero.
                #
                # LOGA como o __reprovider__ ao lado: este `reset` APAGA a conversa da tela e manda
                # recarregar, então quando alguém relata "o chat ficou vazio" a primeira pergunta é
                # se houve reset — e sem esta linha ela não tinha resposta, porque era o único dos
                # dois caminhos que não deixava rastro nenhum (26/08/2026).
                _log.info("sse: transcript trocou name=%s %s -> %s", name,
                          Path(current_jsonl).name if current_jsonl else None,
                          Path(data).name if data else None)
                if tail_task is not None:
                    tasks.remove(tail_task)
                    tail_task.cancel()
                tasks.remove(stats_task)
                stats_task.cancel()          # transcript novo -> acumulador novo (faixa zera)
                committed["text"] = ""
                if broker is not None:
                    _enqueue_preview("")
                    broker.reset()       # o texto em voo era do transcript APAGADO: sem isto o
                                         # broker republicava a conversa velha na 1a reconexao
                                         # (a supressao acabou de ser desarmada na linha acima)
                current_jsonl = data
                # O broker lê o transcript pela conexão que o pegou por último; se ela já fechou, o
                # leitor dela ficou no transcript apagado e a prévia parava de vez. Quem fez o reset
                # está viva: reinstala o leitor dela.
                if broker is not None:
                    broker = _broker_de(current_provider)
                ask_q_emitted = False
                if tail_task is not None:
                    tail_task = asyncio.create_task(tail_pump(data))
                    tasks.append(tail_task)
                stats_task = asyncio.create_task(stats_pump(data))
                tasks.append(stats_task)
                # O monitor e compartilhado por transcript: o deste chat agora e outro. O canal do
                # hub segue a troca sozinho (o `rebind` dele vem do `info` da conexão interna).
                if "state" in state_tasks:
                    tasks.remove(state_tasks["state"])
                    state_tasks["state"].cancel()
                    state_tasks["state"] = asyncio.create_task(pump("state", _monitor_de(current_provider)))
                    tasks.append(state_tasks["state"])
                yield (_info_event(name, current_provider, current_jsonl) if side
                       else {"event": "reset", "data": "{}"})
                continue
            if event == "preview":
                # Le o ULTIMO texto do slot na hora do envio (frames antigos ja foram sobrescritos).
                # SEM id: pra reconexao do EventSource nao replayar preview velho via Last-Event-ID.
                preview_slot["pending"] = False
                _sent["preview"] += 1
                if preview_slot["text"] and ultimo_estado not in (None, "working"):
                    # Assinatura da bolha fantasma: texto em voo numa sessao que nao esta
                    # trabalhando. Um por stream basta pra apontar a fonte (md/full) e o trecho.
                    if not _fantasma_logado["v"]:
                        _fantasma_logado["v"] = True
                        _log.info("sse: previa com texto em sessao %s name=%s md=%s full=%s "
                                  "trecho=%r", ultimo_estado, name, preview_slot["md"],
                                  preview_slot["full"], preview_slot["text"][:80])
                yield {"event": "preview",
                       "data": PreviewEvent(session=name, text=preview_slot["text"],
                                            md=bool(preview_slot["md"]),
                                            full=bool(preview_slot["full"]),
                                            vivo=isinstance(broker, PushPreviewSource)).model_dump_json()}
                continue
            if event in em_voo_slots:
                # SEM id, como a prévia: reconexão não pode replayar o que já saiu de cena.
                slot = em_voo_slots[event]
                slot["pending"] = False
                yield {"event": event, "data": json.dumps({"text": slot["text"]})}
                continue
            if event == "plugin_ui":
                # Com o estado no hub, a faixa vem dele (`__rust__`): a daqui não tem a dos mods que o
                # Rust atende. A conexão interna segue mandando a daqui, que é a fonte do hub nas outras.
                if not (rust_state and not side):
                    yield {"event": "plugin_ui", "data": plugin_bridge.band_json(name)}
                continue
            if event == "state":
                # Sugestão do terminal (a frase cinza que o Tab aceita lá): sem fonte própria, ela
                # pega carona no tique do estado — 0,75s é de sobra pra uma frase que só aparece no
                # fim do turno, e isso evita mais um pump por sessão. Só quando MUDA.
                sug = plugin_bridge.sugestao(name)
                if sug != sugestao_emitida:
                    sugestao_emitida = sug
                    yield {"event": "suggest", "data": json.dumps({"text": sug}, ensure_ascii=False)}
                # Rastreia transicoes do awaiting_input pra resetar o guard de emissao unica.
                # Quando awaiting_input + overlay (rodape de abas = AskUserQuestion estruturado),
                # emite ask_question UMA VEZ por prompt; reseta ao sair do estado.
                parsed_state = json.loads(data)
                if current_provider == "claude" and not parsed_state.get("problema"):
                    # Sessão com terminal no Rust: o monitor do pane não sabe que o runtime falhou.
                    from app.runtime_adapter import runtime_problem
                    if problem := runtime_problem(name):
                        parsed_state.update(problema=problem[0], problema_detalhe=problem[1])
                        data = json.dumps(parsed_state, ensure_ascii=False)
                ultimo_estado = parsed_state.get("state")
                # Diagnostico do "medição indisponível": loga o statusline CRU quando o segmento 💬
                # nao tem os 2 pares. Uma vez por statusline DISTINTO (nao a cada tick) pra nao virar
                # firehose — um StateEvent sai a cada 0.75s.
                _sl = parsed_state.get("status_line")
                if _sl and context_pairs(_sl) < 2 and _sl != _last_ctx_warn["sl"]:
                    _last_ctx_warn["sl"] = _sl
                    _log.info("sse: sem métrica de contexto name=%s statusline=%r", name, _sl)
                if current_provider in ("codex", CLAUDE_HEADLESS):
                    question_data = json.dumps(parsed_state.get("codex_question"), ensure_ascii=False)
                    if question_data != codex_question_emitted:
                        codex_question_emitted = question_data
                        yield {"event": "ask_question", "data": question_data}
                elif parsed_state.get("state") != "awaiting_input":
                    ask_q_emitted = False
                elif not ask_q_emitted:
                    ask_ev = _ask_question_event(data, current_jsonl)
                    if ask_ev:
                        ask_q_emitted = True
                        yield ask_ev
                # Drain gatilho: quando o pane volta a aceitar texto livre (overlay/menu fechou, ou a
                # sessao voltou ao idle), entrega as enfileiradas pendentes. Deriva a entregabilidade
                # dos campos do PROPRIO StateEvent — reusa o stream do StateMonitor, sem novo poll.
                deliverable_now = (
                    parsed_state.get("state") not in ("awaiting_input", "dead")
                    and not parsed_state.get("overlay")
                )
                if deliverable_now and not prev_deliverable:
                    # fire-and-forget (adapter.drain ja roda no threadpool internamente) — nunca await
                    # no loop SSE. FORA de `tasks`: deixar um drain em voo terminar apos o phone
                    # desconectar e correto (entrega duravel nao depende do phone ficar conectado).
                    # get_adapter na hora, e nao um `adapter` fixado na abertura: o provider da
                    # sessao pode ter trocado no meio do stream (__reprovider__), e drenar a fila
                    # pelo adapter errado digitaria no terminal de um jeito que aquela TUI nao espera.
                    dt = asyncio.create_task(get_adapter(current_provider).drain(name, current_jsonl))
                    drain_tasks.add(dt)
                    dt.add_done_callback(drain_done)
                prev_deliverable = deliverable_now
            _sent[event if event in _sent else "other"] += 1
            out = {"event": event, "data": data}
            if ev_id is not None:
                out["id"] = str(ev_id)
            yield out
    except asyncio.CancelledError:
        # Fechamento NORMAL: o cliente sumiu e o starlette cancela o gerador. Distinguir isso de
        # um erro é o ponto — os dois terminavam o stream do mesmo jeito silencioso.
        _why = "cancelado (cliente sumiu / servidor encerrando)"
        motivo_diag = "cancelado"
        raise
    except Exception as exc:
        if motivo_diag != "falha_pump":
            motivo_diag = "falha_stream"
            diag.registrar("sse.falhou", "erro", sessao=name, provider=current_provider,
                           erro_tipo=type(exc).__name__)
        raise
    finally:
        if count_app:
            plugin_bridge.app_saiu()
        diag.registrar("sse.fechou", "erro" if motivo_diag.startswith("falha_") else "ok",
                       sessao=name, provider=current_provider, detalhe=motivo_diag,
                       ms=int((time.monotonic() - _t0) * 1000), quantidade=sum(_sent.values()))
        _log.info(
            "sse: fechou name=%s dur=%.1fs motivo=%s enviados=msg:%d state:%d preview:%d ping:%d",
            name, time.monotonic() - _t0, _why,
            _sent["message"], _sent["state"], _sent["preview"], _sent["ping"],
        )
        # So cancela e retorna (NAO await): um pump preso num asyncio.to_thread (tmux) nao e
        # cancelavel -> aguardar o gather aqui travava o aclose() do gerador, segurava a conexao
        # meio-aberta e, em rajada de reconexao do mobile, ia acumulando ate exaurir o threadpool
        # (a /api/sessions travava). Os inotify saem no GC; melhor isso que travar o disconnect.
        for t in tasks:
            t.cancel()
