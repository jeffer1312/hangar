"""Agregador da faixa de estatísticas por sessão (evento SSE `stats`).

Lê o MESMO arquivo que o transcript da sessão já usa (jsonl do Claude, wire.jsonl do
Kimi, session.jsonl do Pi), mas incrementalmente e por conta própria: guarda o offset
de bytes e a cada `collect()` folda só as linhas novas. Nenhum estado de módulo — o
acumulador vive dentro do `merged_events` de uma conexão e morre com ela.

Todos os números de tempo/velocidade são APROXIMADOS (atribuição de gaps entre
timestamps de linhas); o front prefixa "~" neles. Codex fica de fora nesta leva:
o usage dele chega pelo app-server, não por arquivo (v2 = alimentar pelo adapter).

Campos do snapshot (todos opcionais — o front só desenha o que veio):
  turns, steps, llm_ms, tool_ms, in_tok, out_tok, cache_pct, tok_s, ttft_ms
"""
from __future__ import annotations

import json
from pathlib import Path

# Mesmo filtro de linha sintética do parser de chat: eco de /comando, <task-notification> de
# subagente e bloco <system-reminder> chegam como `type: user` SEM isMeta (transcript.py:261).
# Importar em vez de duplicar — duas heurísticas iguais divergem com o tempo.
from app.transcript import _is_command_meta, _strip_meta_blocks

# Gap acima disto não é trabalho, é sessão parada (pane aberto de um dia pro outro).
_GAP_TETO_S = 3600.0
# ponytail: mapa tool_use->ts com teto pra não crescer sem fim numa sessão longa;
# par que nunca fechou (tool abortada) fica de fora da conta e tudo bem.
_TOOLS_PENDENTES_MAX = 200


class _Fold:
    """Estado comum dos três folds; subclasse implementa feed(obj)."""

    def __init__(self) -> None:
        self.turns = 0
        self.steps = 0
        self.llm_ms = 0.0
        self.tool_ms = 0.0
        self.in_tok = 0
        self.out_tok = 0
        self.cache_read_tok = 0
        self.ttft_ms = 0.0
        self.ttft_n = 0
        self._prev_ts: float | None = None      # ts (s) da linha anterior — atribuição de gap
        self._prompt_ts: float | None = None    # user prompt aguardando 1ª resposta (TTFT)
        self._tools: dict[str, float] = {}      # tool_use_id -> ts do disparo

    def feed(self, obj: dict) -> None:  # pragma: no cover - abstrato
        raise NotImplementedError

    # -- helpers comuns -------------------------------------------------------
    def _gap(self, ts: float | None) -> float | None:
        """Gap em s desde a linha anterior (e avança o cursor). None = sem par ou fora do teto."""
        if ts is None:
            return None
        prev, self._prev_ts = self._prev_ts, ts
        if prev is None or ts < prev or ts - prev > _GAP_TETO_S:
            return None
        return ts - prev

    def _tool_start(self, call_id: str | None, ts: float | None) -> None:
        if call_id and ts is not None and len(self._tools) < _TOOLS_PENDENTES_MAX:
            self._tools[call_id] = ts

    def _tool_end(self, call_id: str | None, ts: float | None) -> None:
        t0 = self._tools.pop(call_id, None) if call_id else None
        if t0 is not None and ts is not None and 0 <= ts - t0 <= _GAP_TETO_S:
            self.tool_ms += (ts - t0) * 1000.0

    def _ttft(self, ts: float | None) -> None:
        if self._prompt_ts is not None and ts is not None and 0 <= ts - self._prompt_ts <= _GAP_TETO_S:
            self.ttft_ms += (ts - self._prompt_ts) * 1000.0
            self.ttft_n += 1
        self._prompt_ts = None

    def snapshot(self) -> dict | None:
        if self.steps == 0:
            return None
        out: dict = {"turns": self.turns, "steps": self.steps,
                     "in_tok": self.in_tok, "out_tok": self.out_tok}
        if self.llm_ms > 0:
            out["llm_ms"] = int(self.llm_ms)
            if self.out_tok:
                out["tok_s"] = round(self.out_tok / (self.llm_ms / 1000.0), 1)
        if self.tool_ms > 0:
            out["tool_ms"] = int(self.tool_ms)
        if self.in_tok > 0:
            out["cache_pct"] = round(100.0 * self.cache_read_tok / self.in_tok)
        if self.ttft_n:
            out["ttft_ms"] = int(self.ttft_ms / self.ttft_n)
        return out


class _FoldClaude(_Fold):
    """jsonl do Claude Code: usage completo em cada linha `assistant` (repetido numa linha
    por BLOCO da mesma mensagem -> dedup pelo message.id). Linhas sidechain (subagente)
    contam tokens/steps mas ficam fora de turno/tempo — os timestamps delas se entrelaçam
    com os do principal e envenenariam a atribuição de gap."""

    def __init__(self) -> None:
        super().__init__()
        self._last_msg_id: str | None = None

    def feed(self, obj: dict) -> None:
        t = obj.get("type")
        if t not in ("user", "assistant") or obj.get("isMeta"):
            return
        ts = _iso_ts(obj.get("timestamp"))
        sidechain = bool(obj.get("isSidechain"))

        if t == "assistant":
            msg = obj.get("message") or {}
            mid = msg.get("id")
            usage = msg.get("usage")
            if isinstance(usage, dict) and mid and mid != self._last_msg_id:
                self._last_msg_id = mid
                self.steps += 1
                inp = _int(usage.get("input_tokens"))
                cr = _int(usage.get("cache_read_input_tokens"))
                cc = _int(usage.get("cache_creation_input_tokens"))
                self.in_tok += inp + cr + cc
                self.cache_read_tok += cr
                self.out_tok += _int(usage.get("output_tokens"))
            if not sidechain:
                gap = self._gap(ts)
                if gap is not None:
                    self.llm_ms += gap * 1000.0
                self._ttft(ts)
                for b in _blocks(msg):
                    if b.get("type") == "tool_use":
                        self._tool_start(b.get("id"), ts)
            return

        # t == "user": ou resultado de tool, ou prompt humano de verdade
        if sidechain:
            return
        msg = obj.get("message") or {}
        content = msg.get("content")
        blocks = content if isinstance(content, list) else []
        results = [b for b in blocks if isinstance(b, dict) and b.get("type") == "tool_result"]
        if results:
            for b in results:
                self._tool_end(b.get("tool_use_id"), ts)
            self._gap(ts)            # gap até aqui é espera de tool, não LLM — só avança o cursor
            return
        textos = [content] if isinstance(content, str) else [
            b.get("text") or "" for b in blocks
            if isinstance(b, dict) and b.get("type") == "text"]
        texto = "\n".join(t for t in textos if t)
        # Turno humano = sobra texto depois de tirar o sintético (eco de /comando, aviso de
        # task, system-reminder). Sem isso, cada fim de subagente virava "turno" e sujava o TTFT.
        if texto and not _is_command_meta(texto) and _strip_meta_blocks(texto):
            self.turns += 1
            self._prompt_ts = ts
            self._gap(ts)            # gap até aqui é o usuário pensando — não conta pra ninguém


class _FoldKimi(_Fold):
    """wire.jsonl do Kimi: `usage.record` traz o DELTA por chamada (regra medida no
    costs_sources.linhas_kimi); `llm.request` marca o início da chamada — o par
    sequencial request->record é a duração de LLM. Tools e TTFT vêm dos eventos
    embrulhados em `context.append_loop_event`. Tempo em ms no envelope `time`."""

    def __init__(self) -> None:
        super().__init__()
        self._req_ts: float | None = None      # llm.request aguardando seu usage.record

    def feed(self, obj: dict) -> None:
        t = obj.get("type")
        ts = _ms_ts(obj.get("time"))
        if t == "turn.prompt":
            self.turns += 1
            self._prompt_ts = ts
        elif t == "llm.request":
            self._req_ts = ts
        elif t == "usage.record":
            u = obj.get("usage")
            if isinstance(u, dict):
                self.steps += 1
                cr = _int(u.get("inputCacheRead"))
                self.in_tok += _int(u.get("inputOther")) + cr + _int(u.get("inputCacheCreation"))
                self.cache_read_tok += cr
                self.out_tok += _int(u.get("output"))
            if self._req_ts is not None and ts is not None and 0 <= ts - self._req_ts <= _GAP_TETO_S:
                self.llm_ms += (ts - self._req_ts) * 1000.0
            self._req_ts = None
        elif t == "context.append_loop_event":
            ev = obj.get("event")
            if not isinstance(ev, dict):
                return
            et = ev.get("type")
            if et == "content.part":
                self._ttft(ts)
            elif et == "tool.call":
                self._tool_start(ev.get("id") or ev.get("toolCallId"), ts)
            elif et == "tool.result":
                self._tool_end(ev.get("id") or ev.get("toolCallId"), ts)


class _FoldPi(_Fold):
    """session.jsonl do Pi: uma linha `message` por mensagem, usage completo na de
    assistente (input/output/cacheRead/cacheWrite — regra de SOMA, ver
    costs_sources.linhas_pi). toolCall no bloco do assistente; toolResult tem role
    próprio. Timestamp em ms no `message.timestamp`."""

    def feed(self, obj: dict) -> None:
        if obj.get("type") != "message":
            return
        msg = obj.get("message")
        if not isinstance(msg, dict):
            return
        ts = _ms_ts(msg.get("timestamp"))
        role = msg.get("role")
        if role == "user":
            self.turns += 1
            self._prompt_ts = ts
            self._gap(ts)
        elif role == "assistant":
            u = msg.get("usage")
            if isinstance(u, dict):
                self.steps += 1
                cr = _int(u.get("cacheRead"))
                self.in_tok += _int(u.get("input")) + cr + _int(u.get("cacheWrite"))
                self.cache_read_tok += cr
                self.out_tok += _int(u.get("output"))
            gap = self._gap(ts)
            if gap is not None:
                self.llm_ms += gap * 1000.0
            self._ttft(ts)
            content = msg.get("content")
            for b in (content if isinstance(content, list) else []):
                if isinstance(b, dict) and b.get("type") == "toolCall":
                    self._tool_start(b.get("id"), ts)
        elif role == "toolResult":
            self._tool_end(msg.get("toolCallId"), ts)
            self._gap(ts)            # espera de tool — só avança o cursor


_FOLDS = {"claude": _FoldClaude, "kimi": _FoldKimi, "pi": _FoldPi}


class Accumulator:
    """Fold incremental sobre o arquivo da sessão. `collect()` é SÍNCRONO e faz IO —
    o chamador roda em asyncio.to_thread (regra do incidente do git status no tick)."""

    def __init__(self, provider: str, path: str) -> None:
        self._path = Path(path)
        self._fold: _Fold = _FOLDS[provider]()
        self._provider = provider
        self._offset = 0
        self._resto = b""            # linha parcial no fim do arquivo (escrita em andamento)

    @classmethod
    def for_provider(cls, provider: str, path: str) -> "Accumulator | None":
        if provider not in _FOLDS or not path:
            return None
        return cls(provider, path)

    def collect(self) -> dict | None:
        try:
            size = self._path.stat().st_size
        except FileNotFoundError:
            # Transitório (arquivo ainda não existe / rotação). Outro OSError — permissão,
            # disco — PROPAGA: o stats_pump loga e desliga a faixa, em vez de congelá-la
            # calada nos últimos números válidos pra sempre.
            return self._fold.snapshot()
        if size < self._offset:      # truncou/regravou -> refolda do zero
            self._fold = _FOLDS[self._provider]()
            self._offset = 0
            self._resto = b""
        if size > self._offset:
            with self._path.open("rb") as f:
                f.seek(self._offset)
                data = self._resto + f.read(size - self._offset)
            self._offset = size
            # Última linha pode estar pela metade: guarda pro próximo collect.
            corte = data.rfind(b"\n")
            self._resto = data[corte + 1:] if corte >= 0 else data
            if corte >= 0:
                for raw in data[:corte].split(b"\n"):
                    if not raw.strip():
                        continue
                    try:
                        obj = json.loads(raw)
                    except ValueError:
                        continue
                    if isinstance(obj, dict):
                        self._fold.feed(obj)
        return self._fold.snapshot()


# -- helpers ------------------------------------------------------------------

def _int(v) -> int:
    return v if isinstance(v, int) and not isinstance(v, bool) else 0


def _ms_ts(v) -> float | None:
    return v / 1000.0 if isinstance(v, (int, float)) and not isinstance(v, bool) else None


def _iso_ts(v) -> float | None:
    if not isinstance(v, str):
        return None
    from datetime import datetime, timezone
    try:
        dt = datetime.fromisoformat(v.replace("Z", "+00:00"))
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.timestamp()


def _blocks(msg: dict) -> list[dict]:
    content = msg.get("content")
    return [b for b in content if isinstance(b, dict)] if isinstance(content, list) else []
