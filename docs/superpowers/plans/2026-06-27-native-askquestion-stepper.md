# Native AskUserQuestion stepper — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render Claude Code's multi-question `AskUserQuestion` prompt as a native step-by-step UI in the webui and drive the TUI selection safely (verify-before-submit, fall back to the mirror on any mismatch).

**Architecture:** Backend reads the structured `AskUserQuestion` payload from the transcript jsonl and emits an `ask_question` SSE event. The frontend auto-opens a native stepper, collects answers + a review, and posts them to a new `/answer` endpoint. The backend drives the TUI by key macro from the known initial cursor, then parses the TUI's own Review screen to verify before pressing Submit.

**Tech Stack:** Python 3.14, FastAPI, Pydantic v2, pytest; Svelte 5 + TypeScript.

Spec: `docs/superpowers/specs/2026-06-27-native-askquestion-stepper-design.md` (read it — the "Investigation findings" section is the authoritative driving model: single=Enter+auto-advance, multi=Space toggle + `→` to advance, `→` after last tab opens the Review screen, settle ~0.3s after each key).

Run backend tests from `backend/` with `.venv/bin/python -m pytest`. Frontend check: `cd frontend && npm run check`.

---

## Task 1: Backend — parse AskUserQuestion payload from the jsonl

**Files:**
- Create: `backend/app/askquestion.py` — `parse_ask_question(jsonl: str) -> AskQuestion | None`
- Modify: `backend/app/models.py` — add `AskOption`, `AskQuestionItem`, `AskQuestion` models
- Test: `backend/tests/test_askquestion.py` (create)

- [ ] **Step 1: Models** in `backend/app/models.py` (Pydantic v2, match existing style):
```python
class AskOption(BaseModel):
    label: str
    description: str = ""


class AskQuestionItem(BaseModel):
    header: str
    question: str
    multiSelect: bool = False
    options: list[AskOption]


class AskQuestion(BaseModel):
    questions: list[AskQuestionItem]
```

- [ ] **Step 2: Failing test** `backend/tests/test_askquestion.py`:
```python
import json
from pathlib import Path
from app.askquestion import parse_ask_question


def _jsonl(tmp_path: Path, *lines: dict) -> str:
    p = tmp_path / "s.jsonl"
    p.write_text("".join(json.dumps(o) + "\n" for o in lines), encoding="utf-8")
    return str(p)


def _askq_line(questions):
    return {"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "tool_use", "name": "AskUserQuestion", "input": {"questions": questions}}]}}


def test_parse_returns_latest_askquestion(tmp_path):
    q = [{"header": "Cor", "question": "Escolha", "multiSelect": False,
          "options": [{"label": "A", "description": "op A"}, {"label": "B", "description": ""}]}]
    j = _jsonl(tmp_path, {"type": "user", "message": {"content": "oi"}}, _askq_line(q))
    out = parse_ask_question(j)
    assert out is not None
    assert [it.header for it in out.questions] == ["Cor"]
    assert out.questions[0].options[0].label == "A"
    assert out.questions[0].multiSelect is False


def test_parse_none_when_no_askquestion(tmp_path):
    j = _jsonl(tmp_path, {"type": "assistant", "message": {"role": "assistant",
              "content": [{"type": "tool_use", "name": "Read", "input": {}}]}})
    assert parse_ask_question(j) is None
```
NOTE on "already answered": the SSE layer already knows the live state (`awaiting_input`); detection only fires when the session is awaiting input, so the parser itself stays simple (returns the latest AskUserQuestion tool_use's payload). The *gating* on whether to emit is done in Task 2 using the live `awaiting_input` state.

- [ ] **Step 3: Run, verify fail**
`cd /home/jefferson/pessoal/claude-pocket/backend && .venv/bin/python -m pytest tests/test_askquestion.py -q` → FAIL (no module).

- [ ] **Step 4: Implement** `backend/app/askquestion.py`:
```python
import json
from typing import Optional
from app.models import AskQuestion


def parse_ask_question(jsonl: str) -> Optional[AskQuestion]:
    """Le o jsonl do transcript e devolve o payload do ULTIMO tool_use AskUserQuestion (perguntas
    estruturadas), ou None. Robusto a linhas malformadas. A decisao de SE oferecer (sessao realmente
    aguardando input) fica na camada de estado/SSE; aqui so extrai a estrutura."""
    found = None
    try:
        with open(jsonl, "r", encoding="utf-8", errors="replace") as fh:
            for line in fh:
                line = line.strip()
                if not line or "AskUserQuestion" not in line:
                    continue
                try:
                    obj = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                content = ((obj.get("message") or {}).get("content")) or []
                if not isinstance(content, list):
                    continue
                for block in content:
                    if (isinstance(block, dict) and block.get("type") == "tool_use"
                            and block.get("name") == "AskUserQuestion"):
                        inp = block.get("input") or {}
                        try:
                            found = AskQuestion.model_validate(inp)
                        except Exception:
                            pass
    except OSError:
        return None
    return found
```

- [ ] **Step 5: Run, verify pass.** Commit:
```bash
git add backend/app/askquestion.py backend/app/models.py backend/tests/test_askquestion.py
git commit -m "feat(backend): parse structured AskUserQuestion payload from transcript"
```

---

## Task 2: Backend — emit `ask_question` SSE event when awaiting input on AskUserQuestion

**Files:**
- Modify: `backend/app/sse.py` (emit `ask_question` once when the live state is `awaiting_input` and the latest tool_use is AskUserQuestion)
- Test: `backend/tests/test_sse.py` or `test_askquestion.py` (append)

- [ ] **Step 1: READ `backend/app/sse.py` fully** to see how `merged_events`/the state stream emits events (the `state`/`message`/`preview` SSE event names) and where `awaiting_input` is detected. Mirror that pattern.

- [ ] **Step 2: Failing test** — feed a fake awaiting_input + an AskUserQuestion jsonl through the event generator (or the small helper that decides the event); assert one `event: ask_question` with the questions JSON is emitted, and NOT emitted when state != awaiting_input. Adapt to sse.py's real seam (prefer unit-testing the small decision helper over driving the whole async generator).

- [ ] **Step 3: Implement** — when the monitor reports `awaiting_input` AND the pane footer indicates the tabbed prompt (`Tab/Arrow keys to navigate` present — distinguishes from the single-list menu), call `parse_ask_question(jsonl)`; if it returns questions, emit:
```python
yield {"event": "ask_question", "data": json.dumps(payload.model_dump(), ensure_ascii=False)}
```
Emit it ONCE per prompt instance (track last-emitted so it doesn't repeat every poll). When state leaves `awaiting_input`, reset the tracker.

- [ ] **Step 4: Run tests, verify pass. Commit:**
```bash
git add backend/app/sse.py backend/tests/
git commit -m "feat(sse): emit ask_question event for tabbed AskUserQuestion prompts"
```

---

## Task 3: Backend — driving routine + `/answer` endpoint

**Files:**
- Modify: `backend/app/terminal_input.py` — `answer_questions(name, answers)` (key macro + Review-screen verify)
- Modify: `backend/app/api.py` — `POST /api/sessions/{name}/answer` + `AnswerBody`
- Test: `backend/tests/test_terminal_answer.py` (create)

- [ ] **Step 1: READ** `terminal_input.py` (the `select`/`send_prompt` helpers, `send_keys`, the control-char validation) and how `capture_pane` is read in `tmux.py`/`state.py`.

- [ ] **Step 2: Failing test** `backend/tests/test_terminal_answer.py` — mock `send_keys` (capture the sequence) and the pane reader (return a fake Review screen). Assert:
  - single-select answer `indices=[1]` → sends `Down, Enter`.
  - multi-select `indices=[0,1]` → `Space, Down, Space, Right` (cursor starts at 0).
  - verify: a Review capture matching the requested labels → final `Enter` (Submit). A mismatching Review → `Escape` + raises (API → 409).
```python
from unittest.mock import patch
from app import terminal_input as ti


def test_single_select_macro():
    keys = []
    review = "Review your answers\n ● Q1\n   → B\nReady to submit\n❯ 1. Submit answers\n  2. Cancel\n"
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: review):
        ti.answer_questions("s", [{"kind": "option", "indices": [1], "multi": False, "labels": ["B"]}])
    assert keys[:2] == ["Down", "Enter"]   # Down x1 (index 1) + Enter (auto-avanca)
    assert keys[-1] == "Enter"             # submit confirmado (review bate "B")
```
(Adapt to the real signatures; the chosen `labels` travel with the answer so verify can compare against the Review text.)

- [ ] **Step 3: Implement** `answer_questions(name, answers)` in `terminal_input.py`:
```python
import time

_SETTLE = 0.3  # TUI redesenha com atraso; ler antes disso pega frame velho (race comprovado)


def answer_questions(name: str, answers: list[dict]) -> None:
    """Dirige o prompt tabbed AskUserQuestion a partir do estado INICIAL (cursor aba1/opt0) e CONFERE
    no Review antes do Submit. Mismatch/erro -> Escape (cancela, nao envia) + ValueError (API -> 409).
    Modelo de teclas (ver spec Investigation findings): single = Down*idx + Enter (auto-avanca);
    multi = (Down ate idx + Space) por opcao, depois Right; texto = Down ate 'Type something' +
    Enter + digita + Enter; chat = Down ate 'Chat about this' + Enter."""
    def key(k):
        send_keys(name, k)
        time.sleep(_SETTLE)

    for a in answers:
        kind = a.get("kind")
        if kind == "option" and not a.get("multi"):
            for _ in range(a["indices"][0]):
                key("Down")
            key("Enter")  # single: seleciona + auto-avanca
        elif kind == "option":  # multi
            cur = 0
            for idx in sorted(a["indices"]):
                for _ in range(idx - cur):
                    key("Down")
                cur = idx
                key("Space")
            key("Right")  # avanca (multi nao auto-avanca)
        elif kind == "text":
            for _ in range(a["type_index"]):
                key("Down")
            key("Enter")
            if any(ord(c) < 32 and c not in "\t" for c in a["value"]):
                raise ValueError("control characters not allowed")
            send_keys(name, a["value"], literal=True); time.sleep(_SETTLE)
            key("Enter")
        elif kind == "chat":
            for _ in range(a["chat_index"]):
                key("Down")
            key("Enter")

    # Review screen: confere as escolhas antes do Submit irreversivel.
    screen = _capture(name)
    if "Submit answers" not in screen or not _review_matches(screen, answers):
        send_keys(name, "Escape")
        raise ValueError("review mismatch — nao submetido")
    key("Enter")  # ❯ 1. Submit answers


def _review_matches(screen: str, answers: list[dict]) -> bool:
    # Cada pergunta no review vira "→ <labels juntos por ', '>". Confere que todo label escolhido
    # aparece numa linha "→" do review. (Texto livre/chat: sem label -> pula a checagem estrita.)
    for a in answers:
        for lbl in a.get("labels", []):
            if not any(line.strip().startswith("→") and lbl in line for line in screen.splitlines()):
                return False
    return True
```
Add `_capture(name)` reusing the existing capture-pane helper from tmux/state. Reuse the module's `send_keys` import.

- [ ] **Step 4: Endpoint** in `api.py`:
```python
class AnswerItem(_StrictBody):
    kind: str
    indices: list[int] | None = None
    multi: bool = False
    value: str | None = None
    labels: list[str] = []
    type_index: int | None = None
    chat_index: int | None = None


class AnswerBody(_StrictBody):
    answers: list[AnswerItem]


@app.post("/api/sessions/{name}/answer", dependencies=[Depends(require_auth)])
def answer(name: str, body: AnswerBody):
    try:
        terminal.answer_questions(name, [a.model_dump() for a in body.answers])
    except ValueError as e:
        raise HTTPException(409, str(e))
    return {"ok": True}
```

- [ ] **Step 5: Run tests, verify pass. Commit:**
```bash
git add backend/app/terminal_input.py backend/app/api.py backend/tests/test_terminal_answer.py
git commit -m "feat(backend): drive + verify AskUserQuestion answers via /answer endpoint"
```

---

## Task 4: Frontend — types + api helper

**Files:**
- Modify: `frontend/src/lib/types.ts` — `AskQuestionPayload`, `AskOption`, `AskQuestionItem`, `AnswerItem`
- Modify: `frontend/src/lib/api.ts` — `answerQuestions(name, answers)`
- Verify: `cd frontend && npm run check`

- [ ] **Step 1: Types** (`types.ts`):
```typescript
export interface AskOption { label: string; description: string }
export interface AskQuestionItem { header: string; question: string; multiSelect: boolean; options: AskOption[] }
export interface AskQuestionPayload { questions: AskQuestionItem[] }
export type AnswerItem =
  | { kind: 'option'; indices: number[]; multi: boolean; labels: string[] }
  | { kind: 'text'; value: string; type_index: number; labels: string[] }
  | { kind: 'chat'; chat_index: number };
```

- [ ] **Step 2: api helper** (`api.ts`, match the existing `apiFetch`/error pattern):
```typescript
export function answerQuestions(name: string, answers: AnswerItem[]): Promise<{ ok: boolean }> {
  return apiFetch(`/api/sessions/${encodeURIComponent(name)}/answer`, {
    method: 'POST', body: JSON.stringify({ answers }),
  });
}
```

- [ ] **Step 3: `npm run check`** — no new errors. Commit:
```bash
git add frontend/src/lib/types.ts frontend/src/lib/api.ts
git commit -m "feat(frontend): types + answerQuestions api for AskUserQuestion"
```

---

## Task 5: Frontend — `AskQuestionSheet` component + wiring

**Files:**
- Create: `frontend/src/components/AskQuestionSheet.svelte`
- Modify: `frontend/src/screens/Chat.svelte` — listen for the `ask_question` SSE event, open the sheet, call `answerQuestions`, on 409 open the mirror
- Verify: `cd frontend && npm run check`

- [ ] **Step 1: READ** `Chat.svelte`'s `connectSSE` (where `message`/`state`/`preview` listeners are added) and the existing sheet pattern (`BottomSheet`, `SessionSwitcherSheet`) + `openMirror`.

- [ ] **Step 2: SSE listener** in `connectSSE` (next to the others):
```javascript
es.addEventListener('ask_question', (e) => {
  try { askPayload = JSON.parse(e.data); askOpen = true; } catch {}
});
```
Add `let askPayload = $state<AskQuestionPayload | null>(null); let askOpen = $state(false);` and render `<AskQuestionSheet open={askOpen} payload={askPayload} onSubmit={handleAnswer} onClose={() => askOpen=false} onFallback={openMirror} />`. `handleAnswer(answers)` calls `answerQuestions(sessionName, answers)`; on success close; on error (409) `askOpen=false; openMirror()`.

- [ ] **Step 3: Component** `AskQuestionSheet.svelte` — Svelte 5, uses `BottomSheet`. State: `step` index, `picks` (per question: `number[]` for option(s) | `{text}` | `{chat}`). Render:
  - step < questions.length: show `questions[step].header` (title), `.question`, each `.options[i]` as a button (description below). multiSelect → checkboxes + "Próximo"; single → tap sets pick + `step++`. Plus "✎ Digitar resposta" (opens a text field) and "💬 Conversar sobre isso".
  - step === questions.length: **review** — list each question → chosen label(s); buttons **Enviar** and **Cancelar** (`onClose`).
  Build each `AnswerItem` from `picks`. For `option`: `{kind:'option', indices, multi: q.multiSelect, labels: indices.map(i=>q.options[i].label)}`. `type_index` = `q.options.length`, `chat_index` = `q.options.length + 1`.

- [ ] **Step 4: `npm run check`** — no new errors. Commit:
```bash
git add frontend/src/components/AskQuestionSheet.svelte frontend/src/screens/Chat.svelte
git commit -m "feat(frontend): native AskUserQuestion stepper sheet + wiring"
```

---

## Task 6: Full verification + manual smoke

- [ ] **Step 1: Backend suite** `cd backend && .venv/bin/python -m pytest -q` — new tests pass; the 2 pre-existing `test_state_classifier` failures remain (unrelated).
- [ ] **Step 2: ruff** `cd backend && uvx ruff check app/` — clean.
- [ ] **Step 3: Restart** (kill by port pid + `setsid` relaunch preserving `CLAUDE_CONFIG_DIR`, NOT `pkill -f app.main`; `curl --retry --retry-connrefused` to wait — startup race is slow).
- [ ] **Step 4: Manual smoke** — create a throwaway session, induce a 2-question AskUserQuestion (one single, one multi) like the investigation did, answer it via the app stepper, and confirm: the stepper auto-opens, the review shows the picks, Enviar submits, Claude proceeds (the prompt clears, no "User declined"). Then force a mismatch path once (verify it Escapes + opens the mirror). Kill the throwaway.

---

## Self-review

- **Spec coverage:** detection (T1), event (T2), drive+verify+endpoint (T3), types/api (T4), stepper+wiring (T5), tests throughout, smoke (T6). All mapped.
- **Type consistency:** `AskQuestionItem{header,question,multiSelect,options[{label,description}]}` identical in models.py / types.ts. `AnswerItem` kinds (option/text/chat) identical across api.py body, terminal_input driving, types.ts, and the sheet's builder. `type_index = options.length`, `chat_index = options.length+1` consistent between frontend builder and backend driving.
- **The risky part (driving) is anchored** to the empirically-confirmed model in the spec; the Review-screen verify (T3) is the safety net — a wrong drive Escapes and 409s, never submits, and the frontend falls back to the mirror (T5).
