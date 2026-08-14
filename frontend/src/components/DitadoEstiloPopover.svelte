<script lang="ts">
  // Estilo do ditado, na MESMA forma do ClaudeEffortPopover: pill ancorada, lista com tique na
  // escolhida e uma dica no rodape. Nasceu como folha modal e estava errado — "não é pra abrir um
  // modal, junta ele ao microfone, no mesmo estilo". Escolher o estilo do ditado é do mesmo
  // tamanho que escolher o esforço: um toque, uma lista curta, sem cobrir a tela.
  import Popover from './Popover.svelte';
  import { ditadoEstilo, ESTILOS, type EstiloDitado } from '../lib/ditadoEstilo.svelte';

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    onClose: () => void;
  }
  let { open, anchor, onClose }: Props = $props();

  let err = $state<string | null>(null);
  let aplicando = $state<EstiloDitado | null>(null);

  // Zera junto com o erro: fechar com pedido em voo travava a lista na reabertura (mesmo cuidado
  // do popover de esforço).
  $effect(() => {
    if (open) { err = null; aplicando = null; }
  });

  async function escolher(v: EstiloDitado) {
    if (aplicando) return;
    aplicando = v;
    err = null;
    try {
      await ditadoEstilo.trocar(v);
    } catch (e) {
      // Fica ABERTO no erro: fechar mostrando o estilo antigo (o store já desfez) faria a pessoa
      // achar que trocou.
      err = e instanceof Error ? e.message : 'Falha ao salvar';
      aplicando = null;
      return;
    }
    aplicando = null;
    onClose();
  }
</script>

<Popover {open} {anchor} {onClose} width={230} ariaLabel="Estilo do ditado">
  {#if err}
    <p class="err" role="alert">{err}</p>
  {/if}

  <ul class="lista">
    {#each ESTILOS as e (e.valor)}
      <li>
        <button
          class="linha"
          class:ativa={ditadoEstilo.valor === e.valor}
          aria-pressed={ditadoEstilo.valor === e.valor}
          disabled={!!aplicando}
          data-foco={ditadoEstilo.valor === e.valor ? true : undefined}
          onclick={() => escolher(e.valor)}
          title={e.hint}
        >
          <span class="nome">{e.rotulo}</span>
          {#if aplicando === e.valor}
            <span class="tick" aria-hidden="true">…</span>
          {:else if ditadoEstilo.valor === e.valor}
            <svg class="tick" width="16" height="16" viewBox="0 0 24 24" fill="none"
              stroke="currentColor" stroke-width="2.5" stroke-linecap="round"
              stroke-linejoin="round" aria-hidden="true">
              <polyline points="20 6 9 17 4 12" />
            </svg>
          {/if}
        </button>
      </li>
    {/each}
  </ul>
  <p class="dica">Do mais fiel à sua fala ao mais estruturado.</p>
</Popover>

<style>
  /* Copiado do ClaudeEffortPopover de proposito: duas pills lado a lado no mesmo composer com a
     lista desenhada diferente e o que faz UI parecer amadora — o mesmo argumento que o app.css usa
     pro switch morar num lugar so. */
  .err { color: var(--error); font-size: var(--text-xs); margin: 8px 10px 0; }
  .lista { list-style: none; margin: 0; padding: 4px 0; overflow-y: auto; }

  .linha {
    display: flex; align-items: center; gap: 6px; width: 100%;
    padding: 6px 10px; background: transparent; border: none;
    color: var(--text-primary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .linha:hover:not(:disabled) { background: var(--bg-hover); }
  .linha.ativa:hover:not(:disabled) { background: var(--accent-dim); }
  .linha:disabled { cursor: default; }
  /* Marcacao pelo FUNDO, nao pela cor do texto: --accent sobre o papel do tema claro nao alcanca
     os 4,5:1 que o AA pede pra texto de 14px. O tique fica em --accent porque e grafico (3:1). */
  .linha.ativa { background: var(--accent-dim); color: var(--text-primary); }

  .nome { flex: 1; }
  .tick { flex: none; color: var(--accent); }

  .dica {
    font-size: var(--text-xs); color: var(--text-muted);
    padding: 6px 10px 8px; margin: 0; border-top: 1px solid var(--border-subtle);
  }
</style>
