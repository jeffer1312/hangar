<script lang="ts">
  // Spy do Chat para o teste da rota legada (REV G1): em vez de montar a tela real, expoe dois
  // botoes — um que chama `onOpenTerminalPanel` (o que o botao "Terminal" do Chat real faria) e um
  // que chama `onOpenSplit` (pra montar o painel de split). `data-nome` carrega a sessao, pra o
  // teste poder clicar no botao do painel certo quando ha mais de um Chat montado.
  let {
    sessionName,
    onOpenTerminalPanel,
    onOpenSplit,
  }: {
    sessionName: string;
    onOpenTerminalPanel: () => void;
    onOpenSplit?: (name: string) => void;
  } = $props();
</script>

<button type="button" data-spy="split" data-nome={sessionName}
        onclick={() => onOpenSplit?.('split-s')}>abrir split</button>
<button type="button" data-spy="abrir-term" data-nome={sessionName}
        onclick={onOpenTerminalPanel}>abrir terminal</button>
