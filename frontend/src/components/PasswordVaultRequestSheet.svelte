<script lang="ts" module>
  // Push do pedido abre `/?server=<id>&vault_request=<id>`: a folha abre uma vez por carga da página.
  const deepLink = (() => {
    if (typeof window === 'undefined') return null;
    const p = new URLSearchParams(window.location.search);
    const server = p.get('server');
    const request = p.get('vault_request');
    return server && request ? { server, request } : null;
  })();
  let deepLinkUsed = false;
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import BottomSheet from './BottomSheet.svelte';
  import * as m from '../paraglide/messages';
  import { answerVaultRequest, vaultErrorText, type Server, type VaultAnswer, type VaultEvent, type VaultPolicy, type VaultRequest } from '@hangar/core';

  let { server, event }: { server: Server; event: VaultEvent } = $props();

  // O cartão que a pessoa viu: a resposta leva o `interaction_id` dele, nunca o do pedido atual.
  let shown = $state<VaultRequest | null>(null);
  let password = $state('');
  let name = $state('');
  let policy = $state<VaultPolicy>('ask');
  let visible = $state(false);
  let sending = $state(false);
  let error = $state<string | null>(null);
  let field = $state<HTMLInputElement>();

  const live = $derived(shown ? event.requests.find((r) => r.id === shown!.id) ?? null : null);

  // Respondido em outro aparelho, expirado ou cancelado: o pedido some do evento e a folha fecha.
  $effect(() => { if (shown && !live) close(); });

  $effect(() => {
    if (deepLinkUsed || !deepLink || deepLink.server !== server.id) return;
    const r = event.requests.find((x) => x.id === deepLink.request);
    if (r) { deepLinkUsed = true; open(r); }
  });

  function open(r: VaultRequest) {
    shown = r;
    password = '';
    name = r.label ?? '';
    policy = 'ask';
    visible = false;
    error = null;
  }

  function close() {
    shown = null;
    password = '';
    visible = false;
    error = null;
  }

  const useLabel = (r: VaultRequest) =>
    r.use === 'fill' ? m.vault_web_use_fill() : r.use === 'exec' ? m.vault_web_use_exec() : r.use === 'file' ? m.vault_web_use_file() : '';

  const expires = (r: VaultRequest) =>
    new Date(r.expires_at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

  function failure(err: unknown): string {
    const code = (err as { code?: string })?.code;
    const known = code ? vaultErrorText(code) : undefined;
    if (known) return known;
    return m.vault_web_request_failed({ error: err instanceof Error ? err.message : String(err) });
  }

  async function send(answer: VaultAnswer) {
    if (!shown || sending) return;
    const card = shown;
    sending = true;
    error = null;
    try {
      await answerVaultRequest(server, card.id, answer);
      close();
    } catch (err) {
      error = failure(err);
      // Pedido trocado: mostra o atual para a pessoa decidir de novo sobre o que vê.
      if (live && live.interaction_id !== card.interaction_id) shown = live;
    } finally {
      sending = false;
    }
    if (error && field) { await tick(); field.focus(); }
  }

  function decide(decision: 'allow' | 'deny') {
    if (shown) void send({ interaction_id: shown.interaction_id, decision });
  }

  function save(keep: boolean) {
    if (!shown || !password) return;
    // Recusa com o pedido aberto (chaveiro trancado, nome repetido) mantém o texto; o sucesso fecha e limpa.
    const n = name.trim();
    void send(keep
      ? { interaction_id: shown.interaction_id, decision: 'save', password, keep, policy, ...(n ? { name: n } : {}) }
      : { interaction_id: shown.interaction_id, decision: 'save', password, keep });
  }

  function decline() {
    if (shown) void send({ interaction_id: shown.interaction_id, decision: 'decline' });
  }
</script>

<section class="vault-notice" aria-label={m.vault_web_notice_title({ label: server.label })}>
  <div class="vault-head">
    <svg class="lock" viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
      <rect x="4" y="11" width="16" height="10" rx="2" /><path d="M8 11V7a4 4 0 0 1 8 0v4" />
    </svg>
    <p class="vault-title">{m.vault_web_notice_title({ label: server.label })}</p>
  </div>
  {#each event.requests as r (r.id)}
    <button type="button" class="vault-row" onclick={() => open(r)}>
      <span class="vault-row-txt">
        <span class="vault-row-main">
          {r.kind === 'approve'
            ? m.vault_web_row_approve({ entry: r.entry_name ?? '' })
            : m.vault_web_row_entry({ label: r.label ?? '' })}
        </span>
        <span class="vault-row-sub">{r.origin}</span>
      </span>
      <span class="vault-row-go" aria-hidden="true">›</span>
    </button>
  {/each}
</section>

<BottomSheet open={!!shown} onClose={close}
  ariaLabel={shown?.kind === 'entry' ? m.vault_web_entry_title() : m.vault_web_approve_title()}>
  {#if shown}
    <div class="vr">
      <h2 class="vr-title">{shown.kind === 'entry' ? m.vault_web_entry_title() : m.vault_web_approve_title()}</h2>
      <dl class="vr-facts">
        <dt>{m.vault_web_field_origin()}</dt><dd>{shown.origin}</dd>
        {#if shown.kind === 'approve'}
          <dt>{m.vault_web_field_entry()}</dt><dd>{shown.entry_name ?? ''}</dd>
          {#if shown.use}<dt>{m.vault_web_field_use()}</dt><dd>{useLabel(shown)}</dd>{/if}
          {#if shown.target}<dt>{m.vault_web_field_target()}</dt><dd class="mono">{shown.target}</dd>{/if}
        {:else}
          <dt>{m.vault_web_field_label()}</dt><dd>{shown.label ?? ''}</dd>
        {/if}
        <dt>{m.vault_web_field_reason()}</dt><dd>{shown.reason}</dd>
        <dt>{m.vault_web_field_expires()}</dt><dd>{expires(shown)}</dd>
      </dl>

      {#if shown.kind === 'entry'}
        <label class="vr-field">
          <span>{m.vault_web_password()}</span>
          <span class="pw">
            <input bind:this={field} type={visible ? 'text' : 'password'} autocomplete="off" autocapitalize="off" spellcheck="false"
              bind:value={password} oninput={() => (error = null)} disabled={sending}
              placeholder={shown.placeholder ?? m.vault_web_password()} />
            <button type="button" class="reveal" onclick={() => (visible = !visible)} disabled={sending}
              aria-label={visible ? m.vault_web_hide_password() : m.vault_web_show_password()} aria-pressed={visible}>
              <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
                <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z" /><circle cx="12" cy="12" r="3" />
                {#if visible}<path d="M3 3l18 18" />{/if}
              </svg>
            </button>
          </span>
        </label>
        <label class="vr-field">
          <span>{m.vault_web_name()}</span>
          <input class="vr-input" type="text" autocomplete="off" bind:value={name} disabled={sending} />
        </label>
        <fieldset class="vr-policy" disabled={sending}>
          <legend>{m.vault_web_policy()}</legend>
          <label><input type="radio" bind:group={policy} value="ask" />{m.vault_web_policy_ask()}</label>
          <label><input type="radio" bind:group={policy} value="free" />{m.vault_web_policy_free()}</label>
        </fieldset>
      {/if}

      {#if error}<p class="vr-error" role="alert">{error}</p>{/if}

      {#if shown.kind === 'approve'}
        <div class="vr-actions">
          <button type="button" class="btn btn-quiet" onclick={() => decide('deny')} disabled={sending}>{m.vault_web_deny()}</button>
          <button type="button" class="btn btn-main" onclick={() => decide('allow')} disabled={sending}>{m.vault_web_allow()}</button>
        </div>
      {:else}
        <div class="vr-actions vr-actions-col">
          <button type="button" class="btn btn-main" onclick={() => save(true)} disabled={sending || !password}>{m.vault_web_save_keep()}</button>
          <button type="button" class="btn btn-quiet" onclick={() => save(false)} disabled={sending || !password}>{m.vault_web_save_once()}</button>
          <button type="button" class="btn btn-quiet" onclick={decline} disabled={sending}>{m.vault_web_decline()}</button>
        </div>
      {/if}
    </div>
  {/if}
</BottomSheet>

<style>
  .vault-notice {
    margin: 0 var(--space-4) var(--space-3);
    padding: var(--space-3);
    background: var(--surface-raised);
    border: 1px solid var(--border-default);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius-md);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .vault-head { display: flex; gap: var(--space-2); align-items: center; }
  .lock { flex-shrink: 0; fill: none; stroke: var(--accent); stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  .vault-title { margin: 0; font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .vault-row {
    display: flex; align-items: center; justify-content: flex-start; gap: var(--space-2);
    width: 100%; min-height: 44px; padding: var(--space-2);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: var(--surface-inset); color: var(--text-primary);
    text-align: left; cursor: pointer;
  }
  .vault-row-txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
  .vault-row-main { font-size: var(--text-sm); overflow-wrap: anywhere; }
  .vault-row-sub { font-size: var(--text-xs); color: var(--text-muted); overflow-wrap: anywhere; }
  .vault-row-go { color: var(--text-muted); font-size: var(--text-base); }

  .vr { container-type: inline-size; display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; }
  .vr-title { margin: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .vr-facts { margin: 0; display: grid; grid-template-columns: max-content 1fr; gap: var(--space-1) var(--space-3); font-size: var(--text-sm); }
  .vr-facts dt { color: var(--text-muted); }
  .vr-facts dd { margin: 0; color: var(--text-primary); overflow-wrap: anywhere; white-space: pre-wrap; }
  .mono { font-family: var(--font-mono); font-size: var(--text-xs); }
  @container (max-width: 360px) {
    .vr-facts { grid-template-columns: 1fr; }
    .vr-facts dd { margin-bottom: var(--space-1); }
  }
  .vr-field { display: flex; flex-direction: column; gap: 4px; }
  .vr-field > span:first-child { font-size: var(--text-xs); color: var(--text-secondary); }
  .vr-input {
    height: 44px; background: var(--surface-inset); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); color: var(--text-primary); padding: 0 var(--space-3);
    font: inherit; font-size: 16px;
  }
  .pw {
    display: flex; align-items: center;
    background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-md);
  }
  .pw:focus-within { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-dim); }
  .pw input {
    flex: 1; min-width: 0; height: 44px; border: 0; background: transparent;
    color: var(--text-primary); font-family: var(--font-ui); font-size: 16px;
    padding: 0 var(--space-3); outline: none;
  }
  .reveal {
    width: 44px; height: 44px; flex-shrink: 0; display: grid; place-items: center;
    border: 0; background: transparent; color: var(--text-muted); cursor: pointer;
  }
  .reveal svg { fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  .vr-policy { margin: 0; padding: 0; border: 0; display: flex; flex-wrap: wrap; gap: var(--space-3); font-size: var(--text-sm); color: var(--text-primary); }
  .vr-policy legend { padding: 0; margin-bottom: 4px; font-size: var(--text-xs); color: var(--text-secondary); }
  .vr-policy label { display: flex; align-items: center; gap: var(--space-1); min-height: 44px; }
  .vr-policy input { width: 20px; height: 20px; accent-color: var(--accent); }
  .vr-error { margin: 0; color: var(--error); font-size: var(--text-sm); }
  .vr-actions { display: flex; gap: var(--space-3); }
  .vr-actions-col { flex-direction: column; gap: var(--space-2); }
  .btn { flex: 1; min-height: 48px; border-radius: var(--radius-md); font-size: var(--text-sm); font-weight: 500; }
  .btn-quiet { background: var(--bg-hover); color: var(--text-secondary); }
  .btn-main { background: var(--accent); color: #fff; font-weight: 600; }
  .btn:disabled { opacity: 0.5; cursor: default; }
</style>
