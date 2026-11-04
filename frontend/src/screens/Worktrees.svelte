<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { basename, deleteWorktreeBatchForServer, fetchWorktreesForServer, getWorktreesForServer,
           mergedWorktreeBatch, relativeTime, worktreeAgeDays, worktreeIsAgent, worktreeReady, worktreeState,
           worktreeTitle, WORKTREE_STALE_DAYS, type WorktreeBatchProgress, type WorktreeRepo, type WorktreeState,
           type WorktreeStatus } from '@hangar/core';
  import BottomSheet from '../components/BottomSheet.svelte';
  import Spinner from '../components/Spinner.svelte';
  import WorktreeSheet from '../components/WorktreeSheet.svelte';
  import { listOwnServers, onServersChanged, type Server } from '../lib/auth';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { worktreeStatus } from '../lib/worktreeStatus.svelte';
  import { goToSession, reposAfterError, sessionStateLabel, worktreeSizeLabel, worktreesSizeBytes as somaTamanho, worktreesSizeSum,
           worktreeStateColor, worktreeStateLabel } from '../lib/worktreeView';
  import * as m from '../paraglide/messages';

  let { onBack }: { onBack?: () => void } = $props();
  let servidores = $state<Server[]>(listOwnServers());
  $effect(() => onServersChanged(() => { servidores = listOwnServers(); }));
  // Estado vivo das sessões nas fichas: o mesmo stream da lista, nunca um pedido por linha.
  // `onMount`, não `$effect`: o retain lê e grava estado do store, e dentro de um efeito isso vira laço
  // que aborta a tela antes da primeira leitura.
  onMount(() => {
    sessionsStore.retain();
    return () => sessionsStore.release();
  });

  type Bloco = { servidor: Server; repos: WorktreeRepo[]; erro: string };
  let blocos = $state<Bloco[]>([]);
  let carregando = $state(true);
  let atualizando = $state(false);
  let aberta = $state<{ servidor: Server; path: string } | null>(null);
  let geracao = 0;

  async function carregar() {
    const meu = ++geracao;
    carregando = blocos.length === 0;
    const out = await Promise.all(servidores.map(async (s) => {
      try {
        const repos = await getWorktreesForServer(s);
        for (const r of repos) for (const w of r.worktrees) worktreeStatus.put(s.id, w);
        return { servidor: s, repos, erro: '' };
      } catch (e) { return { servidor: s, repos: reposAfterError(blocos, s.id), erro: e instanceof Error ? e.message : String(e) }; }
    }));
    if (meu !== geracao) return;
    blocos = out;
    carregando = false;
  }

  // Primeiro mostra o que o disco já sabe; o fetch dos remotos (lento) vem depois e relê.
  // Por `serverId::repo`: o fetch do remoto que falhou deixa mesclada/atrasada velhas, e isso aparece.
  let erroRemoto = $state<Record<string, true>>({});
  let remoto = $state({ feitos: 0, total: 0 });
  let geracaoRemoto = 0;
  async function atualizar() {
    // Só a busca mais nova conta e encerra o aviso: uma antiga que termina depois não mexe no da nova.
    const meu = ++geracaoRemoto;
    releituras = 0;
    atualizando = true;
    const alvos = blocos.flatMap((b) => b.repos.map((r) => ({ b, repo: r.repo })));
    remoto = { feitos: 0, total: alvos.length };
    const res = await Promise.allSettled(alvos.map(({ b, repo }) =>
      fetchWorktreesForServer(b.servidor, repo).finally(() => { if (meu === geracaoRemoto) remoto.feitos++; })));
    if (meu !== geracaoRemoto) return;
    const falhas: Record<string, true> = {};
    res.forEach((x, i) => { if (x.status === 'rejected') falhas[chaveLote(alvos[i].b, alvos[i].repo)] = true; });
    erroRemoto = falhas;
    atualizando = false;
    await carregar();
  }

  // O backend mede o tamanho em segundo plano, uma pasta por vez: relê a cada 5 s enquanto houver
  // medida pendente, com teto para não ficar lendo para sempre se a medição travar.
  const medindo = $derived(blocos.some((b) => b.repos.some((r) => r.worktrees.some((w) => w.size_pending))));
  const existentes = $derived(blocos.flatMap((b) => b.repos.flatMap((r) => r.worktrees.filter((w) => w.exists))));
  const medidas = $derived(existentes.filter((w) => !w.size_pending).length);
  const MAX_RELEITURAS = 12;
  let releituras = $state(0);
  const demorando = $derived(medindo && releituras >= MAX_RELEITURAS);
  $effect(() => {
    blocos;   // cada leitura nova reagenda a próxima
    if (!medindo) { releituras = 0; return; }   // conta só releituras seguidas
    if (untrack(() => releituras) >= MAX_RELEITURAS) return;
    const t = setTimeout(() => { releituras++; void carregar(); }, 5000);
    return () => clearTimeout(t);
  });
  function tentarMedirDeNovo() {
    releituras = 0;
    void carregar();
  }

  // Por `serverId::repo`: o lote em andamento (botão desligado) e o aviso dele, que não é de leitura.
  let loteAndando = $state<string | null>(null);
  let erroLote = $state<Record<string, string>>({});
  const chaveLote = (b: Bloco, repo: string) => `${b.servidor.id}::${repo}`;

  // A confirmação congela o que a pessoa viu: o lote apaga essas e só essas.
  // As prontas entram sempre; as mescladas com alteração não commitada só se a pessoa marcar cada uma.
  type Confirmacao = { bloco: Bloco; repo: string; prontas: WorktreeStatus[]; comArquivos: WorktreeStatus[];
                       blocked: WorktreeStatus[] };
  let confirmando = $state<Confirmacao | null>(null);
  let marcadas = $state<Record<string, boolean>>({});
  const selecionadas = $derived(confirmando
    ? [...confirmando.prontas, ...confirmando.comArquivos.filter((w) => marcadas[w.path])] : []);

  function pedirLote(b: Bloco, r: WorktreeRepo) {
    const { deletable, blocked } = mergedWorktreeBatch(r);
    marcadas = {};
    confirmando = { bloco: b, repo: r.repo, prontas: deletable.filter(worktreeReady),
                    comArquivos: deletable.filter((w) => !worktreeReady(w)), blocked };
  }

  // O lote em andamento fica na folha, que só fecha no fim: o progresso é a confirmação de cada uma.
  let execucao = $state<{ k: string; p: WorktreeBatchProgress; fim: boolean } | null>(null);
  const pct = $derived(execucao && execucao.p.total ? Math.round((execucao.p.done / execucao.p.total) * 100) : 0);

  async function apagarMescladas() {
    if (!confirmando || !selecionadas.length || execucao) return;
    const { bloco: b, repo } = confirmando;
    const deletable = [...selecionadas];
    const k = chaveLote(b, repo);
    loteAndando = k;
    delete erroLote[k];
    execucao = { k, fim: false, p: { total: deletable.length, done: 0, current: null, removed: [], failed: [],
                                  freed: { bytes: 0, partial: false } } };
    try {
      await deleteWorktreeBatchForServer(b.servidor, repo, deletable, (p) => {
        for (const w of p.removed) worktreeStatus.drop(b.servidor.id, w.path);
        execucao = { k, fim: false, p };
      });
    } catch (e) {
      erroLote[k] = e instanceof Error ? e.message : String(e);
    } finally {
      loteAndando = null;
      if (execucao) execucao = { ...execucao, fim: true };
    }
    await carregar();
  }

  function fecharLote() {
    if (execucao && !execucao.fim) return;
    execucao = null;
    confirmando = null;
  }

  // Mesma chave do Orq: lista recarregada com os mesmos servidores não refaz a leitura nem zera a tela.
  const chaveServidores = $derived(servidores.map((s) => `${s.id}|${s.baseUrl}|${s.token}`).join('\n'));
  $effect(() => { chaveServidores; untrack(() => { void carregar().then(atualizar); }); });

  const vazio = $derived(!carregando && blocos.every((b) => !b.erro && b.repos.length === 0));

  // ── Filtros e contadores ──────────────────────────────────────────────────
  type Filtro = 'todas' | 'uso' | 'dirty' | 'prontas' | 'paradas' | 'sem_branch';
  let filtro = $state<Filtro>('todas');
  const parada = (w: WorktreeStatus) => (worktreeAgeDays(w) ?? 0) > WORKTREE_STALE_DAYS;
  const passa: Record<Filtro, (w: WorktreeStatus) => boolean> = {
    todas: () => true,
    uso: (w) => w.sessions.length > 0,
    dirty: (w) => w.dirty > 0,
    prontas: worktreeReady,
    paradas: parada,
    sem_branch: (w) => !w.branch,
  };
  type Item = { servidor: Server; w: WorktreeStatus; estado: WorktreeState };
  const todas = $derived<Item[]>(blocos.flatMap((b) => b.repos.flatMap((r) =>
    r.worktrees.map((w) => ({ servidor: b.servidor, w, estado: worktreeState(w) })))));
  const conta = (f: Filtro) => todas.filter((i) => passa[f](i.w)).length;
  const prontas = $derived(todas.filter((i) => worktreeReady(i.w)).map((i) => i.w));
  const liberaProntas = $derived(somaTamanho(prontas));
  const nomesEmUso = $derived([...new Set(todas.flatMap((i) => i.w.sessions))].join(' · '));

  const contadores = $derived<{ f: Filtro; rotulo: string; dica: string; cor: string }[]>([
    { f: 'uso', rotulo: m.worktrees_contador_em_uso(), dica: nomesEmUso || m.worktrees_contador_em_uso_vazio(), cor: 'var(--accent)' },
    { f: 'dirty', rotulo: m.worktrees_contador_nao_commitado(), dica: m.worktrees_contador_nao_commitado_dica(), cor: 'var(--warning)' },
    { f: 'prontas', rotulo: m.worktrees_contador_prontas(), dica: m.worktrees_contador_prontas_dica({ tamanho: worktreesSizeSum(prontas) }), cor: 'var(--success)' },
    { f: 'paradas', rotulo: m.worktrees_contador_paradas(), dica: m.worktrees_contador_paradas_dica(), cor: 'var(--text-muted)' },
  ]);
  const filtros = $derived<{ f: Filtro; rotulo: string }[]>([
    { f: 'todas', rotulo: m.worktrees_filtro_todas() },
    { f: 'uso', rotulo: m.worktrees_filtro_em_uso() },
    { f: 'dirty', rotulo: m.worktrees_filtro_nao_commitado() },
    { f: 'prontas', rotulo: m.worktrees_filtro_prontas() },
    { f: 'paradas', rotulo: m.worktrees_filtro_paradas() },
    { f: 'sem_branch', rotulo: m.worktrees_filtro_sem_branch() },
  ]);

  // ── Disco ─────────────────────────────────────────────────────────────────
  const TOPO = 6;
  const porTamanho = $derived(todas.filter((i) => (i.w.size ?? 0) > 0).sort((a, b) => (b.w.size ?? 0) - (a.w.size ?? 0)));
  const discoTotal = $derived(worktreesSizeSum(todas.map((i) => i.w)));
  const discoResto = $derived(porTamanho.slice(TOPO));
  const legenda: { e: WorktreeState; rotulo: () => string }[] = [
    { e: 'merged', rotulo: m.worktrees_filtro_prontas },
    { e: 'dirty', rotulo: m.worktrees_filtro_nao_commitado },
    { e: 'session', rotulo: m.worktrees_filtro_em_uso },
    { e: 'active', rotulo: m.worktree_estado_andamento },
    { e: 'detached', rotulo: m.worktrees_filtro_sem_branch },
  ];

  // Subagentes recolhidos por repositório; com filtro ligado, abertos (é o que a pessoa procura).
  let subAbertos = $state<Record<string, boolean>>({});

  function atividade(w: WorktreeStatus): string {
    const d = worktreeAgeDays(w);
    if (d === null) return '—';
    return d >= 1 ? m.worktree_dias_atras({ n: d }) : relativeTime(w.last_commit?.at ?? w.created_at);
  }
  const estadoSessao = (s: Server, nome: string) => sessionsStore.rows.find((r) => r.serverId === s.id && r.name === nome)?.state;
</script>

{#snippet ficha(servidor: Server, w: WorktreeStatus)}
  {@const e = worktreeState(w)}
  {@const dias = worktreeAgeDays(w)}
  <li class="ficha">
    <div class="ficha-topo">
      <button type="button" class="abrir" onclick={() => (aberta = { servidor, path: w.path })}>{worktreeTitle(w)}</button>
      <span class="tam" class:aviso={w.size_error}>{worktreeSizeLabel(w)}</span>
    </div>
    <div class="chips">
      <span class="chip" style:--cor={worktreeStateColor[e]}>{worktreeStateLabel[e]()}</span>
      {#if parada(w) && dias !== null}<span class="chip" style:--cor="var(--warning)">{m.worktree_parada_dias({ n: dias })}</span>{/if}
    </div>
    {#if w.sessions.length}
      <div class="sessoes">
        {#each w.sessions as nome (nome)}
          {@const est = estadoSessao(servidor, nome)}
          <button type="button" class="sessao" aria-label={m.worktree_ir_sessao_nome({ nome })} onclick={() => goToSession(servidor.id, nome)}>
            <span class="ponto" class:ponto--vivo={est === 'working'} class:ponto--espera={est === 'awaiting_input'} aria-hidden="true"></span>
            <span class="sessao-nome">{nome}</span>
            {#if est}<span class="muted">{sessionStateLabel(est)}</span>{/if}
            <span aria-hidden="true">→</span>
          </button>
        {/each}
      </div>
    {/if}
    <div class="mono"><span>{w.branch ?? m.worktree_sem_branch_rotulo()}</span><span class="muted"> ← {w.base ?? '—'}</span></div>
    {#if w.degraded}<p class="aviso" role="note">{m.worktree_leitura_incompleta()}</p>{/if}
    {#if w.last_commit}<div class="assunto">{w.last_commit.subject}</div>{/if}
    <div class="metricas">
      <span class="mono">↑{w.ahead} ↓{w.behind ?? 0}</span>
      <span class:aviso={w.residual || w.dirty > 0} class:muted={!w.residual && !w.dirty}>{w.residual ? m.worktree_estado_residual() : w.dirty ? m.worktree_n_nao_commitados({ n: w.dirty }) : m.worktree_limpa()}</span>
      <span class="muted">{atividade(w)}</span>
      {#if w.created_at}<span class="muted">{m.worktree_criada({ quando: relativeTime(w.created_at) })}</span>{/if}
    </div>
  </li>
{/snippet}

<div class="wt">
  <header class="topo">
    {#if onBack}<button class="voltar" onclick={onBack} aria-label={m.orq_voltar()}>←</button>{/if}
    <h1>{m.worktrees_titulo()}</h1>
    {#if atualizando}<span class="muted" role="status">{m.worktrees_remoto_progresso(remoto)}</span>{/if}
  </header>
  {#if carregando}
    <div class="centro" role="status"><Spinner /><span class="muted">{m.worktrees_lendo()}</span></div>
  {:else if vazio}
    <p class="vazio">{m.worktrees_vazio()}</p>
  {:else}
    {#each blocos as b (b.servidor.id)}
      {#if b.erro}<p class="erro" role="alert">{b.servidor.label}: {m.worktrees_erro({ motivo: b.erro })}</p>{/if}
    {/each}

    {#if todas.length}
      <section class="contadores">
        {#each contadores as c (c.f)}
          <button type="button" class="contador" class:on={filtro === c.f} style:--cor={c.cor} aria-pressed={filtro === c.f}
                  onclick={() => (filtro = filtro === c.f ? 'todas' : c.f)}>
            <span class="contador-rotulo">{c.rotulo}</span>
            <span class="contador-n">{conta(c.f)}</span>
            <span class="contador-dica">{c.dica}</span>
          </button>
        {/each}
      </section>

      <section class="disco">
        <h2>{m.worktrees_disco_titulo()}</h2>
        {#if demorando}
          <p class="aviso">{m.worktrees_disco_demorando()}
            <button type="button" class="tentar" onclick={tentarMedirDeNovo}>{m.busca_tentar_de_novo()}</button></p>
        {:else if medindo}<p class="muted" role="status">{m.worktrees_disco_progresso({ feitas: medidas, total: existentes.length })}</p>{/if}
        {#if porTamanho.length}
          <p class="muted">{m.worktrees_disco_resumo({ total: discoTotal, n: porTamanho.length })}
            {#if liberaProntas} · <span class="libera">{m.worktrees_disco_libera({ tamanho: worktreesSizeSum(prontas) })}</span>{/if}</p>
          <div class="barra" aria-hidden="true">
            {#each porTamanho as i (i.servidor.id + i.w.path)}<span style:flex-grow={String(i.w.size)} style:background={worktreeStateColor[i.estado]}></span>{/each}
          </div>
          <ul class="disco-linhas">
            {#each porTamanho.slice(0, TOPO) as i (i.servidor.id + i.w.path)}
              <li>
                <button type="button" class="disco-linha" onclick={() => (aberta = { servidor: i.servidor, path: i.w.path })}>
                  <span class="ponto" style:background={worktreeStateColor[i.estado]} aria-hidden="true"></span>
                  <span class="disco-nome">{worktreeTitle(i.w)}</span>
                  <span class="muted">{worktreeStateLabel[i.estado]()}</span>
                  <span class="tam">{worktreeSizeLabel(i.w)}</span>
                </button>
              </li>
            {/each}
            {#if discoResto.length}
              <li class="disco-linha disco-linha--resto">
                <span class="ponto" aria-hidden="true"></span>
                <span class="disco-nome">{m.worktrees_disco_outras({ n: discoResto.length })}</span>
                <span class="tam">{worktreesSizeSum(discoResto.map((i) => i.w))}</span>
              </li>
            {/if}
          </ul>
          <div class="legenda">
            {#each legenda as l (l.e)}<span><span class="ponto" style:background={worktreeStateColor[l.e]} aria-hidden="true"></span>{l.rotulo()}</span>{/each}
          </div>
        {/if}
      </section>

      <div class="filtros" role="toolbar">
        {#each filtros as f (f.f)}
          <button type="button" class="filtro" class:on={filtro === f.f} aria-pressed={filtro === f.f} onclick={() => (filtro = f.f)}>
            {f.rotulo}<span class="filtro-n">{conta(f.f)}</span>
          </button>
        {/each}
      </div>
    {/if}

    {@const algumVisivel = todas.some((i) => passa[filtro](i.w))}
    {#if todas.length && !algumVisivel}<p class="vazio">{m.worktrees_filtro_vazio()}</p>{/if}

    {#each blocos as b (b.servidor.id)}
      {#each b.repos as r (r.repo)}
        {@const visiveis = r.worktrees.filter(passa[filtro])}
        {@const principais = visiveis.filter((w) => !worktreeIsAgent(w))}
        {@const agentes = visiveis.filter(worktreeIsAgent)}
        {@const lote = mergedWorktreeBatch(r)}
        {@const prontasRepo = r.worktrees.filter(worktreeReady)}
        <!-- O botão conta as prontas, igual ao contador; sem prontas, o lote (mescladas com alteração) segue alcançável. -->
        {@const [limpar, rotuloLimpar] = prontasRepo.length ? [prontasRepo, m.worktree_limpar_mescladas]
                                                            : [lote.deletable, m.worktree_clean_merged_with_changes]}
        {@const k = chaveLote(b, r.repo)}
        {@const subAberto = filtro !== 'todas' || !!subAbertos[k]}
        {@const principal = r.worktrees.find((w) => w.main_branch)?.main_branch}
        {#if visiveis.length || erroLote[k] || erroRemoto[k]}
          <section class="repo">
            <div class="repo-topo">
              <h2>{basename(r.repo)}{servidores.length > 1 ? ` · ${b.servidor.label}` : ''}</h2>
              {#if principal}<span class="muted">{m.worktree_repo_principal({ branch: principal })}</span>{/if}
              <span class="muted">{m.worktree_repo_resumo({ n: r.worktrees.length, tamanho: worktreesSizeSum(r.worktrees) })}</span>
            </div>
            {#if lote.deletable.length}
              <button type="button" class="lote" disabled={loteAndando === k} onclick={() => pedirLote(b, r)}>
                {#if loteAndando === k}<Spinner />{/if}
                {rotuloLimpar({ n: limpar.length, tamanho: worktreesSizeSum(limpar) })}
              </button>
            {/if}
            {#if erroLote[k]}<p class="erro" role="alert">{erroLote[k]}</p>{/if}
            {#if erroRemoto[k]}<p class="aviso" role="alert">{m.worktrees_busca_remoto_falhou({ repo: basename(r.repo) })}</p>{/if}
            <ul class="fichas">
              {#each principais as w (w.path)}{@render ficha(b.servidor, w)}{/each}
            </ul>
            {#if agentes.length}
              {@const recente = Math.max(...agentes.map((w) => w.last_commit?.at ?? w.created_at ?? 0))}
              <button type="button" class="subagentes" aria-expanded={subAberto} onclick={() => (subAbertos[k] = !subAbertos[k])}>
                <span class="seta" class:seta--aberta={subAberto} aria-hidden="true">›</span>
                <span class="sub-titulo">{m.worktree_subagentes()} <span class="filtro-n">{agentes.length}</span></span>
                {#if recente}<span class="muted">{m.worktree_subagentes_dica({ quando: relativeTime(recente) })}</span>{/if}
              </button>
              {#if subAberto}
                <ul class="fichas">
                  {#each agentes as w (w.path)}{@render ficha(b.servidor, w)}{/each}
                </ul>
              {/if}
            {/if}
          </section>
        {/if}
      {/each}
    {/each}
    {#if todas.length}<p class="muted legenda-commits">{m.worktrees_legenda_commits()}</p>{/if}
  {/if}
</div>
{#if aberta}
  <WorktreeSheet open={true} server={aberta.servidor} path={aberta.path} onClose={() => (aberta = null)} onDeleted={carregar} />
{/if}
{#if confirmando}
  {@const libera = somaTamanho(selecionadas)}
  <BottomSheet open={true} onClose={fecharLote} ariaLabel={m.worktree_lote_titulo({ n: selecionadas.length })}>
    {#if execucao}
      {@const p = execucao.p}
      <div class="sheet">
        <h2 class="title">{execucao.fim ? m.worktree_lote_fim_titulo() : m.worktree_lote_apagando_titulo()}</h2>
        <div class="progresso" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}
             aria-label={m.worktree_lote_apagando_titulo()}><span style:width="{pct}%"></span></div>
        <p class="muted" aria-live="polite">{m.worktree_lote_progresso({ feitas: p.removed.length, total: p.total,
          tamanho: worktreesSizeSum(p.removed) })}</p>
        {#if p.current}<p class="muted">{m.worktree_lote_apagando({ nome: basename(p.current.path) })}</p>{/if}
        {#if p.failed.length}
          <div class="falhas" role="alert">
            <p class="erro">{m.worktree_lote_falhas({ n: p.failed.length })}</p>
            <ul class="perde">
              {#each p.failed as f (f.worktree.path)}
                <li>{f.error === null ? m.worktree_lote_mudou({ nome: basename(f.worktree.path) })
                                      : m.worktree_lote_erro_item({ nome: basename(f.worktree.path), motivo: f.error })}</li>
              {/each}
            </ul>
          </div>
        {/if}
        {#if erroLote[execucao.k]}<p class="erro" role="alert">{erroLote[execucao.k]}</p>{/if}
        {#if execucao.fim}
          <div class="acoes"><button type="button" class="cancelar" onclick={fecharLote}>{m.worktree_lote_fechar()}</button></div>
        {/if}
      </div>
    {:else}
    <div class="sheet">
      <h2 class="title">{m.worktree_lote_titulo({ n: selecionadas.length })}</h2>
      {#if libera}<p class="libera">{m.worktree_libera({ tamanho: worktreesSizeSum(selecionadas) })}</p>{/if}
      {#snippet perdas(w: WorktreeStatus)}
        {#if w.dirty || w.ignored.length}
          <span class="aviso">{m.worktree_apagar_perde()}</span>
          <ul class="perde">
            {#if w.dirty}<li>{m.worktree_nao_commitados({ n: w.dirty })}</li>{/if}
            {#each w.ignored as f (f)}<li>{f}</li>{/each}
          </ul>
        {:else}
          <span class="muted">{m.worktree_lote_nada_perde()}</span>
        {/if}
      {/snippet}
      {#if confirmando.prontas.length}
        <ul class="itens">
          {#each confirmando.prontas as w (w.path)}
            <li><span class="nome">{basename(w.path)}</span>{@render perdas(w)}</li>
          {/each}
        </ul>
      {/if}
      {#if confirmando.comArquivos.length}
        <p class="aviso">{m.worktree_lote_com_arquivos()}</p>
        <ul class="itens">
          {#each confirmando.comArquivos as w (w.path)}
            <li>
              <label class="marca-lote"><input type="checkbox" bind:checked={marcadas[w.path]} /> <span class="nome">{basename(w.path)}</span></label>
              {@render perdas(w)}
            </li>
          {/each}
        </ul>
      {/if}
      {#if confirmando.blocked.length}
        <p class="aviso">{m.worktree_lote_ficam()}</p>
        <ul class="itens">
          {#each confirmando.blocked as w (w.path)}
            <li>
              <span class="nome">{basename(w.path)}</span>
              <span class="muted">{w.sessions.length ? m.worktree_sessao_aberta({ nomes: w.sessions.join(', ') }) : m.worktree_lote_leitura_falhou()}</span>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="acoes">
        <button type="button" class="cancelar" onclick={() => (confirmando = null)}>{m.comum_cancelar()}</button>
        <button type="button" class="apagar" disabled={!selecionadas.length} onclick={apagarMescladas}>{m.worktree_lote_apagar_n({ n: selecionadas.length })}</button>
      </div>
    </div>
    {/if}
  </BottomSheet>
{/if}

<style>
  .wt { display: flex; flex-direction: column; gap: 12px; height: 100%; overflow-y: auto; padding: 12px 16px 32px; }
  .topo { display: flex; align-items: center; gap: 8px; }
  .topo h1 { font-size: 1.15rem; margin: 0; flex: 1; }
  .voltar { background: none; border: 0; font-size: 1.2rem; min-width: 40px; min-height: 40px; color: inherit; }
  h2 { font-size: 0.95rem; margin: 0; }
  .muted { color: var(--text-muted); font-size: 0.85rem; }
  .mono { font-family: var(--font-mono); font-size: 0.82rem; overflow-wrap: anywhere; }
  .aviso { color: var(--warning-text); font-size: 0.85rem; }
  .libera { color: var(--success-text); }
  .erro { color: var(--error); margin: 0; }
  .vazio { color: var(--text-muted); text-align: center; padding: 32px 0; margin: 0; }
  .centro { display: flex; justify-content: center; align-items: center; gap: 10px; padding: 32px; }
  .ponto { width: 8px; height: 8px; border-radius: 50%; flex: none; background: var(--text-muted); display: inline-block; }
  .ponto--vivo { background: var(--success); }
  .ponto--espera { background: var(--warning); }
  .tam { margin-left: auto; font-variant-numeric: tabular-nums; color: var(--text-secondary); font-size: 0.85rem; white-space: nowrap; }

  /* Contadores: 2×2 no celular; cada um liga o filtro do mesmo nome. */
  .contadores { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .contador { display: flex; flex-direction: column; align-items: flex-start; gap: 2px; text-align: left; padding: 10px 12px;
              border-radius: 12px; border: 1px solid var(--border-subtle); background: var(--surface-raised); font: inherit; color: inherit; }
  .contador.on { border-color: var(--cor); }
  .contador-rotulo { font-size: 0.78rem; color: var(--text-secondary); display: flex; align-items: center; gap: 6px; }
  .contador-rotulo::before { content: ''; width: 8px; height: 8px; border-radius: 50%; background: var(--cor); }
  .contador-n { font-size: 1.4rem; font-weight: 600; font-variant-numeric: tabular-nums; }
  .contador-dica { font-size: 0.75rem; color: var(--text-muted); overflow-wrap: anywhere; }

  .disco { display: flex; flex-direction: column; gap: 8px; padding: 12px; border-radius: 12px; background: var(--surface-raised); }
  .disco p { margin: 0; }
  .barra { display: flex; height: 10px; border-radius: 5px; overflow: hidden; gap: 1px; }
  .barra span { min-width: 2px; }
  .disco-linhas { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
  .disco-linha { width: 100%; display: flex; align-items: center; gap: 8px; text-align: left; padding: 6px 4px; min-height: 36px;
                 border: 0; background: transparent; font: inherit; color: inherit; border-radius: 6px; }
  .disco-linha--resto { color: var(--text-muted); }
  .disco-nome { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; flex: 0 1 auto; }
  .legenda { display: flex; flex-wrap: wrap; gap: 4px 12px; font-size: 0.75rem; color: var(--text-muted); }
  .legenda > span { display: inline-flex; align-items: center; gap: 4px; }

  /* Chips do filtro rolam na horizontal: seis não cabem numa linha de celular. */
  .filtros { display: flex; gap: 6px; overflow-x: auto; margin: 0 -16px; padding: 0 16px; scrollbar-width: none; }
  .filtro { flex: none; display: inline-flex; align-items: center; gap: 6px; min-height: 34px; padding: 0 12px; border-radius: 999px;
            border: 1px solid var(--border-subtle); background: transparent; font: inherit; font-size: 0.85rem; color: inherit; white-space: nowrap; }
  .filtro.on { border-color: var(--border-strong); background: var(--surface-raised); }
  .filtro-n { color: var(--text-muted); font-variant-numeric: tabular-nums; }

  .repo { display: flex; flex-direction: column; gap: 8px; }
  .repo-topo { display: flex; flex-direction: column; gap: 2px; margin-top: 8px; }
  .lote { align-self: flex-start; gap: 8px; padding: 0 16px; border-radius: 8px;
          border: 1px solid color-mix(in srgb, var(--success) 35%, transparent);
          background: color-mix(in srgb, var(--success) 8%, transparent); color: var(--success-text); font-weight: 600; }
  .lote:disabled { opacity: 0.6; }
  .fichas { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
  /* A ficha inteira abre o detalhe pelo ::after do título; as sessões ficam por cima e levam à conversa. */
  .ficha { position: relative; display: flex; flex-direction: column; gap: 4px; padding: 10px 12px; border-radius: 10px;
           background: var(--surface-raised); }
  .ficha-topo { display: flex; align-items: baseline; gap: 8px; }
  .abrir { padding: 0; min-height: 0; border: 0; background: transparent; font: inherit; font-weight: 600; color: inherit;
           text-align: left; overflow-wrap: anywhere; }
  .abrir::after { content: ''; position: absolute; inset: 0; border-radius: 10px; }
  .chips, .sessoes, .metricas { display: flex; flex-wrap: wrap; gap: 4px 10px; align-items: center; }
  .chips { gap: 6px; }
  .chip { font-size: 0.72rem; padding: 2px 8px; border-radius: 999px; color: var(--cor);
          background: color-mix(in srgb, var(--cor) 14%, transparent); }
  .sessao { position: relative; z-index: 1; display: inline-flex; align-items: center; gap: 6px; min-height: 32px; padding: 0 10px;
            border-radius: 999px; border: 1px solid var(--border-default); background: var(--surface-inset); font: inherit; font-size: 0.82rem; color: inherit; }
  .sessao-nome { font-weight: 600; }
  .assunto { font-size: 0.85rem; color: var(--text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .metricas { font-size: 0.82rem; }
  .subagentes { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 8px; text-align: left; padding: 8px 4px; min-height: 40px;
                border: 0; background: transparent; font: inherit; color: inherit; }
  .sub-titulo { font-weight: 600; font-size: 0.9rem; }
  .seta { display: inline-block; transition: rotate 0.15s; }
  .seta--aberta { rotate: 90deg; }
  .tentar { min-height: 32px; margin-left: 6px; padding: 0 10px; border-radius: 8px; border: 1px solid var(--border-default); font-size: 0.8rem; }
  .ficha .aviso { margin: 0; position: relative; z-index: 1; }
  .legenda-commits { margin: 4px 0 0; font-size: 0.75rem; }

  .sheet { padding: 16px; display: flex; flex-direction: column; gap: 10px; }
  .sheet .libera { margin: 0; font-size: 0.9rem; }
  .title { font-size: 1.05rem; margin: 0; }
  .nome { font-weight: 600; }
  .itens { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  .itens > li { display: flex; flex-direction: column; gap: 2px; }
  .sheet .aviso { margin: 0; }
  .perde { margin: 0; padding-left: 18px; font-family: var(--font-mono); font-size: 0.8rem; overflow-wrap: anywhere; }
  .acoes { display: flex; justify-content: flex-end; gap: 8px; margin-top: 4px; }
  .cancelar { padding: 0 16px; border-radius: 8px; border: 1px solid var(--border-default); }
  .apagar { padding: 0 16px; border-radius: 8px; background: var(--error); color: #fff; font-weight: 600; }
  .apagar:disabled { opacity: 0.5; }
  .marca-lote { display: flex; align-items: center; gap: 8px; }
  .progresso { height: 8px; border-radius: 4px; overflow: hidden; background: var(--surface-inset); }
  .progresso > span { display: block; height: 100%; background: var(--error); transition: width 0.2s; }
  .sheet > .muted, .falhas > .erro { margin: 0; }
</style>
