<script lang="ts">
  // Linha da tela raiz das Configuracoes: icone de traco + rotulo + chevron, uma linha só —
  // formato da lista de ajustes do iOS (referência: app do Claude). A descricao embaixo saiu:
  // treze linhas com duas linhas cada faziam a raiz ser uma leitura, não um escaneamento. Quem
  // segue com segunda linha é o MOTIVO da linha desabilitada (informação de estado, não enfeite).
  // Presentacional pura — quem sabe o que a linha ABRE e o SettingsModal.
  import ConfigIcone from './ConfigIcone.svelte';
  interface Props {
    icone: string;
    rotulo: string;
    /** Linha que existe mas nao da pra abrir agora (ex.: sem servidor alvo no celular). */
    desabilitada?: boolean;
    /** Por que esta desabilitada — texto VISIVEL, nao tooltip: em tela de toque nao ha hover. */
    motivo?: string;
    onPick: () => void;
  }
  let { icone, rotulo, desabilitada = false, motivo, onPick }: Props = $props();
</script>

<button class="sr" disabled={desabilitada} onclick={onPick}>
  <span class="sr-icone" aria-hidden="true"><ConfigIcone nome={icone} /></span>
  <span class="sr-txt">
    <span class="sr-rotulo">{rotulo}</span>
    {#if desabilitada && motivo}
      <span class="sr-desc sr-motivo">{motivo}</span>
    {/if}
  </span>
  {#if !desabilitada}<span class="sr-chevron" aria-hidden="true">›</span>{/if}
</button>

<style>
  .sr {
    /* justify-content: o `button` global e inline-flex CENTRADO (app.css) — sem isto a linha inteira
       centraliza e o icone para num x diferente a cada item (bug ja visto no modal de git). */
    display: flex; align-items: center; justify-content: flex-start; gap: var(--space-3);
    width: 100%; min-height: 46px; padding: var(--space-1) var(--space-3);
    border: 0; background: transparent; color: var(--text-primary);
    font-size: var(--text-sm); text-align: left; cursor: pointer;
    transition: transform 160ms ease-out;
  }
  @media (hover: hover) { .sr:not(:disabled):hover { background: var(--bg-hover); } }
  .sr:not(:disabled):active { transform: scale(0.99); }
  .sr:disabled { color: var(--text-muted); cursor: default; }
  .sr-icone { flex-shrink: 0; width: 1.6em; display: grid; place-items: center;
              color: var(--text-secondary); }
  .sr:disabled .sr-icone { opacity: 0.5; }
  .sr-txt { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
  .sr-rotulo { color: inherit; }
  .sr-desc { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.3; }
  .sr-motivo { color: var(--warning); }
  .sr-chevron { flex-shrink: 0; color: var(--text-muted); font-size: var(--text-base); line-height: 1; }
</style>
