<script lang="ts">
  // Campo de busca com lupa e o segmentado Nomes | Conteúdo — desenho do mock aprovado
  // (docs/mocks/2026-08-15-arvore/base.css, classes .busca e .seg). O atraso de digitação
  // (250ms) mora aqui; quem chama a rede é o store, via onBusca.
  import * as m from '../../paraglide/messages';
  import { onDestroy } from 'svelte';

  export type ModoBusca = 'names' | 'contents';

  export interface Props {
    q: string;
    mode: ModoBusca;
    onBusca: (q: string, mode: ModoBusca) => void;
  }

  let { q, mode, onBusca }: Props = $props();

  // Rascunho do campo: começa em q e acompanha as mudanças externas da prop (troca de
  // sessão, limpeza). A digitação vive aqui — o clique de aba usa ELE, nunca a prop q,
  // que pode não ter ecoado o que o usuário ainda está vendo.
  // A inicialização via onMount (e a guarda no $effect) evita o warning do svelte-check
  // de "referência captura só o valor inicial" e o reset no primeiro flush.
  // Rascunho do campo: começa em q e acompanha as mudanças externas da prop (troca de
  // sessão, limpeza). A digitação vive aqui — o clique de aba usa ELE, nunca a prop q,
  // que pode não ter ecoado o que o usuário ainda está vendo.
  // svelte-ignore state_referenced_locally — captura intencional do valor inicial de q;
  // as mudanças posteriores da prop são sincronizadas pela guarda no $effect abaixo.
  let texto = $state(q);
  // svelte-ignore state_referenced_locally — idem: qAnterior guarda a última prop vista.
  let qAnterior = q;
  $effect(() => {
    if (q !== qAnterior) {
      // q mudou por fora: o debounce pendente é da sessão anterior — cancela e sincroniza.
      if (timer) {
        clearTimeout(timer);
        timer = null;
      }
      texto = q;
      qAnterior = q;
    }
  });

  let timer: ReturnType<typeof setTimeout> | null = null;

  function digitar(e: Event) {
    texto = (e.currentTarget as HTMLInputElement).value;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      onBusca(texto, mode);
    }, 250);
  }

  function escolherModo(novo: ModoBusca) {
    if (novo === mode) return;
    // O clique descarta a digitação pendente e busca já no modo novo, com o termo que o
    // usuário está vendo (o rascunho), em vez de buscar duas vezes.
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    onBusca(texto, novo);
  }

  onDestroy(() => {
    if (timer) clearTimeout(timer);
  });
</script>

<div class="busca">
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></svg>
  <input
    type="text"
    value={texto}
    placeholder={m.arq_buscar()}
    aria-label={m.arq_buscar()}
    oninput={digitar}
  />
</div>
<div class="seg" role="group" aria-label={m.arq_buscar()}>
  <button type="button" class:sel={mode === 'names'} aria-pressed={mode === 'names'} onclick={() => escolherModo('names')}>
    {m.arq_modo_nomes()}
  </button>
  <button type="button" class:sel={mode === 'contents'} aria-pressed={mode === 'contents'} onclick={() => escolherModo('contents')}>
    {m.arq_modo_conteudo()}
  </button>
</div>

<style>
  /* Barra: docs/mocks/2026-08-15-arvore/base.css (.busca e .seg), token por token. */
  .busca {
    margin: 0 10px 6px;
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--surface-inset);
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    padding: 6px 8px;
  }
  .busca:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }
  .busca svg {
    width: 13px;
    height: 13px;
    color: var(--text-muted);
    flex: none;
  }
  .busca input {
    flex: 1;
    min-width: 0;
    background: none;
    border: 0;
    outline: none;
    color: var(--text-primary);
    font: inherit;
    font-size: 12.5px;
  }
  .busca input::placeholder {
    color: var(--text-muted);
  }

  /* Segmentado Nomes | Conteúdo — mesmo desenho do seletor de esforço do app. */
  .seg {
    margin: 0 10px 8px;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 2px;
    background: var(--fill-subtle);
    border-radius: 8px;
    padding: 2px;
  }
  .seg button {
    appearance: none;
    border: 0;
    background: none;
    color: var(--text-muted);
    font: inherit;
    font-size: 12px;
    padding: 5px 0;
    border-radius: 6px;
    cursor: pointer;
    /* Sobrescreve o alvo global de 44px (app.css) — o segmentado e compacto por desenho
       (mesmo do seletor de esforco do app). */
    min-height: 0;
    min-width: 0;
  }
  .seg button.sel {
    background: var(--surface-raised);
    color: var(--text-primary);
  }
  .seg button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
</style>
