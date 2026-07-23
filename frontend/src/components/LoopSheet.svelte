<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { getLoopForServer, createLoopForServer, stopLoopForServer, resolveLoopForServer } from '../lib/api';
  import { listServers, getActiveId } from '../lib/auth';
  import type { Server } from '../lib/auth';
  import type { LoopState } from '../lib/types';
  import { loopBadge } from '../lib/loop';
  import { LOOP_GUIDE } from '../lib/loopGuide';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  // Sem prop serverId (padrao do GitSheet): o chamador ja fez selectServer(serverId) antes de
  // montar e restaura o ativo no fechar -> aqui mira SEMPRE o servidor ATIVO.
  function activeServer(): Server | null {
    return listServers().find((s) => s.id === getActiveId()) ?? null;
  }

  const FINAL = new Set(['done', 'stopped', 'exhausted', 'failed']);
  const STATUS_LABEL: Record<string, string> = {
    running: 'rodando',
    paused_awaiting: 'aguardando input',
    done_claimed: 'pronto (aguardando confirmação)',
    done: 'concluído',
    stopped: 'parado',
    exhausted: 'esgotou iterações',
    failed: 'falhou',
  };
  const TONE_COLOR: Record<string, string> = {
    ok: 'var(--success)', warn: 'var(--warning)', attention: 'var(--warning)', muted: 'var(--text-muted)',
  };

  let loop = $state<LoopState | null>(null);
  let suggestions = $state<string[]>([]);
  let loadErr = $state('');
  let forceForm = $state(false);   // "novo loop" clicado num estado final -> mostra o form de novo

  const isFinal = $derived(!!loop && FINAL.has(loop.status));
  const isForm = $derived(!loop || (isFinal && forceForm));
  const isPolling = $derived(!!loop && !isFinal);   // running / paused_awaiting / done_claimed

  function cleanErr(e: unknown): string {
    const m = e instanceof Error ? e.message : 'falhou';
    return m.replace(/^\d+:\s*/, '');   // tira o prefixo "409: " do status HTTP
  }

  async function load() {
    const s = activeServer();
    if (!s) { loadErr = 'servidor não encontrado'; return; }
    try {
      const r = await getLoopForServer(s, sessionName);
      loop = r.loop;
      suggestions = r.suggestions;
    } catch (e) {
      loadErr = cleanErr(e);
    }
  }

  // ── Form (novo loop) ──────────────────────────────────────────────────────
  let goal = $state('');
  let checkCmd = $state('');
  let maxIters = $state(10);
  let requireBranch = $state(true);
  let creating = $state(false);
  let createErr = $state('');
  let guideOpen = $state(false);
  let guideOpenSections = $state<Set<number>>(new Set());

  function resetForm() {
    goal = ''; checkCmd = ''; maxIters = 10; requireBranch = true; createErr = '';
    guideOpen = false; guideOpenSections = new Set();
  }

  function toggleGuideSection(i: number) {
    const next = new Set(guideOpenSections);
    if (next.has(i)) next.delete(i); else next.add(i);
    guideOpenSections = next;
  }

  async function startLoop() {
    const s = activeServer();
    if (!s || !goal.trim() || creating) return;
    creating = true; createErr = '';
    try {
      const r = await createLoopForServer(s, sessionName, {
        goal: goal.trim(),
        check_cmd: checkCmd.trim() || null,
        max_iters: maxIters,
        require_branch: requireBranch,
      });
      loop = r.loop;
      forceForm = false;
    } catch (e) {
      createErr = cleanErr(e);   // 409 (loop já ativo) cai aqui, texto do detail
    } finally {
      creating = false;
    }
  }

  // ── Loop ativo ───────────────────────────────────────────────────────────
  let expandedHist = $state<number | null>(null);
  function toggleHist(n: number) { expandedHist = expandedHist === n ? null : n; }
  function firstLine(t: string): string { return t.split('\n')[0] ?? ''; }

  let confirmStop = $state(false);
  let stopErr = $state('');
  async function doStop() {
    const s = activeServer();
    confirmStop = false;
    if (!s) return;
    try { loop = (await stopLoopForServer(s, sessionName)).loop; }
    catch (e) { stopErr = cleanErr(e); }
  }

  let resolving = $state(false);
  async function resolveClaim(accept: boolean) {
    const s = activeServer();
    if (!s || resolving) return;
    resolving = true;
    try { loop = (await resolveLoopForServer(s, sessionName, accept)).loop; }
    catch (e) { stopErr = cleanErr(e); }
    finally { resolving = false; }
  }

  // Recarrega a cada abertura; zera o form e o override de "novo loop".
  $effect(() => {
    if (open) {
      loadErr = ''; stopErr = ''; forceForm = false; expandedHist = null;
      resetForm();
      load();
    }
  });

  // Polling leve so com a sheet aberta e loop em status ativo. Depende de `isPolling` (derived
  // booleano) e nao de `loop` cru -> nao reinicia o setInterval a cada tick (so quando MUDA de fase).
  $effect(() => {
    if (!open || !isPolling) return;
    const id = setInterval(load, 3000);
    return () => clearInterval(id);
  });
</script>

<BottomSheet {open} {onClose} ariaLabel="Loop">
  <div class="loop">
    <h2 class="loop-title">Loop</h2>

    {#if loadErr}<p class="error-msg" role="alert">{loadErr}</p>{/if}

    {#if isForm}
      <div class="field">
        <label class="field-label" for="loop-goal">Objetivo</label>
        <textarea
          id="loop-goal" class="field-input loop-textarea" bind:value={goal} rows="4"
          placeholder="ex: migre utils/date.ts pra date-fns e mantenha npm run check verde"
        ></textarea>
      </div>

      <div class="field">
        <label class="field-label" for="loop-check">Check (comando que decide se acabou)</label>
        <input
          id="loop-check" type="text" class="field-input" bind:value={checkCmd}
          placeholder="npm run check" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
        />
        {#if suggestions.length}
          <div class="loop-chips">
            {#each suggestions as sug (sug)}
              <button type="button" class="chip" class:on={checkCmd === sug} onclick={() => (checkCmd = sug)}>{sug}</button>
            {/each}
          </div>
        {/if}
      </div>

      <div class="field">
        <label class="field-label" for="loop-max">Máx. iterações</label>
        <input id="loop-max" type="number" class="field-input loop-max-input" bind:value={maxIters} min="1" />
      </div>

      <div class="field">
        <span class="field-label">Exigir branch</span>
        <div class="provider-toggle" role="group" aria-label="Exigir branch">
          <button type="button" class="provider-btn" class:on={requireBranch} onclick={() => (requireBranch = true)}>Sim</button>
          <button type="button" class="provider-btn" class:on={!requireBranch} onclick={() => (requireBranch = false)}>Não</button>
        </div>
      </div>

      {#if createErr}<p class="error-msg" role="alert">{createErr}</p>{/if}

      <button class="primary-btn" onclick={startLoop} disabled={creating || !goal.trim()}>
        {creating ? 'Iniciando…' : 'Iniciar loop'}
      </button>

      <button type="button" class="guide-toggle" onclick={() => (guideOpen = !guideOpen)}>
        <span>? Como escrever um bom loop</span>
        <span class="chevron" class:chevron--open={guideOpen} aria-hidden="true">›</span>
      </button>
      {#if guideOpen}
        <div class="loop-guide">
          {#each LOOP_GUIDE as sec, i (sec.title)}
            <div class="loop-guide-sec">
              <button type="button" class="loop-guide-head" onclick={() => toggleGuideSection(i)}>
                <span>{sec.title}</span>
                <span class="chevron" class:chevron--open={guideOpenSections.has(i)} aria-hidden="true">›</span>
              </button>
              {#if guideOpenSections.has(i)}
                <p class="loop-guide-body">{sec.body}</p>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    {:else if loop}
      <div class="loop-status-row">
        <span class="loop-status-dot" style="background: {TONE_COLOR[loopBadge(loop.status)?.tone ?? 'muted']};"></span>
        <span class="loop-status-label">{STATUS_LABEL[loop.status] ?? loop.status}</span>
        <span class="loop-iter">{loop.iter}/{loop.max_iters}</span>
      </div>
      <p class="loop-goal">{loop.goal}</p>

      {#if loop.status === 'done_claimed'}
        <div class="loop-claim">
          <p class="loop-claim-msg">O loop terminou e marcou pronto. Confirma?</p>
          <div class="loop-claim-actions">
            <button class="primary-btn" onclick={() => resolveClaim(true)} disabled={resolving}>Confirmar pronto</button>
            <button class="ghost-btn" onclick={() => resolveClaim(false)} disabled={resolving}>Rejeitar (continuar)</button>
          </div>
        </div>
      {:else if isFinal}
        {#if loop.ended_reason}<p class="loop-reason">{loop.ended_reason}</p>{/if}
        <button class="primary-btn" onclick={() => (forceForm = true)}>Novo loop</button>
      {/if}

      {#if loop.history.length}
        <div class="loop-history">
          {#each loop.history as h (h.n)}
            <div class="loop-hist-row">
              <button type="button" class="loop-hist-line" onclick={() => toggleHist(h.n)}>
                {h.n} · exit {h.check_exit ?? '—'} · {firstLine(h.tail)}
              </button>
              {#if expandedHist === h.n}
                <pre class="loop-hist-tail">{h.tail}</pre>
              {/if}
            </div>
          {/each}
        </div>
      {/if}

      {#if stopErr}<p class="error-msg" role="alert">{stopErr}</p>{/if}

      {#if isPolling && loop.status !== 'done_claimed'}
        <button class="ghost-btn loop-stop-btn" onclick={() => (confirmStop = true)}>Parar loop</button>
      {/if}
    {/if}
  </div>
</BottomSheet>

{#if confirmStop}
  <ConfirmDialog
    title="Parar este loop?"
    aria="Parar loop"
    actions={[
      { label: 'Cancelar', onClick: () => (confirmStop = false) },
      { label: 'Parar', kind: 'danger', onClick: doStop },
    ]}
    onClose={() => (confirmStop = false)}
  />
{/if}

<style>
  .loop { display: flex; flex-direction: column; gap: var(--space-2); }
  .loop-title { font-size: var(--text-xl); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-2); }

  .field { display: flex; flex-direction: column; gap: var(--space-2); margin-bottom: var(--space-4); }
  .field-label { font-size: var(--text-sm); color: var(--text-secondary); font-weight: 500; }
  .field-input {
    height: 44px; background: var(--bg-surface); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); color: var(--text-primary); font-family: var(--font-ui);
    font-size: 16px; padding: 0 var(--space-3); outline: none; transition: border-color 180ms var(--ease-out);
  }
  .field-input::placeholder { color: var(--text-muted); }
  .field-input:focus { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-dim); }
  .loop-textarea { height: auto; padding: var(--space-3); resize: vertical; font-size: var(--text-sm); line-height: 1.4; }
  .loop-max-input { width: 100px; }

  .loop-chips { display: flex; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-1); }
  .chip {
    height: 30px; padding: 0 var(--space-3); border-radius: var(--radius-full);
    border: 1px solid var(--border-default); background: var(--bg-surface); color: var(--text-secondary);
    font-family: var(--font-mono); font-size: var(--text-xs); transition: border-color 160ms ease-out, color 160ms ease-out;
  }
  .chip.on { border-color: var(--accent); color: var(--text-primary); }

  .provider-toggle { display: flex; gap: var(--space-2); }
  .provider-btn {
    height: 34px; padding: 0 var(--space-4); border-radius: var(--radius-full);
    border: 1px solid var(--border-default); background: var(--bg-surface); color: var(--text-secondary);
    font-size: var(--text-sm); font-weight: 500; transition: border-color 160ms ease-out, color 160ms ease-out;
  }
  .provider-btn.on { border-color: var(--accent); color: var(--text-primary); }

  .error-msg { font-size: var(--text-sm); color: var(--error); margin-bottom: var(--space-3); }

  .primary-btn {
    width: 100%; height: 50px; background: var(--accent); border-radius: var(--radius-md);
    color: #fff; font-size: var(--text-base); font-weight: 600; transition: background 180ms var(--ease-out);
  }
  .primary-btn:active:not(:disabled) { background: var(--accent-press); }
  .primary-btn:disabled { opacity: 0.5; cursor: default; }

  .ghost-btn { width: 100%; height: 44px; margin-top: var(--space-2); color: var(--text-secondary); font-size: var(--text-sm); border-radius: var(--radius-md); }
  .ghost-btn:active { background: var(--bg-hover); }
  .loop-stop-btn { color: var(--error); }

  .guide-toggle {
    width: 100%; height: 44px; display: flex; align-items: center; justify-content: space-between;
    padding: 0 var(--space-1); margin-top: var(--space-2); color: var(--text-secondary); font-size: var(--text-sm);
    border-top: 1px solid var(--border-subtle);
  }
  .chevron { color: var(--text-muted); transition: transform 180ms var(--ease-out); }
  .chevron--open { transform: rotate(90deg); }

  .loop-guide { display: flex; flex-direction: column; gap: var(--space-1); margin-top: var(--space-2); }
  .loop-guide-sec { border-bottom: 1px solid var(--border-subtle); padding-bottom: var(--space-2); }
  .loop-guide-head {
    width: 100%; display: flex; align-items: center; justify-content: space-between;
    padding: var(--space-2) 0; color: var(--text-primary); font-size: var(--text-sm); font-weight: 500; text-align: left;
  }
  .loop-guide-body { margin: 0; padding-bottom: var(--space-2); color: var(--text-secondary); font-size: var(--text-sm); line-height: 1.5; }

  .loop-status-row { display: flex; align-items: center; gap: var(--space-2); }
  .loop-status-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  .loop-status-label { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .loop-iter { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); margin-left: auto; }
  .loop-goal { margin: 0 0 var(--space-3); color: var(--text-secondary); font-size: var(--text-sm); line-height: 1.4; }
  .loop-reason { margin: 0 0 var(--space-3); color: var(--text-muted); font-size: var(--text-sm); }

  .loop-claim {
    display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-3);
    margin-bottom: var(--space-3); background: var(--accent-dim); border-radius: var(--radius-md);
  }
  .loop-claim-msg { margin: 0; color: var(--text-primary); font-size: var(--text-sm); }
  .loop-claim-actions { display: flex; gap: var(--space-2); }
  .loop-claim-actions .primary-btn, .loop-claim-actions .ghost-btn { width: auto; flex: 1; margin-top: 0; }

  .loop-history {
    display: flex; flex-direction: column; gap: var(--space-1); margin-bottom: var(--space-3);
    max-height: 40vh; overflow-y: auto; -webkit-overflow-scrolling: touch;
  }
  .loop-hist-row { border-bottom: 1px solid var(--border-subtle); }
  .loop-hist-line {
    width: 100%; padding: var(--space-2) 0; text-align: left; color: var(--text-secondary);
    font-family: var(--font-mono); font-size: var(--text-xs); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .loop-hist-tail {
    margin: 0 0 var(--space-2); padding: var(--space-2); background: var(--bg-elevated);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: pre; overflow-x: auto;
  }
</style>
