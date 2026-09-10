<script lang="ts">
  import type { Server } from '../../lib/auth';
  import { codexOpcoes, type CodexOpcoes } from '../../lib/credenciais';
  import * as m from '../../paraglide/messages';
  import BottomSheet from '../BottomSheet.svelte';

  let { apiTarget, nome, onClose }: { apiTarget: Server | null; nome: string; onClose: () => void } = $props();
  let dado = $state<CodexOpcoes | null>(null);
  let habilitado = $state(false);
  let vozBeta = $state(false);
  let ocupado = $state(false);
  let erro = $state('');
  let salvo = $state(false);
  let ctx: { alvo: Server | null; controle: AbortController };
  const titulo = $derived(m.sessao_aria_opcoes({ n: nome }));

  async function consultar(contexto: typeof ctx, gravar = false) {
    ocupado = true; erro = ''; salvo = false;
    try {
      const resposta = await codexOpcoes(contexto.alvo, contexto.controle.signal,
        gravar ? { contexto_estendido: habilitado, codex_voice_beta: vozBeta } : undefined);
      if (ctx !== contexto || contexto.controle.signal.aborted) return;
      dado = resposta; habilitado = resposta.contexto_estendido; vozBeta = resposta.codex_voice_beta; salvo = gravar;
      if (gravar) window.dispatchEvent(new CustomEvent('hangar:codex-voice-config', {
        detail: { serverId: contexto.alvo?.id ?? null, enabled: resposta.codex_voice_beta },
      }));
    } catch (e) {
      if (ctx === contexto && !contexto.controle.signal.aborted) erro = e instanceof Error ? e.message : m.comum_falha_aplicar();
    } finally { if (ctx === contexto) ocupado = false; }
  }

  $effect(() => {
    const contexto = { alvo: apiTarget, controle: new AbortController() };
    ctx = contexto; dado = null;
    void consultar(contexto);
    return () => contexto.controle.abort();
  });
</script>

<BottomSheet open={true} {onClose} ariaLabel={titulo} wide centered>
  <div class="opcoes">
    <header><h2>{titulo}</h2><button class="fechar" onclick={onClose} aria-label={m.sessao_fechar()}>×</button></header>
    {#if dado}
      <label class="campo">
        <span><b>{m.codex_contexto_titulo()}</b><small>{m.codex_contexto_ajuda()}</small></span>
        <input id="codex-contexto-estendido" type="checkbox" role="switch" bind:checked={habilitado} disabled={ocupado} />
      </label>
      <label class="campo">
        <span><b>{m.codex_voice_config_title()} <i class="beta">{m.comum_beta()}</i></b><small>{m.codex_voice_config_help()}</small></span>
        <input id="codex-voice-beta" type="checkbox" role="switch" bind:checked={vozBeta} disabled={ocupado} />
      </label>
      <p>{m.codex_contexto_novas()}</p>
      {#if dado.modelos.length}
        <p>{m.codex_contexto_limites()}</p>
        <ul>{#each dado.modelos as modelo (modelo.model)}<li>{modelo.model}: {modelo.max.toLocaleString()}</li>{/each}</ul>
      {:else}<p>{m.codex_contexto_sem_catalogo()}</p>{/if}
      {#if dado.compactacao}<p>{m.codex_contexto_compactacao({ n: dado.compactacao.toLocaleString() })}</p>{/if}
      <footer>
        {#if salvo}<span role="status">{m.arq_salvo()}</span>{/if}
        <button class="btn" disabled={ocupado || habilitado === dado.contexto_estendido && vozBeta === dado.codex_voice_beta} onclick={() => consultar(ctx, true)}>
          {ocupado ? m.arq_salvando() : m.ctx_salvar()}
        </button>
      </footer>
    {:else if ocupado}<p role="status">{m.comum_carregando()}</p>{/if}
    {#if erro}
      <p class="erro" role="alert">{erro}</p>
      {#if !dado}<button class="btn" onclick={() => consultar(ctx)}>{m.config_server_tentar_de_novo()}</button>{/if}
    {/if}
  </div>
</BottomSheet>

<style>
  .opcoes { container-type: inline-size; padding: var(--space-4); }
  header, .campo, footer { display: flex; align-items: center; gap: var(--space-3); }
  h2 { flex: 1; font-size: var(--text-lg); }
  .campo { justify-content: space-between; margin-top: var(--space-4); }
  .campo span { flex: 1; min-width: 0; }
  small { display: block; margin-top: var(--space-2); }
  small, p, li, footer span { color: var(--text-secondary); font-size: var(--text-sm); }
  .beta { display: inline-block; margin-left: var(--space-1); padding: 1px 6px; border-radius: var(--radius-full);
    background: var(--accent-dim); color: var(--accent); font-size: 10px; font-style: normal; text-transform: uppercase; }
  li { overflow-wrap: anywhere; }
  input { width: 24px; height: 24px; accent-color: var(--accent); }
  .fechar { min-width: 44px; min-height: 44px; border: 0; background: transparent; color: var(--text-secondary); font-size: var(--text-xl); }
  footer { justify-content: flex-end; margin-top: var(--space-4); }
  .btn { min-height: 44px; padding: var(--space-2) var(--space-4); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm); background: var(--surface-raised); color: var(--text-primary); }
  .erro { color: var(--error); }
  @container (max-width: 380px) { h2 { font-size: var(--text-base); } }
</style>
