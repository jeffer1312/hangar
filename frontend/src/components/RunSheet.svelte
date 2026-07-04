<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { getRunners, startRun, stopRun, getRunPane } from '../lib/api';
  import type { Runner, RunInfo } from '../lib/types';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  let detected = $state<Runner[]>([]);
  let running = $state<RunInfo | null>(null);
  let picking = $state(false);      // mostrando a lista pra escolher
  let paneText = $state('');
  let err = $state<string | null>(null);
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  async function load() {
    err = null;
    try {
      const r = await getRunners(sessionName);
      detected = r.detected;
      running = r.running;
      picking = !running && !r.remembered;         // sem run e sem lembrado -> escolher
      if (!running && r.remembered) await run(r.remembered);
    } catch (e) {
      err = String(e);
    }
  }

  async function run(command: string) {
    err = null;
    try {
      running = await startRun(sessionName, command);
      picking = false;
    } catch (e) {
      err = String(e);
    }
  }

  async function stop() {
    try { await stopRun(sessionName); } catch (e) { err = String(e); }
    running = null;
    paneText = '';
    picking = true;
  }

  async function poll() {
    if (!running) return;
    try { paneText = (await getRunPane(sessionName)).pane; } catch { /* transitorio */ }
  }

  $effect(() => {
    if (open) {
      load();
      pollTimer = setInterval(poll, 1000);
    }
    return () => {
      if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
    };
  });
</script>

<BottomSheet {open} {onClose} ariaLabel="Rodar projeto">
  <h2 class="sheet-title">Rodar</h2>

  {#if err}<p class="err">{err}</p>{/if}

  {#if picking}
    {#if detected.length === 0}
      <p class="empty">Nenhum script detectado neste projeto.</p>
    {:else}
      <ul class="run-list">
        {#each detected as r (r.source + ':' + r.label)}
          <li>
            <button class="run-row" class:guess={r.is_dev_guess} onclick={() => run(r.command)}>
              <span class="run-label">{r.label}</span>
              <span class="run-cmd">{r.command}</span>
              {#if r.is_dev_guess}<span class="badge">dev</span>{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  {:else}
    <div class="run-head">
      <span class="run-cmd">{running?.command ?? ''}</span>
      <div class="run-actions">
        <button class="act" onclick={() => (picking = true)}>Trocar</button>
        <button class="act act--stop" onclick={stop}>Parar</button>
      </div>
    </div>
    <pre class="mirror">{paneText}</pre>
  {/if}
</BottomSheet>

<style>
  .sheet-title { font-size: var(--text-xl); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-4); }
  .err { color: var(--error); font-size: var(--text-sm); margin-bottom: var(--space-2); }
  .empty { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: var(--space-4) 0; }
  .run-list { list-style: none; display: flex; flex-direction: column; gap: var(--space-1); }
  .run-row { width: 100%; display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md); text-align: left; background: transparent; }
  .run-row:active { background: var(--bg-hover); }
  .run-row.guess { background: var(--accent-dim); }
  .run-label { font-family: var(--font-mono); font-weight: 600; color: var(--text-primary); }
  .run-cmd { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; }
  .badge { flex-shrink: 0; font-size: 10px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em;
    padding: 2px 6px; border-radius: var(--radius-full); color: var(--accent); background: var(--accent-dim); }
  .run-head { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); margin-bottom: var(--space-3); }
  .run-actions { display: flex; gap: var(--space-2); flex-shrink: 0; }
  .act { padding: 0 var(--space-3); height: 32px; border-radius: var(--radius-sm); font-size: var(--text-sm);
    font-weight: 600; color: var(--text-secondary); background: var(--bg-hover); }
  .act--stop { color: #fff; background: var(--error); }
  .mirror { font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.4; color: var(--text-secondary);
    background: var(--bg-surface); border-radius: var(--radius-md); padding: var(--space-3); max-height: 52vh;
    overflow: auto; white-space: pre-wrap; word-break: break-word; }
</style>
