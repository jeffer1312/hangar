<script lang="ts">
  import { tick } from 'svelte';
  import * as m from '../paraglide/messages';
  import { unlockKeyringForServer, type Server } from '@hangar/core';

  let { server }: { server: Server } = $props();

  let password = $state('');
  let sending = $state(false);
  let error = $state<string | null>(null);
  let visible = $state(false);
  let opened = $state(false);
  const busy = $derived(sending || opened);
  let field = $state<HTMLInputElement>();

  async function unlock(e: SubmitEvent) {
    e.preventDefault();
    if (sending || opened || !password) return;
    const typed = password;
    password = '';   // a senha não fica no estado depois da tentativa
    sending = true;
    error = null;
    try {
      // Aberto: quem esconde o aviso é o evento `keyring` da lista. Até ele chegar o botão segue
      // ocupado, e não parece que a senha certa foi ignorada.
      await unlockKeyringForServer(server, typed);
      opened = true;
      setTimeout(() => (opened = false), 15_000);
    } catch (err) {
      error = (err as { code?: string })?.code === 'keyring_wrong_password'
        ? m.keyring_wrong_password()
        : m.keyring_unlock_failed({ error: err instanceof Error ? err.message : String(err) });
    } finally {
      sending = false;
      visible = false;
    }
    // Recusada: o campo volta vazio e com o foco, para digitar de novo sem tocar nele.
    if (error) { await tick(); field?.focus(); }
  }
</script>

<form class="keyring-notice" aria-label={m.keyring_locked_title({ label: server.label })} onsubmit={unlock}>
  <div class="keyring-head">
    <svg class="lock" viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
      <rect x="4" y="11" width="16" height="10" rx="2" /><path d="M8 11V7a4 4 0 0 1 8 0v4" />
    </svg>
    <div class="keyring-text">
      <p class="keyring-title">{m.keyring_locked_title({ label: server.label })}</p>
      <p class="keyring-hint">{m.keyring_locked_hint()}</p>
    </div>
  </div>
  <div class="pw" class:invalid={!!error}>
    <input bind:this={field} type={visible ? 'text' : 'password'} autocomplete="off" autocapitalize="off" spellcheck="false"
      bind:value={password} oninput={() => (error = null)}
      placeholder={m.keyring_password_placeholder()} aria-label={m.keyring_password_placeholder()}
      aria-invalid={!!error} disabled={busy} />
    <button type="button" class="reveal" onclick={() => (visible = !visible)} disabled={busy}
      aria-label={visible ? m.keyring_hide_password() : m.keyring_show_password()} aria-pressed={visible}>
      <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
        <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z" /><circle cx="12" cy="12" r="3" />
        {#if visible}<path d="M3 3l18 18" />{/if}
      </svg>
    </button>
  </div>
  {#if error}
    <p class="keyring-error" role="alert">{error}</p>
  {/if}
  <button type="submit" class="unlock-btn" disabled={busy || !password}>
    {busy ? m.keyring_unlocking() : m.keyring_unlock()}
  </button>
</form>

<style>
  .keyring-notice {
    margin: 0 var(--space-4) var(--space-3);
    padding: var(--space-3);
    background: var(--surface-raised);
    border: 1px solid var(--border-default);
    border-left: 3px solid var(--warning);
    border-radius: var(--radius-md);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .keyring-head { display: flex; gap: var(--space-2); align-items: flex-start; }
  .lock { flex-shrink: 0; margin-top: 1px; fill: none; stroke: var(--warning); stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  .keyring-text { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .keyring-title { margin: 0; font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .keyring-hint { margin: 0; font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.4; }
  .pw {
    position: relative;
    display: flex;
    align-items: center;
    background: var(--surface-inset);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
  }
  .pw:focus-within { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-dim); }
  .pw.invalid { border-color: var(--error); }
  .pw input {
    flex: 1;
    min-width: 0;
    height: 44px;
    border: 0;
    background: transparent;
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px;
    padding: 0 var(--space-3);
    outline: none;
  }
  .pw input::placeholder { color: var(--text-muted); }
  .reveal {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    border: 0;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .reveal svg { fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  .reveal:focus-visible { outline: 2px solid var(--accent); outline-offset: -4px; border-radius: var(--radius-sm); }
  .unlock-btn {
    min-height: 44px;
    border: none;
    border-radius: var(--radius-md);
    background: var(--accent);
    color: #fff;
    font-weight: 600;
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .unlock-btn:disabled { background: var(--surface-inset); color: var(--text-muted); cursor: default; }
  .keyring-error { margin: -4px 0 0; font-size: var(--text-xs); color: var(--error); }
</style>
