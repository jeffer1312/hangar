<script lang="ts">
  import { onDestroy } from 'svelte';
  import { terminateCodexCommand, type CodexCommand } from '@hangar/core';
  import { useSessionServer } from '../lib/sessionServer';
  import * as m from '../paraglide/messages';
  import IconChevronDown from './icons/IconChevronDown.svelte';

  let { commands, output, sessionName, readOnly = false }: {
    commands: CodexCommand[]; output: Record<string, string>; sessionName: string; readOnly?: boolean;
  } = $props();
  const sessionServer = useSessionServer();
  let opened = $state<string | null>(null);
  let pending = $state<string | null>(null);
  let error = $state('');
  let sequence = 0;
  onDestroy(() => { sequence += 1; });
  $effect(() => {
    if (pending && !commands.some(command => command.id === pending)) { sequence += 1; pending = null; }
    if (opened && !commands.some(command => command.id === opened)) opened = null;
  });
  async function stop(command: CodexCommand) {
    const server = sessionServer();
    if (!server || pending || readOnly) return;
    const request = ++sequence;
    pending = command.id;
    error = '';
    try { await terminateCodexCommand(sessionName, command.process_id, server); }
    catch (cause) {
      if (sequence !== request) return;
      pending = null;
      error = cause instanceof Error ? cause.message : m.codex_command_stop_failed();
    }
  }
</script>

{#if commands.length}
  <section aria-label={m.codex_commands_title()}>
    <h3>{m.codex_commands_title()}</h3>
    {#each commands as command (command.id)}
      <div class="command">
        <div class="row">
          <button class="open" onclick={() => (opened = opened === command.id ? null : command.id)}
            aria-expanded={opened === command.id} aria-label={`${m.codex_command_output()}: ${command.command}`}>
            <span class="chevron" class:closed={opened !== command.id} aria-hidden="true"><IconChevronDown size={14} /></span>
            <span class="body"><code>{command.command}</code>
              <span class="meta">pid {command.process_id} · {command.background ? m.codex_terminals_count({ n: 1 }) : m.native_tool_running()}</span>
            </span>
          </button>
          {#if !readOnly}
            <button class="stop" disabled={pending !== null || !sessionServer()} onclick={() => void stop(command)}
              aria-label={m.codex_command_stop_label({ command: command.command })}>
              {pending === command.id ? m.codex_command_stopping() : m.codex_command_stop()}
            </button>
          {/if}
        </div>
        {#if opened === command.id}<pre>{output[command.id] || m.codex_command_no_output()}</pre>{/if}
      </div>
    {/each}
    {#if error}<p role="alert">{error}</p>{/if}
  </section>
{/if}

<style>
  section { display: flex; flex-direction: column; gap: 8px; }
  h3 { font-size: var(--text-xs); color: var(--text-muted); font-weight: 600; margin: 0; }
  .row { display: flex; align-items: center; gap: 8px; }
  button { min-height: 44px; background: transparent; border: 0; color: var(--text-primary); cursor: pointer; }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  button:disabled { opacity: .5; cursor: default; }
  .open { flex: 1; min-width: 0; display: flex; gap: 8px; align-items: center; text-align: left; }
  .body { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  code { font-family: var(--font-mono); font-size: var(--text-sm); overflow-wrap: anywhere; white-space: pre-wrap; }
  .meta { color: var(--text-muted); font-size: var(--text-xs); }
  .chevron { flex-shrink: 0; display: inline-flex; }
  .chevron.closed { transform: rotate(-90deg); }
  .stop { color: var(--accent); flex-shrink: 0; }
  pre { position: relative; max-height: 240px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; font-family: var(--font-mono); font-size: var(--text-sm); padding: 8px; margin: 0; background: var(--surface-inset); }
  p { color: var(--error); font-size: var(--text-sm); }
</style>
