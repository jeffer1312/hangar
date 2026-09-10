<script lang="ts">
  import { onDestroy } from 'svelte';
  import * as m from '../paraglide/messages';
  import IconMic from './icons/IconMic.svelte';
  import HangarMark from './icons/HangarMark.svelte';
  import BottomSheet from './BottomSheet.svelte';
  import { desktop } from '../lib/desktop.svelte';
  import { getActiveId, listServers } from '../lib/auth';
  import { CodexVoiceCall, type VoiceState, type VoiceFailure } from '../lib/codexVoice';
  import { getCodexVoicesForServer } from '@hangar/core';

  let { sessionName, disabled = false, onPrepare, onBusyChange }: {
    sessionName: string; disabled?: boolean; onPrepare: () => void; onBusyChange: (busy: boolean) => void;
  } = $props();
  const server = listServers().find(s => s.id === getActiveId());
  let open = $state(false);
  let callState = $state<VoiceState>('idle');
  let error = $state('');
  let detail = $state('');
  function savedVoice() {
    try { return localStorage.getItem('cp_codex_voice') ?? ''; }
    catch { return ''; }
  }
  let voice = $state(savedVoice());
  let voices = $state<string[]>([]);
  let muted = $state(false);
  let inputLevel = $state(0);
  let outputLevel = $state(0);
  let meterAvailable = $state(false);
  let pendingDraft = $state<string | null>(null);
  let audio: HTMLAudioElement;
  let call: CodexVoiceCall | undefined;
  const busy = $derived(callState === 'connecting' || callState === 'connected');

  function failure(code: VoiceFailure) {
    return { busy: m.codex_voice_busy(), unavailable: m.codex_voice_unavailable(),
      disabled: m.codex_voice_disabled(), closed: m.codex_voice_closed(),
      microphone: m.composer_sem_acesso_mic(), timeout: m.codex_voice_timeout(),
      playback: m.codex_voice_playback(), failed: m.codex_voice_failed() }[code];
  }

  function start() {
    if (!server) { error = m.codex_voice_unavailable(); return; }
    onPrepare();
    error = ''; detail = ''; muted = false;
    call ??= new CodexVoiceCall(audio, (next, code, message) => {
      callState = next;
      if (next === 'connected') open = false;
      if (code) { error = failure(code); open = true; }
      if (message) detail = message;
      onBusyChange(next === 'connecting' || next === 'connected');
    }, values => { voices = values; }, levels => {
      inputLevel = levels.input; outputLevel = levels.output; meterAvailable = levels.available;
    }, text => { pendingDraft = text; });
    call.setMuted(false);
    void call.start(server, sessionName, voice);
  }

  function stop() { call?.stop(); open = false; }
  function saveVoice(event: Event) {
    const value = (event.currentTarget as HTMLSelectElement).value;
    try { localStorage.setItem('cp_codex_voice', value); }
    catch { error = m.codex_voice_save_failed(); }
  }
  function toggleMute() { muted = !muted; call?.setMuted(muted); }
  $effect(() => { sessionName; return () => call?.stop(); });
  $effect(() => {
    if (!open || !server) return;
    let cancelled = false;
    getCodexVoicesForServer(server, sessionName).then(values => {
      if (!cancelled) voices = values;
    }).catch(() => { if (!cancelled) error = m.codex_voice_failed(); });
    return () => { cancelled = true; };
  });
  onDestroy(() => call?.stop());
</script>

<svelte:window onpagehide={stop} />
<button class="voice-pill" class:active={busy} disabled={disabled && !busy}
  onclick={() => { open = true; }} aria-haspopup="dialog" aria-expanded={open}
  aria-label={busy ? muted ? m.codex_voice_muted() : m.codex_voice_active() : m.codex_voice_title()}
  title={busy ? muted ? m.codex_voice_muted() : m.codex_voice_listening() : m.codex_voice_title()}>
  {#if busy}
    <span class="voice-levels" title={meterAvailable ? m.codex_voice_meter_hint() : m.codex_voice_meter_unavailable()} aria-hidden="true">
      <span class="level input" style:transform={'scaleY(' + (0.12 + (muted ? 0 : inputLevel) * 0.88) + ')'}></span>
      <span class="voice-mark" style:transform={'scale(' + (1 + Math.max(muted ? 0 : inputLevel, outputLevel) * 0.25) + ')'}><HangarMark size={16} arcs={2} /></span>
      <span class="level output" style:transform={'scaleY(' + (0.12 + outputLevel * 0.88) + ')'}></span>
    </span>
  {:else}<IconMic size={16} />{/if}
  <span>{callState === 'connecting' ? m.codex_voice_connecting() : busy ? m.codex_voice_active() : m.codex_voice_label()}</span>
  <span class="beta-badge">{m.comum_beta()}</span>
</button>

<BottomSheet {open} onClose={() => { open = false; }} ariaLabel={m.codex_voice_title()} centered={desktop.atual}>
  <h2>{m.codex_voice_title()}</h2>
  <div class="voice-controls">
    <label for="codex-call-voice"><IconMic size={16} />{m.codex_voice_label()}</label>
    <select id="codex-call-voice" bind:value={voice} onchange={saveVoice} disabled={busy || disabled}>
      <option value="">{m.codex_voice_default()}</option>
      {#each voices as item (item)}<option value={item}>{item}</option>{/each}
    </select>
      {#if busy}
        {#if callState === 'connected'}
          <button onclick={toggleMute} aria-pressed={muted}
            aria-label={muted ? m.codex_voice_unmute() : m.codex_voice_mute()}
            title={muted ? m.codex_voice_unmute() : m.codex_voice_mute()}>{muted ? m.codex_voice_unmute_short() : m.codex_voice_mute_short()}</button>
        {/if}
        <button onclick={stop} aria-label={m.codex_voice_stop()} title={m.codex_voice_stop()}>{m.codex_voice_stop_short()}</button>
      {:else}
        <button class="primary" onclick={start} {disabled} aria-label={m.codex_voice_connect()} title={m.codex_voice_connect()}>{m.codex_voice_connect_short()}</button>
      {/if}
  </div>
  {#if busy}
    <p class="status" role="status">{callState === 'connecting' ? m.codex_voice_connecting()
      : muted ? m.codex_voice_muted() : m.codex_voice_listening()}</p>
  {/if}
  {#if pendingDraft}
    <p class="draft-label">{m.codex_voice_draft_waiting()}</p>
    <p>{pendingDraft}</p>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if detail}<p class="error detail">{detail}</p>{/if}
</BottomSheet>
<audio bind:this={audio} autoplay></audio>

<style>
  .voice-pill { display: inline-flex; align-items: center; gap: 5px; padding: 4px 8px; border: 0; background: transparent; white-space: nowrap; color: var(--text-secondary); }
  .voice-pill.active { color: var(--accent); background: var(--surface-raised); }
  .beta-badge { padding: 1px 4px; border-radius: var(--radius-full); background: var(--accent-dim);
    color: var(--accent); font-size: 8px; font-weight: 700; line-height: 1.3; text-transform: uppercase; }
  .voice-levels { display: inline-flex; align-items: center; gap: 3px; height: 20px; }
  .voice-mark { display: inline-flex; transition: transform 80ms linear; }
  .level { width: 3px; height: 18px; border-radius: 2px; background: var(--accent); transition: transform 80ms linear; }
  .level.output { background: var(--success); }
  .draft-label { color: var(--accent); font-weight: 600; }
  @media (prefers-reduced-motion: reduce) { .voice-mark { transform: none !important; } .voice-mark, .level { transition: none; } }
  h2 { margin-bottom: var(--space-4); font-size: var(--text-lg); color: var(--text-primary); }
  .voice-controls { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  button:disabled { opacity: .45; cursor: default; }
  p { margin: 12px 0 0; color: var(--text-secondary); font-size: var(--text-sm); line-height: 1.5; }
  label { display: flex; align-items: center; gap: 5px; color: var(--text-secondary); font-size: var(--text-xs); }
  select, button { min-height: 44px; padding: 6px 10px; border: 1px solid var(--border-default); border-radius: var(--radius-sm); color: var(--text-primary); font: inherit; font-size: var(--text-xs); }
  select { flex: 1; min-width: 80px; max-width: 180px; background: var(--surface-inset); }
  button { cursor: pointer; background: var(--surface-raised); }
  .primary { margin-left: auto; }
  .error { color: var(--error); }
  .detail { overflow-wrap: anywhere; }
  button:focus-visible, select:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
