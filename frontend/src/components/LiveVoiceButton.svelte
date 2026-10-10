<script lang="ts">
  import HangarWorking from './icons/HangarWorking.svelte';
  import { liveVoiceStore } from '../lib/liveVoiceStore.svelte';
  import * as m from '../paraglide/messages';

  const server = $derived(liveVoiceStore.activeServer);
  const reply = $derived(liveVoiceStore.settingsOf(server)?.reply ?? null);
  const phase = $derived(liveVoiceStore.phase);
  const inCall = $derived(phase === 'connecting' || phase === 'live');
  // Chamada aberta noutra máquina continua alcançável daqui, mesmo onde a voz está desligada.
  const visible = $derived(inCall || !!liveVoiceStore.error || !!(reply?.enabled && reply.codex));
  const level = (v: number) => `scaleY(${0.15 + v * 0.85})`;

  $effect(() => { if (server && !server.invite) void liveVoiceStore.loadSettings(server); });

  function tap() {
    if (!inCall && server) void liveVoiceStore.start(server);
    else liveVoiceStore.open = true;
  }
</script>

{#if visible}
  <button type="button" class="live-voice-btn" class:live={phase === 'live'} onclick={tap}
    aria-label={m.live_voice_open()} title={m.live_voice_open()}>
    {#if phase === 'connecting'}
      <HangarWorking size={18} />
    {:else if phase === 'live'}
      <span class="bars" aria-hidden="true">
        <span class="bar" style:transform={level(liveVoiceStore.muted ? 0 : liveVoiceStore.levels.input)}></span>
        <span class="bar out" style:transform={level(liveVoiceStore.levels.output)}></span>
        <span class="bar" style:transform={level(liveVoiceStore.muted ? 0 : liveVoiceStore.levels.input)}></span>
      </span>
    {:else}
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect x="9" y="3" width="6" height="11" rx="3"/>
        <path d="M5 11a7 7 0 0 0 14 0M12 18v3"/>
      </svg>
      {#if liveVoiceStore.error}<span class="alert-dot" aria-hidden="true"></span>{/if}
    {/if}
  </button>
{/if}

<style>
  .live-voice-btn {
    position: relative;
    width: 40px;
    height: 40px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    color: var(--text-secondary);
    border-radius: var(--radius-full);
    transition: background 150ms var(--ease-out), color 150ms var(--ease-out);
  }
  .live-voice-btn:active { background: var(--bg-hover); color: var(--text-primary); }
  .live-voice-btn.live { color: var(--accent); background: var(--accent-dim); }
  .bars { display: inline-flex; align-items: center; gap: 3px; height: 18px; }
  .bar { width: 3px; height: 18px; border-radius: 2px; background: var(--accent); transition: transform 80ms linear; }
  .bar.out { background: var(--success); }
  .alert-dot {
    position: absolute; top: 7px; right: 7px;
    width: 8px; height: 8px; border-radius: 50%;
    background: var(--error);
  }
  @media (prefers-reduced-motion: reduce) { .bar { transition: none; } }
</style>
