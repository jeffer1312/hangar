<script lang="ts">
  import { chavesUnicas } from '../lib/messageKeys';
  import { tick } from 'svelte';
  import type { ChatEvent, StateEvent, AskQuestionPayload, AnswerItem } from '../lib/types';
  import UserBubble from './UserBubble.svelte';
  import AssistantBubble from './AssistantBubble.svelte';
  import ToolCard from './ToolCard.svelte';
  import ToolGroup from './ToolGroup.svelte';
  import OptionButtons from './OptionButtons.svelte';
  import AskQuestionCard from './AskQuestionCard.svelte';
  import Spinner from './Spinner.svelte';
  import ImageBubble from './ImageBubble.svelte';
  import FileAttachment from './FileAttachment.svelte';
  import { parseImageMessage, parseFilePaths, parsePeerMessage } from '../lib/format';
  import { transcriptImageUrl, uploadUrl } from '../lib/api';
  import { windowStartFor, nextWindowEnd } from '../lib/window';

  interface Props {
    events: ChatEvent[];
    stateEvent: StateEvent | null;
    pending: { id: string; text: string; solid?: boolean }[];
    sessionName: string;
    dockH: number;
    preview?: string;
    previewMd?: boolean;   // o texto da previa e markdown cru -> a bolha renderiza
    onSelectOption: (i: number) => void;
    onCancel: () => void;
    // AskUserQuestion inline (desktop): quando askOpen, renderiza o card no fim da lista.
    askOpen?: boolean;
    askPayload?: AskQuestionPayload | null;
    // Stepper nativo AskUserQuestion ativo (em qualquer view). Suprime os OptionButtons crus:
    // o AskUserQuestion tambem desenha um picker TUI no pane -> sem isto os dois apareciam juntos.
    askActive?: boolean;
    onAnswer?: (answers: AnswerItem[]) => Promise<void>;
    onAskClose?: () => void;
    // Override da URL de imagem do transcript (ex: arquivo de conversas mortas, que nao tem sessao).
    imageUrl?: (id: string, idx: number) => string;
    // Ids de assistant_msg que substituiram um preview em tela: montam SEM animacao (swap invisivel).
    swapIds?: Set<string>;
    // Encaminhar bolha pra outra sessao (long-press/hover ↗). Ausente (ex: Archive) = sem acao.
    onForward?: (text: string) => void;
    // Tap no chip "de: X" de recado peer -> abre o chat da sessao remetente. Ausente = chip estatico.
    onOpenSession?: (name: string) => void;
  }

  let {
    events, stateEvent, pending, sessionName, dockH, preview = '', previewMd = false, onSelectOption, onCancel,
    askOpen = false, askPayload = null, askActive = false, onAnswer, onAskClose, imageUrl, swapIds,
    onForward, onOpenSession
  }: Props = $props();

  let listEl: HTMLElement | undefined = $state();
  // O usuario "gruda" no fim por padrao; ao rolar pra cima, paramos de arrastar.
  let atBottom = $state(true);
  // Rolou MUITO pra cima (mais de uma tela do fim) -> mostra o botao "ir pro fim".
  let scrolledUp = $state(false);

  // Janela de render: monta SO os ultimos WINDOW eventos (a cauda). Sessao longa/compactada (milhares de
  // linhas no .jsonl) montando tudo = tempestade de mount/layout = congela no celular. windowEnd inicia
  // SINCRONO em events.length (o prop ja vem populado: o Chat so monta o MessageList apos loadHistory) ->
  // ja no PRIMEIRO paint a fatia e a cauda, sem montar os 5000 e so depois encolher.
  // WINDOW = botao de calibragem (ajuste no device real); tool_result e filtrado depois, entao bolhas < WINDOW.
  const WINDOW = 120;
  const PAGE = 100;            // quantos eventos antigos revelar por vez ao rolar pro topo (paginacao)
  let windowEnd = $state(0);
  let extra = $state(0);       // eventos revelados ALEM da janela padrao (cresce ao rolar pro topo)

  function onScroll() {
    if (!listEl) return;
    const gap = listEl.scrollHeight - listEl.scrollTop - listEl.clientHeight;
    atBottom = gap < 64; // threshold ~64px do fim
    scrolledUp = gap > listEl.clientHeight; // mais de uma tela do fim = "muito pra cima" -> botao
    // Perto do topo + ainda ha eventos antigos fora da janela -> revela a proxima pagina.
    if (listEl.scrollTop < 200 && hasOlder) revealOlder();
  }

  let revealing = false;
  async function revealOlder() {
    if (revealing || !listEl || !hasOlder) return;
    revealing = true;
    // Preserva a posicao de leitura: o conteudo cresce PRA CIMA (prepend); mede a altura antes, revela,
    // e empurra o scrollTop pelo delta -> a tela nao "pula" pro topo.
    const prevH = listEl.scrollHeight;
    const prevTop = listEl.scrollTop;
    extra += PAGE;
    await tick();
    if (listEl) listEl.scrollTop = prevTop + (listEl.scrollHeight - prevH);
    revealing = false;
  }

  // tool_use_id -> tool_result, INCREMENTAL: `events` e append-only na pratica (replaces do replay
  // repetem o mesmo conteudo; encolheu = reset//clear -> refaz do zero). Reconstruir o Map inteiro a
  // cada evento era O(n) por mensagem. Devolve um wrapper NOVO por rodada (notifica o template; o
  // Map interno persiste) — entradas de transcript antigo pos-reset ficam orfas no Map, inofensivas.
  let _trMap = new Map<string, ChatEvent>();
  let _trLen = 0;
  const toolResults = $derived.by(() => {
    if (events.length < _trLen) { _trMap = new Map(); _trLen = 0; }
    for (let i = _trLen; i < events.length; i++) {
      const ev = events[i];
      if (ev.kind === 'tool_result' && ev.tool_use_id) _trMap.set(ev.tool_use_id, ev);
    }
    _trLen = events.length;
    return { get: (id: string) => _trMap.get(id) };
  });

  // Ids presentes no MOMENTO do mount = historico. Bubble de historico NAO anima: a paginacao pra
  // cima (revealOlder) e o re-ancorar da janela REMONTAM eventos antigos — sem isto, mensagem de
  // ontem entrava com fade/slide como se fosse nova (historico "piscando"). So evento que CHEGA
  // com a tela aberta anima. Snapshot do valor INICIAL de proposito (nao reativo).
  // svelte-ignore state_referenced_locally
  const histIds = new Set(events.map((e) => e.id));

  // O Chat carrega em dois tempos: a cauda pinta a tela e o historico antigo entra depois, POR
  // CIMA de `events` (prepend). `windowEnd` e indice ABSOLUTO -> sem corrigir aqui, a fatia visivel
  // escorregaria pra outro trecho da conversa e o usuario perderia o ponto de leitura. Somar o
  // tamanho do prepend mantem a MESMA fatia montada: nada remonta e o scrollTop nao se mexe.
  // $effect.pre (e nao $effect) porque roda ANTES do DOM: a fatia errada nunca chega a ser montada.
  // Os que entraram sao historico -> vao pro histIds, senao apareceriam com animacao de mensagem
  // nova quando a paginacao pra cima os revelasse.
  // svelte-ignore state_referenced_locally
  let headId: string | undefined = events[0]?.id;
  $effect.pre(() => {
    const first = events[0]?.id;
    if (first === headId) return;
    const grew = headId === undefined ? -1 : events.findIndex((e) => e.id === headId);
    if (grew > 0) {
      for (let i = 0; i < grew; i++) histIds.add(events[i].id);
      windowEnd += grew;
    } else if (windowEnd > events.length) {
      // O topo antigo SUMIU da lista (findIndex = -1): o appendTail re-ancorou tudo na cauda nova
      // depois de um background longo, ou o transcript trocou. windowEnd e ABSOLUTO -> ficaria
      // apontando pra alem do fim do array. O slice clampa, mas por um frame a fatia sai errada,
      // e depender do effect de auto-scroll (nextWindowEnd) consertar no ciclo seguinte e depender
      // de um effect que nem sabe deste. Clampa aqui, com a MESMA regra dele.
      windowEnd = events.length;
    }
    headId = first;
  });

  // Renderiza so tool_use (tool_result vira card) e SO dentro da janela [windowEnd-WINDOW, windowEnd].
  // Fatiamos o array CRU por indice ANTES de filtrar -> windowEnd/length sao indices crus; filtrar a
  // fatia mantem o {#each} keyed (ev.id) valido. toolResults (acima) segue sobre o array INTEIRO, entao
  // um tool_use na janela ainda resolve seu result.
  // Inicio da janela = windowEnd - (WINDOW + extra): por padrao so a cauda; cada reveal cresce `extra`,
  // revelando uma pagina de eventos MAIS ANTIGOS (paginacao pra cima). Os antigos JA estao em `events`
  // (o /history carrega tudo) -> revelar e so expandir a fatia, sem chamada ao backend.
  const windowStart = $derived(windowStartFor(windowEnd, WINDOW + extra));
  const hasOlder = $derived(windowStart > 0);   // ainda ha eventos fora da janela (acima)?
  const visibleEvents = $derived(
    events.slice(windowStart, windowEnd).filter(ev => ev.kind !== 'tool_result')
  );

  // Agrupa RUNS de tool_use consecutivos (sem texto no meio) num card recolhível — uma sessao de
  // exploracao (dezenas de Read/Bash/grep) vira uma linha só em vez de encher a lista. Threshold: 1-2
  // seguidos ficam inline (nao e clutter); >=3 colapsam. `event` = user/assistant normal; `tool` = tool
  // solto; `group` = burst. Key do grupo = 1o tool id (estavel enquanto o run cresce na cauda).
  const GROUP_MIN = 3;
  type RenderItem =
    | { type: 'event'; id: string; ev: ChatEvent }
    | { type: 'tool'; id: string; ev: ChatEvent }
    | { type: 'group'; id: string; tools: ChatEvent[] };
  const renderItems = $derived.by(() => {
    const items: RenderItem[] = [];
    let run: ChatEvent[] = [];
    const flush = () => {
      if (run.length >= GROUP_MIN) items.push({ type: 'group', id: `g-${run[0].id}`, tools: run });
      else for (const t of run) items.push({ type: 'tool', id: t.id, ev: t });
      run = [];
    };
    for (const ev of visibleEvents) {
      if (ev.kind === 'tool_use') { run.push(ev); continue; }
      flush();
      items.push({ type: 'event', id: ev.id, ev });
    }
    flush();
    // Rede de seguranca do {#each} keyed: no Svelte 5 chave repetida e THROW, e ele derruba a arvore
    // toda — a conversa abre vazia e a tela trava, com navbar e composer ainda desenhados por cima.
    // Aconteceu de verdade: duas entradas de fila consumidas no MESMO milissegundo com o MESMO texto
    // sairam do backend com o mesmo `queued:<ts>:<md5>`. O id do backend precisa ser corrigido, mas a
    // lista le um arquivo que outro processo escreve: ela tem que ser imune a qualquer transcript.
    const chaves = chavesUnicas(items.map((i) => i.id));
    return items.map((i, n) => (chaves[n] === i.id ? i : { ...i, id: chaves[n] }));
  });

  // Claude trabalhando? -> msgs da fila durável (id "queued-") ficam atenuadas (= na fila).
  const working = $derived(stateEvent?.state === 'working');

  // Auto-scroll APENAS quando ja estamos no fim. NAO depende de stateEvent (o tick do cronometro/status
  // atualiza stateEvent toda hora e arrastaria o scroll-up do usuario).
  $effect(() => {
    const len = events.length;
    void pending.length;
    void dockH; // composer cresceu (anexo/multilinha) -> re-scrolla pra ultima msg limpar o glass
    void preview; // preview cresce token a token -> acompanha o fundo enquanto o usuario esta colado
    // Mantem a janela: encolheu (reset/clear) re-ancora na cauda; colado no fim acompanha a cauda
    // (remonta o topo SO com o usuario no fundo = sem pulo); rolado pra cima congela. Termina: ao
    // escrever windowEnd=len o effect re-roda e nextWindowEnd vira no-op.
    const next = nextWindowEnd(atBottom, len, windowEnd);
    if (next !== windowEnd) windowEnd = next;
    // De volta ao fim (live): re-ancora na janela-cauda, descartando o que foi revelado pra cima ->
    // limita o mount count de novo. So reseta quando colado no fim (lendo historico, extra persiste).
    if (atBottom && extra !== 0) extra = 0;
    if (!atBottom) return;
    tick().then(scrollToBottom);
  });

  let rafScroll = 0;
  function scrollToBottom() {
    // Coalesce as escritas num rAF: o preview muda a cada ~150ms (e ate token a token), e uma
    // escrita scrollTop=scrollHeight por chunk = tempestade de layout/repaint sincrono = trepidacao
    // (e pressao de repaint que vira bloco preto no iOS). Um rAF + pular quando ja esta no alvo corta isso.
    if (!listEl) return;
    cancelAnimationFrame(rafScroll);
    rafScroll = requestAnimationFrame(() => {
      if (!listEl) return;
      const target = listEl.scrollHeight - listEl.clientHeight;
      if (Math.abs(listEl.scrollTop - target) > 2) listEl.scrollTop = target;
    });
  }
</script>

<section
  class="message-list"
  style="--dock-h: {dockH}px"
  bind:this={listEl}
  onscroll={onScroll}
  aria-label="Mensagens"
>
  <div class="messages-inner">
    {#each renderItems as item (item.id)}
      {#if item.type === 'group'}
        <ToolGroup tools={item.tools} {toolResults} {sessionName} animate={!histIds.has(item.tools[0].id)} />
      {:else}
        {@const ev = item.ev}
        {#if ev.kind === 'user_msg' && (ev.text || ev.image_count)}
        {@const img = ev.text ? parseImageMessage(ev.text) : null}
        <!-- `img` com filenames VAZIO acontece quando o agente absorveu a unica foto como anexo real
             (some o path, fica o marcador): ali a legenda limpa ainda serve, mas uma ImageBubble sem
             nenhuma miniatura nao — por isso os ramos abaixo exigem `imgFotos`, e so o ramo do
             image_count (que traz a foto do transcript) usa o `img` sozinho. -->
        {@const imgFotos = img && img.filenames.length ? img : null}
        {@const peer = ev.text ? parsePeerMessage(ev.text) : null}
        {@const shownText = peer ? peer.text : ev.text ?? ''}
        {#if ev.image_count}
          <!-- Imagem(ns) colada(s) no TERMINAL: thumbnail buscado lazy do .jsonl (base64). Quando a
               msg veio do APP (tem "📎 imagem: <path>"), a legenda entra LIMPA e as fotos enviadas
               entram junto: o Claude Code so absorve a ULTIMA como anexo real e deixa as outras como
               path escrito — sem isto a bolha que fica no chat mostra os caminhos em texto cru. -->
          <ImageBubble caption={img ? img.caption : ev.text ?? ''}
                       srcs={[...(img?.filenames ?? []).map((f) => uploadUrl(sessionName, f)),
                              ...Array.from({ length: ev.image_count }, (_, i) => imageUrl ? imageUrl(ev.id, i) : transcriptImageUrl(sessionName, ev.id, i))]} />
        {:else if ev.id.startsWith('queued-')}
          <!-- Msg da fila durável: atenuada enquanto o Claude trabalha (= na fila, ainda nao
               processada); acende solida quando ele fica idle (= aceita). Da o sinal de "quando
               foi aceita" que o usuario pediu. -->
          <div class="queued-row" class:dim={working}>
            {#if imgFotos}
              <ImageBubble caption={imgFotos.caption} srcs={imgFotos.filenames.map((f) => uploadUrl(sessionName, f))} />
            {:else}
              <UserBubble text={shownText} ts={ev.ts} from={peer?.from} scope={peer?.scope}
                          onForward={onForward ? () => onForward(shownText) : null}
                          onOpenPeer={peer && onOpenSession ? () => onOpenSession(peer.from) : null} />
            {/if}
          </div>
        {:else if imgFotos}
          <ImageBubble caption={imgFotos.caption} srcs={imgFotos.filenames.map((f) => uploadUrl(sessionName, f))} />
        {:else}
          <UserBubble text={shownText} ts={ev.ts} animate={!histIds.has(ev.id)} from={peer?.from} scope={peer?.scope}
                      onForward={onForward ? () => onForward(shownText) : null}
                      onOpenPeer={peer && onOpenSession ? () => onOpenSession(peer.from) : null} />
          {#if ev.text}{@const fr = parseFilePaths(ev.text)}{#if fr.length}<FileAttachment {sessionName} refs={fr} />{/if}{/if}
        {/if}
      {:else if ev.kind === 'assistant_msg' && ev.text}
        <AssistantBubble text={ev.text} ts={ev.ts} {sessionName}
                         animate={!histIds.has(ev.id) && !swapIds?.has(ev.id)}
                         onForward={onForward ? () => onForward(ev.text ?? '') : null} />
        {:else if ev.kind === 'tool_use'}
          <ToolCard event={ev} result={toolResults.get(ev.tool_use_id ?? '') ?? null} {sessionName} animate={!histIds.has(ev.id)} />
        {/if}
      {/if}
    {/each}

    {#if preview}
      <AssistantBubble text={preview} ts={undefined} preview md={previewMd} />
    {/if}

    {#if stateEvent?.state === 'working'}
      <Spinner label={stateEvent.label} />
    {/if}

    {#each pending as p (p.id)}
      {@const pimg0 = parseImageMessage(p.text)}
      {@const pimg = pimg0 && pimg0.filenames.length ? pimg0 : null}
      <div class="pending-bubble" class:solid={p.solid}>
        {#if pimg}
          <ImageBubble caption={pimg.caption} srcs={pimg.filenames.map((f) => uploadUrl(sessionName, f))} />
        {:else}
          <UserBubble text={p.text} ts={undefined} />
        {/if}
      </div>
    {/each}

    {#if stateEvent?.state === 'awaiting_input' && stateEvent.question && !askActive}
      <OptionButtons
        question={stateEvent.question}
        options={stateEvent.options ?? []}
        onSelect={onSelectOption}
        onCancel={onCancel}
      />
    {/if}

    {#if askOpen && askPayload && onAnswer}
      <AskQuestionCard
        open={askOpen}
        payload={askPayload}
        onSubmit={onAnswer}
        onClose={onAskClose ?? (() => {})}
      />
    {/if}
  </div>
</section>

{#if scrolledUp}
  <!-- Botao "ir pro fim": aparece so quando rolou muito pra cima. Ao tocar, volta pra cauda E zera a
       paginacao revelada (extra=0) -> nao fica montando/segurando paginas antigas que nao precisam. -->
  <button
    class="to-bottom"
    style="bottom: calc({dockH}px + var(--space-3))"
    onclick={() => { extra = 0; atBottom = true; scrollToBottom(); }}
    aria-label="Ir para a última mensagem"
  >
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor"
         stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6" /></svg>
  </button>
{/if}

<style>
  .message-list {
    flex: 1;
    overflow-y: scroll;
    /* GUARDA: no mobile NUNCA pode rolar na horizontal (todo o conteudo desloca). Qualquer elemento
       largo (chip de arquivo, token longo) fica clipado aqui; code-block tem seu proprio overflow-x
       interno (continua rolavel dentro da box). */
    overflow-x: hidden;
    overscroll-behavior-y: contain;
    scroll-behavior: auto;
    /* Anti-glitch de repaint do iOS (bloco PRETO no momentum scroll): fundo solido (area
       nao-pintada vira o bg, nao preto). Removido -webkit-overflow-scrolling:touch (legado) E o
       translateZ — o translateZ criava uma CAMADA que renderizava PRETA quando o iOS nao repintava
       a tempo (por isso o preto era puro, ignorando o bg). O guard no fit() (Chat) tira o thrash. */
    background: var(--surface-inset);
    /* O dock (composer glass) flutua sobre a lista (overlap). Padding = altura REAL do dock
       (--dock-h, medido via ResizeObserver no Chat) + respiro, pra ultima msg sempre limpar o
       glass mesmo com anexo/multilinha. ResizeObserver nao dispara na animacao do teclado
       (composer mantem altura), entao nao volta o reflow que glitchava a NavBar. */
    padding-bottom: calc(var(--dock-h, 150px) + var(--space-3));
    /* Navbar overlay (glass): a 1a msg limpa a navbar; ao rolar, o conteudo passa POR BAIXO dela. */
    padding-top: calc(var(--nav-h, 56px) + var(--space-3));
  }

  .messages-inner {
    padding: var(--space-4) var(--space-4) var(--space-2);
    display: flex;
    flex-direction: column;
    max-width: 600px;
    width: 100%;
    min-width: 0;       /* permite os filhos encolherem em vez de empurrar a largura */
    margin: 0 auto;
  }

  /* Desktop: coluna de leitura fixa (~920px), como Claude/ChatGPT/Gemini (~740-920). Linha curta
     cansa menos que encher a viewport. Codigo/tabela longos rolam na horizontal dentro do proprio
     bloco (pre/.md-table tem overflow-x:auto), entao o cap nao os espreme. min(,94vw) da margem
     lateral em telas menores. */
  @media (min-width: 820px) {
    /* A largura vive numa variável porque quem manda nela é de fora: os degraus por tamanho de tela
       (abaixo), o teto maior quando o painel de contexto está aberto (Chat.svelte) e a escolha do
       usuário em Aparência → Largura. Sem a variável, cada um desses seria uma briga de
       especificidade entre arquivos. */
    /* A escala da largura (Aparencia -> Texto da conversa) entra AQUI, no teto, e nao no 94vw: o
       94vw e margem lateral minima da tela, nao preferencia de leitura. */
    /* A escala multiplica o que a coluna TERIA (o menor entre o teto de leitura e o espaco real), e
       nao o teto sozinho. Com o teto cru, numa janela onde o espaco ja e menor que ele — desktop de
       1440 com sidebar e painel de contexto da 891px — a coluna ja estava no limite e QUALQUER valor
       acima de 100 nao fazia nada: medido, 100 e 150 davam os mesmos 891px, metade do slider era
       decorativa. Com a base proporcional, reduzir sempre reduz; aumentar cresce ate onde houver
       espaco (o `min(..., 100%)` de fora e o limite fisico). */
    .messages-inner { --read-w: calc(min(920px, 100%) * var(--cp-width-scale, 1)); max-width: min(var(--read-w), 94vw); }
    /* Respiro lateral do TEXTO, não da coluna. Quando a coluna bate no teto e sobra tela, a margem
       vem do `margin: 0 auto`; mas com o painel de contexto aberto o teto sobe pra 1200px
       (Chat.svelte:1289) e a coluna passa a ocupar 100% do espaço — aí os 16px de padding eram tudo
       o que separava a primeira letra do trilho. Medido em 1514px de janela: sobravam ~25px. */
    .messages-inner { padding-inline: var(--space-6); }
  }
  @media (min-width: 1280px) {
    .messages-inner { padding-inline: var(--space-8); }
  }
  /* Tela grande tem espaço sobrando: a coluna cresce em degraus em vez de ficar presa nos 920 e
     deixar duas faixas vazias. Não vira largura livre de propósito — linha muito longa faz o olho
     perder a volta —, mas 1080/1200 ainda é confortável no tamanho de fonte daqui. */
  @media (min-width: 1600px) { .messages-inner { max-width: min(calc(min(1080px, 100%) * var(--cp-width-scale, 1)), 82vw); } }
  @media (min-width: 1900px) { .messages-inner { max-width: min(calc(min(1200px, 100%) * var(--cp-width-scale, 1)), 76vw); } }

  /* Leitura SÓLIDA (Aparência → Leitura; `auto` liga sozinho quando o fundo é uma imagem): a coluna
     da conversa vira uma folha quase opaca e a foto passa a viver no cromo e nas margens, em vez de
     ficar atrás do texto. Sem isto, quanto mais legível o texto, menos sobra da imagem — é o que o
     mock .claude/mock-fundo-imagem-completo.html compara lado a lado. */
  /* TEXTO: sem superfície nenhuma — a foto continua aparecendo entre as mensagens, e o que muda é
     o texto: contraste cheio e uma sombra curta que o segura mesmo sobre as partes claras da
     imagem. É o padrão do 'auto' porque a conversa é a página, e um retângulo do tamanho da tela
     lê como outro site colado por cima. */
  :global(html[data-read='text']) .messages-inner {
    /* Os tokens de texto do app são de propósito MAIS ESCUROS que branco (app.css:24 — branco puro
       sobre fundo escuro sangra nas bordas e cansa em sessão longa). Sobre uma FOTO a conta muda:
       ali o fundo é claro em pedaços e o texto precisa voltar pro branco. `--cp-text-boost` (0..100,
       slider "Contraste") diz quanto dessa volta acontece — 0 mantém o conforto, 100 é branco puro.
       Redefinir o token aqui alcança os filhos que usam var(--text-primary) por conta própria; só
       trocar `color` pegava apenas o texto que herda. */
    --text-primary: color-mix(in srgb, #fff calc(var(--cp-text-boost, 60) * 1%), var(--text-primary-base));
    --text-secondary: color-mix(in srgb, #fff calc(var(--cp-text-boost, 60) * 0.7%), var(--text-secondary-base));
    /* muted é onde estão as linhas de ferramenta e os horários — o texto mais fraco da tela, e o
       primeiro a sumir sobre a parte clara da foto. Sobe junto, num passo menor. */
    --text-muted: color-mix(in srgb, #fff calc(var(--cp-text-boost, 60) * 0.55%), var(--text-muted-base));
    color: var(--text-primary);
    text-shadow: 0 1px 3px rgba(0, 0, 0, calc(var(--cp-read-alpha, 92) * 0.009)),
                 0 0 12px rgba(0, 0, 0, calc(var(--cp-read-alpha, 92) * 0.006));
  }
  /* No tema CLARO o reforço vai pro outro lado. A regra acima empurra o texto pro BRANCO e põe
     sombra preta: certo sobre foto num app escuro, suicida num app claro, onde --text-primary-base
     já é #221d1b (app.css:104). Misturar 60% de branco nele dá cinza claro, e sobre a parte clara da
     foto a conversa some — foi o que apareceu no print. Aqui o alvo é o PRETO e o halo é branco,
     mesma conta e mesmo slider, espelhados. */
  :global(html[data-theme='light'][data-read='text']) .messages-inner {
    --text-primary: color-mix(in srgb, #000 calc(var(--cp-text-boost, 60) * 1%), var(--text-primary-base));
    --text-secondary: color-mix(in srgb, #000 calc(var(--cp-text-boost, 60) * 0.7%), var(--text-secondary-base));
    --text-muted: color-mix(in srgb, #000 calc(var(--cp-text-boost, 60) * 0.55%), var(--text-muted-base));
    text-shadow: 0 1px 3px rgba(255, 255, 255, calc(var(--cp-read-alpha, 92) * 0.009)),
                 0 0 12px rgba(255, 255, 255, calc(var(--cp-read-alpha, 92) * 0.006));
  }

  /* FOLHA: o oposto — card explícito atrás da conversa inteira, com moldura e sombra.
     --cp-read-alpha (0..100) vem do slider: 100 tapa a foto, 0 some com a folha. */
  :global(html[data-read='solid']) .messages-inner {
    background: color-mix(in srgb, var(--bg-elevated) calc(var(--cp-read-alpha, 92) * 1%), transparent);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-lg);
    padding: var(--space-4);
    margin-block: var(--space-2);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.28);
  }

  /* Bubble enfileirado: ainda nao processado pelo Claude — atenuado ate solidificar. Precisa ser
     flex-column pra que UserBubble/ImageBubble (que alinham pelo pai flex) fiquem na mesma margem
     esquerda da resposta — senao o wrapper block muda o comportamento do align-self da imagem. */
  .pending-bubble {
    display: flex;
    flex-direction: column;
    /* align-items default (stretch): o UserBubble (.bubble-wrap) ocupa a largura e alinha o balao
       sozinho; a ImageBubble usa align-self. NAO trocar por flex-start/flex-end aqui: encolheria o
       bubble-wrap pro min-content -> palavra curta ("sim") quebrava letra a letra. */
    opacity: 0.5;
    transform: scale(0.97);
    transition: opacity 360ms var(--spring), transform 360ms var(--spring);
  }
  /* Solidificado: o Claude ja consumiu a fila -> vira bubble normal (sem atenuar). */
  .pending-bubble.solid {
    opacity: 1;
    transform: none;
  }

  /* Msg da fila durável (evento sintetico "queued-"): mesma margem das demais, atenuada enquanto
     esta na fila. Acende sozinha quando o Claude fica idle (transition). */
  .queued-row {
    display: flex;
    flex-direction: column;
    /* stretch (default): bubble de texto ocupa a largura e alinha sozinho; imagem usa align-self.
       flex-start/flex-end encolhem o bubble pro min-content -> "sim" quebrava letra a letra. */
    transition: opacity 240ms var(--ease-out), transform 360ms var(--spring);
  }
  .queued-row.dim {
    opacity: 0.5;
    transform: scale(0.97);   /* na fila: atenua E encolhe um tico; assenta com spring ao ser aceita. */
  }

  /* Botao flutuante "ir pro fim": fixo no canto, acima do dock (bottom = altura do composer + respiro).
     z acima das msgs, abaixo dos sheets. So aparece quando scrolledUp (rolou +1 tela pra cima). */
  .to-bottom {
    position: fixed;
    right: var(--space-4);
    z-index: 6;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--surface-raised);
    border: 1px solid var(--border-default);
    color: var(--text-primary);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
    transition: transform 140ms var(--ease-out), background 140ms var(--ease-out);
  }
  .to-bottom:active {
    transform: scale(0.92);
    background: var(--bg-hover);
  }
</style>
