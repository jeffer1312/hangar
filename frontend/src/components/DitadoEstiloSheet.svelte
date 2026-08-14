<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { ditadoEstilo, ESTILOS, type EstiloDitado } from '../lib/ditadoEstilo.svelte';

  // Escolha de quanto o ditado pode mexer no que você falou. Fica ao lado do microfone, não nas
  // Configurações: é decisão que muda ANTES de falar ("isto é um comando" vs "isto é um pedido
  // inteiro"), e ninguém abre modal pra isso. O que for escolhido aqui também vale pro atalho
  // Ctrl+Espaço, porque quem lê o estilo é o backend, na hora de limpar.
  interface Props {
    open: boolean;
    onClose: () => void;
    isDesktop?: boolean;
  }
  let { open, onClose, isDesktop = false }: Props = $props();

  let erro = $state('');

  async function escolher(v: EstiloDitado) {
    erro = '';
    try {
      await ditadoEstilo.trocar(v);
      onClose();
    } catch (e) {
      // A folha FICA ABERTA no erro. Fechar mostrando o estilo antigo (o store já desfez) deixaria
      // a pessoa achando que trocou — falha calada é o que a regra do projeto proíbe.
      erro = e instanceof Error ? e.message : 'não deu pra salvar a escolha';
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Estilo do ditado" centered={isDesktop}>
  <h2 class="sheet-title">Estilo do ditado</h2>
  <p class="intro">O que fazer com a sua fala depois de transcrever. Vale também pro Ctrl+Espaço.</p>

  <div class="opcoes">
    {#each ESTILOS as e (e.valor)}
      <button
        class="op"
        class:sel={ditadoEstilo.valor === e.valor}
        onclick={() => escolher(e.valor)}
        aria-pressed={ditadoEstilo.valor === e.valor}
      >
        <span class="op-topo">
          <strong>{e.rotulo}</strong>
          {#if ditadoEstilo.valor === e.valor}<span class="marca">em uso</span>{/if}
        </span>
        <span class="op-hint">{e.hint}</span>
      </button>
    {/each}
  </div>

  {#if erro}<p class="erro">{erro}</p>{/if}
</BottomSheet>

<style>
  .sheet-title { margin: 0 0 var(--space-2); font-size: var(--text-md); }
  .intro {
    margin: 0 0 var(--space-4);
    color: var(--text-muted);
    font-size: var(--text-xs);
    line-height: 1.45;
  }
  .opcoes { display: flex; flex-direction: column; gap: var(--space-2); }
  .op {
    display: flex;
    flex-direction: column;
    /* flex-start explícito: o reset de <button> do app centraliza os filhos, e sem isto o título e
       a explicação nasciam centralizados — fora do padrão de toda lista de opção do app. */
    align-items: flex-start;
    gap: 3px;
    padding: var(--space-3);
    text-align: left;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    /* transparent, não --bg-elevated: quem carrega o material é a folha. Superfície própria aqui
       viraria retângulo chapado boiando sobre o papel de parede (regra de transparência do app). */
    background: transparent;
    color: inherit;
    cursor: pointer;
    transition: border-color var(--dur-fast) var(--ease-out), background var(--dur-fast) var(--ease-out);
  }
  .op:hover { background: var(--bg-elevated); }
  /* Realce de ESTADO usa tinta por cima, não superfície — por isso --bg-elevated e não --surface-*. */
  .op.sel { border-color: var(--accent); background: var(--bg-elevated); }
  .op-topo { display: flex; align-items: center; gap: var(--space-2); }
  .op-topo strong { font-size: var(--text-sm); }
  .marca {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--accent);
  }
  .op-hint { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4; }
  .erro { margin: var(--space-3) 0 0; color: var(--danger); font-size: var(--text-xs); }
</style>
