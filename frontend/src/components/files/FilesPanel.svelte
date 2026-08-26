<script lang="ts">
  // Aba Arquivos do painel do desktop: barra de controles, busca e arvore. Desenho do mock
  // aprovado (docs/mocks/2026-08-15-arvore/1-desktop-painel.html e base.css), token por token.
  // O arquivo aberto e desenhado pela Task 11, sobre a area da conversa (mock 2) — aqui o
  // clique so marca a selecao no store. O store vem do registry do modulo (filesStores.retain)
  // — o App remonta o Chat por {#key} a cada troca de sessao, e um store criado no mount
  // morreria com as pastas abertas (a regua "pasta aberta continua aberta ao voltar").
  import * as m from '../../paraglide/messages';
  import { onMount, onDestroy, untrack } from 'svelte';
  import { filesStores } from '../../lib/filesStore.svelte';
  import FileSearchBar, { type ModoBusca } from './FileSearchBar.svelte';
  import FileTree from './FileTree.svelte';
  import FileIcon from './FileIcon.svelte';
  import CitadosView from './CitadosView.svelte';
  import { SvelteMap, SvelteSet } from 'svelte/reactivity';
  import { fileUrl, resolverCitados, searchFiles } from '../../lib/api';
  import { acumularCitados, estadoVazio, type Citado } from '../../lib/arquivosCitados';
  import type { ChatEvent } from '../../lib/types';

  interface Props {
    sessionName: string;
    // Identidade do servidor (parecer Task 11, B2): dois servidores podem ter sessoes com o
    // MESMO nome, e o registry do FilesStore chaveia por serverId::sessionName — sem o servidor
    // na chave, o arquivo aberto/arvore do servidor A vazaria pro B. O host desktop passa a
    // MESMA identidade do Chat (getActiveId), nunca calculada diferente por caller.
    serverId: string;
    // A assinatura que as Tasks 11 (desktop) e 12 (celular) hospedam — desktop=true e o caso
    // deste painel; o celular decide o layout proprio na Task 12.
    desktop: boolean;
    // Eventos do chat pra visão "Citados". Só o Chat os tem: o modal Git aberto pela Sidebar
    // monta este painel sem eles, e aí o segmento nem aparece (nunca "Citados (0)").
    events?: ChatEvent[] | null;
    histGap?: string;            // '' = histórico completo; senão a visão avisa "parcial"
    cwd?: string | null;
  }
  let { sessionName, serverId, desktop, events = null, histGap = '', cwd = null }: Props = $props();

  // Citados: acúmulo INCREMENTAL — só os eventos novos desde a última varredura. O histórico
  // completo chega por prepend (prependOlder no Chat), o que muda o índice de tudo: quando o
  // primeiro evento troca, recomeça do zero (uma vez por carga).
  let vista = $state<'arvore' | 'citados'>('arvore');
  let estado = $state(estadoVazio());
  let primeiroId = '';
  $effect(() => {
    const evs = events;
    const base = cwd;
    if (!evs || !base) return;
    const id0 = evs[0]?.id ?? '';
    // `estado` é lido sem tracking: o efeito escreve nele, e lê-lo tracked reagendava o efeito
    // uma segunda vez a cada evento (mesmo laço que o OrquestracaoSheet evita com untrack).
    untrack(() => {
      let atual = estado;
      if (id0 !== primeiroId || evs.length < atual.desde) { primeiroId = id0; atual = estadoVazio(); }
      if (evs.length > atual.desde) atual = acumularCitados(atual, evs, atual.desde, base);
      if (atual !== estado) estado = atual;
    });
  });
  // Só entra na lista o que o servidor confirmou que EXISTE (decisão do usuário, 26/08): os
  // caminhos novos vão em lote pro /files/resolver, que também acerta o relativo de quem foi
  // citado a partir de outra pasta. Quem falta vai pro `ocultos` e nunca aparece.
  const resolvidos = new SvelteMap<string, { relativo: string | null; real: string }>();
  // `pendentes` = na fila, ainda não mandados; `emVoo` = já num POST. Separados pra um lote
  // novo não reenviar o que o anterior ainda está resolvendo.
  const pendentes = new Set<string>();
  const emVoo = new Set<string>();
  let timerResolver: ReturnType<typeof setTimeout> | null = null;
  let vivo = true;
  function agendarResolver(ms: number) {
    if (timerResolver) clearTimeout(timerResolver);
    timerResolver = setTimeout(async () => {
      timerResolver = null;
      const lote = [...pendentes];
      if (!lote.length) return;
      pendentes.clear();
      lote.forEach((c) => emVoo.add(c));
      try {
        const r = await resolverCitados(sessionName, lote);
        if (!vivo) return;
        for (const [cru, v] of Object.entries(r.ok)) resolvidos.set(cru, v);
        for (const cru of r.faltam) ocultos.add(cru);
      } catch {
        // Servidor fora: volta pra fila e tenta de novo daqui a pouco — sem isto o lote que
        // falhou sumia da lista até chegar uma citação nova.
        if (!vivo) return;
        lote.forEach((c) => pendentes.add(c));
        agendarResolver(5000);
      } finally {
        lote.forEach((c) => emVoo.delete(c));
      }
    }, ms);
  }
  $effect(() => {
    const novos = estado.lista.map((c) => c.cru)
      .filter((cru) => !resolvidos.has(cru) && !ocultos.has(cru) && !emVoo.has(cru) && !pendentes.has(cru));
    if (!novos.length) return;
    novos.forEach((c) => pendentes.add(c));
    agendarResolver(250);
  });
  onDestroy(() => { vivo = false; if (timerResolver) clearTimeout(timerResolver); });

  // Mesmo arquivo citado de dois jeitos (absoluto pela tool, relativo na prosa) é UM item.
  const citados = $derived.by(() => {
    const porChave = new Map<string, Citado>();
    for (const bruto of estado.lista) {
      const res = resolvidos.get(bruto.cru);
      if (!res) continue;
      const c = { ...bruto, relativo: res.relativo };
      // Chave = caminho REAL: `~/x`, `/home/.../x` e o symlink viram um item só.
      const k = res.real;
      const j = porChave.get(k);
      if (!j) { porChave.set(k, c); continue; }
      const origens = { ...j.origens };
      for (const [o, n] of Object.entries(c.origens)) origens[o as keyof typeof origens] = (origens[o as keyof typeof origens] ?? 0) + (n ?? 0);
      porChave.set(k, { ...j, origens, ultimoTs: Math.max(j.ultimoTs, c.ultimoTs), primeiroTs: Math.min(j.primeiroTs, c.primeiroTs) });
    }
    return [...porChave.values()].filter((c) => !ocultos.has(c.cru)).sort((a, b) => b.ultimoTs - a.ultimoTs);
  });
  // Citado que não abre SOME da lista (decisão do usuário, 26/08): item cinza e morto só
  // ocupava lugar e não dava pra clicar de novo.
  const ocultos = new SvelteSet<string>();
  const MIDIA = /\.(png|jpe?g|gif|webp|svg|avif|bmp|mp4|mov|webm|mkv|m4v|avi|mp3|wav|m4a|ogg|flac|aac|pdf|html?)$/i;
  // O resultado vem do RETORNO do store (desta abertura), nao do estado global: dois cliques
  // rapidos resolvem fora de ordem, e o estado no fim do await e o da abertura mais nova.
  async function abrirCitado(c: Citado) {
    if (c.relativo === null) {
      // Fora do cwd: texto abre no visor do app; mídia/pdf/html vão pro navegador.
      if (MIDIA.test(c.cru)) { window.open(fileUrl(sessionName, c.cru), '_blank', 'noopener'); return; }
      if (!(await store.abrirExterno(c.cru, fileUrl(sessionName, c.cru)))) ocultos.add(c.cru);
      return;
    }
    if (await store.abrir(c.relativo)) return;
    // O caminho citado era relativo a OUTRA pasta (`tests/x.py` dentro de um `cd backend &&`):
    // procura pelo nome e abre o que termina com o mesmo sufixo, antes de desistir. Homonimo em
    // duas pastas: o sufixo mais longo (o `relativo` inteiro) vence; so o nome puro e o plano B.
    try {
      const r = await searchFiles(sessionName, c.nome, 'names');
      const hit = r.hits.find((h) => h.path === c.relativo || h.path.endsWith('/' + c.relativo))
        ?? r.hits.find((h) => h.path === c.nome || h.path.endsWith('/' + c.nome));
      if (hit && await store.abrir(hit.path)) return;
    } catch { /* busca falhou: cai no esconder */ }
    ocultos.add(c.cru);
  }

  // svelte-ignore state_referenced_locally — captura intencional: o App remonta este painel
  // por {#key} a cada troca de sessao, entao o store do mount e o store da sessao — se a prop
  // mudasse no meio (nao muda), o FilesStore novo substituiria o velho e a regua de pastas
  // abertas morreria.
  const store = filesStores.retain(`${serverId}::${sessionName}`, sessionName);
  onDestroy(() => filesStores.release(`${serverId}::${sessionName}`));

  // Termo e modo da busca. O rascunho do campo vive no FileSearchBar; aqui mora o estado que a
  // arvore e os vazios leem. Busca vazia = arvore.
  let q = $state('');
  let mode = $state<ModoBusca>('names');
  const temBusca = $derived(q.trim() !== '');

  onMount(() => {
    store.recarregar();
    // Selecao lembrada (localStorage) sem conteudo = pagina recarregada: reabre o arquivo.
    if (store.selecionado && !store.conteudo) store.abrir(store.selecionado);
  });

  // Breadcrumb da selecao: raiz › pasta › subpasta (o arquivo em si fica na arvore). Clicar num
  // nivel abre a pasta (se fechada) e rola ate ela.
  const trilha = $derived.by(() => {
    const p = store.selecionado;
    if (!p) return [] as { nome: string; path: string }[];
    const partes = p.split('/');
    const out: { nome: string; path: string }[] = [];
    for (let i = 0; i < partes.length - 1; i++) out.push({ nome: partes[i], path: partes.slice(0, i + 1).join('/') });
    return out;
  });
  let corpoEl = $state<HTMLElement | null>(null);
  async function irPara(path: string) {
    if (path && !store.abertos.has(path)) await store.alternarPasta(path);
    const alvo = path
      ? corpoEl?.querySelector<HTMLElement>(`[data-path="${CSS.escape(path)}"]`)
      : corpoEl?.querySelector<HTMLElement>('.arvore');
    if (!alvo) return;
    if (path) { alvo.scrollIntoView({ block: 'nearest' }); alvo.focus(); }
    else alvo.scrollTo({ top: 0 });
  }
  const carregandoRaiz = $derived(!store.raizCarregada && !store.erro);

  function onBusca(texto: string, novoModo: ModoBusca) {
    q = texto;
    mode = novoModo;
    if (texto.trim() === '') {
      store.resultados = [];   // apagou o campo: volta a arvore sem chamar a rede
      return;
    }
    store.buscar(texto, novoModo);
  }

  // O botao de olho da barra e o "mostrar tudo" da linha de aviso fazem a mesma coisa: alterna
  // o filtro de modificados e re-lista do zero.
  function alternarFiltro() {
    store.soModificados = !store.soModificados;
    store.recarregar();
  }
</script>

<div class="files-panel" class:mobile={!desktop}>
  {#if store.erro}<p class="aviso erro">{store.erro}</p>{/if}

  <div class="barra-ctl">
      <button type="button" class="ordenar">
        {m.arq_ordenar_nome()}<span class="ord-seta" aria-hidden="true">▾</span>
      </button>
      <div class="acoes">
        <button
          type="button"
          class="ic"
          class:ativo={store.soModificados}
          onclick={alternarFiltro}
          aria-label={store.soModificados ? m.arq_mostrar_tudo() : m.arq_mostrar_so_modificados()}
          title={store.soModificados ? m.arq_mostrar_tudo() : m.arq_mostrar_so_modificados()}
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/><circle cx="12" cy="12" r="3"/></svg>
        </button>
        <button type="button" class="ic" onclick={() => store.recolherTudo()} disabled={store.abertos.size === 0} aria-label={m.arq_recolher_tudo()} title={m.arq_recolher_tudo()}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M8 5l4 4 4-4"/><path d="M8 19l4-4 4 4"/><path d="M4 12h16"/></svg>
        </button>
        <button type="button" class="ic" onclick={() => store.recarregar()} aria-label={m.arq_recarregar()} title={m.arq_recarregar()}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M21 12a9 9 0 1 1-2.6-6.4"/><path d="M21 3v6h-6"/></svg>
        </button>
      </div>
    </div>

    {#if events}
      <div class="seg" role="group" aria-label={m.arq_aba()}>
        <button type="button" class:sel={vista === 'arvore'} aria-pressed={vista === 'arvore'} onclick={() => (vista = 'arvore')}>{m.arq_vista_arvore()}</button>
        <button type="button" class:sel={vista === 'citados'} aria-pressed={vista === 'citados'} onclick={() => (vista = 'citados')}>{m.arq_vista_citados({ n: citados.length })}</button>
      </div>
    {/if}

    {#if vista === 'citados' && events}
      <div class="corpo">
        <CitadosView {citados} carregando={events.length === 0} parcial={histGap !== ''} onAbrir={abrirCitado} />
      </div>
    {:else}
    <FileSearchBar {q} {mode} {onBusca} />

    {#if store.soModificados && !temBusca}
      <!-- O filtro vem LIGADO (padrao) e o botao de olho so tem icone: a linha explica por que
           o README nao esta na arvore e da a saida num clique. Com busca ativa ela some — a
           busca varre tudo, o filtro nao se aplica. -->
      <div class="filtro-aviso">
        <span>{m.arq_so_modificados()}</span>
        <button type="button" onclick={alternarFiltro}>{m.arq_mostrar_tudo()}</button>
      </div>
    {/if}

    <!-- Aqui entra o terceiro segmento (Citados) quando houver eventos do chat — outra Task. -->
    <div class="corpo" bind:this={corpoEl}>
      {#if temBusca}
        {#if store.buscaCortada}<p class="aviso">{m.arq_primeiros_200()}</p>{/if}
        {#if store.resultados.length > 0}
          <div class="resultados">
            {#each store.resultados as hit (hit.path)}
              {@const nome = hit.path.slice(hit.path.lastIndexOf('/') + 1)}
              <button type="button" class="no" onclick={() => store.abrir(hit.path)}>
                <span class="linha1">
                  <span class="ico" aria-hidden="true"><FileIcon nome={nome} /></span>
                  <span class="nome">{nome}</span>
                </span>
                {#if mode === 'contents' && hit.line !== null && hit.text !== null}
                  <!-- O trecho so existe no modo Conteudo: e ele que diz POR QUE o arquivo
                       casou. Nome e trecho em linhas separadas — os dois truncam. -->
                  <span class="trecho">{hit.line}: {hit.text}</span>
                {/if}
              </button>
            {/each}
          </div>
        {:else}
          <!-- Busca sem achado: o vazio depende do modo — nomes nao achou arquivo, conteudo
               nao achou linha. -->
          <p class="aviso">{mode === 'names' ? m.arq_sem_nome() : m.arq_sem_conteudo()}</p>
        {/if}
      {:else if carregandoRaiz}
        <!-- So a raiz: a listagem leva ~0,2s; o que doia era a tela vazia sem sinal. -->
        <div class="skel" role="status" aria-busy="true" aria-label={m.arq_carregando()}>
          {#each [58, 34, 46, 72, 40, 62, 30, 50] as w, k (k)}
            <div class="skel-linha"><span class="skel-ico"></span><span class="skel-bar" style="width: {w}%"></span></div>
          {/each}
        </div>
      {:else}
        {#if store.listaCortada}<p class="aviso">{m.arq_pasta_grande()}</p>{/if}
        {#if trilha.length}
          <nav class="trilha" aria-label={m.arq_raiz()}>
            <button type="button" onclick={() => irPara('')}>{m.arq_raiz()}</button>
            {#each trilha as t (t.path)}
              <span class="trilha-sep" aria-hidden="true">›</span>
              <button type="button" onclick={() => irPara(t.path)}>{t.nome}</button>
            {/each}
          </nav>
        {/if}
        {#if store.entries.length === 0 && store.soModificados}
          <p class="aviso">{m.arq_nada_mudou()}</p>
        {:else}
          <!-- O clique so marca a selecao (o desenho do arquivo aberto e da Task 11, sobre a
               conversa — mock 2). Contratos da montagem que a Task 11 herda: quem renderizar o
               FileViewer passa path = o MESMO store.selecionado usado no abrir(), SEM
               normalizar (o backend devolve o caminho como veio, e "./a.py" != "a.py"
               deixaria o viewer em "carregando" para sempre), mais loading, conteudo e diff
               juntos. -->
          <FileTree
            entries={store.entries}
            abertos={store.abertos}
            selecionado={store.selecionado}
            onToggle={(p) => store.alternarPasta(p)}
            onPick={(p) => store.abrir(p)}
          />
        {/if}
      {/if}
    </div>
    {/if}
</div>

<style>
  /* Segmentado Árvore | Citados — o mesmo desenho do Nomes | Conteúdo do FileSearchBar. */
  .seg {
    margin: 0 10px 8px; display: grid; grid-template-columns: 1fr 1fr; gap: 2px;
    background: var(--fill-subtle); border-radius: 8px; padding: 2px;
  }
  .seg button {
    appearance: none; border: 0; background: none; color: var(--text-muted); font: inherit; font-size: 12px;
    padding: 5px 0; border-radius: 6px; cursor: pointer; min-height: 0; min-width: 0;
  }
  .seg button.sel { background: var(--surface-raised); color: var(--text-primary); }
  .seg button:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  /* Mesmas regras do mock (docs/mocks/2026-08-15-arvore/base.css), token por token. O painel
     real usa --space-4 nas margens das secoes; a barra de controles mantem os 10px do mock —
     ela e a face da aba, e o alinhamento interno (busca, linha de filtro, arvore) e dele. */
  .files-panel {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* Celular (Task 12): o mock 3 foi desenhado com mouse, e a regua de toque manda — linha da
     arvore E resultado de busca com no minimo 44px de altura (WCAG 2.5.8 / HIG; o desktop
     preserva a densidade aprovada de ~25px). Fonte 14px como o mock do celular (13px no
     desktop). A altura vem do min-height, nao do padding: a fonte pode crescer sem perder o
     alvo. A aba irma (GitChangesTab no celular) tem a MESMA regra de area de toque nas linhas
     (medido: .git-file 44px, .git-mini 44px). */
  .files-panel.mobile :global(.no) {
    min-height: 44px;
    font-size: 14px;
  }
  /* Os botoes de controle seguem a irma (git-mini 44px) — o mock de 26px era para mouse. O
     icone dentro continua 15px; quem cresce e o alvo. */
  .files-panel.mobile :global(.ic) {
    width: 44px;
    height: 44px;
  }
  .files-panel.mobile :global(.ordenar) {
    min-height: 44px;
  }
  /* Busca e segmentado: o mock do celular pede 13px (12.5/12 no desktop) e mais area de toque
     vertical — o campo e o segmentado com 44px de altura, mesmo padrao do alvo da arvore. */
  .files-panel.mobile :global(.busca input),
  .files-panel.mobile :global(.seg button) {
    font-size: 13px;
    min-height: 44px;
  }
  /* O aviso de filtro e a MESMA linha, mas o botao "mostrar tudo" precisa de alvo de toque no
     celular (B3): a regra compartilhada abaixo (compacta, do mock) manda min-height:0 — o
     override vem DEPOIS e vence por especificidade. O desktop preserva a densidade do mock. */
  .files-panel.mobile .filtro-aviso {
    align-items: center;
    min-height: 44px;
  }
  .files-panel.mobile .filtro-aviso button {
    min-height: 44px;
    min-width: 44px;
    padding-inline: 8px;
  }

  .barra-ctl {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 10px 6px;
    gap: 8px;
  }
  .ordenar {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--text-secondary);
    background: none;
    border: 0;
    padding: 2px 4px;
    cursor: pointer;
    font-family: inherit;
    border-radius: 6px;
    /* Sobrescreve o alvo global de 44px (app.css) pra preservar a densidade do painel —
       mesmo remedio do Composer.svelte. O piso de 24px e o minimo de alvo standalone
       (WCAG 2.5.8 AA): o "Nome" e botao independente, nao texto inline. */
    min-height: 24px;
    min-width: 24px;
  }
  .ord-seta { font-size: 8px; }
  .acoes { display: flex; gap: 2px; }
  .ic {
    width: 26px;
    height: 26px;
    display: grid;
    place-items: center;
    border-radius: 6px;
    border: 0;
    background: none;
    color: var(--text-muted);
    cursor: pointer;
    /* Sobrescreve o alvo global de 44px (app.css) — o desenho do mock e 26x26 e o par
       olho/recarregar so faz sentido na mesma escala. */
    min-height: 0;
    min-width: 0;
  }
  .ic:hover { background: var(--bg-hover); color: var(--text-secondary); }
  .ic:disabled { opacity: 0.4; cursor: default; }
  .ic svg { width: 15px; height: 15px; }

  /* Esqueleto: mesma altura da linha da arvore (25,5px) — a lista nasce no lugar, sem pulo. */
  .skel { padding: 2px 0 10px; }
  .skel-linha { display: flex; align-items: center; gap: 5px; padding: 4px 10px 4px 20px; min-height: 25.5px; }
  .skel-ico, .skel-bar {
    display: block; height: 12px; border-radius: 6px; flex: none;
    background: linear-gradient(90deg, var(--fill-subtle) 25%, color-mix(in srgb, var(--text-muted) 28%, transparent) 50%, var(--fill-subtle) 75%);
    background-size: 200% 100%;
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .skel-ico { width: 16px; height: 16px; border-radius: 4px; }

  /* Trilha da selecao: raiz › pasta › subpasta. Texto clicavel, sem caixa (a arvore e o material). */
  .trilha {
    display: flex; align-items: center; flex-wrap: wrap; gap: 2px;
    margin: 0 10px 2px; font-size: 11px; color: var(--text-muted); font-family: var(--font-mono);
  }
  .trilha button {
    background: none; border: 0; padding: 1px 3px; border-radius: 4px; cursor: pointer;
    color: var(--text-secondary); font: inherit; min-height: 0; min-width: 0;
  }
  .trilha button:hover { background: var(--bg-hover); color: var(--text-primary); }
  .trilha button:focus-visible { outline: 1px solid var(--accent); }
  .trilha-sep { color: var(--text-muted); }
  /* Filtro LIGADO e o padrao, entao ele precisa se anunciar — senao a arvore parece
     incompleta e ninguem sabe por que. */
  .ic.ativo { background: var(--accent-dim); color: var(--accent); }
  .ordenar:focus-visible, .ic:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }

  /* A linha que explica o filtro. Sem ela, "cade o README.md" vira duvida. */
  .filtro-aviso {
    margin: 0 10px 4px;
    display: flex;
    align-items: baseline;
    gap: 6px;
    font-size: 11px;
    color: var(--text-muted);
  }
  .filtro-aviso button {
    background: none; border: 0; padding: 0; cursor: pointer;
    color: var(--accent); font: inherit; font-size: 11px;
    /* Link de texto dentro de uma frase (excecao inline da WCAG 2.5.8) — o alvo global de
       44px faria a linha do aviso dobrar de altura. */
    min-height: 0;
    min-width: 0;
  }

  .corpo {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  /* O FileTree e escopado; a rolagem da arvore so funciona com altura definida. */
  .corpo :global(.arvore) { flex: 1; min-height: 0; }

  .aviso {
    margin: 0 10px 6px;
    padding: 7px 9px;
    border-radius: 7px;
    background: var(--fill-subtle);
    color: var(--text-muted);
    font-size: 11.5px;
    line-height: 1.4;
  }
  /* Erro legivel (sessao encerrada, abrir falhou): a mesma caixa, na cor que o resto do app
     usa pra erro — erro nao e informacao de rodape. */
  .aviso.erro { color: var(--error); }

  /* Resultados de busca: linhas na mesma altura e tipografia da arvore (densidade dos
     vizinhos), com o trecho em linha propria no modo Conteudo. */
  .resultados { padding: 2px 0 10px; overflow-y: auto; }
  .no {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 1px;
    padding: 4px 10px 4px 0;
    cursor: pointer;
    font-size: 13px;
    line-height: 1.35;
    color: var(--text-secondary);
    border: 0;
    background: none;
    width: 100%;
    text-align: left;
    font-family: inherit;
    /* Sobrescreve o alvo global de 44px (app.css) — densidade da arvore, como o mock e a
       aba irma (linha de Task do plano: ~24,6px). */
    min-height: 0;
    min-width: 0;
  }
  .no:hover { background: var(--bg-hover); }
  .no:focus-visible { outline: 1px solid var(--accent); outline-offset: -1px; }
  .linha1 { display: flex; align-items: center; gap: 5px; min-width: 0; }
  .no .ico { width: 16px; flex: none; display: grid; place-items: center; }
  .no .nome {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .trecho {
    display: block;
    padding-left: 19px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--text-muted);
  }
</style>
