<script module lang="ts">
  // Aba escolhida POR SESSÃO. `ctxPanel.aba` é uma só pro app inteiro — sem esta memória, passar
  // por uma sessão sem navegador zerava a aba (o guard logo abaixo) e a sessão de origem voltava
  // em Contexto, com o navegador vivo e escondido atrás dela.
  //
  // Mora no módulo, e não na instância, porque este painel NÃO remonta na troca de sessão: só o
  // `navChave` muda. Foi o que derrubou a primeira tentativa, feita com onMount/onDestroy — ela
  // nunca reexecutava, e a aba continuava se perdendo.
  const ABA_POR_SESSAO = new Map<string, 'contexto' | 'arquivos' | 'navegador'>();
</script>

<script lang="ts">
  import { ctxPanel, alternarCtxPanel, arrastarLargura, salvarLargura } from '../lib/ctxPanel.svelte';
  import { navegadorPanel, arrastarNav, salvarNav } from '../lib/navegadorPanel.svelte';
  import { workspaceSessionKey } from '../lib/workspaceCommands';
  import NavegadorPane from './NavegadorPane.svelte';
import * as m from '../paraglide/messages';
import GroupGlyph from './icons/GroupGlyph.svelte';
  import HangarWorking from './icons/HangarWorking.svelte';
  import RateChips from './RateChips.svelte';
  import PlanPanel from './PlanPanel.svelte';
  import PlanRing from './PlanRing.svelte';
  import FilesPanel from './files/FilesPanel.svelte';
  import StateChip from './StateChip.svelte';
  import type { Provider, State, SessionInfo, PlanDetail, ChatEvent } from '../lib/types';
  import type { StatusFields } from '../lib/statusline';
  import { ctxWindow, providerName } from '../lib/format';
  import { planBadge } from '../lib/plan';

  interface Props {
    state: State;
    stateDetail?: string | null;
    status?: StatusFields | null;
    pairPeers?: string[] | null;
    serverLabel?: string;
    provider?: Provider;
    // Identidade do servidor (B2 do parecer): o FilesPanel chaveia o store por
    // serverId::sessionName; o Chat passa o MESMO getActiveId que ele usa, nunca calculado
    // diferente por caller.
    serverId?: string;
    // Nome da sessao aberta: com a NavBar escondida (>=1280px) ele some da tela — e no overlay
    // do board/canvas nem a lista lateral esta a vista. Vai no header do painel.
    sessionName?: string;
    // Progresso do plano (Task 5b): a SessionInfo completa (o PlanPanel/PlanBar leem plan_* dela)
    // + o detalhe de Task/Step, buscado à parte pelo Chat.
    session?: SessionInfo | null;
    planDetail?: PlanDetail | null;
    planLoading?: boolean;
    planError?: boolean;
    // ── Acoes da NavBar ──────────────────────────────────────────────────────
    // No desktop LARGO (>=1280px, mesma breakpoint deste painel) o Chat esconde a NavBar — a
    // informacao dela ja vivia toda aqui — e as ACOES migram pra faixa sob o header. Todas
    // opcionais: sem handler, sem botao (mesma regra da NavBar).
    onOpenTerminal?: () => void;
    terminalAlert?: boolean;
    // Navegador embutido: o botão na fileira de ações ATIVA a aba (criando o navegador da sessão
    // se não tem). A aba Navegador na tab bar só EXISTE quando a sessão tem navegador aberto.
    onOpenNavegador?: () => void;
    onOpenRun?: () => void;
    runRunning?: boolean;
    onOpenAttachments?: () => void;
    onOpenActivity?: () => void;
    activityBadge?: number;
    activityRunning?: boolean;
    onExpandUsage?: () => void;
    limited?: boolean;
    limitReset?: string | null;
    // Sinal "rodando": hairline varrendo o topo do painel (a work-sweep saiu da NavBar junto).
    working?: boolean;
    // Chip do loop (🔁 N/M) — morava na NavBar; aqui vai junto do estado.
    loopLabel?: string | null;
    loopColor?: string;
    // Follow-up visual: existe um toggle do painel FORA dele (barra de abas no modo 'tabs' OU
    // rodapé do rail no modo 'rail' com a sidebar recolhida) — este painel NÃO renderiza o próprio
    // .ctx-fold (sem duplicação) e, recolhido, SOME em vez de virar aba vertical. Sem toggle
    // externo (sidebar expandida), a porta acessível do painel é preservada (aba vertical).
    toggleExterno?: boolean;
    onLoopTap?: () => void;
    // Codex: tocar o provider abre os limites de uso (na NavBar era o badge tappavel).
    onProviderTap?: () => void;
    // Grupo pareado -> PairSheet (conversa do par, contrato compartilhado, split). A secao dizia
    // "2 sessoes pareadas" e parava ali; a tela do par ja existia, so nao tinha porta aqui.
    onOpenPair?: () => void;
    // Grupo -> modal Orquestração (quem roda cada papel, contas liberadas).
    onOpenOrq?: () => void;
    // Repositorio -> modal de git do cwd. Mesmo caso: dado sem porta.
    onOpenGit?: () => void;
    // Abre a sessao do MEMBRO num modal (PairChatModal). So com UM par: com 2+ nao da pra escolher
    // por quem clicou, entao a secao segue abrindo a PairSheet, que tem o botao por membro.
    // `undefined` quando o Chat esta `nested` (dentro de um modal) — a guarda que evita modal
    // dentro de modal, e com ela SSE empilhado.
    onOpenPeerChat?: (peer: string) => void;
    // Pra visão "Citados" da aba Arquivos (vêm do Chat).
    events?: ChatEvent[] | null;
    histGap?: string;
    cwd?: string | null;
  }

  let {
    state, stateDetail = null, status = null, pairPeers = null,
    events = null, histGap = '', cwd = null,
    serverLabel = '', provider = 'claude', sessionName = '', serverId = '',
    onOpenTerminal = undefined, terminalAlert = false,
    onOpenNavegador = undefined,
    onOpenRun = undefined, runRunning = false,
    onOpenAttachments = undefined,
    onOpenActivity = undefined, activityBadge = 0, activityRunning = false,
    onExpandUsage = undefined, limited = false, limitReset = null,
    working = false,
    loopLabel = null, loopColor = undefined, onLoopTap = undefined,
    onProviderTap = undefined, onOpenPair = undefined, onOpenOrq = undefined, onOpenGit = undefined,
    onOpenPeerChat = undefined,
    session = null, planDetail = null, planLoading = false, planError = false,
    toggleExterno = false,
  }: Props = $props();

  const hasActions = $derived(onOpenTerminal || onOpenNavegador || onOpenRun || onOpenAttachments || onOpenActivity);
  const navChave = $derived(workspaceSessionKey({ serverId, name: sessionName }));
  // A aba Navegador só existe na tab bar quando a sessão TEM navegador aberto (quem cria é o
  // botão da fileira ou o agente via hangar-preview open).
  const temNav = $derived(navChave in navegadorPanel.abertos);
  // Qual sessão este painel já viu. Por INSTÂNCIA, não no módulo: hoje só existe um painel montado
  // por vez (no split os Chat extras não recebem showContextPanel, e o overlay é ramo `:else if`),
  // mas com a marca no módulo dois painéis vivos brigariam — um deles nunca casaria a chave e
  // ficaria forçando a própria aba por cima da do irmão. O Map continua no módulo de propósito: ele
  // é a memória por sessão, e é ela que precisa sobreviver a uma remontagem.
  let chaveVista: string | null = null;
  // Mesma sessão: a aba de agora é a escolha dela, guarda. Sessão nova: devolve a aba em que ela
  // estava. Lê as duas coisas no topo de propósito — o efeito precisa acordar tanto na troca de
  // sessão quanto no clique de aba, e ler só dentro de um ramo perderia uma das duas.
  $effect(() => {
    const chave = navChave;
    const aba = ctxPanel.aba;
    if (chave === chaveVista) {
      ABA_POR_SESSAO.set(chave, aba);
      return;
    }
    chaveVista = chave;
    // Sessão que nunca escolheu aba mas já tem navegador (o agente abriu com ela fora da tela):
    // entra direto nele — é o que "o navegador está aberto lá quando eu for" quer dizer.
    const lembrada = ABA_POR_SESSAO.get(chave) ?? (temNav ? 'navegador' : 'contexto');
    if (lembrada !== aba) ctxPanel.aba = lembrada;   // reentra uma vez e cai no ramo de cima
  });
  // A aba é global (ctxPanel, por desenho) mas o navegador é POR SESSÃO: sessão sem navegador com
  // a aba ativa volta pra Contexto — senão a coluna fica em branco (os três painéis têm guard, e
  // header/fileira somem pelo mesmo motivo). Medido no app dele: troca de sessão com a aba ativa
  // deixava a coluna vazia.
  $effect(() => {
    if (ctxPanel.aba === 'navegador' && !temNav) ctxPanel.aba = 'contexto';
  });
  // Atalho da secao Grupo: com UM par, tocar abre a sessao dele direto no modal. Com 2+ membros a
  // secao continua abrindo a PairSheet — la existe o botao por membro, e escolher por quem clicou
  // seria adivinhacao.
  const soloPeer = $derived(pairPeers?.length === 1 ? pairPeers[0] : null);
  const planRing = $derived(session ? planBadge(session) : null);
  // Atalho direto pro modal SO quando ha um par; a PairSheet (contrato, conversa do grupo, lado a
  // lado, sair) nunca perde a porta — vira um segundo botao "grupo" na mesma secao.
  const openPeer = $derived(soloPeer && onOpenPeerChat ? () => onOpenPeerChat(soloPeer) : null);
  const openGroup = $derived(openPeer ?? onOpenPair);
  // RateChips se auto-esconde quando nao tem dial nem banner; a SECAO precisa do mesmo guarda,
  // senao sobra um rotulo "Limites" orfao. isFinite como o known() do RateChips: uma statusline
  // custom que escreva NaN/Infinity nao abre a secao pra um corpo vazio.
  const _known = (pct: number | undefined) => typeof pct === 'number' && isFinite(pct);
  const hasRate = $derived(limited || _known(status?.fiveHourPct) || _known(status?.weeklyPct) || _known(status?.monthlyPct));

  // Mesmos limiares do resto do app (RateChips, ContextRing): 70 ambar, 90 vermelho. Um vocabulario
  // so de medidor — a barra de contexto era a unica que ficava accent ate os 100%.
  const ctxTone = $derived.by(() => {
    const p = status?.ctxPct;
    if (typeof p !== 'number' || !isFinite(p)) return 'ok';
    return p >= 90 ? 'hot' : p >= 70 ? 'warn' : 'ok';
  });
  // Abaixo de 1k o ctxWindow arredondaria "590" pra "1k" (ele pensa em janelas de modelo, nao em
  // contagens pequenas do turno).
  function tokenShort(n: number): string {
    return n < 1000 ? String(Math.round(n)) : ctxWindow(n);
  }
  // ── Largura redimensionavel (drag na divisória da esquerda), persistida ─────────────
  // Mesma pegada da Sidebar (cp_sidebar_w): pointer capture no handle, largura clampsa no
  // store (arrastarLargura), salva no soltar. Sem transicao de width no painel, o arrasto
  // segue o ponteiro sem lag — a classe resizing fica como trava contra alguém adicionar
  // transicao depois (mesmo motivo do .sidebar.resizing). O flag `resizing` vive no store
  // (ctxPanel) porque o componente tem uma prop `state` e o rune `$state` colide com ela.
  function resizeStart(e: PointerEvent) {
    ctxPanel.resizing = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function resizeMove(e: PointerEvent) {
    if (!ctxPanel.resizing) return;
    // Com a aba Navegador ativa a divisória mexe na largura DELE (store próprio, teto próprio) —
    // a coluna engrossa pro browser; nas outras abas, a do contexto como sempre.
    if (ctxPanel.aba === 'navegador') arrastarNav(e.clientX);
    else arrastarLargura(e.clientX);
  }
  function resizeEnd() {
    if (!ctxPanel.resizing) return;
    ctxPanel.resizing = false;
    if (ctxPanel.aba === 'navegador') salvarNav();
    else salvarLargura();
  }
  // O flag vive no store (singleton de modulo) e sobrevive à desmontagem. Se a alca sair do DOM
  // no meio do arrasto — recolher, cruzar os 820px, trocar de sessao — o pointerup nao tem
  // destino e o resizing ficaria preso, fazendo a divisoria redimensionar so com o cursor por cima.
  // Dois bracos, porque sao dois mecanismos de saida: recolher NAO desmonta o componente (a alca
  // some por {#if} interno) — o flag zera quando recolhido vira true; os outros caminhos
  // (820px, troca de sessao) desmontam o componente inteiro — o cleanup zera na desmontagem.
  $effect(() => {
    if (ctxPanel.recolhido) ctxPanel.resizing = false;
    return () => { ctxPanel.resizing = false; };
  });
</script>

<aside class="session-context" class:recolhido={ctxPanel.recolhido} class:toggle-externo={toggleExterno} class:resizing={ctxPanel.resizing} aria-label={m.ctx_painel_titulo()}>
  {#if !ctxPanel.recolhido}
  <!-- Drag na divisória da esquerda pra redimensionar: mesma pegada do resize-handle da Sidebar
       (lá a borda é a direita, aqui a esquerda — o painel cola na direita). So quando aberto: o
       trilho recolhido não é largura, é estado. -->
  <div class="ctx-resize-handle" role="separator" aria-label={m.ctx_redimensionar()}
    aria-orientation="vertical"
    onpointerdown={resizeStart} onpointermove={resizeMove}
    onpointerup={resizeEnd} onpointercancel={resizeEnd}></div>
  {/if}
  <!-- Botão de recolher: com toggle externo (barra no modo tabs OU rail recolhido) o controle mora lá — aqui fica
       sem botão duplicado. Sem a barra, o botão do topo é a porta acessível dos dois sentidos. -->
  {#if !toggleExterno}
  <button class="ctx-fold" onclick={alternarCtxPanel}
          aria-label={ctxPanel.recolhido ? m.ctx_expandir_contexto() : m.ctx_recolher_contexto()}
          title={ctxPanel.recolhido ? m.sessao_expandir() : m.sessao_recolher()}>
    <!-- MESMO ícone do recolher da barra esquerda: os dois fazem a mesma coisa em lados opostos,
         então usar desenhos diferentes obrigava a reaprender o controle de cada lado. -->
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <rect x="3" y="4" width="18" height="16" rx="2"/>
      <line x1="9" y1="4" x2="9" y2="20"/>
    </svg>
  </button>
  {/if}

  {#if ctxPanel.recolhido}
    <!-- Recolhido = ESCONDIDO, não trilho. O trilho foi tentado e não se pagou: em 50px cabia só um
         texto vertical e um anel, enquanto o valor do painel é o plano, as ações e as métricas —
         coisas que precisam de largura. Estado e progresso continuam à vista na barra da esquerda. -->
  {:else}
  <!-- Na aba Navegador a varredura de "sessão trabalhando" some: em cima de uma página web ela
       lê como "a página está carregando" — e o carregamento de verdade tem barra própria lá. -->
  {#if working && ctxPanel.aba !== 'navegador'}<div class="ctx-sweep" aria-hidden="true"></div>{/if}
  <!-- Com a aba Navegador ativa o header some: o browser ganha a altura (pedido dele). -->
  {#if ctxPanel.aba !== 'navegador'}
  <header>
    <div class="ctx-heading">
      <!-- Sem kicker: "Contexto da sessão" repetia o que o painel inteiro e (e ja esta no
           aria-label do <aside>). O nome da sessao e o titulo real. -->
      {#if sessionName}<strong class="header-session" title={sessionName}>{sessionName}</strong>{/if}
      {#if stateDetail}<p class="header-detail">{stateDetail}</p>{/if}
    </div>
    <div class="header-right">
      <StateChip {state} size="md" />
      {#if loopLabel}
        <button type="button" class="loop-chip" style="color: {loopColor};" onclick={onLoopTap} aria-label={m.ctx_aria_loop({ n: loopLabel })}>{loopLabel}</button>
      {/if}
    </div>
  </header>
  {/if}

  <!-- Barra de abas do painel (Contexto | Arquivos), no desenho do mock aprovado. A aba ativa
       vive no ctxPanel (modulo): o App remonta este painel por {#key} a cada troca de sessao,
       e um $state local devolveria o usuario pra Contexto com Arquivos aberta. Recolhido, o
       painel some e a barra some junto — sem porta fantasma. -->
  <!-- Acoes da sessao (Terminal, Rodar, Anexos, Atividade) ficam ACIMA das abas: valem pra
       sessao inteira, nao pra aba Contexto — e ninguem devia trocar de aba pra achar o Terminal.
       Com a aba Navegador ativa a fileira SOME: quem ta ali ta mexendo no browser, e o browser
       ganha a altura. O Navegador nao e mais acao — e a aba ao lado. -->
  {#if hasActions && ctxPanel.aba !== 'navegador'}
    <div class="ctx-actions" role="toolbar" aria-label={m.ctx_painel_titulo()}>
      {#if onOpenTerminal}
        <button class="ctx-action terminal-btn" class:alert={terminalAlert} onclick={onOpenTerminal} aria-label={m.ctx_terminal()}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="2.5" y="4" width="19" height="16" rx="2"/>
            <path d="M6.5 9l3 3-3 3"/>
            <line x1="12.5" y1="15" x2="17" y2="15"/>
          </svg>
          <span>{m.ctx_terminal()}</span>
        </button>
      {/if}
      {#if onOpenNavegador}
        <button class="ctx-action" onclick={onOpenNavegador} aria-label={m.ctx_navegador()}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="12" cy="12" r="9"/>
            <path d="M3 12h18"/>
            <path d="M12 3c2.5 2.6 3.9 5.7 3.9 9s-1.4 6.4-3.9 9c-2.5-2.6-3.9-5.7-3.9-9s1.4-6.4 3.9-9z"/>
          </svg>
          <span>{m.ctx_navegador()}</span>
        </button>
      {/if}
      {#if onOpenRun}
        <button class="ctx-action run-btn" class:running={runRunning} onclick={onOpenRun}
                aria-label={runRunning ? m.ctx_rodando_abrir() : m.ctx_rodar_projeto()}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            {#if runRunning}
              <rect x="6" y="6" width="12" height="12" rx="2" />
            {:else}
              <path d="M8 5v14l11-7z" />
            {/if}
          </svg>
          <span>{runRunning ? m.ctx_rodando() : m.ctx_rodar()}</span>
        </button>
      {/if}
      {#if onOpenAttachments}
        <button class="ctx-action" onclick={onOpenAttachments} aria-label={m.ctx_anexos_da_sessao()}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M21 11l-8.5 8.5a5 5 0 0 1-7-7L14 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L16 6"/>
          </svg>
          <span>{m.ctx_anexos()}</span>
        </button>
      {/if}
      {#if onOpenActivity}
        <button class="ctx-action activity-btn" class:running={activityRunning} onclick={onOpenActivity} aria-label={m.ctx_atividade()}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="3 5 4.5 6.5 7 4"/>
            <polyline points="3 11.5 4.5 13 7 10.5"/>
            <line x1="10" y1="5.5" x2="20" y2="5.5"/>
            <line x1="10" y1="12" x2="20" y2="12"/>
            <line x1="10" y1="18.5" x2="20" y2="18.5"/>
          </svg>
          <span>{m.ctx_atividade()}</span>
          {#if activityBadge > 0}<span class="activity-badge">{activityBadge}</span>{/if}
        </button>
      {/if}
    </div>
  {/if}

  <div class="abas" role="tablist" aria-label={m.ctx_painel_titulo()}>
    <button type="button" id="aba-ctx-contexto" class="aba" class:sel={ctxPanel.aba === 'contexto'}
            role="tab" aria-selected={ctxPanel.aba === 'contexto'} aria-controls="painel-ctx-contexto"
            onclick={() => (ctxPanel.aba = 'contexto')}>
      {m.ctx_aba_contexto()}
    </button>
    <button type="button" id="aba-ctx-arquivos" class="aba" class:sel={ctxPanel.aba === 'arquivos'}
            role="tab" aria-selected={ctxPanel.aba === 'arquivos'} aria-controls="painel-ctx-arquivos"
            onclick={() => (ctxPanel.aba = 'arquivos')}>
      {m.arq_aba()}
    </button>
    {#if temNav}
    <button type="button" id="aba-ctx-navegador" class="aba" class:sel={ctxPanel.aba === 'navegador'}
            role="tab" aria-selected={ctxPanel.aba === 'navegador'} aria-controls="painel-ctx-navegador"
            onclick={() => (ctxPanel.aba = 'navegador')}>
      {m.ctx_navegador()}
    </button>
    {/if}
  </div>

  {#if ctxPanel.aba === 'navegador' && temNav}
  <!-- O navegador é uma ABA da coluna: trocar pra Contexto/Arquivos esconde o view (desmonta o
       painel -> nav-hide; o agente segue usando via CDP), nunca fecha. O × dele é quem fecha. -->
  <div id="painel-ctx-navegador" role="tabpanel" aria-labelledby="aba-ctx-navegador" class="ctx-tab ctx-tab-nav">
    <NavegadorPane navKey={navChave} />
  </div>
  {/if}

  {#if ctxPanel.aba === 'contexto'}
  <div id="painel-ctx-contexto" role="tabpanel" aria-labelledby="aba-ctx-contexto" class="ctx-tab">

  <!-- A secao "Estado" saiu: repetia o chip do header a 60px de distancia, mesma palavra e mesma
       cor. O detalhe e o chip do loop subiram pro header, que ja era o lugar do estado. -->

  <div class="ctx-scroll">
  <!-- SAUDE: Contexto + Limites num bloco so (eram duas secoes irmaos com a mesma cara).
       Vem primeiro: responde "esta tudo bem?" antes de qualquer detalhe. -->
  <section class="sec-metric">
    <span class="section-label">{m.ctx_saude()}</span>
    {#if status?.ctxPct != null}
      <div class="metric-row">
        <span>
          {m.ctx_contexto()} · {#if status.ctxUsed != null && status.ctxTotal}{m.ctx_usado_de_total({ usado: ctxWindow(status.ctxUsed), total: ctxWindow(status.ctxTotal) })}{:else}{status.ctxTotal ? `${ctxWindow(status.ctxTotal)} ${m.ctx_tokens()}` : m.ctx_janela()}{/if}
        </span>
        <strong>{Math.round(status.ctxPct)}%</strong>
      </div>
      <div class="progress tone-{ctxTone}" aria-label={m.ctx_pct_usado({ n: Math.round(status.ctxPct) })}>
        <span style:width={`${status.ctxPct}%`}></span>
      </div>
      {#if status.turnIn != null || status.turnOut != null}
        <p class="turn-tokens">
          {m.ctx_ultimo_turno()} {ctxWindow(status.turnIn ?? 0)} {m.ctx_entrada()} · {status.turnOut != null ? tokenShort(status.turnOut) : '—'} {m.ctx_saida()}
        </p>
      {/if}
    {:else}
      <p>{m.ctx_medicao_indisponivel()}</p>
    {/if}
    {#if hasRate}
      <div class="saude-limites">
        <RateChips {status} onExpand={onExpandUsage} {limited} {limitReset} variant="bars" />
      </div>
    {/if}
  </section>

  {#if session?.plan_name || session?.plan_hidden}
    <!-- Só quando ha plano ativo nesta sessao (Task 5b) — sem gate a secao apareceria vazia pra
         toda sessao sem superpowers rodando. plan_hidden entra junto: com "nenhum plano" escolhido
         o plan_name some, e o painel — que e onde fica o seletor pra voltar — sumiria com ele. -->
    <section class="sec-metric">
      <div class="section-head">
        <span class="section-label">{m.ctx_plano()}</span>
        {#if planRing}
          <span title={planRing.title}><PlanRing pct={planRing.pct} complete={planRing.complete} /></span>
        {/if}
      </div>
      <PlanPanel {session} detail={planDetail ?? null} loading={planLoading ?? false}
                 error={planError ?? false} />
    </section>
  {/if}

  {#if status?.repo}
  <section class="sec-break">
    <span class="section-label">{m.ctx_repositorio()}</span>
    {#if onOpenGit}
      <button type="button" class="sec-open" onclick={onOpenGit} aria-label={m.ctx_abrir_git({ n: status.repo })}>
        <span class="sec-open-body">
          <strong class="mono">{status.repo} · {status.branch ?? m.ctx_sem_branch()}</strong>
          {#if status.dirty}<p class="mono">{m.ctx_alteracoes_locais()}</p>{/if}
        </span>
        <span class="sec-open-arrow" aria-hidden="true">›</span>
      </button>
    {:else}
      <strong class="mono">{status.repo} · {status.branch ?? m.ctx_sem_branch()}</strong>
      {#if status.dirty}<p class="mono">{m.ctx_alteracoes_locais()}</p>{/if}
    {/if}
  </section>
  {/if}

  <!-- EQUIPE: Grupo + Orquestração fundidos. Sem par e sem porta de orquestração a seção nem
       existe — era um bloco morto pra quem raramente pareia. A linha de orquestração fica sempre
       que há onOpenOrq: é a única porta pro time padrão sem grupo. -->
  {#if pairPeers?.length || onOpenPair || onOpenOrq}
  <section class="sec-break">
    <span class="section-label">{m.ctx_equipe()}</span>
    {#if pairPeers?.length}
      {#if openGroup}
        <button type="button" class="sec-open" onclick={openGroup}
                aria-label={soloPeer && onOpenPeerChat ? m.ctx_abrir_sessao_modal({ n: soloPeer }) : m.ctx_abrir_par({ n: pairPeers.join(', ') })}>
          <span class="sec-open-body">
            <strong><GroupGlyph size={13} /> {pairPeers.join(' · ')}</strong>
            <p>{soloPeer && onOpenPeerChat ? m.ctx_abrir_conversa_dele() : `${pairPeers.length + 1} ${m.ctx_sessoes_pareadas()}`}</p>
          </span>
          <span class="sec-open-arrow" aria-hidden="true">›</span>
        </button>
        {#if openPeer && onOpenPair}
          <!-- O atalho abre a conversa do par; o grupo em si (contrato, conversa, lado a lado, sair)
               continua a um toque daqui. -->
          <button type="button" class="sec-side" onclick={onOpenPair} aria-label={m.ctx_abrir_grupo_pareado()}>
            {m.ctx_ver_grupo()}
          </button>
        {/if}
      {:else}
        <strong><GroupGlyph size={13} /> {pairPeers.join(' · ')}</strong>
        <p>{pairPeers.length + 1} {m.ctx_sessoes_pareadas()}</p>
      {/if}
    {:else if onOpenPair}
      <button type="button" class="sec-open" onclick={onOpenPair} aria-label={m.ctx_parear_outra()}>
        <span class="sec-open-body">
          <strong>{m.ctx_parear_outra()}</strong>
        </span>
        <span class="sec-open-arrow" aria-hidden="true">›</span>
      </button>
    {/if}
    {#if onOpenOrq}
      <button type="button" class="sec-open" onclick={onOpenOrq} aria-label={m.ctx_orquestracao()}>
        <span class="sec-open-body">
          <strong>{m.ctx_orquestracao_titulo()}</strong>
          <p>{m.ctx_orquestracao_desc()}</p>
        </span>
        <span class="sec-open-arrow" aria-hidden="true">›</span>
      </button>
    {/if}
  </section>
  {/if}

  <!-- EXECUÇÃO sem modelo/esforço: isso já está nas pills do composer (onde se troca). Aqui fica
       o que não está em mais nenhum lugar — provider e máquina. -->
  <section class="sec-break">
    <span class="section-label">{m.ctx_execucao()}</span>
    {#if onProviderTap}
      <button type="button" class="provider-tap" onclick={onProviderTap} aria-label={m.ctx_limites_provider()}>
        {providerName(provider)}{serverLabel ? ` · ${serverLabel}` : ''}
      </button>
    {:else}
      <strong>{providerName(provider)}{serverLabel ? ` · ${serverLabel}` : ''}</strong>
    {/if}
  </section>
  </div>
  </div>
  {:else if ctxPanel.aba === 'arquivos'}
  <div id="painel-ctx-arquivos" role="tabpanel" aria-labelledby="aba-ctx-arquivos" class="ctx-tab">
    <FilesPanel sessionName={sessionName} {serverId} desktop={true} {events} {histGap} {cwd} />
  </div>
  {/if}
  {/if}
</aside>

<style>
  /* O cabeçalho RESERVA a faixa do botão de recolher: sem isso ele cai por cima do chip de estado —
     `position: absolute` não empurra nada. 44px do botão + folga. */
  .session-context:not(.recolhido) header { padding-right: 56px; }
  .session-context {
    /* Coluna: header + acoes ficam PRESOS no topo e so o corpo rola. Antes o painel inteiro era o
       scroller, entao o nome da sessao e o botao Terminal subiam junto com as metricas. */
    display: flex;
    flex-direction: column;
    position: absolute;
    /* A navbar tem um fade visual abaixo da altura medida; começa depois dele para o título do
       painel não ficar sob o scrim (o conteúdo do chat pode rolar ali, um header fixo não).
       >=1280px a navbar some (Chat esconde) e o painel sobe pro topo. */
    top: calc(var(--nav-h, 56px) + var(--navbar-fade, 24px) + var(--ctx-gap));
    right: var(--ctx-gap);
    bottom: var(--ctx-gap);
    z-index: 17;
    /* CAIXA SOLTA, não parede colada na borda (mesma ideia do painel do Gemini no Gmail): folga em
       volta, cantos redondos e sombra, pra ler como uma seção à parte em vez de "o chat encolheu".
       A faixa reservada pelo Chat (`--ctx-w`, Chat.svelte:1287) continua a MESMA: a folga sai de
       dentro dela, então a coluna de mensagens não precisa saber que o painel virou card. */
    --ctx-gap: var(--space-3);
    width: calc(var(--ctx-w, 248px) - var(--ctx-gap));
    overflow: hidden;
    /* MESMA receita da sidebar e da faixa de cota (18/08): eram três acabamentos parecidos e
       nenhum igual — este usava --border-default (mais forte) e uma sombra SEM o brilho de borda
       que os outros dois tinham. A sombra agora é o token --elev-3, um lugar só. */
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-xl);
    box-shadow: var(--elev-3);
    background: transparent;   /* o fundo vai pro leaf ::before (vidro) */
  }

  /* Vidro do painel — mesmo material do composer/navbar/sidebar. Leaf ::before pra o
     backdrop-filter do Chromium nao virar containing block do conteudo. */
  .session-context::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: -1;
    pointer-events: none;
    border-radius: inherit;   /* sem isto o vidro vaza os cantos do card */
    background: var(--glass-bg-solid);
  }
  /* Mesmo motivo do BottomSheet: o filtro vai no ELEMENTO. Num pseudo dentro de um ancestral
     transformado o backdrop root muda e o blur nao pega nada — sobra so a transparencia, com o
     texto do chat atravessando o painel. O painel nao tem filho `position: fixed`, entao virar
     containing block aqui nao quebra nada. */
  :global(html[data-liquid]) .session-context {
    /* `url(#liquid-glass)` junto: sem ele o painel tinha só blur, e ao lado do composer (que refrata)
       parecia outro material. O filtro continua no ELEMENTO pelo motivo do bloco acima. */
    backdrop-filter: url(#liquid-glass) blur(20px) saturate(170%);
  }
  /* Mesmo vidro do composer sob liquid (Sidebar.svelte tem a mesma regra e o porquê): com o filtro
     no elemento e o Chromium honrando, `--glass-panel` só empilhava um 2o fundo e matava a refração.
     Escuro só — no claro 0.52 de branco não cobre texto escuro e o chat atravessa o painel. */
  :global(html[data-liquid]) .session-context::before {
    background: var(--glass-panel);
  }
  :global(html[data-liquid][data-theme='dark']) .session-context::before {
    background: var(--glass-bg);
  }

  /* Aparência → Painéis → "Colados": volta o painel de ponta a ponta, como era antes do card. */
  :global(html[data-panels='edge']) .session-context {
    --ctx-gap: 0px;
    border: 0;
    border-left: 1px solid var(--border-subtle);
    border-radius: 0;
    box-shadow: none;
  }

  /* Enquanto arrasta: sem transicao (segue o ponteiro sem lag). Hoje o painel nao tem
     transition de width; a regra trava contra alguem adicionar depois (mesmo motivo do
     .sidebar.resizing). */
  .session-context.resizing { transition: none; }
  .ctx-resize-handle {
    position: absolute; top: 0; left: 0; width: 6px; height: 100%;
    cursor: col-resize; z-index: 6; touch-action: none;
  }
  @media (hover: hover) {
    .ctx-resize-handle:hover { background: var(--accent-dim); }
  }

  header, .ctx-actions { flex: 0 0 auto; }
  /* Envoltorio de cada aba: o conteudo de contexto e o FilesPanel ocupam a mesma faixa
     flexivel abaixo das abas. O ctx-scroll (rolagem do contexto) fica dentro dele. */
  .ctx-tab {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  /* A aba Navegador deixa 8px à esquerda: o handle de redimensionar da coluna (absolute, left:0,
     6px) precisa ficar FORA do view nativo, senão o view cobre a divisória e o clique morre —
     era o "depois que abre uma página não dá pra redimensionar". */
  .ctx-tab-nav { margin-left: 8px; }
  .ctx-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  @media (min-width: 1280px) {
    /* Sem navbar, --nav-h carrega so o topInset (ex: faixa de atencao, 52px) — o painel comeca
       abaixo dela, nunca embaixo. */
    .session-context { top: calc(var(--nav-h, 0px) + var(--ctx-gap)); }
  }

  header {
    min-height: 64px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    /* A partir de 1280px com este painel aberto a NavBar some (Chat.svelte) e quem encosta na
       borda direita da janela é este header — logo é ele que abre a faixa dos controles da
       janela no PWA em window-controls-overlay. Zero fora desse modo. */
    padding: var(--space-2) calc(var(--space-4) + var(--cp-wco-right)) var(--space-2) var(--space-4);
    border-bottom: 1px solid var(--border-subtle);
    background: color-mix(in srgb, var(--bg-elevated) 52%, transparent);
  }

  .ctx-heading {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .header-right {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  /* Detalhe do estado ("Coalescing… 44s"): vive sob o nome, no lugar de onde o chip esta. */
  .header-detail {
    margin: 2px 0 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--text-xs);
    line-height: 1.3;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Nome da sessao vira o titulo real do painel: no desktop largo a NavBar some e ele some junto. */
  .header-session {
    min-width: 0;
    overflow: hidden;
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    font-weight: var(--fw-semibold);
    line-height: 1.25;
    text-overflow: ellipsis;
    white-space: nowrap;
  }


  /* Faixa de acoes (ex-botoes da NavBar): icones sozinhos pediam memorizacao. Em 264px o par
     icone+rotulo cabe em duas colunas sem virar grade de cards. */
  /* Barra de acoes: uma linha so, um bloco por acao (icone em cima, rotulo curto embaixo), dentro
     de uma unica superficie — le como toolbar do painel, nao como quatro cards. Quantas couberem
     (o Atividade so existe as vezes): auto-fit divide a linha por igual. */
  .ctx-actions {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(0, 1fr));
    gap: 2px;
    margin: 0 var(--space-4) var(--space-3);
    padding: 2px;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: var(--surface-inset);
  }

  .ctx-action {
    position: relative;
    min-width: 0;
    min-height: 50px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 3px;
    padding: 6px 2px 5px;
    border: 0;
    border-radius: calc(var(--radius-md) - 3px);
    background: transparent;
    color: var(--text-secondary);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.01em;
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out);
  }
  .ctx-action:hover { background: var(--surface-raised); color: var(--text-primary); }
  .ctx-action:active { background: var(--bg-hover); }
  .ctx-action:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  .ctx-action svg { flex-shrink: 0; width: 18px; height: 18px; }
  .ctx-action span:not(.activity-badge) {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1;
  }

  .terminal-btn { color: var(--text-secondary); }
  .terminal-btn.alert { color: var(--accent); }
  .terminal-btn.alert svg { animation: breathe 1.4s ease-in-out infinite; }

  .run-btn { position: relative; }
  .run-btn.running { color: var(--success); }
  .run-btn.running::after {
    content: ''; position: absolute; top: 6px; right: 6px; width: 6px; height: 6px;
    border-radius: 50%; background: var(--success); animation: pulse-scale 1.6s var(--ease-out) infinite;
  }

  .activity-btn { position: relative; color: var(--text-secondary); }
  .activity-btn.running { color: var(--accent); }
  .activity-btn.running svg { animation: breathe 1.5s ease-in-out infinite; }
  @keyframes breathe {
    0%, 100% { opacity: 0.55; transform: scale(0.92); }
    50%      { opacity: 1;    transform: scale(1.05); }
  }
  @media (prefers-reduced-motion: reduce) {
    .terminal-btn.alert svg, .activity-btn.running svg { animation: none; }
  }

  .activity-badge {
    position: absolute;
    top: 5px;
    right: 5px;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: var(--radius-full);
    background: var(--accent);
    color: var(--bg-base);   /* nunca #fff: o neutro do tema ja e quente e tem contraste no indigo */
    font-size: var(--text-3xs);
    font-weight: 600;
    line-height: 16px;
    text-align: center;
  }

  /* Turno ativo: hairline accent varrendo o TOPO do painel (a irma da work-sweep da NavBar).
     prefers-reduced-motion global (app.css) ja neutraliza o loop. */
  .ctx-sweep {
    position: sticky;
    top: 0;
    height: 2px;
    z-index: 1;
    background: linear-gradient(90deg, transparent, var(--accent), transparent);
    background-size: 50% 100%;
    background-repeat: no-repeat;
    animation: ctx-sweep 1.8s ease-in-out infinite;
  }
  @keyframes ctx-sweep {
    0%   { background-position: -60% 0; }
    100% { background-position: 160% 0; }
  }

  /* Barra de abas Contexto | Arquivos (desenho do mock aprovado). O padding lateral segue o
     resto do painel (--space-4), nao os 10px do mock — a aba nasce dentro de um card que ja
     alinha tudo em 16px, e 10px desalinharia com o header e as secoes (divergencia mock-vs-app
     resolvida a favor do app). */
  .abas {
    flex: 0 0 auto;
    display: flex;
    gap: 2px;
    padding: 0 var(--space-4);
    border-bottom: 1px solid var(--border-subtle);
  }
  .aba {
    appearance: none;
    background: none;
    border: 0;
    border-bottom: 2px solid transparent;
    color: var(--text-muted);
    font: inherit;
    font-size: var(--text-xs);
    padding: 9px 10px 8px;
    cursor: pointer;
    margin-bottom: -1px;
    /* Sobrescreve o alvo global de 44px (app.css) — a barra de abas e compacta por desenho
       (mock: ~34px de altura). */
    min-height: 0;
    min-width: 0;
  }
  .aba.sel {
    color: var(--text-primary);
    border-bottom-color: var(--accent);
  }
  .aba:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }

  /* Ritmo: seis secoes com o MESMO padding e uma hairline cada viravam uma escada monotona. As
     medidas (contexto, limites) andam juntas e sem regua entre elas; a regua so aparece onde o
     assunto muda de fato — medidas | grupo | ambiente. */
  section {
    margin: 0 var(--space-4);
    padding: var(--space-4) 0 var(--space-3);
  }

  .sec-metric + .sec-metric { padding-top: var(--space-3); }
  /* Limites dentro da seção Saúde: respiro entre a barra de contexto e as de cota, sem régua
     (continuam sendo o mesmo assunto). */
  .saude-limites { margin-top: var(--space-3); }
  .sec-break {
    padding-top: var(--space-4);
    border-top: 1px solid var(--border-subtle);
  }

  .section-label {
    display: block;
    margin-bottom: var(--space-2);
    color: var(--text-muted);
    /* Receita unificada de rotulo de secao (tokens de app.css) — era 10px/700/0.08em so aqui. */
    font-size: var(--label-size);
    font-weight: var(--label-weight);
    letter-spacing: var(--label-tracking);
    text-transform: uppercase;
  }

  /* Cabecalho de secao com anel na ponta (Plano): o rotulo nao pode herdar o margin-bottom do
     .section-label global — ele e o flex item da esquerda, quem respira e o .section-head. */
  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    margin-bottom: var(--space-2);
  }
  .section-head .section-label { margin-bottom: 0; }

  /* Chip do loop (🔁 N/M): mono como os badges numericos; cor vem do tone via style inline. */
  .loop-chip {
    margin-left: var(--space-1);
    font-family: var(--font-mono); font-size: var(--text-xs); font-weight: 600;
    padding: 2px 8px; border-radius: var(--radius-full);
    background: var(--surface-card); border: 1px solid var(--border-subtle); cursor: pointer;
  }
  .loop-chip:active { background: var(--bg-hover); }

  /* Provider tappavel (Codex -> limites): visual do <strong> irmao, comportamento de botao. */
  .provider-tap {
    display: block;
    width: 100%;
    padding: 0;
    text-align: left;
    overflow: hidden;
    color: var(--accent);
    font-size: var(--text-xs);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-radius: var(--radius-sm);
  }

  section strong {
    display: block;
    overflow: hidden;
    color: var(--text-secondary);
    font-size: var(--text-xs);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  p {
    margin: 3px 0 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
    line-height: 1.45;
  }

  .mono { font-family: var(--font-mono); }

  /* Secao que ABRE alguma coisa: o conteudo fica igual ao das secoes mudas (mesma tipografia, mesmo
     alinhamento) e o que muda e o alvo inteiro ficar clicavel, com um chevron discreto na direita.
     Sem caixa nem borda: virariam cards aninhados dentro do painel. */
  .sec-open {
    display: flex;
    min-height: 0;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    /* Sangra os --space-2 laterais pra fora da secao: o texto continua alinhado com o rotulo, e o
       realce do hover cobre a largura util do painel em vez de uma faixa recuada. */
    margin: calc(var(--space-1) * -1) calc(var(--space-2) * -1);
    padding: var(--space-1) var(--space-2);
    width: calc(100% + var(--space-4));
    border-radius: var(--radius-md);
    text-align: left;
    transition: background 160ms var(--ease-out);
  }
  .sec-open:hover { background: var(--bg-hover); }
  .sec-open:hover .sec-open-arrow { color: var(--text-secondary); transform: translateX(2px); }
  .sec-open-body { min-width: 0; }
  /* Acao secundaria da secao: peso de link, nao de botao — quem manda na linha e o atalho acima. */
  .sec-side {
    min-height: 0;
    min-width: 0;
    margin-top: var(--space-1);
    padding: 2px 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
    text-decoration: underline;
    text-underline-offset: 3px;
    text-decoration-color: var(--border-default);
  }
  .sec-side:hover { color: var(--text-secondary); }
  .sec-open-arrow {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--text-base);
    line-height: 1;
    transition: color 160ms var(--ease-out), transform 160ms var(--ease-out);
  }

  .metric-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
  }

  .metric-row strong {
    display: inline;
    color: var(--text-primary);
    font-weight: var(--fw-semibold);
    /* Percentual de limite/contexto muda com o turno — digito tabular nao danca. */
    font-variant-numeric: tabular-nums;
  }

  .progress {
    height: 4px;
    margin-top: var(--space-3);
    overflow: hidden;
    border-radius: var(--radius-full);
    background: var(--surface-raised);
  }

  .progress span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
    transition: width 400ms var(--ease-out), background 300ms ease;
  }
  .progress.tone-warn span { background: var(--warning); }
  .progress.tone-hot span { background: var(--error); }

  .turn-tokens {
    margin-top: var(--space-2);
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--text-2xs);
    font-variant-numeric: tabular-nums;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Botão de recolher: mesmo ícone, tamanho e peso do irmão na barra esquerda (.fold-btn) — os dois
     fazem a mesma coisa em lados opostos. O cabeçalho reserva a faixa dele (padding-right acima),
     senão ele cai por cima do chip de estado: absoluto não empurra nada. */
  .ctx-fold {
    position: absolute; top: var(--space-2); right: var(--space-2); z-index: 2;
    width: 36px; height: 36px; display: grid; place-items: center;
    background: transparent; border: 0; border-radius: var(--radius-md);
    color: var(--text-secondary); cursor: pointer;
  }
  @media (hover: hover) { .ctx-fold:hover { background: var(--bg-hover); color: var(--text-primary); } }

  /* RECOLHIDO = escondido, não trilho nem aba flutuante. Clarificação final do usuário: a aba
     vertical isolada no MEIO da borda direita (26×64, top:50%) morreu nos DOIS modos. Com toggle
     externo (barra no modo tabs OU rodapé do rail), o painel SOME de vez (display:none). Sem
     toggle externo (sidebar expandida), sobra uma PORTA DISCRETA no topo — o ctx-fold na posição
     do header (36px), não uma aba flutuante: clicar reexpande. */
  .session-context.recolhido {
    width: 36px;
    border-color: transparent;
    box-shadow: none;
    pointer-events: none;
  }
  .session-context.recolhido::before { opacity: 0; }
  .session-context.recolhido .ctx-fold {
    pointer-events: auto;
    position: static;
    width: 36px; height: 36px;
    margin: var(--space-2) auto 0;
    border-radius: var(--radius-md);
    background: var(--surface-raised);
    box-shadow: none;
  }
  .session-context.recolhido.toggle-externo {
    display: none;
    width: 0;
  }

  @media (max-width: 1279px) {
    .session-context { display: none; }
  }
</style>
