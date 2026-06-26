# Phase 4 — Restaurar métricas (Opção 1: espalhado) + stop no composer + spinner slim

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** Restaurar as métricas que o redesign Phase 1 (commit 0dcb0ff, que aposentou o `StatusBar.svelte` `<pre>`) deixou órfãs, com **disposição espalhada e harmônica** (Opção 1): janelas **5h/7d na NavBar** (topo, info de conta); **custo discreto no composer**; **ctx fica no ring e modelo no pill** (sem duplicar); tempo de sessão + tokens absolutos + resets no **UsageSheet** (tap). Mais: **Stop volta pro composer com confirmação**, e o **spinner** (ex-ActivityBadge) vira linha slim sem bubble.

**Architecture:** Client-side — `parseStatusLine` já extrai os 11 campos; `status` chega no `Chat.svelte:66`. `Chat` vira o dono do `UsageSheet` (estado `usageOpen`) e do `status`, passando `status` + `onExpandUsage` pra NavBar (chips 5h/7d) e pro Composer (chip de custo). Stop+confirm ficam no Composer. Sem mudança de backend.

**Tech Stack:** Svelte 5 (runes), TS, Vite. Reuso de `BottomSheet.svelte` (props `{open,onClose,ariaLabel,children}`). Verificação por `npm run check` + `npm run build` + teste manual (sem unit no front).

**Git:** commits direto na `main`, 1 por task, conventional, inglês, SEM trailer `Co-Authored-By`. Nunca trocar branch. `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend`, `git -C /home/jefferson/pessoal/claude-pocket`, nunca `cd`.

**StatusFields (statusline.ts):** `model, effort, ctxUsed, ctxTotal, ctxPct, costUsd, fiveHourPct, fiveHourReset, weeklyPct, weeklyReset, sessionTime, raw`.

**Disposição final (quem mostra o quê):**
- NavBar (topo): chips `⚡{5h}%` `📅{7d}%` (cor por limiar) → tap abre UsageSheet
- Composer control-left: ring(ctx) + pill(modelo·effort) [já existem] + **chip `$custo`** [novo] → tap abre UsageSheet
- Composer control-right: enviar [+ stop quando working → confirm]
- UsageSheet: 5h/7d com reset completo, ctx abs vs janela, custo, tempo de sessão, modelo, e a statusline crua (fallback)
- Spinner (fim da lista, working): atividade + cronômetro vivo (sem cost/stop)

---

### Task 1: RateChips.svelte (chips 5h/7d pra NavBar)

**Files:**
- Create: `frontend/src/components/RateChips.svelte`

- [ ] **Step 1: Criar o componente**

Create `frontend/src/components/RateChips.svelte`:
```svelte
<script lang="ts">
  import type { StatusFields } from '../lib/statusline';

  interface Props {
    status: StatusFields | null;
    onExpand: () => void;
  }
  let { status, onExpand }: Props = $props();

  // Saturacao da janela: verde calmo -> ambar -> vermelho.
  function pctClass(pct: number | undefined): string {
    if (typeof pct !== 'number' || !isFinite(pct)) return '';
    if (pct >= 90) return 'hot';
    if (pct >= 70) return 'warm';
    return 'cool';
  }

  const has = $derived(
    typeof status?.fiveHourPct === 'number' || typeof status?.weeklyPct === 'number'
  );
</script>

{#if status && has}
  <div class="rate-chips">
    {#if typeof status.fiveHourPct === 'number'}
      <button class="rchip {pctClass(status.fiveHourPct)}" onclick={onExpand} aria-label="Janela de 5 horas">
        <span aria-hidden="true">⚡</span>{status.fiveHourPct}%
      </button>
    {/if}
    {#if typeof status.weeklyPct === 'number'}
      <button class="rchip {pctClass(status.weeklyPct)}" onclick={onExpand} aria-label="Janela de 7 dias">
        <span aria-hidden="true">📅</span>{status.weeklyPct}%
      </button>
    {/if}
  </div>
{/if}

<style>
  .rate-chips {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    flex-shrink: 0;
  }

  /* Chip compacto pro topo: alvo de toque ok, visual de 28px. Abre o UsageSheet. */
  .rchip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 28px;
    min-height: 0;
    min-width: 0;
    padding: 0 var(--space-2);
    background: var(--bg-hover);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .rchip.cool { color: var(--success); }
  .rchip.warm { color: var(--warning); }
  .rchip.hot  { color: var(--error); }
</style>
```

- [ ] **Step 2: Verificar + Commit**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/components/RateChips.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(metrics): RateChips — compact 5h/7d window chips"
```

---

### Task 2: UsageSheet.svelte (detalhe completo + linha crua)

**Files:**
- Create: `frontend/src/components/UsageSheet.svelte`

- [ ] **Step 1: Criar o sheet** (reusa BottomSheet)

Create `frontend/src/components/UsageSheet.svelte`:
```svelte
<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import type { StatusFields } from '../lib/statusline';

  interface Props {
    open: boolean;
    status: StatusFields | null;
    onClose: () => void;
  }
  let { open, status, onClose }: Props = $props();

  const rows = $derived.by(() => {
    const s = status;
    if (!s) return [] as { label: string; value: string }[];
    const out: { label: string; value: string }[] = [];
    if (typeof s.fiveHourPct === 'number')
      out.push({ label: 'Janela 5h', value: `${s.fiveHourPct}%` + (s.fiveHourReset ? ` · reset ${s.fiveHourReset}` : '') });
    if (typeof s.weeklyPct === 'number')
      out.push({ label: 'Janela 7d', value: `${s.weeklyPct}%` + (s.weeklyReset ? ` · reset ${s.weeklyReset}` : '') });
    if (typeof s.ctxUsed === 'number')
      out.push({ label: 'Contexto', value: `${s.ctxUsed.toLocaleString('pt-BR')}${s.ctxTotal ? ' / ' + s.ctxTotal.toLocaleString('pt-BR') : ''}${typeof s.ctxPct === 'number' ? ` (${Math.round(s.ctxPct)}%)` : ''}` });
    if (typeof s.costUsd === 'number')
      out.push({ label: 'Custo', value: `$${s.costUsd.toFixed(2)}` });
    if (s.sessionTime)
      out.push({ label: 'Tempo de sessão', value: s.sessionTime });
    if (s.model)
      out.push({ label: 'Modelo', value: s.model + (s.effort ? ` · ${s.effort}` : '') });
    return out;
  });
</script>

<BottomSheet {open} {onClose} ariaLabel="Uso e limites">
  <div class="usage">
    <h2 class="usage-title">Uso & limites</h2>
    {#each rows as r}
      <div class="usage-row">
        <span class="usage-label">{r.label}</span>
        <span class="usage-value">{r.value}</span>
      </div>
    {/each}
    {#if status?.raw}
      <div class="usage-raw">
        <span class="usage-label">Statusline crua</span>
        <code class="usage-raw-line">{status.raw}</code>
      </div>
    {/if}
  </div>
</BottomSheet>

<style>
  .usage { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; }
  .usage-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-1); }
  .usage-row { display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-4); }
  .usage-label { font-size: var(--text-sm); color: var(--text-secondary); }
  .usage-value { font-family: var(--font-mono); font-size: var(--text-sm); font-variant-numeric: tabular-nums; color: var(--text-primary); text-align: right; }
  .usage-raw { display: flex; flex-direction: column; gap: var(--space-1); margin-top: var(--space-2); padding-top: var(--space-3); border-top: 1px solid var(--border-subtle); }
  .usage-raw-line { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); word-break: break-all; white-space: pre-wrap; }
</style>
```

- [ ] **Step 2: Verificar + Commit**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/components/UsageSheet.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(metrics): UsageSheet — full detail + raw statusline fallback"
```

---

### Task 3: ConfirmSheet.svelte (confirmação genérica)

**Files:**
- Create: `frontend/src/components/ConfirmSheet.svelte`

- [ ] **Step 1: Criar**

Create `frontend/src/components/ConfirmSheet.svelte`:
```svelte
<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';

  interface Props {
    open: boolean;
    title: string;
    message?: string | null;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onClose: () => void;
  }
  let { open, title, message = null, confirmLabel = 'Confirmar', cancelLabel = 'Cancelar', danger = false, onConfirm, onClose }: Props = $props();

  function confirm() {
    onConfirm();
    onClose();
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={title}>
  <div class="confirm">
    <h2 class="confirm-title">{title}</h2>
    {#if message}<p class="confirm-msg">{message}</p>{/if}
    <div class="confirm-actions">
      <button class="btn btn-cancel" onclick={onClose}>{cancelLabel}</button>
      <button class="btn btn-confirm" class:danger onclick={confirm}>{confirmLabel}</button>
    </div>
  </div>
</BottomSheet>

<style>
  .confirm { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; }
  .confirm-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .confirm-msg { font-size: var(--text-sm); color: var(--text-secondary); }
  .confirm-actions { display: flex; gap: var(--space-3); margin-top: var(--space-2); }
  .btn { flex: 1; height: 48px; border-radius: var(--radius-md); font-size: var(--text-sm); font-weight: 500; }
  .btn-cancel { background: var(--bg-hover); color: var(--text-secondary); }
  .btn-confirm { background: var(--accent); color: #fff; }
  .btn-confirm.danger { background: var(--error); color: #fff; }
</style>
```

- [ ] **Step 2: Verificar + Commit**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/components/ConfirmSheet.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(ui): generic ConfirmSheet (bottom-sheet confirm)"
```

---

### Task 4: Spinner (renomear ActivityBadge → linha slim sem bubble)

**Files:**
- Create: `frontend/src/components/Spinner.svelte`
- Delete: `frontend/src/components/ActivityBadge.svelte`
- Modify: `frontend/src/components/MessageList.svelte`, `frontend/src/screens/Chat.svelte`

- [ ] **Step 1: Criar `Spinner.svelte`** (sem cost, sem stop, slim)

Create `frontend/src/components/Spinner.svelte`:
```svelte
<script lang="ts">
  import { onDestroy } from 'svelte';

  interface Props {
    label?: string | null;
  }
  let { label = null }: Props = $props();

  const stateLabel = $derived(label ?? 'Trabalhando…');

  // Cronometro vivo de liveness (igual "(20s…)" do Claude Code). Conta do mount; o tempo de
  // sessao AUTORITATIVO vive no UsageSheet (status.sessionTime).
  let elapsedLabel = $state('00:00');
  let startedAt = Date.now();
  function fmtElapsed(ms: number): string {
    const total = Math.max(0, Math.floor(ms / 1000));
    const mm = Math.floor(total / 60);
    const ss = total % 60;
    return String(mm).padStart(2, '0') + ':' + String(ss).padStart(2, '0');
  }
  const timer = setInterval(() => {
    elapsedLabel = fmtElapsed(Date.now() - startedAt);
  }, 1000);
  onDestroy(() => clearInterval(timer));
</script>

<div class="spinner" role="status" aria-live="polite">
  <span class="dot" aria-hidden="true"></span>
  <span class="spinner-label">{stateLabel}</span>
  <span class="spinner-time">{elapsedLabel}</span>
</div>

<style>
  /* Linha slim, sem bubble/card — estilo da linha de spinner do Claude Code. */
  .spinner { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-1); animation: bubble-in 200ms var(--ease-out); }
  .dot { width: 7px; height: 7px; border-radius: var(--radius-full); flex-shrink: 0; background: var(--pill-working-fg); animation: pulse-scale 1.4s ease-in-out infinite; }
  .spinner-label { font-size: var(--text-sm); color: var(--text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .spinner-time { font-family: var(--font-mono); font-size: var(--text-xs); font-variant-numeric: tabular-nums; color: var(--text-muted); flex-shrink: 0; }
</style>
```

- [ ] **Step 2: Deletar ActivityBadge**

Run: `git -C /home/jefferson/pessoal/claude-pocket rm frontend/src/components/ActivityBadge.svelte`

- [ ] **Step 3: MessageList — usar Spinner, remover prop `costUsd`**

Trocar `import ActivityBadge from './ActivityBadge.svelte';` por `import Spinner from './Spinner.svelte';`.
Remover `costUsd` da `Props` interface e do `$props()` (volta a):
```svelte
  interface Props {
    events: ChatEvent[];
    stateEvent: StateEvent | null;
    pending: { id: string; text: string }[];
    onSelectOption: (i: number) => void;
    onCancel: () => void;
  }

  let { events, stateEvent, pending, onSelectOption, onCancel }: Props = $props();
```
Trocar o render:
```svelte
    {#if stateEvent?.state === 'working'}
      <ActivityBadge label={stateEvent.label} {costUsd} onCancel={onCancel} />
    {/if}
```
por:
```svelte
    {#if stateEvent?.state === 'working'}
      <Spinner label={stateEvent.label} />
    {/if}
```

- [ ] **Step 4: Chat — remover `costUsd` do `<MessageList>`**

Remover a linha `costUsd={status?.costUsd}` do `<MessageList>` (o `onCancel` continua).

- [ ] **Step 5: Verificar + Commit**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
Expected: 0 errors / 0 warnings.
```bash
git -C /home/jefferson/pessoal/claude-pocket add -A frontend/src/components/Spinner.svelte frontend/src/components/MessageList.svelte frontend/src/screens/Chat.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "refactor(chat): ActivityBadge -> slim Spinner (non-bubble, no cost/stop)"
```

---

### Task 5: Wire — NavBar(5h/7d) + Composer(cost + stop-confirm) + Chat(UsageSheet)

**Files:**
- Modify: `frontend/src/components/NavBar.svelte`
- Modify: `frontend/src/components/Composer.svelte`
- Modify: `frontend/src/screens/Chat.svelte`

- [ ] **Step 1: NavBar — aceitar `status` + `onExpandUsage`, render RateChips no slot direito**

Em `frontend/src/components/NavBar.svelte`, adicionar imports e props:
```svelte
  import RateChips from './RateChips.svelte';
  import type { StatusFields } from '../lib/statusline';
```
Estender a interface e o destructure:
```svelte
  interface Props {
    title?: string;
    showBack?: boolean;
    onBack?: () => void;
    onMenu?: () => void;
    onTitleTap?: () => void;
    status?: StatusFields | null;
    onExpandUsage?: () => void;
  }
  let { title = 'claude pocket', showBack = false, onBack, onMenu, onTitleTap, status = null, onExpandUsage }: Props = $props();
```
No markup, trocar o bloco direito (o `{#if onMenu} ... {:else}<div class="nav-spacer"></div>{/if}`) por: chips de rate quando houver status+callback; senão o menu; senão o spacer:
```svelte
    {#if status && onExpandUsage}
      <RateChips {status} onExpand={onExpandUsage} />
    {:else if onMenu}
      <button class="nav-btn menu-btn" onclick={onMenu} aria-label="Menu">
        <svg width="20" height="5" viewBox="0 0 20 5" fill="currentColor" aria-hidden="true">
          <circle cx="2.5" cy="2.5" r="2.5"/>
          <circle cx="10" cy="2.5" r="2.5"/>
          <circle cx="17.5" cy="2.5" r="2.5"/>
        </svg>
      </button>
    {:else}
      <div class="nav-spacer"></div>
    {/if}
```

- [ ] **Step 2: Composer — chip de custo no control-left + stop-confirm**

Em `frontend/src/components/Composer.svelte`:
- Re-adicionar imports: `IconInterrupt` (`./icons/IconInterrupt.svelte`), `UsageSheet`? NÃO — o UsageSheet vive no Chat. Adicionar só `ConfirmSheet` (`./ConfirmSheet.svelte`).
- Estender `Props` com `onInterrupt` e `onExpandUsage`:
```svelte
  interface Props {
    sessionName: string;
    sessionState: State;
    status: StatusFields | null;
    onSend: (text: string) => void;
    onCommand: (cmd: string) => void;
    onInterrupt: () => void;
    onExpandUsage: () => void;
  }
  let { sessionName, sessionState, status, onSend, onCommand, onInterrupt, onExpandUsage }: Props = $props();
```
- Re-adicionar `const isWorking = $derived(sessionState === 'working');` (perto do `canSend`).
- Adicionar estado: `let confirmStopOpen = $state(false);`
- No `control-left`, DEPOIS do `model-pill` e ANTES do `slash-btn`, inserir o chip de custo (só se houver):
```svelte
        {#if typeof status?.costUsd === 'number'}
          <button class="cost-chip" onclick={onExpandUsage} aria-label="Custo e uso">
            ${status.costUsd.toFixed(2)}
          </button>
        {/if}
```
- Trocar `control-right` (hoje só send) por stop(working)+send:
```svelte
      <div class="control-right">
        {#if isWorking}
          <button class="stop-btn" onclick={() => (confirmStopOpen = true)} aria-label="Interromper Claude">
            <IconInterrupt size={16} />
          </button>
        {/if}
        <button
          class="send-btn"
          class:send-btn--disabled={!canSend}
          onclick={submit}
          disabled={!canSend}
          aria-label="Enviar mensagem"
        >
          <IconSend size={18} />
        </button>
      </div>
```
- Montar o ConfirmSheet junto dos outros sheets no fim do `<footer>`:
```svelte
  <ConfirmSheet
    open={confirmStopOpen}
    title="Interromper o Claude?"
    message="Isso envia ESC e para a resposta atual."
    confirmLabel="Interromper"
    danger={true}
    onConfirm={onInterrupt}
    onClose={() => (confirmStopOpen = false)}
  />
```
- CSS: garantir `.control-right { display: flex; align-items: center; gap: var(--space-2); flex-shrink: 0; }`. Re-adicionar `.stop-btn` (removido na sessão anterior):
```svelte
  .stop-btn { width: 44px; height: 44px; flex-shrink: 0; background: transparent; border: 1px solid var(--error); border-radius: var(--radius-md); color: var(--error); transition: background 180ms var(--ease-out); }
  .stop-btn:active { background: rgba(255, 69, 58, 0.08); }
```
E o chip de custo (compacto, como o model-pill):
```svelte
  .cost-chip { display: inline-flex; align-items: center; height: 28px; min-height: 0; padding: 0 var(--space-2); background: var(--bg-hover); border-radius: var(--radius-md); font-family: var(--font-mono); font-size: var(--text-xs); font-variant-numeric: tabular-nums; color: var(--text-secondary); white-space: nowrap; flex-shrink: 0; }
```

- [ ] **Step 3: Chat — dono do UsageSheet; passar status/onExpandUsage/onInterrupt**

Em `frontend/src/screens/Chat.svelte`:
- Import: `import UsageSheet from '../components/UsageSheet.svelte';`
- Estado: `let usageOpen = $state(false);`
- NavBar (linha ~172) passa status + onExpandUsage:
```svelte
  <NavBar title={sessionName} showBack={true} onBack={onBack} onTitleTap={openSwitcher} {status} onExpandUsage={() => (usageOpen = true)} />
```
- Composer passa onInterrupt + onExpandUsage:
```svelte
      <Composer
        {sessionName}
        sessionState={currentState}
        status={status}
        onSend={handleSend}
        onCommand={handleCommand}
        onInterrupt={handleInterrupt}
        onExpandUsage={() => (usageOpen = true)}
      />
```
- Montar o UsageSheet (perto dos outros sheets, ex depois do `<CreateSessionSheet>`):
```svelte
  <UsageSheet open={usageOpen} {status} onClose={() => (usageOpen = false)} />
```

- [ ] **Step 4: Verificar + Commit**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
Expected: 0 errors / 0 warnings. Grep conferindo que `RateChips`, `UsageSheet`, `ConfirmSheet`, `IconInterrupt`, `isWorking`, `onInterrupt`, `onExpandUsage`, `cost-chip` estão todos usados.
```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/components/NavBar.svelte frontend/src/components/Composer.svelte frontend/src/screens/Chat.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(metrics): 5h/7d chips in NavBar, cost chip + stop-confirm in composer, UsageSheet in Chat"
```

---

### Task 6: Verificação manual no aparelho

- [ ] Re-parear (token estável `B_cCngF3YyM31J3CAOMMK9-e`) se 401. Recarregar o PWA.
- [ ] **NavBar**: chips `⚡5h%` `📅7d%` no topo, cor por saturação; tap abre UsageSheet.
- [ ] **Composer**: chip `$custo` no control-left (sempre, inclusive idle); ctx no ring + modelo no pill, sem duplicar; tap no custo abre UsageSheet.
- [ ] **UsageSheet**: 5h/7d com reset completo, ctx abs, custo, tempo de sessão, statusline crua no rodapé.
- [ ] **Stop**: working → stop ao lado do enviar; tap → confirm; confirmar interrompe, cancelar não.
- [ ] **Spinner**: working → linha slim (sem bubble) no fim da lista, atividade + cronômetro.
- [ ] **Harmonia**: nada empilhado/duplicado; 5h/7d no topo, custo discreto no composer, detalhe sob demanda.
- [ ] Atualizar handoff.

---

## Cobertura (self-review)
- 5h/7d restaurados, espalhados no topo → RateChips (Task 1) + NavBar wire (Task 5).
- ctx/modelo NÃO duplicados (ficam no ring/pill) → Task 5 não adiciona chips deles.
- cost des-gated e discreto no composer → cost-chip (Task 5); removido do spinner (Task 4).
- tempo de sessão autoritativo + tokens abs + resets → UsageSheet (Task 2).
- Stop no composer + confirm → Task 5 + ConfirmSheet (Task 3).
- Spinner slim não-bubble → Task 4.
- Consistência de props: `RateChips {status,onExpand}`; `UsageSheet {open,status,onClose}`; `ConfirmSheet {open,title,message?,confirmLabel?,danger?,onConfirm,onClose}`; `Spinner {label}`; NavBar +{status?,onExpandUsage?}; Composer +{onInterrupt,onExpandUsage}.
