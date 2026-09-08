<script lang="ts">
  import { untrack } from 'svelte';
  import type { Server } from '../../lib/auth';
  import { criarConfigServidor } from '../../lib/serverConfig.svelte';
  import * as m from '../../paraglide/messages';
  import BottomSheet from '../BottomSheet.svelte';
  import LinhaConfig from './LinhaConfig.svelte';

  interface Props { apiTarget: Server | null; nome: string; onClose: () => void }
  let { apiTarget, nome, onClose }: Props = $props();
  const store = criarConfigServidor(() => apiTarget);
  const titulo = $derived(m.sessao_aria_opcoes({ n: nome }));

  $effect(() => {
    apiTarget;
    untrack(() => { void store.carregar(); });
    return () => store.invalidar();
  });
</script>

<BottomSheet open={true} {onClose} ariaLabel={titulo} wide centered>
  <div class="opcoes">
    <header>
      <h2>{titulo}</h2>
      <button type="button" class="fechar" onclick={onClose} aria-label={m.sessao_fechar()}>×</button>
    </header>
    {#if store.carregando}
      <p role="status">{m.comum_carregando()}</p>
    {:else if store.campos.claude_statusline_update}
      <LinhaConfig {store} campo={{
        chave: 'claude_statusline_update', tipo: 'liga',
        rotulo: m.harness_claude_statusline_atualizar(), ajuda: m.harness_claude_statusline_ajuda(),
      }} />
      <footer>
        {#if store.salvo}<span role="status">{m.arq_salvo()}</span>{/if}
        <button type="button" class="btn" onclick={() => void store.salvar()}
          disabled={!store.temMudanca || store.salvando}>
          {store.salvando ? m.arq_salvando() : m.ctx_salvar()}
        </button>
      </footer>
    {:else if !store.erro}
      <p role="status">{m.harness_opcoes_indisponiveis()}</p>
    {/if}
    {#if store.erro}
      <p class="erro" role="alert">{store.erro}</p>
      {#if !store.campos.claude_statusline_update}
        <button type="button" class="btn" onclick={() => void store.carregar()}>{m.config_server_tentar_de_novo()}</button>
      {/if}
    {/if}
  </div>
</BottomSheet>

<style>
  .opcoes { container-type: inline-size; padding: var(--space-4); }
  header { display: flex; align-items: center; gap: var(--space-3); }
  h2 { flex: 1; margin: 0; font-size: var(--text-lg); color: var(--text-primary); }
  .fechar { min-width: 44px; min-height: 44px; background: transparent; color: var(--text-secondary); border: 0; font-size: var(--text-xl); }
  p, footer span { color: var(--text-secondary); font-size: var(--text-sm); }
  .erro { color: var(--error); }
  footer { display: flex; align-items: center; justify-content: flex-end; gap: var(--space-3); margin-top: var(--space-4); }
  .btn { min-height: 44px; padding: var(--space-2) var(--space-4); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm); background: var(--surface-raised); color: var(--text-primary); }
  @container (max-width: 380px) { h2 { font-size: var(--text-base); } }
</style>
