<script lang="ts">
  import { applyCodexContext, applyToOwnServers, codexOpcoes, machineLabel, ownUpdatableServers, type ApplyResult, type Server } from '@hangar/core';
  import * as m from '../paraglide/messages';
  import { getActiveId, listOwnServers, listServers } from '../lib/auth';

  // `desktop`: a vista desktop web é legada e não ganha o "Em todas as minhas máquinas".
  let { server, busy = $bindable(false), desktop = false }: { server: Server | null; busy?: boolean; desktop?: boolean } = $props();
  let enabled = $state(false);
  // O valor que a pessoa ESCOLHEU enquanto a gravação não volta: o switch mostra ele na hora.
  // Desfazer a marca e esperar o servidor fazia o clique parecer não ter pegado.
  let pedido = $state<boolean | null>(null);
  let ready = $state(false);
  let error = $state('');
  let context: { server: Server | null; controller: AbortController };

  async function update(ctx: typeof context, value?: boolean) {
    busy = true;
    error = '';
    if (value !== undefined) pedido = value;
    try {
      const result = await codexOpcoes(ctx.server, ctx.controller.signal, value);
      if (context !== ctx || ctx.controller.signal.aborted) return;
      enabled = result.contexto_estendido;
      ready = true;
    } catch (e) {
      if (context === ctx && !ctx.controller.signal.aborted) {
        error = e instanceof Error ? e.message : m.comum_falha_aplicar();
      }
    } finally {
      // Falhou ou confirmou: quem manda de novo é o valor do servidor.
      if (context === ctx && !ctx.controller.signal.aborted) { busy = false; pedido = null; }
    }
  }

  // "Em todas as minhas máquinas": a mesma gravação nas outras máquinas próprias (a desta tela já
  // recebe a de cima). Máquina sem Codex instalado fica de fora e diz isso na linha dela.
  const others = () => {
    const here = server?.id ?? getActiveId();
    return ownUpdatableServers(listServers()).filter((s) => s.id !== here);
  };
  const hasOthers = $derived(!desktop && others().length > 0);
  let applyAll = $state(false);
  let applying = $state(false);
  let results = $state<ApplyResult[]>([]);
  let lastValue = false;
  // Rodadas em fila: cada uma só começa quando a anterior termina, e a superada antes de começar
  // não roda. Assim o último toque é o último POST em cada máquina e o último a escrever
  // `results`; em paralelo, a rodada antiga podia chegar depois e desfazer a nova.
  let round = 0;
  let queue: Promise<void> = Promise.resolve();

  async function applyOthers(value: boolean, only?: string) {
    const id = ++round;
    lastValue = value;
    applying = true;
    const previous = queue;
    let done = () => {};
    queue = new Promise<void>((resolve) => (done = resolve));
    try {
      await previous;
      if (id !== round) return;
      const fresh = await applyToOwnServers(others().filter((s) => !only || s.id === only), (s) => applyCodexContext(s, value));
      if (id !== round) return;
      results = only ? results.map((r) => fresh.find((f) => f.serverId === r.serverId) ?? r) : fresh;
    } finally {
      done();
      if (id === round) applying = false;
    }
  }
  function resultText(r: ApplyResult): string {
    switch (r.status) {
      case 'ok': return m.tags_apply_ok();
      case 'offline': return m.tags_apply_offline();
      case 'old': return m.tags_apply_old();
      case 'absent': return m.tags_apply_absent();
      case 'failed': return m.tags_apply_failed({ reason: r.error ?? '' });
    }
  }
  // Nome por id, refeito só quando chegam resultados.
  const labels = $derived.by(() => {
    void results;
    return new Map(listOwnServers().map((s) => [s.id, machineLabel(s)]));
  });

  $effect(() => {
    const ctx = { server, controller: new AbortController() };
    context = ctx;
    ready = false;
    pedido = null;
    void update(ctx);
    return () => { ctx.controller.abort(); busy = false; };
  });
</script>

<div class="context-control">
  <label class:carregando={!ready}>
    <input type="checkbox" role="switch" checked={pedido ?? enabled} disabled={busy || !ready}
      onchange={(e) => {
        const value = !enabled;
        e.currentTarget.checked = value;
        void update(context, value);
        if (applyAll && hasOthers) void applyOthers(value);
      }} />
    <span class="titulo">{m.codex_contexto_titulo()}</span>
    {#if busy && pedido !== null}<span class="estado" role="status">{m.codex_contexto_salvando()}</span>
    {:else if busy}<span class="estado" role="status">{m.comum_carregando()}</span>{/if}
  </label>
  <p>{m.codex_contexto_padrao()}</p>
  {#if hasOthers}
    <label class="all">
      <input type="checkbox" checked={applyAll} onchange={(e) => {
        applyAll = e.currentTarget.checked;
        // Marcar já espalha o valor de agora: sem isso o 1M ligado só iria às outras no próximo clique.
        if (applyAll && ready) void applyOthers(enabled); else results = [];
      }} />
      <span class="titulo">{m.tags_apply_all()}</span>
      {#if applying}<span class="estado">{m.codex_contexto_salvando()}</span>{/if}
    </label>
    {#if applyAll && results.length}
      <ul class="results">
        {#each results as r (r.serverId)}
          <li class:falha={r.status !== 'ok'}>
            <span>{labels.get(r.serverId) ?? r.serverId}: {resultText(r)}</span>
            {#if r.status === 'offline' || r.status === 'failed'}
              <button type="button" disabled={applying} onclick={() => applyOthers(lastValue, r.serverId)}>{m.tags_apply_retry()}</button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
  {#if error}
    <p role="alert">{error}</p>
    <button type="button" disabled={busy} onclick={() => update(context)}>{m.config_server_tentar_de_novo()}</button>
  {/if}
</div>

<style>
  .context-control { display: grid; grid-column: 1 / -1; gap: var(--space-2); }
  /* A linha inteira é o alvo do toque, com o switch colado ao nome: a caixa de 20px no canto
     oposto da linha era difícil de acertar. */
  label { display: flex; align-items: center; gap: var(--space-3); min-height: 44px;
          padding: 0 var(--space-2); margin: 0 calc(-1 * var(--space-2));
          border-radius: var(--radius-md); font-size: var(--text-sm); cursor: pointer; }
  label:hover { background: var(--surface-raised); }
  label.carregando { cursor: progress; }
  input { flex: none; width: 22px; height: 22px; margin: 0; accent-color: var(--accent); cursor: inherit; }
  .titulo { flex: 1; }
  .estado { font-size: var(--text-xs); color: var(--text-secondary); }
  p { margin: 0; font-size: var(--text-xs); color: var(--text-secondary); }
  [role="alert"] { color: var(--error); }
  button { min-height: 44px; background: var(--surface-raised); border-radius: var(--radius-md); }
  .results { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--space-1); }
  .results li { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2);
                font-size: var(--text-xs); color: var(--text-secondary); }
  .results li.falha { color: var(--warning); }
</style>
