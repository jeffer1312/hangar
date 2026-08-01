<script lang="ts">
  import { lerMaosLivres, setMaosLivres } from '../../lib/maosLivres';

  let ligado = $state(lerMaosLivres());
</script>

<div class="dt">
  <div class="dt-row">
    <div class="dt-label">
      <strong>Mãos-livres</strong>
      <span>o silêncio encerra a gravação e a mensagem vai sozinha, com 3s pra cancelar</span>
    </div>
    <label>
      <input class="switch" type="checkbox" bind:checked={ligado}
             onchange={() => setMaosLivres(ligado)} />
      <span class="sr-only">Ditado mãos-livres</span>
    </label>
  </div>
  <p class="dt-nota">
    Guardado só neste aparelho. Depois de enviar, o microfone não religa sozinho — toque de novo
    para falar outra vez.
  </p>
</div>

<style>
  /* Container query, NUNCA media query: quem aperta a linha e a largura do PAINEL, e no desktop ele
     tem ~530px mesmo num monitor de 1440. Sem isto a descricao quebra em uma palavra por linha —
     esta e a regra registrada no CLAUDE.md e no comentario do AppearanceSettings. */
  .dt { container-type: inline-size; display: flex; flex-direction: column; gap: var(--space-4); }
  .dt-row { display: flex; align-items: center; justify-content: space-between; gap: var(--space-4); }
  .dt-label { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .dt-label strong { font-size: var(--text-sm); }
  .dt-label span { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.35; }
  .dt-nota { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.45; margin: 0; }
  @container (max-width: 560px) {
    .dt-row { flex-direction: column; align-items: flex-start; gap: var(--space-2); }
  }
</style>
