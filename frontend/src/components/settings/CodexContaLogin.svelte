<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { serverIdentidade } from '../../lib/auth';
  import { createCodexAccountForServer, prepareCodexAccountForServer,
    getCodexPreparationForServer, startCodexAccountLoginForServer,
    getCodexAccountLoginForServer, cancelCodexAccountLoginForServer, getCodexAccountsForServer,
    getCodexIntegrationForServer, startCodexIntegrationForServer,
    getRootsForServer, createSessionForServer, mensagemDeErro, textoEtapaCodex,
    codexAccountMessage, contaCodexParaEntrar, importacaoAposLoginCodex, idContaCodex,
    type Server, type CodexLoginAttempt, type CodexAccount } from '@hangar/core';
  import { copyText } from '../../lib/clipboard';
  import * as m from '../../paraglide/messages';

  type Importacao = 'claude' | 'heranca';
  type Resultado = { ok: boolean; issues: CodexAccount['sync']['issues']; trust_pending: boolean;
    herdado?: Record<string, number> | null };
  // Aviso do que NÃO foi copiado de propósito: a variável de execução aponta pra pasta da conta
  // de origem, e copiada mandaria a conta nova ler a pasta da outra. Não é erro.
  const DELIBERADO = new Set(['codex_account_mcp_runtime_excluded', 'codex_account_mcp_auth_excluded']);

  // `herdar`: o card já respondeu "herdar agora" por esta conta — pula login e pergunta.
  let { server, accountId, herdar = false, oncomplete }: {
    server: Server; accountId?: string; herdar?: boolean; oncomplete: () => void;
  } = $props();
  let name = $state('');
  let account = $state<string | undefined>();
  let attempt = $state<CodexLoginAttempt | null>(null);
  let pergunta = $state<Importacao | null>(null);
  let importando = $state(false);
  let resultado = $state<Resultado | null>(null);
  let etapa = $state('');
  let abrindoSessao = $state(false);
  let busy = $state(false);
  let error = $state('');
  let generation = 0;
  let controller = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const identity = $derived(serverIdentidade(server));
  const id = $derived(idContaCodex(name));
  const url = $derived.by(() => {
    try {
      const u = new URL(attempt?.verification_url ?? '');
      return u.protocol === 'https:' && !u.username && !u.password ? u.href : null;
    } catch { return null; }
  });
  const failed = (e: unknown) => e instanceof Error ? e.message : m.codex_ui_login_error();

  function espera(ms: number, signal: AbortSignal) {
    return new Promise<void>((resolve) => {
      const t = setTimeout(resolve, ms);
      signal.addEventListener('abort', () => { clearTimeout(t); resolve(); }, { once: true });
    });
  }

  function show(next: CodexLoginAttempt | null, g: number) {
    if (g !== generation) return;
    attempt = next;
    if (next?.status === 'completed' && account) void aposLogin(server, account, g, controller.signal);
  }

  // Logou. Primeira conta (a padrão) → oferecer a importação do Claude; adicional → herdar da
  // padrão, se ela tiver algo. Sem nada a oferecer, fecha como sempre fechou.
  async function aposLogin(s: Server, idc: string, g: number, signal: AbortSignal) {
    try {
      const tipo = importacaoAposLoginCodex(await getCodexAccountsForServer(s, signal), idc);
      if (g !== generation) return;
      if (tipo === 'heranca') { pergunta = 'heranca'; return; }
      if (tipo === 'claude') {
        const estado = await getCodexIntegrationForServer(s, signal);
        if (g !== generation) return;
        if (!estado.ultima_execucao) { pergunta = 'claude'; return; }
      }
    } catch { /* lista indisponível: nada a perguntar */ }
    if (g === generation) oncomplete();
  }

  function nomeEtapa(codigo: string | null | undefined): string {
    if (codigo === 'configuracoes') return m.codex_etapa_configuracoes();
    if (codigo === 'recursos') return m.codex_etapa_recursos();
    if (codigo === 'plugins') return m.codex_etapa_plugins();
    return '';
  }

  /** Quem aprova hook é o Codex, nunca o app: o botão só abre a sessão onde ele pergunta. */
  async function abrirSessaoCodex() {
    if (abrindoSessao || !account) return;
    abrindoSessao = true; error = '';
    try {
      const raizes = await getRootsForServer(server);
      const cwd = raizes[0]?.path;
      if (!cwd) throw new Error(m.codex_ui_sem_pasta());
      const nome = `codex-${account}`;
      try {
        await createSessionForServer(server, { name: nome, cwd, provider: 'codex', codex_account: account });
      } catch (e) {
        // Sessão com esse nome já existe: ela serve igual para o Codex perguntar.
        if (!(e instanceof Error) || !/409/.test(e.message)) throw e;
      }
      window.location.hash = `#/chat/${encodeURIComponent(server.id)}/${encodeURIComponent(nome)}`;
      oncomplete();
    } catch (e) {
      error = e instanceof Error && e.message ? e.message : m.codex_ui_sessao_erro();
    } finally { abrindoSessao = false; }
  }

  async function importar(tipo: Importacao) {
    if (importando || !account) return;
    const s = server, idc = account, g = ++generation;
    const signal = controller.signal;
    clearTimeout(timer);
    pergunta = null; importando = true; error = ''; etapa = '';
    let saida: Resultado;
    try {
      if (tipo === 'heranca') {
        let sync = await prepareCodexAccountForServer(s, idc);
        while (sync.status === 'running') {
          etapa = nomeEtapa(sync.etapa);
          await espera(1000, signal);
          if (g !== generation) return;
          sync = await getCodexPreparationForServer(s, idc, signal);
        }
        if (g !== generation) return;
        saida = { ok: sync.status === 'ready', issues: sync.issues, trust_pending: sync.trust_pending,
          herdado: sync.herdado };
      } else {
        let estado = await startCodexIntegrationForServer(s);
        while (estado.estado === 'executando') {
          etapa = textoEtapaCodex(estado.etapa, (codigo, params) => {
            const fn = (m as unknown as Record<string, unknown>)[`harness_codex_m_${codigo}`];
            return typeof fn === 'function'
              ? (fn as (p: Record<string, string>) => string)(params) : undefined;
          });
          await espera(1000, signal);
          if (g !== generation) return;
          estado = await getCodexIntegrationForServer(s, signal);
        }
        if (g !== generation) return;
        saida = { ok: estado.estado === 'ok', issues: [], trust_pending: false };
      }
    } catch (e) {
      if (g !== generation) return;
      error = failed(e);
      saida = { ok: false, issues: [], trust_pending: false };
    } finally { if (g === generation) { importando = false; etapa = ''; } }
    if (saida.ok && !saida.issues.length && !saida.trust_pending) { oncomplete(); return; }
    resultado = saida;
  }

  // "Adicionar conta" com a padrao ainda sem login = entrar NELA, sem nome nem conta extra.
  async function entrarNaPadrao(s: Server, g: number, signal: AbortSignal) {
    try {
      const idc = contaCodexParaEntrar(await getCodexAccountsForServer(s, signal));
      if (g !== generation || !idc) return;
      account = idc;
      void read(s, idc, g, signal);
    } catch { /* lista indisponivel: segue criando conta nomeada, como antes */ }
  }

  async function read(s: Server, idc: string, g: number, signal: AbortSignal) {
    try {
      const next = await getCodexAccountLoginForServer(s, idc, signal);
      if (g !== generation) return;
      error = '';
      show(next, g);
      if (next?.status === 'waiting') timer = setTimeout(() => read(s, idc, g, signal), 1000);
    } catch (e) {
      if (g === generation) error = failed(e);
    }
  }

  $effect(() => {
    void identity;
    const s = untrack(() => server), idc = accountId, direto = herdar;
    const g = ++generation;
    controller.abort();
    controller = new AbortController();
    clearTimeout(timer);
    account = idc; attempt = null; pergunta = null; importando = false; resultado = null;
    busy = false; error = ''; name = '';
    // untrack: importar() lê e escreve estado antes do primeiro await; rastreado, o efeito se re-dispara em laço.
    if (idc && direto) untrack(() => void importar('heranca'));
    else if (idc) void read(s, idc, g, controller.signal);
    else void entrarNaPadrao(s, g, controller.signal);
    return () => { ++generation; controller.abort(); clearTimeout(timer); };
  });

  onMount(() => {
    const resume = () => {
      if (document.visibilityState !== 'visible' || !account || busy || attempt?.status !== 'waiting') return;
      const g = ++generation;
      controller.abort();
      controller = new AbortController();
      clearTimeout(timer);
      void read(server, account, g, controller.signal);
    };
    document.addEventListener('visibilitychange', resume);
    return () => document.removeEventListener('visibilitychange', resume);
  });

  async function start() {
    if (busy) return;
    const s = server, g = ++generation;
    const signal = controller.signal;
    clearTimeout(timer);
    busy = true; error = '';
    try {
      let idc = account;
      if (!idc) {
        const created = await createCodexAccountForServer(s, id);
        if (g !== generation) return;
        account = idc = created.id;
      }
      const next = await startCodexAccountLoginForServer(s, idc);
      if (g !== generation) return;
      show(next, g);
      if (next.status === 'waiting') timer = setTimeout(() => read(s, idc!, g, signal), 1000);
    } catch (e) {
      if (g === generation) error = failed(e);
    } finally { if (g === generation) busy = false; }
  }

  async function cancel() {
    if (!account || !attempt || busy) return;
    const s = server, idc = account, attemptId = attempt.attempt_id, g = ++generation;
    clearTimeout(timer); busy = true; error = '';
    try { show(await cancelCodexAccountLoginForServer(s, idc, attemptId), g); }
    catch (e) { if (g === generation) error = failed(e); }
    finally { if (g === generation) busy = false; }
  }
</script>

<div class="codex-login" aria-busy={busy || importando}>
  {#if attempt?.status === 'waiting'}
    <p role="status">{m.novacred_codex_aguardando()}</p>
    <ol class="passos">
      <li>{m.novacred_codex_passo1()}{#if url}<br /><a href={url} target="_blank" rel="noopener noreferrer">{url}</a>{/if}</li>
      {#if attempt.user_code}
        <li>{m.novacred_codex_passo2()}<br />
          <span class="codigo"><code>{attempt.user_code}</code>
            <button type="button" class="mini" onclick={() => copyText(attempt!.user_code!).catch((e) => error = failed(e))}>{m.comum_copiar_codigo()}</button>
          </span>
        </li>
      {/if}
    </ol>
    <div class="acoes">
      <button type="button" disabled={busy} onclick={cancel}>{m.codex_ui_cancel_login()}</button>
      {#if error}<button type="button" onclick={() => account && read(server, account, generation, controller.signal)}>{m.novacred_codex_tentar()}</button>{/if}
    </div>
  {:else if pergunta}
    <p role="status">{m.novacred_codex_concluido()}</p>
    <p class="pergunta">{pergunta === 'claude' ? m.codex_ui_importar_claude_pergunta() : m.codex_ui_herdar_pergunta()}</p>
    <p class="dica">{pergunta === 'claude' ? m.codex_ui_importar_claude_desc() : m.codex_ui_herdar_desc()}</p>
    <div class="acoes">
      <button type="button" class="primaria" onclick={() => importar(pergunta!)}>
        {pergunta === 'claude' ? m.codex_ui_importar_agora() : m.codex_ui_herdar_agora()}
      </button>
      <button type="button" onclick={oncomplete}>{m.codex_ui_depois()}</button>
    </div>
  {:else if importando}
    <p role="status">{m.codex_ui_importando()}</p>
    {#if etapa}<p class="dica">{etapa}</p>{/if}
  {:else if resultado}
    {#if resultado.ok}<p role="status">{m.codex_ui_importado()}</p>
    {:else}<p role="alert">{error || m.codex_ui_importar_erro()}</p>{/if}
    {#if resultado.herdado}
      <ul class="herdado">
        {#each [['skills', m.codex_herdado_skills], ['hooks', m.codex_herdado_hooks], ['agents', m.codex_herdado_agents], ['plugins', m.codex_herdado_plugins], ['mcps', m.codex_herdado_mcps]] as [chave, rotulo] (chave)}
          {#if resultado.herdado[chave as string]}
            <li>{(rotulo as (p: { n: number }) => string)({ n: resultado.herdado[chave as string] })}</li>
          {/if}
        {/each}
      </ul>
    {/if}
    <!-- Aviso deliberado (variável de execução do MCP) não é erro: cor e papel diferentes. -->
    {#each resultado.issues as issue, i (i)}
      {#if DELIBERADO.has(issue.code)}
        <p class="aviso" role="status">{codexAccountMessage(issue)}{#if issue.params?.server} <span class="dica">{m.codex_ui_mcp_detalhe({ servidor: issue.params.server, variavel: issue.params.variable ?? '—' })}</span>{/if}</p>
      {:else}
        <p role="alert">{codexAccountMessage(issue)}</p>
      {/if}
    {/each}
    {#if resultado.trust_pending}
      <p role="status">{m.harness_codex_confianca()}</p>
      <p class="dica">{m.codex_ui_confianca_como()}</p>
    {/if}
    <div class="acoes">
      {#if resultado.trust_pending}
        <button type="button" class="primaria" disabled={abrindoSessao} onclick={abrirSessaoCodex}>
          {abrindoSessao ? m.codex_ui_abrindo_sessao() : m.codex_ui_abrir_sessao()}
        </button>
        <button type="button" onclick={oncomplete}>{m.codex_ui_depois()}</button>
      {:else}
        <button type="button" class="primaria" onclick={oncomplete}>{m.codex_ui_concluir()}</button>
      {/if}
    </div>
  {:else if attempt?.status === 'completed'}
    <p role="status">{m.novacred_codex_concluido()}</p>
  {:else}
    <p class="dica">{m.codex_ui_intro()}</p>
    {#if !account}
      <label>{m.novacred_nome_conta()}<input bind:value={name} disabled={busy} autocomplete="off" /></label>
      <p class="dica">{m.codex_ui_nome_dica({ id: id || '…' })}</p>
    {/if}
    {#if attempt?.error}<p role="alert">{codexAccountMessage(attempt.error)}</p>{/if}
    {#if attempt?.status === 'cancelled'}<p role="status">{m.codex_ui_cancelled()}</p>{/if}
    <button type="button" class="primaria" disabled={busy || (!account && !id)} onclick={start}>
      {busy ? (account ? m.codex_ui_conectando() : m.codex_ui_criando()) : m.contas_entrar()}
    </button>
  {/if}
  {#if error && !resultado}<p role="alert">{error}</p>{/if}
</div>

<style>
  .codex-login { display: flex; flex-direction: column; gap: var(--space-3); min-width: 0; }
  p { margin: 0; font-size: var(--text-sm); }
  .dica { color: var(--text-muted); }
  .aviso { color: var(--warning); }
  .herdado { margin: 0; padding-left: 1.25em; font-size: var(--text-sm); color: var(--text-secondary); }
  .pergunta { font-weight: 600; }
  .passos { margin: 0; padding-left: 1.25em; display: flex; flex-direction: column; gap: var(--space-2); font-size: var(--text-sm); }
  .codigo { display: inline-flex; align-items: center; gap: var(--space-2); flex-wrap: wrap; }
  .codigo code { font-size: var(--text-lg); letter-spacing: .08em; }
  .acoes { display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .acoes > button { flex: 1 1 auto; }
  label { display: flex; flex-direction: column; gap: var(--space-2); }
  input { background: var(--surface-inset); color: var(--text-primary); border: 1px solid var(--border-default); border-radius: var(--radius-sm); padding: var(--space-3); }
  button { background: var(--surface-raised); color: var(--text-primary); border: 1px solid var(--border-default); border-radius: var(--radius-sm); min-height: 44px; padding: var(--space-2) var(--space-3); cursor: pointer; }
  button.primaria { background: var(--accent); color: #fff; border-color: var(--accent); }
  button.mini { min-height: 32px; padding: 0 var(--space-2); font-size: var(--text-xs); }
  button:disabled { opacity: .55; cursor: default; }
  a, code { overflow-wrap: anywhere; color: var(--accent); }
  [role='alert'] { color: var(--error); }
  button:focus-visible, input:focus-visible, a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
