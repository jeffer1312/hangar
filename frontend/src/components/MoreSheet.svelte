<script lang="ts">
  import * as m from '../paraglide/messages';
  import BottomSheet from './BottomSheet.svelte';

  // Acoes que saíram da NavBar do CELULAR pro menu "⋯". Elas custavam 80px fixos da barra e sao de
  // uso raro; o nome da sessao, que e a informacao mais disputada ali, chegava a "clau…". No desktop
  // a barra tem folga e os botoes continuam inline — o Chat so passa `onMenu` no mobile.
  interface Props {
    open: boolean;
    onClose: () => void;
    onRun: () => void;
    runRunning?: boolean;
    onActivity?: () => void;      // ausente = sessao sem atividade pra mostrar
    onAttachments: () => void;
    /** Passagem de bastão. Mora AQUI porque no celular a lista de sessões não tem menu por sessão
     *  (as ações dela são swipe no SessionCard) — o "⋯" do chat aberto é a única entrada. */
    onBastao: () => void;
    activityRunning?: boolean;
    activityBadge?: number;
  }
  let {
    open, onClose, onRun, runRunning = false,
    onActivity, activityRunning = false, activityBadge = 0, onAttachments, onBastao,
  }: Props = $props();

  function pick(fn: () => void) {
    onClose();
    fn();
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={m.navbar_mais_acoes()}>
  <div class="more">
    <h2 class="more-title">{m.navbar_mais_acoes()}</h2>

    <button class="item" onclick={() => pick(onRun)}>
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

    <button class="item" onclick={() => pick(onBastao)}>
      <span class="ico" aria-hidden="true">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 12h10" /><polyline points="10 8 14 12 10 16" /><path d="M19 4v16" />
        </svg>
      </span>
      <span class="txt">
        <span class="label">{m.bastao_menu()}</span>
        <span class="sub">{m.bastao_sub()}</span>
      </span>
      <span class="chev" aria-hidden="true">›</span>
    </button>
  </div>
</BottomSheet>

<style>
  .more {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-2) var(--space-4) var(--space-5);
  }
  .more-title {
    margin: 0 0 var(--space-2);
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
  }
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
    background: var(--bg-elevated);
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
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill {
    flex-shrink: 0;
    font-size: 11px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    color: var(--text-secondary);
    background: var(--bg-elevated);
  }
  .pill.on { color: var(--accent); background: var(--accent-dim); }
  .chev { flex-shrink: 0; color: var(--text-muted); font-size: var(--text-lg); line-height: 1; }
</style>
