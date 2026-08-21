<script lang="ts">
  // Abas horizontais no topo (Task 6): quando a sidebar está recolhida, a navegação de sessões
  // vira uma faixa de abas. Só a <aside> some — os workflows pesados (criar, menu de contexto,
  // kebab) continuam na Sidebar montada, chamados via sidebarBridge.
  import { onMount } from 'svelte';
import * as m from '../paraglide/messages';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { buildSessionTabs, focusedTabKey, tabKeyOf } from '../lib/sessionTabs';
  import { stateColors, rotuloEstado } from '../lib/format';
  import { planBadge } from '../lib/plan';
  import { sidebarPin } from '../lib/sidebarPin.svelte';
  import { sidebarBridge } from '../lib/sidebarBridge';
  import { ctxPanel, alternarCtxPanel } from '../lib/ctxPanel.svelte';
  import { navMode } from '../lib/navMode.svelte';
  import { getActiveId, serverColor } from '../lib/auth';
  import HangarMark from './icons/HangarMark.svelte';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
  import QuotaPill from './QuotaPill.svelte';
  import type { AggSession } from '../lib/types';

  interface Props {
    currentKey: string | null;
    onSelect: (session: AggSession) => void;
    onOpenConfig: () => void;
    /** Leva à aba Contas (a pílula de cota abre por lá) — construído pelo DesktopShell. */
    onIrParaContas: () => void;
    // Painel de contexto montado (o DesktopShell deriva: sessão aberta sem split, ou overlay).
    // false → botão desabilitado com tooltip (decisão do usuário) — sem painel não há o que
    // alternar. Default true: a barra fora do shell não perde o controle.
    ctxDisponivel?: boolean;
  }
  let { currentKey, onSelect, onOpenConfig, onIrParaContas, ctxDisponivel = true }: Props = $props();

  // Sem retain()/release(): DesktopShell é o owner do store (refcount do singleton SSE).
  const model = $derived(buildSessionTabs(sessionsStore.byServer));

  // Servidor ativo, só pro ponto colorido da engrenagem (ver o comentário no template).
  // `getActiveId` lê localStorage e NÃO é reativo, e `selectServer` (lib/auth.ts:440) só grava a
  // chave — NÃO chama `notifyChanged()`. Então `sessionsStore.servers` sozinho não bastava: abrir
  // uma aba de outro servidor trocava o ativo e o ponto continuava na cor do anterior até algo
  // MAIS (sessão criada/removida) reatribuir a lista.
  // Quem faz o ponto acompanhar é o `currentKey` (`<serverId>::<nome>`), que muda no MESMO clique.
  // Mesmo truque do `void currentKey` do foco, mais abaixo. Sem sessão aberta cai no id salvo.
  const servidorAtivo = $derived.by(() => {
    const lista = sessionsStore.servers;
    const id = currentKey?.split('::')[0] || getActiveId();
    return lista.find((s) => s.id === id) ?? lista[0] ?? null;
  });
  const corDoServidor = $derived(servidorAtivo ? serverColor(servidorAtivo.id) : 'var(--text-muted)');

  // Conta da sessão ABERTA (id do /api/cotas), pra pílula de cota mostrar o uso DELA em vez do
  // pior-geral — pedido do usuário. Sem sessão aberta (ou sessão kimi/pi sem motor): null, e a
  // pílula cai no smart.
  const contaAtiva = $derived(
    model.tabs.find((t) => tabKeyOf(t.session) === currentKey)?.session.conta ?? null,
  );

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
  // A MARCA alterna Abas <-> Trilho (pedido do usuário, 10/08/2026). Ela ficou sem função quando a
  // barra virou permanente: "expandir barra lateral" não quer dizer nada no modo abas, onde a barra
  // lateral está escondida de propósito. Como alternador ela ganha o papel que a posição sugere —
  // é o primeiro elemento da barra, do lado de onde a navegação mora.
  // Sair do modo abas RESTAURA a barra lateral (setUser(false)): sem isto, quem tivesse recolhido o
  // trilho antes de ir pras abas voltava pro trilho recolhido e parecia que o botão não fez nada.
  function alternarModo() {
    if (navMode.mode === 'tabs') {
      navMode.mode = 'rail';
      if (sidebarPin.forcedOverride !== true) sidebarPin.setUser(false);
    } else {
      navMode.mode = 'tabs';
    }
  }
  const rotuloModo = $derived(navMode.mode === 'tabs'
    ? m.tabs_mostrar_barra()
    : m.tabs_mostrar_abas());

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
  <button class="tab-expand" onclick={alternarModo} disabled={expandBlocked}
    aria-label={expandBlocked ? m.sessao_barra_recolhida_quadro() : rotuloModo}
    aria-pressed={navMode.mode === 'tabs'}
    title={expandBlocked ? m.sessao_quadro_recolhe() : rotuloModo}>
    <HangarMark size={18} />
  </button>

  <!-- A tira de sessões é a ÚNICA parte da barra que a configuração Trilho/Abas liga e desliga: a
       barra em si é permanente (ver DesktopShell). No modo trilho as sessões vivem na esquerda, e
       repeti-las aqui seria a mesma lista em dois lugares na mesma tela. O `tabs-vazio` abaixo
       ocupa o lugar dela pra que as ações continuem ancoradas à direita. -->
  {#if navMode.mode === 'tabs'}
  <div class="tabs-strip" role="tablist" aria-label={m.lista_titulo()} tabindex="-1"
       bind:this={stripEl} onkeydown={onStripKey}>
    {#each model.tabs as tab, i (tabKeyOf(tab.session))}
      {@const key = tabKeyOf(tab.session)}
      {@const active = key === currentKey}
      {@const badge = planBadge(tab.session)}
      {@const stateName = rotuloEstado(tab.session.state)}
      {@const plano = badge ? `${m.sessao_plano_pct({ n: Math.round(badge.pct) })}${badge.complete ? m.sessao_concluido() : ''}` : ''}
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
        <!-- Marca colorida do provider em TODA aba (referência: tab bar do super.engineering, onde
             cada agente carrega seu ícone). Diferente do prov-chip da lista, aqui o Claude também
             ganha glifo — pedido do usuário: na aba o ícone é o reconhecimento, não há texto. -->
        <ProviderGlyph provider={tab.session.provider} size={14} />
        <span class="tab-name">{tab.session.name}</span>
        {#if badge}
          <span class="tab-plan" class:done={badge.complete} style:--pct={`${badge.pct}%`} title={badge.title} aria-hidden="true"></span>
        {/if}
      </button>
    {/each}
  </div>
  {:else}
    <div class="tabs-vazio" aria-hidden="true"></div>
  {/if}

  {#if model.offlineLabels.length > 0}
    <span class="tab-offline" title={model.offlineLabels.join(', ')}
      aria-label={m.tabs_servidores_offline({ n: model.offlineLabels.length, lista: model.offlineLabels.join(', ') })}>
      ⚠ {m.tabs_offline_curto({ n: model.offlineLabels.length })}
    </span>
  {/if}

  <button class="tab-action" onclick={() => sidebarBridge.openCreate()} aria-label={m.sessao_nova()} title={m.sessao_nova()}>+</button>
  <button class="tab-action" onclick={(e) => sidebarBridge.openKebab(e)} aria-haspopup="menu" aria-label={m.tabs_mais_opcoes()} title={m.tabs_buscar_arquivo_custos()}>⋯</button>
  <!-- Pílula de cota (o medidor do super.engineering): entre o ⋯ e a engrenagem, no espaço morto
       da barra. Mostra a conta da sessão ativa (ou a pior, sem sessão); o clique abre o detalhe. -->
  <QuotaPill serverKey={currentKey ?? ''} {contaAtiva} {onIrParaContas} />
  <!-- O ponto colorido veio junto com a engrenagem quando ela saiu do rodapé do trilho: ele não é
       enfeite, é a única coisa na tela que diz EM QUAL SERVIDOR você está, na mesma cor que agrupa
       as sessões por servidor. Tirar a engrenagem de lá sem trazer o ponto perderia essa metade da
       informação sem ninguém notar. -->
  <button class="tab-action tab-config" onclick={onOpenConfig}
    aria-label={m.tabs_config_servidor({ n: servidorAtivo?.label ?? m.tabs_servidor() })}
    title={m.tabs_config_servidor({ n: servidorAtivo?.label ?? m.tabs_servidor() })}>
    ⚙
    <span class="tab-srv-dot" style:background={corDoServidor} aria-hidden="true"></span>
  </button>
  <!-- Toggle do painel de contexto (follow-up visual): vive no EXTREMO DIREITO da barra, como o
       OpenCode. MESMO ícone do .ctx-fold (painel dividido) e alterna o store nos dois sentidos.
       Sem painel montado: desabilitado com tooltip (decisão do usuário). -->
  <button class="tab-action tab-ctx" class:aberto={!ctxPanel.recolhido}
    onclick={alternarCtxPanel} disabled={!ctxDisponivel}
    aria-label={!ctxDisponivel ? m.ctx_sem_painel()
      : (ctxPanel.recolhido ? m.ctx_expandir_painel() : m.ctx_recolher_painel())}
    title={!ctxDisponivel ? m.ctx_sem_painel()
      : (ctxPanel.recolhido ? m.ctx_expandir_painel() : m.ctx_recolher_painel())}>
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <rect x="3" y="4" width="18" height="16" rx="2"/>
      <line x1="9" y1="4" x2="9" y2="20"/>
    </svg>
  </button>
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
  /* Aparência → Painéis → "Colados": a barra deixa de ser uma faixa vazia por cima de duas paredes
     e vira a MESMA parede — mesma superfície da sidebar e do painel de contexto, sem risco de
     separação entre elas. Em "Soltos" ela continua transparente de propósito: ali os painéis são
     cards flutuando, e uma faixa opaca no topo brigaria com o papel de parede que passa em volta
     deles. Mesma escolha de material da Sidebar (`--glass-panel`, e `--glass-bg` no liquid escuro,
     onde o panel só empilharia um segundo fundo e viraria parede chapada). */
  :global(html[data-panels='edge']) .tabs-bar {
    background: var(--glass-panel);
    border-bottom-color: transparent;
  }
  :global(html[data-panels='edge'][data-liquid][data-theme='dark']) .tabs-bar {
    background: var(--glass-bg);
  }

  .tabs-strip {
    flex: 1;
    min-width: 0;
    display: flex;
    overflow-x: auto;
    scrollbar-width: none;
  }
  /* Modo trilho: sem a tira, algo precisa empurrar as ações pro extremo direito — senão elas
     encostam no botão de expandir, à esquerda, e a barra fica com um bloco de ícones no canto
     errado. Mesmo `flex: 1` da tira, sem conteúdo. */
  .tabs-vazio { flex: 1; min-width: 0; }

  .tab-config { position: relative; }
  .tab-srv-dot {
    position: absolute;
    right: 3px;
    bottom: 3px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    /* Anel da cor da barra: sobre a engrenagem o ponto encostava no traço do ícone e virava borrão. */
    box-shadow: 0 0 0 1.5px var(--bg-base);
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
  /* Toggle do contexto: painel ABERTO = accent (mesmo vocabulário do .select-toggle-btn.active);
     sem contexto montado = esmaecido e inerte (decisão do usuário). */
  .tab-ctx.aberto { color: var(--accent); }
  .tab-action:disabled { color: var(--text-muted); opacity: 0.55; cursor: default; }
  .tab-action:disabled:hover { background: transparent; }
  /* PWA em window-controls-overlay: a faixa vira a área arrastável da janela; botões e faixa de
     abas seguem clicáveis. Mesmo padrão do NavBar desktop. */
  @media (display-mode: window-controls-overlay) {
    .tabs-bar { -webkit-app-region: drag; }
    .tabs-bar button, .tabs-strip { -webkit-app-region: no-drag; }
  }
</style>
