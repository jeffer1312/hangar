# Progresso do plano em execução — Design

**Data:** 2026-07-30
**Revisado:** 2026-07-30, após pass adversarial (`ecc:architect` + `Explore`). Achados incorporados
abaixo; os que mudaram decisão estão marcados com **[rev]**.

**Problema:** quase todo trabalho no repo passa por um plano do `superpowers:writing-plans`, mas
olhando o celular não dá pra saber em que pé o plano está: qual Task, quanto falta, qual o próximo
passo, e o que é conferência humana (minha) e não trabalho do Claude.

## Estado atual (o que já existe, não recriar)

- `frontend/src/lib/activity.ts:59` — `createActivityFolder` folda `TodoWrite`/`TaskCreate`/
  `TaskUpdate` do transcript e já produz `{tasks, total, done, inProgress}`. Consumido pela
  `ActivitySheet` e pelo badge do `⋯` na `NavBar`.
- `DesktopSessionContext.svelte:637` — classe `.progress` (4px, trilho `--bg-elevated`,
  preenchimento `--accent`, variantes `.tone-warn`/`.tone-hot`), já usada por Contexto e Limites.
- **O precedente exato:** `loop_status`/`loop_iter`/`loop_max` — sidecar → `registry._decorate_loop`
  (`registry.py:49`, chamado em `:831`) → `_list_sig` (`sse.py:162`) → chip nas listas via
  `lib/loop.ts:33` (`loopBadge`, `LOOP_TONE_COLOR`).
- `SessionInfo` mora em **`backend/app/models.py:44`** (Pydantic), campos `loop_*` em `:95-97`.
  No front: `types.ts:64-66` (`SessionInfo`) e `:122-124` (`StateEvent`).
- `lib/markdown.ts` — `renderMarkdown`. Chamado por `HoverPreview`, `ActivitySheet`, `PairSheet`,
  `AssistantBubble` — **todos recebem string em memória**.

**O que falta:** nada disso sabe que existe um *plano*. Os todos da sessão somem no `/clear`, não
têm o nome do plano, nem o total de Tasks, nem o que é verificação manual.

## Achado que define a fonte de verdade

Plano executado hoje fica com **0 checkbox marcado** (contagens conferidas):

```
0/28  2026-07-22-git-status-badge-panel.md
0/56  2026-07-22-loop-runner.md
0/34  2026-07-26-mobile-ux-refinement.md
0/49  2026-07-26-motores-de-modelo.md
6/44  2026-07-27-pi-adapter.md
```

O formato já tem `- [ ] **Step N: …**` dentro de `### Task N: …`; falta o executor **marcar**.

## Decisão

| Fonte | Diz | Onde aparece | Durável |
|---|---|---|---|
| Arquivo do plano (`.md`) | **onde tu está** (Task 2/3, 9/17 steps, próximo) | card + painel | sim |
| Todos da sessão (`activity.ts`) | **o que roda agora** dentro do step | `ActivitySheet`, como já é | não |

---

## Backend — `app/planprog.py` (novo)

`plan_progress(cwd) -> PlanProgress | None`.

### Descoberta do plano ativo

**[rev]** A versão anterior era "`glob` + `mtime` mais recente que tenha um `[x]`". Quatro furos
achados no pass adversarial, todos corrigidos aqui:

1. **Raiz do repo, não o cwd da pane.** `tmux.list_panes_active()` (`tmux.py:113`) usa
   `#{pane_current_path}`, que segue o `cd`. Um `cd frontend` fazia a barra sumir. → sobe no máximo
   4 parents procurando `docs/superpowers/plans/`. São 4 `is_dir()`, mais barato que o glob.
2. **Um regex só.** A descoberta procurava `- [x]` qualquer e o parse só contava
   `- [x] **Step N:**`. Um `[x]` fora de Step (checklist de critérios, exemplo em bloco de código)
   elegia um plano que depois lia `0/N`, escondendo o real. Hoje não ocorre nos 11 planos do repo,
   mas a regra de processo vai pro CLAUDE.md **global**, em repos cujo formato eu não controlo.
   → **o mesmo regex de Step nos dois lugares.**
3. **Sticky por cwd.** Se o `writing-plans` reescreve outro plano no meio da execução, ele vira o
   `mtime` mais novo e rouba o posto: `9/17` vira `3/56` e volta. → se o plano eleito no ciclo
   anterior ainda tem step pendente, ele **continua** eleito; só troca quando fecha ou some.
4. **Plano concluído não desaparece — mostra "concluído".** A versão anterior escondia a barra em
   100%, mas o arquivo seguia sendo o `mtime` mais recente com `[x]`, e o plano seguinte (que começa
   com zero `[x]`) nunca acendia: tu fecha um plano, começa outro, e o celular fica mudo —
   indistinguível de bug. → 100% é informação, não ausência dela; e a eleição **prefere plano com
   step pendente**.

**Plano abandonado:** `mtime` além de **14 dias** → não elege. Sem isso um `3/44` de três semanas
atrás fica de "ativo" pra sempre.

### Parse

`### Task <n>: <título>` abre uma Task; `- [ ]` / `- [x]` **`**Step N: <título>**`** são os steps.

**[rev] `Task 0` existe** (`2026-07-27-pi-adapter.md:116`, com 6 steps). O rótulo usa **posição
ordinal** (1-based), não o `N` do heading; o painel mostra o **título literal** do heading. Assim
"Task 2/3" nunca vira "Task 0/6" e o texto que tu lê é o mesmo que está no arquivo.

```python
@dataclass(frozen=True)
class PlanProgress:
    name: str                    # stem sem a data: "git-stash-manager"
    path: str
    task_idx: int                # posição ordinal da 1ª Task com step pendente
    task_total: int
    done: int
    total: int
    tasks: tuple[TaskProgress, ...]   # título, done, total, steps
    complete: bool               # done == total
```

**`total <= 0` → devolve `None`.** Sem isso `done/total` levanta `ZeroDivisionError` no backend e
vira `width: NaN%` no CSS. Vale no front também.

**Verificação manual:** step com `/verifica(ção|r) manual|manual verification/i` no título ganha
`manual=True`. Ocorre em 5 planos hoje (ex. `git-log-hub.md:869`). Se o plano não escreveu, não
aparece nada.

### Custo e cache **[rev]**

A versão anterior tinha cache só do parse do vencedor — mas a *descoberta* precisa **abrir** os
candidatos pra saber se têm step marcado. Neste repo os 5 planos mais novos têm zero `[x]`: seriam
**2.572 linhas lidas por sessão, por poll de 1,5 s**, antes de chegar no pi-adapter.

- Cache de **arquivo**: `path -> (mtime_ns, size, PlanProgress|None)`. Memoriza também o "não tem
  step marcado" — é o que mata a releitura dos candidatos.
- Cache de **descoberta por cwd**, TTL 3 s — mesmo padrão do `_summary_cache` (`git_ops.py:93`).
  Com N sessões no mesmo repo o trabalho passa de N× pra 1×.
- **`st_mtime_ns`, não `st_mtime`+`size`.** `- [ ]` → `- [x]` **preserva o tamanho**; com mtime de
  granularidade grossa a chave não muda e o cache serve valor velho — a barra congela e parece que o
  executor esqueceu de marcar.
- `path.read_bytes()` de uma vez, não linha a linha: o `Edit` trunca e reescreve, e um poll no meio
  lê menos steps → sig cai e sobe = piscada em todas as views ao mesmo tempo.
- `is_dir()` antes do glob; `cwd` falsy sai antes de qualquer I/O (`registry.py:665` monta
  `SessionInfo` com `cwd=meta.get("cwd")`, que pode ser `None` em sessão Codex — sem o guard, é um
  traceback por sessão por poll, pra sempre).

### Integração

1. **[rev] `_decorate_plan(info)` roda DENTRO da função `_decorate_git`**, que já está em
   `asyncio.to_thread` (`registry.py:829`) justamente por causa do incidente de 2026-07-23.
   `_decorate_loop` está fora porque lê um json minúsculo; markdown não é isso. Zero hop novo.
2. **[rev] `_list_sig` ganha `plan_name`, `plan_done`, `plan_total`.** Sem o **nome** é o bug já
   documentado no comentário do `engine` (`sse.py:166-168`): trocar do plano A (9/17) pro B (9/17)
   não muda a sig, a lista não re-emite, e o chip mostra o plano errado até outra coisa mudar.
3. Campos no `SessionInfo` (`models.py`): `plan_name`, `plan_task`, `plan_task_total`, `plan_done`,
   `plan_total`, `plan_complete`, `plan_tasks`.
4. **[rev] `plan_tasks: list[tuple[int,int]]`** — `[(done,total)]` por Task, 3 a 8 pares. Vai no
   payload porque a barra segmentada precisa dele: derivar segmento de `task_idx/task_total` mentiria
   toda vez que uma Task anterior ficasse com step pendente (acontece sempre que se pula um step de
   verificação manual). São dezenas de bytes.
5. `GET /api/sessions/<name>/plan` — detalhe (Tasks, steps, `manual`) **+ o markdown cru**.
   **[rev]** O `GET /api/sessions/{name}/file` **não serve** pra isso: ele só entrega path que
   aparece no transcript (`api.py:2186`). Plano descoberto por glob e não citado naquela sessão
   (pós-`/clear`, sessão nova) seria bloqueado. O arquivo já foi lido e parseado aqui — devolve
   junto.

**Sessão não-Claude** (Codex, Pi): decora igual. Uma sessão Pi roda no mesmo `~/.claude` e pode
executar plano; barra parada é sinal legítimo, não erro.

**Worktrees — limitação declarada [rev].** `git worktree add` não copia arquivo untracked, e
`.gitignore:56` ignora `docs/superpowers/` (os 5 planos de git foram versionados à força em
`390e91c`; os demais são untracked). Os 4 worktrees em `.claude/worktrees/` têm **0 planos**. Uma
sessão executando Task dentro do worktree não acha o plano se ele for untracked. Não resolvo na v1:
a correção seria ler o `gitdir:` do arquivo `.git` do worktree e subir pro common dir — anotado como
melhoria, não como requisito.

---

## Frontend

### `lib/plan.ts` + `plan.test.ts`

`planBadge(session) -> {label: '📋 Task 2/3', pct, title} | null`. Espelha `lib/loop.ts`. Guard de
`total <= 0` testado.

### `components/PlanBar.svelte`

- **≤ 8 Tasks → segmentada**, uma por Task, alimentada por `plan_tasks`. Verde (`--success`) = Task
  fechada, roxo (`--accent`) = a de agora.
- **> 8 Tasks → barra única.** Segmento de ~20px vira listra ilegível.
- **prop `compact` (rail recolhido) → sempre única.** Trilho de 34px. Prop, não medição.
- **`plan_complete` → barra cheia em `--success`** e rótulo `17/17`.

5px, `border-radius: var(--radius-full)`, rótulo à direita em `--text-muted`, `tabular-nums`.
O estado da sessão segue sendo a única cor forte da row.

### Card de sessão — **[rev] três arquivos, dois vocabulários de CSS**

O design anterior dizia "a `badges-line` que os três já têm". Falso:

| Arquivo | Linha de chips | Classe a reusar |
|---|---|---|
| `Sidebar.svelte:985` | `badges-line` | `badges-line` |
| `SessionCard.svelte:297` | `badges-line` | `badges-line` |
| `BoardCard.svelte:378-398` | `.bc-sub` | `bc-chip` |

Chip `📋 Task 2/3` + `<PlanBar>` nos três, **cada um com a classe local do próprio arquivo**. É
exatamente a deriva que o `CLAUDE.md` avisa; nomear os três pontos é o que evita ela. Canvas herda do
`BoardCard`. Sem plano → nada renderiza.

### `components/PlanPanel.svelte`

Barra + Tasks; só a Task atual abre os steps. "Próximo" não é campo — é o primeiro `○` da lista.
Nome do plano abre o markdown que veio no `/plan`, via `renderMarkdown`.

**[rev] Um lugar, não dois.** A `ActivitySheet` é montada pelo `Chat.svelte:1221`, e o `Chat` roda
nas **duas** views — pôr o painel nela *e* no `DesktopSessionContext` duplicaria no desktop. Então:

- **Desktop:** seção no `DesktopSessionContext`, acima de "Contexto".
- **Mobile:** `PlanPanel` no topo da `ActivitySheet`, atrás de uma prop `showPlan` que **só o mobile
  liga**. Chip `📋` do card abre ela.

**[rev] Multi-servidor:** Board e Canvas agregam sessões de vários servidores — o `/plan` tem que ser
chamado na base URL do servidor daquele card, não na local.

---

## Regra de processo — `~/.claude/CLAUDE.md` global

**[rev]** Sem "no mesmo commit": `docs/superpowers/` é gitignored e metade dos planos é untracked —
a regra seria silenciosamente impossível de cumprir, que é o modo de falha que este design existe
pra evitar.

> **Executando plano do superpowers:** ao terminar cada Step, marcar `- [ ]` → `- [x]` no arquivo do
> plano. Step que precisa de conferência humana leva "verificação manual" no título. O progresso que
> aparece no celular lê daí.

Autocorretivo: esqueceu de marcar → a barra fica parada e isso é visível na hora.

---

## Ordem de implementação

1. `planprog.py` + `backend/tests/test_planprog.py` **isolados**. Fixtures reais: pi-adapter
   (parcial) e um `2026-07-29-git-*` (zero marcado).
2. Campos em `models.py` → `_decorate_plan` dentro do `to_thread` → `plan_name`/`plan_done`/
   `plan_total` no `_list_sig`, **no mesmo commit**. Separar a sig da decoração produz barra
   congelada e um parser depurado à toa. Verificação: `/api/sessions` mostra os campos, nenhuma UI
   mudou, o log do refresher não regrediu.
3. `lib/plan.ts` + teste → chip nos três cards. **Aqui o valor já está entregue.**
4. `PlanBar` (segmentada) nos três cards.
5. `GET /plan` + `PlanPanel` no desktop e no mobile.

Dependência dura: `models.py` antes de `registry.py`; `lib/plan.ts` + teste antes de qualquer
`.svelte`.

## Não-objetivos

- Seletor manual de plano. Sai do cwd; duas sessões no mesmo repo mostram o mesmo plano.
- Casar todo da sessão com step do plano. O Claude renomeia livremente.
- Editar o plano pelo app (marcar checkbox pelo celular).
- Progresso por commits do git. Commit fora do plano infla, step sem commit some.
- Achar o plano a partir de um worktree cujo plano é untracked (ver limitação acima).

## Verificação

- **Backend** (`tests/test_planprog.py`): plano parcial; plano sem `[x]` de Step → `None`;
  `[x]` fora de Step não elege; `total == 0` → `None`; `Task 0` numera ordinal; sticky mantém o
  plano quando outro é reescrito; plano com 15 dias não elege; `plan_complete` em 100%.
  **[rev] Os modos de falha reais são I/O, não parse** — markdown não falha ao parsear. Testar:
  arquivo removido entre o glob e o read, permissão negada, leitura durante truncate.
- **Front:** `lib/plan.test.ts`; `npm --prefix frontend run check` no gate.
- **Manual:** marcar um step à mão, ver a barra andar no celular **e** no desktop, no card **e** no
  painel — as duas views, como o `CLAUDE.md` exige.
