<script lang="ts">
  import { setCredentials } from '../lib/auth';
  import { getSessions } from '../lib/api';

  interface Props {
    onLogin: () => void;
  }
  let { onLogin }: Props = $props();

  let baseUrl = $state(localStorage.getItem('cp_base_url') ?? '');
  let token = $state('');
  let loading = $state(false);
  let error = $state('');

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    loading = true;
    error = '';

    // Temporarily set credentials so api.ts picks them up
    const prevBase = localStorage.getItem('cp_base_url');
    const prevToken = localStorage.getItem('cp_token');
    setCredentials(baseUrl.trim(), token.trim());

    try {
      await getSessions();
      onLogin();
    } catch (err) {
      // Restore previous credentials on failure
      if (prevBase !== null) localStorage.setItem('cp_base_url', prevBase);
      else localStorage.removeItem('cp_base_url');
      if (prevToken !== null) localStorage.setItem('cp_token', prevToken);
      else localStorage.removeItem('cp_token');

      error = err instanceof Error
        ? `Falha na conexão: ${err.message}`
        : 'Erro desconhecido';
    } finally {
      loading = false;
    }
  }
</script>

<div class="login-screen">
  <div class="login-content">
    <h1 class="app-name">Claude Pocket</h1>
    <p class="app-tagline">Controle suas sessões de qualquer lugar</p>

    <form onsubmit={handleSubmit} class="login-form">
      <div class="field">
        <label class="field-label" for="base-url">URL do servidor</label>
        <input
          id="base-url"
          type="url"
          class="field-input"
          bind:value={baseUrl}
          placeholder="http://192.168.x.x:8000"
          autocomplete="url"
          autocorrect="off"
          autocapitalize="off"
          spellcheck={false}
          inputmode="url"
        />
      </div>

      <div class="field">
        <label class="field-label" for="token">Token</label>
        <input
          id="token"
          type="password"
          class="field-input"
          bind:value={token}
          placeholder="••••••••••••••••"
          autocomplete="current-password"
          required
        />
      </div>

      {#if error}
        <p class="error-msg" role="alert">{error}</p>
      {/if}

      <button
        type="submit"
        class="connect-btn"
        disabled={loading || !token.trim()}
      >
        {loading ? 'Conectando…' : 'Conectar'}
      </button>
    </form>
  </div>
</div>

<style>
  .login-screen {
    flex: 1;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: calc(env(safe-area-inset-top) + 80px);
    padding-left: var(--space-6);
    padding-right: var(--space-6);
    padding-bottom: env(safe-area-inset-bottom);
    overflow-y: auto;
  }

  .login-content {
    width: 100%;
    max-width: 400px;
  }

  .app-name {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    text-align: center;
    margin-bottom: var(--space-2);
  }

  .app-tagline {
    font-size: var(--text-sm);
    color: var(--text-muted);
    text-align: center;
    margin-bottom: var(--space-8);
  }

  .login-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .field-label {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    font-weight: 500;
  }

  .field-input {
    height: 48px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px;
    padding: 0 var(--space-4);
    outline: none;
    transition: border-color 180ms ease-out;
  }

  .field-input::placeholder {
    color: var(--text-muted);
  }

  .field-input:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .error-msg {
    font-size: var(--text-sm);
    color: var(--error);
    background: rgba(255, 69, 58, 0.08);
    border: 1px solid rgba(255, 69, 58, 0.2);
    border-radius: var(--radius-sm);
    padding: var(--space-3);
  }

  .connect-btn {
    height: 52px;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    font-size: var(--text-base);
    font-weight: 600;
    transition: background 180ms ease-out;
    width: 100%;
  }

  .connect-btn:active:not(:disabled) {
    background: var(--accent-press);
  }

  .connect-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
