<script lang="ts">
  // Etiqueta de escopo ao lado do rótulo: onde aquele controle grava. Só existe quando a gravação
  // sai deste aparelho — ausência = localStorage do navegador, e essa regra está escrita uma vez
  // na linha fixa do topo do modal.
  import * as m from '../../paraglide/messages';

  interface Props {
    escopo: 'servidor' | 'env';
  }
  let { escopo }: Props = $props();
</script>

<!-- As duas variantes diferem só pelo texto: o desenho é o mesmo de propósito, e não há regra CSS
     por variante pra uma classe carregar. -->
<span class="escopo">{escopo === 'env' ? m.config_escopo_env() : m.config_escopo_servidor()}</span>

<style>
  /* Mesma posição e tamanho da etiqueta "editado" da linha genérica; cor discreta porque esta
     aparece em quase toda linha de servidor e a de override é a que precisa saltar. */
  .escopo {
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.04em;
    color: var(--text-muted);
    background: var(--surface-raised);
    border: 1px solid var(--border-subtle);
    padding: 1px 6px;
    border-radius: var(--radius-full);
    white-space: nowrap;
  }
  /* Container query, não media query: quem aperta a linha é a largura do PAINEL (o dock do desktop
     tem ~530px e uma media query de janela nunca dispara ali). Apertado, a etiqueta desce pra linha
     de baixo em vez de espremer o rótulo em uma palavra por linha. */
  /* `max-content` junto do `flex-basis`: sem ele a etiqueta ganha a linha inteira e vira uma
     barra de ponta a ponta (medido no celular, em "Palavras do seu ditado"). O `flex-basis` é
     quem empurra a quebra; o `max-width` é quem a mantém do tamanho do texto. */
  @container (max-width: 380px) {
    .escopo { flex-basis: 100%; max-width: max-content; }
  }
</style>
