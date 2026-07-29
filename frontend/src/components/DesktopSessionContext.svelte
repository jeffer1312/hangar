<script lang="ts">
  import RateChips from './RateChips.svelte';
  import type { State } from '../lib/types';
  import type { StatusFields } from '../lib/statusline';
  import { stateColors, stateLabels, ctxWindow, providerName } from '../lib/format';

  interface Props {
    state: State;
    stateDetail?: string | null;
    status?: StatusFields | null;
    pairPeers?: string[] | null;
    serverLabel?: string;
    provider?: 'claude' | 'codex' | 'pi';
    // Nome da sessao aberta: com a NavBar escondida (>=1280px) ele some da tela — e no overlay
    // do board/canvas nem a lista lateral esta a vista. Vai no header do painel.
    sessionName?: string;
    // ── Acoes da NavBar ──────────────────────────────────────────────────────
    // No desktop LARGO (>=1280px, mesma breakpoint deste painel) o Chat esconde a NavBar — a
    // informacao dela ja vivia toda aqui — e as ACOES migram pra faixa sob o header. Todas
    // opcionais: sem handler, sem botao (mesma regra da NavBar).
    onOpenTerminal?: () => void;
    terminalAlert?: boolean;
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
    onLoopTap?: () => void;
    // Codex: tocar o provider abre os limites de uso (na NavBar era o badge tappavel).
    onProviderTap?: () => void;
    // Grupo pareado -> PairSheet (conversa do par, contrato compartilhado, split). A secao dizia
    // "2 sessoes pareadas" e parava ali; a tela do par ja existia, so nao tinha porta aqui.
    onOpenPair?: () => void;
    // Repositorio -> GitSheet do cwd. Mesmo caso: dado sem porta.
    onOpenGit?: () => void;
    // Abre a sessao do MEMBRO num modal (PairChatModal). So com UM par: com 2+ nao da pra escolher
    // por quem clicou, entao a secao segue abrindo a PairSheet, que tem o botao por membro.
    // `undefined` quando o Chat esta `nested` (dentro de um modal) — a guarda que evita modal
    // dentro de modal, e com ela SSE empilhado.
    onOpenPeerChat?: (peer: string) => void;
  }

  let {
    state, stateDetail = null, status = null, pairPeers = null,
    serverLabel = '', provider = 'claude', sessionName = '',
    onOpenTerminal = undefined, terminalAlert = false,
    onOpenRun = undefined, runRunning = false,
    onOpenAttachments = undefined,
    onOpenActivity = undefined, activityBadge = 0, activityRunning = false,
    onExpandUsage = undefined, limited = false, limitReset = null,
    working = false,
    loopLabel = null, loopColor = undefined, onLoopTap = undefined,
    onProviderTap = undefined, onOpenPair = undefined, onOpenGit = undefined,
    onOpenPeerChat = undefined,
  }: Props = $props();

  const hasActions = $derived(onOpenTerminal || onOpenRun || onOpenAttachments || onOpenActivity);
  // Atalho da secao Grupo: com UM par, tocar abre a sessao dele direto no modal. Com 2+ membros a
  // secao continua abrindo a PairSheet — la existe o botao por membro, e escolher por quem clicou
  // seria adivinhacao.
  const soloPeer = $derived(pairPeers?.length === 1 ? pairPeers[0] : null);
  // Atalho direto pro modal SO quando ha um par; a PairSheet (contrato, conversa do grupo, lado a
  // lado, sair) nunca perde a porta — vira um segundo botao "grupo" na mesma secao.
  const openPeer = $derived(soloPeer && onOpenPeerChat ? () => onOpenPeerChat(soloPeer) : null);
  const openGroup = $derived(openPeer ?? onOpenPair);
  // RateChips se auto-esconde quando nao tem dial nem banner; a SECAO precisa do mesmo guarda,
  // senao sobra um rotulo "Limites" orfao. isFinite como o known() do RateChips: uma statusline
  // custom que escreva NaN/Infinity nao abre a secao pra um corpo vazio.
  const _known = (pct: number | undefined) => typeof pct === 'number' && isFinite(pct);
  const hasRate = $derived(limited || _known(status?.fiveHourPct) || _known(status?.weeklyPct));

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
</script>

<aside class="session-context" aria-label="Contexto da sessão">
  {#if working}<div class="ctx-sweep" aria-hidden="true"></div>{/if}
  <header>
    <div class="ctx-heading">
      <!-- Sem kicker: "Contexto da sessão" repetia o que o painel inteiro e (e ja esta no
           aria-label do <aside>). O nome da sessao e o titulo real. -->
      {#if sessionName}<strong class="header-session" title={sessionName}>{sessionName}</strong>{/if}
      {#if stateDetail}<p class="header-detail">{stateDetail}</p>{/if}
    </div>
    <div class="header-right">
      <span class="state-chip header-state" style="color: {stateColors[state]}">{stateLabels[state]}</span>
      {#if loopLabel}
        <button type="button" class="loop-chip" style="color: {loopColor};" onclick={onLoopTap} aria-label="Loop da sessão: {loopLabel}">{loopLabel}</button>
      {/if}
    </div>
  </header>

  {#if hasActions}
    <div class="ctx-actions">
      {#if onOpenTerminal}
        <button class="ctx-action terminal-btn" class:alert={terminalAlert} onclick={onOpenTerminal} aria-label="Terminal (espelho da TUI)">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="2.5" y="4" width="19" height="16" rx="2"/>
            <path d="M6.5 9l3 3-3 3"/>
            <line x1="12.5" y1="15" x2="17" y2="15"/>
          </svg>
          <span>Terminal</span>
        </button>
      {/if}
      {#if onOpenRun}
        <button class="ctx-action run-btn" class:running={runRunning} onclick={onOpenRun}
                aria-label={runRunning ? 'Rodando (abrir)' : 'Rodar projeto'}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            {#if runRunning}
              <rect x="6" y="6" width="12" height="12" rx="2" />
            {:else}
              <path d="M8 5v14l11-7z" />
            {/if}
          </svg>
          <span>{runRunning ? 'Rodando' : 'Rodar'}</span>
        </button>
      {/if}
      {#if onOpenAttachments}
        <button class="ctx-action" onclick={onOpenAttachments} aria-label="Anexos da sessão">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M21 11l-8.5 8.5a5 5 0 0 1-7-7L14 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L16 6"/>
          </svg>
          <span>Anexos</span>
        </button>
      {/if}
      {#if onOpenActivity}
        <button class="ctx-action activity-btn" class:running={activityRunning} onclick={onOpenActivity} aria-label="Atividade">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="3 5 4.5 6.5 7 4"/>
            <polyline points="3 11.5 4.5 13 7 10.5"/>
            <line x1="10" y1="5.5" x2="20" y2="5.5"/>
            <line x1="10" y1="12" x2="20" y2="12"/>
            <line x1="10" y1="18.5" x2="20" y2="18.5"/>
          </svg>
          <span>Atividade</span>
          {#if activityBadge > 0}<span class="activity-badge">{activityBadge}</span>{/if}
        </button>
      {/if}
    </div>
  {/if}

  <!-- A secao "Estado" saiu: repetia o chip do header a 60px de distancia, mesma palavra e mesma
       cor. O detalhe e o chip do loop subiram pro header, que ja era o lugar do estado. -->

  <div class="ctx-scroll">
  <section class="sec-metric">
    <span class="section-label">Contexto</span>
    {#if status?.ctxPct != null}
      <!-- Mesma forma das barras de Limites: qualificador a esquerda, leitura a direita. Antes o
           Contexto punha o numero a esquerda e os Limites a direita — dois medidores irmaos com a
           coluna de leitura em lados opostos. -->
      <div class="metric-row">
        <span>
          {#if status.ctxUsed != null && status.ctxTotal}{ctxWindow(status.ctxUsed)} de {ctxWindow(status.ctxTotal)}{:else}{status.ctxTotal ? ctxWindow(status.ctxTotal) + ' tokens' : 'janela'}{/if}
        </span>
        <strong>{Math.round(status.ctxPct)}%</strong>
      </div>
      <div class="progress tone-{ctxTone}" aria-label={`${Math.round(status.ctxPct)}% do contexto usado`}>
        <span style:width={`${status.ctxPct}%`}></span>
      </div>
      {#if status.turnIn != null || status.turnOut != null}
        <!-- Tokens do ULTIMO turno: o dado ja vinha na statusline crua (💬 271k/590) e so a barra
             de percentual chegava aqui. E o que responde "por que o contexto pulou". -->
        <p class="turn-tokens">
          último turno: {ctxWindow(status.turnIn ?? 0)} entrada · {status.turnOut != null ? tokenShort(status.turnOut) : '—'} saída
        </p>
      {/if}
    {:else}
      <p>medição indisponível</p>
    {/if}
  </section>

  {#if hasRate}
    <section class="sec-metric">
      <span class="section-label">Limites</span>
      <RateChips {status} onExpand={onExpandUsage} {limited} {limitReset} variant="bars" />
    </section>
  {/if}

  <section class="sec-break">
    <span class="section-label">Grupo</span>
    {#if pairPeers?.length}
      {#if openGroup}
        <button type="button" class="sec-open" onclick={openGroup}
                aria-label={soloPeer && onOpenPeerChat ? `Abrir a sessão ${soloPeer} num modal` : `Abrir o par: ${pairPeers.join(', ')}`}>
          <span class="sec-open-body">
            <strong>🤝 {pairPeers.join(' · ')}</strong>
            <p>{soloPeer && onOpenPeerChat ? 'abrir a conversa dele' : `${pairPeers.length + 1} sessões pareadas`}</p>
          </span>
          <span class="sec-open-arrow" aria-hidden="true">›</span>
        </button>
        {#if openPeer && onOpenPair}
          <!-- O atalho abre a conversa do par; o grupo em si (contrato, conversa, lado a lado, sair)
               continua a um toque daqui. -->
          <button type="button" class="sec-side" onclick={onOpenPair} aria-label="Abrir o grupo pareado">
            ver grupo
          </button>
        {/if}
      {:else}
        <strong>🤝 {pairPeers.join(' · ')}</strong>
        <p>{pairPeers.length + 1} sessões pareadas</p>
      {/if}
    {:else if onOpenPair}
      <!-- Sem par, a secao era so a frase "sessao independente" — e justamente aqui que se pensa em
           parear. Mesmo alvo clicavel das outras secoes, abrindo a PairSheet no modo "Parear com
           sessao". -->
      <button type="button" class="sec-open" onclick={onOpenPair} aria-label="Parear com outra sessão">
        <span class="sec-open-body">
          <strong>sessão independente</strong>
          <p>parear com outra sessão</p>
        </span>
        <span class="sec-open-arrow" aria-hidden="true">›</span>
      </button>
    {:else}
      <p>sessão independente</p>
    {/if}
  </section>

  <section class="sec-break">
    <span class="section-label">Repositório</span>
    {#if status?.repo}
      {#if onOpenGit}
        <button type="button" class="sec-open" onclick={onOpenGit} aria-label="Abrir o git de {status.repo}">
          <span class="sec-open-body">
            <strong class="mono">{status.repo}</strong>
            <p class="mono">{status.branch ?? 'sem branch'}{status.dirty ? ' · alterações locais' : ''}</p>
          </span>
          <span class="sec-open-arrow" aria-hidden="true">›</span>
        </button>
      {:else}
        <strong class="mono">{status.repo}</strong>
        <p class="mono">{status.branch ?? 'sem branch'}{status.dirty ? ' · alterações locais' : ''}</p>
      {/if}
    {:else}
      <p>não detectado</p>
    {/if}
  </section>

  <section class="sec-break">
    <span class="section-label">Execução</span>
    {#if onProviderTap}
      <button type="button" class="provider-tap" onclick={onProviderTap} aria-label="Limites de uso do provider">
        {providerName(provider)}
      </button>
    {:else}
      <strong>{providerName(provider)}</strong>
    {/if}
    {#if status?.model}<p>{status.model}{status.effort ? ` · ${status.effort}` : ''}</p>{/if}
    {#if serverLabel}<p>{serverLabel}</p>{/if}
  </section>
  </div>
</aside>

<style>
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
    border: 1px solid var(--border-default);
    border-radius: var(--radius-xl);
    box-shadow: 0 18px 44px rgba(0, 0, 0, 0.34);
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
     no elemento e o Chromium honrando, `--glass-panel` só empilhava um 2o fundo e matava a refração. */
  :global(html[data-liquid]) .session-context::before {
    background: var(--glass-bg);
  }

  /* Aparência → Painéis → "Colados": volta o painel de ponta a ponta, como era antes do card. */
  :global(html[data-panels='edge']) .session-context {
    --ctx-gap: 0px;
    border: 0;
    border-left: 1px solid var(--border-default);
    border-radius: 0;
    box-shadow: none;
  }

  header, .ctx-actions { flex: 0 0 auto; }
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
    padding: var(--space-2) var(--space-4);
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
    font-weight: 650;
    line-height: 1.25;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .header-state { flex-shrink: 0; }

  /* Faixa de acoes (ex-botoes da NavBar): icones sozinhos pediam memorizacao. Em 264px o par
     icone+rotulo cabe em duas colunas sem virar grade de cards. */
  .ctx-actions {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-2);
    padding: var(--space-3) var(--space-4);
    border-bottom: 1px solid var(--border-subtle);
  }
  /* Numero IMPAR de acoes (o botao Atividade so existe as vezes): o ultimo ficava pendurado numa
     coluna com um buraco do lado. Ele passa a ocupar a linha inteira, entao a faixa fecha em bloco
     em vez de terminar num degrau. */
  .ctx-action:last-child:nth-child(odd) { grid-column: 1 / -1; }

  .ctx-action {
    position: relative;
    min-width: 0;
    min-height: 44px;
    display: inline-flex;
    align-items: center;
    justify-content: flex-start;
    gap: var(--space-2);
    padding: 0 var(--space-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: var(--bg-base);
    color: var(--text-secondary);
    font-size: var(--text-xs);
    font-weight: 600;
    transition: background 180ms var(--ease-out), border-color 180ms var(--ease-out), color 180ms var(--ease-out);
  }
  .ctx-action:hover {
    background: var(--bg-hover);
    border-color: var(--border-default);
    color: var(--text-primary);
  }
  .ctx-action:active { background: var(--bg-hover); }
  .ctx-action svg { flex-shrink: 0; }
  .ctx-action span:not(.activity-badge) {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
    font-size: 10px;
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

  /* Ritmo: seis secoes com o MESMO padding e uma hairline cada viravam uma escada monotona. As
     medidas (contexto, limites) andam juntas e sem regua entre elas; a regua so aparece onde o
     assunto muda de fato — medidas | grupo | ambiente. */
  section {
    margin: 0 var(--space-4);
    padding: var(--space-4) 0 var(--space-3);
  }

  .sec-metric + .sec-metric { padding-top: var(--space-3); }
  .sec-break {
    padding-top: var(--space-4);
    border-top: 1px solid var(--border-subtle);
  }

  .section-label {
    display: block;
    margin-bottom: var(--space-2);
    color: var(--text-muted);
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .state-chip {
    display: inline-flex;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    background: var(--bg-elevated);
    font-size: var(--text-xs);
    font-weight: 650;
  }

  /* Chip do loop (🔁 N/M): mono como os badges numericos; cor vem do tone via style inline. */
  .loop-chip {
    margin-left: var(--space-1);
    font-family: var(--font-mono); font-size: var(--text-xs); font-weight: 600;
    padding: 2px 8px; border-radius: var(--radius-full);
    background: var(--bg-surface); border: 1px solid var(--border-subtle); cursor: pointer;
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
    font-weight: 650;
  }

  .progress {
    height: 4px;
    margin-top: var(--space-3);
    overflow: hidden;
    border-radius: var(--radius-full);
    background: var(--bg-elevated);
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
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  @media (max-width: 1279px) {
    .session-context { display: none; }
  }
</style>
