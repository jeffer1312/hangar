<script lang="ts">
  import * as m from '../paraglide/messages';
  import { summarizeToolInput } from '../lib/format';
  import { pensamentoEmPt } from '../lib/api';
  import { ehBusca } from '../lib/pensamentoTools.svelte';
  import type { ChatEvent } from '../lib/types';

  // Um turno de raciocínio RECOLHIDO numa linha só, do tamanho do cabeçalho de grupo de ferramenta
  // (ToolGroup), que abre no lugar.
  //
  // `eventos` traz os pensamentos E as buscas que aconteceram entre eles, na ordem. A busca entra
  // AQUI dentro, e não como card solto na conversa, porque foi dentro do raciocínio que ela
  // aconteceu — é o que o app do Claude faz. Quem decide o que entra é o MessageList (só WebSearch
  // e WebFetch); Bash, Edit e Read continuam cards visíveis, senão a sessão inteira sumiria atrás
  // desta linha (medido: 89,6% das chamadas caem entre dois pensamentos).
  interface Props {
    eventos: ChatEvent[];
  }
  let { eventos }: Props = $props();

  let aberto = $state(false);

  const pensamentos = $derived(eventos.filter((e) => e.kind === 'thinking'));
  // O ToolSearch vem junto (senão quebraria o bloco, ver MessageList), mas não é desenhado: ele é
  // encanamento — "select:WebSearch,WebFetch" não diz nada a quem quer saber o que foi pesquisado.
  const chamadas = $derived(
    eventos.filter((e) => e.kind === 'tool_use' && e.tool_name !== 'ToolSearch'),
  );

  // Linha fechada: a primeira frase do PRIMEIRO pensamento. O corte é só visual (o CSS ainda dá
  // reticências); pegar a frase inteira evita cortar no meio de uma palavra na maioria dos casos.
  const resumo = $derived.by(() => {
    const p0 = pensamentos[0];
    const limpo = ((p0 && pt[p0.id]) || p0?.text || '').replace(/\s+/g, ' ').trim();
    const fim = limpo.search(/[.!?](\s|$)/);
    return fim > 0 && fim < 140 ? limpo.slice(0, fim + 1) : limpo;
  });

  // Rótulo do estado aberto. Sem contagem de passos: eles estão à vista logo abaixo. O que a lista
  // aberta NÃO conta sozinha é quanta chamada há mais abaixo, fora da tela — essa é a contada.
  // Conta BUSCA quando só há busca lá dentro, e CHAMADA quando entrou Bash/Edit/Read junto (modo
  // "Tudo"): dizer "3 buscas" pra um bloco que esconde cinco curl seria mentir sobre o que o
  // clique revela.
  const soBusca = $derived(chamadas.every((e) => ehBusca(e.tool_name)));
  const rotulo = $derived.by(() => {
    const n = chamadas.length;
    if (n === 0) return m.pensamento_rotulo();
    if (soBusca) return n === 1 ? m.pensamento_uma_busca() : m.pensamento_buscas({ n });
    return n === 1 ? m.pensamento_uma_chamada() : m.pensamento_chamadas({ n });
  });

  function paragrafos(texto: string): string[] {
    return texto.split(/\n{2,}/).map((p) => p.trim()).filter(Boolean);
  }

  // Tradução quando o bloco ENTRA NA TELA, não no clique: a linha fechada também mostra o resumo,
  // e traduzir só ao abrir deixava fechado em inglês e aberto em português na mesma conversa.
  //
  // Nem no carregamento, porém: uma sessão de código tem centenas de pensamentos, e traduzir todos
  // de uma vez seriam centenas de chamadas por conversa aberta. O observador limita ao que a
  // pessoa realmente vê; o backend ainda guarda por hash, então rolar de volta não paga de novo.
  //
  // Falhou? Fica o inglês. Trocar o texto por erro apagaria justamente o que ela está lendo.
  let pt = $state<Record<string, string>>({});
  let pedindo = false;
  let pronto = false;    // já traduzido: não repete a chamada nem no clique nem no scroll
  async function traduzir() {
    if (pedindo || pronto || !pensamentos.length) return;
    pedindo = true;
    const ids = pensamentos.map((e) => e.id);
    try {
      const r = await pensamentoEmPt(pensamentos.map((e) => e.text ?? ''));
      const mapa: Record<string, string> = {};
      r.textos.forEach((t, i) => { if (t) mapa[ids[i]] = t; });
      pt = mapa;
      pronto = true;
    } catch {
      // Rede caiu: NÃO marca pronto, então abrir o bloco tenta de novo. O observador não serve de
      // segunda chance — ele se desliga na primeira interseção.
    } finally {
      pedindo = false;
    }
  }

  function abrir() {
    aberto = !aberto;
    if (aberto) void traduzir();
  }

  // `raiz` é o elemento do bloco; o observador dispara uma vez e se desliga.
  let raiz = $state<HTMLDivElement | null>(null);
  $effect(() => {
    const el = raiz;
    if (!el) return;
    if (typeof IntersectionObserver === 'undefined') { void traduzir(); return; }   // jsdom/teste
    const obs = new IntersectionObserver((entradas) => {
      if (entradas.some((e) => e.isIntersecting)) { obs.disconnect(); void traduzir(); }
    }, { rootMargin: '200px' });     // adianta o pedido um pouco antes de aparecer
    obs.observe(el);
    return () => obs.disconnect();
  });
</script>

<div class="th" bind:this={raiz}>
  <div
    class="th-head"
    role="button"
    tabindex="0"
    aria-expanded={aberto}
    aria-label={m.pensamento_abrir()}
    onclick={abrir}
    onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); abrir(); } }}
  >
    <svg class="th-chevron" class:open={aberto} width="12" height="12" viewBox="0 0 24 24"
         fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"
         stroke-linejoin="round" aria-hidden="true"><path d="M6 9l6 6 6-6" /></svg>
    <span class="th-resumo">{aberto ? rotulo : resumo}</span>
  </div>

  {#if aberto}
    <div class="th-corpo">
      {#each eventos as ev (ev.id)}
        {#if ev.kind === 'thinking'}
          {#each paragrafos(pt[ev.id] ?? ev.text ?? '') as p, i (i)}<p>{p}</p>{/each}
        {:else if ev.tool_name === 'ToolSearch'}
          <!-- encanamento: entra no bloco só pra não quebrá-lo, não vira linha -->
        {:else if ehBusca(ev.tool_name)}
          <p class="th-busca">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.7" aria-hidden="true">
              <circle cx="12" cy="12" r="9" />
              <path d="M3 12h18M12 3c2.5 2.7 2.5 15.3 0 18M12 3c-2.5 2.7-2.5 15.3 0 18" />
            </svg>
            <span>{summarizeToolInput(ev.tool_name ?? '', ev.tool_input ?? {})}</span>
          </p>
        {:else}
          <!-- No modo "Tudo" entra Bash, Read, Edit… O globo é da BUSCA: pôr o mesmo ícone num
               `sed` diria que ele foi à rede. Aqui o nome da ferramenta é que identifica. -->
          <p class="th-busca th-chamada">
            <span class="th-tool">{ev.tool_name}</span>
            <span>{summarizeToolInput(ev.tool_name ?? '', ev.tool_input ?? {})}</span>
          </p>
        {/if}
      {/each}
    </div>
  {/if}
</div>

<style>
  .th { margin-bottom: var(--space-1); }

  /* Mesmas medidas do .tg-head do ToolGroup: a linha do pensamento e a do grupo de ferramenta são
     o mesmo gesto, e duas medidas diferentes leriam como dois controles diferentes. */
  .th-head {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
    padding: var(--space-1) 0;
    font-size: var(--text-xs);
    line-height: 1.5;
    color: var(--text-muted);
    cursor: pointer;
  }

  .th-chevron {
    flex-shrink: 0;
    align-self: center;
    color: var(--text-muted);
    transition: transform 200ms var(--ease-out);
    transform: rotate(-90deg);
  }
  .th-chevron.open { transform: rotate(0deg); }

  .th-resumo {
    min-width: 0;
    font-size: 12.5px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Aberto: fio à esquerda em vez de caixa. Superfície própria aqui viraria retângulo chapado por
     cima do papel de parede — o texto é aparte da conversa, não um cartão. */
  .th-corpo {
    margin: 2px 0 var(--space-2);
    padding-left: 18px;
    border-left: 1px solid var(--border-subtle);
  }
  /* Mesmo tamanho e mesma cor da linha fechada: aberto, o pensamento continua sendo o aparte, e a
     resposta segue a coisa mais forte da tela. Com 13px em --text-secondary ele competia com a
     resposta em peso visual. A largura tem teto pelo mesmo motivo — linha de 100 caracteres puxa
     o olho pra cá antes de qualquer outra coisa. */
  .th-corpo p {
    margin: 0 0 var(--space-2);
    max-width: 62ch;
    font-size: 12.5px;
    line-height: 1.6;
    color: var(--text-muted);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .th-corpo p:last-child { margin-bottom: 0; }

  /* A busca ganha o globo e o texto um degrau mais claro: dentro do bloco ela é o que a pessoa
     procura ("o que ele pesquisou?"), e o pensamento é o contorno. */
  /* A busca é o que se procura aqui dentro ("o que ele pesquisou?"), então ela — e só ela — sobe
     um degrau de contraste em relação ao texto do pensamento. */
  .th-busca {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    max-width: 62ch;
    color: var(--text-secondary);
    font-size: 12.5px;
    overflow-wrap: anywhere;
  }
  .th-busca svg { flex-shrink: 0; margin-top: 2px; color: var(--text-muted); }

  /* Chamada que não é busca (modo "Tudo"): o nome da ferramenta no lugar do globo, em mono como no
     card da conversa — é o que deixa `Bash` e `Read` distinguíveis de uma linha de pensamento. */
  .th-chamada .th-tool {
    flex-shrink: 0;
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-muted);
  }
</style>
