<script lang="ts">
  // Confirmação do gesto de arrastar sessão sobre sessão (agrupar) ou soltar do grupo (sair).
  // Montado UMA vez no shell (App.svelte), reagindo a arrastarGrupo.pedido — as quatro superfícies
  // que arrastam (Sidebar/Board/Canvas/celular, Task 3+) só chamam arrastarGrupo.soltar/pedirSaida.
  import { untrack } from 'svelte';
  import ModalDialog from './ModalDialog.svelte';
  import { arrastarGrupo, mensagemRecusa, type ChaveSessao } from '../lib/arrastarGrupo.svelte';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { withServer } from '../lib/auth';
  import { pairSession, unpairSession, suggestGroupTask, formataErro, canPair } from '@hangar/core';
  import type { AggSession } from '@hangar/core';
  import * as m from '../paraglide/messages';

  const pedido = $derived(arrastarGrupo.pedido);

  // Relido do store a CADA render — nunca o objeto capturado no clique: se o SSE mudar o grupo (ou
  // a sessão morrer) durante a confirmação, é este dado vivo que decide.
  function porChave(chave: ChaveSessao | undefined): AggSession | null {
    if (!chave) return null;
    return sessionsStore.rows.find((s) => s.serverId === chave.serverId && s.name === chave.name) ?? null;
  }

  const origemSessao = $derived(porChave(pedido?.origem));
  const alvoSessao = $derived.by(() => {
    const p = pedido;
    return p && p.modo === 'agrupar' ? porChave(p.alvo) : null;
  });

  // join_group funde os grupos INTEIROS (api.py:3581): a lista é a união de origem, alvo e os
  // pares de cada um — mostrar só os dois nomes escondia quem mais o arrasto afeta.
  const afetados = $derived.by(() => {
    if (pedido?.modo !== 'agrupar') return [];
    const nomes = new Set<string>();
    if (origemSessao) {
      nomes.add(origemSessao.name);
      for (const p of origemSessao.pair_peers ?? []) nomes.add(p);
    }
    if (alvoSessao) {
      nomes.add(alvoSessao.name);
      for (const p of alvoSessao.pair_peers ?? []) nomes.add(p);
    }
    return [...nomes];
  });

  // Sessão avulsa + grupo que já existe = só entrar, herdando a tarefa do grupo. Campo e "Sugerir"
  // ficam pra grupo que nasce agora ou dois grupos que se fundem.
  const grupoExistente = $derived.by((): AggSession | null => {
    if (pedido?.modo !== 'agrupar' || !origemSessao || !alvoSessao) return null;
    const emGrupo = (s: AggSession) => (s.pair_peers?.length ?? 0) > 0;
    if (emGrupo(origemSessao) === emGrupo(alvoSessao)) return null;
    return emGrupo(alvoSessao) ? alvoSessao : origemSessao;
  });
  // Com a chamada em voo o SSE já mostra a entrada feita; a tela fica como estava no clique.
  let grupoNoClique = $state<AggSession | null>(null);

  const quemFica = $derived(pedido?.modo === 'sair' ? (origemSessao?.pair_peers ?? []) : []);

  // Recheca canPair contra o dado VIVO — pode ter deixado de valer entre abrir o pedido e
  // confirmar (sessão morreu, grupo mudou por outro caminho). Sessão sumida da lista conta como
  // 'dead': não há mais o que mostrar dela.
  const bloqueio = $derived.by((): string | null => {
    if (!pedido) return null;
    if (pedido.modo === 'sair') return origemSessao ? null : mensagemRecusa('dead');
    if (!origemSessao || !alvoSessao) return mensagemRecusa('dead');
    const r = canPair(origemSessao, alvoSessao);
    return r.ok ? null : mensagemRecusa(r.reason);
  });

  let tarefa = $state('');
  let busy = $state(false);
  let sugerindo = $state(false);
  let erro = $state<string | null>(null);
  let conflito = $state<string | null>(null); // mensagem do 409: troca o botão de confirmar pelo de sobrescrever
  // true quando o backend já fez a fusão/saída mas o `warning` diz que o aviso não chegou a
  // alguém do grupo — a ação está feita, só resta fechar (ver template).
  let avisoFalhou = $state(false);

  // Reinicia o formulário só quando um PEDIDO NOVO abre (a referência de `pedido` muda uma vez por
  // arrasto) — nunca a cada recompute do sessionsStore: um poll no meio da digitação não pode
  // apagar o campo. `untrack` pelo mesmo motivo do chainOpen no SessionContextMenu.
  $effect(() => {
    const p = pedido;
    if (!p) return;
    untrack(() => {
      tarefa = p.modo === 'agrupar' ? (alvoSessao?.pair_task || origemSessao?.pair_task || '') : '';
      busy = false;
      sugerindo = false;
      erro = null;
      conflito = null;
      avisoFalhou = false;
      grupoNoClique = null;
    });
  });

  // Esc/fundo do ModalDialog chamam isto sem saber de `busy` — com um pairSession/unpairSession em
  // voo, soltar o pedido aqui jogaria o resultado (warning, 409, erro) no vazio. Só dissolve quando
  // não há chamada pendente; enquanto busy, o botão Cancelar já fica desabilitado pelo mesmo motivo.
  function fechar() {
    if (busy) return;
    arrastarGrupo.cancelar();
  }

  async function confirmarAgrupar(substituir: boolean) {
    if (pedido?.modo !== 'agrupar' || busy) return;
    // Identidade capturada ANTES do await: se um arrasto novo abrir outro pedido enquanto esta
    // chamada está em voo, a resposta da chamada VELHA não pode mexer no diálogo do pedido novo.
    const pedidoEmVoo = pedido;
    const o = origemSessao;
    const a = alvoSessao;
    if (!o || !a) { erro = mensagemRecusa('dead'); return; }
    const checagem = canPair(o, a);
    if (!checagem.ok) { erro = mensagemRecusa(checagem.reason); return; }
    grupoNoClique = grupoExistente;
    const herda = grupoExistente !== null;
    busy = true;
    erro = null;
    try {
      const res = await withServer(a.serverId, () => pairSession(a.name, [o.name], herda ? '' : tarefa.trim(), substituir));
      if (pedido !== pedidoEmVoo) return;
      if (res.warning) {
        conflito = null;
        avisoFalhou = true;
        erro = formataErro(res.warning) ?? String(res.warning);
      } else {
        arrastarGrupo.cancelar();
      }
    } catch (e) {
      if (pedido !== pedidoEmVoo) return;
      if (e instanceof Error && (e as { status?: number }).status === 409) {
        conflito = e.message;
      } else {
        conflito = null;
        erro = e instanceof Error && e.message ? e.message : m.grupo_drop_falhou();
      }
    } finally {
      if (pedido === pedidoEmVoo) busy = false;
    }
  }

  async function sugerir() {
    if (pedido?.modo !== 'agrupar' || busy || sugerindo || !alvoSessao) return;
    const pedidoEmVoo = pedido; // mesmo motivo do confirmarAgrupar
    const serverId = alvoSessao.serverId;
    const nomes = afetados;
    sugerindo = true;
    erro = null;
    try {
      const res = await withServer(serverId, () => suggestGroupTask(nomes));
      if (pedido !== pedidoEmVoo) return;
      tarefa = res.task;
      conflito = null;
    } catch (e) {
      if (pedido !== pedidoEmVoo) return;
      erro = e instanceof Error && e.message ? e.message : m.grupo_drop_falhou();
    } finally {
      if (pedido === pedidoEmVoo) sugerindo = false;
    }
  }

  async function confirmarSair() {
    if (pedido?.modo !== 'sair' || busy) return;
    const pedidoEmVoo = pedido; // mesmo motivo do confirmarAgrupar
    const o = origemSessao;
    if (!o) { erro = mensagemRecusa('dead'); return; }
    busy = true;
    erro = null;
    try {
      const res = await withServer(o.serverId, () => unpairSession(o.name));
      if (pedido !== pedidoEmVoo) return;
      if (res.warning) {
        avisoFalhou = true;
        erro = formataErro(res.warning) ?? String(res.warning);
      } else {
        arrastarGrupo.cancelar();
      }
    } catch (e) {
      if (pedido !== pedidoEmVoo) return;
      erro = e instanceof Error && e.message ? e.message : m.grupo_drop_sair_falhou();
    } finally {
      if (pedido === pedidoEmVoo) busy = false;
    }
  }
</script>

{#if pedido}
  <ModalDialog
    open={true}
    ariaLabel={pedido.modo === 'agrupar'
      ? (avisoFalhou ? m.grupo_drop_titulo_feito() : m.grupo_drop_titulo())
      : (avisoFalhou ? m.grupo_drop_sair_titulo_feito() : m.grupo_drop_sair_titulo())}
    onClose={fechar}
    className="grupo-drop"
  >
    {#if pedido.modo === 'agrupar'}
      <h2 class="gd-title">{avisoFalhou ? m.grupo_drop_titulo_feito() : m.grupo_drop_titulo()}</h2>

      <div class="gd-afetadas">
        <p class="gd-label">{m.grupo_drop_afetadas()}</p>
        <ul class="gd-lista">
          {#each afetados as nome (nome)}<li>{nome}</li>{/each}
        </ul>
      </div>

      {@const grupo = busy ? grupoNoClique : grupoExistente}
      {#if !avisoFalhou && grupo}
        {#if grupo.pair_task}
          <p class="gd-label">{m.grupo_drop_tarefa_herdada({ tarefa: grupo.pair_task })}</p>
        {/if}
      {:else if !avisoFalhou}
        <div class="gd-tarefa-linha">
          <input
            type="text"
            class="gd-tarefa"
            bind:value={tarefa}
            oninput={() => { conflito = null; }}
            placeholder={m.grupo_drop_tarefa()}
            disabled={busy || sugerindo}
          />
          <button type="button" class="gd-btn gd-sugerir" onclick={sugerir} disabled={busy || sugerindo || !!bloqueio}>
            {sugerindo ? m.grupo_drop_sugerindo() : m.grupo_drop_sugerir()}
          </button>
        </div>
      {/if}

      {#if erro}<p class="gd-erro">{erro}</p>{/if}
      {#if conflito}<p class="gd-erro">{conflito}</p>{/if}
      {#if bloqueio && !erro && !conflito && !busy}<p class="gd-erro">{bloqueio}</p>{/if}

      <div class="gd-acoes">
        {#if avisoFalhou}
          <button type="button" class="gd-btn gd-primary" onclick={fechar}>{m.sessao_fechar()}</button>
        {:else}
          <button type="button" class="gd-btn" onclick={fechar} disabled={busy}>{m.comum_cancelar()}</button>
          {#if conflito}
            <button type="button" class="gd-btn gd-primary" onclick={() => confirmarAgrupar(true)} disabled={busy || sugerindo || !!bloqueio}>
              {m.grupo_drop_substituir_tarefa()}
            </button>
          {:else}
            <button type="button" class="gd-btn gd-primary" onclick={() => confirmarAgrupar(false)} disabled={busy || sugerindo || !!bloqueio}>
              {m.grupo_drop_confirmar()}
            </button>
          {/if}
        {/if}
      </div>
    {:else}
      <h2 class="gd-title">{avisoFalhou ? m.grupo_drop_sair_titulo_feito() : m.grupo_drop_sair_titulo()}</h2>

      {#if quemFica.length}
        <div class="gd-afetadas">
          <p class="gd-label">{m.grupo_drop_afetadas()}</p>
          <ul class="gd-lista">
            {#each quemFica as nome (nome)}<li>{nome}</li>{/each}
          </ul>
        </div>
      {/if}

      {#if erro}<p class="gd-erro">{erro}</p>{/if}
      {#if bloqueio && !erro}<p class="gd-erro">{bloqueio}</p>{/if}

      <div class="gd-acoes">
        {#if avisoFalhou}
          <button type="button" class="gd-btn gd-danger" onclick={fechar}>{m.sessao_fechar()}</button>
        {:else}
          <button type="button" class="gd-btn" onclick={fechar} disabled={busy}>{m.comum_cancelar()}</button>
          <button type="button" class="gd-btn gd-danger" onclick={confirmarSair} disabled={busy || !!bloqueio}>
            {m.grupo_drop_sair_confirmar()}
          </button>
        {/if}
      </div>
    {/if}
  </ModalDialog>
{/if}

<style>
  :global(.grupo-drop) {
    width: min(420px, 92vw);
    padding: var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .gd-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .gd-afetadas { display: flex; flex-direction: column; gap: var(--space-1); }
  .gd-label { font-size: var(--text-sm); color: var(--text-secondary); }
  .gd-lista { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
  .gd-lista li { font-size: var(--text-sm); color: var(--text-primary); }
  .gd-tarefa-linha { display: flex; gap: var(--space-2); }
  .gd-tarefa {
    flex: 1;
    min-width: 0;
    height: 44px;
    padding: 0 var(--space-3);
    background: var(--surface-card);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px;
  }
  .gd-tarefa:focus { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-dim); }
  .gd-tarefa::placeholder { color: var(--text-muted); }
  .gd-erro { font-size: var(--text-sm); color: #e5484d; }
  .gd-acoes { display: flex; gap: var(--space-2); }
  .gd-btn {
    flex: 1;
    height: 40px;
    border-radius: var(--radius-md);
    font-size: var(--text-sm);
    font-weight: 600;
    background: var(--bg-hover);
    color: var(--text-secondary);
    border: 0;
    cursor: pointer;
  }
  .gd-btn:hover { background: var(--bg-surface); }
  .gd-btn:disabled { opacity: 0.5; cursor: default; }
  .gd-sugerir { flex: 0 0 auto; height: 44px; padding: 0 var(--space-3); }
  .gd-primary { background: var(--accent); color: #fff; }
  .gd-danger { background: var(--error); color: #fff; }
</style>
