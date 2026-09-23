<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { desktop } from '../lib/desktop.svelte';
  import { getRunners, startRun, stopRun, getRunPane, setCustomRunners } from '@hangar/core';
  import type { Runner, RunInfo } from '@hangar/core';
  import * as m from '../paraglide/messages';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
    onRunningChange?: (running: boolean) => void;
  }
  let { open, sessionName, onClose, onRunningChange }: Props = $props();

  let detected = $state<Runner[]>([]);
  let custom = $state<Runner[]>([]);
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
      custom = r.custom ?? [];
      running = r.running;
      picking = !running && !r.remembered;         // sem run e sem lembrado -> escolher
      if (!running && r.remembered) await run(r.remembered);
    } catch (e) {
      err = String(e);
    }
    onRunningChange?.(!!running);
  }

  async function run(command: string) {
    err = null;
    try {
      running = await startRun(sessionName, command);
      picking = false;
    } catch (e) {
      err = String(e);
    }
    onRunningChange?.(!!running);
  }

  async function stop() {
    try { await stopRun(sessionName); } catch (e) { err = String(e); }
    running = null;
    paneText = '';
    picking = true;
    onRunningChange?.(!!running);
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

  // ── Comandos personalizados (CRUD, lista inteira vai num POST só) ───────────
  let formOpen = $state(false);
  let editIdx = $state<number | null>(null);   // índice em `custom`; null = novo
  let fLabel = $state('');
  let fCmd = $state('');
  let savingCustom = $state(false);
  const formValid = $derived(!!fLabel.trim() && !!fCmd.trim());

  function openNew() {
    editIdx = null; fLabel = ''; fCmd = '';
    formOpen = true;
  }
  function openEdit(i: number) {
    editIdx = i; fLabel = custom[i].label; fCmd = custom[i].command;
    formOpen = true;
  }
  async function saveCustom(commands: { label: string; command: string }[]) {
    savingCustom = true;
    err = null;
    try {
      custom = await setCustomRunners(sessionName, commands);
      formOpen = false;
    } catch (e) {
      err = String(e);
    } finally {
      savingCustom = false;
    }
  }
  function submitForm() {
    if (!formValid) return;
    const item = { label: fLabel.trim(), command: fCmd.trim() };
    const base = custom.map((c) => ({ label: c.label, command: c.command }));
    void saveCustom(editIdx === null ? [...base, item] : base.map((c, i) => (i === editIdx ? item : c)));
  }
  function removeCustom(i: number) {
    void saveCustom(custom.filter((_, k) => k !== i).map((c) => ({ label: c.label, command: c.command })));
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={m.ctx_rodar_projeto()} centered={desktop.atual}>
  <h2 class="sheet-title">{m.ctx_rodar()}</h2>

  {#if err}<p class="err">{err}</p>{/if}

  {#if picking}
    {#if custom.length > 0}
      <p class="secao">{m.run_personalizados()}</p>
      <ul class="run-list">
        {#each custom as r, i (r.label + ':' + r.command)}
          <li class="run-item">
            <button class="run-row" onclick={() => run(r.command)}>
              <span class="run-label">{r.label}</span>
              <span class="run-cmd">{r.command}</span>
            </button>
            <button class="mini" onclick={() => openEdit(i)} aria-label={m.atalhos_editar()}>✎</button>
            <button class="mini" onclick={() => removeCustom(i)} disabled={savingCustom}
                    aria-label={m.atalhos_remover()}>✕</button>
          </li>
        {/each}
      </ul>
    {/if}

    {#if formOpen}
      <div class="form">
        <input type="text" bind:value={fLabel} placeholder={m.atalhos_rotulo()}
               aria-label={m.atalhos_rotulo()} maxlength="32" />
        <input type="text" bind:value={fCmd} placeholder={m.run_comando_dica()}
               aria-label={m.atalhos_comando()} />
        <div class="form-acoes">
          <button class="act" onclick={() => (formOpen = false)}>{m.comum_cancelar()}</button>
          <button class="act act--ok" onclick={submitForm} disabled={!formValid || savingCustom}>
            {m.comum_confirmar()}
          </button>
        </div>
      </div>
    {:else}
      <button class="add" onclick={openNew}>{m.run_add_comando()}</button>
    {/if}

    {#if detected.length === 0 && custom.length === 0}
      <p class="empty">{m.run_nenhum_script()}</p>
    {:else if detected.length > 0}
      {#if custom.length > 0}<p class="secao">{m.run_detectados()}</p>{/if}
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
        <button class="act" onclick={() => (picking = true)}>{m.run_trocar()}</button>
        <button class="act act--stop" onclick={stop}>{m.loop_parar()}</button>
      </div>
    </div>
    <pre class="mirror">{paneText}</pre>
  {/if}
</BottomSheet>

<style>
  .sheet-title { font-size: var(--text-xl); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-4); }
  .err { color: var(--error); font-size: var(--text-sm); margin-bottom: var(--space-2); }
  .empty { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: var(--space-4) 0; }
  .secao { font-size: 11px; font-weight: 700; letter-spacing: 0.05em; text-transform: uppercase;
    color: var(--text-muted); margin: var(--space-3) 0 var(--space-1); }
  .run-list { list-style: none; display: flex; flex-direction: column; gap: var(--space-1); }
  .run-item { display: flex; align-items: center; gap: 2px; }
  .run-item .run-row { flex: 1; min-width: 0; }
  .run-row { width: 100%; display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md); text-align: left; background: transparent; }
  .run-row:active { background: var(--bg-hover); }
  .run-row.guess { background: var(--accent-dim); }
  .run-label { font-family: var(--font-mono); font-weight: 600; color: var(--text-primary); }
  .run-cmd { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; }
  .mini { min-width: 32px; min-height: 32px; flex-shrink: 0; border-radius: var(--radius-sm);
    color: var(--text-secondary); background: transparent; font-size: var(--text-sm); }
  .mini:active { background: var(--bg-hover); }
  .mini:disabled { opacity: 0.4; }
  .add { align-self: flex-start; margin-top: var(--space-1); padding: 6px 12px; border-radius: var(--radius-md);
    font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary);
    background: transparent; border: 1px dashed var(--border-default); }
  .add:active { background: var(--bg-hover); }
  .form { display: flex; flex-direction: column; gap: var(--space-2); margin: var(--space-2) 0;
    padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: var(--surface-inset); }
  .form input { padding: 8px 10px; border-radius: var(--radius-sm); border: 1px solid var(--border-subtle);
    background: var(--surface-raised); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); }
  .form-acoes { display: flex; justify-content: flex-end; gap: var(--space-2); }
  .badge { flex-shrink: 0; font-size: 10px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em;
    padding: 2px 6px; border-radius: var(--radius-full); color: var(--accent); background: var(--accent-dim); }
  .run-head { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); margin-bottom: var(--space-3); }
  .run-actions { display: flex; gap: var(--space-2); flex-shrink: 0; }
  .act { padding: 0 var(--space-3); height: 32px; border-radius: var(--radius-sm); font-size: var(--text-sm);
    font-weight: 600; color: var(--text-secondary); background: var(--bg-hover); }
  .act--ok { color: #fff; background: var(--accent); }
  .act--stop { color: #fff; background: var(--error); }
  .mirror { font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.4; color: var(--text-secondary);
    background: var(--bg-surface); border-radius: var(--radius-md); padding: var(--space-3); max-height: 52vh;
    overflow: auto; white-space: pre-wrap; word-break: break-word; }
</style>
