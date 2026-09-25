"""Claude SEM terminal: o `claude` como processo filho do backend, falando stream-json.

Sem pane, sem tmux, sem raspar tela. O que muda em relação à sessão Claude comum é só a FONTE
do que é vivo: estado, prévia, permissão, pergunta e statusline vêm do stdout do processo, e a
entrada vai pelo stdin. O histórico continua no `.jsonl` que o próprio Claude grava, lido pelo
mesmo parser (`TranscriptTailer`) das sessões no tmux — bolhas, tool cards, thinking, imagens,
`/history`, estatísticas e o `reset` do `/clear` não sabem que não há pane.

Protocolo (medido contra a CLI, ver docs/research/claude-sem-terminal-monocode.md):
- stdin: `{"type":"user","message":{...}}` manda prompt; `{"type":"control_request",...}` com
  `initialize`, `interrupt`, `set_model`, `set_permission_mode`, `list_models`; e
  `{"type":"control_response",...}` responde permissão/pergunta.
- stdout: `system/init` (session_id, modelo, modo), `stream_event` (deltas), `assistant`,
  `user` (tool_result), `control_request can_use_tool` (permissão e AskUserQuestion), `result`
  (fim de turno, custo, uso), `rate_limit_event`, `conversation_reset` (/clear).

O processo morre com o backend; o próximo prompt sobe outro com `--resume <sid>`. Tudo que
precisa sobreviver está no sidecar (sessions.py).
"""
from __future__ import annotations

import asyncio
import base64
import collections
import hashlib
import json
import logging
import os
import re
import shutil
import signal
import sys
import time
import uuid
from pathlib import Path
from typing import AsyncIterator, Callable, Optional

from app import atomico, cotas, log_paths, model_args, pensamento, runtime_config
from app.adapters.claude_headless import cano as cano_mod
from app.adapters.claude_headless import sessions as hl_sessions
from app.adapters.codex.adapter import _fmt_tok, _format_reset
from app.adapters.preview_push import PushPreviewSource, fonte_ferramenta, fonte_pensamento
from app.config import settings
from app.pqueue import PromptQueue
from app.procinfo import pid_vivo
from app.state import StateEvent
from app.transcript import ChatEvent, TranscriptTailer

_log = logging.getLogger("hangar.claude_headless")

# Mesma regra de app.registry.sanitize_cwd (duplicada pelo mesmo motivo do adapters/claude.py).

# Chave interna do adapter no registro de providers. O `provider` da sessão continua "claude"
# (é Claude para o front, comandos, estatísticas e cotas); só o transporte é outro.
CHAVE = "claude-headless"

# Os hooks de SessionStart rodam antes do initialize responder, e com muitos plugins passam de 20s.
# Até o aviso a sessão só aparece "Iniciando…"; depois dele o problema fica à vista, mas a espera
# segue até o teto — resposta tardia limpa o problema.
_AVISO_INIT_S = 60.0
_TETO_INIT_S = 180.0
_TETO_CTRL_S = 15.0
_TETO_CANO_S = 10.0        # do spawn do cano até ele escutar
# Sessão parada há mais que isto sem nada em aberto tem o processo encerrado; o próximo prompt
# sobe outro com --resume. Fica acima da janela de 1h do cache do prompt: passado dela o próximo
# turno relê o contexto de qualquer jeito, e religar não custa cota a mais.
_OCIOSA_S = 65 * 60
_VIGIA_S = 60.0
# Subidas seguidas sem `initialize` bom: o drain do fim da subida chama a próxima, então sem teto
# um processo que morre ao nascer vira laço. A espera dobra a cada tentativa.
_TETO_SUBIDAS = 3
_ESPERA_SUBIDA_S = 5.0
# Evento que o adapter não conhece vai pro log privado pra decidir depois o que fazer com ele. O
# teto por tipo é pra ver todos os estados de um evento sem um tipo ruidoso encher o disco.
_TETO_DESCONHECIDOS = 30
_MAX_DESCONHECIDOS_B = 10 << 20
_LIMITE_LINHA = 16 << 20   # uma linha do stream-json (initialize responde >100 KB)
# Env do cano (e do claude, que herda): a chave do sidecar. É por ela que a varredura de órfãos
# distingue "cano de sessão viva" de "cano cuja sessão foi encerrada com o backend fora".
_MARCADOR_CANO = "HANGAR_CANO_KEY"
_CANO_PY = Path(__file__).with_name("cano.py")
# Valores literais, não `subprocess.CREATE_*`: os atributos só existem no Windows (ver atualizar.py).
_FLAGS_WINDOWS = 0x00000200 | 0x08000000   # CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW

OPCOES_PERMISSAO = ["Permitir", "Negar"]
# 3ª opção só quando a CLI mandou `permission_suggestions` (a regra que a TUI ofereceria como
# "sempre permitir"); a resposta leva as regras em `updatedPermissions` e a CLI grava no settings.
OPCAO_SEMPRE = "Sempre permitir"
# `ExitPlanMode` pendente é a aprovação do plano, não uma permissão de ferramenta: o texto do plano
# vai no estado, e negar mantém a sessão planejando.
PERGUNTA_PLANO = "Aprovar o plano?"
OPCOES_PLANO = ["Aprovar plano", "Continuar planejando"]
_RECUSA_PLANO = "O usuário não aprovou o plano. Continue no modo plano e aguarde as instruções dele."


def respostas_do_app(perguntas: list[dict], answers: list[dict]) -> tuple[dict[str, str], list[str]]:
    """Respostas do stepper do app viradas no mapa pergunta→texto que o AskUserQuestion espera.

    O segundo valor são as perguntas deixadas em "Conversar sobre isso". ValueError = faltou resposta."""
    respostas: dict[str, str] = {}
    conversar: list[str] = []
    for i, item in enumerate(perguntas):
        a = answers[i] if i < len(answers) else None
        if not a:
            raise ValueError("responda a todas as perguntas")
        if a.get("kind") == "chat":
            # "Conversar sobre isso": sem picker pra fechar, o equivalente é recusar a tool com
            # as respostas que já existem no texto — senão viravam 409 e a sessão ficava presa.
            conversar.append(item["question"])
            continue
        if a.get("kind") == "text":
            texto = (a.get("value") or "").strip()
        else:
            opcoes = item.get("options") or []
            idx = a.get("indices") or []
            rotulos = a.get("labels") or [opcoes[j]["label"] for j in idx if 0 <= j < len(opcoes)]
            texto = ", ".join(rotulos)
        if not texto:
            raise ValueError("responda a todas as perguntas")
        respostas[item["question"]] = texto
    return respostas, conversar


def _mensagem_conversar(respostas: dict[str, str], conversar: list[str]) -> str:
    """Tool_result do AskUserQuestion quando alguma pergunta ficou em "Conversar sobre isso".

    A recusa vem na PRIMEIRA linha: é ela que a tela mostra resumida, em vermelho de tool recusada.
    """
    if len(conversar) == 1:
        linhas = [f"Sobre «{conversar[0]}» ele prefere conversar antes de responder."]
    else:
        linhas = ["Sobre estas perguntas ele prefere conversar antes de responder:"]
        linhas.extend(f"- {p}" for p in conversar)
    if respostas:
        linhas.append("Ele já respondeu:")
        linhas.extend(f"- {p} → {r}" for p, r in respostas.items())
    linhas.append("Não repita a pergunta: responda em texto e aguarde a mensagem dele.")
    return "\n".join(linhas)


class _CanoOcupado(RuntimeError):
    """Cano vivo que não respondeu: há outro cliente nele. Não se mata nem se substitui."""


class _SubidaEsgotada(RuntimeError):
    """A sessão já falhou ao subir `_TETO_SUBIDAS` vezes seguidas; só ação do usuário tenta de novo."""


class _Ligacao:
    """Conexão com o cano — o que o adapter antes chamava de processo. Mesma forma (stdin,
    stdout, pid, returncode, wait) pra o resto do adapter não saber que há um socket no meio."""

    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter, pid: int | None):
        self.stdout = reader
        self.stdin = writer
        self.pid = pid
        self.returncode: int | None = None
        self._fim = asyncio.Event()

    def saiu(self, rc: int | None) -> None:
        self.returncode = rc
        self._fim.set()

    async def wait(self) -> int | None:
        await self._fim.wait()
        return self.returncode


class _Sessao:
    def __init__(self, name: str, meta: dict):
        self.name = name
        self.meta = meta
        self.proc: _Ligacao | None = None
        self.leitor: asyncio.Task | None = None
        self.desligando = False    # backend saindo: fecha a conexão, o cano continua
        self.state = "idle"
        self.label: str | None = None
        self.in_progress = False
        self.model: str | None = meta.get("model")
        self.effort: str | None = meta.get("effort")
        self.permission_mode: str | None = meta.get("permission_mode")
        # Último modo que não era `plan`: é pra onde "Implementar o plano" volta.
        self.modo_nao_plan: str | None = meta.get("previous_non_plan")
        # Plano aprovado esperando a CLI sair do plano: o modo que deve ser reaplicado depois.
        self.base_apos_plano: str | None = None
        self.context_window: int | None = meta.get("context_window")
        self.usage: dict | None = None
        self.cost: float | None = None
        self.limited = False
        self.limit_reset: str | None = None
        # Pedidos de permissão em aberto, na ordem em que chegaram: request_id -> request.
        self.pending: dict[str, dict] = {}
        self.question: dict | None = None      # AskUserQuestion pendente (payload pro front)
        self.previa = ""
        self.pensamento = ""       # resumo do raciocínio em voo (só chega com --thinking-display)
        self.version = 0
        self.cond = asyncio.Condition()
        self.waiters: dict[str, asyncio.Future] = {}
        self.n_req = 0
        self.initialized = asyncio.Event()
        # Código + detalhe do último problema (turno com erro, processo caiu, sem resposta):
        # vai pro StateEvent e pro card. Limpa quando um turno fecha bem.
        gravado = meta.get("problema") or (None, None)   # do sidecar: sobrevive ao restart
        self.problema: str | None = gravado[0]
        self.problema_detalhe: str | None = gravado[1]
        self.stderr_tail: collections.deque[str] = collections.deque(maxlen=20)
        self.linhas_ruins = 0
        self.loop: asyncio.AbstractEventLoop | None = None
        self.encerrando = False    # SIGTERM nosso: sair não é "caiu"
        # Janelas de cota da CONTA (⚡5h/📅7d), lidas pelo mesmo leitor da faixa de contas — o
        # stream só diz "allowed" e o reset, não o percentual.
        self.janelas: list = []
        self.janelas_ts = 0.0
        self.drenador: asyncio.Task | None = None   # referência viva do drain de fim de turno
        self.tarefas: dict[str, dict] = {}          # subagentes em voo: task_id -> {tipo, passo}
        self.effort_pendente: str | None = None     # `/effort` pedido com turno em voo: sai no result
        self.effort_aguardando: str | None = None   # `/effort` já no stdin, esperando a CLI confirmar
        self.tipos_desconhecidos: set[str] = set()  # eventos do stdout já avisados (uma nota por tipo)
        self.desconhecidos_gravados: collections.Counter[str] = collections.Counter()
        self.iniciando = False     # processo novo esperando o `initialize` (hooks de SessionStart)
        # Contadores do turno em voo, pro rótulo "(7s · ↓ 334 tokens · thought for 2s)" da TUI.
        self.turno_inicio: float | None = None
        self.tokens_fechados = 0      # output_tokens das mensagens já fechadas do turno
        self.tokens_msg: int | None = None   # output_tokens real da mensagem em voo (message_delta)
        self.tokens_msg_chars = 0     # caracteres da mensagem em voo, até o real chegar
        self.pensando_desde: float | None = None
        self.pensou_s = 0.0
        self.compactando = False
        self.ativa_em = time.monotonic()   # último evento da CLI ou prompt nosso (estacionar)
        # Input da tool em voo (partial_json acumulado): o rótulo mostra o alvo antes dela rodar.
        self.tool_nome: str | None = None
        self.tool_json = ""
        # Lista do `/` vinda da própria CLI: nomes+descrição do initialize; os só-de-TUI do init.
        self.comandos: list[dict] | None = None
        self.comandos_terminal: frozenset[str] = frozenset()

    def iniciar_turno(self) -> None:
        self.turno_inicio = time.monotonic()
        self.tokens_fechados = self.tokens_msg_chars = 0
        self.tokens_msg = self.pensando_desde = None
        self.pensou_s = 0.0
        self.compactando = False

    def fechar_mensagem(self) -> None:
        self.tokens_fechados += self._tokens_da_mensagem()
        self.tokens_msg, self.tokens_msg_chars = None, 0

    def _tokens_da_mensagem(self) -> int:
        return self.tokens_msg if self.tokens_msg is not None else self.tokens_msg_chars // 4

    def rotulo_turno(self) -> str | None:
        if self.turno_inicio is None:
            return None
        agora = time.monotonic()
        seg = int(agora - self.turno_inicio)
        partes = [f"{seg // 60}m {seg % 60}s" if seg >= 60 else f"{seg}s"]
        tokens = self.tokens_fechados + self._tokens_da_mensagem()
        if tokens:
            partes.append(f"↓ {tokens / 1000:.1f}k tokens" if tokens >= 1000 else f"↓ {tokens} tokens")
        pensou = self.pensou_s + (agora - self.pensando_desde if self.pensando_desde is not None else 0)
        if pensou >= 1:
            partes.append(f"thought for {int(pensou)}s")
        return f"({' · '.join(partes)})"

    @property
    def sid(self) -> str:
        return self.meta["session_id"]

    @property
    def vivo(self) -> bool:
        return self.proc is not None and self.proc.returncode is None


class ClaudeHeadlessAdapter:
    provider = "claude"

    def __init__(self) -> None:
        self._sessions: dict[str, _Sessao] = {}
        self._delivery_locks: dict[str, asyncio.Lock] = {}
        # Problema da última vida do processo, por nome: a sessão sai de `_sessions` quando o
        # processo morre, e o card/chat ainda precisam dizer por quê.
        self._problemas: dict[str, tuple[str, str | None]] = {}
        self._problemas_lidos: set[str] = set()     # nomes cujo problema do sidecar já foi lido
        self._spawn_locks: dict[str, asyncio.Lock] = {}
        self._tarefas: set[asyncio.Task] = set()
        self._religadas: dict[str, float] = {}
        self._vigia: asyncio.Task | None = None
        self._subidas: dict[str, int] = {}   # subidas seguidas sem initialize bom, por nome
        self.apos_entrega: Callable[[str], None] | None = None   # api agenda a confirmação da fila

    # ── contrato Adapter ────────────────────────────────────────────────────────────────────

    def transcript_stream(self, path: str, start_offset: int | None = None) -> AsyncIterator[ChatEvent]:
        Path(path).parent.mkdir(parents=True, exist_ok=True)
        return TranscriptTailer(path).follow(start_offset)

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]:
        return self._state_stream(name)

    def spawn_command(self, cwd: str, session_id: str,
                      model: str | None = None, effort: str | None = None,
                      permission_mode: str | None = None) -> list[str]:
        return self._argv(session_id, resume=False, model=model, effort=effort, permission_mode=permission_mode)

    def transcript_path(self, cwd: str, session_id: str, config_dir: str | None = None) -> str:
        base = (Path(config_dir) / "projects") if config_dir else Path(settings.projects_dir)
        from app.registry import sanitize_cwd   # local: registry importa os adapters
        esperado = base / sanitize_cwd(cwd) / f"{session_id}.jsonl"
        if esperado.exists():
            return str(esperado)
        # EnterWorktree move o transcript pra pasta do cwd da worktree; o sid não repete entre pastas.
        movido = next(base.glob(f"*/{session_id}.jsonl"), None)
        return str(movido or esperado)

    def transcript_path_de(self, meta: dict) -> str:
        return self.transcript_path(meta["cwd"], meta["session_id"], meta.get("config_dir"))

    def delivery_lock(self, name: str) -> asyncio.Lock:
        return self._delivery_locks.setdefault(name, asyncio.Lock())

    async def deliverable(self, name: str) -> bool:
        # Parada ou subindo: o prompt vai pra fila e a resposta HTTP sai na hora. Quem sobe é o
        # `acordar`, e quem entrega é o fim do `initialize` — nunca o POST esperando os hooks.
        sess = self._sessions.get(name)
        if sess is None or not sess.vivo:
            return False
        return not (sess.iniciando or sess.in_progress or sess.pending or sess.question)

    def acordar(self, name: str) -> None:
        """Sobe (ou religa) a sessão em segundo plano e entrega a fila quando ela estiver pronta."""
        self._subidas.pop(name, None)   # ação do usuário: nova rodada de tentativas
        sess = self._sessions.get(name)
        if sess is not None and sess.vivo:
            return
        t = asyncio.get_running_loop().create_task(self._acordar(name))
        self._tarefas.add(t)
        t.add_done_callback(self._tarefas.discard)

    async def _acordar(self, name: str) -> None:
        try:
            sess = await self.ensure_running(name, esperar_pronta=False)
        except Exception:
            return   # ensure_running já registrou o problema que a tela mostra
        if sess is not None and not sess.iniciando:
            # Religou num cano vivo (sem initialize a esperar): ninguém mais drenaria a fila.
            await self._drenar_fim_de_turno(sess)

    async def send_prompt(self, name: str, text: str) -> str:
        sess = await self.ensure_running(name, esperar_pronta=False)
        if sess is not None:
            # Antes de qualquer outra espera: a vigia confere isto logo antes de encerrar.
            sess.ativa_em = time.monotonic()
        if sess is None or not await self.deliverable(name):
            return "deferred"
        try:
            await self._escrever_prompt(sess, text)
        except Exception:
            _log.exception("claude headless: escrita no stdin falhou name=%s", name)
            return "deferred"
        sess.in_progress = True
        sess.state = "working"
        sess.label = None
        sess.ativa_em = time.monotonic()
        sess.iniciar_turno()
        await self._notify(sess)
        if self.apos_entrega is not None:
            self.apos_entrega(name)
        return "sent"

    async def _escrever_prompt(self, sess: _Sessao, text: str) -> None:
        blocos, avisos = await asyncio.to_thread(_blocos_do_prompt, text)
        await self._write(sess, {
            "type": "user", "session_id": "", "parent_tool_use_id": None,
            "message": {"role": "user", "content": blocos},
        })
        for aviso in avisos:
            await self._nota_local(sess, aviso)

    async def drain(self, name: str, path: str) -> int:
        async with self.delivery_lock(name):
            q = PromptQueue(name)
            if not any(e.get("delivered") is False for e in await asyncio.to_thread(q.load)):
                return 0
            sent = 0
            while True:
                claimed = await asyncio.to_thread(q.claim_undelivered, limit=1)
                if not claimed:
                    return sent
                entry = claimed[0]
                try:
                    result = await self.send_prompt(name, entry["text"])
                except _SubidaEsgotada:
                    # Fica entregue-e-desistida: a bolha avisa que não chegou e o drain não a pega mais.
                    await asyncio.to_thread(q.desistir, entry["id"])
                    return sent
                except Exception:
                    _log.exception("claude headless drain: falha entry=%s name=%s", entry.get("id"), name)
                    result = "deferred"
                if result != "sent":
                    # claim_undelivered marcou entregue de forma otimista; nada saiu, reverte.
                    try:
                        await asyncio.to_thread(q.set_delivered, entry["id"], False)
                    except OSError:
                        pass
                    return sent
                sent += 1

    async def steer(self, name: str, text: str) -> None:
        """Mensagem no MEIO do turno: a CLI aceita `user` com turno em voo e injeta no próximo
        passo (medido no MonoCode e na sonda). Sem turno em voo é um envio comum. Sob a mesma
        trava de entrega do /input e do drain — todo escritor do stdin passa por ela."""
        async with self.delivery_lock(name):
            sess = await self.ensure_running(name)
            if sess is None:
                raise RuntimeError("sessão indisponível")
            await self._steer_vivo(sess, text)

    async def _steer_vivo(self, sess: _Sessao, text: str) -> None:
        # Só no processo que está aí: subir outro "pra orientar" seria começar outra conversa.
        if not sess.vivo:
            raise RuntimeError("o processo encerrou antes de receber a mensagem")
        await self._escrever_prompt(sess, text)
        if not sess.in_progress:
            sess.in_progress = True
            sess.state = "working"
            await self._notify(sess)

    async def steer_queue(self, name: str, *, entry_id: str | None = None) -> list[str]:
        """Promove a fila durável pro turno em curso (o "mandar agora" do chip). Devolve os ids
        entregues. Sem turno em voo não há o que promover: o drain normal entrega."""
        async with self.delivery_lock(name):
            sess = self._sessions.get(name)
            if sess is None or not sess.vivo or not sess.in_progress:
                raise RuntimeError("Não há turno em andamento para orientar")
            if sess.pending or sess.question:
                # Parada numa permissão/pergunta a CLI não lê o stdin de mensagens: a fila iria
                # sumir da tela e ficar invisível até alguém responder. Melhor dizer.
                raise RuntimeError("Responda a permissão ou pergunta pendente antes de orientar")
            q = PromptQueue(name)
            sent: list[str] = []
            while claimed := await asyncio.to_thread(q.claim_undelivered, limit=1, entry_id=entry_id):
                entry = claimed[0]
                try:
                    await self._steer_vivo(sess, entry["text"])
                except BaseException:
                    await asyncio.to_thread(q.set_delivered, entry["id"], False)
                    raise
                sent.append(entry["id"])
                try:
                    await asyncio.to_thread(q.set_delivered, entry["id"], True, steered=True)
                except OSError:
                    _log.exception("claude headless: orientação aceita, recibo falhou name=%s entry=%s", name, entry["id"])
                    return sent
            return sent

    # ── controles ───────────────────────────────────────────────────────────────────────────

    async def interrupt(self, name: str) -> bool:
        sess = self._sessions.get(name)
        if sess is None or not sess.vivo or not (sess.in_progress or sess.pending or sess.question):
            return False
        sess.base_apos_plano = None
        # Pedido pendente some junto com o turno: negar antes evita a tool rodar depois do Esc.
        for rid in list(sess.pending):
            await self._responder(sess, rid, {"behavior": "deny", "message": "Interrompido pelo usuário."})
        sess.pending.clear()   # já respondido: um cancel da CLI depois disto não é "decisão de hook"
        if sess.question:
            await self._responder(sess, sess.question["request_id"],
                                  {"behavior": "deny", "message": "Interrompido pelo usuário."})
            sess.question = None
        try:
            await self._ctrl(sess, "interrupt", esperar=False)
        except Exception:
            _log.exception("claude headless: interrupt falhou name=%s", name)
            return False
        return True

    async def select(self, name: str, option: int) -> bool:
        """Resposta ao pedido de permissão em aberto: 1 = permitir, 2 = negar."""
        sess = self._sessions.get(name)
        if sess is None or not sess.pending:
            return False
        rid, req = next(iter(sess.pending.items()))
        sugestoes = _sugestoes_de(req)
        if req.get("tool_name") == "ExitPlanMode":
            # Aprovar sai do plano para o `prePlanMode` da CLI, que não existe em sessão que
            # nasceu no plano: ela cai em `default` e passa a pedir cada edição. A base é
            # reaplicada quando a CLI anunciar a saída (`_reaplicar_base_do_plano`).
            aprovou = (option == 1 or (option == 3 and bool(sugestoes))) \
                and sess.modo_nao_plan not in (None, "manual")
            sess.base_apos_plano = sess.modo_nao_plan if aprovou else None
        if option == 1:
            resposta = {"behavior": "allow", "updatedInput": req.get("input") or {}}
        elif option == 3 and sugestoes:
            resposta = {"behavior": "allow", "updatedInput": req.get("input") or {},
                        "updatedPermissions": sugestoes}
        elif req.get("tool_name") == "ExitPlanMode":
            resposta = {"behavior": "deny", "message": _RECUSA_PLANO}
        else:
            resposta = {"behavior": "deny", "message": "Usuário recusou."}
        await self._responder(sess, rid, resposta)
        sess.pending.pop(rid, None)
        self._recalcular_estado(sess)
        await self._notify(sess)
        return True

    async def answer_questions(self, name: str, request_id: int | str | None, answers: list[dict]) -> None:
        sess = self._sessions.get(name)
        if sess is None or not sess.question:
            raise ValueError("nenhuma pergunta pendente")
        q = sess.question
        if request_id is not None and str(request_id) != str(q["request_id"]):
            raise ValueError("a pergunta mudou")
        perguntas = q["questions"]
        respostas, conversar = respostas_do_app(perguntas, answers)
        if conversar:
            resposta = {"behavior": "deny", "message": _mensagem_conversar(respostas, conversar)}
        else:
            resposta = {"behavior": "allow",
                        "updatedInput": {"questions": perguntas, "answers": respostas}}
        await self._responder(sess, q["request_id"], resposta)
        sess.question = None
        self._recalcular_estado(sess)
        await self._notify(sess)

    async def set_permission_mode(self, name: str, mode: str) -> str:
        sess = await self.ensure_running(name)
        if sess is None:
            raise ValueError("sessão indisponível")
        r = await self._ctrl(sess, "set_permission_mode", mode=mode)
        self._definir_modo(sess, (r or {}).get("mode") or mode)
        hl_sessions.update(name, permission_mode=sess.permission_mode)
        await self._notify(sess)
        return sess.permission_mode

    @staticmethod
    def _definir_modo(sess: _Sessao, modo: str) -> None:
        sess.permission_mode = _modo_do_app(modo)
        if sess.permission_mode != "plan" and sess.permission_mode != sess.modo_nao_plan:
            sess.modo_nao_plan = sess.permission_mode
            hl_sessions.update(sess.name, previous_non_plan=sess.modo_nao_plan)

    def escolhas(self, name: str) -> tuple[str | None, str | None]:
        """(modelo, esforço) em uso no processo vivo — o `init` da CLI e o `/effort` confirmado."""
        sess = self._sessions.get(name)
        return (sess.model, sess.effort) if sess else (None, None)

    async def set_model(self, name: str, model: str | None, effort: str | None) -> bool:
        """Troca modelo em voo (`set_model`). Esforço vai como o comando local `/effort <x>` pelo
        stdin — medido: a CLI responde "Set effort level to <x> (this session only)" sem chamar a
        API, igual à TUI. Com turno em voo fica guardado e sai no `result`.
        Devolve se o esforço já vale (False = vai valer no fim do turno)."""
        sess = await self.ensure_running(name)
        if sess is None:
            raise ValueError("sessão indisponível")
        if model:
            await self._ctrl(sess, "set_model", model=model)
            sess.model = model
        esforco_ja_vale = True
        if effort and effort != sess.effort:
            # `sess.effort` só muda quando a CLI confirmar (ver `_confirmar_effort`).
            if await self.deliverable(name):
                await self._comando_local(sess, f"/effort {effort}")
            else:
                sess.effort_pendente = effort
                esforco_ja_vale = False
        hl_sessions.update(name, model=sess.model)
        await self._notify(sess)
        return esforco_ja_vale

    def _confirmar_effort(self, sess: _Sessao, texto: str) -> None:
        """Resposta do `/effort`: "Set effort level to X" confirma; qualquer outra coisa (Usage…)
        é recusa — o valor não muda e o problema aparece, em vez de um status line mentindo."""
        pedido, sess.effort_aguardando = sess.effort_aguardando, None
        if pedido is None:
            return
        if texto.startswith("Set effort level to"):
            sess.effort = pedido
            hl_sessions.update(sess.name, effort=pedido)
        else:
            self._registrar_problema(sess, "headless_turno_erro", f"esforço {pedido!r} não aceito: {texto[:200]}")

    async def _comando_local(self, sess: _Sessao, texto: str) -> None:
        """Comando local da CLI (`/effort`, `/compact`…) direto no stdin, fora da fila: a
        resposta volta como `assistant` + `result` sem chamada à API, e vira nota no chat.
        Sob a trava de entrega, como todo escritor do stdin."""
        async with self.delivery_lock(sess.name):
            if texto.startswith("/effort "):
                sess.effort_aguardando = texto.split(" ", 1)[1].strip()
            await self._write(sess, {
                "type": "user", "session_id": "", "parent_tool_use_id": None,
                "message": {"role": "user", "content": [{"type": "text", "text": texto}]},
            })
            sess.in_progress = True
            sess.state = "working"

    async def list_models(self, name: str) -> list[dict]:
        sess = await self.ensure_running(name)
        if sess is None:
            return []
        r = await self._ctrl(sess, "list_models")
        return list((r or {}).get("models") or [])

    async def _reabrir(self, sess: _Sessao) -> None:
        """Mata o processo e sobe outro com `--resume` (mesma conversa, flags novas)."""
        await self._encerrar(sess)
        await self.ensure_running(sess.name)

    def motivo_recarga(self, sess: _Sessao) -> str | None:
        """Por que o processo desta sessão está desatualizado, ou None. Hoje um motivo só: a
        config da conta (MCP do `.claude.json`, `settings.json`) mudou depois de ele subir —
        o `claude -p` só relê isso quando nasce, e sem terminal não existe `/mcp reconnect`."""
        marca = ((sess.meta or {}).get("cano") or {}).get("config_marca")
        if not marca:
            return None
        agora = time.monotonic()
        cache = getattr(sess, "_recarga_cache", None)
        if cache is None or agora - cache[0] >= 10:
            # Lê arquivo fora do laço de eventos: o `state` sai a cada evento e o núcleo não
            # espera feature. Responde o último valor conhecido e renova em segundo plano.
            sess._recarga_cache = (agora, cache[1] if cache else None)   # type: ignore[attr-defined]
            self._renovar_marca(sess, marca)
        return sess._recarga_cache[1]   # type: ignore[attr-defined]

    def _renovar_marca(self, sess: _Sessao, marca: str) -> None:
        async def _rodar() -> None:
            try:
                atual = await asyncio.to_thread(_marca_config, sess.meta.get("config_dir"))
            except Exception:
                _log.warning("claude headless: marca de config ilegível name=%s", sess.name, exc_info=True)
                return
            sess._recarga_cache = (time.monotonic(), "config" if atual != marca else None)   # type: ignore[attr-defined]
        try:
            t = asyncio.get_running_loop().create_task(_rodar())
        except RuntimeError:
            return   # sem loop (teste síncrono): fica o último valor
        self._tarefas.add(t)
        t.add_done_callback(self._tarefas.discard)

    async def recarregar(self, name: str) -> None:
        """Encerra o processo e sobe outro com `--resume`, na mesma conversa. Quem chama já
        garantiu sessão ociosa e nada em aberto (a rota); parada, só acorda."""
        sess = self._sessions.get(name)
        if sess is not None and sess.vivo:
            pid = ((sess.meta or {}).get("cano") or {}).get("pid")
            await self._encerrar(sess)
            _esquecer_cano(name, pid)
        self.acordar(name)

    async def _encerrar(self, sess: _Sessao) -> None:
        """Mata o processo e tira a sessão da memória. Saída nossa deixa `returncode` None, então
        sem o pop ela seguiria "viva" e o próximo prompt não subiria outro processo."""
        await asyncio.to_thread(self._matar, sess)
        if self._sessions.get(sess.name) is sess:
            self._sessions.pop(sess.name, None)
        if sess.leitor is not None:
            try:
                await asyncio.wait_for(sess.leitor, 5)
            except asyncio.TimeoutError:
                pass
            except Exception:
                _log.exception("claude headless: leitor falhou ao encerrar name=%s", sess.name)

    # ── processo ────────────────────────────────────────────────────────────────────────────
    # O `claude` não é filho do backend: é filho do CANO (cano.py), um processo por sessão que
    # segura stdin/stdout e escuta num socket local. O backend conecta, e reconecta quando volta
    # de um restart — o processo, o turno em voo e a permissão pendente sobrevivem. Ver o
    # snapshot em cano.py e a decisão em docs/decisoes/harnesses.md.

    async def ensure_running(self, name: str, *, so_reconectar: bool = False,
                             esperar_pronta: bool = True) -> _Sessao | None:
        sess = await self._ligar(name, so_reconectar=so_reconectar)
        if sess is not None and esperar_pronta and sess.iniciando:
            # Controles (set_model, modo, lista de modelos) só valem depois do `initialize`.
            await asyncio.wait_for(sess.initialized.wait(), _TETO_INIT_S + 5)
        return sess

    async def _ligar(self, name: str, *, so_reconectar: bool = False) -> _Sessao | None:
        # Um spawn por nome de cada vez: prompt e troca de modelo chegando juntos numa sessão
        # parada subiriam dois `claude` no mesmo .jsonl.
        async with self._spawn_locks.setdefault(name, asyncio.Lock()):
            sess = self._sessions.get(name)
            if sess is not None and sess.vivo:
                return sess
            meta = hl_sessions.load(name)
            if meta is None:
                return None
            if so_reconectar and not meta.get("cano"):
                return None
            sess = _Sessao(name, meta)
            sess.loop = asyncio.get_running_loop()
            self._sessions[name] = sess
            try:
                if not await self._spawn(sess, so_reconectar=so_reconectar):
                    self._sessions.pop(name, None)
                    return None
            except Exception as e:
                if self._sessions.get(name) is sess:
                    self._sessions.pop(name, None)
                if isinstance(e, _SubidaEsgotada):
                    # O problema da última queda (com o stderr) segue na tela; não trocar por este.
                    _log.warning("claude headless: %s name=%s", e, name)
                    raise
                _log.exception("claude headless: não subiu name=%s", name)
                if not isinstance(e, _CanoOcupado):
                    # Ocupado é passageiro (a religada resolve): gravar o problema o deixaria na
                    # lista depois de a sessão voltar.
                    try:
                        await asyncio.to_thread(self._matar, sess)
                    except Exception as stop_error:
                        self._problemas[name] = ("headless_nao_subiu", str(stop_error)[:300])
                        _log.exception("claude headless: falha ao encerrar após erro de subida name=%s", name)
                        raise
                    self._problemas[name] = ("headless_nao_subiu", str(e)[:300])
                raise
            self._garantir_vigia()
            return sess

    def _garantir_vigia(self) -> None:
        if self._vigia is None or self._vigia.done():
            self._vigia = asyncio.get_running_loop().create_task(self._vigiar_ociosas())

    async def _vigiar_ociosas(self) -> None:
        """Encerra o processo de sessão parada há `_OCIOSA_S`. Termina quando não há sessão ligada;
        a próxima ligação a recria."""
        while self._sessions:
            await asyncio.sleep(_VIGIA_S)
            for sess in list(self._sessions.values()):
                try:
                    async with self.delivery_lock(sess.name):
                        if await self._pode_estacionar(sess):
                            _log.info("claude headless: estacionando sessão ociosa name=%s parada=%ds",
                                      sess.name, int(time.monotonic() - sess.ativa_em))
                            await self._encerrar(sess)
                except Exception:
                    _log.exception("claude headless: vigia de ociosas falhou name=%s", sess.name)

    async def _pode_estacionar(self, sess: _Sessao) -> bool:
        # Nada em aberto: turno, permissão, pergunta, subida, drain, /effort pendente, troca de
        # cano (lock de spawn) ou fila por entregar — encerrar qualquer um deles perderia trabalho.
        if (not sess.vivo or sess.state != "idle" or sess.in_progress or sess.pending or sess.question
                or sess.iniciando or sess.effort_pendente or sess.effort_aguardando):
            return False
        if time.monotonic() - sess.ativa_em < _OCIOSA_S:
            return False
        if sess.drenador is not None and not sess.drenador.done():
            return False
        lock = self._spawn_locks.get(sess.name)
        if lock is not None and lock.locked():
            return False
        fila = await asyncio.to_thread(PromptQueue(sess.name).load)
        if any(not r.get("delivered") for r in fila):
            return False
        # Reconfere a atividade após a leitura da fila.
        return (not sess.in_progress and sess.state == "idle"
                and time.monotonic() - sess.ativa_em >= _OCIOSA_S)

    async def reconectar_todas(self) -> int:
        """Na subida do backend: religa em todo cano que ficou vivo (sidecar com `cano`). Sem
        isto a lista mostraria "ociosa" uma sessão parada numa permissão."""
        n = 0
        for meta in hl_sessions.list_all():
            if not meta.get("cano"):
                continue
            try:
                if await self.ensure_running(meta["name"], so_reconectar=True):
                    n += 1
            except Exception:
                _log.warning("claude headless: reconexão falhou name=%s", meta["name"], exc_info=True)
        return n

    def _agendar_religar(self, name: str) -> None:
        # ponytail: uma religada por nome a cada 10s; dois backends vivos no mesmo HOME ficariam
        # tomando a conexão um do outro — o teto só impede que isso vire laço apertado.
        agora = time.monotonic()
        if agora - self._religadas.get(name, 0.0) < 10:
            return
        self._religadas[name] = agora

        async def _religar() -> None:
            await asyncio.sleep(1.0)
            try:
                await self.ensure_running(name, so_reconectar=True)
            except Exception:
                _log.warning("claude headless: religar falhou name=%s", name, exc_info=True)
        t = asyncio.get_running_loop().create_task(_religar())
        self._tarefas.add(t)
        t.add_done_callback(self._tarefas.discard)

    def desligar_todas(self) -> None:
        """Backend saindo: fecha as conexões e deixa os canos vivos pro próximo backend."""
        for sess in list(self._sessions.values()):
            sess.desligando = True
            if sess.proc is not None:
                try:
                    sess.proc.stdin.close()
                except Exception:
                    pass

    @staticmethod
    def _matar(sess: _Sessao, meta: dict | None = None) -> None:
        """Mata o cano (e com ele o claude, mesmo grupo de processos). Idempotente; o leitor vê
        o EOF e fecha o resto. `meta` serve quando a sessão nem chegou a conectar."""
        # As duas fontes: o meta passado (sidecar já apagado) pode não ter `cano`; o da memória tem.
        cano = ((meta or {}).get("cano")) or ((sess.meta or {}).get("cano")) or {}
        pid = cano.get("pid")
        if pid is None:
            return      # nunca ligou num cano: não há o que matar
        sess.encerrando = True
        try:
            _matar_grupo(int(pid), sess.name)
        except Exception:
            sess.encerrando = False
            raise

    def _argv(self, sid: str, *, resume: bool, model=None, effort=None, permission_mode=None,
              permitir_bypass: bool = False) -> list[str]:
        base = ["claude", "-p", "--output-format", "stream-json", "--input-format", "stream-json",
                "--verbose", "--include-partial-messages", "--permission-prompt-tool", "stdio",
                "--setting-sources", "user,project,local"]
        base += ["--resume", sid] if resume else ["--session-id", sid]
        if pensamento.ler():
            # Com `-p` a CLI ignora `showThinkingSummaries` e o bloco vem cifrado; só a flag
            # explícita traz o texto, no stream e no .jsonl.
            base += ["--thinking-display", "summarized"]
        if permitir_bypass and permission_mode != "bypassPermissions":
            # Base bypass rodando noutro modo (o plano): sem a flag a CLI recusa voltar ao bypass
            # pelo resto do processo, e aprovar o plano ou "Implementar" caem em pedir cada edição.
            base.append("--allow-dangerously-skip-permissions")
        # A CLI nasce no `permissions.defaultMode` do settings.json da conta, não num padrão dela;
        # passar o modo explícito é o que faz a sessão nascer no modo que o Hangar mostra.
        return base + model_args.args_de("claude", model, effort, permission_mode)

    async def _spawn(self, sess: _Sessao, *, so_reconectar: bool = False) -> bool:
        """Liga a sessão a um cano: o que já existe (sidecar com `cano`), ou um novo. Devolve
        False só em `so_reconectar` sem cano vivo."""
        meta = sess.meta
        cano = meta.get("cano")
        if cano:
            ligado = await self._conectar(cano)
            if ligado is not None:
                lig, snap = ligado
                sess.proc = lig
                sess.leitor = asyncio.create_task(self._ler(sess))
                await self._aplicar_snapshot(sess, snap)
                _log.info("claude headless: religou name=%s pid=%s aberto=%s pendentes=%d",
                          sess.name, snap.get("pid"), snap.get("aberto"), len(snap.get("pendentes") or []))
                if (snap.get("versao") != cano_mod.VERSAO and not snap.get("aberto")
                        and not snap.get("pendentes") and snap.get("saiu") is None):
                    # Cano de outra versão e sessão ociosa: troca agora, que não custa nada.
                    _log.info("claude headless: cano versão %s != %s, reabrindo name=%s",
                              snap.get("versao"), cano_mod.VERSAO, sess.name)
                    await self._reabrir(sess)
                self._agendar_cota(sess)
                return True
            # Sem snapshot com o cano vivo: outro cliente está preso nele (cano antigo atende em
            # série). Matar derrubaria um claude saudável; subir outro poria dois no mesmo .jsonl.
            if cano.get("pid") is not None and pid_vivo(int(cano["pid"])):
                raise _CanoOcupado("o processo da sessão está ocupado por outra conexão; tente de novo")
            _esquecer_cano(sess.name, cano.get("pid"))
            meta = sess.meta = hl_sessions.load(sess.name) or {**meta, "cano": None}
        if so_reconectar:
            return False
        falhas = self._subidas.get(sess.name, 0)
        if falhas >= _TETO_SUBIDAS:
            raise _SubidaEsgotada(f"desistiu de subir após {falhas} tentativas seguidas")
        if falhas:
            # Sob a trava de spawn: todo gatilho (drain do fim da subida, SSE, conferência da fila) espera igual.
            await asyncio.sleep(_ESPERA_SUBIDA_S * 2 ** (falhas - 1))
        # Relido depois da espera: um `acordar` do usuário no meio zerou a contagem.
        self._subidas[sess.name] = self._subidas.get(sess.name, 0) + 1
        await self._subir_cano(sess)
        return True

    async def _subir_cano(self, sess: _Sessao) -> None:
        meta = sess.meta
        transcript = self.transcript_path_de(meta)
        resume = Path(transcript).exists()
        # Modo de permissão TAMBÉM no --resume: sem a flag a CLI volta ao defaultMode da conta
        # (medido: sessão "manual" reaberta após restart rodou Bash sem perguntar).
        argv = self._argv(sess.sid, resume=resume, model=sess.model, effort=sess.effort,
                          permission_mode=sess.permission_mode,
                          permitir_bypass=sess.modo_nao_plan == "bypassPermissions")
        if meta.get("engine"):
            pre = ["hangar-engine", "--exec", meta["engine"]]
            if sess.model:
                pre += ["--model", sess.model]
                if sess.context_window:
                    pre += ["--context", str(sess.context_window)]
            argv = pre + ["--"] + argv
        env = dict(os.environ)
        # Backend subido de dentro de um tmux (dev) passaria o pane do OPERADOR pro processo, e
        # o hangar-send de dentro da sessão se identificaria como a sessão dele.
        env.pop("TMUX", None)
        env.pop("TMUX_PANE", None)
        env["CP_SESSION_NAME"] = sess.name
        if not meta.get("key"):
            meta = sess.meta = hl_sessions.update(sess.name, key=uuid.uuid4().hex) or meta
        if meta.get("key"):
            env["CP_SESSION_KEY"] = meta["key"]
        env[_MARCADOR_CANO] = meta["key"]
        if meta.get("config_dir"):
            env["CLAUDE_CONFIG_DIR"] = meta["config_dir"]
        if meta.get("subagent_model"):
            env["CLAUDE_CODE_SUBAGENT_MODEL"] = meta["subagent_model"]
        # Escolha da abertura. O marcador vai sempre; a chave, só com o recurso ligado. Herdar do
        # backend seria dar o Jev a TODA sessão sem terminal, que é o contrário do que se pediu.
        env.update(runtime_config.env_jev(bool(meta.get("jev"))))
        # Configuração do servidor, lida agora: sessão que sobe depois de alguém ligar o portão já
        # nasce com ele, sem precisar recriar nada.
        env.update(runtime_config.env_function_hooks())
        log = hl_sessions._dir() / f"cano-{meta['key'][:16]}.log"
        cano, proc = await subir_cano_processo(argv, cwd=meta["cwd"], env=env, key=meta["key"], log=log,
                                               tarefas=self._tarefas)
        try:
            cano["config_marca"] = await asyncio.to_thread(_marca_config, meta.get("config_dir"))
        except (OSError, ValueError):
            # Sem marca não há motivo de recarga; a sessão sobe do mesmo jeito.
            _log.warning("claude headless: config da conta ilegível, sem marca de recarga name=%s", sess.name, exc_info=True)
        sess.meta = hl_sessions.update(sess.name, cano=cano) or {**meta, "cano": cano}
        ligado = await self._conectar(cano, esperar=_TETO_CANO_S)
        if ligado is None:
            cauda = _cauda(log)
            await asyncio.to_thread(_matar_grupo, proc.pid, sess.name)
            hl_sessions.update(sess.name, cano=None)
            raise RuntimeError(f"cano não escutou em {_TETO_CANO_S:.0f}s: {cauda}")
        sess.proc, snap = ligado
        sess.leitor = asyncio.create_task(self._ler(sess))
        for linha in snap.get("stderr_tail") or []:
            sess.stderr_tail.append(linha)
        _log.info("claude headless: subiu name=%s cano=%s claude=%s resume=%s", sess.name, proc.pid, sess.proc.pid, resume)
        sess.iniciando = True
        sess.iniciar_turno()      # relógio do "Iniciando sessão… (Ns)"
        t = asyncio.create_task(self._esperar_initialize(sess))
        self._tarefas.add(t)
        t.add_done_callback(self._tarefas.discard)

    async def _esperar_initialize(self, sess: _Sessao) -> None:
        pedido = asyncio.ensure_future(self._ctrl(sess, "initialize"))
        try:
            feito, _ = await asyncio.wait({pedido}, timeout=_AVISO_INIT_S)
            if not feito:
                # Normalmente é a CLI parada numa pergunta que só o terminal responderia (confiança
                # na pasta, login). A espera continua: hook lento responde e limpa o problema.
                _log.warning("claude headless: initialize sem resposta em %.0fs name=%s", _AVISO_INIT_S, sess.name)
                self._registrar_problema(sess, "headless_sem_resposta", "\n".join(sess.stderr_tail) or None)
                await self._notify(sess)
            resposta = await pedido
            validos = [c for c in (resposta or {}).get("commands") or [] if isinstance(c, dict) and isinstance(c.get("name"), str)]
            if validos:
                sess.comandos = validos
            else:
                _log.warning("claude headless: initialize sem lista de comandos name=%s; / usa a sonda ou a lista fixa", sess.name)
        except asyncio.TimeoutError:
            _log.warning("claude headless: initialize desistiu em %.0fs name=%s", _TETO_INIT_S, sess.name)
        except RuntimeError as e:
            _log.warning("claude headless: initialize falhou name=%s: %s", sess.name, e)
            if sess.vivo:
                # Vivo = a CLI recusou o initialize. Morto = o leitor já registrou a queda.
                self._registrar_problema(sess, "headless_nao_subiu", str(e)[:300])
        except Exception:
            _log.exception("claude headless: initialize quebrou name=%s", sess.name)
            self._registrar_problema(sess, "headless_nao_subiu", "falha interna ao iniciar a sessão")
        else:
            self._subidas.pop(sess.name, None)
            if sess.problema == "headless_sem_resposta":
                self._limpar_problema(sess)
        finally:
            sess.iniciando = False
            if not sess.in_progress:
                sess.turno_inicio = None
            sess.initialized.set()
        await self._notify(sess)
        self._agendar_cota(sess)
        # O que chegou enquanto subia está na fila: sai agora, na ordem.
        await self._drenar_fim_de_turno(sess)

    async def _conectar(self, cano: dict, *, esperar: float = 0.0) -> tuple[_Ligacao, dict] | None:
        return await conectar_cano(cano, esperar=esperar)

    async def _aplicar_snapshot(self, sess: _Sessao, snap: dict) -> None:
        # O que estava em aberto quando o backend anterior saiu — na ordem em que aconteceu.
        for linha in snap.get("stderr_tail") or []:
            sess.stderr_tail.append(linha)
        for chave in ("init", "ultimo_result", "rate_limit"):
            bruto = snap.get(chave)
            if not bruto:
                continue
            try:
                ev = json.loads(bruto)
            except ValueError:
                continue
            if chave == "ultimo_result":
                self._aplicar_uso(sess, ev)
            else:
                await self._on_event(sess, ev)
        if sess.usage is None:
            # O snapshot só guarda o `result`, que não diz o contexto: a última chamada está no .jsonl.
            try:
                uso = await asyncio.to_thread(_uso_da_ultima_chamada, self.transcript_path_de(sess.meta))
            except Exception:
                # Contexto é contabilidade: falhar aqui não pode virar "a sessão não subiu".
                _log.warning("claude headless: contexto do transcript ilegível name=%s", sess.name, exc_info=True)
                uso = None
            _aplicar_uso_da_chamada(sess, uso)
        sess.initialized.set()
        if snap.get("aberto"):
            sess.in_progress = True
        for bruto in snap.get("pendentes") or []:
            try:
                await self._on_control_request(sess, json.loads(bruto))
            except ValueError:
                continue
        self._recalcular_estado(sess)
        await self._notify(sess)

    async def _ler(self, sess: _Sessao) -> None:
        assert sess.proc and sess.proc.stdout
        try:
            while True:
                try:
                    linha = await sess.proc.stdout.readline()
                except (OSError, ValueError, asyncio.IncompleteReadError) as e:
                    # Conexão quebrou (ou linha acima do teto): sem tratar, o leitor morria com a
                    # exceção e o `wait()` do finally esperava pra sempre — sessão presa em working.
                    _log.warning("claude headless: leitura do cano falhou name=%s: %s", sess.name, e)
                    linha = b""
                if not linha:
                    # EOF sem `cano_saiu`: fomos nós (desligando/encerrando) ou o cano sumiu.
                    sess.proc.saiu(None if (sess.desligando or sess.encerrando) else -1)
                    break
                try:
                    ev = json.loads(linha)
                except ValueError:
                    sess.linhas_ruins += 1
                    if sess.linhas_ruins <= 3:
                        _log.warning("claude headless: linha não-JSON no stdout name=%s: %r", sess.name, linha[:200])
                    continue
                t = ev.get("type")
                if t == "cano_stderr":
                    sess.stderr_tail.append(str(ev.get("linha") or ""))
                    continue
                if t == "cano_saiu":
                    for l in ev.get("stderr_tail") or []:
                        if l not in sess.stderr_tail:
                            sess.stderr_tail.append(l)
                    sess.proc.saiu(ev.get("rc"))
                    break
                try:
                    await self._on_event(sess, ev)
                except Exception:
                    _log.exception("claude headless: evento mal digerido name=%s tipo=%s", sess.name, ev.get("type"))
        finally:
            rc = await sess.proc.wait() if sess.proc else None
            try:
                sess.proc.stdin.close()
            except Exception:
                pass
            cano_pid = ((sess.meta or {}).get("cano") or {}).get("pid")
            perdeu_conexao = (rc == -1 and not sess.encerrando and cano_pid is not None
                              and pid_vivo(int(cano_pid)))
            if sess.desligando:
                _log.info("claude headless: desligou name=%s (cano segue vivo)", sess.name)
            elif perdeu_conexao:
                # Outro cliente tomou a conexão (o cano troca de cliente); o claude segue vivo lá.
                # Esquecer o cano aqui deixava-o órfão e o próximo prompt subia um segundo claude.
                _log.warning("claude headless: conexão com o cano perdida, cano vivo name=%s pid=%s; religando",
                             sess.name, cano_pid)
                rc = None
                self._agendar_religar(sess.name)
            else:
                _log.info("claude headless: processo saiu name=%s rc=%s", sess.name, rc)
                # Processo foi embora: o próximo prompt sobe outro cano, não tenta este.
                _esquecer_cano(sess.name, ((sess.meta or {}).get("cano") or {}).get("pid"))
            # A CLI apanha o SIGTERM e sai com 143 (128+15), não com -15 — só o nosso encerramento
            # marca `encerrando`; qualquer outra saída não-zero é queda (-1 = o cano sumiu).
            caiu = not sess.encerrando and not sess.desligando and rc not in (0, None, -signal.SIGTERM, -getattr(signal, "SIGKILL", signal.SIGTERM))
            if caiu:
                # A faixa do chat mostra só a 1ª linha: ela tem que ser o motivo, não o rc.
                self._registrar_problema(sess, "headless_processo_caiu",
                                         "\n".join([*sess.stderr_tail, f"rc={rc}"]))
            for fut in sess.waiters.values():
                if not fut.done():
                    fut.set_exception(RuntimeError("processo encerrou"))
            sess.waiters.clear()
            sess.in_progress = False
            sess.pending.clear()
            sess.question = None
            # Queda vira marcador `dead` (push de "caiu", como no tmux); saída nossa com espera
            # em aberto só desfaz o `awaiting_input` que o adapter gravou.
            if caiu:
                self._gravar_marcador(sess, "dead")
            elif sess.state == "awaiting_input":
                self._gravar_marcador(sess, "idle")
            sess.state = "dead"
            await PushPreviewSource.get(sess.name).push("")
            await self._limpar_pensamento(sess)
            await self._limpar_ferramenta(sess)
            await self._notify(sess)

    async def _write(self, sess: _Sessao, obj: dict) -> None:
        if not sess.vivo or sess.proc is None or sess.proc.stdin is None:
            raise RuntimeError("processo não está vivo")
        sess.proc.stdin.write((json.dumps(obj) + "\n").encode())
        await sess.proc.stdin.drain()

    async def _ctrl(self, sess: _Sessao, subtype: str, *, esperar: bool = True, **req) -> dict | None:
        sess.n_req += 1
        rid = f"hangar_{sess.n_req}"
        fut: asyncio.Future | None = None
        if esperar:
            fut = asyncio.get_running_loop().create_future()
            sess.waiters[rid] = fut
        try:
            await self._write(sess, {"type": "control_request", "request_id": rid,
                                     "request": {"subtype": subtype, **req}})
            if fut is None:
                return None
            return await asyncio.wait_for(fut, _TETO_CTRL_S if subtype != "initialize" else _TETO_INIT_S)
        finally:
            sess.waiters.pop(rid, None)

    async def _responder(self, sess: _Sessao, rid: str, resposta: dict) -> None:
        await self._write(sess, {"type": "control_response",
                                 "response": {"subtype": "success", "request_id": rid, "response": resposta}})

    # ── eventos do stdout ──────────────────────────────────────────────────────────────────

    async def _on_event(self, sess: _Sessao, ev: dict) -> None:
        t = ev.get("type")
        if t != "keep_alive":
            sess.ativa_em = time.monotonic()
        if ev.get("parent_tool_use_id") and not str(t).startswith("control_"):
            # Conversa de subagente: fica fora do rótulo e da prévia do principal (o transcript
            # dele mora em subagents/agent-*.jsonl; o que ele faz agora vem por task_progress).
            # Vale pra qualquer tipo: um `result` de filho fechando o turno do pai seria pior.
            return
        if t == "control_response":
            r = ev.get("response") or {}
            fut = sess.waiters.get(r.get("request_id"))
            if fut is not None and not fut.done():
                if r.get("subtype") == "error":
                    fut.set_exception(RuntimeError(r.get("error") or "control_request recusado"))
                else:
                    fut.set_result(r.get("response") or {})
            return
        if t == "system":
            await self._on_system(sess, ev)
            return
        if t == "stream_event":
            await self._on_stream(sess, ev.get("event") or {})
            return
        if t == "command_lifecycle":
            if ev.get("state") == "started" and not sess.in_progress:
                sess.in_progress = True
                sess.state = "working"
                sess.iniciar_turno()
                await self._notify(sess)
            return
        if t == "assistant":
            blocos = (ev.get("message") or {}).get("content") or []
            if ev.get("local_command_source") is not None:
                # Saída de comando local (/context, /cost…): a CLI responde no stdout e NÃO grava
                # no .jsonl — no terminal ela aparece na tela; aqui, sem tela, vai pra fila
                # durável como bolha do assistente (histórico, reload e SSE já sabem lê-la).
                texto = "\n".join(b.get("text", "") for b in blocos
                                  if isinstance(b, dict) and b.get("type") == "text").strip()
                if sess.effort_aguardando is not None:
                    self._confirmar_effort(sess, texto)
                if texto.startswith("## Context Usage"):
                    texto += _tabela_limites(sess.janelas)
                if texto:
                    await self._nota_local(sess, texto)
                return
            _aplicar_uso_da_chamada(sess, (ev.get("message") or {}).get("usage"))
            tools = [b for b in blocos if isinstance(b, dict) and b.get("type") == "tool_use"]
            if tools:
                sess.label = _rotulo_tool(tools[-1].get("name"), tools[-1].get("input"))
            if any(isinstance(b, dict) and b.get("type") == "text" for b in blocos):
                # O bloco fechou: o .jsonl já tem a mensagem, a prévia sai de cena.
                sess.previa = ""
                await PushPreviewSource.get(sess.name).push("")
            if any(isinstance(b, dict) and b.get("type") == "thinking" for b in blocos):
                # Mesmo raciocínio: o bloco já está no .jsonl e vira o ThinkingBlock da conversa.
                await self._limpar_pensamento(sess)
            if tools:
                await self._limpar_ferramenta(sess)
            await self._notify(sess)
            return
        if t == "user":
            sess.label = None
            await self._notify(sess)
            return
        if t in ("control_request", "sdk_control_request"):
            await self._on_control_request(sess, ev)
            return
        if t == "control_cancel_request":
            rid = str(ev.get("request_id"))
            req = sess.pending.pop(rid, None)
            nota = None
            if req is not None:
                nota = f"⚙️ {self._permissao_texto(req)} — decidido por hook, sem você"
            elif sess.question and str(sess.question["request_id"]) == rid:
                perguntas = sess.question.get("questions") or []
                primeira = (perguntas[0].get("question") if perguntas and isinstance(perguntas[0], dict) else "") or ""
                nota = f"⚙️ Pergunta cancelada antes da resposta: {primeira[:120]}".rstrip(": ")
                sess.question = None
            self._recalcular_estado(sess)
            await self._notify(sess)
            if nota:
                # Alguém decidiu antes do usuário (hook PermissionRequest, ou a CLI desistiu):
                # no terminal isso aparece como uma linha; aqui a pergunta sumiria calada.
                await self._nota_local(sess, nota)
            return
        if t == "result":
            sess.in_progress = False
            sess.base_apos_plano = None
            sess.turno_inicio = None
            sess.pending.clear()
            sess.question = None
            sess.label = None
            sess.tarefas.clear()
            sess.previa = ""
            sub = ev.get("subtype") or ""
            if ev.get("local_command"):
                # Comando local não vira linha `user` no .jsonl (só `<command-name>`, às vezes com
                # outro nome: /cost grava /usage), então o reconcile nunca o acharia e o
                # redigitaria até desistir — medido: /context executado 3 vezes. A CLI já o
                # consumiu; a entrada está confirmada. Só a de slash: um prompt comum entregue
                # há pouco continua com o reconcile normal (confirmado só quando cair no .jsonl).
                await asyncio.to_thread(PromptQueue(sess.name).confirm_delivered,
                                        lambda r: str(r.get("text") or "").lstrip().startswith("/"))
            if sub != "error_during_execution" and (ev.get("is_error") or sub.startswith("error")):
                # `error_during_execution` é o interrupt (medido); o resto é falha de verdade
                # (limite de turnos, credencial, API) e some calado se não for dito aqui.
                detalhe = str(ev.get("result") or "")
                # Sem login a CLI responde `success` + is_error com este texto (medido) e segue
                # viva; o app precisa dizer o que fazer, não só "deu erro".
                codigo = "headless_sem_login" if "not logged in" in detalhe.lower() else "headless_turno_erro"
                self._registrar_problema(sess, codigo, f"{sub}: {detalhe[:300]}")
            elif sub == "success" and not ev.get("local_command"):
                # Comando local "dando certo" não diz nada da saúde da sessão (e apagaria o
                # problema que a própria resposta dele acabou de registrar, ex.: /effort recusado).
                self._limpar_problema(sess)
            # `permission_denials` (hook, regra deny, dontAsk) NÃO vira nota: cada negação já está no
            # .jsonl como tool_result com o motivo, e o card da ferramenta mostra igual ao terminal.
            self._aplicar_uso(sess, ev)
            self._recalcular_estado(sess)
            await PushPreviewSource.get(sess.name).push("")
            await self._limpar_pensamento(sess)
            await self._limpar_ferramenta(sess)
            await self._notify(sess)
            if time.time() - sess.janelas_ts > 300:
                self._agendar_cota(sess)
            if sess.effort_pendente:
                # Esforço pedido no meio do turno: agora, antes da fila, pra o próximo prompt já
                # sair no nível novo. O `result` desse comando volta aqui com `effort_pendente`
                # já vazio (ou com um pedido mais novo, que sai na sequência).
                pendente, sess.effort_pendente = sess.effort_pendente, None
                await self._comando_local(sess, f"/effort {pendente}")
                await self._notify(sess)
                return
            # Fim de turno é o momento certo de entregar o que ficou na fila. O hook Stop também
            # dispara o drain server-side, mas pode correr ANTES deste `result` chegar — aí o
            # adapter ainda se acha em turno e devolve "deferred".
            sess.drenador = asyncio.create_task(self._drenar_fim_de_turno(sess))
            self._tarefas.add(sess.drenador)
            sess.drenador.add_done_callback(self._tarefas.discard)
            return
        if t == "rate_limit_event":
            info = ev.get("rate_limit_info") or {}
            # Só `rejected` é bloqueio. `allowed_warning` é a janela passando de um patamar com
            # a cota ainda livre — pintá-lo como limite mostrava "volta HH:MM" sem limite nenhum.
            sess.limited = info.get("status") == "rejected"
            sess.limit_reset = _hora_local(info.get("resetsAt")) if sess.limited else None
            await self._notify(sess)
            return
        if t in ("keep_alive", "conversation_reset", "tool_progress"):
            return
        await self._gravar_desconhecido(sess, str(t), ev)
        if t not in sess.tipos_desconhecidos:
            # Evento que este adapter não conhece: no terminal teria tela, aqui sumiria calado.
            # Uma nota por tipo por sessão, senão vira spam.
            sess.tipos_desconhecidos.add(str(t))
            _log.warning("claude headless: evento não tratado name=%s tipo=%s", sess.name, t)
            await self._nota_local(sess, f"⚙️ Evento desconhecido da CLI: {t}")

    async def _gravar_desconhecido(self, sess: _Sessao, tipo: str, ev: dict) -> None:
        """Payload bruto no log privado (pode carregar texto de conversa), nunca no diário."""
        if sess.desconhecidos_gravados[tipo] >= _TETO_DESCONHECIDOS:
            return
        sess.desconhecidos_gravados[tipo] += 1
        linha = json.dumps({"ts": time.time(), "sessao": sess.name, "tipo": tipo, "evento": ev},
                           ensure_ascii=False, default=str)

        def gravar() -> None:
            pasta = log_paths.base() / "privado"
            pasta.mkdir(mode=0o700, parents=True, exist_ok=True)
            arq = pasta / "claude-headless-desconhecidos.jsonl"
            # O teto por tipo é da _Sessao, que renasce a cada religada: o arquivo precisa do
            # próprio limite. Append de uma linha em FS local é atômico; sem lock de propósito.
            if arq.exists() and arq.stat().st_size > _MAX_DESCONHECIDOS_B:
                return
            with open(arq, "a", encoding="utf-8") as f:
                f.write(linha + "\n")
        try:
            await asyncio.to_thread(gravar)
        except Exception:
            _log.exception("claude headless: evento desconhecido não gravado name=%s tipo=%s",
                           sess.name, tipo)

    async def _on_system(self, sess: _Sessao, ev: dict) -> None:
        sub = ev.get("subtype")
        if sub == "init":
            sid = ev.get("session_id")
            if sid and sid != sess.sid:
                # /clear (ou resume que trocou de id): o transcript agora é outro arquivo. O
                # sidecar é a fonte da lista, e o jsonl_watcher do SSE faz o reset a partir dela.
                sess.meta = hl_sessions.update(sess.name, session_id=sid) or {**sess.meta, "session_id": sid}
            if ev.get("model"):
                sess.model = ev["model"]
            if ev.get("permissionMode"):
                self._definir_modo(sess, ev["permissionMode"])
                self._reaplicar_base_do_plano(sess)
            if isinstance(ev.get("terminal_slash_commands"), list):
                sess.comandos_terminal = frozenset(str(c) for c in ev["terminal_slash_commands"])
            sess.initialized.set()
        elif sub == "status":
            if ev.get("permissionMode"):
                self._definir_modo(sess, ev["permissionMode"])
                self._reaplicar_base_do_plano(sess)
            status = ev.get("status")
            if status == "compacting":
                # A CLI repete este status a cada 30s enquanto resume; o fim vem como
                # `status: null` (com compact_result) e como o `compact_boundary` abaixo.
                # Fase própria, não o texto do rótulo: um "Pensando…" no meio não pode
                # deixar o fim sem efeito.
                sess.compactando = True
                sess.label = "Compactando…"
            elif status is None and "permissionMode" not in ev and sess.compactando:
                sess.compactando = False
                sess.label = None
            elif status == "requesting" and sess.in_progress and not sess.compactando:
                sess.label = "Pensando…"
        elif sub == "thinking_tokens":
            if sess.in_progress and not sess.compactando:
                sess.label = "Pensando…"
        elif sub == "compact_boundary":
            if sess.compactando:
                sess.compactando = False
                sess.label = None
        elif sub == "task_started":
            # Subagente (tool Agent/skill que forka): o rótulo passa a dizer o que ELE faz, que é
            # o que o terminal mostra em vez de "Agent…" parado até o fim.
            sess.tarefas[str(ev.get("task_id"))] = {
                "tipo": ev.get("subagent_type") or ev.get("task_type") or "agente",
                "passo": ev.get("description") or "",
            }
            sess.label = self._rotulo_tarefas(sess)
        elif sub == "task_progress":
            t = sess.tarefas.get(str(ev.get("task_id")))
            if t is not None:
                t["passo"] = ev.get("description") or t["passo"]
                sess.label = self._rotulo_tarefas(sess)
        elif sub in ("task_notification", "task_updated"):
            status = ev.get("status") or (ev.get("patch") or {}).get("status")
            if status in ("completed", "failed", "killed", "cancelled"):
                sess.tarefas.pop(str(ev.get("task_id")), None)
                sess.label = self._rotulo_tarefas(sess)   # None quando era o último
        else:
            return
        await self._notify(sess)

    @staticmethod
    async def _limpar_pensamento(sess: _Sessao) -> None:
        sess.pensamento = ""
        await fonte_pensamento(sess.name).push("")

    @staticmethod
    async def _limpar_ferramenta(sess: _Sessao) -> None:
        await fonte_ferramenta(sess.name).push("")

    def _rotulo_tarefas(self, sess: _Sessao) -> str | None:
        vivas = list(sess.tarefas.values())
        if not vivas:
            return None
        t = vivas[-1]
        passo = t["passo"].removeprefix("Running ").strip()
        rotulo = f"{t['tipo']}: {passo}" if passo else f"{t['tipo']}…"
        if len(vivas) > 1:
            rotulo = f"{len(vivas)} agentes · {rotulo}"
        return rotulo[:120]

    async def _on_stream(self, sess: _Sessao, e: dict) -> None:
        tipo = e.get("type")
        if tipo == "content_block_start":
            bloco = e.get("content_block") or {}
            if bloco.get("type") == "text":
                sess.previa = ""
                sess.label = None
            elif bloco.get("type") in ("tool_use", "server_tool_use", "mcp_tool_use"):
                sess.tool_nome, sess.tool_json = bloco.get("name"), ""
                sess.label = _rotulo_tool(sess.tool_nome, None)
                await fonte_ferramenta(sess.name).push(json.dumps({"nome": sess.tool_nome or "tool", "input": {}}))
            elif bloco.get("type") == "thinking":
                sess.label = "Pensando…"
                sess.pensando_desde = time.monotonic()
            await self._notify(sess)
        elif tipo == "content_block_delta":
            d = e.get("delta") or {}
            pedaco = d.get("text") or d.get("thinking") or d.get("partial_json") or ""
            # Estimativa enquanto a mensagem escreve (o `output_tokens` real só chega no fim dela);
            # o tique de 1s do stream leva o número pra tela, sem notificar a cada delta.
            sess.tokens_msg_chars += len(pedaco)
            if d.get("type") == "text_delta" and d.get("text"):
                sess.previa += d["text"]
                await PushPreviewSource.get(sess.name).push(sess.previa)
            elif d.get("type") == "thinking_delta" and d.get("thinking"):
                sess.pensamento += d["thinking"]
                await fonte_pensamento(sess.name).push(sess.pensamento)
            elif d.get("type") == "input_json_delta" and sess.tool_nome is not None:
                sess.tool_json += d.get("partial_json") or ""
                parcial = _input_parcial(sess.tool_json)
                await fonte_ferramenta(sess.name).push(json.dumps({"nome": sess.tool_nome, "input": parcial}))
                rotulo = _rotulo_tool(sess.tool_nome, parcial)
                if rotulo != sess.label:
                    sess.label = rotulo
                    await self._notify(sess)
        elif tipo == "content_block_stop":
            sess.tool_nome, sess.tool_json = None, ""
            if sess.pensando_desde is not None:
                sess.pensou_s += time.monotonic() - sess.pensando_desde
                sess.pensando_desde = None
        elif tipo == "message_delta":
            real = (e.get("usage") or {}).get("output_tokens")
            if isinstance(real, int):
                sess.tokens_msg = real
        elif tipo == "message_start":
            sess.fechar_mensagem()
            if sess.turno_inicio is None:
                sess.iniciar_turno()
            if not sess.in_progress:
                # Turno iniciado por outro caminho (steer, hook): o estado acompanha o stream.
                sess.in_progress = True
                sess.state = "working"
                await self._notify(sess)

    async def _on_control_request(self, sess: _Sessao, ev: dict) -> None:
        rid = str(ev.get("request_id"))
        req = ev.get("request") or {}
        sub = req.get("subtype")
        if sub != "can_use_tool":
            # Subtype que não tratamos: responder vazio destrava a CLI (mesma escolha do MonoCode),
            # mas a pessoa precisa saber que algo foi pedido e decidido sem ela.
            await self._responder(sess, rid, {})
            await self._gravar_desconhecido(sess, f"control_request/{sub}", ev)
            if sub not in sess.tipos_desconhecidos:
                sess.tipos_desconhecidos.add(str(sub))
                await self._nota_local(sess, f"⚙️ A CLI pediu `{sub}`; respondi vazio")
            return
        tool = req.get("tool_name")
        if tool == "AskUserQuestion":
            perguntas = (req.get("input") or {}).get("questions") or []
            sess.question = {"provider": "claude", "request_id": rid, "questions": perguntas}
        elif _plano_sem_perguntar(sess, tool):
            await self._responder(sess, rid, {"behavior": "allow",
                                              "updatedInput": req.get("input") or {}})
            return
        else:
            sess.pending[rid] = req
        self._recalcular_estado(sess)
        await self._notify(sess)

    @staticmethod
    def _aplicar_uso(sess: _Sessao, ev: dict) -> None:
        """Custo e janela de contexto de um `result` — também do que veio no snapshot. O `usage`
        dele é a SOMA das chamadas do turno (cada uma relê o cache inteiro), não o contexto: o
        contexto sai da última chamada (`_aplicar_uso_da_chamada`)."""
        if isinstance(ev.get("total_cost_usd"), (int, float)):
            sess.cost = float(ev["total_cost_usd"])
        # O turno lista também o modelo interno do Claude Code (haiku, 200k). O da conversa é o que
        # mais leu contexto: ele relê o histórico inteiro a cada chamada.
        com_janela = [(m, d) for m, d in (ev.get("modelUsage") or {}).items()
                      if isinstance(d, dict) and d.get("contextWindow")]
        if com_janela:
            m, dados = max(com_janela, key=lambda md: sum(
                md[1].get(k) or 0 for k in ("inputTokens", "cacheReadInputTokens", "cacheCreationInputTokens")))
            sess.model = sess.model or m
            sess.context_window = int(dados["contextWindow"])
            if sess.meta.get("context_window") != sess.context_window:
                # Durável: parada (ou depois de um restart) a sessão não tem de onde tirar a
                # janela, e sem ela o contexto some da barra — some justamente quando a pessoa
                # precisa dele pra decidir se continua aqui ou abre outra.
                gravado = hl_sessions.update(sess.name, context_window=sess.context_window)
                if gravado is None:
                    # Sidecar sumiu no meio do turno: seguir com o meta antigo é o certo, mas
                    # calado o sintoma só apareceria num restart, sem nada apontando pra cá.
                    _log.warning("claude headless: janela não gravada no sidecar name=%s", sess.name)
                sess.meta = gravado or sess.meta

    def _recalcular_estado(self, sess: _Sessao) -> None:
        antes = sess.state
        if not sess.vivo:
            sess.state = "dead"
        elif sess.pending or sess.question:
            sess.state = "awaiting_input"
        elif sess.in_progress:
            sess.state = "working"
        else:
            sess.state = "idle"
        # Sem pane não há hook `Notification`: é o adapter que sabe que a sessão está esperando.
        # Gravar o marcador do state_hook põe a espera na mesma esteira das sessões no tmux
        # (push de "aguardando", pausa do loop) sem outro caminho. O state_hook continua
        # escrevendo working/idle no mesmo arquivo, sem corrida: a CLI espera o PreToolUse
        # terminar antes de pedir permissão, e o PostToolUse só roda depois da resposta.
        if sess.state != antes and "awaiting_input" in (sess.state, antes):
            self._gravar_marcador(sess, sess.state)

    def _gravar_marcador(self, sess: _Sessao, state: str) -> None:
        base = _dir_marcadores(sess.meta)
        try:
            base.mkdir(parents=True, exist_ok=True)
            tmp = base / f"{sess.sid}.json.tmp"
            tmp.write_text(json.dumps({"state": state, "ts": time.time()}), encoding="utf-8")
            atomico.substituir(tmp, base / f"{sess.sid}.json")
        except OSError:
            _log.warning("claude headless: marcador de estado não gravado name=%s", sess.name, exc_info=True)

    async def _notify(self, sess: _Sessao) -> None:
        async with sess.cond:
            sess.version += 1
            sess.cond.notify_all()

    # ── estado pro SSE ─────────────────────────────────────────────────────────────────────

    def _permissao_texto(self, req: dict) -> str:
        tool, detalhe = _alvo_da_permissao(req)
        return f"Permitir {tool}? {detalhe}".strip()

    async def _nota_local(self, sess: _Sessao, texto: str) -> None:
        """Bolha do assistente fora do transcript (comando local, aviso de permissão): vai pela
        fila durável, que o histórico e o SSE já sabem ler. Falha vira log e problema visível."""
        try:
            await asyncio.to_thread(PromptQueue(sess.name).append_saida_local, texto)
        except Exception:
            _log.exception("claude headless: nota local não gravada name=%s", sess.name)
            if not sess.problema:   # um problema real (login, turno) não pode ser coberto por este
                self._registrar_problema(sess, "headless_turno_erro", f"aviso perdido: {texto[:120]}")

    def status_line(self, sess: _Sessao) -> str | None:
        parts: list[str] = []
        if sess.model:
            seg = f"🤖 {_rotulo_modelo(sess.model)}"
            esforco = sess.effort or _esforco_padrao(sess.meta.get("config_dir"))
            if esforco:
                seg += f" ({esforco})"
            parts.append(seg)
        u = sess.usage or {}
        usado = (u.get("input_tokens") or 0) + (u.get("cache_creation_input_tokens") or 0) + (u.get("cache_read_input_tokens") or 0)
        if usado and sess.context_window:
            parts.append(f"💬 {_fmt_tok(usado)}/{_fmt_tok(u.get('output_tokens') or 0)} "
                         f"{_fmt_tok(usado)}/{_fmt_tok(sess.context_window)}")
        if sess.cost is not None:
            parts.append(f"💵 ${sess.cost:.2f}")
        agora = time.time()
        for j in sess.janelas:
            emoji = {"5h": "⚡", "7d": "📅"}.get(j.rotulo)
            if not emoji:
                continue
            seg = f"{emoji}{j.rotulo}:{round(j.pct)}%"
            if j.reset_ts:
                seg += f" ↺{_format_reset(j.reset_ts, agora)}"
            parts.append(seg)
        return " │ ".join(parts) or None

    async def _drenar_fim_de_turno(self, sess: _Sessao) -> None:
        try:
            await self.drain(sess.name, self.transcript_path_de(sess.meta))
        except Exception:
            # A fila segue pendente (nada foi marcado); o próximo fim de turno tenta de novo.
            _log.exception("claude headless: drain de fim de turno falhou name=%s", sess.name)

    def _reaplicar_base_do_plano(self, sess: _Sessao) -> None:
        """A CLI saiu do plano aprovado: volta ao modo de base. Em tarefa própria porque a
        resposta do `set_permission_mode` chega por este mesmo leitor."""
        base = sess.base_apos_plano
        if not base or sess.permission_mode == "plan":
            return
        sess.base_apos_plano = None
        if sess.permission_mode == base:
            return

        async def _reaplicar() -> None:
            try:
                await self.set_permission_mode(sess.name, base)
            except Exception as e:
                _log.exception("claude headless: modo de base não reaplicado após o plano name=%s", sess.name)
                self._registrar_problema(sess, "headless_turno_erro",
                                         f"modo {base!r} não reaplicado após o plano: {str(e)[:200]}")
                await self._notify(sess)

        t = asyncio.get_running_loop().create_task(_reaplicar())
        self._tarefas.add(t)
        t.add_done_callback(self._tarefas.discard)

    def _agendar_cota(self, sess: _Sessao) -> None:
        # Referência guardada e falha logada: tarefa solta some com a exceção junto.
        t = asyncio.get_running_loop().create_task(self._atualizar_cota(sess))
        self._tarefas.add(t)
        t.add_done_callback(self._tarefas.discard)

    async def _atualizar_cota(self, sess: _Sessao) -> None:
        """Janelas da conta desta sessão, pelo leitor da faixa (cache de 5 min, rede na thread)."""
        sess.janelas_ts = time.time()   # também sem achar a conta: a cadência é a mesma
        try:
            alvo = Path(sess.meta.get("config_dir") or Path.home() / ".claude").resolve()
            contas = await asyncio.to_thread(cotas.listar_cotas)
            for c in contas:
                if c.provedor != "claude" or ":" not in c.id:
                    continue
                if Path(c.id.split(":", 1)[1]).resolve() == alvo:
                    sess.janelas = [j for j in c.janelas if not j.por_modelo]
                    await self._notify(sess)
                    return
            _log.info("claude headless: conta %s não está na faixa de cotas — sem ⚡/📅 name=%s", alvo, sess.name)
        except Exception:
            _log.warning("claude headless: leitura de cota falhou name=%s", sess.name, exc_info=True)

    def _evento(self, sess: _Sessao) -> StateEvent:
        question = options = plano = None
        if sess.pending and not sess.question:
            req = next(iter(sess.pending.values()))
            plano = _plano_pendente(req)
            if plano is not None:
                question, options = PERGUNTA_PLANO, list(OPCOES_PLANO)
            else:
                question = self._permissao_texto(req)
                options = list(OPCOES_PERMISSAO) + ([OPCAO_SEMPRE] if _sugestoes_de(req) else [])
        label, state = sess.label, sess.state
        if sess.iniciando and state == "idle":
            # Sem terminal, este é o único sinal de que o prompt foi aceito e a sessão está subindo.
            state, label = "working", "Iniciando sessão…"
        if state == "working" and (contas := sess.rotulo_turno()):
            label = f"{label or 'Trabalhando…'} {contas}"
        return StateEvent(session=sess.name, state=state, label=label, headless=True,
                          question=question, options=options,
                          recarregar_motivo=self.motivo_recarga(sess),
                          status_line=self.status_line(sess),
                          claude_permission_mode=sess.permission_mode,
                          claude_previous_non_plan=sess.modo_nao_plan,
                          claude_plan_pending=plano,
                          limited=sess.limited, limit_reset=sess.limit_reset,
                          codex_question=sess.question,
                          problema=sess.problema, problema_detalhe=sess.problema_detalhe)

    def comandos(self, name: str) -> tuple[list[dict] | None, frozenset[str]]:
        """Lista do `/` que a CLI desta sessão informou (None = ainda não subiu) e os nomes que só
        rodam na TUI."""
        sess = self._sessions.get(name)
        if sess is None:
            return None, frozenset()
        return sess.comandos, sess.comandos_terminal

    def problema_de(self, name: str) -> tuple[str, str | None] | None:
        sess = self._sessions.get(name)
        if sess is not None and sess.problema:
            return sess.problema, sess.problema_detalhe
        if name not in self._problemas_lidos:
            # Depois de um restart a memória está vazia; o sidecar guarda o último problema.
            self._problemas_lidos.add(name)
            gravado = (hl_sessions.load(name) or {}).get("problema")
            if gravado and name not in self._problemas:
                self._problemas[name] = (gravado[0], gravado[1])
        return self._problemas.get(name)

    def _registrar_problema(self, sess: _Sessao, codigo: str, detalhe: str | None) -> None:
        sess.problema, sess.problema_detalhe = codigo, (detalhe or None)
        self._problemas[sess.name] = (codigo, detalhe or None)
        # Durável: um restart do backend não pode apagar da tela por que a sessão parou.
        sess.meta = hl_sessions.update(sess.name, problema=[codigo, detalhe or None]) or sess.meta
        _log.warning("claude headless: %s name=%s %s", codigo, sess.name, (detalhe or "")[:200])

    def _limpar_problema(self, sess: _Sessao) -> None:
        sess.problema = sess.problema_detalhe = None
        self._problemas.pop(sess.name, None)
        self._problemas_lidos.add(sess.name)
        if (sess.meta or {}).get("problema"):
            sess.meta = hl_sessions.update(sess.name, problema=None) or sess.meta

    def snapshot(self, name: str) -> StateEvent | None:
        """Estado atual sem abrir stream (lista/board). None = sem processo vivo (sessão parada)."""
        sess = self._sessions.get(name)
        if sess is None or not sess.vivo:
            return None
        return self._evento(sess)

    async def _state_stream(self, name: str) -> AsyncIterator[StateEvent]:
        while True:
            if not hl_sessions.exists(name):
                if hl_sessions.em_troca(name):
                    await asyncio.sleep(1.0)
                    continue
                yield StateEvent(session=name, state="dead", headless=True)
                return
            sess = self._sessions.get(name)
            if sess is None or not sess.vivo:
                # Sessão parada (o processo morre com o backend): ociosa até o próximo prompt
                # subir outro. Não sobe aqui — abrir o chat não deve custar um processo.
                meta = hl_sessions.load(name) or {}
                prob = self.problema_de(name)
                yield StateEvent(session=name, state="idle", headless=True,
                                 claude_permission_mode=meta.get("permission_mode"),
                                 claude_previous_non_plan=meta.get("previous_non_plan"),
                                 status_line=await asyncio.to_thread(
                                     _linha_parada, meta, self.transcript_path_de(meta)),
                                 problema=prob[0] if prob else None,
                                 problema_detalhe=prob[1] if prob else None)
                while True:
                    await asyncio.sleep(1.0)
                    sess = self._sessions.get(name)
                    if sess is not None and sess.vivo:
                        break
                    if self._problemas.get(name) != prob:
                        # Subida em segundo plano (acordar) falhou com o chat já aberto: sem
                        # reemitir, o problema só apareceria numa conexão nova.
                        break
                    if not hl_sessions.exists(name):
                        break   # o laço de fora decide: encerrada, ou só trocando de modo
                if sess is None or not sess.vivo:
                    continue
            last = -1
            while True:
                async with sess.cond:
                    # Turno em voo: tique de 1s, senão o relógio e os tokens do rótulo ficam parados
                    # entre dois eventos (uma tool longa não emite nada).
                    try:
                        await asyncio.wait_for(sess.cond.wait_for(lambda: sess.version != last),
                                               1.0 if sess.turno_inicio is not None else None)
                    except TimeoutError:
                        pass
                    last = sess.version
                    ev = self._evento(sess)
                if ev.state == "dead":
                    # Processo caiu (ou o backend o matou). Com sidecar, a sessão segue existindo
                    # e volta ociosa (laço de fora); sem sidecar (kill), morreu de vez.
                    if self._sessions.get(name) is sess:
                        self._sessions.pop(name, None)
                    break
                yield ev

    # ── encerramento ───────────────────────────────────────────────────────────────────────

    def close_sync(self, name: str, meta: dict | None = None) -> None:
        """Mata o cano da sessão (chamado do registry.kill, numa thread). O leitor vê o EOF e
        fecha o resto; o sidecar é apagado por quem chamou — ANTES de chamar aqui, senão um drain
        no meio acha o sidecar e sobe outro processo — e vem em `meta`, porque é nele que está o
        pid do cano (que pode estar vivo sem este backend nunca ter conectado).

        Os dicionários são do event loop: mexer neles daqui é corrida. A retirada vai pro loop
        por `call_soon_threadsafe`; o sinal pode sair já, é só `os.kill`."""
        sess = self._sessions.get(name)
        if sess is None:
            pid = ((meta or {}).get("cano") or {}).get("pid")
            if pid is not None:
                _matar_grupo(int(pid), name)
            _limpar_rastros_do_cano(meta)
            PushPreviewSource._sources.pop(name, None)
            self._problemas.pop(name, None)
            self._problemas_lidos.discard(name)
            return

        self._matar(sess, meta)
        _limpar_rastros_do_cano(meta)

        def _retirar() -> None:
            if self._sessions.get(name) is sess:
                self._sessions.pop(name, None)
            PushPreviewSource._sources.pop(name, None)
            self._problemas.pop(name, None)
            self._problemas_lidos.discard(name)

        loop = sess.loop
        try:
            no_loop = loop is not None and loop.is_running() and asyncio.get_running_loop() is loop
        except RuntimeError:
            no_loop = False
        if no_loop or loop is None or not loop.is_running():
            _retirar()
        else:
            loop.call_soon_threadsafe(_retirar)

    def rename(self, old: str, new: str) -> None:
        sess = self._sessions.pop(old, None)
        if sess is not None:
            sess.name = new
            sess.meta["name"] = new
            self._sessions[new] = sess
        lock = self._delivery_locks.pop(old, None)
        if lock is not None:
            self._delivery_locks[new] = lock


# Anexo de imagem do composer ("legenda — 📎 imagem: <path>"). No terminal a TUI reconhece o path
# e anexa a imagem de verdade; aqui é o adapter que anexa, como bloco `image` ao lado do texto.
# O texto vai inteiro (com o path): é o que o .jsonl grava e o que a fila usa pra confirmar.
_IMG_RE = re.compile(r"📎\s*imagem:\s*(.+?)(?=\s*📎|$)", re.M)
_IMG_MIME = {".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
             ".gif": "image/gif", ".webp": "image/webp"}
_IMG_TETO = 5 * 1024 * 1024   # teto da API por imagem; acima disso fica só o path (o Read abre)


def _caminho_de_imagem(trecho: str) -> Path | None:
    # O que vem depois de "📎 imagem:" até o próximo marcador: path gerado pelo upload (sem espaço)
    # ou digitado à mão (pode ter espaço, ou texto colado depois). Tenta o trecho inteiro e
    # depois só a primeira palavra; pontuação colada no fim (vírgula, ponto) não conta.
    for cand in (trecho.strip(), trecho.split()[0] if trecho.split() else ""):
        cand = cand.rstrip(".,;:)")
        if cand and Path(cand).suffix.lower() in _IMG_MIME:
            return Path(cand)
    return None


def _blocos_do_prompt(text: str) -> tuple[list[dict], list[str]]:
    """(blocos da mensagem `user`, avisos de imagem que ficou só como path)."""
    blocos: list[dict] = [{"type": "text", "text": text}]
    avisos: list[str] = []
    for trecho in _IMG_RE.findall(text):
        caminho = _caminho_de_imagem(trecho)
        if caminho is None:
            continue
        mime = _IMG_MIME[caminho.suffix.lower()]
        try:
            dados = caminho.read_bytes()
        except OSError as e:
            avisos.append(f"⚠️ Imagem não anexada (não abre: {e.strerror or e}); só o path foi: {caminho.name}")
            continue
        if len(dados) > _IMG_TETO:
            avisos.append(f"⚠️ Imagem não anexada ({len(dados) // (1024 * 1024)} MB, teto 5 MB); só o path foi: {caminho.name}")
            continue
        blocos.append({"type": "image", "source": {"type": "base64", "media_type": mime,
                                                   "data": base64.b64encode(dados).decode("ascii")}})
    return blocos, avisos


def _alvo_da_permissao(req: dict) -> tuple[str, str]:
    """(ferramenta, detalhe curto) de um pedido de permissão ou de uma negação do `result`."""
    tool = req.get("tool_name") or "ferramenta"
    inp = req.get("input") or {}
    detalhe = req.get("description") or inp.get("command") or inp.get("file_path") or inp.get("path") or ""
    detalhe = str(detalhe)
    if len(detalhe) > 200:
        detalhe = detalhe[:200] + "…"
    return str(tool), detalhe


def _plano_pendente(req: dict) -> dict | None:
    """Plano de um `ExitPlanMode` aguardando aprovação; None para qualquer outra permissão."""
    if req.get("tool_name") != "ExitPlanMode":
        return None
    inp = req.get("input") or {}
    plano = inp.get("plan")
    caminho = inp.get("planFilePath")
    tool_use_id = req.get("tool_use_id")
    return {
        "plan": plano if isinstance(plano, str) else "",
        "path": caminho if isinstance(caminho, str) else None,
        "tool_use_id": tool_use_id if isinstance(tool_use_id, str) else None,
    }


def _tabela_limites(janelas: list) -> str:
    """Limites da conta no mesmo formato de tabela do `/context`, pra o cartão do app ler junto; o
    reset vai em epoch pra cada aparelho mostrar no próprio fuso."""
    linhas = [f"| {j.rotulo} | {round(j.pct)}% | {int(j.reset_ts) if j.reset_ts else ''} |"
              for j in janelas if not j.por_modelo]
    if not linhas:
        return ""
    return "\n\n### Plan limits\n\n| Limit | Used | Resets |\n|-------|------|--------|\n" + "\n".join(linhas)


_CAMPOS_ALVO = ("command", "file_path", "notebook_path", "path", "pattern", "url", "query", "description", "prompt")
_CAMPO_PARCIAL = re.compile(r'"(' + "|".join(_CAMPOS_ALVO) + r')"\s*:\s*"((?:[^"\\]|\\.)*)')


def _input_parcial(bruto: str) -> dict:
    """Input de tool ainda sendo escrito: JSON inteiro se já fechou, senão os campos-alvo cujo
    valor string já começou (mesmo sem a aspa final)."""
    try:
        obj = json.loads(bruto)
        return obj if isinstance(obj, dict) else {}
    except ValueError:
        pass
    achados: dict = {}
    for campo, valor in _CAMPO_PARCIAL.findall(bruto):
        try:
            achados.setdefault(campo, json.loads(f'"{valor}"'))
        except ValueError:
            achados.setdefault(campo, valor)
    return achados


def _rotulo_tool(nome: str | None, inp) -> str:
    nome = nome or "tool"
    inp = inp if isinstance(inp, dict) else {}
    campo = next((c for c in _CAMPOS_ALVO if isinstance(inp.get(c), str) and inp[c].strip()), None)
    if campo is None:
        return f"{nome}…"
    alvo = inp[campo].strip().splitlines()[0]
    if campo in ("file_path", "notebook_path"):
        alvo = re.split(r"[\\/]", alvo.rstrip("\\/"))[-1] or alvo
    if len(alvo) > 80:
        alvo = alvo[:80] + "…"
    return f"{nome}: {alvo}"


# Fora da permissão automática do plano. `ExitPlanMode` é o cartão do plano em si. As de escrita
# estão aqui como trava: medido, é a própria CLI que as barra no plano (nem chega a perguntar) —
# se uma versão futura passar a perguntar, isso vira cartão, não um "sim" calado.
_NUNCA_SOZINHO = {"ExitPlanMode", "Edit", "Write", "MultiEdit", "NotebookEdit"}


def _plano_sem_perguntar(sess: _Sessao, tool: str | None) -> bool:
    """Em plano, sessão cujo modo de base é bypass não pergunta por ferramenta.

    O modo da CLI é um só: entrar no plano tira o bypass e cada ferramenta que o plano não libera
    sozinha volta a pedir. Quem abriu em bypass não pediu isso — pediu o plano.
    """
    return (sess.permission_mode == "plan" and sess.modo_nao_plan == "bypassPermissions"
            and tool not in _NUNCA_SOZINHO)


def _modo_do_app(modo: str) -> str:
    # A CLI aceita `--permission-mode manual` mas reporta "default" no stream; o app (e a flag do
    # próximo processo) só conhecem "manual".
    return "manual" if modo == "default" else modo


def _dir_marcadores(meta: dict) -> Path:
    # Mesmo diretório que o state_hook da conta usa (`<config_dir>/.hangar-state`).
    return Path(meta.get("config_dir") or Path.home() / ".claude") / ".hangar-state"


def _sugestoes_de(req: dict) -> list[dict]:
    s = req.get("permission_suggestions")
    return [x for x in s if isinstance(x, dict)] if isinstance(s, list) else []


def _aplicar_uso_da_chamada(sess: _Sessao, u) -> None:
    """Contexto = o que UMA chamada mandou pro modelo. Uso zerado (interrupt, comando local) não
    apaga o contexto anterior."""
    if isinstance(u, dict) and any(u.get(k) for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")):
        sess.usage = u


_TAIL_TRANSCRIPT = 512 << 10


def _uso_da_ultima_chamada(path: str) -> dict | None:
    """`usage` da última mensagem do assistente no .jsonl (fora de subagente), ou None."""
    try:
        with open(path, "rb") as f:
            f.seek(0, os.SEEK_END)
            f.seek(max(0, f.tell() - _TAIL_TRANSCRIPT))
            linhas = f.read().decode("utf-8", errors="replace").splitlines()
    except FileNotFoundError:
        return None   # conversa que ainda não teve turno: o arquivo nasce depois
    except OSError:
        # Ilegível não é o mesmo que vazio: quem chama só vê "sem contexto" e não teria como
        # saber que a leitura é que está falhando.
        _log.warning("claude headless: transcript %s ilegível", path, exc_info=True)
        return None
    for linha in reversed(linhas):
        if '"assistant"' not in linha:
            continue
        try:
            o = json.loads(linha)
        except ValueError:
            continue   # a primeira linha do corte, ou uma escrita em andamento
        if o.get("type") != "assistant" or o.get("isSidechain"):
            continue
        u = (o.get("message") or {}).get("usage")
        if isinstance(u, dict) and any(u.get(k) for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")):
            return u
    return None


_FAMILIAS = ("opus", "sonnet", "haiku", "fable")


def _rotulo_modelo(modelo: str) -> str:
    """`claude-opus-5[1m]` -> `Opus5·1M`, a mesma grafia da statusline das sessões no tmux (que
    parte do display_name). Id que não é de família conhecida (motor, alias) passa como veio."""
    base = modelo.strip()
    um = base.lower().endswith("[1m]")
    if um:
        base = base[:-4]
    partes = base.lower().removeprefix("claude-").split("-")
    if not partes or partes[0] not in _FAMILIAS:
        return modelo
    versao = ".".join(p for p in partes[1:] if p.isdigit() and len(p) < 8)   # 8 dígitos = data
    familia = partes[0].capitalize()
    rotulo = (f"{familia}{versao}" if familia == "Opus" else f"{familia} {versao}").strip()
    return rotulo + ("·1M" if um else "")


def _linha_parada(meta: dict, transcript: str | None = None) -> str | None:
    """Sessão sem processo: o que ela escolheu na abertura e o contexto que já gastou.

    O contexto vem do transcript, não da memória: a sessão parada não tem processo, e é o único
    número que responde "continuo aqui ou abro outra".
    """
    partes = []
    if meta.get("model"):
        esforco = meta.get("effort") or _esforco_padrao(meta.get("config_dir"))
        partes.append(f"🤖 {_rotulo_modelo(meta['model'])}" + (f" ({esforco})" if esforco else ""))
    janela = meta.get("context_window")
    u = _uso_da_ultima_chamada(transcript) if transcript and janela else None
    if u:
        usado = sum(u.get(k) or 0 for k in
                    ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
        if usado:
            partes.append(f"💬 {_fmt_tok(usado)}/{_fmt_tok(u.get('output_tokens') or 0)} "
                          f"{_fmt_tok(usado)}/{_fmt_tok(janela)}")
    return " │ ".join(partes) or None


def _esforco_padrao(config_dir: str | None) -> str | None:
    """Esforço que a CLI usa quando a sessão não escolheu nenhum: env, depois o settings da conta."""
    env = os.environ.get("CLAUDE_CODE_EFFORT_LEVEL")
    if env:
        return env
    base = Path(config_dir) if config_dir else Path.home() / ".claude"
    try:
        nivel = json.loads((base / "settings.json").read_text(encoding="utf-8")).get("effortLevel")
    except (OSError, ValueError, AttributeError):
        return None
    return nivel if isinstance(nivel, str) and nivel else None


def _hora_local(epoch) -> str | None:
    if not isinstance(epoch, (int, float)):
        return None
    return time.strftime("%H:%M", time.localtime(epoch))


def _matar_grupo(pid: int, name: str) -> None:
    """SIGTERM no grupo do cano (cano + claude, que é filho dele no mesmo grupo). Idempotente."""
    if os.name == "nt":
        import subprocess
        if not pid_vivo(pid):
            return
        exe = shutil.which("taskkill")
        if exe is None:
            raise RuntimeError("taskkill não encontrado; o processo da sessão segue vivo")
        try:
            r = subprocess.run([exe, "/T", "/F", "/PID", str(pid)], capture_output=True, timeout=10)
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise RuntimeError("não foi possível encerrar o processo da sessão") from exc
        # 128 = processo não existe (já morreu): não é falha. Outro código é.
        if r.returncode not in (0, 128):
            raise RuntimeError(f"taskkill falhou ao encerrar a sessão (código {r.returncode})")
        deadline = time.monotonic() + 2
        while pid_vivo(pid):
            if time.monotonic() >= deadline:
                raise RuntimeError("o processo da sessão continua vivo após taskkill")
            time.sleep(0.02)
        return
    try:
        os.killpg(os.getpgid(pid), signal.SIGTERM)
    except ProcessLookupError:
        pass
    except OSError:
        _log.warning("claude headless: não matou o cano name=%s pid=%s", name, pid, exc_info=True)


def _marca_config(config_dir: str | None) -> str:
    """Impressão do que o `claude -p` lê ao nascer e não relê depois. Só o `mcpServers` do
    `.claude.json` (o resto do arquivo o próprio Claude Code reescreve a toda hora — pela data
    do arquivo, toda sessão parecia desatualizada) e o `settings.json` inteiro (hooks, statusline,
    permissões). Arquivo ausente conta como vazio; ilegível (permissão, JSON quebrado, bytes
    inválidos) levanta: "não li" não pode virar nem "mudou" nem "não mudou"."""
    raiz = Path(config_dir or (Path.home() / ".claude")).expanduser()
    partes: list[str] = []
    try:
        dados = json.loads((raiz / ".claude.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        dados = None
    partes.append(json.dumps(dados.get("mcpServers") if isinstance(dados, dict) else None, sort_keys=True))
    try:
        partes.append((raiz / "settings.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        partes.append("")
    return hashlib.sha1("\0".join(partes).encode("utf-8")).hexdigest()


def _esquecer_cano(name: str, pid: int | None) -> None:
    """Tira o `cano` do sidecar SÓ se ainda for este (pid): o leitor de um cano velho terminando
    tarde não pode apagar o cano novo que `_reabrir` acabou de subir — senão o novo vira um
    processo invisível escrevendo no mesmo .jsonl."""
    meta = hl_sessions.load(name)
    atual = ((meta or {}).get("cano") or {}).get("pid")
    if meta is not None and (pid is None or atual == pid):
        hl_sessions.update(name, cano=None)


async def conectar_cano(cano: dict, *, esperar: float = 0.0) -> tuple[_Ligacao, dict] | None:
    """Abre a conexão com o cano e lê o snapshot. None = não há cano escutando ali (morto, ou
    ainda subindo além de `esperar` segundos)."""
    escuta, token = cano.get("escuta") or "", cano.get("token")
    fim = time.monotonic() + esperar
    while True:
        try:
            # limit: o `control_response` do initialize passa de 100 KB numa linha só; o teto
            # padrão do asyncio (64 KB) estourava a leitura e o leitor ficava pendurado.
            if escuta.startswith("unix:"):
                reader, writer = await asyncio.wait_for(
                    asyncio.open_unix_connection(escuta[5:], limit=_LIMITE_LINHA), 3)
            elif escuta.startswith("tcp:"):
                host, porta = escuta[4:].rsplit(":", 1)
                reader, writer = await asyncio.wait_for(
                    asyncio.open_connection(host, int(porta), limit=_LIMITE_LINHA), 3)
            else:
                return None
            break
        except (OSError, asyncio.TimeoutError):
            if time.monotonic() >= fim:
                return None
            await asyncio.sleep(0.1)
    try:
        if token:
            writer.write((token + "\n").encode())
            await writer.drain()
        linha = await asyncio.wait_for(reader.readline(), 5)
        snap = json.loads(linha)
        if not isinstance(snap, dict) or snap.get("type") != "cano_snapshot":
            raise ValueError("primeira linha não é snapshot")
    except (OSError, ValueError, asyncio.TimeoutError):
        _log.warning("cano em %s não deu snapshot", escuta, exc_info=True)
        writer.close()
        return None
    return _Ligacao(reader, writer, snap.get("pid")), snap


async def subir_cano_processo(argv: list[str], *, cwd: str, env: dict, key: str, log: Path,
                              tarefas: set | None = None) -> tuple[dict, asyncio.subprocess.Process]:
    """Sobe um cano com `argv` como filho, fora do cgroup do backend. Devolve o dict `cano` do
    sidecar (pid, escuta, token) e o processo. Serve a qualquer sessão sem terminal (Claude, Codex)."""
    exe = shutil.which(argv[0])
    if exe is None:
        raise RuntimeError(f"binário não encontrado: {argv[0]}")
    # Caminho resolvido: no Windows o `hangar-engine` é `.CMD`, e o CreateProcess do cano não
    # acha o nome sem extensão (WinError 2) — sessão com motor não subia.
    argv = [exe, *argv[1:]]
    escuta, token = _escuta_nova(key, log.parent)
    cmd = [sys.executable, str(_CANO_PY), "--escuta", escuta, "--log", str(log), "--cwd", cwd]
    if token:
        cmd += ["--token", token]
    cmd += ["--", *argv]
    extra: dict = {}
    if os.name == "nt":
        extra["creationflags"] = _FLAGS_WINDOWS
    else:
        extra["start_new_session"] = True
        # Escopo transiente do systemd: fora do cgroup do serviço, senão o `systemctl restart`
        # mata o cano junto (mesmo motivo do tmux._scope_prefix).
        from app import tmux
        cmd = tmux._scope_prefix() + cmd
    proc = await asyncio.create_subprocess_exec(
        *cmd, cwd=cwd, env=env,
        stdin=asyncio.subprocess.DEVNULL, stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL,
        **extra)
    ceifador = asyncio.create_task(proc.wait())      # só pra não deixar zumbi
    if tarefas is not None:
        tarefas.add(ceifador)
        ceifador.add_done_callback(tarefas.discard)
    return {"pid": proc.pid, "escuta": escuta, "token": token, "ts": time.time()}, proc


def _escuta_nova(key: str, pasta: Path | None = None) -> tuple[str, str | None]:
    """Endereço do cano de uma sessão nova: socket unix na pasta dos sidecars (Linux/mac), TCP em
    loopback com token onde não há socket unix ou o caminho passa do limite do kernel."""
    if os.name != "nt":
        # Sufixo por subida: o cano anterior (mesma chave) pode ainda estar morrendo, e um path
        # igual faria o novo roubar o socket dele. A limpeza vai por `cano-<chave>*`.
        caminho = (pasta or hl_sessions._dir()) / f"cano-{key[:16]}-{uuid.uuid4().hex[:4]}.sock"
        if len(str(caminho).encode()) < 100:
            return f"unix:{caminho}", None
    import socket
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        porta = s.getsockname()[1]
    return f"tcp:127.0.0.1:{porta}", uuid.uuid4().hex


def _limpar_rastros_do_cano(meta: dict | None) -> None:
    """Sessão encerrada: socket e log do cano vão junto (o log fica só enquanto a sessão vive)."""
    key = (meta or {}).get("key")
    if not key:
        return
    for arq in hl_sessions._dir().glob(f"cano-{key[:16]}*"):
        try:
            arq.unlink()
        except OSError:
            pass


def _cauda(log: Path, n: int = 5) -> str:
    try:
        return "\n".join(log.read_text(encoding="utf-8", errors="replace").splitlines()[-n:])
    except OSError:
        return ""


def matar_orfaos() -> int:
    """Canos cuja sessão já não existe (encerrada com o backend fora, ou sidecar perdido). Os
    outros são de propósito: sobreviveram ao restart e o backend religa neles. Chamado na subida.
    Só Linux (/proc); sem ele não há varredura, e o kill de sessão continua matando pelo pid."""
    mortos = 0
    proc = Path("/proc")
    if not proc.exists():
        return 0
    vivas = {m.get("key") for m in hl_sessions.list_all() if m.get("key")}
    from app.adapters.codex import sessions as codex_sessions
    vivas |= {m.get("key") for m in codex_sessions.list_all() if m.get("headless") and m.get("key")}
    meu_uid = os.getuid()
    sem_permissao = 0
    marca = f"{_MARCADOR_CANO}=".encode()
    for p in proc.iterdir():
        if not p.name.isdigit():
            continue
        try:
            if p.stat().st_uid != meu_uid:
                continue      # processo de outro usuário: não é meu e o environ nem seria legível
            env = (p / "environ").read_bytes()
        except PermissionError:
            sem_permissao += 1
            continue
        except OSError:
            continue
        for item in env.split(b"\0"):
            if item.startswith(marca):
                chave = item[len(marca):].decode(errors="replace")
                if chave and chave not in vivas:
                    try:
                        os.kill(int(p.name), signal.SIGTERM)
                        mortos += 1
                    except OSError:
                        _log.warning("claude headless: órfão pid=%s não morreu", p.name, exc_info=True)
                break
    if sem_permissao:
        _log.info("claude headless: varredura de órfãos sem permissão em %d processo(s) meus", sem_permissao)
    return mortos
