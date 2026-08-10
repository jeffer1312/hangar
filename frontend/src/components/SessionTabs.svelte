<script lang="ts">
  // Abas horizontais no topo (Task 6): quando a sidebar está recolhida, a navegação de sessões
  // vira uma faixa de abas. Só a <aside> some — os workflows pesados (criar, menu de contexto,
  // kebab) continuam na Sidebar montada, chamados via sidebarBridge.
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { buildSessionTabs } from '../lib/sessionTabs';
  import { stateColors } from '../lib/format';
  import { planBadge } from '../lib/plan';
  import { sidebarPin } from '../lib/sidebarPin.svelte';
  import { sidebarBridge } from '../lib/sidebarBridge';
  import HangarMark from './icons/HangarMark.svelte';
  import type { AggSession } from '../lib/types';

  interface Props {
    currentKey: string | null;
    onSelect: (session: AggSession) => void;
    onOpenConfig: () => void;
  }
  let { currentKey, onSelect, onOpenConfig }: Props = $props();

  // Sem retain()/release(): DesktopShell é o owner do store (refcount do singleton SSE).
  const model = $derived(buildSessionTabs(sessionsStore.byServer));

  const keyOf = (s: AggSession) => `${s.serverId}::${s.name}`;

  function expand() {
    sidebarPin.setUser(false);
  }

  // Tabs de ROLE button já respondem a Enter/Space nativamente; as setas movem o FOCO entre elas
  // (padrão tablist). Wrap nas pontas.
  function onStripKey(e: KeyboardEvent) {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    const tabs = Array.from(document.querySelectorAll<HTMLButtonElement>('.tabs-strip [role="tab"]'));
    const current = tabs.indexOf(document.activeElement as HTMLButtonElement);
    if (current < 0) return;
    e.preventDefault();
    const delta = e.key === 'ArrowRight' ? 1 : -1;
    tabs[(current + delta + tabs.length) % tabs.length].focus();
  }

  // Aba ativa entra na viewport: quando o ativo muda (troca de sessão, remontagem da lista) ou o
  // conjunto de abas muda, re-loca a seleção. `nearest` evita pular quando já está visível.
  $effect(() => {
    void currentKey;
    void model.tabs.length;
    document.querySelector('[aria-selected="true"]')?.scrollIntoView({ inline: 'nearest', block: 'nearest' });
  });
</script>

<div class="tabs-bar">
  <button class="tab-expand" onclick={expand} aria-label="Expandir barra lateral" title="Expandir barra lateral">
    <HangarMark size={18} />
  </button>

  <div class="tabs-strip" role="tablist" aria-label="Sessões" tabindex="-1" onkeydown={onStripKey}>
    {#each model.tabs as tab (keyOf(tab.session))}
      {@const active = keyOf(tab.session) === currentKey}
      {@const badge = planBadge(tab.session)}
      <button class="tab" class:boundary={tab.boundary} class:active
        role="tab" aria-selected={active} tabindex={active ? 0 : -1}
        onclick={() => onSelect(tab.session)}
        oncontextmenu={(e) => {
          e.preventDefault();
          sidebarBridge.openSessionMenu(e, tab.session, tab.session.serverId);
        }}
        title={tab.session.name}>
        <span class="tab-dot" style:background={stateColors[tab.session.state]} aria-hidden="true"></span>
        <span class="tab-name">{tab.session.name}</span>
        {#if badge}
          <span class="tab-plan" style:width={`${badge.pct}%`} title={badge.title} aria-hidden="true"></span>
        {/if}
      </button>
    {/each}
  </div>

  {#if model.offlineLabels.length > 0}
    <span class="tab-offline" title={model.offlineLabels.join(', ')}
      aria-label={`${model.offlineLabels.length} servidor(es) offline: ${model.offlineLabels.join(', ')}`}>
      ⚠ {model.offlineLabels.length} offline
    </span>
  {/if}

  <button class="tab-action" onclick={() => sidebarBridge.openCreate()} aria-label="Nova sessão" title="Nova sessão">+</button>
  <button class="tab-action" onclick={(e) => sidebarBridge.openKebab(e)} aria-haspopup="menu" aria-label="Mais opções" title="Buscar, Arquivo, Custos, Agrupar">⋯</button>
  <button class="tab-action" onclick={onOpenConfig} aria-label="Configurações" title="Configurações">⚙</button>
</div>

<style>
  .tabs-bar {
    height: 44px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: 0 calc(var(--space-2) + var(--cp-wco-right)) 0 var(--space-2);
    background: transparent;
    border-bottom: 1px solid var(--border-subtle);
  }
  .tabs-strip {
    flex: 1;
    min-width: 0;
    display: flex;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .tabs-strip::-webkit-scrollbar { display: none; }
  .tab {
    position: relative;
    flex-shrink: 0;
    max-width: 200px;
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    padding: 0 var(--space-2);
    height: 32px;
    margin-block: 6px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-secondary);
    font-size: var(--text-sm);
    cursor: pointer;
    overflow: hidden;
  }
  .tab:hover { color: var(--text-primary); background: var(--bg-hover); }
  .tab.boundary { margin-left: var(--space-3); }
  .tab.active { background: var(--accent-dim); box-shadow: inset 0 -2px 0 var(--accent); color: var(--text-primary); }
  .tab-dot { flex-shrink: 0; width: 8px; height: 8px; border-radius: 50%; }
  .tab-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Filete do progresso do plano, na base da aba. Surface própria (camada fina por cima da aba),
     não fundo chapado — quem vaza é o véu, ver regra de transparência no CLAUDE.md. */
  .tab-plan {
    position: absolute;
    left: 0;
    bottom: 0;
    height: 2px;
    background: var(--surface-inset);
  }
  .tab-expand {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--accent);
    cursor: pointer;
  }
  .tab-expand:hover { background: var(--bg-hover); }
  .tab-offline {
    flex-shrink: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
    white-space: nowrap;
  }
  .tab-action {
    flex-shrink: 0;
    width: 28px;
    height: 28px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-secondary);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .tab-action:hover { color: var(--text-primary); background: var(--bg-hover); }
  /* PWA em window-controls-overlay: a faixa vira a área arrastável da janela; botões e faixa de
     abas seguem clicáveis. Mesmo padrão do NavBar desktop. */
  @media (display-mode: window-controls-overlay) {
    .tabs-bar { -webkit-app-region: drag; }
    .tabs-bar button, .tabs-strip { -webkit-app-region: no-drag; }
  }
</style>
