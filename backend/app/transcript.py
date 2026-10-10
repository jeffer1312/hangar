import asyncio
import base64
import hashlib
import json
import logging
import os
import re
import time
from contextlib import nullcontext
from datetime import datetime, timezone
from pathlib import Path
from typing import AsyncIterator, Iterable, Optional
from watchfiles import awatch
from app.models import ChatEvent

_log = logging.getLogger("hangar.transcript")

# Backfill do SSE: re-envia so as ULTIMAS N linhas do transcript em cada (re)conexao, nao o arquivo
# inteiro. Antes o follow() comecava em pos=0 e re-shippava dezenas de MB a cada reconexao do mobile
# (background/foreground, watchdog). 200 e a maneta de calibracao: cobre o gap de uma reconexao normal
# (poucos segundos) com folga; sessao com <= 200 linhas mantem o backfill completo (offset 0).
_BACKFILL_LINES = 200

# Intervalo do poll enquanto a pasta do transcript nao existe (ver o laco do follow). Vale so no
# boot da sessao, entao 1s nao custa nada e nao atrasa nada que o usuario veja. Passando de
# _AVISA_ESPERA_PASTA polls a espera deixou de ser boot e vira aviso (uma vez, nao a cada poll).
_ESPERA_PASTA_S = 1.0
_AVISA_ESPERA_PASTA = 30

# Janela inicial do tail-read reverso do _tail_offset: 256KB cobre as 200 linhas do backfill na
# esmagadora maioria dos transcripts; quando nao cobre (linha gigante com base64 de imagem colada),
# ela quadruplica ate juntar as linhas ou alcancar o inicio do arquivo.
_TAIL_WINDOW = 256 * 1024

# Imagem colada no TERMINAL (TUI do Claude). O Claude grava 2 coisas: a msg do user com um bloco
# `image` (base64) + um marcador "[Image #N]" no texto; E uma entrada user SINTETICA cujo texto é só
# "[Image: source: <path>]" (referência). A 1ª vira bubble com thumbnail (image_count); a 2ª é meta.
# Quando o MODELO le uma imagem (tool Read), o harness injeta outra entrada user sintetica cujo texto
# e so "[Image: original WxH, displayed at ...]" (ou "[Image]" sem resize) — tambem meta, nao conversa.
# Pega qualquer entrada cujo texto INTEIRO seja "[Image]" ou "[Image: ...]": usuario nunca digita isso.
_IMAGE_SOURCE_RE = re.compile(r"^\[Image(?:\]|: [^\]]*\])$")   # entrada sintetica inteira = meta
_IMAGE_MARKER_RE = re.compile(r"\[Image #\d+\]\s*")             # ruido na legenda -> remover

# Interrupção: o Claude Code grava uma entrada "user" sintética com este texto. Não é fala de
# ninguém — vira aviso, pra interface desenhar linha discreta em vez de bolha em inglês.
_INTERRUPCAO_RE = re.compile(r"^\[Request interrupted by user[^\]]*\]$")


def _first(content: list, type_name: str) -> Optional[dict]:
    for item in content:
        if isinstance(item, dict) and item.get("type") == type_name:
            return item
    return None


# Claude Code logs slash-commands and local command I/O as synthetic "user" entries
# wrapped in these tags. They are tooling meta, not conversation — keep them out of the chat.
_COMMAND_META_PREFIXES = (
    "<command-name>", "<command-message>", "<command-args>",
    "<local-command-caveat>", "<local-command-stdout>", "<local-command-stderr>",
    "<bash-input>", "<bash-stdout>", "<bash-stderr>",
    # Invocacao de skill (/handoff, etc): o Claude Code injeta o corpo do SKILL.md como
    # entrada "user" sintetica que comeca com esta linha. E meta de tooling, nao conversa —
    # mesmo tratamento dos comandos acima (nao renderiza bubble).
    "Base directory for this skill:",
    # Notificacao de Workflow concluido: o harness injeta um <task-notification>...</task-notification>
    # como entrada "user" sintetica. Tooling meta, nao conversa — fora do chat.
    "<task-notification>",
    # Lembrete do harness ("The user named this session…", contexto de skill, etc): injetado como
    # entrada "user" sintetica. Quando vem sozinho (sem texto real), e meta — fora do chat. Quando
    # vem ANEXADO a uma msg real, _strip_meta_blocks remove so o bloco e mantem o texto do usuario.
    "<system-reminder>",
)

# Blocos de meta do harness embutidos no texto de uma msg de usuario. Removidos antes de exibir;
# se sobrar so o bloco, a msg inteira e meta e nao vira bubble.
_META_BLOCK_RE = re.compile(r"<system-reminder>.*?</system-reminder>", re.DOTALL)

# Texto COLADO no composer (o hangar-send entrega por paste-buffer, entao todo recado cai aqui):
# o CLI grava embrulhado em <pasted_content id="…">…</pasted_content>. O envelope e marcacao do
# harness, nao conversa — fica so o conteudo, que e o que a pessoa (ou a sessao-irma) escreveu.
# O id se repete na tag de fechamento e e exigido igual: texto da pessoa que so CITE as tags nao
# tem esse par casado e continua inteiro.
_PASTED_RE = re.compile(r'<pasted_content id="([^"]*)">\n?(.*?)\n?</pasted_content id="\1">', re.DOTALL)

# task-id de uma <task-notification> (fim de agente/workflow em background). A notificacao fica
# fora do chat (e ruido), mas o painel de Atividade precisa do sinal de termino: viram um
# tool_result SINTETICO com tool_use_id="task:<id>" (o front nunca renderiza tool_result orfao;
# so o fold de atividade consome).
_TASK_NOTIF_RE = re.compile(r"<task-id>([^<]+)</task-id>")

# Relatório de subagente entregue pelo harness como msg "user" embrulhada, com `from` = agentId do
# launch. É conversa entre agentes, não do usuário: o hand-back final fecha o Agent como a
# <task-notification>; o resto fica fora do chat, como ela. Exige o embrulho inteiro, pra texto
# do usuário que só CITE a tag continuar dele.
_AGENT_MSG_RE = re.compile(r'<agent-message from="([^"]+)"[^>]*>(.*)</agent-message>', re.DOTALL)


# Equipe de agentes do Claude Code: recado de colega pro líder, entregue como msg "user" que começa
# com "Another Claude session sent a message:" e traz um ou mais <teammate-message teammate_id=…>.
# Não é fala do usuário: recado com conteúdo vira "[de: colega] …" (a bolha de outra sessão que o
# front já desenha); o aviso JSON de colega ocioso some.
_TEAMMATE_INICIO_RE = re.compile(r"(?:Another Claude session sent a message:\s*)?<teammate-message\b")
_TEAMMATE_BLOCO_RE = re.compile(r'<teammate-message\b([^>]*)>\n?(.*?)\n?</teammate-message>', re.DOTALL)


def _teammate_textos(texto) -> Optional[tuple[list[str], list[str]]]:
    """None = não é recado de colega. Senão (recados a mostrar, colegas que ficaram ociosos)."""
    if not isinstance(texto, str):
        return None
    t = texto.lstrip()
    if not _TEAMMATE_INICIO_RE.match(t):
        return None
    blocos = _TEAMMATE_BLOCO_RE.findall(t)
    if not blocos:
        return None  # tag sem bloco válido: segue como texto normal, nada some
    out, ociosos = [], []
    for attrs, corpo in blocos:
        corpo = corpo.strip()
        nome = dict(_PEER_ATTR_RE.findall(attrs)).get("teammate_id") or "colega"
        if corpo.startswith("{"):
            try:
                aviso = json.loads(corpo)
            except ValueError:
                aviso = None
            if isinstance(aviso, dict):
                if aviso.get("type") == "idle_notification":
                    ociosos.append((nome, _ts(aviso)))
                continue  # aviso estruturado (colega ocioso etc.), não recado
        if corpo:
            out.append(f"[de: {nome}] {corpo}")
    return out, ociosos


def _teammate_eventos(texto, id_: str) -> Optional[list[ChatEvent]]:
    lido = _teammate_textos(texto)
    if lido is None:
        return None
    textos, ociosos = lido
    eventos = [ChatEvent(kind="user_msg", id=_sub_id(id_, k), text=t) for k, t in enumerate(textos)]
    # Colega ocioso fecha o "teammate:<nome>" do spawn (bg_agent_id), como a <task-notification>.
    # O ts e o do aviso, nao o da entrega: ele chega ao lider atrasado, depois do SendMessage que ja
    # reacordou o colega, e o fold so fecha com aviso posterior a esse SendMessage.
    eventos += [ChatEvent(kind="tool_result", id=_sub_id(id_, len(eventos) + j),
                          tool_use_id=f"task:teammate:{nome}", result="task-notification", ts=ts)
                for j, (nome, ts) in enumerate(ociosos)]
    return eventos


def _agent_msg(texto, id_: str) -> Optional[list[ChatEvent]]:
    if not isinstance(texto, str) or not (m := _AGENT_MSG_RE.fullmatch(texto.strip())):
        return None
    # Dois envelopes colados casariam inteiros no `.*` e o segundo agente sumiria com o id do primeiro.
    if "<agent-message" in m.group(2) or "</agent-message>" in m.group(2):
        return None
    if "[Subagent hand-back]" not in m.group(2):
        return []   # ponytail: recado intermediário do subagente some; mostrar se fizer falta
    return [ChatEvent(kind="tool_result", id=id_, tool_use_id=f"task:{m.group(1)}",
                      result="task-notification")]


# Recado NATIVO de outra sessao Claude (cross-session messaging, claude 2.1.224+). Medido em
# 07/08/2026 num envio real: chega como type='user' com `isMeta: True` — ou seja, cairia no descarte
# de meta la embaixo e o app nunca mostraria recado nenhum (bolha ausente, nao bolha feia). Vem com
# um `origin` estruturado: {kind:"peer", from:"uds:/run/user/1000/cc-socks/<pid>.sock",
# verifiedPeerPid, name, fromMode, body, msg_id}.
#
# O `message.content` NAO serve pra exibir: traz o embrulho <cross-session-message> mais um paragrafo
# inteiro de instrucao sobre lavagem de permissao. Quem presta e o `origin.body`.
#
# Normaliza pro MESMO formato do hangar-send ("[de: <sessao>] texto") de proposito: e o que o front ja
# sabe ler (lib/format.ts parsePeerMessage) e o que alimenta a conversa do grupo no PairSheet, os
# badges e a deduplicacao. Assim o caminho nativo entra sem uma linha de mudanca no front, e o
# hangar-send segue valendo pra tudo que o nativo nao faz (outra maquina iniciando, Codex/Pi, --group).
_PEER_WRAP_RE = re.compile(r"<cross-session-message\b([^>]*)>\n?(.*?)\n?</cross-session-message>",
                           re.DOTALL)
_PEER_ATTR_RE = re.compile(r'([\w-]+)="([^"]*)"')
_PEER_PREFIXO_RE = re.compile(r"^\[(de|grupo|painel):\s*[^\]]+\]")
_PEER_SOCK_PID_RE = re.compile(r"/(\d+)\.sock")


def _peer_nome(pid: Optional[int], fallback) -> str:
    """Nome tmux do remetente. Cai no `from-name` (o TITULO da sessao) quando nao der pra resolver —
    recado com nome menos preciso e melhor que recado sumido.

    `fallback` chega SEM tipo declarado de proposito: e valor cru de JSON. Um `origin.name` que venha
    como numero ou lista fazia `(fallback or "").strip()` levantar AttributeError — e essa excecao
    subia por parse_line -> _read_from -> follow() ate o `except` do sse.py, derrubando o tail da
    sessao INTEIRA. Uma linha malformada parava o transcript todo, que e exatamente o oposto do que
    o docstring aqui promete (achado da revisao, reproduzido com origin.name=123)."""
    if isinstance(pid, int):
        try:
            # Import tardio: registry importa meio mundo (tmux, git, pair, pqueue) e este modulo e o
            # parser puro do transcript. So paga quando um recado nativo aparece, que e raro.
            from app.registry import name_of_pid
            if nome := name_of_pid(pid):
                return nome
        except Exception:                            # noqa: BLE001
            # Resolver o nome NUNCA pode derrubar o parse — mas engolir CALADO tambem nao: hoje o
            # name_of_pid ja se blinda sozinho, entao o que sobra aqui e o import. Um ImportError de
            # regressao circular deixaria TODO recado nativo caindo no titulo, sem nenhuma pista.
            _log.debug("nao consegui resolver o nome do remetente (pid %s)", pid, exc_info=True)
    return (fallback.strip() or "sessão") if isinstance(fallback, str) else "sessão"


def _peer_msg(obj: dict) -> Optional[str]:
    """Recado nativo de uma entrada `type='user'`, no formato do hangar-send ("[de: X] corpo").

    Le SO o `origin` estruturado — NUNCA procura o embrulho no texto. Procurar no texto aqui fazia
    qualquer mensagem do usuario contendo `<cross-session-message ...>` (colar este proprio codigo
    numa conversa, por exemplo) ser DESCARTADA e substituida pelo miolo das tags, com atribuicao
    `[de: alguem]` inventada — perda calada do que a pessoa escreveu, e forja da identidade que o
    PairSheet e os badges usam pra dizer de quem e a fala (achado da revisao).
    """
    origin = obj.get("origin")
    if not isinstance(origin, dict) or origin.get("kind") != "peer":
        return None
    corpo = origin.get("body")
    if not isinstance(corpo, str) or not corpo.strip():
        return None
    pid = origin.get("verifiedPeerPid")
    # Recado que o backend escreveu no socket ja vem com o prefixo do hangar-send ("[de: X] …",
    # "[grupo: X] …"): repetir "[de: …]" na frente perderia o [grupo:] e dobraria o remetente.
    if _PEER_PREFIXO_RE.match(corpo.lstrip()):
        return corpo.strip()
    return (f"[de: {_peer_nome(pid if isinstance(pid, int) else None, origin.get('name'))}] "
            f"{corpo.strip()}")


def _peer_msg_embrulhado(texto) -> Optional[str]:
    """Recado nativo consumido NO MEIO do turno: ali nao ha `origin`, so o texto embrulhado.

    Exige que o conteudo seja EXATAMENTE o embrulho — nada antes, nada depois. Medido no jsonl real
    (07/08/2026): o `queue-operation` do recado nativo comeca em `<cross-session-message` e termina
    em `</cross-session-message>`, sempre. Um texto do usuario que apenas CONTENHA as tags (codigo
    colado, este arquivo citado numa conversa) nao casa, entao continua sendo exibido inteiro e como
    dele. Sem essa exigencia, uma mensagem enfileirada durante trabalho podia ser trocada pelo miolo
    das tags com remetente forjado.
    """
    if not isinstance(texto, str):
        return None
    t = texto.strip()
    if not t.startswith("<cross-session-message") or not t.endswith("</cross-session-message>"):
        return None
    m = _PEER_WRAP_RE.fullmatch(t)
    if not m:
        return None
    corpo = m.group(2).strip()
    if not corpo or "<cross-session-message" in corpo or "</cross-session-message>" in corpo:
        # "Exatamente UM embrulho" levado ate DENTRO do corpo: com dois embrulhos concatenados, o
        # fullmatch casa mesmo assim (o corpo nao-guloso engole o fechamento do primeiro e a abertura
        # do segundo) e sairia uma bolha unica, com as tags cruas no meio, atribuida so ao primeiro
        # remetente. Nao foi observado — mas o dia em que dois recados forem coalescidos num campo so
        # e o dia em que isso vira texto podre exibido, em vez de duas mensagens.
        return None
    attrs = dict(_PEER_ATTR_RE.findall(m.group(1)))
    if _PEER_PREFIXO_RE.match(corpo.lstrip()):
        return corpo.strip()
    sock = _PEER_SOCK_PID_RE.search(attrs.get("from", ""))
    return f"[de: {_peer_nome(int(sock.group(1)) if sock else None, attrs.get('from-name'))}] {corpo}"


# Entrega BLOQUEADA por hook, registrada na entrada system: o Claude Code nao entrega o prompt ao
# agente mas deixa a tentativa no content. Formato medido (10x no transcript de 18/08/2026):
#   UserPromptSubmit operation blocked by hook:
#   ["<path do hook>"]: <erro>
#   (linha em branco)
#   Original prompt: <texto>
# Um recado '[de: desc2-exec2] RODADA 3 ENTREGUE...' ficou preso assim, tres vezes (as tres
# tentativas de reenvio), e o transcript.py o descartava junto com o ruido de tooling — o app
# nunca mostrou o texto e a bolha da fila ficou marcada 'nao chegou' pra sempre.
# O cabecalho "UserPromptSubmit operation blocked by hook:" e PARTE da ancora: "Original prompt:"
# sozinho aparece em citações (um aviso que menciona a frase); os dois juntos so existem na
# entrega bloqueada registrada pelo harness.
_ORIGINAL_PROMPT_RE = re.compile(
    r"UserPromptSubmit operation blocked by hook:\s*(.*?)\s*Original prompt: (.+)$", re.DOTALL)


def _blocked_prompt(content) -> Optional[tuple[str, str]]:
    """(texto, erro do hook) de um prompt barrado registrado numa entrada type='system', ou None.

    Vale pra recado entre sessoes E pra fala da pessoa: nos dois o agente nunca recebeu, e sem a
    bolha o envio some da conversa sem motivo a vista. O filtro e pelo FORMATO, nunca pelo tipo:
    system tambem carrega ruido de verdade (o /model, avisos do harness, 'Held peer message').

    'Original prompt:' e a ancora: e o marcador do harness que prova que o texto depois dele e
    uma ENTREGA registrada, nao um aviso SOBRE ela. Um aviso que apenas MENCIONE a frase nao
    passa — sem o cabecalho 'blocked by hook' + o marcador, sem bolha.
    """
    if not isinstance(content, str):
        return None
    m = _ORIGINAL_PROMPT_RE.search(content)
    if not m:
        # A ancora e o FORMATO do harness (medido na 2.1.234). Se uma versao futura mudar o texto,
        # o recado volta a sumir da tela EM SILENCIO — que e a caracteristica que fez este defeito
        # durar semanas sem ninguem notar. Entao: entrada `system` que cheira a recado e nao casa a
        # ancora vira aviso no log. Troca falha muda por falha registrada; nao muda comportamento.
        if "[de: " in content or "<cross-session-message" in content:
            _log.warning("entrada system parece recado e NAO casou a ancora do harness "
                         "(formato mudou?): %.200s", content.replace("\n", " "))
        return None
    erro, texto = m.group(1), m.group(2).strip()
    if not texto:
        return None
    return _peer_msg_embrulhado(texto) or texto, erro


def _is_command_meta(text: str) -> bool:
    return text.lstrip().startswith(_COMMAND_META_PREFIXES)


def _strip_meta_blocks(text: str) -> str:
    return _PASTED_RE.sub(r"\2", _META_BLOCK_RE.sub("", text)).strip()


_PATCH_MAX_LINES = 2000
_PATCH_MAX_START = 2**32 - 1


def _patch_hunks(obj: dict) -> Optional[list[dict]]:
    """Trechos do `structuredPatch` gravado ao lado do resultado. O arquivo inteiro fica para trás."""
    tur = obj.get("toolUseResult")
    raw = tur.get("structuredPatch") if isinstance(tur, dict) else None
    if not isinstance(raw, list) or not raw:
        return None
    hunks, total = [], 0
    for h in raw:
        if not isinstance(h, dict):
            return None
        starts = (h.get("oldStart"), h.get("newStart"))
        lines = h.get("lines")
        # bool é int em Python; um `true` no lugar do número não é posição de linha.
        if not all(isinstance(s, int) and not isinstance(s, bool) for s in starts):
            return None
        # Sem negativo, e com o teto do u32 do tipo compartilhado do Rust: os dois parsers recusam igual.
        if not all(0 <= s <= _PATCH_MAX_START for s in starts):
            return None
        if not isinstance(lines, list) or not all(isinstance(text, str) for text in lines):
            return None
        total += len(lines)
        if total > _PATCH_MAX_LINES:
            return None
        hunks.append({"old_start": starts[0], "new_start": starts[1], "lines": lines})
    return hunks


def _bg_agent_id(obj: dict) -> Optional[str]:
    """Id do subagente quando o resultado é o lançamento em segundo plano, não o resultado final."""
    tur = obj.get("toolUseResult")
    if not isinstance(tur, dict):
        return None
    # Agent com `name` vira colega de equipe: roda até o 1o aviso de ocioso (ver _teammate_eventos).
    if tur.get("status") == "teammate_spawned":
        nome = tur.get("name")
        return f"teammate:{nome}" if isinstance(nome, str) and nome else None
    if tur.get("status") != "async_launched":
        return None
    aid = tur.get("agentId")
    return aid if isinstance(aid, str) and aid else None


def parse_line(line: str) -> list[ChatEvent]:
    line = line.strip()
    if not line:
        return []
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return []
    return parse_obj(obj)


class RewriteFilter:
    """`claude --resume` regrava a conversa INTEIRA no mesmo jsonl: cada mensagem volta com uuid
    novo e o timestamp original, entao lido do zero o transcript mostra tudo duas vezes. Descarta
    linha de user/assistant cujo relogio esta mais de _JANELA_S atras do maior ja visto (a
    reescrita recomeca no inicio da sessao; retrocesso legitimo e de milissegundos) e, dentro da
    janela, repeticao exata de (timestamp, conteudo). Um por leitor: o estado e a posicao dele."""
    _JANELA_S = 60.0

    def __init__(self) -> None:
        self._max = 0.0
        self._recentes: dict[tuple[str, str], float] = {}

    def keep(self, obj: dict) -> bool:
        if obj.get("type") not in ("user", "assistant"):
            return True
        t = obj.get("timestamp")
        if not isinstance(t, str):
            return True
        try:
            ts = datetime.fromisoformat(t.replace("Z", "+00:00")).timestamp()
        except ValueError:
            return True
        if ts < self._max - self._JANELA_S:
            _log.debug("reescrita do --resume: descartando %s de %s (max %.0f)", obj.get("type"), t, self._max)
            return False
        conteudo = json.dumps((obj.get("message") or {}).get("content"), sort_keys=True,
                              ensure_ascii=False)
        fp = (t, hashlib.md5(conteudo.encode("utf-8")).hexdigest())
        if fp in self._recentes:
            _log.debug("reescrita do --resume: descartando repeticao exata de %s em %s", obj.get("type"), t)
            return False
        self._recentes[fp] = ts
        if ts > self._max:
            self._max = ts
            if len(self._recentes) > 256:
                self._recentes = {k: v for k, v in self._recentes.items() if v >= ts - self._JANELA_S}
        return True

    def parse_line(self, line: str) -> list[ChatEvent]:
        line = line.strip()
        if not line:
            return []
        try:
            obj = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            return []
        return parse_obj(obj) if self.keep(obj) else []


def _sub_id(uid: str, k: int) -> str:
    # Eventos extras da MESMA linha ganham sufixo deterministico (":1", ":2"...): o front deduplica
    # e keia bubble por id -> ids repetidos colapsariam blocos distintos numa bubble so. O 1o fica
    # com o uuid puro (o fetch de imagem usa o id cru como uuid da entrada no jsonl).
    return uid if k == 0 else f"{uid}:{k}"


# Anexos que o parse_obj transforma em evento. Os outros (resposta de hook assíncrono, hook_success,
# lembrete de tokens...) nunca viram bolha e, numa sessão longa, são quase todos os bytes do jsonl:
# ler só o relógio deles, sem json.loads, é o que deixa barato o /history de transcript grande.
_ATTACHMENT_EVENT_TYPES = frozenset({"queued_command", "hook_additional_context"})
_ATTACHMENT_HEAD_RE = re.compile(
    r'\{"parentUuid":(?:null|"[^"\\]*"),"isSidechain":(?:true|false),"attachment":\{"type":"([^"\\]*)"')
_ATTACHMENT_TAIL = '},"type":"attachment","uuid":"'
_ATTACHMENT_TAIL_RE = re.compile(r'\},"type":"attachment","uuid":"[^"\\]*","timestamp":"([^"\\]*)"')


def silent_attachment_timestamp(line: str) -> str | None:
    """`timestamp` de uma linha de anexo do Claude que o parse_obj descartaria, lido sem json.loads.
    None = não é esse caso (ou o formato não é o esperado): a linha segue pelo parse completo.
    Exige a linha terminada: a última, ainda sendo gravada, não pode dar relógio que o json recusaria."""
    if not line.endswith("}\n"):
        return None
    head = _ATTACHMENT_HEAD_RE.match(line)
    if not head or head.group(1) in _ATTACHMENT_EVENT_TYPES:
        return None
    # rfind: o fechamento do anexo no topo vem depois de todo o conteúdo dele, e aspas dentro de
    # string JSON são sempre escapadas, então o último casamento é o do topo.
    i = line.rfind(_ATTACHMENT_TAIL)
    tail = _ATTACHMENT_TAIL_RE.match(line, i) if i >= 0 else None
    return tail.group(1) if tail else None


def _delivery_id(value: object) -> str | None:
    # Sem terminal, a mesma entrega grava o anexo `queued_command` e o `remove`: o id comum vira uma bolha só.
    return f"delivery:{value}" if isinstance(value, str) and value else None


def parse_obj(obj: dict) -> list[ChatEvent]:
    """Eventos de chat de UMA entrada (ja parseada) do transcript. Lista pq uma entrada pode
    carregar VARIOS blocos (tool calls paralelas = varios tool_result numa msg user so; assistant
    com text + tool_use juntos) — devolver so o 1o engolia os demais silenciosamente."""
    etype = obj.get("type")
    uid = obj.get("uuid", "")

    # Fim de agente ENFILEIRADO. Quando o agente de background termina com o assistente no meio de um
    # turno, o harness NAO grava a <task-notification> como mensagem de user: grava uma entrada
    # `queue-operation`/enqueue, que nao tem `message` nem `uuid` — e morria no early-return logo
    # abaixo. Resultado: o painel de Atividade nunca recebia o sinal de termino e o agente ficava
    # "RODANDO AGORA" pra sempre (observado ao vivo: 2 de 6 agentes travados, os 2 que terminaram
    # enquanto o turno corria; os 4 que chegaram entre turnos vieram como user e fechavam certo).
    # Mesmo tool_result sintetico do caminho normal — `resulted` no fold e um Set, entao a entrega
    # posterior da mesma notificacao (quando vier) so repete, sem efeito.
    if etype == "system":
        # Prompt em entrega bloqueada por hook (ver _blocked_prompt): o texto existe no
        # transcript e o app precisa mostra-lo — mas a entrega tem preventContinuation=true: o
        # agente NUNCA recebeu o prompt. A bolha nasce marcada (desistiu=True) pra o aviso
        # vermelho "nao chegou" seguir de pe — e a verdade. id DETERMINISTICO pelo texto: as N
        # tentativas do reenvio gravam N entradas system com o MESMO texto, e o front deduplica
        # por id — uma bolha so, no lugar da 1a tentativa. ts da entrada: a bolha nasce no
        # momento da 1a tentativa, nao da ultima.
        content = obj.get("content")
        if (barrado := _blocked_prompt(content)) is not None:
            texto, erro = barrado
            digest = hashlib.md5(texto.encode("utf-8", "replace")).hexdigest()[:8]
            return [ChatEvent(kind="user_msg", id=f"held:{digest}", text=texto, ts=_ts(obj),
                              desistiu=True, hook_error=erro or None)]
        return []

    if etype == "queue-operation":
        queued = obj.get("content")
        # id pelo conteúdo: enqueue e remove da mesma entrega deduplicam no front.
        if (agente := _agent_msg(queued, "queued-agent:" + hashlib.md5(
                str(queued).encode("utf-8", "replace")).hexdigest()[:8])) is not None:
            return agente
        if (colega := _teammate_eventos(queued, "queued-teammate:" + hashlib.md5(
                str(queued).encode("utf-8", "replace")).hexdigest()[:8])) is not None:
            return colega
        if isinstance(queued, str) and queued.lstrip().startswith("<task-notification>"):
            m = _TASK_NOTIF_RE.search(queued)
            if m:
                tid = m.group(1).strip()
                # id proprio: a entrada nao tem uuid, e o front deduplica por id.
                return [ChatEvent(kind="tool_result", id=f"queued-task:{tid}",
                                  tool_use_id=f"task:{tid}", result="task-notification")]
            return []
        # Msg de usuario digitada ENQUANTO o agente trabalha: o harness enfileira (`enqueue`) e, ao
        # consumi-la DENTRO do turno em andamento, grava `remove` — nunca vira uma entrada type='user'.
        # Sem isto ela some do chat (aparece so no terminal, que conhece a fila direto). Renderiza no
        # `remove` (o consumo mid-turn): e o par EXATO das invisiveis. As que viram turno de verdade
        # saem por `dequeue` -> ja tem seu type='user' e NAO passam por aqui, entao nao duplica. id
        # pelo timestamp (a entrada nao tem uuid) pro front deduplicar por id. Mesma filtragem de meta
        # do caminho user normal (comando/skill/system-reminder/imagem sintetica nao viram bubble).
        if obj.get("operation") == "remove" and isinstance(queued, str):
            # Recado nativo consumido NO MEIO do turno: aqui nao ha `origin`, so o texto embrulhado.
            # Antes do _is_command_meta porque o embrulho nao e comando nem system-reminder — sem
            # este ramo ele passaria batido e viraria bolha com o paragrafo de instrucao a mostra.
            if (peer := _peer_msg_embrulhado(queued)) is not None:
                digest = hashlib.md5(queued.encode("utf-8", "replace")).hexdigest()[:8]
                entrega = _delivery_id(obj.get("deliveryId"))
                return [ChatEvent(kind="user_msg", text=peer, ts=_ts(obj) if entrega else None,
                                  id=entrega or f"queued:{obj.get('timestamp', '')}:{digest}")]
            if _is_command_meta(queued):
                return []
            cleaned = _strip_meta_blocks(queued)
            if not cleaned or _IMAGE_SOURCE_RE.match(cleaned):
                return []
            cleaned = _IMAGE_MARKER_RE.sub("", cleaned).strip()   # tira "[Image #N]" da legenda
            if not cleaned:
                return []
            # id = timestamp + hash do conteudo. So o timestamp NAO basta: duas msgs consumidas no
            # MESMO instante (medido: "no caso..." e "so pra..." removidas as 17:16:37) colidiriam e o
            # front, que deduplica por id, esconderia uma. Hash estavel (nao o hash() randomizado do
            # processo) pra o mesmo remove reparseado manter o id e nao duplicar na reconexao do SSE.
            digest = hashlib.md5(queued.encode("utf-8", "replace")).hexdigest()[:8]
            entrega = _delivery_id(obj.get("deliveryId"))
            # Com entrega, este evento pode substituir o do anexo no SSE: leva o próprio horário.
            return [ChatEvent(kind="user_msg", text=cleaned, ts=_ts(obj) if entrega else None,
                              id=entrega or f"queued:{obj.get('timestamp', '')}:{digest}")]
        return []

    # Claude sem terminal: a msg orientada no meio do turno (stdin com turno em voo) entra como
    # `attachment/queued_command`, não como `queue-operation remove` da TUI. O CLI a mostra ao
    # modelo dentro de um system-reminder junto do próximo tool_result; sem este ramo, ela some do
    # chat ao recarregar (só a bolha local a exibia).
    if etype == "attachment":
        att = obj.get("attachment")
        if isinstance(att, dict) and att.get("type") == "queued_command":
            texto = "\n".join(
                b.get("text", "") for b in (att.get("prompt") or [])
                if isinstance(b, dict) and b.get("type") == "text"
            ).strip()
            if not texto or _is_command_meta(texto):
                return []
            texto = _strip_meta_blocks(texto)
            if texto:
                return [ChatEvent(kind="user_msg", id=_delivery_id(att.get("delivery_id")) or uid,
                                  text=texto, ts=_ts(obj))]
        if isinstance(att, dict) and att.get("type") == "hook_additional_context" and att.get("hookEvent") == "Stop":
            # Contexto devolvido no Stop reabre o turno: sem o aviso, a resposta seguinte aparece
            # sem motivo. Só o Stop — o de UserPromptSubmit vem em todo prompt e seria ruído.
            conteudo = att.get("content")
            texto = "\n".join(c for c in conteudo if isinstance(c, str)) if isinstance(conteudo, list) else str(conteudo or "")
            if texto.strip():
                return [ChatEvent(kind="notice", id=uid, text="hook_prompt", hook_error=texto.strip(), ts=_ts(obj))]
        return []

    msg = obj.get("message")
    if not isinstance(msg, dict):
        return []
    content = msg.get("content")

    if etype == "user":
        # Entrada sintetica que o proprio Claude Code marca com isMeta: expansao de slash-command/
        # skill (o corpo do comando vira "mensagem do usuario"), prompt injetado de loop/cron,
        # "Continue from where you left off", avisos de hook. No terminal isso nao aparece; aqui
        # viraria bubble e poluiria o chat. Fora do chat. (Os <command-*> tags e a task-notification
        # NAO vem com isMeta -> seguem tratados abaixo pelo caminho de sempre.)
        # ANTES do descarte de meta: o recado nativo entre sessoes Claude vem marcado isMeta e sumiria
        # inteiro (ver _peer_msg). E conversa de verdade, nao ruido do harness.
        origem = obj.get("origin")
        if isinstance(origem, dict) and (colega := _teammate_eventos(origem.get("body"), uid)) is not None:
            return colega
        if (peer := _peer_msg(obj)) is not None:
            return [ChatEvent(kind="user_msg", id=uid, text=peer)]
        # O resumo do /compact e gravado como msg de usuario; o terminal mostra so a marca.
        if obj.get("isCompactSummary") is True:
            return [ChatEvent(kind="notice", id=uid, text="compacted")]
        if obj.get("isMeta") is True:
            return []
        primeiro_texto = (content if isinstance(content, str)
                          else (_first(content, "text") or {}).get("text") if isinstance(content, list) else None)
        if (agente := _agent_msg(primeiro_texto, uid)) is not None:
            return agente
        if (colega := _teammate_eventos(primeiro_texto, uid)) is not None:
            return colega
        if isinstance(content, str):
            if content.lstrip().startswith("<task-notification>"):
                m = _TASK_NOTIF_RE.search(content)
                if m:
                    return [ChatEvent(kind="tool_result", id=uid,
                                      tool_use_id=f"task:{m.group(1).strip()}",
                                      result="task-notification")]
                return []
            if _is_command_meta(content):
                return []
            if _INTERRUPCAO_RE.match(content.strip()):
                return [ChatEvent(kind="notice", id=uid, text="interrupted")]
            cleaned = _strip_meta_blocks(content)
            if not cleaned or _IMAGE_SOURCE_RE.match(cleaned):
                return []
            return [ChatEvent(kind="user_msg", id=uid, text=cleaned)]
        if isinstance(content, list):
            trs = [it for it in content if isinstance(it, dict) and it.get("type") == "tool_result"]
            if trs:
                out = []
                for k, tr in enumerate(trs):
                    res = tr.get("content")
                    if isinstance(res, list):
                        res = " ".join(str(b.get("text", "")) for b in res if isinstance(b, dict))
                    failed = bool(tr.get("is_error", False))
                    out.append(ChatEvent(
                        kind="tool_result", id=_sub_id(uid, k),
                        tool_use_id=tr.get("tool_use_id"),
                        result=str(res) if res is not None else None,
                        is_error=failed, ts=_ts(obj),
                        # O `toolUseResult` é um por linha: com dois resultados não dá para saber de quem é.
                        patch=_patch_hunks(obj) if len(trs) == 1 and not failed else None,
                        bg_agent_id=_bg_agent_id(obj) if len(trs) == 1 else None,
                    ))
                return out
            # Imagens coladas no terminal: contar os blocos `image` -> o front busca cada uma lazy.
            img_count = sum(1 for it in content if isinstance(it, dict) and it.get("type") == "image")
            txt = _first(content, "text")
            t = txt.get("text", "") if txt is not None else ""
            if _is_command_meta(t):
                return []
            if _INTERRUPCAO_RE.match(t.strip()):
                return [ChatEvent(kind="notice", id=uid, text="interrupted")]
            cleaned = _strip_meta_blocks(t)
            if _IMAGE_SOURCE_RE.match(cleaned):
                return []
            cleaned = _IMAGE_MARKER_RE.sub("", cleaned).strip()   # tira "[Image #N]" da legenda
            if not cleaned and not img_count:
                return []
            return [ChatEvent(kind="user_msg", id=uid, text=cleaned,
                              image_count=img_count or None)]
        return []

    # A transferência pro Codex importa resposta em texto puro; o histórico precisa mostrá-la igual.
    if etype == "assistant" and isinstance(content, str) and content.strip():
        content = [{"type": "text", "text": content}]
    if etype == "assistant" and isinstance(content, list):
        # Um evento POR BLOCO, na ordem do content (thinking etc. ignorados). Antes o 1o tool_use
        # vencia e um bloco text na mesma entrada sumia do chat.
        cache_read, ttl = _cache_info(msg)
        ts = _ts(obj)
        out = []
        for it in content:
            if not isinstance(it, dict):
                continue
            if it.get("type") == "tool_use":
                out.append(ChatEvent(
                    kind="tool_use", id=_sub_id(uid, len(out)),
                    tool_name=it.get("name"), tool_use_id=it.get("id"),
                    tool_input=it.get("input") or {}, ts=ts,
                ))
            elif it.get("type") == "text":
                out.append(ChatEvent(kind="assistant_msg", id=_sub_id(uid, len(out)),
                                     text=it.get("text", ""), ts=ts,
                                     cache_read=cache_read, cache_ttl_s=ttl))
            elif it.get("type") == "thinking":
                # Só com TEXTO. Sem `showThinkingSummaries` no settings.json o Claude Code pede o
                # pensamento `omitted`, e a API devolve o bloco cifrado: `thinking: ""` mais a
                # assinatura. Medido nesta máquina: 8746 de 8746 blocos de opus-5 vazios antes de
                # ligar a chave. Emitir os vazios encheria a conversa de linhas que não abrem nada.
                # isinstance, e nao `or ""`: campo com outro tipo (JSON malformado, formato que
                # muda) faria `.strip()` levantar AttributeError, que nenhum parse_line captura —
                # e a excecao derrubaria o tail da sessao INTEIRA, nao so esta linha. Mesmo defeito
                # que o `_peer_nome` ja teve aqui.
                pensamento = it.get("thinking")
                if isinstance(pensamento, str) and pensamento.strip():
                    out.append(ChatEvent(kind="thinking", id=_sub_id(uid, len(out)),
                                         text=pensamento, ts=ts))
        return out
    return []


def _ts(obj: dict) -> Optional[float]:
    """Epoch (segundos) do `timestamp` ISO da entrada. O campo `ts` do ChatEvent existia desde
    sempre e NUNCA era preenchido — por isso a hora nao aparecia em bubble nenhuma."""
    raw = obj.get("timestamp")
    if not isinstance(raw, str) or not raw:
        return None
    try:
        dt = datetime.fromisoformat(raw.replace("Z", "+00:00"))
    except ValueError:
        return None
    # Sem fuso no texto, .timestamp() assume o fuso LOCAL do processo -> epoch deslocado (3h aqui),
    # calado. O transcript escreve UTC; assumimos UTC em vez de herdar o fuso da maquina.
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.timestamp()


def _cache_info(msg: dict) -> tuple[Optional[int], Optional[int]]:
    """(tokens lidos do cache, TTL em segundos) do usage do turno.

    O TTL vem MEDIDO, nao suposto: `usage.cache_creation` separa `ephemeral_1h_input_tokens` de
    `ephemeral_5m_input_tokens`, entao da pra dizer qual janela a sessao esta usando. Sem esse
    detalhe (formato antigo), devolve None em vez de chutar 5min — melhor nao mostrar prazo do que
    mostrar um prazo errado."""
    usage = msg.get("usage")
    if not isinstance(usage, dict):
        return None, None
    read = usage.get("cache_read_input_tokens")
    read = int(read) if isinstance(read, (int, float)) else None
    creation = usage.get("cache_creation")
    ttl: Optional[int] = None
    if isinstance(creation, dict):
        # int() sem guarda de tipo levantava aqui com qualquer valor nao-numerico, e a excecao subia
        # ate derrubar a SSE. Como o backfill relê as ultimas linhas a cada reconexao, UMA linha
        # estranha viraria loop de queda pra aquela sessao.
        if _tok(creation.get("ephemeral_1h_input_tokens")) > 0:
            ttl = 3600
        elif _tok(creation.get("ephemeral_5m_input_tokens")) > 0:
            ttl = 300
    return read, ttl


def _tok(v: object) -> int:
    return int(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else 0


def path_in_transcript(jsonl: str | Path, needle: str) -> bool:
    """True se `needle` (um caminho de arquivo) aparece em ALGUMA linha do transcript. Trava de
    seguranca do endpoint de arquivo: so servimos arquivos CITADOS na conversa (consentidos) — nao
    leitura arbitraria de disco. Streaming com early-exit (nao carrega o jsonl inteiro)."""
    if not needle:
        return False
    try:
        with open(jsonl, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if needle in line:
                    return True
    except OSError:
        pass
    return False


def citation_cwds(jsonl: str | Path, needles: list[str], *,
                  rows: Iterable[bytes] | None = None) -> dict[str, list[str]]:
    """Caminho citado -> cwd das linhas que o citaram, do mais recente ao mais antigo."""
    wanted = {needle for needle in needles if needle}
    if not wanted:
        return {}
    pattern = re.compile("(?=(" + "|".join(re.escape(x) for x in sorted(wanted, key=len, reverse=True)) + "))")
    # Bytes só como filtro: o transcript passa de centenas de MB e cada imagem citada na tela refaz
    # esta varredura. A regex (o mais longo vence na mesma posição) decide nas poucas linhas que passam.
    encoded = [needle.encode() for needle in wanted]
    seen: set[str] = set()
    cwds: dict[str, list[str]] = {}
    try:
        with (open(jsonl, "rb") if rows is None else nullcontext(rows)) as fh:
            for raw_line in fh:
                if not any(raw in raw_line for raw in encoded):
                    continue
                line = raw_line.decode("utf-8", errors="replace")
                matched = {m.group(1) for m in pattern.finditer(line)}
                if not matched:
                    continue
                seen.update(matched)
                try:
                    cwd = json.loads(line).get("cwd")
                except (json.JSONDecodeError, AttributeError):
                    continue
                if isinstance(cwd, str) and cwd:
                    for needle in matched:
                        values = cwds.setdefault(needle, [])
                        if cwd in values:
                            values.remove(cwd)
                        values.append(cwd)
    except OSError:
        return {}
    return {needle: list(reversed(cwds.get(needle, []))) for needle in seen}


# Caracteres que não entram num caminho citado: aspas, crase e a barra invertida das sequências do JSON
# (`\"`, `\n`) delimitam a citação. Espaço entra: "Área de trabalho" é pasta comum.
_CAMINHO_CHAR = r"[^\\\n\"`'<>|*?]"


# Onde uma citação começa: depois de espaço, aspas, crase, parêntese ou das sequências `\n`/`\t` do JSON. Sem isso
# cada `/` do meio de um caminho (ou de um base64 na mesma linha) virava um começo, e um sufixo nunca citado
# (`/etc/hosts` de `/home/u/etc/hosts`) era candidato.
_CITACAO_INICIO = r"(?:(?<=[\s\"`'(\[=:,])|(?<=\\n)|(?<=\\t)|^)"
# Fim da citação: `x.sql` não casa em `x.sql.bak` nem em `x.sql2`, mas casa com o ponto final da frase.
_CITACAO_FIM = r"(?![\w-]|\.\w)"
_CAMINHO_MAX = 400


def cited_elsewhere(jsonl: str | Path, path: str, *,
                    rows: Iterable[bytes] | None = None) -> tuple[list[str], list[str]]:
    """Numa leitura só do transcript, onde mais a conversa citou o arquivo `path` (nome solto ou relativo), do mais
    recente ao mais antigo: absolutos que terminam em `/path` e existem, e relativos citados que terminam no nome
    (um `git status` de outro repositório cita `docs/x/nome` sem dizer de onde)."""
    tail = path.replace("\\", "/").removeprefix("./").strip("/")
    if not tail or ".." in tail.split("/"):
        return [], []
    name = tail.rsplit("/", 1)[-1]
    absolute = re.compile(_CITACAO_INICIO + "(/" + _CAMINHO_CHAR + "{0," + str(_CAMINHO_MAX) + "}?/"
                          + re.escape(tail) + ")" + _CITACAO_FIM)
    relative = re.compile(r"(?<![\w./-])((?:[\w.-]+/)+" + re.escape(name) + ")" + _CITACAO_FIM)
    needle = name.encode()
    absolutes: list[str] = []
    relatives: list[str] = []
    try:
        with (open(jsonl, "rb") if rows is None else nullcontext(rows)) as fh:
            for raw_line in fh:
                if needle not in raw_line:
                    continue
                line = raw_line.decode("utf-8", errors="replace")
                absolutes.extend(absolute.findall(line))
                if "/" not in tail:
                    relatives.extend(relative.findall(line))
    except OSError:
        _log.warning("transcript ilegível ao procurar %s citado: %s", tail, jsonl, exc_info=True)
        return [], []
    found = [c for c in list(dict.fromkeys(reversed(absolutes)))[:50] if os.path.isfile(c)]
    return found, [p for p in dict.fromkeys(reversed(relatives)) if ".." not in p.split("/")][:20]


def last_assistant_text(jsonl: str | Path) -> Optional[str]:
    """Texto do ULTIMO evento de assistant do transcript (modo done_claimed do loop procura
    'LOOP_DONE' aqui). Streaming linha a linha (padrao path_in_transcript); None se ausente."""
    last: Optional[str] = None
    try:
        with open(jsonl, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                for ev in parse_line(line):
                    if ev.kind == "assistant_msg" and ev.text:
                        last = ev.text
    except OSError:
        return None
    return last


def get_transcript_image(jsonl: str | Path, uuid: str, idx: int) -> Optional[tuple[bytes, str]]:
    """Bytes + media_type da idx-ésima imagem base64 da msg de uuid no transcript, ou None.

    Fonte das imagens coladas no terminal (a image-cache do Claude não persiste). Serve sob demanda
    pra não inchar o payload do histórico/SSE com base64."""
    try:
        fh = Path(jsonl).open("rb")
    except OSError:
        return None
    needle = uuid.encode()
    with fh:  # streaming linha-a-linha: nao carrega o transcript inteiro (dezenas de MB) em RAM
        for line in fh:
            # Só a linha que contém o uuid vira JSON: decodificar todas custava segundos por imagem.
            if needle not in line:
                continue
            try:
                obj = json.loads(line.decode("utf-8", errors="replace"))
            except (json.JSONDecodeError, ValueError):
                continue
            if obj.get("uuid") != uuid:
                continue
            content = (obj.get("message") or {}).get("content")
            if not isinstance(content, list):
                return None
            imgs = [it for it in content if isinstance(it, dict) and it.get("type") == "image"]
            if idx < 0 or idx >= len(imgs):
                return None
            src = imgs[idx].get("source") or {}
            data = src.get("data")
            if not isinstance(data, str):
                return None
            try:
                raw = base64.b64decode(data)
            except (ValueError, base64.binascii.Error):
                return None
            media = src.get("media_type") if isinstance(src.get("media_type"), str) else "image/png"
            return raw, media
    return None


class TranscriptTailer:
    def __init__(self, path: str | Path, parse_line=parse_line):
        self.path = Path(path)
        # Parser injetavel: default e o parse_line do Claude (snake_case), mas o CodexAdapter
        # reaproveita a mesma mecanica de tail (backfill + watch de append) passando
        # parse_rollout_line (shape do rollout do Codex e diferente).
        # So o parser do Claude ganha o filtro de reescrita do --resume; embrulhar o do Pi
        # esconderia o `flush_events` que _read_from procura no dono do parser.
        self._parse_line = RewriteFilter().parse_line if parse_line is globals()["parse_line"] else parse_line

    def _read_from(self, pos: int) -> tuple[list[ChatEvent], int]:
        # Le do offset `pos` ate o fim -> (eventos parseados, novo offset). Sincrono de proposito:
        # chamado via asyncio.to_thread no follow() pra nao bloquear o event loop com I/O de arquivo
        # (o backfill inicial le o transcript inteiro, que cresce pra dezenas de MB em sessao longa).
        # Binario: tell()/seek() em modo texto sao cookies opacos (nao offsets em bytes) -> nao
        # daria pra comparar com st_size no guard de truncamento; o decode fica por linha lida.
        if not self.path.exists():
            return [], pos
        evs: list[ChatEvent] = []
        with self.path.open("rb") as fh:
            if os.fstat(fh.fileno()).st_size < pos:
                # arquivo ENCOLHEU (truncado/reescrito): o offset antigo cairia alem do EOF e a
                # leitura retomaria no meio de linha nova = lixo/eventos perdidos. Recomeca do
                # zero (o arquivo pos-truncamento e pequeno; o front deduplica por id).
                pos = 0
            fh.seek(pos)
            start = pos
            while True:
                start = fh.tell()
                line = fh.readline()
                if not line:
                    break
                if not line.endswith(b"\n"):
                    # awatch disparou no meio de um append -> linha incompleta. Rebobina pro inicio
                    # dela e nao avanca pos: a versao COMPLETA e relida no proximo evento do watcher.
                    fh.seek(start)
                    break
                parsed = self._parse_line(line.decode("utf-8", "replace"))
                for ev in parsed:
                    # Offset do INICIO da linha, nao do fim: uma linha pode render VARIOS eventos e
                    # eles compartilham o id. Se o cliente recebeu so o 1o e reconectou, retomar
                    # pelo fim PULARIA os irmaos. Pelo inicio a linha e relida inteira e o front
                    # descarta o que ja tem (dedup por ev.id) -- sobreposicao barata, perda zero.
                    ev.offset = start
                evs.extend(parsed)
            # Parser com memoria (o do Pi segura o user_msg por uma linha, ver adapters/pi/
            # transcript.Stream): o que ficou retido sai no fim do LOTE, nao na proxima leitura —
            # senao a mensagem do usuario so apareceria quando o Pi gravasse a linha seguinte, que
            # num turno longo demora minutos.
            owner = getattr(self._parse_line, "__self__", None)
            if hasattr(owner, "flush_events"):
                evs.extend(self._flush_com_espera(fh, owner, start))
            return evs, fh.tell()

    # Espera curta antes de soltar o que o parser reteve. As duas linhas do par (mensagem do
    # usuario + o marcador do hook que diz o que nela e do hook) nascem no MESMO milissegundo, mas
    # nada garante que o watcher acorde depois das duas: se o lote fechar entre elas, a bolha sai
    # sem o corte e o marcador chega orfao no lote seguinte — volta calada ao bug antigo. Uma
    # releitura curta fecha essa fresta sem atrasar nada perceptivel, e so roda quando ha algo
    # retido (Claude/Codex nunca retem). ponytail: 200ms e o knob — medido 1ms entre as duas
    # escritas do Pi, entao a folga e de duas ordens de grandeza.
    _ESPERA_PAR_S = 0.2

    def _flush_com_espera(self, fh, owner, start: int) -> list[ChatEvent]:
        """Releitura curta e depois solta o retido. Devolve (linhas novas + o que sobrou retido)."""
        if not owner.tem_retido():
            return owner.flush_events()          # nada preso: sem espera nenhuma
        time.sleep(self._ESPERA_PAR_S)
        novos: list[ChatEvent] = []
        while True:
            ini = fh.tell()
            line = fh.readline()
            if not line or not line.endswith(b"\n"):
                fh.seek(ini)                     # EOF ou linha pela metade: fica pro proximo ciclo
                break
            for ev in self._parse_line(line.decode("utf-8", "replace")):
                ev.offset = ini
                novos.append(ev)
            start = ini
        held = owner.flush_events()
        for ev in held:
            ev.offset = start        # inicio da ULTIMA linha lida (a que segurou o release)
        return novos + held

    def _size(self) -> int:
        try:
            return self.path.stat().st_size
        except OSError:
            return 0

    def _tail_offset(self, max_lines: int) -> int:
        # Offset do inicio da (max_lines)-esima linha a partir do fim -> o follow() faz backfill so do
        # tail. Le do FIM pra tras (mesmo desenho do _tail_offset do pqueue): varrer pra frente
        # custava o arquivo inteiro -- 136MB lidos pra pular pros ultimos ~500KB, em todo connect de
        # SSE sem Last-Event-ID. <= max_lines linhas, arquivo vazio ou ausente -> 0 (backfill do
        # inicio = comportamento antigo).
        #
        # Conta so `\n`: a linha completa k comeca depois do k-esimo `\n`, entao o inicio da
        # max_lines-esima a partir do fim fica logo apos o (max_lines+1)-esimo `\n` contado de tras
        # pra frente. Cauda sem `\n` (append em voo) nao entra na conta nem desloca nada, igual antes.
        try:
            with self.path.open("rb") as fh:
                size = fh.seek(0, os.SEEK_END)
                window = _TAIL_WINDOW
                while True:
                    start = max(0, size - window)
                    fh.seek(start)
                    buf = fh.read(size - start)
                    if buf.count(b"\n") > max_lines:
                        idx = len(buf)
                        for _ in range(max_lines + 1):
                            idx = buf.rindex(b"\n", 0, idx)
                        return start + idx + 1
                    if start == 0:
                        return 0     # arquivo inteiro na janela e ainda nao deu max_lines linhas
                    window *= 4      # janela curta (ou uma linha gigante, base64 de imagem): cresce
        except OSError:
            return 0

    async def follow(self, start_offset: int | None = None) -> AsyncIterator[ChatEvent]:
        """Backfill + watch de append. `start_offset` (do Last-Event-ID) retoma EXATAMENTE dali.

        Sem ele, backfill so do TAIL (ultimas _BACKFILL_LINES linhas). Essa janela cobre ~2 min de
        trabalho pesado (medido: mediana 44 linhas/min, pico 133) -- uma queda de celular mais longa
        que isso perdia o miolo do buraco. Com o offset o resume e exato e barato (nao reenvia 200
        linhas a cada reconexao). Offset invalido (arquivo trocado/truncado) cai no tail de sempre.
        """
        if start_offset is not None:
            size = await asyncio.to_thread(self._size)
            # Alem do EOF = transcript trocado ou truncado sob o cliente -> o offset nao significa
            # mais nada. Volta pro tail em vez de retomar no lugar errado (ou reler o arquivo todo).
            pos = start_offset if 0 <= start_offset <= size else None
        else:
            pos = None
        if pos is None:
            pos = await asyncio.to_thread(self._tail_offset, _BACKFILL_LINES)
        # backfill inicial + cada append: a leitura de arquivo roda no threadpool (nao bloqueia o loop).
        evs, pos = await asyncio.to_thread(self._read_from, pos)
        for ev in evs:
            yield ev
        # yield_on_timeout: alem dos eventos do FS, acorda a cada rust_timeout mesmo sem mudanca
        # (changes vazio) e rele -> fecha a janela morta entre o backfill acima e o watcher armar
        # (evento gravado nesse gap so apareceria no proximo write) e cobre inotify perdido.
        # A pasta do transcript pode NAO existir ainda (sessao recem-criada, agente ainda bootando):
        # ali o awatch levanta FileNotFoundError, o pump do sse manda o erro pro cliente, o
        # EventSource reconecta e cai no mesmo erro — laco de "reconectando" com o chat mudo. Espera
        # a pasta nascer em vez de estourar.
        esperas = 0
        while True:
            if not await asyncio.to_thread(self.path.parent.is_dir):
                esperas += 1
                # Espera sem teto e sem log seria chat mudo pra sempre quando a pasta nunca nasce
                # (bug noutro lugar): avisa UMA vez, como o _warn_ready_timeout_once do envio.
                if esperas == _AVISA_ESPERA_PASTA:
                    _log.warning("transcript %s: a pasta %s nao existe ha %.0fs — seguindo em "
                                 "espera; se a sessao esta viva, o chat fica mudo ate ela nascer",
                                 self.path.name, self.path.parent,
                                 esperas * _ESPERA_PASTA_S)
                await asyncio.sleep(_ESPERA_PASTA_S)
                continue
            try:
                # recursive=False: o transcript mora direto na pasta; as subpastas
                # (<uuid>/subagents/) so acordavam o watch a toa a cada escrita de subagente.
                async for changes in awatch(self.path.parent, yield_on_timeout=True,
                                            rust_timeout=5000, recursive=False):
                    # O watch e do DIRETORIO (o proprio arquivo pode nem existir ainda), mas escrita de
                    # jsonl IRMAO (ex: subagente gravando o proprio transcript ao lado) acordava todos os
                    # tailers -> so rele quando o toque e no NOSSO arquivo (ou no timeout do heartbeat).
                    if changes and not any(Path(p).name == self.path.name for _, p in changes):
                        continue
                    evs, pos = await asyncio.to_thread(self._read_from, pos)
                    for ev in evs:
                        yield ev
            except FileNotFoundError:
                # So engole quando a pasta REALMENTE sumiu. Com ela no lugar o erro veio de outra
                # coisa (o _read_from tambem roda dentro deste try) e tem que subir pro pump do sse,
                # como antes — senao o chat fica mudo sem uma linha de log.
                if await asyncio.to_thread(self.path.parent.is_dir):
                    raise
                _log.warning("transcript %s: a pasta %s sumiu debaixo do watch — esperando ela "
                             "voltar", self.path.name, self.path.parent)
                continue


from app.git_ops import GitError as _WorkspaceError
from app.workspace_bridge import delegate as _workspace_delegate, text_rows as _text_rows

citation_cwds = _workspace_delegate("citation_cwds", _WorkspaceError, prepare=_text_rows)(citation_cwds)
cited_elsewhere = _workspace_delegate("cited_elsewhere", _WorkspaceError, prepare=_text_rows)(cited_elsewhere)
