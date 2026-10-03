# Parte 2A — Claude/Codex: buffers e publicação parcial

> **Para os agentes de execução:** usar `superpowers:subagent-driven-development` ou
> `superpowers:executing-plans`, conforme escolha do dono; marcar os Steps concluídos no arquivo.

**Goal:** retirar acumulação/parse/publicação por delta do Claude headless e da prévia Codex nos
dois modos, preservando conteúdo, primeira/última atualização, limpeza e controles imediatos.

**Architecture:** buffer stdlib compartilhado materializa prefixos somente ao publicar ou ler;
primeira publicação imediata, intermediárias a cada150ms, flush/clear por geração. Python
continua responsável único por cano, fila, envio e recuperação. Esta entrega não porta runtime.

**Tech Stack:** Python3.14, asyncio/StringIO, dependências existentes e pytest. Rust1.98.1 e
crates da parte1 são base, sem alteração nesta subparte.

**Spec:** `docs/superpowers/specs/2026-10-02-hangar-server-parte2-design-claude-codex.md`.

**Análise:** `docs/analise-hangar-server-parte2-2026-10-02-claude-codex.md`.

**Status:** aguardando aprovação da divisão2A–2D e da prioridade2A. Código abaixo é proposta
completa dos módulos/substituições, **não aplicado nem compilado**. Nenhum comando futuro de
teste/commit/serviço abaixo foi executado nesta etapa de planejamento.

## Global Constraints

- Somente Claude/Codex. Pi/Kimi/omp/orq/lista/quadro/canvas fora. Preservar documentos anteriores.
- 2A não migra writer/fila/cano/estado/permissions; não conectar Rust ao cano de uma sessão viva.
- `_input_parcial` mantém semântica e campos existentes; sem parser JSON novo, truncamento ou lib.
- Cada canal conserva nome/formato/md/full; primeira imediata, timer150ms, último pelo flush/timer,
  limpeza autoritativa imediata. Controles, pending/questions, usage e drain não usam esse timer.
- Runtime generation impede callback antigo após result/clear/EOF/rename/rebind/close. Cancelamento
  pertence ao loop; funções sync chamadas de thread encaminham corretamente antes de cancelar.
- Fonte PushPreviewSource é resolvida por publicação, porque unsubscribe pode remover a instância.
- Buffer seeded precisa cursor no fim; Unicode/escapes continuam texto exato; contador tokens vê
  todos os pedaços e não conta assinatura como texto. Falha aparece sem inventar erro do turnoCLI.
- Protocol interno atual1 e canov1 não mudam em2A. Regra de bump conjunto permanece para2B–2D.
- Nunca iniciar/reiniciar/encerrar backend, CLI real ou rodar instaladores durante esta tarefa.
  Verificação no app só com o dono no ambiente/canal acordado, sem backend duplicado.
- Tests-first; Rodar somente a pedido do dono. Fixtures sintéticas; nenhuma conversa real no repo.
  Medições posteriores indicam shape/chunk/bytes/intervalo/CPU/parede/quantidade de publicações.
- Identificadores novos em inglês; doc/comentários em pt-BR; não rodar formatter --write sem pedido.
- Não trocar/criar branch. Antes de integrar consertos upstream, status e diff; árvore alheia suja
  não acompanha merge. Commits futuros descritivos, add explícito; docs desta etapa sem commit/push.
- Executar este plano pelo Superpowers só após aprovação e escolha de método; planos2B–2D ainda
  serão elaborados próprios, com contratos da spec, não executados por este documento.

## Review Focus

- Tool com JSON parcial, escapes e campo-alvo longo: deve continuar visível e no final idêntico,
  sem reparse a cada delta (Task2).
- Deltas chegam durante callback em voo: revisão/generation não apagam texto novo e descarte
  termina antes da limpeza autoritativa (Task1).
- Último delta sem fechamento: timer publica; EOS/assistant/result/EOF limpa sem ressuscitar
  prefixo (Tasks1–3).
- Same-name/thread change/rename/source removida por unsubscribe: callback antigo não escreve
  na sessão/fonte nova (Tasks2/3).
- Prévia pendente junto com turno completo/permissão/pergunta: estado/drain permanecem imediatos;
  não drenar duas vezes nem converter falha de prévia em erroCLI (Tasks2–4).

## Ordem e dependências

Task1 define helper. Tasks2/3 integram Claude e Codex nos arquivos respectivos, usando a mesma
interface; executar sequencialmente nesta árvore. Task4 comprova regressão/uso real e registra
medição. Task2 não precisa de código Codex e Task3 não reimplementa helper. Estado/entrada/fila
não são extraídos neste plano. Nenhuma sessão de trabalho é criada como parte deste planejamento.

Antes da primeira edição na execução aprovada:

```bash
git status --short
git fetch origin
git log --oneline HEAD..origin/hangar-server-parte1
git diff --stat HEAD..origin/hangar-server-parte1
```

Na elaboração só aa8eec36/2eef31e8 mudaram CI/teste. Incorporar esses consertos na branch atual
na execução aprovada: `git merge --no-edit origin/hangar-server-parte1`, apenas com árvore sem
mudanças alheias. Se contrato/adapters avançarem, atualizar os trechos do plano antes de portá-los.
Não inferir que uma etapa CI/Windows passou só porque existe commit novo.

---


### Task 1: Buffer stdlib com publicação por geração

Status: ready-for-agent (execução só após aprovação do dono)
Risk: high (risco de ordem e limpeza da prévia)
**Files:**
- Create: `backend/app/adapters/stream_buffer.py`
- Create: `backend/tests/test_stream_buffer.py`
**Interfaces:** StreamBuffer.append/flush/discard/reset/invalidate/value; callbacks publishasync e on_errorsync. Nenhuma dependência nova nem contrato público novo.

- [x] **Step 1: Escrever testes do helper primeiro**

```python
# backend/tests/test_stream_buffer.py
import asyncio

from app.adapters.stream_buffer import StreamBuffer


def test_first_periodic_last_and_idle_tail():
    async def run():
        values = []
        periodic = asyncio.Event()
        async def publish(value):
            values.append(value)
            if value == "abc":
                periodic.set()
        buffer = StreamBuffer(publish, interval=0.01)
        await buffer.append("a")
        assert values == ["a"]
        await buffer.append("b")
        await buffer.append("c")
        assert values == ["a"]
        await asyncio.wait_for(periodic.wait(), 1)
        await buffer.append("d")
        await buffer.flush()
        assert values == ["a", "abc", "abcd"]
        await buffer.flush()
        assert values == ["a", "abc", "abcd"]
        await buffer.discard()
    asyncio.run(run())


def test_discard_cancels_old_generation_before_new_text():
    async def run():
        values = []
        async def publish(value):
            values.append(value)
        buffer = StreamBuffer(publish, interval=60)
        await buffer.append("old")
        await buffer.append(" tail")
        await buffer.discard()
        values.append("")
        await buffer.append("new")
        await asyncio.sleep(0.03)
        assert values == ["old", "", "new"]
        assert buffer.value == "new"
        await buffer.discard()
    asyncio.run(run())


def test_seeded_reset_appends_after_prefix():
    async def run():
        values = []
        async def publish(value):
            values.append(value)
        buffer = StreamBuffer(publish)
        await buffer.reset("prefix")
        await buffer.append(" suffix")
        assert buffer.value == "prefix suffix"
        assert values == ["prefix suffix"]
        buffer.invalidate("again")
        await buffer.append(" suffix")
        assert buffer.value == "again suffix"
        assert values[-1] == "again suffix"
        await buffer.discard()
    asyncio.run(run())


def test_timer_failure_is_reported_without_hot_retry_loop():
    async def run():
        errors = []
        failed = asyncio.Event()
        calls = []
        async def publish(value):
            calls.append(value)
            if value != "first":
                raise RuntimeError("synthetic publication error")
        def on_error(error):
            errors.append(type(error).__name__)
            failed.set()
        buffer = StreamBuffer(publish, on_error=on_error, interval=0.01)
        await buffer.append("first")
        await buffer.append(" next")
        await asyncio.wait_for(failed.wait(), 1)
        await asyncio.sleep(0.03)
        assert errors == ["RuntimeError"]
        assert calls == ["first", "first next"]
        await buffer.discard()
    asyncio.run(run())
```

#### Complemento da revisão estática: publicação em voo, Unicode e alias

Acrescentar à Task1/tests ANTES da implementação:

```python
def test_revision_keeps_delta_written_during_blocked_publication():
    async def run():
        entered = asyncio.Event()
        release = asyncio.Event()
        values = []
        async def publish(value):
            if value == "a":
                entered.set()
                await release.wait()
            values.append(value)
        buffer = StreamBuffer(publish, interval=60)
        first = asyncio.create_task(buffer.append("a"))
        await asyncio.wait_for(entered.wait(), 1)
        second = asyncio.create_task(buffer.append("b"))
        await asyncio.sleep(0)
        assert buffer.value == "ab"
        release.set()
        await asyncio.gather(first, second)
        await buffer.flush()
        assert values == ["a", "ab"]
        await buffer.discard()
    asyncio.run(run())


def test_discard_waits_for_inflight_publication_before_authoritative_clear():
    async def run():
        entered = asyncio.Event()
        release = asyncio.Event()
        values = []
        async def publish(value):
            if value == "old":
                entered.set()
                await release.wait()
            values.append(value)
        buffer = StreamBuffer(publish, interval=60)
        first = asyncio.create_task(buffer.append("old"))
        await asyncio.wait_for(entered.wait(), 1)
        closing = asyncio.create_task(buffer.discard())
        await asyncio.sleep(0)
        assert buffer.value == ""
        assert not closing.done()
        release.set()
        await asyncio.gather(first, closing)
        values.append("")  # o adapter limpa source só DEPOIS de discard
        await buffer.append("new")
        await buffer.flush()
        assert values == ["old", "", "new"]
        await buffer.discard()
    asyncio.run(run())


def test_full_snapshot_keeps_unicode_escapes_and_older_snapshot_alias():
    import json
    async def run():
        values = []
        async def publish(value):
            values.append(value)
        buffer = StreamBuffer(publish, interval=60)
        expected = {"command": 'echo "ação"\nlinha 😀', "path": "C:\\pasta\\arquivo"}
        raw = json.dumps(expected, ensure_ascii=False)
        for char in raw:
            await buffer.append(char)
        first_snapshot = values[0]
        await buffer.flush()
        assert first_snapshot == "{"
        assert values[0] == first_snapshot
        assert values[-1] == raw
        assert json.loads(values[-1]) == expected
        await buffer.discard()
    asyncio.run(run())
```

Guard adicional de tool: captura name/tool/generation antes de _input_parcial, confere imediatamente depois do parse, antes do push e antes do label/notify. O código acima da Task2 já contém essa mudança. Não declarar que invalidate síncrono cancela uma publicação direta que já está dentro de await push; o fechamento async usa discard e só depois publica vazio, provado pelo teste bloqueado. Rename/close síncronos correm no loop e têm guard de identidade; source.push real mantém seção crítica sem await além de adquirir Condition. Qualquer publisher futuro que fizer I/O precisa preservar esse limite ou oferecer check sob trava, em vez de supor atomicidade futura.

Erros de publicação Claude: diário/log+notify de estado, NÃO mensagem falsa de erro do turnoCLI e NÃO promessa de faixaUI nova. Propagação de erro da fonte Codex segue o complemento do provider. A tarefa de aviso Claude é guardada em _tarefas e seu callback lê a exceção, registrando somente o tipo se o aviso falhar; corpo completo na Task2.


- [x] **Step 2: Rodar teste novo antes da implementação (quando autorizado)**

`(cd backend && uv run pytest tests/test_stream_buffer.py)`
Esperado: falta de StreamBuffer. Falha de ambiente não comprova o comportamento.

- [x] **Step 3: Criar helper completo**

#### Helper compartilhado desta Task

Somente código de plano, não implementado nem compilado. Importável futuramente como `from app.adapters.stream_buffer import StreamBuffer`. Sem dependência nova.

```python
# backend/app/adapters/stream_buffer.py
from __future__ import annotations

import asyncio
import io
import logging
from collections.abc import Awaitable, Callable

_log = logging.getLogger(__name__)


class StreamBuffer:
    """Acumula pedaços e publica primeiro, periódico e último como texto completo."""

    def __init__(self, publish: Callable[[str], Awaitable[None]], *,
                 on_error: Callable[[Exception], None] | None = None,
                 interval: float = 0.15):
        self._publish = publish
        self._on_error = on_error
        self._interval = interval
        self._data = io.StringIO()
        self._generation = 0
        self._revision = 0
        self._published = False
        self._dirty = False
        self._last = 0.0
        self._pending: asyncio.Task | None = None
        self._tasks: set[asyncio.Task] = set()
        self._publishing = asyncio.Lock()

    @property
    def value(self) -> str:
        return self._data.getvalue()

    async def append(self, piece: str) -> None:
        if not piece:
            return
        self._data.write(piece)
        self._revision += 1
        self._dirty = True
        now = asyncio.get_running_loop().time()
        if not self._published or now - self._last >= self._interval:
            await self._emit(self._generation)
        else:
            self._schedule()

    def _schedule(self) -> None:
        if not self._dirty or (self._pending is not None and not self._pending.done()):
            return
        task = asyncio.create_task(self._later(self._generation))
        self._pending = task
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    async def _later(self, generation: int) -> None:
        failed = False
        try:
            delay = max(0.0, self._last + self._interval - asyncio.get_running_loop().time())
            await asyncio.sleep(delay)
            await self._emit(generation)
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            failed = True
            _log.warning("publicação parcial falhou: %s", type(exc).__name__)
            if self._on_error is not None:
                try:
                    self._on_error(exc)
                except Exception as callback_error:
                    _log.warning("aviso da publicação parcial falhou: %s", type(callback_error).__name__)
        finally:
            if self._pending is asyncio.current_task():
                self._pending = None
            if not failed and generation == self._generation and self._dirty:
                self._schedule()

    async def _emit(self, generation: int) -> None:
        async with self._publishing:
            if generation != self._generation or not self._dirty:
                return
            revision = self._revision
            await self._publish(self.value)
            if generation == self._generation:
                self._published = True
                self._last = asyncio.get_running_loop().time()
                self._dirty = revision != self._revision

    async def flush(self) -> None:
        await self._emit(self._generation)

    def invalidate(self, value: str = "") -> None:
        # Chamada só no event loop: uma geração encerrada nunca publica no bloco seguinte.
        self._generation += 1
        self._revision = 0
        self._data = io.StringIO(value)
        self._data.seek(0, io.SEEK_END)
        self._published = False
        self._dirty = bool(value)
        self._last = 0.0
        self._pending = None
        for task in tuple(self._tasks):
            task.cancel()

    async def discard(self) -> None:
        tasks = tuple(self._tasks)
        self.invalidate()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        async with self._publishing:
            pass

    async def reset(self, value: str = "") -> None:
        await self.discard()
        if value:
            self._data = io.StringIO(value)
            self._data.seek(0, io.SEEK_END)
            self._dirty = True
```

Contrato:
- append: primeira por geração imediata; outras acumulam, publicam em até150ms quando loop consegue trabalhar. Último chega pelo timer mesmo se agente parar de emitir deltas.
- flush: publica imediatamente último estado sujo, sem repetição se nada mudou; buffer continua legível.
- discard/reset: cancelam timer, invalidam geração e esperam publicação em voo; NÃO publicam string vazia. O adapter decide quando limpar fonte ou quando conservar último pedido até transcript.
- invalidate: só loop, síncrono para rename/close callbacks. Em thread externa usar `self._loop.call_soon_threadsafe(buffer.invalidate)`, nunca cancelar Task dali.
- value: compatibilidade de string para leitores/testes; materializa só quando lido. Não usar `buffer.value += chunk`, que reintroduz cópia a cada delta.
- callback deve resolver `PushPreviewSource.get(name)` NA HORA e, no Codex, conferir identidade `self._sessions.get(name) is sess`; não guardar source deletável pelo ref-count.
- on_error(Exception) síncrono no loop; adapter deve expor problema ou acordar ouvintes de erro, além do aviso do helper. Controles/permissions/questions/estado e fila NÃO passam por este timer.
- snapshot completo em150ms ainda custa soma dos tamanhos publicados; não prometer custo linear do pipeline inteiro. O buffer elimina cópia/parse por delta, não o contrato de substituição completa.

Revisão estática: callback pode aguardar enquanto outro delta entra; `_revision` evita apagar dirty desse delta. discard aguarda lock para publicar o `""` autoritativo DEPOIS de qualquer publicação em voo. invalidate sozinho impede futuros timers mas não cancela uma chamada direta append já em publicação; guards de identidade e discard nos encerramentos async continuam necessários.


- [x] **Step 4: Rodar teste focado (quando autorizado)**

`(cd backend && uv run pytest tests/test_stream_buffer.py)`
Não usar uv automaticamente nesta análise/execução sem ambiente confirmado; o binário acima é o venv canônico preexistente, conferir caminho antes. Não rodar agora.

- [x] **Step 5: Commit seletivo (na execução autorizada)**

`git add backend/app/adapters/stream_buffer.py backend/tests/test_stream_buffer.py`
`git commit -m "perf(stream): coalesce full snapshots with generation-safe buffers"`



### Task 2: Integrar os três canais do Claude sem terminal

Status: ready-for-agent (execução só após aprovação do dono)
Risk: high (risco de ordem e limpeza da prévia)
**Files:**
- Modify: `backend/app/adapters/claude_headless/adapter.py`
- Modify: `backend/tests/test_claude_headless.py`
**Interfaces:** mantém sess.previa/sess.pensamento/sess.tool_json strings para leitores; o hotpath escreve buffers, getters só materializam quando alguém pede. `_input_parcial` permanece EXATAMENTE o atual e roda na publicação, não por delta. No EOS de bloco força último input e atualiza rótulo antes de limpar ferramenta da memória; source ficaatéassistant.

- [x] **Step 1: Ajustar expectativas temporais existentes e acrescentar regressão de EOS/clear**

No teste existente test_rotulo_da_tool_mostra_o_alvo_enquanto_o_input_escreve: depois do segundo e terceiro pedaco, inserir `await sess.tool_buffer.flush()` ANTES de esperar rótulo atualizado. O teste continua provando parser parcial/escape/firstline, mas não exige publicação a cada chunk. A prova do prazo/timer fica no test_stream_buffer.
No teste existente pensamento_em_voo: inserir `await sess.thinking_buffer.flush()` após os deltas e antes da assert de snapshot inteiro. O fluxoassistant continua provando limpezaimediata. Tool primeirochunkteste existente continua imediato sem alteração.

Acrescentar teste que conta invocações do parser, mostra primeiroinput e EOS completo SEM perder caractere:

```python
def test_tool_partial_is_parsed_on_publication_and_final_block(adapter, monkeypatch):
    from app.adapters.preview_push import fonte_ferramenta
    sess = adapter._sessions["s1"]
    sess.tool_buffer._interval = 60  # o teste isola a publicação inicial e a do EOS
    parsed = []
    original = A._input_parcial
    def parse(text):
        parsed.append(text)
        return original(text)
    monkeypatch.setattr(A, "_input_parcial", parse)
    async def run():
        await adapter._on_stream(sess, {"type": "content_block_start", "content_block": {"type": "tool_use", "name": "Write"}})
        await adapter._on_stream(sess, {"type": "content_block_delta", "delta": {"type": "input_json_delta", "partial_json": '{"file_path":"/tmp/synthetic",'}})
        assert json.loads(fonte_ferramenta("s1").text)["input"]["file_path"] == "/tmp/synthetic"
        for part in ('"content":"', "body", '"}'):
            await adapter._on_stream(sess, {"type": "content_block_delta", "delta": {"type": "input_json_delta", "partial_json": part}})
        assert len(parsed) == 1
        await adapter._on_stream(sess, {"type": "content_block_stop"})
        assert json.loads(fonte_ferramenta("s1").text)["input"] == {"file_path": "/tmp/synthetic", "content": "body"}
        assert len(parsed) == 2
        await adapter._on_event(sess, {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Write", "input": {"file_path": "/tmp/synthetic", "content": "body"}}]}})
        assert fonte_ferramenta("s1").text == ""
        assert sess.tool_json == ""
    _run(run())


def test_committed_text_cancels_late_preview_timer(adapter):
    sess = adapter._sessions["s1"]
    async def run():
        await adapter._on_stream(sess, {"type": "content_block_start", "content_block": {"type": "text"}})
        await adapter._on_stream(sess, {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "first"}})
        await adapter._on_stream(sess, {"type": "content_block_delta", "delta": {"type": "text_delta", "text": " final"}})
        await adapter._on_event(sess, {"type": "assistant", "message": {"content": [{"type": "text", "text": "first final"}]}})
        await asyncio.sleep(0.18)
        assert PushPreviewSource.get("s1").text == ""
        assert sess.previa == ""
    _run(run())
```

O teste do parser usa60s apenas na fixture para isolar first/EOS sem depender de um burst acabar antes150ms. O teste independente do helper usa10ms e Event para provar publicação periódica e últimoestado sem delta seguinte. Produção mantém150ms.


- [x] **Step 2: Rodar regressão Claude antes do porte do buffer (quando autorizado)**

`(cd backend && uv run pytest tests/test_claude_headless.py)`
Esperado: caso novo falha pela ausência do buffer/coalescimento; falha de ambiente não comprova comportamento.

- [x] **Step 3: Importar helper e adicionar buffers/propriedades compatíveis da sessão**

Adicionar `from app.adapters.stream_buffer import StreamBuffer`.
Antes de `self.previa = ""` em _Sessao.__init inserir:

```python
        self.live_active: Callable[[], bool] = lambda: True
        self.live_error_handler: Callable[[Exception], None] | None = None
        self.preview_buffer = StreamBuffer(self._publish_preview, on_error=self._live_failed)
        self.thinking_buffer = StreamBuffer(self._publish_thinking, on_error=self._live_failed)
        self.tool_buffer = StreamBuffer(self._publish_tool_input, on_error=self._live_failed)
```

Adicionar métodos/propriedades completos em _Sessao:

```python
    @property
    def previa(self) -> str:
        return self.preview_buffer.value

    @previa.setter
    def previa(self, value: str) -> None:
        self.preview_buffer.invalidate(value)

    @property
    def pensamento(self) -> str:
        return self.thinking_buffer.value

    @pensamento.setter
    def pensamento(self, value: str) -> None:
        self.thinking_buffer.invalidate(value)

    @property
    def tool_json(self) -> str:
        return self.tool_buffer.value

    @tool_json.setter
    def tool_json(self, value: str) -> None:
        self.tool_buffer.invalidate(value)

    async def notify(self) -> None:
        async with self.cond:
            self.version += 1
            self.cond.notify_all()

    def _live_failed(self, error: Exception) -> None:
        if self.live_error_handler is not None:
            self.live_error_handler(error)
        else:
            _log.warning("claude headless: publicação parcial falhou name=%s error_type=%s",
                         self.name, type(error).__name__)

    async def _publish_preview(self, value: str) -> None:
        if self.live_active():
            await PushPreviewSource.get(self.name).push(value)

    async def _publish_thinking(self, value: str) -> None:
        if self.live_active():
            await fonte_pensamento(self.name).push(value)

    async def _publish_tool_input(self, value: str) -> None:
        name, tool = self.name, self.tool_nome
        generation = self.tool_buffer._generation
        def current() -> bool:
            return (self.live_active() and self.name == name and self.tool_nome == tool
                    and self.tool_buffer._generation == generation)
        if tool is None or not current():
            return
        partial = _input_parcial(value)
        if not current():
            return
        await fonte_ferramenta(name).push(json.dumps({"nome": tool, "input": partial}))
        if not current():
            return
        label = _rotulo_tool(tool, partial)
        if label != self.label:
            self.label = label
            await self.notify()
```

Campos existentes `self.previa/self.pensamento/tool_json` podem continuar com suas atribuições init; os setters agora zeram buffer e geração. Não mudar nome desses campos públicos internos históricos.

- [x] **Step 4: Helpers de limpeza e erro fora do turno**

Substituir _notify pelo corpo que delega o mesmo Condition da sessão e adicionar o tratamento do timer:

```python
    async def _notify(self, sess: _Sessao) -> None:
        await sess.notify()

    def _stream_error(self, sess: _Sessao, error: Exception) -> None:
        if self._sessions.get(sess.name) is not sess:
            return
        _log.warning("claude headless: prévia falhou name=%s error_type=%s",
                     sess.name, type(error).__name__)
        diag.registrar("headless.previa_falhou", "erro", sessao=sess.name,
                       provider="claude", erro_tipo=type(error).__name__)
        task = asyncio.get_running_loop().create_task(self._notify(sess))
        self._tarefas.add(task)
        def notified(done: asyncio.Task) -> None:
            self._tarefas.discard(done)
            if done.cancelled():
                return
            error = done.exception()
            if error is not None:
                _log.warning("claude headless: aviso da prévia falhou name=%s error_type=%s",
                             sess.name, type(error).__name__)
                diag.registrar("headless.previa_aviso_falhou", "erro", sessao=sess.name,
                               provider="claude", erro_tipo=type(error).__name__)
        task.add_done_callback(notified)

    @staticmethod
    def _invalidate_streams(sess: _Sessao) -> None:
        sess.preview_buffer.invalidate()
        sess.thinking_buffer.invalidate()
        sess.tool_buffer.invalidate()

    @staticmethod
    async def _clear_preview(sess: _Sessao) -> None:
        await sess.preview_buffer.discard()
        await PushPreviewSource.get(sess.name).push("")

    @staticmethod
    async def _limpar_pensamento(sess: _Sessao) -> None:
        await sess.thinking_buffer.discard()
        await fonte_pensamento(sess.name).push("")

    @staticmethod
    async def _limpar_ferramenta(sess: _Sessao) -> None:
        await sess.tool_buffer.discard()
        await fonte_ferramenta(sess.name).push("")
```

A falha de prévia aparece no diário e log como erro de publicação, sem dizer que o turnoCLI saudável falhou. Não gravar `headless_turno_erro` por timer de UI, nem stdout/JSON/raw parcial. A referência de task fica rastreada; estado/contador continuam normais e próxima publicação/finalflush pode tentar de novo. O handler helper já registra erro se esse aviso também falhar.

- [x] **Step 5: Substituir _on_stream inteiro mantendo uso/estado imediatos**

```python
    async def _on_stream(self, sess: _Sessao, e: dict) -> None:
        sess.live_active = lambda: self._sessions.get(sess.name) is sess
        sess.live_error_handler = lambda error: self._stream_error(sess, error)
        kind = e.get("type")
        if kind == "content_block_start":
            block = e.get("content_block") or {}
            if block.get("type") == "text":
                await self._clear_preview(sess)
                sess.label = None
            elif block.get("type") in ("tool_use", "server_tool_use", "mcp_tool_use"):
                await sess.tool_buffer.reset()
                sess.tool_nome = block.get("name")
                sess.label = _rotulo_tool(sess.tool_nome, None)
                await fonte_ferramenta(sess.name).push(json.dumps({"nome": sess.tool_nome or "tool", "input": {}}))
            elif block.get("type") == "thinking":
                sess.label = "Pensando…"
                sess.pensando_desde = time.monotonic()
            await self._notify(sess)
        elif kind == "content_block_delta":
            delta = e.get("delta") or {}
            piece = delta.get("text") or delta.get("thinking") or delta.get("partial_json") or ""
            sess.tokens_msg_chars += len(piece)
            if delta.get("type") == "text_delta" and delta.get("text"):
                await sess.preview_buffer.append(delta["text"])
            elif delta.get("type") == "thinking_delta" and delta.get("thinking"):
                await sess.thinking_buffer.append(delta["thinking"])
            elif delta.get("type") == "input_json_delta" and sess.tool_nome is not None:
                await sess.tool_buffer.append(delta.get("partial_json") or "")
        elif kind == "content_block_stop":
            await sess.preview_buffer.flush()
            await sess.thinking_buffer.flush()
            await sess.tool_buffer.flush()
            sess.tool_nome = None
            sess.tool_json = ""
            if sess.pensando_desde is not None:
                sess.pensou_s += time.monotonic() - sess.pensando_desde
                sess.pensando_desde = None
        elif kind == "message_delta":
            real = (e.get("usage") or {}).get("output_tokens")
            if isinstance(real, int):
                sess.tokens_msg = real
        elif kind == "message_start":
            sess.fechar_mensagem()
            if sess.turno_inicio is None:
                sess.iniciar_turno()
            if not sess.in_progress:
                sess.in_progress = True
                sess.state = "working"
                await self._notify(sess)
```

Pensamento NÃO reseta em cada thinking block nesta primeira otimização: o código atual acumula até assistant/result; preservar isso evita mudar rascunho exibido sem pedido. Contadores chars/time/outputtokens não esperam timer. `content_block_stop` força3 canais (sujo apenas), e guarda ferramenta source atétool_use autoritativo.

- [x] **Step 6: Substituir todos os clears e cancelar gerações no ciclo de vida**

Os pontos literais de preview atuais são:
- _ler finally: `await PushPreviewSource.get(sess.name).push("")` -> `await self._clear_preview(sess)`.
- _on_event assistant com text: substituir `sess.previa = ""` + `await PushPreviewSource.get(sess.name).push("")` por `await self._clear_preview(sess)`.
- _on_event result: remover atribuição antecipada `sess.previa = ""`; o push vazio no final do ramo vira `await self._clear_preview(sess)` e permanece junto das limpezas pensamento/ferramenta.
- `desligar_todas`: inserir `self._invalidate_streams(sess)` ANTES de `sess.desligando=True`; método roda no loop da saída backend e não mata cano.
- `close_sync` _retirar callback: inserir `self._invalidate_streams(sess)` antes do pop `_sessions`; callback já vai via sess.loop.call_soon_threadsafe fora do loop. Não cancelar Task diretamente na thread do registry.

Substituir rename inteiro para também usar o loop da sessão (api.rename_session manda registry.rename para thread; `api.py:2801`, `registry.py:2311`):

```python
    def rename(self, old: str, new: str) -> None:
        current = self._sessions.get(old)

        def rename_on_loop() -> None:
            sess = self._sessions.get(old)
            if sess is not None:
                self._invalidate_streams(sess)
                for key in (old, f"{old}#pensamento", f"{old}#ferramenta"):
                    source = PushPreviewSource._sources.get(key)
                    if source is not None:
                        source.reset()
                self._sessions.pop(old, None)
                sess.name = new
                sess.meta["name"] = new
                self._sessions[new] = sess
            lock = self._delivery_locks.pop(old, None)
            if lock is not None:
                self._delivery_locks[new] = lock

        loop = current.loop if current is not None else None
        try:
            same_loop = loop is not None and asyncio.get_running_loop() is loop
        except RuntimeError:
            same_loop = False
        if loop is None or not loop.is_running() or same_loop:
            rename_on_loop()
            return
        done = threading.Event()
        errors: list[BaseException] = []
        def apply() -> None:
            try:
                rename_on_loop()
            except BaseException as error:
                errors.append(error)
            finally:
                done.set()
        loop.call_soon_threadsafe(apply)
        if not done.wait(5):
            raise RuntimeError("timeout ao renomear buffer da sessão Claude")
        if errors:
            raise errors[0]
```

Não fechar/matar cano no rename. Sources são resolvidas novamente a cada publicação. A source antiga se limpa; os timers cancelados não escrevem texto do nome anterior na sessão sucessora.

- [x] **Step 7: Rodar focados (quando autorizado) e commit seletivo**

`(cd backend && uv run pytest tests/test_stream_buffer.py tests/test_claude_headless.py tests/test_preview_push.py)`
`git add backend/app/adapters/claude_headless/adapter.py backend/tests/test_claude_headless.py`
`git commit -m "perf(headless): batch Claude previews and partial tool input"`



### Task 3: Integrar a prévia Codex com e sem terminal

**Files:**
- Modify: `backend/app/adapters/codex/adapter.py`
- Modify: `backend/tests/test_codex_adapter.py`

**Interfaces:**
- Consumes: `StreamBuffer` da Task1 e PushPreviewSource existente.
- Produces: `_consumir` com buffer por bomba/item/geração, callback para fonte atual e cleanup
  nos caminhos de attach/close/rename/thread/pane/permissão. Nenhum segundo leitor de notifications.
- Não alterar `map_state`, AppServerClient, API, fila, entrega ou protocolo. Mesmo consumidor
  atende terminal e sem terminal. Estado/requests/usage/drain não aguardam o timer.

- [x] **Step 1: Escrever os testes reais do consumidor primeiro**

#### Testes de comportamento

Adicionar ao fim de `backend/tests/test_codex_adapter.py`. O arquivo já fornece `asyncio`, `_LiveQueueClient`, `_FakeClient`, sidecars em tmp e mocks tmux. Os exemplos usam somente esses doubles, não abrem CLI ou backend vivo.

```python
async def _wait_preview_text(name: str, expected: str) -> None:
    async with asyncio.timeout(2):
        while PushPreviewSource.get(name).text != expected:
            await asyncio.sleep(0)


async def _finish_preview_client(adapter: CodexAdapter, name: str, client) -> None:
    task = adapter._sessions.get(name, {}).get("bomba")
    await client._q.put(None)
    if task is not None:
        await task


async def test_coalesced_preview_cannot_reappear_after_turn_completed():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    name = "preview-turn-clear"
    adapter.attach(name, client, "t")
    monitor = adapter.state_monitor(name, lambda: name)
    await monitor.__anext__()
    try:
        await client._q.put({"method": "turn/started", "params": {"threadId": "t"}})
        assert (await monitor.__anext__()).state == "working"
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "first"}})
        await _wait_preview_text(name, "first")
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "-late"}})
        await client._q.put({"method": "turn/completed", "params": {"threadId": "t"}})
        assert (await monitor.__anext__()).state == "idle"
        assert PushPreviewSource.get(name).text == ""
        await asyncio.sleep(0.2)
        assert PushPreviewSource.get(name).text == ""
    finally:
        await monitor.aclose()
        await _finish_preview_client(adapter, name, client)


async def test_coalesced_preview_does_not_join_distinct_agent_messages():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    name = "preview-item-clear"
    adapter.attach(name, client, "t")
    try:
        await client._q.put({"method": "turn/started", "params": {"threadId": "t"}})
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "Preamble"}})
        await _wait_preview_text(name, "Preamble")
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "-stale"}})
        await client._q.put({"method": "item/completed", "params": {"item": {"type": "agentMessage", "text": "Preamble-stale"}}})
        await client._q.put({"method": "item/started", "params": {"item": {"type": "agentMessage", "text": ""}}})
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "Reply"}})
        await _wait_preview_text(name, "Reply")
        await asyncio.sleep(0.2)
        assert PushPreviewSource.get(name).text == "Reply"
    finally:
        await _finish_preview_client(adapter, name, client)


async def test_final_pending_prefix_reaches_recreated_source_after_last_sse_closes():
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    name = "preview-source-recreate"
    adapter.attach(name, client, "t")
    old = PushPreviewSource.get(name)
    subscription = old.subscribe()
    await subscription.__anext__()
    try:
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "first"}})
        await _wait_preview_text(name, "first")
        await subscription.aclose()
        assert name not in PushPreviewSource._sources
        current = PushPreviewSource.get(name)
        assert current is not old
        await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "-last"}})
        await _wait_preview_text(name, "first-last")
        assert old.text == "first"
        assert current.text == "first-last"
    finally:
        await subscription.aclose()
        await _finish_preview_client(adapter, name, client)


async def test_replaced_session_generation_cannot_publish_old_pending_prefix():
    adapter = CodexAdapter()
    old_client = _LiveQueueClient()
    new_client = _LiveQueueClient()
    name = "preview-generation"
    adapter.attach(name, old_client, "old-thread")
    await old_client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "old"}})
    await _wait_preview_text(name, "old")
    await old_client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "-pending"}})
    await asyncio.sleep(0)
    old_task = adapter._sessions[name]["bomba"]
    adapter.attach(name, new_client, "new-thread")
    try:
        await new_client._q.put({"method": "turn/started", "params": {"threadId": "new-thread"}})
        await new_client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "new"}})
        await _wait_preview_text(name, "new")
        await asyncio.sleep(0.2)
        assert PushPreviewSource.get(name).text == "new"
    finally:
        await _finish_preview_client(adapter, name, new_client)
        await asyncio.gather(old_task, return_exceptions=True)


async def test_close_preserve_preview_keeps_last_published_text_without_late_timer(monkeypatch):
    monkeypatch.setattr(codex_adapter, "matar_app_server", lambda name: None)
    monkeypatch.setattr(codex_adapter.sem_terminal, "matar", lambda meta: None)
    adapter = CodexAdapter()
    client = _LiveQueueClient()
    name = "preview-preserve-close"
    adapter.attach(name, client, "t")
    await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "first"}})
    await _wait_preview_text(name, "first")
    await client._q.put({"method": "item/agentMessage/delta", "params": {"delta": "-pending"}})
    await asyncio.sleep(0)
    task = adapter._sessions[name]["bomba"]
    adapter.close_sync(name, preserve_preview=True)
    await asyncio.gather(task, return_exceptions=True)
    await asyncio.sleep(0.2)
    assert PushPreviewSource.get(name).text == "first"
```

Os testes verificam saída após espera maior que a janela do timer, separação de itens, fonte removida por unsubscribe e troca real de bomba/session dict. Não são asserts sobre contadores privados do próprio helper. Os testes existentes que encerram `_FakeClient` sem `closed=True` continuam precisando da publicação final de todos os deltas: por isso o EOF de fixture faz flush, enquanto EOF real que já é autoridade limpa imediatamente.

Também conservar e rodar quando autorizado os testes existentes: `test_dois_sse_na_mesma_sessao_recebem_a_resposta_inteira`, `test_bomba_publica_na_fonte_recriada_depois_que_sse_fecha`, `test_previa_zera_a_cada_agent_message_do_mesmo_turno` e os de `preview_push` (sem alterar contratos). Se o primeiro passar só porque ambos observam último prefixo via flush, os novos testes acima ainda comprovam timer de fluxo vivo e invalidação.


- [x] **Step 2: Rodar testes Codex focados (quando autorizado)**

`(cd backend && uv run pytest tests/test_codex_adapter.py tests/test_preview_push.py)`
Esperado antes da mudança: falha nas regressões de período/geração; falha de ambiente não prova comportamento.

- [x] **Step 3: Aplicar o consumidor completo e o encerramento de buffer**

#### Consumidor e import

Acrescentar `from app.adapters.stream_buffer import StreamBuffer` ao adapter.

Substituir apenas `_consumir` pelo método abaixo. Toda lógica de estado/settings/perguntas/controle/fila continua a implementação existente; as únicas alterações são buffer e seu encerramento.

```python
    async def _consumir(self, name: str, client: AppServerClient, sess: dict, espalhar) -> None:
        async def publish(text: str) -> None:
            if self._sessions.get(name) is sess and sess.get("client") is client:
                await PushPreviewSource.get(name).push(text)

        def failed(exc: Exception) -> None:
            if self._sessions.get(name) is sess:
                _log.warning("codex: publicação da prévia falhou name=%s tipo=%s",
                             name, type(exc).__name__)
                espalhar(exc)

        buffer = StreamBuffer(publish, on_error=failed)
        sess["preview_buffer"] = buffer
        try:
            async for notif in client.notifications():
                if self._sessions.get(name) is not sess:
                    return
                params = notif.get("params") or {}
                if notif.get("method") == "thread/started" and sess.get("app_pid"):
                    thread = params.get("thread") or {}
                    if thread.get("id") != sess["thread_id"]:
                        await buffer.discard()
                        await publish("")
                        await asyncio.to_thread(codex_sessions.switch_thread, name, thread, sess["thread_id"],
                                                endpoint=sess["endpoint"], app_pid=sess["app_pid"])
                # O app-server também publica estados de outras threads, inclusive subagentes.
                if params.get("threadId") is not None and params["threadId"] != sess["thread_id"]:
                    continue
                if sess.get("voice_events") is not None:
                    from app.codex_voice import forward
                    forward(sess, notif)
                if sess.get("headless") and notif.get("id") is not None:
                    # Pedido do servidor: na TUI quem responde é ela; aqui é o cartão do app.
                    if notif["method"] in sem_terminal.APROVACOES:
                        espalhar(self._question_state(name, sess))
                        continue
                    if notif["method"] != "item/tool/requestUserInput":
                        await self._recusar_pedido(name, client, notif)
                        continue
                mapped = map_state(notif)
                method = notif.get("method")
                compact_updated = method in {"item/started", "item/completed"} and \
                    (params.get("item") or {}).get("type") == "contextCompaction"
                if compact_updated:
                    sess["compacting"] = method == "item/started"
                elif method == "turn/completed":
                    sess.pop("compacting", None)
                current_turn = not sess.get("turn_id") or params.get("turnId") in (None, sess["turn_id"])
                response_started = current_turn and (bool(mapped.preview_delta) or (
                    method == "item/completed" and (params.get("item") or {}).get("type") == "agentMessage"
                    and bool((params.get("item") or {}).get("text"))
                ))
                if method == "turn/started" or mapped.state == "idle":
                    sess.pop("codex_response_started", None)
                elif response_started:
                    sess["codex_response_started"] = True
                buffering_updated = False
                if mapped.buffering is not None:
                    if params.get("threadId") != sess["thread_id"] or not sess["in_progress"] or sess.get("codex_response_started"):
                        continue
                    if sess.get("turn_id") and params.get("turnId") != sess["turn_id"]:
                        continue
                    buffering_updated = sess.get("codex_buffering", False) != mapped.buffering
                    sess["codex_buffering"] = mapped.buffering
                elif method == "turn/started" or mapped.state == "idle" or response_started:
                    buffering_updated = bool(sess.pop("codex_buffering", False))
                async_updated = method in {"item/started", "item/completed"} and \
                    sess["async_questions"].observe(params.get("item") or {})
                if mapped.state is not None:
                    sess["state_revision"] = sess.get("state_revision", 0) + 1
                settings_updated = method == "thread/settings/updated"
                if settings_updated:
                    if params.get("threadId") != sess["thread_id"]:
                        continue
                    settings = params.get("threadSettings") or {}
                    sess["model"] = settings.get("model")
                    sess["effort"] = sess["default_effort"] = settings.get("effort")
                    sess["mode"] = (settings.get("collaborationMode") or {}).get("mode", "default")
                    sess["settings_revision"] = sess.get("settings_revision", 0) + 1
                turn_problem = _turn_problem(notif) if current_turn else None
                problem_updated = False
                if turn_problem is not None:
                    problem_updated = sess.get("turn_problem") != turn_problem
                    sess["turn_problem"] = turn_problem
                elif method == "turn/started" or response_started or \
                        (current_turn and method == "item/started"
                         and (params.get("item") or {}).get("type") != "userMessage") or \
                        (method == "turn/completed" and sess.get("turn_problem", ("",))[0] == "codex_sem_conexao"):
                    # Reconectou (chegou resposta ou qualquer item novo, inclusive só ferramenta) ou o
                    # turno fechou sem erro: o aviso não vale mais.
                    problem_updated = sess.pop("turn_problem", None) is not None
                if method == "turn/started":
                    await buffer.reset()
                    await publish("")
                    # guarda o turnId do turno em voo (turn/interrupt exige threadId+turnId).
                    turn_id = ((notif.get("params") or {}).get("turn") or {}).get("id")
                    if turn_id:
                        sess["turn_id"] = turn_id
                elif mapped.preview_delta is not None:
                    await buffer.append(mapped.preview_delta)
                elif method in ("item/started", "item/completed") and \
                        ((notif.get("params") or {}).get("item") or {}).get("type") == "agentMessage":
                    # Um turno pode ter varios agentMessage (preambulo "Vou conferir…" + resposta). O
                    # completado vira bolha propria pelo rollout; se ficasse no buffer, a previa
                    # mostrava "Vou conferir.Resposta" ate o turno fechar.
                    await buffer.discard()
                    await publish("")
                elif method == "turn/completed":
                    # o texto final ja caiu no rollout -> vira ChatEvent autoritativo via
                    # transcript_stream; o sse.py tambem suprime via _already_committed. Limpa aqui pra
                    # nao deixar o ultimo delta pendurado ate o proximo turno.
                    await buffer.discard()
                    await publish("")
                    # Marca idle ANTES de drenar (nao depender do thread/status/changed idle ter chegado
                    # antes -- a ordem das notifications do app-server nao e garantida). A drain chama
                    # send_prompt -> deliverable(), que le in_progress: se ficasse True aqui, deliverable
                    # daria False, send_prompt viraria "deferred", a drain reverteria e a entrada
                    # enfileirada ficaria presa pra sempre (perda silenciosa). Tambem zera o turn_id: o
                    # turno morreu -> interrupt vira no-op em vez de mandar turn/interrupt de turno morto.
                    sess["state"] = "idle"
                    sess["in_progress"] = False
                    sess["turn_id"] = None
                    # Turno terminou: a bomba única e permanente entrega a fila mesmo sem SSE aberto.
                    # Best-effort: falha aqui nunca derruba o consumidor do app-server.
                    try:
                        await self.drain(name, "")
                    except Exception:
                        _log.exception("codex drain-on-complete falhou name=%s", name)
                    if self._sessions.get(name) is not sess:
                        return
                if mapped.state is not None:
                    sess["state"] = mapped.state
                    was = sess["in_progress"]
                    sess["in_progress"] = mapped.state == "working"
                    sess["turn_state_known"] = not sess["in_progress"] or bool(sess.get("turn_id"))
                    # Carimba QUANDO o turno comecou. O TTL de deliverable() mede a partir daqui; sem
                    # isto um in_progress vindo do stream (nao do send_prompt) ficava com marco 0 e
                    # expirava de imediato, liberando envio no meio de um turno vivo.
                    if sess["in_progress"] and not was:
                        sess["in_progress_since"] = time.monotonic()
                # Task D: acumula tokenUsage/rateLimits por sessao (snapshot mais recente de cada) --
                # sao notifications esparsas, nao vem toda hora, entao guarda no dict quente pra
                # sobreviver ate o proximo StateEvent emitido (mesmo que seja por outro motivo, tipo
                # turn/started).
                if mapped.token_usage is not None:
                    sess["token_usage"] = mapped.token_usage
                if mapped.rate_limits is not None:
                    sess["rate_limits"] = mapped.rate_limits
                question_updated = async_updated or method in ("item/tool/requestUserInput", "serverRequest/resolved")
                if mapped.state is None and mapped.token_usage is None and mapped.rate_limits is None and not settings_updated and not question_updated and not buffering_updated and not problem_updated and not compact_updated:
                    # Neutro (method desconhecido) ou so preview_delta: StateEvent nao tem campo de
                    # preview -> nada a emitir aqui (o preview ja foi empurrado acima, fora do
                    # StateEvent -- efeito colateral adicional, nao substitui).
                    continue
                # status_line SEMPRE montado com o que ha de mais recente acumulado (model/effort do
                # dict quente + token_usage/rate_limits guardados acima) -- nao so quando ESTE notif
                # trouxe token/limite novo, senao o front perderia contexto/limites em StateEvents de
                # working/idle puros (a maioria).
                espalhar(self._question_state(name, sess))
            # notifications() terminou = EOF do app-server (o read loop empurra o sentinela ao morrer).
            # Dead-detection (backlog T4-m2): emite dead pra o front + limpa a sessao da memoria (o
            # sidecar duravel fica; ensure_running reabre num acesso futuro). getattr: um client FAKE de
            # teste sem `closed` termina o stream sem simular morte -> nao emite dead.
            if getattr(client, "closed", False) and self._sessions.get(name) is sess:
                await buffer.discard()
                await publish("")
                sess["state"] = "dead"
                self._sessions.pop(name, None)
                PushPreviewSource._sources.pop(name, None)
                espalhar(StateEvent(session=name, state="dead"))
    
            elif self._sessions.get(name) is sess:
                await buffer.flush()
        finally:
            await buffer.discard()
            if sess.get("preview_buffer") is buffer:
                sess.pop("preview_buffer", None)
```

#### Encerramento síncrono

Acrescentar este método ao `CodexAdapter`. `close_sync` pode ser chamado do threadpool; timer/cancel são do loop da bomba, não da thread do registry.

```python
    def _invalidate_preview(self, sess: dict | None) -> None:
        buffer = sess.get("preview_buffer") if sess is not None else None
        if buffer is None:
            return
        loop = self._loop
        if loop is None or not loop.is_running():
            return
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if running is loop:
            buffer.invalidate()
        else:
            loop.call_soon_threadsafe(buffer.invalidate)
```

Inserções exatas (não alterar ordem dos atos existentes de lifecycle/sidecar):

1. `attach`, depois de `anterior = self._sessions.get(name)` e antes de cancelar a bomba antiga:

```python
        self._invalidate_preview(anterior)
```

2. `close_sync`, imediatamente depois de `sess = self._sessions.pop(name, None)` e antes de watcher/sub/bomba cancel/remover source:

```python
        self._invalidate_preview(sess)
```

3. `rename.rearmar`, depois de `sess = self._sessions.pop(old, None)` e antes de cancelar/rearmar bomba:

```python
            self._invalidate_preview(sess)
```

4. `ensure_running`, caminho "A TUI trocou de conversa", antes de `self._sessions.pop(name, None)`/cancel/gather do bloco de695:

```python
                self._invalidate_preview(sess)
```

5. `_watch_tmux` no encerramento do pane, antes de cancelar `sess["bomba"]` (por volta456):

```python
            self._invalidate_preview(sess)
```

6. `set_permission_mode_sem_terminal`, depois de retirar sessão do mapa (`1368`) e antes de cancelar bomba/fechar client:

```python
                self._invalidate_preview(sess)
```

A invalidação síncrona impede novos timers. A bomba cancela/aguarda timer e publicação em voo no finally async de `_consumir`. O guard de identidade evita que uma bomba anterior já em publicação escreva na sessão nova. Preserve_preview conserva apenas o último texto publicado; não conserva timer, StringIO ou publicação atrasada da geração anterior.


- [x] **Step 4: Rodar regressões Codex/helper (quando autorizado)**

`(cd backend && uv run pytest tests/test_stream_buffer.py tests/test_codex_adapter.py tests/test_preview_push.py)`
Esperado: todos os conteúdos e resets da Task passam; estado/drain imediatos e nenhuma prévia antiga.

- [x] **Step 5: Commit da integração Codex na execução autorizada**

```bash
git status --short
git add backend/app/adapters/codex/adapter.py backend/tests/test_codex_adapter.py
git commit -m "perf(codex): coalesce preview prefixes and cancel stale publications"
git status --short
```

Não duplicar helper ou consumidor. Protocol1 permanece; nenhuma operação no cano muda de dono.


### Task 4: Conferência integrada e uso real — verificação manual

**Files:**
- Modify: `docs/decisoes/harnesses.md` (deltas headless, regra curta e medida)
- Modify: `docs/decisoes/plataforma.md` (evidência ligada à parte2 quando apropriado)

**Interfaces:**
- Consumes: StreamBuffer e consumidores Claude/Codex das Tasks1–3; protocolo1 e fila originais.
- Produces: prova de conteúdo/cadência/limpeza, dados antes/depois, limitações reais registradas.

- [x] **Step 1: Rodar os checks focados finais (quando autorizado)**

```bash
(cd backend && uv run pytest tests/test_stream_buffer.py tests/test_claude_headless.py tests/test_codex_adapter.py tests/test_preview_push.py)
```

Esperado: regressões existentes preservadas e novos testes de coalescimento/geração aprovados.
Os testes de período usam Event ou loop fake/intervalo pequeno; não falhar simplesmente por
milissegundo de agendamento. Falhou: corrigir causa e repetir somente teste/arquivo que falhou.
Não rodar check/frontend/Cargo que não foi alterado para fabricar mais evidência.

- [ ] **Step 2: Conferir conteúdo/controle no uso real — verificação manual**

O dono disponibiliza o commit no canal de testes existente. Sem instalar/reiniciar serviço
nesta sessão. Claude headless: texto, resumo de pensamento separado, input parcial da tool,
rótulo do alvo e objeto final; chamada aparece no transcript e canal limpa. Codex headless e
terminal: prévia separa preâmbulo/resposta, resultado/turn-completed limpa e próximos itens
começam vazios. Claude terminal: conferir comportamento existente como regressão.

Nos fluxos autorizados, confirmar pergunta/permissão/estado/fim de turno e entrega seguinte
sem espera artificial150ms; abrir segundo aparelho; fechar/reabrir chat; confirmar unsubscribe
e fonte recriada; result/interrupção/clear/troca de thread/rename não repõem prévia antiga.
Não alterar sessão de trabalho do dono para fabricar o caso; usar sessão/fixture acordada.

- [x] **Step 3: Repetir as medidas com a semântica final preservada**

Mesma máquina/Python; gerar JSON sintético Write com file_path+content e campo prompt,64/128/200KiB,
chunks128B e prompt200KiB/32B. Registrar bytes e número de pedaços, três repetições/mediana,
parede e CPU do processamento, quantidade de chamadas de `_input_parcial`/publicações e máximo
de uma publicação, com e sem assinante. Separar burst sem relógio avançado de fluxo vivo com
deltas/timer: o primeiro caso limita trabalho inicial/final; o segundo comprova timer de cauda.

Comparar conteúdo e primeira/periódica/final, não velocidade obtida descartando input parcial.
Bench decode-final-only é limite inferior não equivalente de UI; não publicá-lo como ganho real.
Não prometer custo linear de todo full-replace. Medir atraso do loop no mesmo cenário (relógio
monotônico), sem atribuir CPU de todos os clientes a uma sessão ou extrapolar para produção.

- [x] **Step 4: Gravar regra e resultado medido**

Texto-base, ajustar só aos valores comprovados:

```markdown
### Deltas Claude/Codex: acumular antes de publicar

Prévia e input em voo acumulam por sessão/geração; publicam primeiro, intermediários em150ms
e último, com limpeza autoritativa cancelando publicação antiga. Estado, permissões e fila
continuam imediatos. Snapshot completo custa seu tamanho; não reconstruir/parsear por delta.
```

Anotar no mesmo doc shape/chunk/bytes/método, valores antes/depois, versão/ambiente e uso real
conferido. Se faltou provider/Windows/caso de lifecycle, deixar a limitação e o Step correspondente
pendentes; não marcar prova inexistente. Só2A foi entregue, não runtime Rust das subpartes seguintes.

- [x] **Step 5: Commit da evidência, sem publicar sem pedido**

```bash
git status --short
git add docs/decisoes/harnesses.md docs/decisoes/plataforma.md
git commit -m "docs(stream): record Claude and Codex coalescing evidence"
git status --short
```

Stagear somente doc que realmente recebeu evidência (omitir plataforma se nada mudou nele).
Spec/plano/análise desta etapa ficam sem commit agora. Push exige pedido separado; não abrir MR.
Ao terminar execução futura, reportar hashes, branch e estado real. Depois elaborar spec/plano2B
com a transferência de controle acordada, sem assumir que este documento já autorizou esse porte.


**Resultado da execução 2A (03/10/2026):** Tasks 1–3 concluídas; Task 4 tem checks focados (200 aprovados) e medição sintética registrados em docs/decisoes/harnesses.md. Step 2 permanece pendente: uso real no canal do dono, sem serviço/CLI iniciado por esta sessão. Não houve push.

**Revisão posterior solicitada pela sessão hangar:** traceback/causa, falha não fatal de preview Codex e preservação do acumulado no rename corrigidos com regressões RED→GREEN. Checks focados: 203 aprovados. Uso real da Task 4 Step 2 continua pendente.

**Revisão de privacidade:** logs dos novos erros de prévia usam apenas tipo e arquivo:linha dos frames, sem mensagem/input_value; diag.erro_campos conferido. Seis regressões RED→GREEN; 206 testes focados aprovados. Uso real continua pendente.
