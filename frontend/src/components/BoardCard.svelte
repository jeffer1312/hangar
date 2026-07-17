<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import AssistantBubble from './AssistantBubble.svelte';
  import { getHistoryTailCached, getHistoryTailForServer, sendInputForServer, selectOptionForServer } from '../lib/api';
  import { relativeTime, bubblesFromTail, stateLabels } from '../lib/format';
  import { parseStatusLine } from '../lib/statusline';
  import type { Server } from '../lib/auth';
  import type { ChatEvent } from '../lib/types';
  import type { BoardRow, PendingMsg } from '../screens/Board.svelte';

  interface Props {
    session: BoardRow;
    server: Server;
    color: string;                    // cor do servidor (dot)
    // Estado IÇADO no Board. O quadro tem um {#each} POR COLUNA, então quando a sessão muda de estado
    // a row sai de um bloco e entra em outro — o Svelte DESTRÓI e recria este componente. Nada que
    // precise sobreviver ao envio (que é justamente o que muda o estado) pode morar aqui dentro.
    draft: string;
    onDraftChange: (t: string) => void;
    pending: PendingMsg[];
    // Updater (prev => next), não setter: o card pode ser destruído NO MEIO do próprio envio e o
    // callback segue apontando pro Map do Board. Reler a prop `pending` depois do await leria a de
    // uma instância morta; a função recebe sempre o valor atual da fonte da verdade.
    updatePending: (fn: (prev: PendingMsg[]) => PendingMsg[]) => void;
    sendError: string;
    onSendError: (m: string) => void;
    onOpen: () => void;               // abre o chat completo
    // Canvas: o card preenche o wrapper (a altura vem de fora). Board não passa → comportamento intacto.
    fill?: boolean;
  }
  let {
    session, server, color, draft, onDraftChange,
    pending, updatePending, sendError, onSendError, onOpen, fill = false,
  }: Props = $props();

  const TAIL = 15;
  let events = $state<ChatEvent[]>([]);
  let tailError = $state('');
  let loading = $state(true);
  let bodyEl = $state<HTMLElement>();
  // Semeia UMA vez com o rascunho içado; daqui pra frente o dono do texto é o card (untrack: reagir a
  // `draft` faria o Board reescrever por cima do que está sendo digitado).
  let text = $state(untrack(() => draft));
  // Rascunho içado: cada tecla persiste no Map do Board. Corpo em BLOCO de propósito — o
  // onDraftChange do Board é `(t) => drafts.set(...)`, que DEVOLVE o Map; num arrow de corpo-expressão
  // o Svelte leria esse retorno como teardown do efeito e quebraria (effect_returns_value).
  $effect(() => {
    onDraftChange(text);
  });

  // `seq` invalida o resultado de um fetch cuja instância já foi descartada (troca de coluna): sem
  // isto uma cauda de 8s resolve em estado morto e ainda ocupa uma das ~6 conexões/host — competindo
  // com a espiada do hover da Sidebar, que usa o MESMO endpoint.
  let seq = 0;
  async function loadTail() {
    const my = ++seq;
    // Retry re-arma o estado de base: `loading` volta a bloquear o loadMore concorrente (sem isto
    // as duas fetches dividiam o mesmo seq e a última a responder vencia calada) e a paginação é
    // descartada de propósito (tailLimit/atEnd casam com os events de base que vão entrar).
    loading = true;
    tailLimit = TAIL;
    atEnd = false;
    try {
      // Cache por serverId::name::state::last_activity (api.ts): reentrar na view não refaz as
      // dezenas de GET; flip de coluna OU atividade nova mudam a chave -> fetch fresco. `at` =
      // instante em que a cauda foi REALMENTE buscada (não o do hit): só ecos confirmados antes
      // disso podem se aposentar.
      const { evs, at } = await getHistoryTailCached(
        server, session.name, TAIL, session.state, session.last_activity,
      );
      if (my !== seq) return;
      events = evs;
      tailError = '';
      retirePending(at);
      requestAnimationFrame(() => bodyEl?.scrollTo({ top: bodyEl.scrollHeight }));
    } catch {
      if (my !== seq) return;
      // Falha por-requisição (timeout de 8s / 404 / 500) com o SSE do Board saudável NÃO acende o
      // banner de offline (que é por servidor inteiro): sem marcador o card ficava em branco, igual a
      // sessão vazia, e sem retry — a cauda só roda no mount.
      tailError = 'falha ao carregar — tocar pra tentar de novo';
    } finally {
      if (my === seq) loading = false;
    }
  }
  onMount(loadTail);

  // Paginação pra trás: rolar até o TOPO do corpo busca uma janela maior (a conversa inteira em
  // páginas de PAGE, não só a cauda de 15). Direto no getHistoryTailForServer (sem cache): o limit
  // varia e o backend faz tail-read barato. Âncora de leitura preservada via delta de scrollHeight.
  const PAGE = 50;
  let tailLimit = TAIL;
  let atEnd = false; // veio menos que o pedido = conversa inteira já está no card
  let loadingMore = $state(false);
  async function loadMore() {
    if (loadingMore || loading || atEnd || !bodyEl) return;
    loadingMore = true;
    const my = seq;
    const prevH = bodyEl.scrollHeight;
    const want = tailLimit + PAGE;
    const startedAt = Date.now();
    try {
      const evs = await getHistoryTailForServer(server, session.name, want);
      if (my !== seq) return;
      if (evs.length < want) atEnd = true;
      tailLimit = want;
      events = evs;
      tailError = '';
      // Mesmo contrato do loadTail: a janela nova contém ecos que o backend já confirmou antes do
      // fetch — sem aposentá-los, a msg aparecia 2x (bolha real + eco pendente) até o remount.
      retirePending(startedAt);
      requestAnimationFrame(() => {
        if (bodyEl) bodyEl.scrollTop = bodyEl.scrollHeight - prevH;
      });
    } catch {
      // Sinaliza no mesmo canal do tail inicial (retry recarrega a base) — nunca falha calada.
      if (my === seq) tailError = 'falha ao carregar mais — tocar pra tentar de novo';
    } finally {
      // Incondicional: é o flag DESTA chamada, não do resultado compartilhado — condicionar ao seq
      // deixava "carregando mais…" preso pra sempre quando um retry invalidava o fetch em voo.
      loadingMore = false;
    }
  }
  function onBodyScroll() {
    if (bodyEl && bodyEl.scrollTop <= 0 && !loading) loadMore();
  }

  // Retira o eco que o backend JÁ tinha confirmado quando esta cauda começou: /input 200 = texto na
  // fila durável (ou já digitado no transcript), e o merged_history devolve os dois deduplicados no
  // BACKEND (ts-aware, pqueue.py:327) — daí em diante a bolha real cobre o eco. Compara TEMPO, nunca
  // texto: casar por texto só conseguia falso-positivo (um "ok" antigo na cauda engolia o "ok"
  // recém-digitado -> msg some sem rastro -> você redigita -> entrega dobrada).
  // ackAt = 0 -> POST ainda em voo: segue pendente (a cauda não teria como conter).
  function retirePending(startedAt: number) {
    updatePending((prev) => prev.filter((p) => !p.ackAt || p.ackAt > startedAt));
  }

  async function send() {
    const t = text.trim();
    if (!t) return;
    const id = 'bp-' + Math.random().toString(36).slice(2, 10);
    updatePending((prev) => [...prev, { id, text: t, ackAt: 0 }]);
    text = '';
    onSendError('');
    try {
      await sendInputForServer(server, session.name, t);
      // Aceito pelo backend: a partir daqui QUALQUER cauda nova já contém esta msg -> o eco pode sair.
      updatePending((prev) => prev.map((p) => (p.id === id ? { ...p, ackAt: Date.now() } : p)));
    } catch (err) {
      // 404 = sessão morta etc: remove o eco e SINALIZA — msg nunca some calada.
      updatePending((prev) => prev.filter((p) => p.id !== id));
      // Devolve pro input SÓ se ele ainda estiver vazio: o envio NÃO tem timeout (de propósito), então
      // a janela é ilimitada e o usuário pode ter digitado outra coisa — sobrescrever comeria o texto.
      if (!text) text = t;
      onSendError(err instanceof Error ? err.message : 'falha no envio');
    }
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); }
  }

  let optBusy = $state(false);
  async function answer(i: number) {
    if (optBusy) return;
    optBusy = true;
    onSendError('');
    // option é 1-BASED (api.ts:150 -> backend Field(ge=1) -> (option-1)×Down + Enter).
    try { await selectOptionForServer(server, session.name, i + 1); }
    catch (err) { onSendError(err instanceof Error ? err.message : 'falha'); }
    finally { optBusy = false; }
  }

  // Corte da cauda crua em bolhas (sem resposta órfã) — puro/testável em format.ts, junto do
  // latestAssistantEvent que a espiada do hover usa sobre a MESMA cauda.
  const visible = $derived(bubblesFromTail(events));

  // Modelo/contexto no composer: parse da statusline cacheada da lista (~20s de atraso, ver
  // SessionInfo.status_line). Sem statusline (captura pendente/tema sem emojis) a linha some.
  const meta = $derived(parseStatusLine(session.status_line));
</script>

<article class="bcard" class:attention={session.state === 'awaiting_input'} class:fill>
  {#if session.state === 'working'}
    <!-- Progresso indeterminado: o sinal de "trabalhando" do card. NÃO pulsa (só o dot da coluna
         "Precisa de você" pulsa no board inteiro). -->
    <div class="bc-progress" aria-hidden="true"></div>
  {/if}
  <header class="bc-head" onclick={onOpen} onkeydown={(e) => e.key === 'Enter' && onOpen()} role="button" tabindex="0">
    <span class="bc-dot" style="background: {color}" aria-hidden="true"></span>
    <span class="bc-name">{session.name}</span>
    <!-- Servidor por NOME, não só pela cor do dot: com 5+ servidores a cor sozinha não identifica. -->
    <span class="bc-srv" style="color: {color}" title={server.label}>{server.label}</span>
    <!-- Pill de estado SÓ no canvas (fill): lá não há colunas dizendo o estado; no board a coluna
         já diz e o pill viraria ruído repetido. Vocabulário --pill-* do design system. -->
    {#if fill}
      <span class="bc-state" data-state={session.state}>{stateLabels[session.state]}</span>
    {/if}
    <span class="bc-time">{relativeTime(session.last_activity)}</span>
    <span class="bc-open" title="Abrir chat completo">⤢</span>
  </header>
  <!-- Linha de contexto do card: branch/par + infos da statusline (custo, tempo de sessão) — a
       "parte de cima" das infos compartilhadas do turno. Só aparece quando há o que mostrar. -->
  {#if session.branch || session.pair_peers?.length || meta?.costUsd != null || meta?.sessionTime}
    <div class="bc-sub">
      {#if session.branch}<span class="bc-branch">⎇ {session.branch}</span>{/if}
      {#if session.pair_peers?.length}
        <span class="bc-chip" title={'pareada com ' + session.pair_peers.join(', ')}>🤝 {session.pair_peers.join(', ')}</span>
      {/if}
      {#if meta?.costUsd != null}<span title="custo da sessão">💵 ${meta.costUsd.toFixed(2)}</span>{/if}
      {#if meta?.sessionTime}<span title="tempo de sessão">⏱ {meta.sessionTime}</span>{/if}
    </div>
  {/if}

  <div class="bc-body" bind:this={bodyEl} onscroll={onBodyScroll}>
    <!-- "carregando…" só quando NÃO há o que mostrar: o card remonta a cada troca de coluna e os ecos
         içados sobrevivem — cair no loading por cima deles reintroduziria o sumiço da msg. -->
    {#if loading && visible.length === 0 && pending.length === 0}
      <p class="bc-empty">carregando…</p>
    {:else}
      {#if loadingMore}<p class="bc-empty">carregando mais…</p>{/if}
      {#each visible as e (e.id)}
        {#if e.kind === 'assistant_msg'}
          <!-- Sem sessionName: o FileAttachment resolveria o path contra o servidor ATIVO, que pode
               não ser o dono desta sessão (mesma razão do Compare.svelte:137-141). -->
          <AssistantBubble text={e.text ?? ''} animate={false} />
        {:else}
          <p class="bc-user">{e.text}</p>
        {/if}
      {/each}
      <!-- Meio-apagado enquanto o POST está EM VOO; sólido assim que o backend aceita (ackAt). -->
      {#each pending as p (p.id)}
        <p class="bc-user" class:bc-pending={!p.ackAt}>{p.text}</p>
      {/each}
      {#if tailError}
        <button class="bc-error bc-retry" onclick={loadTail}>{tailError}</button>
      {/if}
      {#if session.state === 'working' && session.label}
        <p class="bc-typing">✳ {session.label}</p>
      {/if}
      {#if session.state === 'awaiting_input' && session.question}
        <div class="bc-question">
          <p>{session.question}</p>
          {#if session.options?.length}
            <div class="bc-options">
              {#each session.options as opt, i (i)}
                <button class="bc-opt" disabled={optBusy} onclick={() => answer(i)}>{opt}</button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    {/if}
  </div>

  <!-- `tracked === false` = claude aberto sem --session-id: transcript não rastreável, não recebe
       input. (Sem checar `dead`: esta lista nunca traz esse estado — ver COLS no Board.) -->
  {#if session.tracked !== false}
    <footer class="bc-foot">
      {#if meta?.model || meta?.ctxPct != null}
        <div class="bc-meta">
          {#if meta.model}<span title="modelo da sessão">🤖 {meta.model}</span>{/if}
          {#if meta.ctxPct != null}
            <span class:bc-ctx-warn={meta.ctxPct >= 80} title="uso da janela de contexto">
              💬 {Math.round(meta.ctxPct)}%
            </span>
          {/if}
        </div>
      {/if}
      <div class="bc-inrow">
        <textarea rows="1" placeholder="Mensagem…" bind:value={text} onkeydown={onKey}></textarea>
        <button class="bc-send" onclick={send} disabled={!text.trim()} aria-label="Enviar">↑</button>
      </div>
    </footer>
  {/if}
  <!-- FORA do guard acima de propósito: o erro não pode depender da condição que o input depende.
       `tracked === false` esconde o input mas NÃO esconde o picker de opções (fica no corpo) — o
       erro do answer() ficava engolido ali dentro. E se a sessão sumir da lista o card inteiro
       evapora: aí o recibo é o do Board (orphanErrors). Clicar dispensa. -->
  {#if sendError}
    <button class="bc-error" onclick={() => onSendError('')} title="Dispensar">{sendError}</button>
  {/if}
</article>

<style>
  /* Card = a superfície do board. Elevação por BORDA hairline, nunca sombra (não lê em dark).
     Sem fundo tingido por estado: a luminância é o eixo de hierarquia aqui. */
  .bcard {
    position: relative;
    display: flex; flex-direction: column;
    /* flex-shrink: 0 é OBRIGATÓRIO: o card é flex item da .col-cards (column). No default
       (shrink: 1) o navegador espreme TODOS os cards pra caber na coluna — com ~13 sessões cada
       um cai pra ~54px, o header resiste e o .bc-body colapsa pra ~0: cards vazios, sem conversa
       nenhuma. A coluna já tem overflow-y: auto; é ela que deve rolar, não o card encolher. */
    flex-shrink: 0;
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-lg);
    box-shadow: none;
    overflow: hidden;
    transition: border-color 140ms var(--ease-out);
  }
  .bcard:hover { border-color: var(--border-default); }
  /* Quem espera por você: borda INTEIRA tingida de warning (side-stripe é ban do design system;
     a listra lateral virou aro completo, mais legível de qualquer ângulo do canvas). */
  .bcard.attention { border-color: color-mix(in srgb, var(--warning) 45%, transparent); }
  .bcard.attention:hover { border-color: color-mix(in srgb, var(--warning) 65%, transparent); }
  /* Estado como pill (vocabulário --pill-* do app.css) — só renderizado no canvas. */
  .bc-state {
    font-size: 10.5px; font-weight: 600; letter-spacing: 0.02em;
    padding: 1px 8px; border-radius: var(--radius-full); flex-shrink: 0; white-space: nowrap;
  }
  .bc-state[data-state='working'] { background: var(--pill-working-bg); color: var(--pill-working-fg); }
  .bc-state[data-state='idle'] { background: var(--pill-idle-bg); color: var(--pill-idle-fg); }
  .bc-state[data-state='awaiting_input'] { background: var(--pill-input-bg); color: var(--pill-input-fg); }
  /* Sem branch 'dead' de propósito: a lista nunca traz esse estado (classify() não devolve dead
     pra lista — só o SSE por-sessão do Chat; ver CLAUDE.md). */
  /* Canvas: o wrapper dita a altura; o corpo vira o flexível (o teto de 240px é regra da COLUNA). */
  .bcard.fill { height: 100%; display: flex; flex-direction: column; }
  .bcard.fill .bc-body { max-height: none; flex: 1; }
  .bc-progress {
    position: absolute; top: 0; left: 0; right: 0; height: 2px; overflow: hidden;
    background: transparent;
  }
  .bc-progress::after {
    content: ''; position: absolute; inset: 0; width: 40%;
    background: var(--accent);
    animation: bc-slide 1.6s var(--ease-in-out) infinite;
  }
  @keyframes bc-slide { from { transform: translateX(-100%); } to { transform: translateX(350%); } }
  .bc-head {
    display: flex; align-items: center; gap: 8px; min-width: 0;
    padding: 10px var(--space-3) 6px; cursor: pointer;
  }
  /* Linha 2 do header: branch/par/custo/tempo — tira o ruído da linha do nome. */
  .bc-sub {
    display: flex; flex-wrap: wrap; gap: 10px; align-items: center;
    padding: 0 var(--space-3) 8px;
    font-size: var(--text-xs); color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .bc-head:hover { background: var(--bg-hover); }
  .bc-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  /* Peso 510, não 600: hierarquia por luminância. 14px: é o título do card. */
  .bc-name { font-size: 14px; font-weight: 510; color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .bc-branch, .bc-chip, .bc-time { font-size: var(--text-xs); color: var(--text-muted); flex-shrink: 0; }
  /* Nome do servidor como PILL tingida na cor do dot — identifica com 5+ servidores sem gritar.
     Encolhe com ellipsis antes do nome da sessão. */
  .bc-srv {
    font-size: var(--text-xs); font-weight: 600; flex-shrink: 1; min-width: 0; max-width: 9em;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
    background: color-mix(in srgb, currentColor 12%, transparent);
    padding: 1px 8px; border-radius: var(--radius-full);
  }
  /* Chip do par carrega NOMES: teto + ellipsis pra não engolir o header (tooltip tem a lista). */
  .bc-chip { max-width: 9em; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .bc-time { margin-left: auto; }
  .bc-open { color: var(--text-muted); flex-shrink: 0; }
  /* Fade SÓ no topo (o rodapé é a msg mais recente — o que você não quer apagar). Dois masks:
     o segundo preserva a scrollbar, senão ela some junto com o conteúdo mascarado. */
  .bc-body {
    max-height: 240px; overflow-y: auto;
    /* contain: com o mouse sobre o card, o wheel para na borda do card — sem encadear pro scroll
       da coluna/canvas (que fazia a lista subir e o card fugir de baixo do cursor). */
    overscroll-behavior: contain;
    padding: 0 var(--space-3) var(--space-3);
    display: flex; flex-direction: column; gap: 8px;
    --mask-h: 28px; --sb-w: 8px;
    mask-image:
      linear-gradient(to bottom, transparent, black var(--mask-h)),
      linear-gradient(black, black);
    mask-size: calc(100% - var(--sb-w)) 100%, var(--sb-w) 100%;
    mask-position: 0 0, 100% 0;
    mask-repeat: no-repeat, no-repeat;
  }
  .bc-empty { color: var(--text-muted); font-size: var(--text-xs); }
  /* Bolha de usuário MINI — mesma linguagem do chat (UserBubble: --bubble-user, canto reto embaixo
     à direita, alinhada à direita), em escala de card. Substitui o texto cru muted que não se
     distinguia da resposta. */
  .bc-user {
    align-self: flex-end; max-width: 85%;
    font-size: 13px; line-height: 1.45; color: var(--text-primary);
    background: var(--bubble-user);
    padding: 5px 10px; border-radius: 12px 12px 4px 12px;
    white-space: pre-wrap; word-break: break-word; margin: 0;
  }
  .bc-pending { opacity: 0.55; }
  .bc-typing { color: var(--text-secondary); font-size: var(--text-xs); font-style: italic; }
  /* Pergunta pendente: tinta de fundo no vocabulário awaiting (--pill-input-*) — listra lateral
     é ban; a tinta preenche o bloco e lê melhor com o card estreito. */
  .bc-question {
    background: var(--pill-input-bg);
    border-radius: var(--radius-sm);
    padding: 6px 10px; font-size: var(--text-xs);
  }
  .bc-options { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 6px; }
  .bc-opt { font-size: var(--text-xs); padding: 3px 10px; border-radius: var(--radius-full); border: 1px solid var(--border-default); background: var(--bg-elevated); color: var(--text-primary); cursor: pointer; }
  .bc-opt:hover:not(:disabled) { background: var(--bg-hover); }
  .bc-opt:disabled { opacity: 0.5; cursor: default; }
  /* Input FANTASMA: 15 cards × 15 inputs com borda = 15 alvos competindo. Borda real só no
     hover/focus. */
  .bc-foot {
    display: flex; flex-direction: column; gap: 4px;
    padding: var(--space-2) var(--space-3) var(--space-3);
    border-top: 1px solid var(--border-subtle);
  }
  /* Modelo + contexto da sessão (statusline cacheada): o contexto do que este composer alcança. */
  .bc-meta {
    display: flex; gap: 10px; font-size: var(--text-xs); color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .bc-ctx-warn { color: var(--warning); }
  .bc-inrow { display: flex; gap: 6px; }
  .bc-foot textarea {
    flex: 1; resize: none; min-height: 28px; max-height: 72px; font: inherit; font-size: 12px;
    background: var(--bg-elevated); color: var(--text-primary);
    border: 1px solid transparent; border-radius: var(--radius-md); padding: 6px 10px;
    transition: border-color 120ms var(--ease-out);
  }
  .bc-foot textarea::placeholder { color: var(--text-muted); }
  .bc-foot:hover textarea { border-color: var(--border-subtle); }
  .bc-foot textarea:focus { border-color: var(--accent); outline: none; }
  .bc-send { width: 28px; border-radius: var(--radius-md); border: 0; background: var(--accent); color: var(--text-inverse); cursor: pointer; }
  .bc-send:disabled { opacity: 0.4; cursor: default; }
  /* Erro de envio e marcador de falha da cauda: mesma cor/tamanho. Os dois são <button> porque os
     dois têm ação (dispensar / tentar de novo); min-* zerados = mesmo escape do alvo global de 44px
     usado no .col-collapse do Board (o quadro é desktop-only). */
  .bc-error {
    display: block; width: 100%; text-align: left;
    color: var(--error); font-family: inherit; font-size: var(--text-xs);
    background: none; border: 0; cursor: pointer; min-height: 0; min-width: 0;
    padding: 0 var(--space-3) var(--space-3); margin: 0;
  }
  /* No corpo do card o recuo já vem do .bc-body. */
  .bc-retry { padding: 0; }
</style>
