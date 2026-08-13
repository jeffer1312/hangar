<script lang="ts">
  import SegmentedPicker from '../SegmentedPicker.svelte';
  import * as m from '../../paraglide/messages';
  import { preferenciaSalva, aplicarPreferencia, type Preferencia } from '../../lib/locale';

  // Tela Geral: opcoes gerais do app. Hoje so tem o idioma; as proximas opcoes soltas do backlog
  // caem aqui como mais uma linha, no mesmo formato (rotulo + descricao + controle).
  // `preferencia` e $state e nao preferenciaSalva() direto: aquela funcao le localStorage por
  // chamada comum, sem sinal reativo, entao o bloco nunca reavaliaria — mesmo remedio do `tema` na
  // AppearanceSettings. Trocar de idioma recarrega a pagina (decisao da Task 1), entao nao ha
  // estado pra reconciliar depois do pick: a tela volta ja no idioma novo.
  let preferencia = $state<Preferencia>(preferenciaSalva());

  // Rotulos bilingues e fixos nos dois idiomas (config_idioma_*): quem caiu no idioma errado
  // precisa achar a saida sem saber ler o idioma em que esta. Portugues e English ficam com o
  // proprio nome; "Seguir o sistema" e a unica opcao que traduz de verdade.
  const opcoes: { v: Preferencia; label: string; aria: string }[] = [
    { v: 'sistema', label: m.config_idioma_sistema(), aria: m.config_idioma_sistema() },
    { v: 'pt', label: m.config_idioma_pt(), aria: m.config_idioma_pt() },
    { v: 'en', label: m.config_idioma_en(), aria: m.config_idioma_en() },
  ];
</script>

<div class="gs">
  <div class="gs-row">
    <div class="gs-label">
      <strong>{m.config_idioma_rotulo()}</strong>
      <span>{m.config_idioma_nota_reload()}</span>
    </div>
    <SegmentedPicker value={preferencia} options={opcoes} ariaLabel={m.config_idioma_rotulo()}
                     onPick={(v) => aplicarPreferencia(v)} />
  </div>
</div>

<style>
  /* Container query, nao media query: quem aperta a linha e a largura do PAINEL, nao a da janela.
     No desktop o modal tem ~1100px num monitor de 1440, mas o painel e redimensionavel — o corte
     tem que seguir a largura de verdade. Mesma regra registrada no CLAUDE.md e nos comentarios da
     AppearanceSettings e da DictationSettings. */
  .gs { container-type: inline-size; }
  /* Mesmos tokens da linha do segmentado de Tema (AppearanceSettings): padding, borda inferior e
     alinhamento no topo — o conteudo tem que respirar igual aos irmaos, senao o texto cola nas
     bordas e o controle desalinha (medido na comparacao cega com a barra). */
  .gs-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border-subtle);
  }
  .gs-row:last-child { border-bottom: 0; }
  .gs-label { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .gs-label strong { color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .gs-label span { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.35; }
  @container (max-width: 560px) {
    .gs-row { flex-direction: column; align-items: flex-start; gap: var(--space-2); }
  }
</style>
