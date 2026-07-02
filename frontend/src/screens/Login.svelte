<script lang="ts">
  import { onMount } from 'svelte';
  import { addServer, removeServer, selectServer, getActiveId, getBaseUrl } from '../lib/auth';
  import { getSessions } from '../lib/api';
  import { syncStatus, register as syncRegister, login as syncLogin } from '../lib/sync';
  import QrScanner from '../components/QrScanner.svelte';

  interface Props {
    onLogin: () => void;
    onSyncLogin?: (encKey: CryptoKey) => void | Promise<void>; // sync mode: hydrate the vault before flipping in
  }
  let { onLogin, onSyncLogin }: Props = $props();

  let baseUrl = $state(getBaseUrl());
  let token = $state('');
  let loading = $state(false);
  let error = $state('');
  let scanning = $state(false);

  // Cloud-sync: quando o hub tem CP_SYNC=1, troca o form URL+token por user/senha. null = desabilitado.
  let syncMode = $state<null | { registered: boolean }>(null);
  let user = $state('');
  let password = $state('');
  let bootstrap = $state('');
  let syncLoading = $state(false);
  let syncError = $state('');

  // The QR encodes the pairing URL (…/?token=…). Pull the token (and optional ?api=)
  // out of it — or accept a bare token — then connect. Needed because an installed iOS
  // PWA has its own storage, so it must be paired once from inside the app.
  function handleScan(text: string) {
    let tok = text.trim();
    try {
      const u = new URL(text);
      const t = u.searchParams.get('token');
      if (t) tok = t;
      // baseUrl ABSOLUTO: ?api= se houver, senão a origem da própria URL do QR.
      baseUrl = u.searchParams.get('api') ?? u.origin;
    } catch {
      /* not a URL — treat as a raw token */
    }
    scanning = false;
    if (!tok) return;
    token = tok;
    void connect();
  }

  async function connect() {
    loading = true;
    error = '';

    // Adiciona+ativa o servidor (api.ts já lê o ativo). Em falha, rollback: se era novo remove e
    // restaura o ativo anterior — pra um login ruim não sujar a lista nem trocar o server bom.
    const prevActive = getActiveId();
    const { id, existed } = addServer(baseUrl.trim(), token.trim());

    try {
      await getSessions();
      onLogin();
    } catch (err) {
      if (!existed) removeServer(id);
      if (prevActive) selectServer(prevActive);
      error = err instanceof Error
        ? `Falha na conexão: ${err.message}`
        : 'Erro desconhecido';
    } finally {
      loading = false;
    }
  }

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    await connect();
  }

  // Sync mode: registra (1o acesso) e faz login. A hidratacao do vault (onSyncLogin) precisa
  // TERMINAR antes de onLogin() flipar a tela, senao o app monta com a lista vazia e popula depois.
  async function doSyncSubmit(e: SubmitEvent) {
    e.preventDefault();
    syncLoading = true;
    syncError = '';
    try {
      if (syncMode && !syncMode.registered) {
        await syncRegister(user.trim(), password, bootstrap.trim());
      }
      const encKey = await syncLogin(user.trim(), password);
      await onSyncLogin?.(encKey);
      onLogin();
    } catch (err) {
      syncError = err instanceof Error ? err.message : 'falha';
    } finally {
      syncLoading = false;
    }
  }

  onMount(async () => {
    // Sync ligado? Mostra o form user/senha e NAO roda o pareamento ?token= (o form URL+token fica oculto).
    const s = await syncStatus();
    if (s?.enabled) {
      syncMode = { registered: s.registered };
      return;
    }
    // QR pairing: the QR opens this app with ?token=… (and optional ?api=…). Pick it up,
    // strip it from the URL (don't leave the secret in history), and connect automatically.
    const params = new URLSearchParams(window.location.search);
    const t = params.get('token');
    if (!t) return;
    token = t;
    // baseUrl ABSOLUTO = ?api= ou a origem onde o app foi aberto (ex: https://casa.ts.net).
    baseUrl = params.get('api') ?? window.location.origin;
    window.history.replaceState({}, '', window.location.pathname + window.location.hash);
    void connect();
  });
</script>

<div class="login-screen">
  <div class="login-content">
    <h1 class="app-name">Claude Pocket</h1>
    <p class="app-tagline">Controle suas sessões de qualquer lugar</p>

    {#if syncMode}
      <form onsubmit={doSyncSubmit} class="login-form">
        <div class="field">
          <label class="field-label" for="sync-user">Usuário</label>
          <input id="sync-user" class="field-input" bind:value={user} autocomplete="username" autocapitalize="off" spellcheck={false} required />
        </div>
        <div class="field">
          <label class="field-label" for="sync-pass">Senha</label>
          <input id="sync-pass" type="password" class="field-input" bind:value={password} autocomplete="current-password" required />
        </div>
        {#if !syncMode.registered}
          <div class="field">
            <label class="field-label" for="sync-boot">Token de ativação (primeiro acesso)</label>
            <input id="sync-boot" type="password" class="field-input" bind:value={bootstrap} required />
          </div>
        {/if}
        {#if syncError}
          <p class="error-msg" role="alert">{syncError}</p>
        {/if}
        <button type="submit" class="connect-btn" disabled={syncLoading || !user.trim() || !password}>
          {syncLoading ? 'Entrando…' : (syncMode.registered ? 'Entrar' : 'Criar acesso')}
        </button>
      </form>
    {:else}
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

      <button type="button" class="scan-btn" onclick={() => (scanning = true)}>
        Escanear QR
      </button>
    </form>
    {/if}
  </div>
</div>

{#if scanning}
  <QrScanner onScan={handleScan} onClose={() => (scanning = false)} />
{/if}

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

  /* Disabled = inerte de verdade (bg neutro flat + texto muted), nao indigo cheio a 50% que parece
     meio-clicavel. Mesmo padrao do send-btn--disabled. */
  .connect-btn:disabled {
    background: var(--bg-hover);
    color: var(--text-muted);
    cursor: default;
  }

  .scan-btn {
    height: 48px;
    background: transparent;
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-secondary);
    font-size: var(--text-base);
    font-weight: 500;
    width: 100%;
    transition: background 180ms ease-out;
  }

  .scan-btn:active {
    background: var(--bg-hover);
  }
</style>
