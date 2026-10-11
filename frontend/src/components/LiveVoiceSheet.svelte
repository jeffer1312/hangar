<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { liveVoiceStore, type LiveVoiceSettings } from '../lib/liveVoiceStore.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import { listarCredenciais, modelOptionsForServer, type Credencial, type ModelOption } from '@hangar/core';
  import * as m from '../paraglide/messages';
  import { thoughtTail, type VoiceStatus } from '../lib/liveVoiceStatus';

  type Mode = 'direct' | 'plan';
  const MODES: Mode[] = ['direct', 'plan'];

  const ERRORS: Record<string, () => string> = {
    disabled: m.live_voice_error_disabled,
    codex_missing: m.live_voice_error_codex_missing,
    account_missing: m.live_voice_error_account_missing,
    unknown_account: m.live_voice_error_account_missing,
    connection_lost: m.live_voice_error_connection_lost,
    microphone: m.live_voice_error_microphone,
    playback: m.live_voice_error_playback,
    timeout: m.live_voice_error_timeout,
    taken: m.live_voice_taken,
  };
  const errorText = (code: string) => (ERRORS[code] ?? m.live_voice_error_failed)();

  const phase = $derived(liveVoiceStore.phase);
  const inCall = $derived(phase === 'connecting' || phase === 'live');
  const callError = $derived(liveVoiceStore.error);
  // Durante a chamada (e no erro dela) o painel é do servidor da voz; parado, do servidor ativo.
  const target = $derived(inCall || callError ? liveVoiceStore.server ?? liveVoiceStore.activeServer : liveVoiceStore.activeServer);
  const entry = $derived(liveVoiceStore.settingsOf(target));
  const reply = $derived(entry?.reply ?? null);
  const vs = $derived(liveVoiceStore.state);
  const STATUS: Record<VoiceStatus, () => string> = {
    connecting: m.codex_voice_connecting, voice: m.live_voice_status_voice, muted: m.codex_voice_muted,
    you: m.live_voice_status_you, thinking: m.live_voice_status_thinking, searching: m.live_voice_status_searching,
    working: m.live_voice_status_working, listening: m.codex_voice_listening,
  };
  const status = $derived(liveVoiceStore.status);
  // Servidor mais antigo pode não mandar `activity`/`thought`: ausente vale ocioso e vazio.
  const busy = $derived(!!vs?.activity && vs.activity !== 'idle');
  const thought = $derived(busy && vs ? thoughtTail(vs.thought ?? '') : []);

  $effect(() => { if (liveVoiceStore.open && target) void liveVoiceStore.loadSettings(target, true); });

  // Cronômetro da fase ao vivo; zera ao sair dela.
  let liveSince = $state<number | null>(null);
  let now = $state(Date.now());
  $effect(() => {
    if (phase !== 'live') { liveSince = null; return; }
    if (liveSince === null) liveSince = Date.now();
    if (!liveVoiceStore.open) return;
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });
  const elapsed = $derived.by(() => {
    const s = liveSince === null ? 0 : Math.max(0, Math.floor((now - liveSince) / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  });

  // Contas e modelos só para os ajustes, relidos quando servidor ou conta mudam.
  let accounts = $state<Credencial[]>([]);
  let accountsFor = '';
  $effect(() => {
    if (!liveVoiceStore.open || !target || !reply) return;
    const key = target.id;
    if (accountsFor === key) return;
    accountsFor = key;
    listarCredenciais(target).then(list => { if (accountsFor === key) accounts = list.filter(c => c.tipo === 'codex'); },
      e => console.warn('live voice: codex accounts unreadable', e));
  });

  let models = $state<ModelOption[]>([]);
  let modelsError = $state(false);
  let modelsFor = '';
  $effect(() => {
    if (!liveVoiceStore.open || !target || !reply) return;
    const key = `${target.id}::${reply.settings.codex_account}`;
    if (modelsFor === key) return;
    modelsFor = key;
    modelsError = false;
    modelOptionsForServer(target, 'codex', null, null, reply.settings.codex_account)
      .then(r => { if (modelsFor === key) models = r.models; },
        e => { if (modelsFor === key) { models = []; modelsError = true; } console.warn('live voice: models unreadable', e); });
  });

  const accountLabel = (c: Credencial) => c.apelido || c.nome_natural || c.nome;
  // Sem modelo escolhido vale o da conta, que a tela não conhece: só os níveis que todos aceitam.
  function efforts(model: string | null, current: string): string[] {
    const chosen = models.find(x => x.id === model);
    const list = chosen ? chosen.efforts ?? []
      : models.reduce<string[] | null>((acc, x) => acc === null ? [...x.efforts ?? []] : acc.filter(e => x.efforts?.includes(e)), null) ?? [];
    return list.includes(current) ? list : [current, ...list];
  }

  let saving = $state(false);
  let saveError = $state<string | null>(null);
  // Gravação recusada: remonta os campos para voltarem ao que o servidor guardou.
  let formKey = $state(0);
  async function save(patch: (s: LiveVoiceSettings) => LiveVoiceSettings) {
    if (!target || !reply) return;
    saving = true;
    saveError = null;
    try {
      await liveVoiceStore.saveSettings(target, patch($state.snapshot(reply.settings)));
    } catch (e) {
      const code = (e as { code?: string }).code ?? '';
      saveError = code === 'unknown_account' ? errorText(code) : m.comum_falha_aplicar();
      formKey++;
    } finally {
      saving = false;
    }
  }
  const pick = (e: Event) => (e.currentTarget as HTMLSelectElement).value;
  const orNull = (v: string) => (v === '' ? null : v);
  function setPair(mode: Mode, field: 'model' | 'effort' | 'tier', value: string) {
    void save(s => ({ ...s, organizer: { ...s.organizer, [mode]: { ...s.organizer[mode], [field]: field === 'effort' ? value : orNull(value) } } }));
  }

  function close() {
    liveVoiceStore.open = false;
    if (!inCall) liveVoiceStore.dismissError();
  }

  const pct = (w:{ used_percent: number } | null) => (w ? `${Math.round(w.used_percent)}%` : '—');
</script>

<BottomSheet open={liveVoiceStore.open} onClose={close} ariaLabel={m.live_voice_open()}>
  <div class="lv">
    <header class="lv-head">
      <h2 class="lv-title">{m.live_voice_open()}</h2>
      {#if inCall}
        <p class="lv-status lv-st-{status}" role="status">
          <span class="lv-dot" aria-hidden="true"></span>
          {phase === 'live' ? `${STATUS[status]()} · ${elapsed}` : m.live_voice_connecting()}
        </p>
      {/if}
      {#if target}
        <p class="lv-server">{m.live_voice_server()}: {vs?.server || target.label}</p>
      {/if}
    </header>

    {#if callError}
      <div class="lv-block" role="alert">
        <p class="lv-error">{errorText(callError.code)}</p>
        {#if callError.detail}<p class="lv-detail">{callError.detail}</p>{/if}
      </div>
    {/if}

    {#if inCall}
      <div class="lv-controls">
        <button type="button" class="lv-btn" class:on={liveVoiceStore.muted} onclick={() => liveVoiceStore.toggleMute()}>
          {liveVoiceStore.muted ? m.live_voice_unmute() : m.live_voice_mute()}
        </button>
        <button type="button" class="lv-btn danger" onclick={() => liveVoiceStore.stop()}>{m.live_voice_stop()}</button>
      </div>

      {#if vs}
        <div class="lv-seg" role="group">
          <button type="button" class:on={vs.mode === 'direct'} aria-pressed={vs.mode === 'direct'}
            onclick={() => liveVoiceStore.setMode('direto')}>{m.live_voice_mode_direct()}</button>
          <button type="button" class:on={vs.mode === 'plan'} aria-pressed={vs.mode === 'plan'}
            onclick={() => liveVoiceStore.setMode('planejar')}>{m.live_voice_mode_plan()}</button>
        </div>

        {#if vs.error}<p class="lv-detail">{vs.error.text}</p>{/if}

        {#if busy && (vs.action || thought.length)}
          <section class="lv-block lv-now" aria-live="polite">
            {#if vs.action}<p class="lv-action">{vs.action.text}</p>{/if}
            {#each thought as line, i (i)}<p class="lv-thought">{line}</p>{/each}
          </section>
        {/if}

        {#if vs.draft}
          <section class="lv-block">
            <h3 class="lv-label">{m.live_voice_draft()}</h3>
            <p class="lv-text">{vs.draft}</p>
          </section>
        {/if}

        {#if vs.backstage.length}
          <section class="lv-block">
            <h3 class="lv-label">{m.live_voice_backstage()}</h3>
            <ul class="lv-lines">
              {#each vs.backstage.slice(-6) as line, i (i)}
                <li class="lv-line lv-{line.kind}">{line.text}</li>
              {/each}
            </ul>
          </section>
        {/if}

        {#if vs.plan}
          <section class="lv-block">
            <h3 class="lv-label">{m.live_voice_plan()}</h3>
            <div class="lv-plan">{@html renderMarkdown(vs.plan.markdown)}</div>
          </section>
        {/if}

        {#if vs.followed.length}
          <section class="lv-block">
            <h3 class="lv-label">{m.live_voice_followed()}</h3>
            <p class="lv-chips">
              {#each vs.followed as f (f.server + '::' + f.name)}
                <span class="lv-chip">{f.server ? `${f.server}::${f.name}` : f.name}</span>
              {/each}
            </p>
          </section>
        {/if}

        <section class="lv-block lv-limits">
          <span>{m.ctx_limite_5h()} <strong>{pct(vs.limits.five_hour)}</strong></span>
          <span>{m.ctx_limite_7d()} <strong>{pct(vs.limits.seven_day)}</strong></span>
        </section>
      {/if}
    {:else if entry?.loading && !reply}
      <p class="lv-muted" role="status">{m.comum_carregando()}</p>
    {:else if !reply}
      <div class="lv-block" role="alert">
        <p class="lv-error">{m.live_voice_error_failed()}</p>
        {#if entry?.error}<p class="lv-detail">{entry.error}</p>{/if}
        {#if target}
          <button type="button" class="lv-btn" onclick={() => target && liveVoiceStore.loadSettings(target, true)}>{m.sync_retry()}</button>
        {/if}
      </div>
    {:else if !reply.enabled}
      <p class="lv-error">{m.live_voice_error_disabled()}</p>
    {:else if !reply.codex}
      <p class="lv-error">{m.live_voice_error_codex_missing()}</p>
    {:else if target}
      <button type="button" class="lv-primary" onclick={() => target && liveVoiceStore.start(target)}>{m.live_voice_start()}</button>
    {/if}

    {#if reply}
      <details class="lv-settings">
        <summary class="lv-label">{m.live_voice_settings()}</summary>
        {#key formKey}
        <label class="lv-field">
          <span>{m.live_voice_account()}</span>
          <select class="field-input" disabled={saving} value={reply.settings.codex_account}
            onchange={e => { const v = pick(e); void save(s => ({ ...s, codex_account: v })); }}>
            {#if !accounts.some(c => (c.codex_account ?? c.id) === reply.settings.codex_account)}
              <option value={reply.settings.codex_account}>{reply.settings.codex_account}</option>
            {/if}
            {#each accounts as c (c.id)}
              <option value={c.codex_account ?? c.id}>{accountLabel(c)}</option>
            {/each}
          </select>
        </label>
        <label class="lv-field">
          <span>{m.live_voice_voice()}</span>
          <select class="field-input" disabled={saving} value={reply.settings.voice ?? ''}
            onchange={e => { const v = orNull(pick(e)); void save(s => ({ ...s, voice: v })); }}>
            <option value="">{m.live_voice_default()}</option>
            {#each reply.voices as v (v)}<option value={v}>{v}</option>{/each}
          </select>
        </label>
        {#if modelsError}<p class="lv-detail">{m.comum_falha_carregar_modelos()}</p>{/if}
        {#each MODES as mode (mode)}
          {@const pair = reply.settings.organizer[mode]}
          <fieldset class="lv-mode">
            <legend class="lv-label">{mode === 'direct' ? m.live_voice_mode_direct() : m.live_voice_mode_plan()}</legend>
            <label class="lv-field">
              <span>{m.live_voice_model()}</span>
              <select class="field-input" disabled={saving} value={pair.model ?? ''} onchange={e => setPair(mode, 'model', pick(e))}>
                <option value="">{m.live_voice_default()}</option>
                {#if pair.model && !models.some(x => x.id === pair.model)}<option value={pair.model}>{pair.model}</option>{/if}
                {#each models as x (x.id)}<option value={x.id}>{x.name ?? x.id}</option>{/each}
              </select>
            </label>
            <label class="lv-field">
              <span>{m.live_voice_effort()}</span>
              <select class="field-input" disabled={saving} value={pair.effort} onchange={e => setPair(mode, 'effort', pick(e))}>
                {#each efforts(pair.model, pair.effort) as level (level)}<option value={level}>{level}</option>{/each}
              </select>
            </label>
            <label class="lv-field">
              <span>{m.live_voice_tier()}</span>
              <select class="field-input" disabled={saving} value={pair.tier ?? ''} onchange={e => setPair(mode, 'tier', pick(e))}>
                <option value="">{m.live_voice_default()}</option>
                <option value="default">{m.live_voice_tier_standard()}</option>
                <option value="priority">{m.live_voice_tier_fast()}</option>
              </select>
            </label>
          </fieldset>
        {/each}
        {/key}
        {#if saveError}<p class="lv-error" role="alert">{saveError}</p>{/if}
      </details>
    {/if}
  </div>
</BottomSheet>

<style>
  .lv { padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3); background: transparent; }
  .lv-head { display: flex; flex-direction: column; gap: 2px; }
  .lv-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .lv-status { display: flex; align-items: center; gap: 6px; font-size: var(--text-sm); color: var(--st, var(--text-muted)); font-variant-numeric: tabular-nums; }
  .lv-dot { width: 8px; height: 8px; border-radius: 50%; background: currentColor; flex-shrink: 0; }
  .lv-st-voice { --st: var(--success); }
  .lv-st-you { --st: var(--accent); }
  .lv-st-thinking { --st: var(--text-primary); }
  .lv-st-searching, .lv-st-working { --st: var(--warning); }
  .lv-st-thinking .lv-dot, .lv-st-searching .lv-dot, .lv-st-working .lv-dot { animation: lv-pulse 1s ease-in-out infinite; }
  @keyframes lv-pulse { 50% { opacity: 0.3; } }
  .lv-now { gap: 4px; }
  .lv-action { font-size: var(--text-sm); color: var(--warning); }
  .lv-thought { font-size: var(--text-xs); color: var(--text-muted); overflow-wrap: anywhere; }
  @media (prefers-reduced-motion: reduce) { .lv-dot { animation: none !important; } }
  .lv-server, .lv-muted { font-size: var(--text-sm); color: var(--text-muted); }
  .lv-error { font-size: var(--text-sm); color: var(--error); }
  .lv-detail { font-size: var(--text-xs); color: var(--text-muted); word-break: break-word; }
  .lv-block { display: flex; flex-direction: column; gap: var(--space-2); }
  .lv-label { font-size: var(--text-xs); font-weight: 600; color: var(--text-secondary); text-transform: uppercase; letter-spacing: 0.04em; }
  .lv-text { font-size: var(--text-sm); color: var(--text-primary); line-height: 1.5; }

  .lv-controls { display: flex; gap: var(--space-2); }
  .lv-btn {
    flex: 1; min-height: 44px; padding: 0 var(--space-3);
    border: 1px solid var(--border-default); border-radius: var(--radius-md);
    background: transparent; color: var(--text-primary); font-size: var(--text-sm);
  }
  .lv-btn.on { border-color: var(--accent); color: var(--accent); background: var(--accent-dim); }
  .lv-btn.danger { color: var(--error); }
  .lv-primary {
    width: 100%; height: 50px; border-radius: var(--radius-md);
    background: var(--accent); color: #fff; font-size: var(--text-base); font-weight: 600;
  }

  .lv-seg { display: flex; border: 1px solid var(--border-default); border-radius: var(--radius-md); overflow: hidden; }
  .lv-seg button { flex: 1; min-height: 40px; background: transparent; color: var(--text-secondary); font-size: var(--text-sm); }
  .lv-seg button.on { background: var(--accent-dim); color: var(--accent); font-weight: 600; }

  .lv-lines { display: flex; flex-direction: column; gap: 4px; list-style: none; margin: 0; padding: 0; }
  .lv-line { font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.45; }
  .lv-heard { color: var(--text-primary); }
  .lv-plan {
    font-size: var(--text-sm); line-height: 1.55; color: var(--text-secondary);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md); padding: var(--space-3);
    background: transparent; word-break: break-word;
  }
  .lv-plan :global(:is(h1, h2, h3)) { margin: 0 0 var(--space-2); font-size: var(--text-sm); color: var(--text-primary); }
  .lv-plan :global(:is(p, ul, ol)) { margin: 0 0 var(--space-2); }
  .lv-plan :global(:is(ul, ol)) { padding-left: 1.2em; }
  .lv-chips { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .lv-chip {
    font-size: var(--text-xs); color: var(--text-secondary);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-full); padding: 2px 8px;
  }
  .lv-limits { flex-direction: row; gap: var(--space-4); font-size: var(--text-sm); color: var(--text-secondary); }
  .lv-limits strong { color: var(--text-primary); font-variant-numeric: tabular-nums; }

  .lv-settings { border-top: 1px solid var(--border-subtle); padding-top: var(--space-3); display: flex; flex-direction: column; gap: var(--space-3); }
  .lv-settings summary { cursor: pointer; padding: var(--space-1) 0; }
  .lv-mode { border: none; margin: var(--space-2) 0 0; padding: 0; display: flex; flex-direction: column; gap: var(--space-2); }
  .lv-field { display: flex; flex-direction: column; gap: 4px; margin-top: var(--space-2); font-size: var(--text-sm); color: var(--text-secondary); }
  .lv-field select { width: 100%; }
</style>
