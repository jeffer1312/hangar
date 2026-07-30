# Progresso do plano em execução — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** mostrar, no card de cada sessão e no painel da direita, em que pé está o plano do
superpowers que aquela sessão está executando — Task atual, steps feitos/total, próximo passo e o
que exige conferência humana.

**Architecture:** o backend lê o `.md` do plano no repo da sessão (`app/planprog.py`, cache por
`st_mtime_ns` + cache de descoberta por cwd com TTL de 3 s), decora `SessionInfo` dentro do
`asyncio.to_thread` que já existe pro git, e publica os campos em `/api/sessions` e no `_list_sig`.
O front ganha `lib/plan.ts` (puro, testado), `PlanBar.svelte` (barra segmentada) e
`PlanPanel.svelte` (Tasks + steps), plugados nos três cards e no painel desktop / `ActivitySheet`
mobile.

**Tech Stack:** Python 3.14 + FastAPI + pytest (backend); Svelte 5 (runes) + TypeScript + vitest +
svelte-check (frontend).

**Spec:** `docs/superpowers/specs/2026-07-30-progresso-do-plano-design.md`

**Revisão:** este plano passou por pass adversarial (`ecc:architect` + `Explore`) e foi corrigido em
17 pontos. Os que mudaram decisão estão marcados **[adv]**.

## Global Constraints

- **Worktree:** todo o trabalho acontece em `/home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress`, branch `feat/progresso-do-plano`. **Nunca** commitar na `main`; **nunca** fazer merge (o usuário faz).
- **Baseline medido no worktree antes de começar:** backend `1118 passed, 1 skipped`; front `466 FILES 0 ERRORS 0 WARNINGS`.
- **[adv] Nunca prometer contagem absoluta de testes.** Rodar `pytest -q` no início da task, anotar o número, e ao final exigir "esse número + os testes novos, zero falha". Contagem cravada no plano vira falso alarme de regressão.
- **Backend não pode ler markdown na corrotina.** A decoração roda **dentro** da função `_decorate_git`, que já está em `asyncio.to_thread` (`backend/app/registry.py:829`). O incidente de 2026-07-23 (git status no tick do SSE) é o precedente.
- **Nada pode levantar exceção pro tick da lista.** `plan_progress()` devolve `None` em qualquer erro e loga `warning` — nunca propaga.
- **`st_mtime_ns`, nunca `st_mtime`+`size`:** marcar `- [ ]` → `- [x]` preserva o tamanho do arquivo.
- **Duas views sempre.** Toda mudança de UI entra no caminho mobile **e** no desktop, e a verificação manual testa os dois. `Sidebar.svelte` (desktop) e `SessionCard.svelte` (mobile) são arquivos separados que derivam.
- **`npm --prefix frontend run check` é o gate de tipos** — `build` não tipa. Testes: `npm run test` (vitest).
- **Sempre caminho absoluto no shell** (`npm --prefix /caminho/...`): o cwd persiste entre chamadas e um `cd` vaza pro comando seguinte.
- Comentários de código em pt-BR, no estilo dos arquivos vizinhos (explicam *por que*, não *o que*). Identificadores e mensagens de commit em inglês.
- **Nunca `git add -A`/`git add .`** — stagear por caminho explícito.

## Fatos do codebase confirmados (não re-descobrir)

| Coisa | Verdade |
|---|---|
| `_decorate_git` | `backend/app/registry.py:821`, chamado em `:829` via `asyncio.to_thread` |
| `_decorate_loop` | `registry.py:49`, chamado em `:830-831` **na corrotina** (não usar como modelo de I/O) |
| `_list_sig` | `backend/app/sse.py:162` |
| `SessionInfo.loop_max` | `backend/app/models.py:97` |
| Rotas do `api.py` | `@app.get("/api/...", dependencies=[Depends(require_auth)])`. **Não existe `router`/`APIRouter`** |
| cwd por nome de sessão | `_session_cwd(name)` em `api.py:1845` — já levanta 404. **`registry.get()` NÃO existe** |
| Fixture de teste de rota | `api_client` em `tests/test_api.py:57`, com `headers=_h()` explícito. A fixture `client` é um app descartável só com `/ping` — **não serve** |
| `StateMonitor` | `state.py:209`; `__init__` só guarda `name`, `poll`, `hook_grace`, `sid_get`. **Não tem `cwd`** |
| Prop de viewport no `Chat` | `desktop` (`Chat.svelte:53`), **não** `isDesktop` |
| Props do `DesktopSessionContext` | escalares (`sessionName`, `serverLabel`, `provider`, …). **Não recebe `session`** |
| Multi-servidor no `lib/api.ts` | par `getX(name)` / `getXForServer(s: Server, name)` |
| `renderMarkdown` | `(input: string, opts?) => string` |
| Planos presentes neste worktree | só os 5 `2026-07-29-git-*.md` (o `pi-adapter` é untracked e **não está aqui**) |

## Decisões tomadas sem o usuário (ele não está disponível)

Repetidas no relatório final. Formato: **decisão** — alternativa descartada.

1. **Worktree ignorado via `.git/info/exclude`**, não no `.gitignore` versionado. Alternativa: commitar a linha (mexeria na `main`, não autorizado).
2. **Base do worktree = HEAD local (`abf9a2b`)**, não `origin/main` (o local está 3 commits à frente).
3. **[adv] `plan_*` NÃO vai no `StateEvent`** — só em `SessionInfo`/`/api/sessions`. Reverte a decisão anterior. Motivo: `StateMonitor` não tem cwd, e passá-lo mexeria no Adapter Protocol (`adapters/__init__.py`, `claude.py:26`, `pi/adapter.py:33`) e ainda deixaria o Codex de fora (ele tem stream próprio). O painel é desktop-only e o desktop já recebe a lista agregada. Alternativa: uma task inteira só pra isso — desproporcional.
4. **Sessão não-Claude (Codex/Pi) é decorada igual** (a decoração é por cwd, na lista). Alternativa: gate em `provider == "claude"`.
5. **[adv] Expiração de 14 dias fica, com teto declarado.** `git checkout`/`worktree add` reescrevem mtime, então ela não detecta abandono de verdade — só evita que um plano de meses atrás reapareça. Comentário `# ponytail:` no código diz isso.
6. **[adv] `_plans_dir` para no primeiro diretório com `.git`.** Sem isso, um worktree sem planos subiria até o checkout principal e mostraria **o plano de outro trabalho** — "sem barra" é limitação aceitável, "barra errada" é bug.
7. **[adv] A regra de processo NÃO edita `~/.claude/CLAUDE.md`** (arquivo global do usuário, fora do repo). Vai no `CLAUDE.md` do repo, e o texto pronto pro global fica no relatório final pra ele colar. Alternativa: editar sozinho a config global dele.

## File Structure

| Arquivo | Responsabilidade |
|---|---|
| `backend/app/planprog.py` (novo) | descobrir o plano ativo de um cwd, parsear Tasks/Steps, cachear. Única peça que conhece o formato do markdown. |
| `backend/tests/test_planprog.py` (novo) | testes do acima, incluindo os modos de falha de I/O. |
| `backend/app/models.py` (mod) | campos `plan_*` em `SessionInfo`. |
| `backend/app/registry.py` (mod) | `_decorate_plan` chamado dentro do `to_thread` do git. |
| `backend/app/sse.py` (mod) | `plan_name`/`plan_done`/`plan_total` no `_list_sig`. |
| `backend/app/api.py` (mod) | `GET /api/sessions/{name}/plan`. |
| `frontend/src/lib/plan.ts` + `.test.ts` (novos) | `planBadge()` puro. |
| `frontend/src/components/PlanBar.svelte` (novo) | a barra (segmentada / única / compact). |
| `frontend/src/components/PlanPanel.svelte` (novo) | Tasks + steps + markdown do plano. |
| `frontend/src/lib/types.ts` (mod) | `plan_*` em `SessionInfo`; `PlanDetail`. |
| `frontend/src/lib/api.ts` (mod) | `getPlan` / `getPlanForServer`. |
| `frontend/src/components/{Sidebar,SessionCard,BoardCard}.svelte` (mod) | chip + `PlanBar`. |
| `frontend/src/components/DesktopSessionContext.svelte` (mod) | prop `session` + seção "Plano". |
| `frontend/src/components/ActivitySheet.svelte` (mod) | `PlanPanel` no topo, atrás da prop `showPlan`. |
| `frontend/src/screens/Chat.svelte` (mod) | passa `session` ao painel e `showPlan={!desktop}` à sheet. |

---

### Task 1: `planprog.py` — descoberta, parse e cache

**Files:**
- Create: `backend/app/planprog.py`
- Test: `backend/tests/test_planprog.py`

**Interfaces:**
- Consumes: nada (stdlib apenas — sem import de `app.config`, mesmo padrão do `engines.py`).
- Produces:
  - `plan_progress(cwd: str | None) -> PlanProgress | None`
  - `parse_plan(path: str) -> PlanProgress | None`
  - `PlanProgress` (frozen): `name, path, task_idx, task_total, done, total, complete, tasks`
  - `TaskProgress` (frozen): `title, done, total, steps`
  - `StepProgress` (frozen): `title, done, manual`
  - `_reset_caches()`, `_invalidate_discovery()` — só pra teste.

- [x] **Step 0: Anotar o baseline desta task**

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest -q 2>&1 | tail -2`
Anotar o número exato (esperado: `1118 passed, 1 skipped`). Esse é o número a comparar no fim.

- [x] **Step 1: Escrever os testes que falham**

Criar `backend/tests/test_planprog.py`. **Atenção:** o `PLAN_A` abaixo contém um bloco cercado por
crases dentro da string — é de propósito, é o caso do fence (Bloqueante 1 do pass adversarial).

```python
import os
import time
from pathlib import Path

import pytest

from app import planprog
from app.planprog import plan_progress


PLAN_A = """# Plano de teste — Implementation Plan

## Global Constraints

- [x] isto NAO e um step (checkbox fora de Step)

### Task 0: Medir o que o resto assume

- [x] **Step 1: Medir**
- [x] **Step 2: Anotar**

### Task 1: Fazer a coisa

- [x] **Step 1: Teste que falha**
- [ ] **Step 2: Implementar**
- [ ] **Step 3: Gate de tipos + verificação manual (mobile E desktop)**

Exemplo de codigo que o executor deve colar (NAO sao steps de verdade):

```python
# ### Task 9: fantasma
# - [x] **Step 1: nao conta**
# - [x] **Step 2: nao conta**
```
"""


def _write(tmp_path, name, body):
    d = tmp_path / "docs" / "superpowers" / "plans"
    d.mkdir(parents=True, exist_ok=True)
    (tmp_path / ".git").mkdir(exist_ok=True)      # _plans_dir para no 1o dir com .git
    p = d / name
    p.write_text(body, encoding="utf-8")
    return p


@pytest.fixture(autouse=True)
def _clean():
    planprog._reset_caches()
    yield
    planprog._reset_caches()


def test_parcial_e_ignora_fence(tmp_path):
    _write(tmp_path, "2026-07-29-plano-de-teste.md", PLAN_A)
    r = plan_progress(str(tmp_path))
    assert r is not None
    # 5 steps reais; os 2 do bloco de codigo e o "[x]" fora de Step nao contam
    assert (r.done, r.total) == (3, 5)
    assert r.task_total == 2          # "Task 9" mora dentro do fence
    assert r.task_idx == 2            # ordinal: "Task 0" e a 1a
    assert r.name == "plano-de-teste"
    assert r.complete is False
    assert r.tasks[0].title == "Task 0: Medir o que o resto assume"
    assert (r.tasks[0].done, r.tasks[0].total) == (2, 2)
    assert r.tasks[1].steps[2].manual is True
    assert r.tasks[1].steps[0].manual is False


def test_so_fence_marcado_nao_elege(tmp_path):
    """O caso que envenenava o proprio plano: exemplo de step marcado dentro de bloco de codigo."""
    body = (
        "### Task 1: X\n\n- [ ] **Step 1: A**\n\n"
        "```python\n- [x] **Step 1: exemplo**\n```\n"
    )
    _write(tmp_path, "2026-07-29-so-exemplo.md", body)
    assert plan_progress(str(tmp_path)) is None


def test_sem_step_marcado_nao_elege(tmp_path):
    _write(tmp_path, "2026-07-29-so-escrito.md",
           "### Task 1: X\n\n- [ ] **Step 1: A**\n- [ ] **Step 2: B**\n")
    assert plan_progress(str(tmp_path)) is None


def test_checkbox_fora_de_step_nao_elege(tmp_path):
    _write(tmp_path, "2026-07-29-falso.md",
           "## Criterios\n\n- [x] criterio atendido\n\n### Task 1: X\n\n- [ ] **Step 1: A**\n")
    assert plan_progress(str(tmp_path)) is None


def test_sem_step_nenhum_devolve_none(tmp_path):
    _write(tmp_path, "2026-07-29-vazio.md", "### Task 1: X\n\ntexto solto\n")
    assert plan_progress(str(tmp_path)) is None


def test_completo(tmp_path):
    _write(tmp_path, "2026-07-29-fechado.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [x] **Step 2: B**\n")
    r = plan_progress(str(tmp_path))
    assert r is not None and r.complete is True
    assert (r.done, r.total) == (2, 2)
    assert r.task_idx == 1        # sem pendencia: aponta pra ultima Task


def test_prefere_plano_com_step_pendente(tmp_path):
    _write(tmp_path, "2026-07-29-andando.md",
           "### Task 1: Y\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    time.sleep(0.02)
    _write(tmp_path, "2026-07-29-fechado.md",       # mais NOVO, porem concluido
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [x] **Step 2: Z**\n")
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "andando"


def test_entre_dois_pendentes_vence_o_mais_novo(tmp_path):
    """Fixa a DIREcao do sort. Sem este teste, inverter o reverse=True passava despercebido."""
    _write(tmp_path, "2026-07-29-velho.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    time.sleep(0.02)
    _write(tmp_path, "2026-07-29-novo.md",
           "### Task 1: Y\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "novo"


def test_sticky_mantem_plano_quando_outro_e_reescrito(tmp_path):
    _write(tmp_path, "2026-07-29-executando.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    first = plan_progress(str(tmp_path))
    assert first is not None and first.name == "executando"
    time.sleep(0.02)
    _write(tmp_path, "2026-07-29-outro.md",
           "### Task 1: Y\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n- [ ] **Step 3: C**\n")
    planprog._invalidate_discovery()     # simula o TTL vencendo, sem sleep de 3s
    again = plan_progress(str(tmp_path))
    assert again is not None and again.name == "executando"


def test_plano_velho_nao_elege(tmp_path):
    p = _write(tmp_path, "2026-06-01-antigo.md",
               "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    old = time.time() - 15 * 86400
    os.utime(p, (old, old))
    assert plan_progress(str(tmp_path)) is None


def test_sobe_ate_a_raiz_do_repo(tmp_path):
    _write(tmp_path, "2026-07-29-plano.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    sub = tmp_path / "frontend" / "src" / "lib"
    sub.mkdir(parents=True)
    r = plan_progress(str(sub))
    assert r is not None and r.name == "plano"


def test_para_no_repo_de_dentro_e_nao_vaza_pro_de_fora(tmp_path):
    """Worktree sem planos NAO pode mostrar o plano do checkout principal."""
    _write(tmp_path, "2026-07-29-de-fora.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    dentro = tmp_path / "sub" / "wt"
    dentro.mkdir(parents=True)
    (dentro / ".git").write_text("gitdir: /qualquer\n", encoding="utf-8")   # worktree: .git e ARQUIVO
    assert plan_progress(str(dentro)) is None


def test_cwd_none_e_sem_pasta(tmp_path):
    assert plan_progress(None) is None
    assert plan_progress("") is None
    assert plan_progress(str(tmp_path)) is None


def test_cache_por_mtime_ns(tmp_path):
    """_load e cacheado por mtime_ns: mesmo path + mesmo ns = nao rele; ns diferente = rele."""
    p = _write(tmp_path, "2026-07-29-plano.md",
               "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    first = planprog._load(str(p), 111)
    assert first is not None and first.done == 1
    # marcar troca "[ ]" por "[x]" — MESMO tamanho de arquivo
    p.write_text(p.read_text(encoding="utf-8").replace("- [ ] **Step 2", "- [x] **Step 2"),
                 encoding="utf-8")
    assert planprog._load(str(p), 111).done == 1       # mesmo ns: serve o cache
    assert planprog._load(str(p), 222).done == 2       # ns novo: rele


def test_um_plano_ilegivel_nao_derruba_os_outros(tmp_path, monkeypatch):
    bom = _write(tmp_path, "2026-07-29-bom.md",
                 "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    _write(tmp_path, "2026-07-29-ruim.md", "### Task 1: Y\n\n- [x] **Step 1: A**\n")
    real = planprog.Path.read_bytes

    def _seletivo(self, *a, **k):
        if self.name.endswith("ruim.md"):
            raise PermissionError("negado")
        return real(self, *a, **k)

    monkeypatch.setattr(planprog.Path, "read_bytes", _seletivo)
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "bom"       # o ilegivel nao mata a feature do repo


def test_todos_ilegiveis_devolve_none(tmp_path, monkeypatch):
    _write(tmp_path, "2026-07-29-plano.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")

    def _boom(self, *a, **k):
        raise FileNotFoundError("sumiu")

    monkeypatch.setattr(planprog.Path, "read_bytes", _boom)
    assert plan_progress(str(tmp_path)) is None


def test_formato_real_dos_planos_do_repo():
    """Fixture viva contra deriva de formato: um plano TRACKED deste worktree ainda casa o regex.
    Nao afirma progresso (os git-* tem 0 marcados) — afirma que o formato nao mudou."""
    root = Path(__file__).resolve().parents[2]
    alvo = root / "docs" / "superpowers" / "plans" / "2026-07-29-git-log-hub.md"
    if not alvo.exists():
        pytest.skip("plano ausente neste checkout")
    raw = alvo.read_text(encoding="utf-8")
    assert len(planprog._STEP_RE.findall(raw)) >= 15
    assert len(planprog._TASK_RE.findall(raw)) >= 2
```

- [x] **Step 2: Rodar e ver falhar**

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest tests/test_planprog.py -q`
Expected: FAIL — `ModuleNotFoundError: No module named 'app.planprog'`

- [x] **Step 3: Implementar `backend/app/planprog.py`**

```python
"""Progresso do plano do superpowers que uma sessao esta executando.

Le o .md do plano no repo do cwd da sessao e conta os steps marcados. Roda por sessao a cada poll
da lista, entao TUDO aqui e cacheado e NADA levanta: o unico consumidor e o tick que alimenta o
SSE, e uma excecao ali derruba a lista inteira (incidente de 2026-07-23 com o git status).

Stdlib apenas — sem import de app.config, mesmo motivo do engines.py.
"""

from __future__ import annotations

import logging
import os
import re
import time
from dataclasses import dataclass
from pathlib import Path

_log = logging.getLogger("claude_pocket.planprog")

PLANS_REL = os.path.join("docs", "superpowers", "plans")

# Quantos niveis subir a partir do cwd da pane procurando a raiz do repo. `#{pane_current_path}`
# segue o `cd` do usuario, entao um `cd frontend/src` nao pode fazer o plano sumir da UI. A subida
# PARA no primeiro diretorio com .git: sem isso, um worktree em .claude/worktrees/<x> sem planos
# subiria ate o checkout principal e mostraria o plano de OUTRO trabalho. "Sem barra" e limitacao;
# "barra errada" e bug.
_MAX_PARENTS = 6

# Plano parado ha mais de 14 dias nao reaparece.
# ponytail: mtime nao mede abandono de verdade — git checkout/worktree add reescrevem o mtime de
# todos os planos. Isto so evita que um plano de meses atras ressuscite; nao promete mais que isso.
_MAX_AGE_S = 14 * 86400

# UM regex pros dois usos (descoberta e parse). Quando a descoberta procurava "- [x]" solto e o
# parse so contava Step, um checkbox de checklist elegia um plano que depois lia 0/N e escondia o
# plano real.
_STEP_RE = re.compile(r"^- \[([ xX])\] \*\*(Step\b[^*]*)\*\*", re.M)
_TASK_RE = re.compile(r"^### (Task\b[^\n]*)$", re.M)
# Bloco cercado (``` ou ~~~). Planos MOSTRAM steps de exemplo dentro de bloco de codigo — sem tirar
# isto, um plano recem-escrito ja nasce com "3/47 feitos" e acende a barra antes de comecar
# (medido no proprio plano deste trabalho: 47 casados vs 42 reais, 3 "marcados" vs 0).
_FENCE_RE = re.compile(r"^(```|~~~).*?^\1[^\n]*$", re.M | re.S)
_MANUAL_RE = re.compile(r"verifica(?:ção|cao|r)\s+manual|manual\s+verification", re.I)
_DATE_PREFIX_RE = re.compile(r"^\d{4}-\d{2}-\d{2}-")


@dataclass(frozen=True)
class StepProgress:
    title: str
    done: bool
    manual: bool


@dataclass(frozen=True)
class TaskProgress:
    title: str
    done: int
    total: int
    steps: tuple[StepProgress, ...]


@dataclass(frozen=True)
class PlanProgress:
    name: str
    path: str
    task_idx: int      # posicao ORDINAL (1-based) da 1a Task com step pendente
    task_total: int
    done: int
    total: int
    complete: bool
    tasks: tuple[TaskProgress, ...]


# path -> (mtime_ns, PlanProgress | None). O None memoriza "li e nao serve" — e o que impede reler
# os candidatos sem marcacao a cada poll (neste repo, ~2.5k linhas por sessao por poll).
_file_cache: dict[str, tuple[int, PlanProgress | None]] = {}
# raiz -> (ts, path | None). Mesmo padrao do _summary_cache do git_ops: com N sessoes no mesmo repo,
# o scandir roda 1x e nao N.
_discovery_cache: dict[str, tuple[float, str | None]] = {}
_DISCOVERY_TTL = 3.0
# raiz -> path do plano eleito no ciclo anterior. Enquanto ele tiver step pendente, continua eleito:
# sem isto, o writing-plans reescrevendo OUTRO plano rouba o posto e o progresso pula 9/17 -> 3/56.
_sticky: dict[str, str] = {}


def _reset_caches() -> None:
    """So pra teste."""
    _file_cache.clear()
    _discovery_cache.clear()
    _sticky.clear()


def _invalidate_discovery() -> None:
    """So pra teste: simula o TTL da descoberta vencendo, sem sleep de 3s."""
    _discovery_cache.clear()


def _plans_dir(cwd: str) -> str | None:
    """Sobe ate _MAX_PARENTS niveis procurando docs/superpowers/plans, PARANDO no primeiro nivel que
    tenha .git (raiz do repo ou do worktree)."""
    cur = Path(cwd)
    for _ in range(_MAX_PARENTS + 1):
        cand = cur / PLANS_REL
        if cand.is_dir():
            return str(cand)
        if (cur / ".git").exists():
            return None        # raiz do repo alcancada e sem planos: nao vaza pro repo de fora
        if cur.parent == cur:
            break
        cur = cur.parent
    return None


def parse_plan(path: str) -> PlanProgress | None:
    """Parseia UM plano. None se nao tem step nenhum ou nenhum step marcado. Pode levantar OSError
    (quem chama trata) — so o I/O levanta; markdown nao falha ao parsear."""
    # read_bytes de uma vez (nao linha a linha): o Edit trunca e reescreve, e um poll no meio da
    # escrita leria menos steps -> a sig cai e sobe = piscada em todas as views ao mesmo tempo.
    raw = Path(path).read_bytes().decode("utf-8", errors="replace")
    # Neutraliza os blocos de codigo PRESERVANDO offsets (troca cada char por espaco, menos \n):
    # as fronteiras de Task sao calculadas por posicao, entao remover texto quebraria o recorte.
    raw = _FENCE_RE.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), raw)

    steps = [(m.start(), m.group(1) != " ", m.group(2).strip()) for m in _STEP_RE.finditer(raw)]
    if not steps:
        return None
    done = sum(1 for _, ok, _ in steps if ok)
    if done == 0:
        return None   # escrito mas nunca comecado: nao acende barra

    heads = [(m.start(), m.group(1).strip()) for m in _TASK_RE.finditer(raw)]
    if not heads:
        heads = [(0, "Task 1")]

    tasks: list[TaskProgress] = []
    for i, (pos, title) in enumerate(heads):
        end = heads[i + 1][0] if i + 1 < len(heads) else len(raw)
        mine = [s for s in steps if pos <= s[0] < end]
        tasks.append(TaskProgress(
            title=title,
            done=sum(1 for _, ok, _ in mine if ok),
            total=len(mine),
            steps=tuple(StepProgress(title=t, done=ok, manual=bool(_MANUAL_RE.search(t)))
                        for _, ok, t in mine),
        ))

    # ORDINAL, nao o N do heading: existe "### Task 0" nos planos (pi-adapter), e "Task 0/6" no chip
    # seria mentira. O painel mostra o titulo literal.
    task_idx = next((i + 1 for i, t in enumerate(tasks) if t.done < t.total), len(tasks))
    name = _DATE_PREFIX_RE.sub("", Path(path).stem)
    return PlanProgress(name=name, path=path, task_idx=task_idx, task_total=len(tasks),
                        done=done, total=len(steps), complete=done == len(steps),
                        tasks=tuple(tasks))


def _load(path: str, mtime_ns: int) -> PlanProgress | None:
    hit = _file_cache.get(path)
    if hit is not None and hit[0] == mtime_ns:
        return hit[1]
    got = parse_plan(path)
    _file_cache[path] = (mtime_ns, got)
    return got


def _discover(root: str) -> str | None:
    """Path do plano ativo em `root` (dir de planos), ou None. Preferencia: plano com step pendente;
    entre eles, o mtime mais novo. Sticky: o eleito anterior mantem o posto enquanto tiver pendencia."""
    now_wall = time.time()
    cands: list[tuple[float, str, PlanProgress]] = []
    with os.scandir(root) as it:
        for e in it:
            if not e.name.endswith(".md") or not e.is_file():
                continue
            try:
                st = e.stat()
                if now_wall - st.st_mtime > _MAX_AGE_S:
                    continue
                got = _load(e.path, st.st_mtime_ns)
            except OSError:
                # UM arquivo ilegivel nao pode matar a feature do repo inteiro: sem este continue,
                # a excecao subiria ao except de plan_progress e devolveria None pra TODAS as
                # sessoes daquele repo, a cada poll, pra sempre.
                continue
            if got is not None:
                cands.append((st.st_mtime, e.path, got))
    if not cands:
        return None

    prev = _sticky.get(root)
    for _, path, got in cands:
        if path == prev and not got.complete:
            return path        # sticky: quem esta andando nao perde o posto

    pend = [c for c in cands if not c[2].complete]
    pool = pend or cands
    pool.sort(key=lambda c: c[0], reverse=True)      # mais NOVO primeiro
    chosen = pool[0][1]
    _sticky[root] = chosen
    return chosen


def plan_progress(cwd: str | None) -> PlanProgress | None:
    """Progresso do plano ativo do repo em `cwd`, ou None. NUNCA levanta."""
    try:
        if not cwd:
            return None
        root = _plans_dir(cwd)
        if root is None:
            return None
        now = time.monotonic()
        hit = _discovery_cache.get(root)
        if hit is not None and now - hit[0] < _DISCOVERY_TTL:
            path = hit[1]
        else:
            path = _discover(root)
            _discovery_cache[root] = (now, path)
        if path is None:
            return None
        try:
            mtime_ns = os.stat(path).st_mtime_ns
        except OSError:
            return None
        return _load(path, mtime_ns)
    except Exception:
        # Rede de seguranca. Os modos de falha reais sao I/O e ja sao tratados por arquivo, no
        # _discover — markdown malformado nao falha ao parsear. Excecao propagada mataria o tick
        # da lista inteira.
        _log.warning("plan_progress falhou pra cwd=%r", cwd, exc_info=True)
        return None
```

- [x] **Step 4: Rodar e ver passar**

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest tests/test_planprog.py -q`
Expected: PASS — 17 testes (`test_formato_real_dos_planos_do_repo` pode dar skip fora deste worktree).

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest -q 2>&1 | tail -2`
Expected: o número do Step 0 **+ 17**, zero falha nova.

- [x] **Step 5: Sanity no plano real (dogfooding do parser)**

```bash
cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run python -c "
from app.planprog import parse_plan
p = parse_plan('../docs/superpowers/plans/2026-07-30-progresso-do-plano.md')
print(p.name, p.done, '/', p.total, 'tasks:', p.task_total, 'atual:', p.task_idx)
"
```
Expected: `progresso-do-plano <n> / 48 tasks: 7 atual: <a task em curso>` — o **48** é o que prova o
strip de fence (sem ele daria 53, e `done` começaria em 3 sem nada executado). Medido no arquivo
real antes de começar.

- [x] **Step 6: Commit**

```bash
git add backend/app/planprog.py backend/tests/test_planprog.py
git commit -m "feat(plan): parse plan progress from superpowers plan files"
```

---

### Task 2: publicar o progresso na lista de sessões

**Files:**
- Modify: `backend/app/models.py` (`SessionInfo`, após a linha 97)
- Modify: `backend/app/registry.py` (novo `_decorate_plan` após `:57`; chamada dentro de `_decorate_git`, `:821-828`)
- Modify: `backend/app/sse.py:162-176` (`_list_sig`)
- Test: `backend/tests/test_planprog_wire.py` (novo)

**Interfaces:**
- Consumes: `plan_progress`, `PlanProgress`, `TaskProgress` da Task 1.
- Produces: em `SessionInfo` — `plan_name: str|None`, `plan_task: int|None`,
  `plan_task_total: int|None`, `plan_done: int|None`, `plan_total: int|None`,
  `plan_complete: bool|None`, `plan_tasks: list[tuple[int,int]]|None`;
  `registry._decorate_plan(info) -> None`.

**[adv] Fora de escopo:** `StateEvent` **não** recebe `plan_*` (Decisão 3).

- [ ] **Step 1: Escrever os testes que falham**

Criar `backend/tests/test_planprog_wire.py`:

```python
from app import planprog
from app.models import SessionInfo
from app.registry import _decorate_plan
from app.sse import _list_sig


def _plan(**kw):
    base = dict(name="p", path="/tmp/p.md", task_idx=2, task_total=3,
                done=9, total=17, complete=False,
                tasks=(planprog.TaskProgress("Task 1", 5, 5, ()),
                       planprog.TaskProgress("Task 2", 4, 8, ()),
                       planprog.TaskProgress("Task 3", 0, 4, ())))
    base.update(kw)
    return planprog.PlanProgress(**base)


def test_campos_no_modelo():
    i = SessionInfo(name="s")
    assert i.plan_name is None and i.plan_done is None and i.plan_tasks is None


def test_decorate_plan_preenche(monkeypatch):
    monkeypatch.setattr("app.registry.plan_progress", lambda cwd: _plan())
    i = SessionInfo(name="s", cwd="/repo")
    _decorate_plan(i)
    assert i.plan_name == "p"
    assert (i.plan_task, i.plan_task_total) == (2, 3)
    assert (i.plan_done, i.plan_total) == (9, 17)
    assert i.plan_complete is False
    assert i.plan_tasks == [(5, 5), (4, 8), (0, 4)]


def test_decorate_plan_sem_plano_deixa_none(monkeypatch):
    monkeypatch.setattr("app.registry.plan_progress", lambda cwd: None)
    i = SessionInfo(name="s", cwd="/repo")
    _decorate_plan(i)
    assert i.plan_name is None and i.plan_done is None


def test_decorate_plan_nao_levanta(monkeypatch):
    def _boom(cwd):
        raise RuntimeError("nao pode vazar pro tick da lista")
    monkeypatch.setattr("app.registry.plan_progress", _boom)
    i = SessionInfo(name="s", cwd="/repo")
    _decorate_plan(i)
    assert i.plan_name is None


def test_sig_muda_quando_o_progresso_anda():
    a = SessionInfo(name="s", plan_name="p", plan_done=9, plan_total=17)
    b = SessionInfo(name="s", plan_name="p", plan_done=10, plan_total=17)
    assert _list_sig([a]) != _list_sig([b])


def test_sig_muda_quando_o_plano_troca_com_o_mesmo_numero():
    """Bug ja documentado no comentario do `engine` (sse.py:166): sem o NOME na sig, trocar de plano
    com o mesmo done/total nao re-emite e o chip fica preso no plano errado."""
    a = SessionInfo(name="s", plan_name="alpha", plan_done=9, plan_total=17)
    b = SessionInfo(name="s", plan_name="beta", plan_done=9, plan_total=17)
    assert _list_sig([a]) != _list_sig([b])


def test_sig_estavel_sem_plano():
    assert _list_sig([SessionInfo(name="s")]) == _list_sig([SessionInfo(name="s")])
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest tests/test_planprog_wire.py -q`
Expected: FAIL — `ImportError: cannot import name '_decorate_plan' from 'app.registry'`

- [ ] **Step 3: Campos em `models.py`**

Em `SessionInfo`, logo após `loop_max` (linha 97):

```python
    # Progresso do plano do superpowers que esta sessao esta executando (app.planprog), decorado em
    # list_with_state DENTRO do to_thread do git. Sem plano -> tudo None (sem barra nem chip).
    # plan_task e ORDINAL (existe "### Task 0" nos planos), nao o N do heading.
    plan_name: Optional[str] = None
    plan_task: Optional[int] = None
    plan_task_total: Optional[int] = None
    plan_done: Optional[int] = None
    plan_total: Optional[int] = None
    plan_complete: Optional[bool] = None
    # (done, total) por Task — a barra segmentada precisa disto. Derivar segmento de
    # plan_task/plan_task_total mentiria toda vez que uma Task anterior ficasse com step pendente
    # (acontece sempre que se pula um step de verificacao manual). Sao 3-8 pares.
    plan_tasks: Optional[list[tuple[int, int]]] = None
```

- [ ] **Step 4: `_decorate_plan` em `registry.py`**

Import no topo: `from app.planprog import plan_progress`.

Logo após `_decorate_loop` (linha 57):

```python
def _decorate_plan(info) -> None:
    """Decora plan_* de UMA sessao a partir do .md do plano (app.planprog). Sem plano -> tudo None.
    Engole a excecao de proposito: roda no tick da lista, e uma falha aqui nao pode derrubar o SSE
    (incidente 2026-07-23). Module-level (nao closure) pra ser testavel isolado, igual _decorate_loop."""
    try:
        p = plan_progress(info.cwd)
    except Exception:
        _log.warning("decorate_plan falhou pra %r", getattr(info, "name", "?"), exc_info=True)
        return
    if p is None:
        return
    info.plan_name = p.name
    info.plan_task = p.task_idx
    info.plan_task_total = p.task_total
    info.plan_done = p.done
    info.plan_total = p.total
    info.plan_complete = p.complete
    info.plan_tasks = [(t.done, t.total) for t in p.tasks]
```

Dentro de `_decorate_git` (a função definida em `:821`), no mesmo laço, depois do bloco do
`summary`:

```python
                # Plano vive AQUI dentro, no mesmo to_thread: le markdown do disco, e ler arquivo na
                # corrotina e a mesma classe de erro que motivou o to_thread do git.
                _decorate_plan(info)
```

**Não** adicionar no laço de `_decorate_loop` (`:830`) — esse roda na corrotina.

- [ ] **Step 5: `_list_sig` (sse.py)**

Na tupla, após `getattr(i, "engine", None)`:

```python
          getattr(i, "plan_name", None), getattr(i, "plan_done", None),
          getattr(i, "plan_total", None),
```

E somar ao comentário acima da função: `Sem o plan_name aqui, trocar do plano A pro B com o mesmo
9/17 nao re-emite e o chip fica preso no plano errado — mesmo bug do engine.`

- [ ] **Step 6: Rodar e ver passar**

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest tests/test_planprog_wire.py -q`
Expected: PASS (7 testes)

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest -q 2>&1 | tail -2`
Expected: número da Task 1 **+ 7**, zero falha nova.

- [ ] **Step 7: Verificação de ponta (o dado sai mesmo pela API)**

```bash
cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && \
  CP_AUTH_TOKEN=teste CP_LAN_BIND_IP=127.0.0.1 CP_PORT=8799 \
  setsid uv run python -m app.main > /tmp/cp-plan.log 2>&1 &
sleep 4
curl -s -H "Authorization: Bearer teste" http://127.0.0.1:8799/api/sessions | \
  python3 -c "import sys,json; d=json.load(sys.stdin); [print(s['name'], s.get('plan_name'), s.get('plan_done'), s.get('plan_total')) for s in d['sessions']]"
```

Expected: a sessão que roda neste worktree aparece com `progresso-do-plano <n> 48`. Sessões em repo
sem plano mostram `None None None`.
Matar o backend pelo pid (`kill <pid>`), **nunca** `pkill -f app.main` — casaria a própria shell.

- [ ] **Step 8: Commit**

```bash
git add backend/app/models.py backend/app/registry.py backend/app/sse.py backend/tests/test_planprog_wire.py
git commit -m "feat(plan): expose plan progress on sessions API and list signature"
```

---

### Task 3: `lib/plan.ts` + chip nos três cards

**Files:**
- Create: `frontend/src/lib/plan.ts`, `frontend/src/lib/plan.test.ts`
- Modify: `frontend/src/lib/types.ts` (`SessionInfo`, após `:66`)
- Modify: `Sidebar.svelte` (`:982` condição, `:1006` bloco, CSS junto de `.chain-chip` em `:1749`)
- Modify: `SessionCard.svelte` (`:294` condição, `:313` bloco, CSS junto de `.paired-chip` em `:689`)
- Modify: `BoardCard.svelte` (`:352` derived, **`:378` condição**, bloco no `.bc-sub`, CSS junto de `.bc-chip` em `:609`)

**Interfaces:**
- Consumes: campos `plan_*` do `SessionInfo` (Task 2).
- Produces: `planBadge(s) -> PlanBadge | null`, `PlanBadge = {label, pct, title, complete}`.

- [ ] **Step 1: Teste que falha**

Criar `frontend/src/lib/plan.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { planBadge } from './plan';
import type { SessionInfo } from './types';

const base = { name: 's', state: 'idle' } as unknown as SessionInfo;

describe('planBadge', () => {
  it('devolve null sem plano', () => {
    expect(planBadge(base)).toBeNull();
    expect(planBadge(null)).toBeNull();
  });

  it('monta rótulo, pct e title', () => {
    const b = planBadge({ ...base, plan_name: 'git-stash-manager', plan_task: 2,
      plan_task_total: 3, plan_done: 9, plan_total: 17, plan_complete: false })!;
    expect(b.label).toBe('📋 Task 2/3');
    expect(Math.round(b.pct)).toBe(53);
    expect(b.title).toBe('git-stash-manager · Task 2/3 · 9/17 steps');
    expect(b.complete).toBe(false);
  });

  it('total 0 não divide por zero', () => {
    expect(planBadge({ ...base, plan_name: 'x', plan_done: 0, plan_total: 0 })).toBeNull();
  });

  it('plano concluído marca complete e 100%', () => {
    const b = planBadge({ ...base, plan_name: 'x', plan_task: 3, plan_task_total: 3,
      plan_done: 17, plan_total: 17, plan_complete: true })!;
    expect(b.pct).toBe(100);
    expect(b.complete).toBe(true);
    expect(b.label).toBe('📋 concluído');
  });

  it('sem task_total cai no rótulo de steps e não duplica no title', () => {
    const b = planBadge({ ...base, plan_name: 'x', plan_done: 3, plan_total: 10 })!;
    expect(b.label).toBe('📋 3/10');
    expect(b.title).toBe('x · 3/10 steps');
  });
});
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `npm --prefix /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/frontend run test -- plan.test.ts`
Expected: FAIL — não resolve `./plan`

- [ ] **Step 3: Tipos em `types.ts`**

Em `SessionInfo`, após `loop_max` (`:66`):

```ts
  plan_name?: string | null;        // nome do plano do superpowers em execução
  plan_task?: number | null;        // Task atual (ordinal, 1-based)
  plan_task_total?: number | null;
  plan_done?: number | null;        // steps marcados
  plan_total?: number | null;
  plan_complete?: boolean | null;
  plan_tasks?: [number, number][] | null;   // (done,total) por Task — alimenta a barra segmentada
```

**[adv] Não** adicionar em `StateEvent` — o backend não publica lá (Decisão 3).

- [ ] **Step 4: Implementar `frontend/src/lib/plan.ts`**

```ts
// Helpers puros do progresso do plano (app/planprog.py no backend). Espelha lib/loop.ts: o rótulo e
// a porcentagem são montados aqui, os componentes só renderizam.

import type { SessionInfo } from './types';

export interface PlanBadge {
  label: string;
  pct: number;        // 0..100
  title: string;      // tooltip: plano · Task N/M · done/total steps
  complete: boolean;
}

export type PlanCarrier = Pick<SessionInfo,
  'plan_name' | 'plan_task' | 'plan_task_total' | 'plan_done' | 'plan_total' | 'plan_complete'>;

// null = sem plano (ou payload incoerente) — o chamador esconde chip e barra. O guard de total <= 0
// existe porque done/total vira NaN no width do CSS, e um NaN% não erra visivelmente: some.
export function planBadge(s: PlanCarrier | null | undefined): PlanBadge | null {
  if (!s || !s.plan_name) return null;
  const total = s.plan_total ?? 0;
  const done = s.plan_done ?? 0;
  if (total <= 0) return null;
  const pct = Math.max(0, Math.min(100, (done / total) * 100));
  const complete = s.plan_complete === true || done >= total;
  const hasTask = s.plan_task != null && s.plan_task_total != null;
  const task = hasTask ? `Task ${s.plan_task}/${s.plan_task_total}` : `${done}/${total}`;
  return {
    label: complete ? '📋 concluído' : `📋 ${task}`,
    pct,
    // sem task, o rótulo já É done/total — repetir daria "x · 3/10 · 3/10 steps"
    title: hasTask ? `${s.plan_name} · ${task} · ${done}/${total} steps`
                   : `${s.plan_name} · ${done}/${total} steps`,
    complete,
  };
}
```

- [ ] **Step 5: Rodar e ver passar**

Run: `npm --prefix /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/frontend run test -- plan.test.ts`
Expected: PASS (5 testes)

- [ ] **Step 6: Chip no `Sidebar.svelte` (desktop)**

`<script>`: `import { planBadge } from '../lib/plan';`

Condição do `badges-line` (`:982`) — somar `|| s.plan_name`:

```svelte
{#if provTag || s.limited || s.then_target || s.pair_peers?.length || s.loop_status || s.engine || s.plan_name}
```

Dentro, após o chip do loop (`:1006`):

```svelte
                      {@const pb = planBadge(s)}
                      {#if pb}
                        <span class="plan-chip" class:plan-chip--done={pb.complete} title={pb.title}>{pb.label}</span>
                      {/if}
```

CSS junto de `.chain-chip` (`:1749`):

```css
  .plan-chip {
    padding: 1px 6px;
    border-radius: var(--radius-full);
    background: var(--accent-dim);
    color: var(--accent);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .plan-chip--done {
    background: color-mix(in srgb, var(--success) 14%, transparent);
    color: var(--success);
  }
```

- [ ] **Step 7: Chip no `SessionCard.svelte` (mobile)**

`<script>`, junto do `loopChip` (`:71`): `const planChip = $derived(planBadge(session));`

Condição do `badges-line` (`:294`) — somar `|| planChip`. Dentro, após o chip do loop:

```svelte
          {#if planChip}
            <span class="plan-chip" class:plan-chip--done={planChip.complete} title={planChip.title}>{planChip.label}</span>
          {/if}
```

Mesmo CSS do Step 6, junto de `.paired-chip` (`:689`).

- [ ] **Step 8: Chip no `BoardCard.svelte`**

Este arquivo **não tem** `badges-line`: a linha é `.bc-sub` (`:379`) e a classe de chip é `bc-chip`
(`:609`).

`<script>`, junto do `loopChip` (`:352`): `const planChip = $derived(planBadge(session));`

**[adv] A condição da linha inteira (`:378`) TEM que incluir o chip** — sem isso, sessão sem
branch/custo/motor não renderiza o `.bc-sub` e o chip fica invisível (bug intermitente, o pior tipo):

```svelte
{#if provTag || session.branch || session.pair_peers?.length || meta?.costUsd != null || meta?.sessionTime || loopChip || session.engine || planChip}
```

Dentro do `.bc-sub`:

```svelte
      {#if planChip}
        <span class="bc-chip plan-chip" class:plan-chip--done={planChip.complete} title={planChip.title}>{planChip.label}</span>
      {/if}
```

CSS: só as duas cores (o resto vem do `bc-chip`):

```css
  .plan-chip { background: var(--accent-dim); color: var(--accent); }
  .plan-chip--done { background: color-mix(in srgb, var(--success) 14%, transparent); color: var(--success); }
```

- [ ] **Step 9: Gate de tipos + verificação manual (mobile E desktop)**

Run: `npm --prefix /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/frontend run check`
Expected: `0 ERRORS 0 WARNINGS`

Manual, com o backend do worktree rodando (Task 2 Step 7) e `npm --prefix …/frontend run dev`:
1. Desktop ≥820px: a sessão do worktree mostra `📋 Task N/M` na linha de chips da sidebar.
2. Mobile (DevTools 390×844): mesmo chip no `SessionCard`.
3. `#/board`: mesmo chip — **testar numa sessão sem branch e sem motor**, que é onde o bug da
   condição apareceria.
4. Marcar um step à mão no plano → em ≤5 s o chip anda **nas três** sem recarregar.
5. Sessão em repo sem plano: nenhum chip novo.

- [ ] **Step 10: Commit**

```bash
git add frontend/src/lib/plan.ts frontend/src/lib/plan.test.ts frontend/src/lib/types.ts frontend/src/components/Sidebar.svelte frontend/src/components/SessionCard.svelte frontend/src/components/BoardCard.svelte
git commit -m "feat(plan): plan chip on sidebar, session card and board card"
```

---

### Task 4: `PlanBar.svelte` — a barra

**Files:**
- Create: `frontend/src/components/PlanBar.svelte`
- Modify: `Sidebar.svelte` (dentro de `.row-info`, e o ramo do rail recolhido em `:951-958`), `SessionCard.svelte`, `BoardCard.svelte`

- [ ] **Step 1: Criar `frontend/src/components/PlanBar.svelte`**

```svelte
<script lang="ts">
  // Barra de progresso do plano. Segmentada por Task quando cabe; única quando não cabe. A escolha é
  // do componente, não do chamador — três caminhos de render, zero configuração.
  import { planBadge } from '../lib/plan';
  import type { SessionInfo } from '../lib/types';

  interface Props {
    session: Pick<SessionInfo, 'plan_name' | 'plan_task' | 'plan_task_total' | 'plan_done' | 'plan_total' | 'plan_complete' | 'plan_tasks'>;
    // rail recolhido da sidebar: 34px de trilho não segmentam. Prop, não medição em runtime.
    compact?: boolean;
  }

  let { session, compact = false }: Props = $props();

  const badge = $derived(planBadge(session));
  // > 8 Tasks: segmento de ~20px vira listra ilegível — melhor uma barra honesta.
  const segments = $derived(
    !compact && session.plan_tasks && session.plan_tasks.length > 1 && session.plan_tasks.length <= 8
      ? session.plan_tasks
      : null,
  );
</script>

{#if badge}
  <span class="planrow" class:compact title={badge.title}>
    <span
      class="bar"
      class:solid={!segments}
      role="progressbar"
      aria-valuenow={Math.round(badge.pct)}
      aria-valuemin="0"
      aria-valuemax="100"
      aria-label={badge.title}
    >
      {#if segments}
        {#each segments as [d, t]}
          <span class="s" class:done={t > 0 && d >= t}><i style:width={`${t > 0 ? (d / t) * 100 : 0}%`}></i></span>
        {/each}
      {:else}
        <span class="s" class:done={badge.complete}><i style:width={`${badge.pct}%`}></i></span>
      {/if}
    </span>
    {#if !compact}
      <span class="lbl">{session.plan_done ?? 0}/{session.plan_total ?? 0}</span>
    {/if}
  </span>
{/if}

<style>
  .planrow { display: flex; align-items: center; gap: 8px; margin-top: 6px; }
  /* Rail recolhido: absoluto na base da row, pra não empurrar as iniciais (mesmo motivo do
     .prov-rail). A row precisa de position:relative — conferir antes de plugar. */
  .planrow.compact {
    position: absolute;
    right: 6px;
    bottom: 2px;
    left: 6px;
    margin-top: 0;
  }
  .bar { display: flex; flex: 1; gap: 3px; min-width: 0; }
  .bar.solid { gap: 0; }
  .s {
    flex: 1;
    height: 5px;
    overflow: hidden;
    border-radius: var(--radius-full);
    background: var(--bg-elevated);
  }
  .s i {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
    transition: width 400ms var(--ease-out);
  }
  .s.done i { background: var(--success); }
  .lbl {
    flex: 0 0 auto;
    color: var(--text-muted);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
  }
</style>
```

- [ ] **Step 2: Plugar nos três cards**

Import em cada um: `import PlanBar from './PlanBar.svelte';`

- `SessionCard.svelte`: `<PlanBar {session} />` logo depois do bloco `badges-line`.
- `BoardCard.svelte`: idem, depois do `.bc-sub`.
- `Sidebar.svelte` **expandido**: dentro de `.row-info`, depois da `badges-line`, `<PlanBar session={s} />`.
- `Sidebar.svelte` **rail recolhido**: o ramo `{#if !expanded && !selectMode && provTag}` (`:951`) é
  gated em `provTag` e vive **dentro** do `<span class="lead">`. Não colocar a barra ali. Adicionar um
  `<span>` novo **irmão do `.lead`**, dentro do elemento da row:

  ```svelte
  {#if !expanded && !selectMode}
    <PlanBar session={s} compact />
  {/if}
  ```

  **Antes:** confirmar que o elemento da row tem `position: relative` no CSS; se não tiver,
  adicionar (a barra é absoluta).

- [ ] **Step 3: Gate de tipos**

Run: `npm --prefix /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/frontend run check`
Expected: `0 ERRORS 0 WARNINGS`

- [ ] **Step 4: Verificação manual (mobile E desktop)**

1. Desktop: barra segmentada na sidebar; contar os segmentos = número de Tasks do plano (6, neste).
2. Recolher a sidebar: barra **única** na base da row, sem rótulo, sem empurrar as iniciais nem
   colidir com o `.prov-rail`.
3. Mobile 390px: barra no `SessionCard`, `9/42` legível, sem quebrar a linha do cwd.
4. `#/board` e `#/canvas`: barra no card, sem estourar a largura.
5. Marcar um step à mão: a barra anda com transição, não pula.
6. Plano de teste com todos os steps marcados: barra inteira verde, chip `📋 concluído`.

Salvar print de cada view em `.claude/plan-bar-<view>.png` e citar os caminhos no relatório (o
usuário lê pelo celular).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/PlanBar.svelte frontend/src/components/Sidebar.svelte frontend/src/components/SessionCard.svelte frontend/src/components/BoardCard.svelte
git commit -m "feat(plan): segmented plan progress bar on session cards"
```

---

### Task 5a: endpoint `GET /api/sessions/{name}/plan`

**[adv]** A Task 5 original (endpoint + client + tipos + 4 componentes num commit) era grande demais.
Partida em 5a (backend) e 5b (front).

**Files:**
- Modify: `backend/app/api.py`
- Test: `backend/tests/test_planprog_api.py` (novo)

**Interfaces:**
- Produces: `GET /api/sessions/{name}/plan` →
  `{name, path, task, task_total, done, total, complete, tasks: [{title, done, total, steps: [{title, done, manual}]}], markdown}`.
  404 quando não há sessão/cwd (via `_session_cwd`) ou não há plano ativo.

- [ ] **Step 1: Teste que falha**

Criar `backend/tests/test_planprog_api.py`. **Usar `api_client` e headers explícitos** — é o padrão
real de `tests/test_api.py:57`; a fixture `client` é outra coisa (app descartável só com `/ping`).

```python
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app import api as api_mod
from app.main import app
from app.models import SessionInfo

_H = {"Authorization": "Bearer secret"}


@pytest.fixture
def api_client(monkeypatch):
    """Espelha tests/test_api.py:57 — conferir aquele arquivo e copiar o setup EXATO de token."""
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    return TestClient(app)


PLAN = "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: verificação manual**\n"


def test_plan_404_sem_plano(api_client):
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd="/tmp")]), \
         patch("app.api.plan_progress", return_value=None):
        r = api_client.get("/api/sessions/s/plan", headers=_H)
    assert r.status_code == 404


def test_plan_404_sem_sessao(api_client):
    with patch("app.api.registry.list", return_value=[]):
        r = api_client.get("/api/sessions/s/plan", headers=_H)
    assert r.status_code == 404


def test_plan_devolve_detalhe_e_markdown(api_client, tmp_path):
    d = tmp_path / "docs" / "superpowers" / "plans"
    d.mkdir(parents=True)
    (tmp_path / ".git").mkdir()
    (d / "2026-07-29-plano.md").write_text(PLAN, encoding="utf-8")
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd=str(tmp_path))]):
        r = api_client.get("/api/sessions/s/plan", headers=_H)
    assert r.status_code == 200
    j = r.json()
    assert j["name"] == "plano"
    assert (j["done"], j["total"]) == (1, 2)
    assert j["complete"] is False
    assert j["tasks"][0]["steps"][0]["done"] is True
    assert j["tasks"][0]["steps"][1]["manual"] is True
    # markdown cru viaja na resposta: o GET /file so serve path citado no transcript (api.py:2196),
    # e um plano descoberto por glob nunca aparece la.
    assert "### Task 1" in j["markdown"]
```

- [ ] **Step 2: Rodar e ver falhar**

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest tests/test_planprog_api.py -q`
Expected: FAIL — 404 de rota inexistente em todos, inclusive no de sucesso.

Se a fixture não autenticar, **abrir `tests/test_api.py:57-72` e copiar o setup real** em vez de
adivinhar.

- [ ] **Step 3: Rota em `api.py`**

Import no topo: `from app.planprog import plan_progress`.

Junto das outras rotas de sessão (ex. `branches` em `:1853`):

```python
@app.get("/api/sessions/{name}/plan", dependencies=[Depends(require_auth)])
async def session_plan(name: str):
    """Detalhe do plano ativo da sessao + o markdown cru. O markdown vem JUNTO de proposito: o
    GET /sessions/{name}/file so serve path que aparece no transcript, e um plano descoberto por
    varredura (sessao nova, pos-/clear) nunca aparece la. O arquivo ja foi lido e parseado aqui."""
    cwd = _session_cwd(name)                      # ja levanta 404 sem sessao/cwd
    p = await asyncio.to_thread(plan_progress, cwd)
    if p is None:
        raise HTTPException(404, "sem plano ativo")
    try:
        markdown = await asyncio.to_thread(
            lambda: Path(p.path).read_text(encoding="utf-8", errors="replace"))
    except OSError:
        markdown = ""
    return {
        "name": p.name, "path": p.path,
        "task": p.task_idx, "task_total": p.task_total,
        "done": p.done, "total": p.total, "complete": p.complete,
        "tasks": [{"title": t.title, "done": t.done, "total": t.total,
                   "steps": [{"title": s.title, "done": s.done, "manual": s.manual}
                             for s in t.steps]}
                  for t in p.tasks],
        "markdown": markdown,
    }
```

Conferir que `asyncio` e `Path` já estão importados no `api.py`; se não, importar.

- [ ] **Step 4: Rodar e ver passar**

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest tests/test_planprog_api.py -q`
Expected: PASS (3 testes)

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest -q 2>&1 | tail -2`
Expected: número da Task 2 **+ 3**, zero falha nova.

- [ ] **Step 5: Commit**

```bash
git add backend/app/api.py backend/tests/test_planprog_api.py
git commit -m "feat(plan): plan detail endpoint with raw markdown"
```

---

### Task 5b: `PlanPanel` no painel desktop e na sheet mobile

**Files:**
- Create: `frontend/src/components/PlanPanel.svelte`
- Modify: `frontend/src/lib/types.ts` (`PlanDetail`), `frontend/src/lib/api.ts` (`getPlan`/`getPlanForServer`)
- Modify: `DesktopSessionContext.svelte` (props novas + seção "Plano" antes de `:165`)
- Modify: `ActivitySheet.svelte` (prop `showPlan`), `Chat.svelte` (`:1221` e o painel)

- [ ] **Step 1: `PlanDetail` em `types.ts`**

```ts
export interface PlanStep { title: string; done: boolean; manual: boolean }
export interface PlanTask { title: string; done: number; total: number; steps: PlanStep[] }
export interface PlanDetail {
  name: string; path: string;
  task: number; task_total: number;
  done: number; total: number; complete: boolean;
  tasks: PlanTask[];
  markdown: string;
}
```

- [ ] **Step 2: Client em `lib/api.ts`**

Seguir o par que o arquivo já usa (`getX(name)` + `getXForServer(s, name)` — ver `getConfigForServer`
em `:594` e o vizinho `getX` correspondente, e copiar a forma exata de montar URL/headers):

```ts
export async function getPlanForServer(s: Server, name: string): Promise<PlanDetail | null> {
  const r = await fetch(`${s.baseUrl}/api/sessions/${encodeURIComponent(name)}/plan`,
                        { headers: { Authorization: `Bearer ${s.token}` } });
  if (r.status === 404) return null;      // sem plano ativo não é erro
  if (!r.ok) throw new Error(`plan ${r.status}`);
  return r.json();
}

export const getPlan = (name: string) => getPlanForServer(currentServer(), name);
```

`currentServer()` é o nome **presumido** — usar o helper que os outros `getX` do arquivo usam.

- [ ] **Step 3: `PlanPanel.svelte`**

```svelte
<script lang="ts">
  // Detalhe do plano: barra + Tasks. Só a Task atual abre os steps — "próximo passo" não é campo, é o
  // primeiro ○ da lista. O markdown cru vem do próprio /plan (o /file não serve este arquivo).
  import PlanBar from './PlanBar.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import type { PlanDetail, SessionInfo } from '../lib/types';

  interface Props {
    session: SessionInfo;
    detail: PlanDetail | null;
    loading?: boolean;
  }
  let { session, detail, loading = false }: Props = $props();
  let showMd = $state(false);

  const current = $derived(detail ? detail.task - 1 : -1);
</script>

<div class="plan">
  <button class="plan-name" onclick={() => (showMd = !showMd)} title="ver o plano inteiro">
    {session.plan_name}
    <span class="chev" class:open={showMd}>›</span>
  </button>

  <PlanBar {session} />

  {#if loading && !detail}
    <p class="muted">carregando o plano…</p>
  {:else if detail}
    <ul class="tasks">
      {#each detail.tasks as t, i}
        <li class="task" class:done={t.total > 0 && t.done >= t.total} class:cur={i === current}>
          <span class="mark">{t.total > 0 && t.done >= t.total ? '✓' : i === current ? '◐' : '○'}</span>
          <span class="ttl">{t.title}</span>
          <span class="cnt">{t.done}/{t.total}</span>
        </li>
        {#if i === current}
          <li class="steps">
            <ul>
              {#each t.steps as s}
                <li class:done={s.done}>
                  <span class="mark">{s.done ? '✓' : '○'}</span>
                  <span class="ttl">{s.title}</span>
                  {#if s.manual}<span class="manual" title="precisa de conferência humana">🙋</span>{/if}
                </li>
              {/each}
            </ul>
          </li>
        {/if}
      {/each}
    </ul>

    {#if showMd}
      <!-- Markdown NUNCA aparece cru (regra do CLAUDE.md): um <pre> com ** e ## à mostra é bug. -->
      <div class="md">{@html renderMarkdown(detail.markdown)}</div>
    {/if}
  {/if}
</div>
```

CSS: seguir a tipografia do `DesktopSessionContext` (`.section-label`, `--text-muted`, 11-12px);
`.md` com `max-height: 50vh; overflow: auto`. `✓` em `--success`, `◐` em `--accent`, `○` em
`--text-muted`.

- [ ] **Step 4: Desktop — props novas no `DesktopSessionContext`**

O componente **não recebe `session` hoje** (só escalares como `sessionName`). Adicionar:

```ts
    session?: SessionInfo | null;
    planDetail?: PlanDetail | null;
    planLoading?: boolean;
```

E a seção, **antes** da `<section class="sec-metric">` do Contexto (`:165`):

```svelte
  {#if session?.plan_name}
    <section class="sec-metric">
      <span class="section-label">Plano</span>
      <PlanPanel {session} detail={planDetail ?? null} loading={planLoading ?? false} />
    </section>
  {/if}
```

- [ ] **Step 5: `Chat.svelte` — de onde vem a `session` e o detalhe**

O `Chat` já mantém a lista agregada no desktop (ver o bloco em torno de `Chat.svelte:227-260`, que
tem `if (!desktop) return`). Usar **essa** lista:

```ts
  const planSession = $derived(sessions.find((s) => s.name === sessionName) ?? null);
  let planDetail = $state<PlanDetail | null>(null);
  let planLoading = $state(false);

  // busca só quando o plano MUDA de nome (o progresso já vem na lista, a cada 5s)
  $effect(() => {
    const n = planSession?.plan_name ?? null;
    if (!n) { planDetail = null; return; }
    planLoading = true;
    getPlan(sessionName)
      .then((d) => { planDetail = d; })
      .catch(() => { planDetail = null; })
      .finally(() => { planLoading = false; });
  });
```

**Conferir o nome real da variável da lista** antes de escrever (`sessions`, `openSessions`, …) —
ler o bloco citado. Se o `Chat` puder estar num peek de outro servidor, usar `getPlanForServer` com o
servidor daquele card, não `getPlan`.

Passar ao painel: `session={planSession} {planDetail} {planLoading}`.

- [ ] **Step 6: Mobile — `PlanPanel` no topo da `ActivitySheet`**

Prop nova `showPlan = false` na `ActivitySheet` (+ `session`, `planDetail`, `planLoading`), e o
`PlanPanel` no topo só quando `showPlan && session?.plan_name`.

No `Chat.svelte:1221`: `showPlan={!desktop}` — a prop de viewport do `Chat` chama-se **`desktop`**
(`:53`), não `isDesktop`. **Não** criar um `matchMedia` novo.

**Por quê a prop:** a `ActivitySheet` é montada pelo `Chat`, que roda nas **duas** views — sem o
gate, o painel apareceria duplicado no desktop (uma vez no `DesktopSessionContext`, outra na sheet).

- [ ] **Step 7: Gate de tipos + verificação manual (mobile E desktop)**

Run: `npm --prefix /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/frontend run check`
Expected: `0 ERRORS 0 WARNINGS`

Manual:
1. Desktop: seção "Plano" no painel da direita, acima de "Contexto"; só a Task atual aberta.
2. Desktop: abrir a `ActivitySheet` pelo `⋯` → **não** mostra o painel do plano (sem duplicata).
3. Mobile: `⋯` → `ActivitySheet` com o `PlanPanel` no topo.
4. Clicar no nome do plano: markdown **renderizado**, sem `**` nem `##` à mostra.
5. Step com "verificação manual" no título mostra `🙋`.
6. Sessão sem plano: nenhuma seção nova, painel idêntico ao de hoje.

Prints em `.claude/plan-panel-desktop.png` e `.claude/plan-panel-mobile.png`.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/components/PlanPanel.svelte frontend/src/components/DesktopSessionContext.svelte frontend/src/components/ActivitySheet.svelte frontend/src/screens/Chat.svelte frontend/src/lib/api.ts frontend/src/lib/types.ts
git commit -m "feat(plan): plan panel on desktop context and mobile activity sheet"
```

---

### Task 6: docs + gate final

**Files:**
- Modify: `CLAUDE.md` (raiz do repo, seção "Conventions & gotchas")
- Modify: `README.md` (tabela de API)
- Create: `docs/superpowers/specs/2026-07-30-regra-para-o-claude-md-global.md`

- [ ] **Step 1: Bloco no `CLAUDE.md` do repo**

Bullet em "Conventions & gotchas", no estilo dos vizinhos, cobrindo: a fonte de verdade é o `.md` do
plano (`app/planprog.py`); o strip de fences existe porque planos mostram steps de exemplo dentro de
bloco de código e sem isso o plano nasce "3/47 feito"; a decoração roda dentro do `to_thread` do git;
`plan_name` está no `_list_sig` por causa do bug do `engine`; `plan_tasks` existe porque a barra
segmentada não pode ser derivada de `task_idx/task_total`; `_plans_dir` para no primeiro `.git` pra
um worktree não mostrar o plano do checkout principal; e a regra de marcar `- [x]` ao fim de cada
step.

- [ ] **Step 2: Rota nova na tabela de API do `README.md`**

- [ ] **Step 3: [adv] Texto pronto pro `~/.claude/CLAUDE.md`, sem editá-lo**

O global é config do usuário, fora do repo — não é para o executor alterar sozinho. Salvar em
`docs/superpowers/specs/2026-07-30-regra-para-o-claude-md-global.md`:

```markdown
- **Executando plano do superpowers:** ao terminar cada Step, marcar `- [ ]` → `- [x]` no arquivo do plano. Step que precisa de conferência humana leva "verificação manual" no título. O progresso que aparece no celular (claude-cockpit) lê daí.
```

Sem "no mesmo commit": `docs/superpowers/` é gitignored e metade dos planos é untracked — a regra
seria impossível de cumprir e falharia em silêncio.

- [ ] **Step 4: Gate final completo**

Run: `cd /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/backend && uv run pytest -q 2>&1 | tail -2`
Expected: baseline do Step 0 da Task 1 **+ 27** testes novos (17 + 7 + 3), zero falha.

Run: `npm --prefix /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/frontend run test`
Expected: tudo passa (inclui os 5 de `plan.test.ts`).

Run: `npm --prefix /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/frontend run check`
Expected: `466+ FILES 0 ERRORS 0 WARNINGS`

Run: `npm --prefix /home/jefferson/Projetos/claude-cockpit/.claude/worktrees/plan-progress/frontend run build`
Expected: build ok

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md docs/superpowers/specs/2026-07-30-regra-para-o-claude-md-global.md
git commit -m "docs(plan): document plan progress source of truth and the /plan route"
```

---

## Notas de verificação (self-review do plano)

- **Cobertura do spec:** descoberta com as correções (raiz do repo com parada no `.git`, regex único,
  strip de fence, sticky, concluído visível, expiração) → Task 1; cache `st_mtime_ns` + TTL por cwd →
  Task 1; `to_thread` + `_list_sig` com `plan_name` → Task 2; chip nos três cards com as classes
  locais de cada um e a condição de cada linha → Task 3; `PlanBar` segmentada/única/compact → Task 4;
  `/plan` com markdown junto → Task 5a; `PlanPanel` sem duplicar no desktop → Task 5b; regra de
  processo (sem tocar no global do usuário) → Task 6.
- **Correções do pass adversarial aplicadas:** 6 bloqueantes (fence, `StateEvent` cortado, fiação da
  Task 5, rota com API real, contagens de teste, teste do sort), 4 sérios (I/O por arquivo,
  `_plans_dir` vazando pro repo de fora, condição do `BoardCard`, barra no rail), 7 menores.
- **Onde o plano manda ler antes de escrever** (deliberado — inventar nome ali gera código que não
  compila): fixture de auth em `test_api.py:57`, helper de servidor em `lib/api.ts`, nome da lista de
  sessões no `Chat.svelte`, `position: relative` na row da `Sidebar`.
- **Consistência de tipos:** `PlanProgress.task_idx` (backend) → `plan_task` (payload) →
  `session.plan_task` (front) → `planBadge().label`. `plan_tasks` é `list[tuple[int,int]]` no Python
  e `[number, number][]` no TS, consumido só pelo `PlanBar`. `PlanDetail.task` (não `task_idx`) é o
  nome no JSON da rota — o `PlanPanel` usa `detail.task - 1` como índice.
- **Não coberto de propósito:** `plan_*` no `StateEvent`; worktree com plano untracked (agora
  devolve "sem barra", não barra errada).

## Loop-readiness

Cada Task fecha com suíte verde + commit próprio — dá pra parar entre Tasks sem árvore quebrada. A
Task 3 é onde o valor já está entregue (chip nas três views); 4 a 6 são incremento.
