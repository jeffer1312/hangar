<script lang="ts">
  // Linha da tela raiz das Configuracoes: icone + rotulo (+ descricao) + chevron.
  // Presentacional pura — quem sabe o que a linha ABRE e o SettingsModal.
  interface Props {
    icone: string;
    rotulo: string;
    descricao?: string;
    /** Linha que existe mas nao da pra abrir agora (ex.: sem servidor alvo no celular). */
    desabilitada?: boolean;
    /** Por que esta desabilitada — texto VISIVEL, nao tooltip: em tela de toque nao ha hover. */
    motivo?: string;
    onPick: () => void;
  }
  let { icone, rotulo, descricao, desabilitada = false, motivo, onPick }: Props = $props();
</script>

<button class="sr" disabled={desabilitada} onclick={onPick}>
  <span class="sr-icone" aria-hidden="true">{icone}</span>
  <span class="sr-txt">
    <span class="sr-rotulo">{rotulo}</span>
    {#if desabilitada && motivo}
      <span class="sr-desc sr-motivo">{motivo}</span>
    {:else if descricao}
      <span class="sr-desc">{descricao}</span>
    {/if}
  </span>
  {#if !desabilitada}<span class="sr-chevron" aria-hidden="true">›</span>{/if}
</button>

<style>
  .sr {
    /* justify-content: o `button` global e inline-flex CENTRADO (app.css) — sem isto a linha inteira
       centraliza e o icone para num x diferente a cada item (bug ja visto no modal de git). */
    display: flex; align-items: center; justify-content: flex-start; gap: var(--space-3);
    width: 100%; min-height: 52px; padding: var(--space-2) var(--space-3);
    border: 0; background: transparent; color: var(--text-primary);
    font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  @media (hover: hover) { .sr:not(:disabled):hover { background: var(--bg-hover); } }
  .sr:disabled { color: var(--text-muted); cursor: default; }
  .sr-icone { flex-shrink: 0; font-size: 17px; line-height: 1; width: 1.6em; text-align: center; }
  .sr:disabled .sr-icone { opacity: 0.5; }
  .sr-txt { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
  .sr-rotulo { color: inherit; }
  .sr-desc { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.3; }
  .sr-motivo { color: var(--warning); }
  .sr-chevron { flex-shrink: 0; color: var(--text-muted); font-size: var(--text-base); line-height: 1; }
</style>
