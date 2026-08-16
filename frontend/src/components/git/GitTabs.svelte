<script lang="ts">
  import * as m from '../../paraglide/messages';
  // Corpo do modal de git: cabecalho, fileira de abas, a aba ativa e a faixa de estado no rodape.
  // Nem o BottomSheet nem o ModalDialog desenham chrome (o × da folha so existe no modo persistent),
  // entao sem este cabecalho o modal sairia so por Esc/backdrop e nunca diria de que repositorio e —
  // o que importa porque ele abre pela linha da sidebar, sem abrir o chat.
  import { GIT_TABS, initialNav, selectTab, pushLevel, popLevel, currentLevel, type GitNav, type GitTabId } from '../../lib/gitTabs';
  import GitChangesTab from './GitChangesTab.svelte';
  import GitHistoryTab from './GitHistoryTab.svelte';
  import GitBranchesTab from './GitBranchesTab.svelte';
  import GitStatusBar from './GitStatusBar.svelte';
  import RepoMenu from './RepoMenu.svelte';
  import FilesPanel from '../files/FilesPanel.svelte';
  import FileViewer from '../files/FileViewer.svelte';
  import { filesStores } from '../../lib/filesStore.svelte';
  import { getActiveId } from '../../lib/auth';
  import type { FilesStore } from '../../lib/filesStore.svelte';
  import type { GitCommit } from '../../lib/api';
  import type { GitStore } from '../../lib/gitStore.svelte';
  import { tick } from 'svelte';

  interface Props { git: GitStore; desktop: boolean; filesInContext: boolean; onClose: () => void }
  let { git, desktop, filesInContext, onClose }: Props = $props();

  let nav = $state<GitNav>(initialNav());
  let repoMenu = $state(false);
  let menuAberto = $state(false);   // CommitMenu aberto na aba Historico -> a faixa cala o erro
  // O commit escolhido mora aqui pelo mesmo motivo que o nivel: trocar de aba destroi o componente
  // da aba, e uma selecao que morresse deixaria o nivel apontando pra uma tela que nao existe mais.
  let commitSel = $state<GitCommit | null>(null);

  // Store da aba Arquivos (Task 12): a MESMA instancia do FilesPanel (registry por identidade
  // serverId::sessionName) — o clique na arvore marca a selecao, e quem sobe o nivel do
  // drill-down e este componente observando o store. O retain e CONDICIONAL ao host que nao
  // tem painel de contexto (B1): com filesInContext=true o Git nao oferece a aba (o
  // DesktopSessionContext visivel ja a hospeda) e reter um store que o modal nao usa seguraria
  // estado a toa. O $effect tambem re-retem quando a sessao muda (release da chave antiga) —
  // mesmo padrao do Git.svelte.
  let filesStore = $state<FilesStore | null>(null);
  $effect(() => {
    if (filesInContext) return;
    const chave = `${getActiveId() ?? ''}::${git.sessionName}`;
    const s = filesStores.retain(chave, git.sessionName);
    filesStore = s;
    return () => filesStores.release(chave);
  });
  // Foco do drill-down (B2): o usuario de teclado que abre um arquivo na arvore precisa entrar
  // no visor e voltar para a MESMA linha. Guarda o path e a LINHA que abriu ANTES do push — so
  // linha de arquivo conta (arvore OU resultado de busca); toque/AT deixam o foco no body e a
  // restauracao nao pode devolver o foco pra ele.
  let abridorPath: string | null = null;
  let abridorEl: HTMLElement | null = null;
  // Path do arquivo aberto — variavel LOCAL de proposito: dentro do {#if} o template estreita o
  // tipo pra string (filesStore.selecionado e string | null e o TS nao acompanha o if). Quando a
  // LEITURA falha, o store limpa selecionado e grava erro — o abridorPath mantem o visor montado
  // com o arquivo que falhou (B3), em vez de voltar para uma arvore silenciosamente normal.
  const arquivoAberto = $derived(filesStore?.selecionado ?? abridorPath);

  // No host com painel de contexto visivel o Git nao oferece a aba Arquivos (B1): o
  // DesktopSessionContext ja e a casa dela, e o modal montaria o FilesPanel mobile por engano.
  // A fileira e filtrada, e se o `filesInContext` mudar com o componente montado (resize entre
  // 820 e 1279px), a aba volta para Mudancas e o arquivo aberto e limpo — o nivel da aba files
  // fica no nav, inerte, ate um proximo host sem contexto.
  const abasVisiveis = $derived(filesInContext ? GIT_TABS.filter((t) => t.id !== 'files') : GIT_TABS);
  $effect(() => {
    if (!filesInContext || nav.tab !== 'files') return;
    if (filesStore?.selecionado) filesStore.selecionado = null;
    abridorPath = null;
    abridorEl = null;
    // O nivel da aba files volta a 0 EXPLICITAMENTE (B3, rodada 4): selectTab copia levels
    // inteiro, e um nivel fantasma (1 sem arquivo) faria o proximo drill-down pular o push ao
    // voltar a estreito — e uma leitura que falhasse sem abridorPath desmontaria o visor.
    nav = { tab: 'changes', levels: { ...nav.levels, files: 0 } };
    // O visor desmontou por esta troca: foco para um controle visivel do modal (a primeira aba
    // da fileira). O tick().then do push antigo (foco no .fechar) vira no-op — o visor nao
    // existe mais e o querySelector devolve null.
    void tick().then(() => {
      document.querySelector<HTMLElement>('.gt [role=tab]')?.focus();
    });
  });

  // Foco do drill-down (B2): ...
  // (abridorPath/abridorEl declarados acima, junto do store)

  // Drill-down da aba Arquivos: o clique no arquivo marca a selecao no store (FilesPanel), e o
  // nivel 0 -> 1 sobe aqui — sem o push, o maxLevel do GIT_TABS nao vale. O nivel 1 nao
  // empurra de volta: fecharArquivo (abaixo) e a unica saida.
  $effect(() => {
    if (nav.tab !== 'files' || !filesStore || filesStore.selecionado === null) return;
    if (currentLevel(nav) === 0) {
      abridorPath = filesStore.selecionado;
      const ativo = document.activeElement;
      abridorEl = ativo instanceof HTMLElement
        && (ativo.matches('.files-panel .arvore .no') || ativo.matches('.files-panel .resultados .no'))
        ? ativo : null;
      nav = pushLevel(nav);
      // O foco so pode ir depois do flush (o visor monta no mesmo flush); o Fechar do
      // FileViewer e o primeiro controle do conteudo novo — fallback o proprio visor.
      void tick().then(() => {
        (document.querySelector<HTMLElement>('.gt-body .visor .fechar')
          ?? document.querySelector<HTMLElement>('.gt-body .visor'))?.focus();
      });
    }
  });

  // Unica saida do arquivo aberto (× e "voltar" do FileViewer passam por aqui): limpa a
  // selecao no store e desce o nivel — o FilesPanel volta com a arvore do jeito que estava (o
  // estado da arvore e do store, nao do componente). O foco volta para a linha que abriu (B2);
  // se ela nao estiver mais visivel (filtro/recarga mudaram a arvore), cai na aba Arquivos e,
  // por ultimo, na primeira linha.
  function fecharArquivo() {
    if (!filesStore) return;
    const path = abridorPath;
    const origem = abridorEl;
    filesStore.selecionado = null;
    abridorPath = null;
    abridorEl = null;
    nav = popLevel(nav);
    void tick().then(() => {
      const linhaValida = (el: HTMLElement | null | undefined): el is HTMLElement => !!el
        && el !== document.body && el.isConnected && el.getClientRects().length > 0
        && (el.matches('.files-panel .arvore .no') || el.matches('.files-panel .resultados .no'));
      if (linhaValida(origem)) { origem.focus(); return; }
      const linha = [...document.querySelectorAll<HTMLElement>('.files-panel .arvore .no')]
        .find((n) => n.dataset.path === path);
      if (linhaValida(linha)) { linha.focus(); return; }
      const aba = document.querySelector<HTMLElement>('[role=tab][aria-selected="true"]');
      if (aba?.isConnected && aba.getClientRects().length > 0) { aba.focus(); return; }
      const primeira = document.querySelector<HTMLElement>('.files-panel .arvore .no');
      if (primeira?.isConnected) primeira.focus();
    });
  }

  // Contagem no rotulo: `branches` conta locais + remotas porque o BranchList mostra as duas —
  // contar so as locais daria um numero que nao bate com a lista.
  const contagem = (id: GitTabId) =>
    id === 'changes' ? git.files.length
    : id === 'branches' ? git.branches.length + git.remotes.length
    : 0;

  // O _run forca LC_ALL=C (git_ops.py), entao a mensagem do git nao vem traduzida e o teste de texto
  // e estavel. O stderr cru NAO aparece: quem abriu a pasta errada nao precisa da linha do git.
  const naoEhRepo = $derived(!!git.error && /not a git repository/i.test(git.error));

  // O diff aberto e do STORE, um so pro modal inteiro (Task 3). Sem fechar na troca, a aba Historico
  // herdava o diff aberto na aba Mudancas — trocar de aba mostrava o arquivo da outra, com um
  // "‹ historico" que nao levava a lugar nenhum. Some o diff E o degrau que ele ocupava.
  // O arquivo da aba Arquivos e a mesma regra: sem limpar, a aba volta com o arquivo aberto
  // herdado (Task 12).
  function trocarAba(id: GitTabId) {
    if (git.diffPath) { git.closeDiff(); nav = popLevel(nav); }
    if (nav.tab === 'files' && ((filesStore?.selecionado ?? null) !== null || abridorPath !== null)) {
      if (filesStore) filesStore.selecionado = null;
      abridorPath = null;
      abridorEl = null;
      nav = popLevel(nav);
    }
    nav = selectTab(nav, id);
  }
</script>

{#if naoEhRepo}
  <p class="gt-muted gt-vazio">{m.git_nao_repo()}</p>
{:else}
  <div class="gt" class:gt-desktop={desktop}>
    <header class="gt-head">
      <div class="gt-id">
        <span class="gt-repo">{git.sessionName}</span>
        {#if git.current}<span class="gt-branch">{git.current}</span>{/if}
      </div>
      <div class="gt-actions">
        <button class="gt-icon" onclick={() => (repoMenu = !repoMenu)} aria-label={m.git_acoes_repo()}
          aria-expanded={repoMenu} title={m.git_acoes_repo()}>⋯</button>
        <button class="gt-icon" onclick={onClose} aria-label={m.git_fechar()} title={m.git_fechar()}>✕</button>
        {#if repoMenu}
          <RepoMenu {git} onClose={() => (repoMenu = false)} />
        {/if}
      </div>
    </header>

    <div class="gt-tabs" role="tablist">
      {#each abasVisiveis as t (t.id)}
        {@const n = contagem(t.id)}
        <button class="gt-tab" class:sel={nav.tab === t.id} role="tab" aria-selected={nav.tab === t.id}
          onclick={() => trocarAba(t.id)}>
          {t.label}{#if n}<span class="gt-count">{n}</span>{/if}
        </button>
      {/each}
    </div>

    <div class="gt-body">
      {#if git.loading}
        <p class="gt-muted">{m.board_carregando()}</p>
      {:else if nav.tab === 'changes'}
        <GitChangesTab {git} {desktop} level={currentLevel(nav)}
          onPush={() => (nav = pushLevel(nav))} onPop={() => (nav = popLevel(nav))} />
      {:else if nav.tab === 'files' && !filesInContext}
        <!-- Task 12: a aba Arquivos quando NAO ha painel de contexto visivel (celular, desktop
             estreito 820-1279px, Sidebar e split — B1). Nivel 0 = o FilesPanel (arvore/busca);
             o clique sobe o nivel pelo store (effect acima) e o nivel 1 mostra o arquivo — o
             MESMO FileViewer do desktop, agora como degrau do drill-down dentro do modal. O ×
             e o "voltar" do proprio FileViewer chamam fecharArquivo; o rotulo do voltar e
             m.comum_voltar — nao ha conversa atras no modal (B4). O `erro` do store aparece no
             visor (B3): leitura que falha (binario, 404) nao desmonta o visor nem volta para
             uma arvore silenciosamente normal — o usuario ve qual arquivo falhou e fecha. -->
        {#if currentLevel(nav) >= 1 && arquivoAberto}
          <FileViewer
            path={arquivoAberto}
            diff={filesStore?.diff ?? null}
            conteudo={filesStore?.conteudo ?? null}
            loading={filesStore?.loading ?? false}
            erro={filesStore?.erro ?? null}
            onEscopo={(e) => filesStore?.trocarEscopo(e)}
            onFechar={fecharArquivo}
            rotuloVoltar={m.comum_voltar()}
          />
        {:else}
          <FilesPanel
            sessionName={git.sessionName}
            serverId={getActiveId() ?? ''}
            desktop={false}
          />
        {/if}
      {:else if nav.tab === 'history'}
        <GitHistoryTab {git} {desktop} level={currentLevel(nav)}
          onPush={() => (nav = pushLevel(nav))} onPop={() => (nav = popLevel(nav))}
          onMenuOpen={(aberto) => (menuAberto = aberto)}
          selecionado={commitSel} onSelecionar={(c) => (commitSel = c)} />
      {:else}
        <GitBranchesTab {git} />
      {/if}
    </div>

    <footer class="gt-foot">
      <GitStatusBar {git} {menuAberto} />
    </footer>
  </div>
{/if}

<style>
  .gt { display: flex; flex-direction: column; gap: var(--space-3); min-height: 0; height: 100%; }
  /* No desktop a folha `centered` tem `height: auto` — dimensiona pelo conteudo. Sem uma altura
     propria aqui, os paineis com `flex: N 1 0` das abas colapsam pra ZERO (a aba Historico abria com
     a lista de commits invisivel), e o modal ainda pulava de tamanho a cada troca de aba. */
  .gt-desktop { height: min(72vh, 720px); }
  /* Task 14: no mobile o sheet so tem max-height (sem height) — o height:100% caia em auto e a
     caixa do diff (agora height: fit-content) esticava o modal inteiro. Esta altura e o content
     box do sheet (o max-height menos o padding dele), o que devolve a referencia para o
     drill-down encolher e a caixa rolar por dentro. */
  .gt:not(.gt-desktop) {
    height: calc(100dvh - var(--space-8) - var(--space-4) - var(--space-5) - env(safe-area-inset-bottom, 0px));
  }
  .gt-head { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); flex-shrink: 0; }
  .gt-id { display: flex; align-items: baseline; gap: var(--space-2); min-width: 0; }
  .gt-repo { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .gt-branch { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--accent); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  /* position: relative pro RepoMenu, que nasce absoluto no canto de baixo/direita deste bloco. */
  .gt-actions { position: relative; display: flex; align-items: center; gap: var(--space-1); flex-shrink: 0; }
  .gt-icon {
    width: 32px; height: 32px; border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent; color: var(--text-muted);
    font-size: var(--text-sm); cursor: pointer;
  }
  .gt-icon:hover { background: var(--bg-hover); }

  /* touch-action: pan-x PROPRIO — o BottomSheet declara pan-y e sem isto a fileira nao rola no dedo. */
  .gt-tabs {
    display: flex; gap: var(--space-1); flex-shrink: 0;
    overflow-x: auto; touch-action: pan-x; scrollbar-width: none;
  }
  .gt-tabs::-webkit-scrollbar { display: none; }
  .gt-tab {
    flex-shrink: 0; display: flex; align-items: center; gap: var(--space-1);
    padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-muted); font-size: var(--text-sm); cursor: pointer;
  }
  .gt-tab:hover { background: var(--bg-hover); }
  .gt-tab.sel { background: var(--accent-dim); color: var(--accent); }
  .gt-count {
    padding: 0 6px; border-radius: var(--radius-full);
    background: var(--surface-raised); color: var(--text-muted);
    font-family: var(--font-mono); font-size: 10px;
  }
  .gt-tab.sel .gt-count { color: var(--accent); }

  .gt-body { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; }
  .gt-foot { flex-shrink: 0; display: flex; flex-direction: column; gap: var(--space-2); }
  .gt-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .gt-vazio { padding: var(--space-4); }
</style>
