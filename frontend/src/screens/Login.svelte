<script lang="ts">
  import { onMount } from 'svelte';
  import { addServerWithRollback, getBaseUrl, validarPareamento } from '../lib/auth';
  import { getSessions } from '../lib/api';
  import { focusFirstInvalid } from '../lib/focusCycle';
  import { syncStatus, register as syncRegister, login as syncLogin } from '../lib/sync';
  import QrScanner from '../components/QrScanner.svelte';
  import HangarIntro from '../components/icons/HangarIntro.svelte';
  import * as m from '../paraglide/messages';

  interface Props {
    onLogin: () => void;
    onSyncLogin?: (encKey: CryptoKey) => void | Promise<void>; // sync mode: hydrate the vault before flipping in
  }
  let { onLogin, onSyncLogin }: Props = $props();

  let baseUrl = $state(getBaseUrl() || origemDoProprioServidor());
  // A página é servida pelo PRÓPRIO backend quando a porta é a do uvicorn (8765, default do
  // settings.port) — o caso do Electron e de abrir pelo navegador na própria máquina. Vite
  // dev/preview roda em 5173 (strictPort) e proxya /api, mas a origem dele NÃO é o backend:
  // preencher com ela daria erro de conexão com cara de bug, então fica vazio. Porta custom
  // (CP_PORT) também não preenche — campo vazio é o comportamento atual, que ninguém chama de
  // defeito.
  function origemDoProprioServidor(): string {
    if (typeof window === 'undefined') return '';
    return window.location.port === '8765' ? window.location.origin : '';
  }

  let token = $state('');
  let loading = $state(false);
  let error = $state('');
  // Erro de VALIDAÇÃO (pareamento malformado) marca os campos com aria-invalid e foca o primeiro;
  // erro de REDE (probe falhou) é visível mas NÃO marca campo indevidamente.
  let erroValidacao = $state(false);
  let scanning = $state(false);
  let loginFormEl = $state<HTMLFormElement | null>(null);

  // Foca o primeiro campo inválido DEPOIS do render: o aria-invalid só existe no DOM após o flush,
  // e focusFirstInvalid o procura no DOM — chamar no mesmo handler síncrono não acharia nada.
  $effect(() => { if (erroValidacao) focusFirstInvalid(loginFormEl); });

  // Cloud-sync: quando o hub tem CP_SYNC=1, troca o form URL+token por user/senha. null = desabilitado.
  let syncMode = $state<null | { registered: boolean }>(null);
  let user = $state('');
  let password = $state('');
  let bootstrap = $state('');
  let syncLoading = $state(false);
  let syncError = $state('');

  // The QR encodes the pairing URL (…/?token=…). Passa pelo MESMO validarPareamento estrito do
  // manual/deep-link: QR inválido NÃO conecta nem fecha silencioso — erro visível, form aberto pra
  // escanear de novo ou digitar. Token cru sem URL não é aceito aqui (só o saveToken do
  // ServerManager usa aceitarTokenCru). Necessário porque um PWA iOS instalado tem storage próprio.
  function handleScan(text: string) {
    const cru = text.trim();
    const pareamento = validarPareamento(cru);
    scanning = false;
    if (!pareamento) {
      erroValidacao = true;
      error = cru.includes('://')
        ? m.lista_qr_invalido()
        : m.login_qr_sem_url();
      return;
    }
    baseUrl = pareamento.base;
    token = pareamento.token;
    void connect();
  }

  async function connect() {
    // Valida ANTES de tocar storage (round 4 da 4b): URL/token inválidos não chamam addServer, não
    // alteram storage e não navegam. Campos separados (URL + token) -> monta o texto de pareamento
    // e passa pelo MESMO validarPareamento estrito do QR/deep-link.
    const base = baseUrl.trim();
    const tok = token.trim();
    const cru = base + (base.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(tok);
    const pareamento = validarPareamento(cru);
    if (!pareamento) {
      erroValidacao = true;
      error = !base || !tok
        ? m.login_informe_url_token()
        : m.login_url_token_invalidos();
      return;
    }

    erroValidacao = false;
    loading = true;
    error = '';

    // Add TRANSACIONAL (round 4): o helper valida, adiciona, roda o probe (getSessions) e — em
    // falha — restaura o snapshot COMPLETO (lista, ativo e cookie). Servidor existente com token
    // novo volta como estava; novo não permanece. O form segue aberto pra retry.
    try {
      const r = await addServerWithRollback(pareamento.base, pareamento.token, () => getSessions());
      if (!r.succeeded) {
        error = m.login_url_token_invalidos();
        return;
      }
      onLogin();
    } catch (err) {
      erroValidacao = false;   // erro de REDE: visível, mas não marca campo
      error = err instanceof Error
        ? `${m.falha_conexao()}: ${err.message}`
        : m.erro_desconhecido();
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
      syncError = err instanceof Error ? err.message : m.login_falha();
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
    // Deep-link de pareamento (?token=…): valida a URL COMPLETA antes de extrair QUALQUER coisa.
    // URLSearchParams.get descarta duplicatas e api vazia silenciosamente — o validator precisa
    // ver a URL inteira (round 5): token/api duplicados ou api vazia são rejeitados sem alterar
    // campos, URL, storage ou iniciar conexão. Visita normal (sem token na URL) não é deep-link.
    const href = window.location.href;
    if (!new URL(href).searchParams.has('token')) return;   // presença só: extração é do validator
    const pareamento = validarPareamento(href);
    if (!pareamento) {
      // Deep-link inválido: NÃO limpa a query (o usuário precisa ver o link pra corrigir), NÃO
      // preenche campos e NÃO conecta — erro visível com retry manual. erroValidacao associa o
      // erro aos campos (aria-invalid/aria-describedby) e o $effect foca o primeiro inválido.
      erroValidacao = true;
      error = m.login_deeplink_invalido();
      return;
    }
    // baseUrl ABSOLUTO = ?api= ou a origem onde o app foi aberto (ex: https://casa.ts.net).
    baseUrl = pareamento.base;
    token = pareamento.token;
    // Só depois da validação: remove o segredo do histórico e conecta com os valores validados.
    window.history.replaceState({}, '', window.location.pathname + window.location.hash);
    void connect();
  });
</script>

<div class="login-screen">
  <div class="login-content">
    <div class="app-mark"><HangarIntro size={64} /></div>
    <h1 class="app-name">Hangar</h1>
    <p class="app-tagline">{m.login_tagline()}</p>

    {#if syncMode}
      <form onsubmit={doSyncSubmit} class="login-form">
        <div class="field">
          <label class="field-label" for="sync-user">{m.login_usuario()}</label>
          <input id="sync-user" class="field-input" bind:value={user} autocomplete="username" autocapitalize="off" spellcheck={false} required />
        </div>
        <div class="field">
          <label class="field-label" for="sync-pass">{m.login_senha()}</label>
          <input id="sync-pass" type="password" class="field-input" bind:value={password} autocomplete="current-password" required />
        </div>
        {#if !syncMode.registered}
          <div class="field">
            <label class="field-label" for="sync-boot">{m.login_token_ativacao()}</label>
            <input id="sync-boot" type="password" class="field-input" bind:value={bootstrap} required />
          </div>
        {/if}
        {#if syncError}
          <p class="error-msg" role="alert">{syncError}</p>
        {/if}
        <button type="submit" class="connect-btn" disabled={syncLoading || !user.trim() || !password}>
          {syncLoading ? m.login_entrando() : (syncMode.registered ? m.login_entrar() : m.login_criar_acesso())}
        </button>
      </form>
    {:else}
    <form onsubmit={handleSubmit} class="login-form" bind:this={loginFormEl}>
      <div class="field">
        <label class="field-label" for="base-url">{m.sessao_url_servidor()}</label>
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
          aria-invalid={erroValidacao || undefined}
          aria-describedby={erroValidacao ? 'login-err' : undefined}
        />
      </div>

      <div class="field">
        <label class="field-label" for="token">{m.sessao_token()}</label>
        <input
          id="token"
          type="password"
          class="field-input"
          bind:value={token}
          placeholder="••••••••••••••••"
          autocomplete="current-password"
          required
          aria-invalid={erroValidacao || undefined}
          aria-describedby={erroValidacao ? 'login-err' : undefined}
        />
      </div>

      {#if error}
        <p id="login-err" class="error-msg" role="alert">{error}</p>
      {/if}

      <button
        type="submit"
        class="connect-btn"
        disabled={loading || !token.trim()}
      >
        {loading ? m.login_conectando() : m.login_conectar()}
      </button>

      <button type="button" class="scan-btn" onclick={() => (scanning = true)}>
        {m.sessao_escanear_qr()}
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

  /* A marca adota o --accent (currentColor), que vem da paleta do papel de parede. */
  .app-mark {
    display: flex;
    justify-content: center;
    color: var(--accent);
    margin-bottom: var(--space-4);
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
