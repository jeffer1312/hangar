<script lang="ts">
  import * as m from '../paraglide/messages';
  import BottomSheet from './BottomSheet.svelte';
  import ShortcutTiles from './ShortcutTiles.svelte';
  import ShortcutTransfer from './ShortcutTransfer.svelte';
  import { desktop } from '../lib/desktop.svelte';
  import { rotuloModsCelular } from '../lib/modsCelular.svelte';
import { useSessionServer } from '../lib/sessionServer';
  import { listAccountTargets, type AccountTarget, type LiveShortcutTerminal, type ShortcutSendText, type ShortcutShell } from '@hangar/core';
  import type { CustomScoped } from '../lib/shortcuts.svelte';

  // Acoes que saíram da NavBar do CELULAR pro menu "⋯". Elas custavam 80px fixos da barra e sao de
  // uso raro; o nome da sessao, que e a informacao mais disputada ali, chegava a "clau…". No desktop
  // a barra tem folga e os botoes continuam inline — o Chat so passa `onMenu` no mobile.
  interface Props {
    open: boolean;
    onClose: () => void;
    // Atalhos CUSTOMIZADOS da fileira configurável: na barra estreita eles moram aqui (decisão da
    // sessão de grilling — a barra fica estável e o "⋯" já é o lugar do resto das ações).
    shortcuts?: CustomScoped[];
    projectName?: string;
    projectError?: string;
    projectKey?: string;
    sessionName?: string;
    hangarOf?: (key: string) => LiveShortcutTerminal | null;
    sessionTerminal?: (key: string) => LiveShortcutTerminal | null;
    onShortcut?: (s: ShortcutSendText | ShortcutShell) => void;
    onEditShortcuts?: () => void;
    onRun?: () => void;           // ausente = sessão sem workflow (orquestrador)
    runRunning?: boolean;
    onActivity?: () => void;      // ausente = sessao sem atividade pra mostrar
    onAttachments: () => void;
    /** Passagem de bastão. Mora AQUI porque no celular a lista de sessões não tem menu por sessão
     *  (as ações dela são swipe no SessionCard) — o "⋯" do chat aberto é a única entrada. */
    onBastao: () => void;
    /** A mesma conversa noutra conta Claude. Ausente = sessão fora do Claude na conta Anthropic. */
    onTrocarConta?: (conta: AccountTarget) => void;
    contaBloqueada?: boolean;
    /** Compartilhar a sessão. Ausente = sessão de convite (não se recompartilha). */
    onShare?: () => void;
    /** Troca terminal ⇄ sem terminal (só Claude): rótulo é o destino, bloqueado fora de ociosa. */
    onTrocarModo?: () => void;
    modoDestinoTerminal?: boolean;
    modoBloqueado?: boolean;
    /** Recicla o processo da sessão sem terminal (relê MCP/hooks/settings). Ausente = não é Claude sem terminal. */
    onRecarregar?: () => void;
    recarregarBloqueado?: boolean;
    /** Liga e desliga a interface dos mods no celular. Ausente = sessão sem faixa nem painel de mod: sem item. */
    onAlternarMods?: () => void;
    modsLigado?: boolean;
    /** Painéis de mod abertos na sessão, para a contagem do rótulo quando a interface está oculta. */
    modsPaineis?: number;
    activityRunning?: boolean;
    activityBadge?: number;
  }
  let {
    open, onClose, onRun, runRunning = false,
    shortcuts = [], projectName = '', projectError = '', projectKey = undefined, sessionName = undefined,
    hangarOf = undefined, sessionTerminal = undefined, onShortcut = undefined, onEditShortcuts = undefined,
    onActivity, activityRunning = false, activityBadge = 0, onAttachments, onBastao, onShare,
    onTrocarConta, contaBloqueada = false,
    onTrocarModo, modoDestinoTerminal = false, modoBloqueado = false,
    onRecarregar, recarregarBloqueado = false,
    onAlternarMods, modsLigado = false, modsPaineis = 0,
  }: Props = $props();
  const sessionServer = useSessionServer();

  function pick(fn: () => void) {
    onClose();
    fn();
  }

  // "Continuar em outra conta" abre um segundo nível na mesma folha: com resumo ou a mesma conversa.
  let view = $state<'main' | 'account' | 'accounts'>('main');
  $effect(() => { if (open) view = 'main'; });

  let accounts = $state<AccountTarget[] | null>(null);
  let accountsError = $state('');
  // Só a última leitura escreve: abrir, voltar e abrir de novo não deixa a resposta velha por cima.
  let accountsSeq = 0;
  async function openAccounts() {
    if (!sessionName) return;
    const seq = ++accountsSeq;
    view = 'accounts';
    accounts = null;
    accountsError = '';
    try {
      const list = await listAccountTargets(sessionName, sessionServer());
      if (seq === accountsSeq) accounts = list;
    } catch (e) {
      if (seq === accountsSeq) accountsError = e instanceof Error ? e.message : String(e);
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={m.navbar_mais_acoes()} centered={desktop.atual}>
  {#if view === 'account'}
    <div class="more">
      <div class="more-head">
        <button class="back" onclick={() => (view = 'main')} aria-label={m.comum_voltar()}>‹</button>
        <h2 class="more-title">{m.bastao_menu()}</h2>
      </div>
      <button class="item" onclick={() => pick(onBastao)}>
        <span class="txt">
          <span class="label">{m.conta_com_resumo()}</span>
          <span class="sub">{m.bastao_sub()}</span>
        </span>
        <span class="chev" aria-hidden="true">›</span>
      </button>
      <button class="item" onclick={openAccounts} disabled={!onTrocarConta || contaBloqueada}>
        <span class="txt">
          <span class="label">{m.conta_mesma_conversa()}</span>
          <span class="sub">{!onTrocarConta ? m.conta_mesma_so_claude()
            : contaBloqueada ? m.modo_so_ociosa() : m.conta_mesma_conversa_sub()}</span>
        </span>
        <span class="chev" aria-hidden="true">›</span>
      </button>
    </div>
  {:else if view === 'accounts'}
    <div class="more">
      <div class="more-head">
        <button class="back" onclick={() => (view = 'account')} aria-label={m.comum_voltar()}>‹</button>
        <h2 class="more-title">{m.conta_escolher()}</h2>
      </div>
      {#if accountsError}
        <p class="note error" role="alert">{m.conta_erro_listar({ erro: accountsError })}</p>
      {:else if accounts === null}
        <p class="note">{m.comum_carregando()}</p>
      {:else if accounts.length === 0}
        <p class="note">{m.conta_nenhuma_outra()}</p>
      {:else}
        {#each accounts as conta (conta.path)}
          <!-- Conta perto do limite aparece, mas não aceita a conversa: o primeiro turno reenvia o contexto inteiro. -->
          <button class="item" onclick={() => pick(() => onTrocarConta?.(conta))} disabled={contaBloqueada || conta.full}>
            <span class="txt">
              <span class="label">{conta.label}</span>
              <span class="sub">{conta.pct === null ? m.conta_cota_sem_leitura()
                : conta.full ? m.conta_cota_cheia({ pct: String(Math.round(conta.pct)) })
                : conta.low ? m.conta_cota_acabando({ pct: String(Math.round(conta.pct)) })
                : m.conta_cota_uso({ pct: String(Math.round(conta.pct)) })}</span>
            </span>
            <span class="chev" aria-hidden="true">›</span>
          </button>
        {/each}
      {/if}
    </div>
  {:else}
  <div class="more">
    <h2 class="more-title">{m.navbar_mais_acoes()}</h2>

    <!-- Atalhos customizados em seção própria, em blocos que quebram linha: separados das ações
         fixas abaixo, como no painel do desktop e no app nativo. -->
    {#if (shortcuts.length || projectError) && onShortcut}
      <div class="more-acoes">
        <ShortcutTiles {shortcuts} {projectName} {projectError} {projectKey} {sessionName} {hangarOf} {sessionTerminal}
                       onShortcut={(s) => pick(() => onShortcut?.(s))}
                       onAdd={onEditShortcuts ? () => pick(onEditShortcuts) : undefined}>
          {#snippet extra()}<ShortcutTransfer compact />{/snippet}
        </ShortcutTiles>
      </div>
    {/if}

    {#if onRun}
      <button class="item" onclick={() => onRun && pick(onRun)}>
        <span class="ico" class:on={runRunning} aria-hidden="true">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor">
            {#if runRunning}<rect x="6" y="6" width="12" height="12" rx="2" />{:else}<path d="M8 5v14l11-7z" />{/if}
          </svg>
        </span>
        <span class="txt">
          <span class="label">{runRunning ? m.ctx_rodando() : m.ctx_rodar_projeto()}</span>
          <span class="sub">{runRunning ? m.more_abrir_saida_processo() : m.more_detecta_comando_repo()}</span>
        </span>
        {#if runRunning}<span class="pill on">{m.servidor_ativo()}</span>{/if}
        <span class="chev" aria-hidden="true">›</span>
      </button>
    {/if}

    <button class="item" onclick={() => onActivity && pick(onActivity)} disabled={!onActivity}>
      <span class="ico" class:on={activityRunning} aria-hidden="true">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="3 5 4.5 6.5 7 4" /><polyline points="3 11.5 4.5 13 7 10.5" />
          <line x1="10" y1="5.5" x2="20" y2="5.5" /><line x1="10" y1="12" x2="20" y2="12" /><line x1="10" y1="18.5" x2="20" y2="18.5" />
        </svg>
      </span>
      <span class="txt">
        <span class="label">{m.ctx_atividade()}</span>
        <span class="sub">{onActivity ? m.more_tarefas_agentes() : m.more_nada_rodando()}</span>
      </span>
      <!-- Hoje activityBadge > 0 implica hasActivity (ver lib/activity.ts), mas o componente nao
           depende disso: sem o guard, um contador aceso num item cinza dizendo "Nada rodando agora"
           seria uma contradicao sem explicacao pro usuario. -->
      {#if activityBadge > 0 && onActivity}<span class="pill">{activityBadge}</span>{/if}
      <span class="chev" aria-hidden="true">›</span>
    </button>

    <button class="item" onclick={() => pick(onAttachments)}>
      <span class="ico" aria-hidden="true">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M21 11l-8.5 8.5a5 5 0 0 1-7-7L14 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L16 6"/>
        </svg>
      </span>
      <span class="txt">
        <span class="label">{m.ctx_anexos()}</span>
        <span class="sub">{m.more_fotos_videos_arquivos()}</span>
      </span>
      <span class="chev" aria-hidden="true">›</span>
    </button>

    <button class="item" onclick={() => (view = 'account')}>
      <span class="ico" aria-hidden="true">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 12h10" /><polyline points="10 8 14 12 10 16" /><path d="M19 4v16" />
        </svg>
      </span>
      <span class="txt">
        <span class="label">{m.bastao_menu()}</span>
        <span class="sub">{m.conta_continuar_sub()}</span>
      </span>
      <span class="chev" aria-hidden="true">›</span>
    </button>

    {#if onShare}
      <button class="item" onclick={() => onShare && pick(onShare)}>
        <span class="ico" aria-hidden="true">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M10 14a4 4 0 0 0 5.66 0l3-3a4 4 0 0 0-5.66-5.66l-1 1"/><path d="M14 10a4 4 0 0 0-5.66 0l-3 3a4 4 0 0 0 5.66 5.66l1-1"/>
          </svg>
        </span>
        <span class="txt">
          <span class="label">{m.compartilhar_menu()}</span>
          <span class="sub">{m.compartilhar_sub()}</span>
        </span>
        <span class="chev" aria-hidden="true">›</span>
      </button>
    {/if}

    {#if onTrocarModo}
      <button class="item" onclick={() => pick(onTrocarModo)} disabled={modoBloqueado}>
        <span class="ico" aria-hidden="true">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M4 8h13l-3-3" /><path d="M20 16H7l3 3" />
          </svg>
        </span>
        <span class="txt">
          <span class="label">{modoDestinoTerminal ? m.modo_abrir_no_terminal() : m.modo_continuar_sem_terminal()}</span>
          <span class="sub">{modoBloqueado ? m.modo_so_ociosa()
            : modoDestinoTerminal ? m.modo_abrir_no_terminal_detalhe() : m.modo_continuar_sem_terminal_detalhe()}</span>
        </span>
        <span class="chev" aria-hidden="true">›</span>
      </button>
    {/if}

    {#if onAlternarMods}
      <button class="item" onclick={() => onAlternarMods && pick(onAlternarMods)}>
        <span class="ico" class:on={modsLigado} aria-hidden="true">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="4" width="18" height="16" rx="2" /><path d="M3 9h18" />
          </svg>
        </span>
        <span class="txt">
          <span class="label">{rotuloModsCelular(modsLigado, modsPaineis)}</span>
          <span class="sub">{m.mods_celular_sub()}</span>
        </span>
      </button>
    {/if}

    {#if onRecarregar}
      <button class="item" onclick={() => pick(onRecarregar)} disabled={recarregarBloqueado}>
        <span class="ico" aria-hidden="true">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12a9 9 0 1 1-2.6-6.4" /><path d="M21 3v6h-6" />
          </svg>
        </span>
        <span class="txt">
          <span class="label">{m.recarregar_sessao()}</span>
          <span class="sub">{recarregarBloqueado ? m.modo_so_ociosa() : m.recarregar_sessao_detalhe()}</span>
        </span>
        <span class="chev" aria-hidden="true">›</span>
      </button>
    {/if}
  </div>
  {/if}
</BottomSheet>

<style>
  .more-acoes { padding: var(--space-1) 0 var(--space-3); margin-bottom: var(--space-2); border-bottom: 1px solid var(--border-subtle); }
  .more {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    /* A BottomSheet já dá a margem lateral; somar outra estreitava os itens. */
    padding: var(--space-2) 0 var(--space-5);
  }
  .more-title {
    margin: 0 0 var(--space-2);
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
  }
  .more-head { display: flex; align-items: center; gap: var(--space-2); margin-bottom: var(--space-2); }
  .more-head .more-title { margin-bottom: 0; }
  .back {
    width: 44px; height: 44px; flex-shrink: 0;
    display: inline-flex; align-items: center; justify-content: center;
    background: transparent; color: var(--text-secondary); font-size: var(--text-lg);
    border-radius: var(--radius-sm);
  }
  .note { margin: var(--space-2); font-size: var(--text-sm); color: var(--text-muted); }
  .note.error { color: var(--error); }
  .item {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    min-height: 56px;
    padding: var(--space-2) var(--space-2);
    background: transparent;
    border-radius: var(--radius-md);
    text-align: left;
    -webkit-tap-highlight-color: transparent;
  }
  .item:active { background: var(--bg-hover); }
  .item:disabled { opacity: 0.45; }
  .ico {
    width: 36px;
    height: 36px;
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-sm);
    background: var(--surface-raised);
    color: var(--text-secondary);
  }
  .ico.on { color: var(--accent); background: var(--accent-dim); }
  .txt {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }
  .label { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .sub {
    font-size: var(--text-xs);
    color: var(--text-muted);
    /* Duas linhas: cortado em uma, o subtítulo perdia justo a parte que explica a ação. */
    white-space: normal;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .pill {
    flex-shrink: 0;
    font-size: 11px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    color: var(--text-secondary);
    background: var(--surface-raised);
  }
  .pill.on { color: var(--accent); background: var(--accent-dim); }
  .chev { flex-shrink: 0; color: var(--text-muted); font-size: var(--text-lg); line-height: 1; }
</style>
