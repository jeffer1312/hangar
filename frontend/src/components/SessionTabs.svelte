<script lang="ts">
  // Abas horizontais no topo (Task 6): quando a sidebar está recolhida, a navegação de sessões
  // vira uma faixa de abas. Só a <aside> some — os workflows pesados (criar, menu de contexto,
  // kebab) continuam na Sidebar montada, chamados via sidebarBridge.
  import { onMount } from 'svelte';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { buildSessionTabs, focusedTabKey, tabKeyOf } from '../lib/sessionTabs';
  import { stateColors, stateLabels } from '../lib/format';
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

  // Roving tabindex: focusedKey é a aba que o USUÁRIO focou (Tab/setas); a ÚNICA com tabindex=0
  // é a focável da vez (focusedKey válido -> currentKey -> primeira). Tudo por refs locais — nada
  // de querySelector global: o app tem OUTRO tablist (TerminalPanel), e um seletor de documento
  // inteiro roubaria abas alheias no ArrowRight (medido no gate da Task 6).
  let stripEl = $state<HTMLDivElement | null>(null);
  // $state de propósito: o bind:this escreve por índice e o warning binding_property_non_reactive
  // some (e o array fica reativo pros handlers de foco/setas).
  let tabEls = $state<(HTMLButtonElement | null)[]>([]);
  let focusedKey = $state<string | null>(null);
  const focusableKey = $derived(focusedTabKey(model.tabs, currentKey, focusedKey));

  // Board/Canvas: a sidebar fica recolhida POR FORÇA (override temporário) — expandir ali é
  // clique morto que gravaria a preferência por baixo (o override vence e nada abre, mas o
  // pin do usuário mudava calado). Com override ativo o botão desabilita (tooltip explica);
  // a preferência só muda quando o usuário decide de verdade, sem override.
  const expandBlocked = $derived(sidebarPin.forcedOverride === true);
  function expand() {
    if (sidebarPin.forcedOverride === true) return;   // guard duplo: disabled + handler (teclado)
    sidebarPin.setUser(false);
  }

  // Tabs de ROLE button já respondem a Enter/Space nativamente; as setas movem o FOCO entre elas
  // (padrão tablist). Wrap nas pontas. O onfocus de cada aba atualiza `focusedKey`, então o
  // tabindex=0 migra junto com o foco.
  function onStripKey(e: KeyboardEvent) {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    const current = tabEls.indexOf(document.activeElement as HTMLButtonElement);
    if (current < 0 || tabEls.length === 0) return;
    e.preventDefault();
    const delta = e.key === 'ArrowRight' ? 1 : -1;
    const next = (current + delta + tabEls.length) % tabEls.length;
    tabEls[next]?.focus();
  }

  // Foco pós-rename (round 2): a Sidebar pede foco pra aba recriada (`${serverId}::${novoNome}`)
  // quando o rename do diálogo fecha — mas o modelo ainda tem o nome VELHO. Registramos o handler
  // e deixamos o foco PENDENTE: o $effect abaixo foca assim que o modelo refletir o novo nome (o
  // SSE re-emite e a aba antiga, keyed por nome, é substituída).
  let pendingFocusKey = $state<string | null>(null);
  onMount(() => sidebarBridge.registerTabFocus({
    focusTab: (key: string) => {
      const idx = model.tabs.findIndex((t) => tabKeyOf(t.session) === key);
      if (idx >= 0) tabEls[idx]?.focus();
      else pendingFocusKey = key;
    },
  }));

  // Aba ativa entra na viewport: quando a seleção muda (troca de sessão, remontagem da lista) ou o
  // conjunto de abas muda, re-loca a seleção. Só dentro do strip — `nearest` evita pular quando já
  // está visível. O mesmo efeito consome pendingFocusKey quando a aba recriada aparecer.
  $effect(() => {
    void currentKey;
    void model.tabs.length;
    tabEls.find((t) => t?.getAttribute('aria-selected') === 'true')?.scrollIntoView({ inline: 'nearest', block: 'nearest' });
    if (pendingFocusKey) {
      const idx = model.tabs.findIndex((t) => tabKeyOf(t.session) === pendingFocusKey);
      if (idx >= 0) {
        tabEls[idx]?.focus();
        pendingFocusKey = null;
      }
    }
  });
</script>

<div class="tabs-bar">
  <button class="tab-expand" onclick={expand} disabled={expandBlocked}
    aria-label={expandBlocked ? 'Barra recolhida no Quadro/Canvas' : 'Expandir barra lateral'}
    title={expandBlocked ? 'Quadro/Canvas recolhe a barra — expanda ao sair' : 'Expandir barra lateral'}>
    <HangarMark size={18} />
  </button>

  <div class="tabs-strip" role="tablist" aria-label="Sessões" tabindex="-1"
       bind:this={stripEl} onkeydown={onStripKey}>
    {#each model.tabs as tab, i (tabKeyOf(tab.session))}
      {@const key = tabKeyOf(tab.session)}
      {@const active = key === currentKey}
      {@const badge = planBadge(tab.session)}
      {@const stateName = stateLabels[tab.session.state]}
      {@const plano = badge ? ` · plano ${Math.round(badge.pct)}%${badge.complete ? ', concluído' : ''}` : ''}
      <button class="tab" class:boundary={tab.boundary} class:active
        role="tab" aria-selected={active}
        aria-label={`${tab.session.name} · ${stateName}${plano}`}
        tabindex={key === focusableKey ? 0 : -1}
        bind:this={tabEls[i]}
        onfocus={() => (focusedKey = key)}
        onclick={() => onSelect(tab.session)}
        oncontextmenu={(e) => {
          e.preventDefault();
          sidebarBridge.openSessionMenu(e, tab.session, tab.session.serverId);
        }}
        title={`${tab.session.name} · ${stateName}${plano}`}>
        <span class="tab-dot" style:background={stateColors[tab.session.state]} aria-hidden="true"></span>
        <span class="tab-name">{tab.session.name}</span>
        {#if badge}
          <span class="tab-plan" class:done={badge.complete} style:--pct={`${badge.pct}%`} title={badge.title} aria-hidden="true"></span>
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
  /* Ativo explícito: fundo accent + borda accent + filete inferior + semibold. O contraste com o
     hover (bg-hover, sem borda) tem que ser inequívoco — a primeira prova visual do gate mostrou
     o estado ativo confundível com hover/foco (achado analysis-01..04). */
  .tab.active {
    background: var(--accent-dim);
    border-color: var(--accent);
    box-shadow: inset 0 -2px 0 var(--accent);
    color: var(--text-primary);
    font-weight: 600;
  }
  .tab.active:hover { background: var(--accent-dim); }
  .tab-dot { flex-shrink: 0; width: 8px; height: 8px; border-radius: 50%; }
  .tab-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Filete do progresso do plano, na base da aba: TRILHO cheio (--border-default) + preenchimento
     proporcional via --pct (gradiente num elemento só) — 0% mostra o trilho inteiro e se distingue
     de "sem plano" (elemento nem existe). Parcial = --text-secondary (mesma família do anel do
     PlanRing); concluído = --success. Na aba ATIVA o filete sobe 2px pra não pintar por cima do
     sublinhado accent (round 7/2). */
  .tab-plan {
    --fill: var(--text-secondary);
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 2px;
    background:
      linear-gradient(to right, var(--fill) var(--pct), transparent var(--pct)) no-repeat,
      var(--border-default);
  }
  .tab.active .tab-plan { bottom: 2px; }
  .tab-plan.done { --fill: var(--success); }
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
  .tab-expand:hover:not(:disabled) { background: var(--bg-hover); }
  .tab-expand:disabled { color: var(--text-muted); cursor: default; opacity: 0.55; }
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
