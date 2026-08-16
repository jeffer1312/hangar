<script lang="ts">
  // Aba Arquivos do painel do desktop: barra de controles, busca e arvore. Desenho do mock
  // aprovado (docs/mocks/2026-08-15-arvore/1-desktop-painel.html e base.css), token por token.
  // O arquivo aberto e desenhado pela Task 11, sobre a area da conversa (mock 2) — aqui o
  // clique so marca a selecao no store. O store vem do registry do modulo (filesStores.retain)
  // — o App remonta o Chat por {#key} a cada troca de sessao, e um store criado no mount
  // morreria com as pastas abertas (a regua "pasta aberta continua aberta ao voltar").
  import * as m from '../../paraglide/messages';
  import { onMount, onDestroy } from 'svelte';
  import { filesStores } from '../../lib/filesStore.svelte';
  import FileSearchBar, { type ModoBusca } from './FileSearchBar.svelte';
  import FileTree from './FileTree.svelte';

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
  }
  let { sessionName, serverId, desktop }: Props = $props();

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
  });

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
        <button type="button" class="ic" onclick={() => store.recarregar()} aria-label={m.arq_recarregar()} title={m.arq_recarregar()}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M21 12a9 9 0 1 1-2.6-6.4"/><path d="M21 3v6h-6"/></svg>
        </button>
      </div>
    </div>

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

    <div class="corpo">
      {#if temBusca}
        {#if store.buscaCortada}<p class="aviso">{m.arq_primeiros_200()}</p>{/if}
        {#if store.resultados.length > 0}
          <div class="resultados">
            {#each store.resultados as hit (hit.path)}
              {@const nome = hit.path.slice(hit.path.lastIndexOf('/') + 1)}
              <button type="button" class="no" onclick={() => store.abrir(hit.path)}>
                <span class="linha1">
                  <span class="ico" aria-hidden="true">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>
                  </span>
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
      {:else}
        {#if store.listaCortada}<p class="aviso">{m.arq_pasta_grande()}</p>{/if}
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
</div>

<style>
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
  }
  .files-panel.mobile :global(.busca input),
  .files-panel.mobile :global(.seg button) {
    min-height: 44px;
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
  .ic svg { width: 15px; height: 15px; }
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
  .no .ico { width: 14px; flex: none; color: var(--text-muted); display: grid; place-items: center; }
  .no .ico svg { width: 13px; height: 13px; }
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
