<script lang="ts">
  // Aba Historico: o empilhado do Tortoise — busca, lista, mensagem, arquivos. O diff nao e painel
  // aqui (ao contrario da aba Mudancas): ele ocupa a janela por cima do empilhado, que e a regra do
  // spec pro unico conteudo que merece a tela toda.
  import CommitList from './CommitList.svelte';
  import CommitMenu from './CommitMenu.svelte';
  import CommitMessage from './CommitMessage.svelte';
  import CommitFiles from './CommitFiles.svelte';
  import LogSearch from './LogSearch.svelte';
  import DiffView from './DiffView.svelte';
  import type { GitCommit } from '../../lib/api';
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props {
    git: GitStore;
    desktop: boolean;
    // Nivel do drill-down no celular: 0 = lista, 1 = commit, 2 = diff. No desktop os tres primeiros
    // painels aparecem juntos e so o diff usa o nivel.
    level: number;
    onPush: () => void;
    onPop: () => void;
    // A faixa de estado do rodape cala o erro enquanto o CommitMenu esta aberto (o menu fica por
    // cima e mostra o erro ele mesmo). Quem sabe do menu e esta aba, entao ela avisa.
    onMenuOpen?: (aberto: boolean) => void;
    // O commit escolhido mora FORA desta aba, junto do nivel, porque o corpo do modal troca de aba
    // com `{#if}` — o componente e destruido e recriado. Estado local voltaria a null e o nivel
    // sobreviveria: apertar "voltar" cairia na lista com o nivel dizendo "commit", e a partir dai
    // cada escolha empurraria um nivel a mais do que devia.
    selecionado: GitCommit | null;
    onSelecionar: (c: GitCommit | null) => void;
  }
  let { git, desktop, level, onPush, onPop, onMenuOpen, selecionado, onSelecionar }: Props = $props();

  let menuCommit = $state<GitCommit | null>(null);
  $effect(() => { onMenuOpen?.(!!menuCommit); });

  // Carrega na primeira vez que a aba aparece. Sem isto o log fica vazio: git.load() so faz
  // refresh() (branches + arquivos), quem preenche `commits` e openLog(). Quem chamava openLog
  // antes era a folha de git (e SO no desktop) e o botao da toolbar — os dois foram apagados.
  let carregou = false;
  $effect(() => { if (!carregou) { carregou = true; git.openLog(); } });

  // O diff ocupa a janela nas DUAS views; no celular ele e o nivel 2, no desktop e uma camada por
  // cima do empilhado. `diffPath` e o dono da verdade — o nivel sozinho nao diz se o fetch deu certo.
  const diffAberto = $derived(!!git.diffPath);

  function escolher(c: GitCommit | null) {
    onSelecionar(c);
    if (!desktop && c) onPush();   // no celular, escolher o commit E descer de nivel
  }
  async function abrirArquivo(path: string) {
    if (!selecionado) return;
    if (await git.openCommitFileDiff(selecionado.hash, path)) onPush();
  }
  // O ⋯ da lista abre o diff SEM passar pelo painel do commit. No celular isso pula um degrau: sem
  // o push extra o nivel para em 1 com o diff na tela, e o "‹ commit" devolveria pra lista — um
  // rotulo mentindo sobre o destino.
  async function abrirDiffDeCommit(c: GitCommit, buscar: () => Promise<boolean>) {
    menuCommit = null;
    if (!(await buscar())) return;
    onSelecionar(c);
    if (!desktop && level === 0) onPush();
    onPush();
  }
  const abrirDiffDoCommit = (c: GitCommit) => abrirDiffDeCommit(c, () => git.openCommitFullDiff(c));
  const abrirDiffVsWorktree = (c: GitCommit) => abrirDiffDeCommit(c, () => git.openCommitWorktreeDiff(c));
  function voltar() {
    if (diffAberto) git.closeDiff();
    else onSelecionar(null);
    onPop();
  }
</script>

{#if diffAberto}
  <div class="ht-diff">
    <button class="git-back" onclick={voltar}>‹ {desktop ? 'histórico' : 'commit'}</button>
    <DiffView path={git.diffPath} rows={git.diffRows} loading={git.diffLoading} truncated={git.diffTruncated} />
  </div>
{:else if desktop}
  <div class="ht-stack">
    <div class="ht-pane ht-search"><LogSearch {git} /></div>
    <div class="ht-pane ht-list">
      {#if git.commits.length}
        <!-- wtCount={0}: a linha sintetica "Working tree changes" saiu do log — ela agora e a aba
             Mudancas, e repetir a mesma porta em dois lugares e a duplicacao que este redesenho veio
             matar. -->
        <CommitList commits={git.commits} wtCount={0} noGraph={!!git.logQuery}
          selectedHash={selecionado?.hash} onSelect={escolher} onMenu={(c) => (menuCommit = c)} />
      {:else}
        <p class="git-muted">{git.logQuery ? 'nenhum commit casa com a busca' : 'sem commits ainda'}</p>
      {/if}
    </div>
    {#if selecionado}
      <div class="ht-pane ht-msg"><CommitMessage commit={selecionado} /></div>
      <div class="ht-pane ht-files">
        <CommitFiles commit={selecionado} sessionName={git.sessionName}
          onOpenFile={abrirArquivo} onMenu={(c) => (menuCommit = c)} />
      </div>
    {:else}
      <!-- Uma vez so, nao um "selecione um commit" em cada painel vazio. -->
      <p class="git-muted ht-empty">selecione um commit</p>
    {/if}
  </div>
{:else if level >= 1 && selecionado}
  <div class="ht-stack">
    <button class="git-back" onclick={voltar}>‹ commits</button>
    <CommitMessage commit={selecionado} />
    <CommitFiles commit={selecionado} sessionName={git.sessionName}
      onOpenFile={abrirArquivo} onMenu={(c) => (menuCommit = c)} />
  </div>
{:else}
  <div class="ht-stack">
    <LogSearch {git} />
    {#if git.commits.length}
      <CommitList commits={git.commits} wtCount={0} noGraph={!!git.logQuery}
        selectedHash={selecionado?.hash} onSelect={escolher} onMenu={(c) => (menuCommit = c)} />
    {:else}
      <p class="git-muted">{git.logQuery ? 'nenhum commit casa com a busca' : 'sem commits ainda'}</p>
    {/if}
  </div>
{/if}

{#if menuCommit}
  <CommitMenu commit={menuCommit} {git} onClose={() => (menuCommit = null)}
    onShowDiff={abrirDiffDoCommit} onShowWorktreeDiff={abrirDiffVsWorktree} />
{/if}

<style>
  .ht-stack { display: flex; flex-direction: column; gap: var(--space-3); min-height: 0; height: 100%; }
  .ht-diff { display: flex; flex-direction: column; gap: var(--space-2); min-height: 0; height: 100%; overflow: auto; }
  /* Proporcao fixa por painel, cada um rolando por conta propria — sem divisoria arrastavel (o app
     nao tem nenhuma) e sem os max-height de 52vh/68vh que os componentes traziam: quem limita
     altura agora e este empilhado. */
  .ht-pane { min-height: 0; overflow: auto; }
  .ht-search { flex: 0 0 auto; overflow: visible; }
  .ht-list { flex: 4 1 0; }
  .ht-msg { flex: 3 1 0; }
  .ht-files { flex: 3 1 0; }
  /* A frase de vazio nao reserva altura: com `flex: 5` ela tomava mais espaco que a propria lista de
     commits, que aparecia cortada em cinco linhas com o resto da tela em branco. */
  .ht-empty { flex: 0 0 auto; }

  .git-back {
    align-self: flex-start; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent; color: var(--text-muted);
    font-size: var(--text-sm); cursor: pointer;
  }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
</style>
