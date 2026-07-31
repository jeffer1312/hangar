<script lang="ts">
  // Amostra viva da conversa, dentro da propria tela de Aparencia.
  //
  // Por que existe: TODO slider daqui edita algo que fica ATRAS do painel — tamanho do texto,
  // entrelinha, largura da coluna, contraste, o veu das caixas. Quem arrasta o slider nao ve o efeito
  // sem fechar a tela, e no celular o painel e a tela inteira, entao nao ha "atras" nenhum pra olhar.
  // A amostra reproduz o material real (mesmos tokens do chat: --cp-text-scale, --cp-lh-scale,
  // --bubble-user, --surface-raised, --chrome-bg) sobre o MESMO papel de parede, entao ela muda no
  // mesmo gesto e no mesmo lugar onde esta o dedo.
  //
  // Nao e o chat de verdade de proposito: montar um MessageList aqui traria SSE, markdown, imagens e
  // o custo de um segundo transcript vivo — pra julgar tamanho e contraste, tres linhas bastam.
  interface Props {
    /** Rotulo curto do que esta sendo mexido agora, pra amostra dizer a que veio. */
    titulo?: string;
  }
  let { titulo = 'Prévia' }: Props = $props();
</script>

<div class="amostra" aria-label="Prévia da conversa">
  <span class="amostra-tag">{titulo}</span>

  <div class="linha-user">
    <div class="bolha-user">
      <p class="txt">como ficou o texto?</p>
    </div>
  </div>

  <div class="resposta">
    <p class="txt">
      Assim. Esta é a sua conversa com os ajustes de agora — tamanho, entrelinha,
      contraste e o quanto a foto atravessa as caixas.
    </p>
    <p class="txt cod"><code>npm run check</code></p>
  </div>

  <!-- Faixa igual a do composer: e a caixa que mais aparece na tela e a que o slider Solidez move. -->
  <div class="faixa">
    <span class="chip">/</span>
    <span class="chip">⌘</span>
    <span class="faixa-txt">Mensagem para Claude…</span>
  </div>
</div>

<style>
  .amostra {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    margin-bottom: var(--space-4);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    /* Sem fundo proprio: e o papel de parede do app que tem que aparecer aqui dentro, senao a
       amostra mostraria as caixas sobre uma cor que nao existe em lugar nenhum. */
    background: transparent;
    overflow: hidden;
  }
  /* O mesmo papel de parede do app, recortado dentro da caixa. `fixed` faz o pedaco da foto bater
     com o que esta atras da janela — a amostra vira um "furo" pro fundo real. */
  .amostra::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: -1;
    /* Foto + o MESMO veu que o `body::after` poe por cima dela (`--cp-scrim-*`, que o slider
       Transparencia move). Sem o veu a amostra mente: mostra a foto crua, o texto parece bem menos
       legivel do que fica de verdade, e quem olha corrige um problema que nao existe. */
    background:
      linear-gradient(
        rgba(16, 14, 17, var(--cp-scrim-topo, 0.48)),
        rgba(16, 14, 17, var(--cp-scrim-base, 0.62))
      ),
      var(--cp-wallpaper, var(--bg-base)) center / cover no-repeat;
  }
  :global(html[data-theme="light"]) .amostra::before {
    background:
      linear-gradient(
        rgba(255, 253, 250, var(--cp-scrim-topo, 0.48)),
        rgba(255, 253, 250, var(--cp-scrim-base, 0.62))
      ),
      var(--cp-wallpaper, var(--bg-base)) center / cover no-repeat;
  }

  .amostra-tag {
    align-self: flex-start;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-muted);
  }

  .linha-user { display: flex; }
  .bolha-user {
    background: var(--bubble-user);
    color: var(--text-primary);
    max-width: 80%;
    padding: var(--space-2) var(--space-3);
    border-radius: 14px 14px 14px 4px;
  }

  .resposta { display: flex; flex-direction: column; gap: var(--space-1); }

  /* Os dois tokens que os sliders de Texto movem — os mesmos que UserBubble e AssistantBubble usam. */
  .txt {
    margin: 0;
    font-size: calc(var(--text-base) * var(--cp-text-scale, 1));
    line-height: calc(1.55 * var(--cp-lh-scale, 1));
    color: var(--text-primary);
  }
  .cod code {
    background: var(--surface-inset);
    padding: 1px 6px;
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: 0.9em;
  }

  /* Espelha o composer: fundo de vidro (--chrome-bg) com chips por cima (--surface-raised). E o par
     que o slider Solidez move — e o que estava inerte no Chromium ate o conserto do --glass-bg. */
  .faixa {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-1);
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-lg);
    background: var(--chrome-bg);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 24px;
    height: 24px;
    padding: 0 6px;
    border-radius: var(--radius-md);
    background: var(--surface-raised);
    color: var(--text-secondary);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }
  .faixa-txt { color: var(--text-muted); font-size: var(--text-sm); }
</style>
