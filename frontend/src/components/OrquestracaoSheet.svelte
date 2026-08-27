<script lang="ts">
  // Modal "Orquestração": quem roda cada papel do grupo (aba Papéis, contrato `regras-<gid>.md`)
  // e quais contas a máquina libera (aba Contas, `orquestracao-contas.md`). Mesmo desenho do
  // CreateSessionSheet: lista à esquerda, formulário à direita; no celular, lista → formulário.
  // Salvar um papel grava a tabela e manda recado ao árbitro — a sessão viva NUNCA é tocada.
  import * as m from '../paraglide/messages';
  import BottomSheet from './BottomSheet.svelte';
  import Select from './Select.svelte';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
  import OrquestracaoContas from './OrquestracaoContas.svelte';
  import { providerName } from '../lib/format';
  import { untrack } from 'svelte';
  import { createQuery } from '@tanstack/svelte-query';
  import { postOrqPapeis } from '../lib/api';
  import { clienteQuery, orqGrupo, orqPolitica } from '../lib/queries';
  import { quotaFeed } from '../lib/quotaFeed.svelte';
  import {
    casarViva, contasLiberadas, estadoDoPapel, modelosLiberados, politicaDe,
    type OrqGrupo, type OrqPolitica, type Papel, type Provider,
  } from '../lib/orquestracao';
  import type { SessionInfo } from '../lib/types';

  type Aba = 'papeis' | 'contas';
  interface Props {
    open: boolean;
    onClose: () => void;
    sessionName: string;
    // Sessões vivas (do Chat, já polladas) — é daqui que sai o "medido" de cada papel.
    sessoes: SessionInfo[];
    abaInicial?: Aba;
  }
  let { open, onClose, sessionName, sessoes, abaInicial = 'papeis' }: Props = $props();

  const PROVIDERS: Provider[] = ['claude', 'codex', 'pi', 'kimi'];
  const NIVEIS: Record<string, string[]> = {
    claude: ['low', 'medium', 'high', 'xhigh', 'max'],
    pi: ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'],
  };

  let isDesktop = $state(typeof window !== 'undefined' && window.matchMedia('(min-width: 820px)').matches);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });

  let aba = $state<Aba>('papeis');
  // As duas leituras vêm do cache compartilhado (lib/queries.ts): reabrir o painel entrega o dado
  // que já estava lá e revalida por baixo, em vez de esvaziar a tela e buscar do zero. `enabled`
  // porque o componente fica montado no Chat mesmo fechado — sem ele, buscaria sem ninguém olhando.
  const qPolitica = createQuery(() => ({ ...orqPolitica(), enabled: open }), () => clienteQuery);
  const qGrupo = createQuery(() => ({ ...orqGrupo(sessionName), enabled: open }), () => clienteQuery);
  const grupo = $derived(qGrupo.data ?? null);
  const politica = $derived(qPolitica.data ?? null);
  const carregando = $derived(qPolitica.isPending || qGrupo.isPending);
  const erroCarga = $derived(
    (qPolitica.error ?? qGrupo.error) ? ((qPolitica.error ?? qGrupo.error) as Error).message : '',
  );
  let erro = $state('');
  let conflito = $state(false);
  let salvando = $state(false);
  let aviso = $state('');
  let avisoRuim = $state(false);
  // Papel escolhido: índice no contrato, 'novo', ou null (nada escolhido).
  let sel = $state<number | 'novo' | null>(null);

  let fPapel = $state('');
  let fSessao = $state('');
  let fProvider = $state<Provider>('claude');
  let fConta = $state('');
  let fModelo = $state('');
  let fEsforco = $state('');

  const papeis = $derived(grupo?.papeis ?? []);
  // Papéis da skill orchestrating-idea-to-push; um que já está no contrato não se repete.
  const PAPEIS_CANONICOS = ['árbitro', 'executor', 'revisor', 'revisão final', 'par de research'];
  const papeisDisponiveis = $derived(PAPEIS_CANONICOS.filter((n) => !papeis.some((p) => p.papel.toLowerCase() === n)));
  let papelOutro = $state(false);
  // O nome da sessão não é escolha do usuário: sai do prefixo do grupo (`trab-` do árbitro ou
  // do primeiro papel) + sufixo por papel, no padrão que a skill já usa. Papel existente mantém o dele.
  const SUFIXO: Record<string, string> = { 'árbitro': 'arbitro', executor: 't*', revisor: 'review*', 'revisão final': 'final', 'par de research': 'mock' };
  const prefixoGrupo = $derived.by(() => {
    const base = papeis.find((p) => p.sessao)?.sessao ?? grupo?.arbitro ?? sessionName;
    const i = base.lastIndexOf('-');
    return i > 0 ? base.slice(0, i + 1) : base + '-';
  });
  const sessaoDerivada = (papel: string) =>
    prefixoGrupo + (SUFIXO[papel.toLowerCase()] ?? papel.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') + '*');
  const contasDoProvider = $derived(politica ? contasLiberadas(politica.politica, politica.inventario, fProvider) : []);
  const invConta = $derived(politica?.inventario.find((i) => i.provider === fProvider && i.conta === fConta) ?? null);
  const polConta = $derived(politica ? politicaDe(politica.politica, fProvider, fConta, politica.inventario) : null);
  const modelos = $derived(modelosLiberados(invConta, polConta));
  const contaTravada = $derived(!!polConta && !polConta.trocar);
  const cotaConta = $derived.by(() => {
    const id = grupo?.papeis.find((p) => p.provider === fProvider && p.conta === fConta)?.id_cota ?? invConta?.id_cota ?? null;
    const c = id ? quotaFeed.contas.find((x) => x.id === id) : null;
    if (!c?.janelas.length) return null;
    return c.janelas.reduce((a, b) => (b.pct > a.pct ? b : a));
  });
  const papelAtual = $derived(typeof sel === 'number' ? papeis[sel] ?? null : null);
  const vivaAtual = $derived(papelAtual ? casarViva(papelAtual.sessao, sessoes) : null);
  const estadoAtual = $derived(papelAtual ? estadoDoPapel(papelAtual, vivaAtual) : null);

  $effect(() => {
    if (!open) return;
    aba = abaInicial;
    sel = null; aviso = ''; erro = ''; conflito = false;
    quotaFeed.retain();
    return () => quotaFeed.release();
  });

  // Recarregar do zero (botão do conflito de mtime): ignora o staleTime de propósito — o arquivo
  // mudou no disco, o cache está errado por definição.
  function carregar() {
    erro = ''; conflito = false;
    void qPolitica.refetch();
    void qGrupo.refetch();
  }

  // Edições pendentes por papel (chave = nome do papel no contrato, ou 'novo'). Trocar de card
  // NÃO descarta o que foi mudado: o usuário edita vários e salva tudo no fim, num recado só.
  type Rascunho = { papel: string; sessao: string; provider: Provider; conta: string; modelo: string; esforco: string };
  let rascunhos = $state<Record<string, Rascunho>>({});
  const nRascunhos = $derived(Object.keys(rascunhos).length);
  const chaveDe = (i: number | 'novo') => (i === 'novo' ? 'novo' : papeis[i]?.papel ?? '');

  function guardarRascunho() {
    if (sel === null) return;
    const k = chaveDe(sel);
    const orig = sel === 'novo' ? null : papeis[sel];
    const papelNome = fPapel.trim();
    const r: Rascunho = { papel: papelNome, sessao: (fSessao.trim() || (papelNome ? sessaoDerivada(papelNome) : '')), provider: fProvider, conta: fConta, modelo: fModelo, esforco: fEsforco };
    const igual = !!orig && orig.sessao === r.sessao && (orig.provider || 'claude') === r.provider
      && orig.conta === r.conta && orig.modelo === r.modelo && orig.esforco === r.esforco;
    if (igual || (sel === 'novo' && !r.papel)) delete rascunhos[k]; else rascunhos[k] = r;
  }
  $effect(() => {
    void [fPapel, fSessao, fProvider, fConta, fModelo, fEsforco, sel];
    untrack(guardarRascunho);
  });

  function escolher(i: number | 'novo') {
    sel = i; aviso = ''; erro = ''; conflito = false;
    const p = typeof i === 'number' ? papeis[i] : null;
    const r = rascunhos[chaveDe(i)];
    fPapel = r?.papel ?? p?.papel ?? '';
    papelOutro = i === 'novo' && !!fPapel && !PAPEIS_CANONICOS.includes(fPapel);
    fSessao = r?.sessao ?? p?.sessao ?? '';
    fProvider = (r?.provider ?? p?.provider ?? 'claude') as Provider;
    fConta = r?.conta ?? p?.conta ?? '';
    fModelo = r?.modelo ?? p?.modelo ?? '';
    fEsforco = r?.esforco ?? p?.esforco ?? '';
  }
  function trocarProvider(p: Provider) {
    fProvider = p;
    fConta = (politica?.politica.find((c) => c.provider === p)?.conta) ?? '';
    fModelo = ''; fEsforco = '';
  }
  // Conta travada: o modelo é o primeiro liberado, sem escolha.
  $effect(() => { if (contaTravada && modelos[0]) fModelo = modelos[0].id; });

  async function salvar() {
    guardarRascunho();
    const itens = Object.values(rascunhos).filter((r) => r.papel && r.conta);
    if (!grupo || !itens.length) return;
    salvando = true; erro = ''; aviso = ''; conflito = false;
    try {
      const r = await postOrqPapeis(sessionName, { papeis: itens, mtime: grupo.mtime });
      const lista = [...grupo.papeis];
      let ultimo = sel;
      for (const p of r.papeis) {
        const novo: Papel = { ...p, viva: casarViva(p.sessao, sessoes)?.name ?? null };
        const i = lista.findIndex((x) => x.papel.toLowerCase() === novo.papel.toLowerCase());
        if (i >= 0) lista[i] = { ...lista[i], ...novo }; else lista.push(novo);
        ultimo = i >= 0 ? i : lista.length - 1;
      }
      // Escreve no cache, não num state local: o painel reabre com o que foi salvo, sem esperar
      // uma releitura do disco que já sabemos como terminaria.
      clienteQuery.setQueryData(orqGrupo(sessionName).queryKey,
        { ...grupo, mtime: r.mtime, papeis: lista, arbitro: r.arbitro });
      rascunhos = {};
      sel = typeof ultimo === 'number' ? ultimo : sel;
      const arb = r.arbitro ?? '';
      avisoRuim = r.aviso === 'falhou' || r.aviso === 'sem_arbitro';
      aviso = r.aviso === 'enviado' ? m.orqcfg_aviso_enviado({ arbitro: arb })
        : r.aviso === 'enfileirado' ? m.orqcfg_aviso_enfileirado({ arbitro: arb })
        : r.aviso === 'sem_arbitro' ? m.orqcfg_aviso_sem_arbitro()
        : m.orqcfg_aviso_falhou({ erro: r.erro ?? '' });
    } catch (e) {
      const err = e as Error & { status?: number };
      if (err.status === 409) conflito = true; else erro = err.message;
    } finally {
      salvando = false;
    }
  }
</script>

{#snippet abas()}
  <div class="os-tabs" role="tablist">
    <button type="button" role="tab" class="os-tab" class:on={aba === 'papeis'} aria-selected={aba === 'papeis'} onclick={() => (aba = 'papeis')}>{m.orqcfg_aba_papeis()}</button>
    <button type="button" role="tab" class="os-tab" class:on={aba === 'contas'} aria-selected={aba === 'contas'} onclick={() => (aba = 'contas')}>{m.orqcfg_aba_contas()}</button>
  </div>
{/snippet}

{#snippet listaPapeis()}
  {#if grupo?.gid === 'padrao'}
    <p class="os-intro">{m.orqcfg_sem_grupo()}</p>
  {/if}
  {#if carregando && !grupo}
    <!-- Esqueleto com a altura do card real: a lista nasce no lugar, não "pula" ao chegar. -->
    <div class="os-skel" aria-busy="true" aria-label={m.orqcfg_carregando()}>
      {#each [0, 1, 2, 3] as k (k)}
        <div class="os-item os-skel-item">
          <span class="os-item-body">
            <span class="os-skel-bar" style="width: 38%"></span>
            <span class="os-skel-bar os-skel-bar--sub" style="width: 52%"></span>
            <span class="os-skel-chips"><span class="os-skel-chip"></span><span class="os-skel-chip" style="width: 96px"></span><span class="os-skel-chip" style="width: 84px"></span></span>
          </span>
        </div>
      {/each}
    </div>
  {:else if grupo}
    <p class="os-intro">{papeis.length ? m.orqcfg_papeis_intro() : m.orqcfg_sem_papeis()}</p>
    <!-- Chave com o índice: dois papéis de mesmo nome (contrato editado à mão) derrubavam a lista
         inteira, sem erro na tela (medido em 26/08/2026). -->
    {#each papeis as p, i (`${p.papel}#${i}`)}
      {@const viva = casarViva(p.sessao, sessoes)}
      {@const st = estadoDoPapel(p, viva)}
      <button type="button" class="os-item" class:sel={sel === i} class:bad={st.divergente} onclick={() => escolher(i)}>
        <span class="os-item-body">
          <span class="os-nome">{p.papel}{#if rascunhos[p.papel]} <span class="os-chip os-chip--edit">{m.orqcfg_editado()}</span>{/if}</span>
          <span class="os-sess">{viva?.name ?? p.sessao}</span>
          <span class="os-chips">
            {#if st.viva}<span class="os-chip os-chip--ok">● {m.orqcfg_viva()}</span>{:else}<span class="os-chip os-chip--off">○ {m.orqcfg_nao_aberta()}</span>{/if}
            <span class="os-chip">{providerName(p.provider || 'claude')} · {p.conta}</span>
            <span class="os-chip">{p.modelo || '—'}{p.esforco ? ` · ${p.esforco}` : ''}</span>
            {#if st.divergente}
              <span class="os-chip os-chip--bad">{m.orqcfg_rodando_em({ v: [st.conta === 'divergente' ? st.contaMedida : null, st.modelo === 'divergente' ? st.modeloMedido : null, st.esforco === 'divergente' ? st.esforcoMedido : null].filter(Boolean).join(' · ') })}</span>
            {/if}
          </span>
        </span>
        <span class="os-arrow" aria-hidden="true">›</span>
      </button>
    {/each}
    <button type="button" class="os-item os-item--novo" class:sel={sel === 'novo'} onclick={() => escolher('novo')}>
      <span class="os-item-body"><span class="os-nome">+ {m.orqcfg_novo_papel()}{#if rascunhos.novo} <span class="os-chip os-chip--edit">{m.orqcfg_editado()}</span>{/if}</span></span>
    </button>
  {:else if erroCarga}
    <!-- Falha na leitura: sem este ramo o painel ficava VAZIO — nem esqueleto (carregando já é
         falso) nem lista (grupo é null) —, e a mensagem só existia dentro do formulário de um
         papel, que ninguém consegue escolher justamente porque a lista não veio. -->
    <p class="os-erro" role="alert">{erroCarga}</p>
    <button type="button" class="os-link" onclick={carregar}>{m.orqcfg_recarregar()}</button>
  {/if}
{/snippet}

{#snippet formPapel()}
  {#if sel === null}
    <div class="os-vazio"><p>{m.orqcfg_escolha_papel()}</p></div>
  {:else}
    <h3 class="os-h">{papelAtual?.papel ?? m.orqcfg_novo_papel()}</h3>
    <p class="os-intro">{m.orqcfg_aplica_proxima()}</p>

    {#if sel === 'novo'}
      <div class="field">
        <span class="field-label">{m.orqcfg_papel()}</span>
        <!-- Os papéis da skill são fixos: escolhe-se, não se digita. "outro" abre o campo. -->
        <div class="provider-grid os-papeis-grid" role="group" aria-label={m.orqcfg_papel()}>
          {#each papeisDisponiveis as nome (nome)}
            <button type="button" class="provider-tile" class:on={!papelOutro && fPapel === nome} aria-pressed={!papelOutro && fPapel === nome}
                    onclick={() => { papelOutro = false; fPapel = nome; }}>{nome}</button>
          {/each}
          <button type="button" class="provider-tile" class:on={papelOutro} aria-pressed={papelOutro}
                  onclick={() => { papelOutro = true; fPapel = ''; }}>{m.orqcfg_papel_outro()}</button>
        </div>
        {#if papelOutro}
          <input id="orq-papel" class="field-input" type="text" bind:value={fPapel} placeholder={m.orqcfg_papel_placeholder()} autocomplete="off" />
        {/if}
      </div>
    {/if}

    <div class="field">
      <span class="field-label">{m.orqcfg_onde_roda()}</span>
      <div class="provider-grid" role="group" aria-label={m.orqcfg_onde_roda()}>
        {#each PROVIDERS as p (p)}
          {@const tem = !!politica && contasLiberadas(politica.politica, politica.inventario, p).length > 0}
          <button type="button" class="provider-tile" class:on={fProvider === p} aria-pressed={fProvider === p}
                  disabled={!tem} onclick={() => trocarProvider(p)}>
            <ProviderGlyph provider={p} size={18} />
            <span>{providerName(p)}</span>
          </button>
        {/each}
      </div>
    </div>

    <div class="field">
      <label class="field-label" for="orq-conta">{m.orqcfg_conta()}</label>
      {#if contasDoProvider.length}
        <Select id="orq-conta" class="field-input" ariaLabel={m.orqcfg_conta()} value={fConta}
          opcoes={contasDoProvider.map((c) => ({ value: c.conta, label: c.apelido || c.conta, hint: c.apelido ? c.conta : undefined }))}
          onchange={(v) => { fConta = v; fModelo = ''; }} />
      {:else}
        <p class="os-hint" role="status">{m.orqcfg_nenhuma_conta()}</p>
      {/if}
    </div>

    <div class="os-grid">
      {#if fProvider !== 'codex'}
        <div class="field">
          <label class="field-label" for="orq-modelo">{m.orqcfg_modelo()}</label>
          <Select id="orq-modelo" class="field-input" ariaLabel={m.orqcfg_modelo()} value={fModelo} disabled={contaTravada}
            opcoes={[{ value: '', label: m.orqcfg_modelo_qualquer() },
                     ...modelos.map((x) => ({ value: x.id, label: x.name ?? x.id, hint: x.name ? x.id : (typeof x.context_length === 'number' ? `${Math.round(x.context_length / 1000)}k` : x.context_length || undefined) }))]}
            onchange={(v) => (fModelo = v)} />
          {#if invConta?.reduced}<p class="os-hint" role="status">{m.orqcfg_lista_reduzida()}</p>{/if}
        </div>
      {/if}
      {#if NIVEIS[fProvider]}
        <div class="field">
          <label class="field-label" for="orq-esforco">{m.orqcfg_esforco()}</label>
          <Select id="orq-esforco" class="field-input" ariaLabel={m.orqcfg_esforco()} value={fEsforco}
            opcoes={[{ value: '', label: m.orqcfg_esforco_padrao() }, ...NIVEIS[fProvider].map((n) => ({ value: n, label: n }))]}
            onchange={(v) => (fEsforco = v)} />
        </div>
      {/if}
      <div class="field">
        <span class="field-label">{m.orqcfg_cota()}</span>
        <p class="field-input os-cota" class:os-cota--alta={(cotaConta?.pct ?? 0) >= 80}>
          {cotaConta ? m.orqcfg_cota_usada({ pct: Math.round(cotaConta.pct), janela: cotaConta.rotulo }) : m.orqcfg_cota_sem()}
        </p>
      </div>
    </div>

    {#if estadoAtual?.viva && papelAtual}
      <div class="os-agora" class:bad={estadoAtual.divergente}>
        {#if estadoAtual.modeloMedido || estadoAtual.esforcoMedido}
          {m.orqcfg_agora_viva({ sessao: vivaAtual?.name ?? '', modelo: estadoAtual.modeloMedido ?? '—', esforco: estadoAtual.esforcoMedido ?? '—' })}
          {#if estadoAtual.divergente} {m.orqcfg_agora_diverge({ modelo: papelAtual.modelo || '—', esforco: papelAtual.esforco || '—' })}{/if}
        {:else}
          {m.orqcfg_agora_nao_medido()}
        {/if}
      </div>
    {/if}

    <div class="os-acao">
      {#if conflito}
        <p class="os-erro" role="alert">{m.orqcfg_arquivo_mudou()} <button type="button" class="os-link" onclick={carregar}>{m.orqcfg_recarregar()}</button></p>
      {:else if erro || erroCarga}<p class="os-erro" role="alert">{erro || erroCarga}</p>
      {:else if aviso}<p class="os-ok" class:os-ok--ruim={avisoRuim} role="status">{aviso}</p>{/if}
      <button type="button" class="os-primary" onclick={salvar} disabled={salvando || conflito || nRascunhos === 0}>
        {salvando ? m.orqcfg_salvando() : nRascunhos > 1 ? m.orqcfg_salvar_n({ n: nRascunhos }) : m.orqcfg_salvar_avisar()}
      </button>
      <p class="os-rodape">{m.orqcfg_rodape_papel({ arquivo: grupo?.arquivo?.split('/').pop() ?? '' })}</p>
    </div>
  {/if}
{/snippet}

<BottomSheet {open} {onClose} ariaLabel={m.orqcfg_titulo()} wide={isDesktop} centered={isDesktop} split={isDesktop}>
  {#if aba === 'contas'}
    <div class="os-col">
      <h2 class="sheet-title">{m.orqcfg_titulo()}</h2>
      {@render abas()}
      <div class="os-corpo">
        <OrquestracaoContas desktop={isDesktop} {papeis} onSalvo={(p) => clienteQuery.setQueryData(orqPolitica().queryKey, p)} />
      </div>
    </div>
  {:else if isDesktop}
    <div class="os-split">
      <aside class="os-pane os-esq">
        <h2 class="sheet-title">{m.orqcfg_titulo()}{#if grupo} <small class="os-gid">{m.orqcfg_sub_grupo({ gid: grupo.gid })}</small>{/if}</h2>
        {@render abas()}
        {@render listaPapeis()}
      </aside>
      <section class="os-pane os-dir">{@render formPapel()}</section>
    </div>
  {:else}
    <h2 class="sheet-title">{m.orqcfg_titulo()}</h2>
    {#if sel === null}
      {@render abas()}
      {@render listaPapeis()}
    {:else}
      <button type="button" class="os-back" onclick={() => (sel = null)}>‹ {m.orqcfg_aba_papeis()}</button>
      {@render formPapel()}
    {/if}
  {/if}
</BottomSheet>

<style>
  .sheet-title { font-size: var(--text-xl); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-3); }
  .os-gid { font-size: 12px; font-weight: 400; color: var(--text-muted); margin-left: var(--space-2); }
  .os-tabs { display: flex; gap: 6px; margin-bottom: var(--space-3); }
  .os-tab { padding: 7px 14px; border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary); border: 1px solid transparent; }
  .os-tab.on { color: var(--text-primary); border-color: var(--accent); background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .os-col { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .os-corpo { flex: 1; min-height: 0; }
  .os-split { display: grid; grid-template-columns: minmax(340px, 5fr) 6fr; grid-template-rows: minmax(0, 1fr); height: 100%; }
  .os-pane { min-height: 0; overflow-y: auto; display: flex; flex-direction: column; }
  .os-pane > :global(*) { flex: none; }
  .os-esq { padding-right: var(--space-5); border-right: 1px solid var(--border-subtle); }
  .os-dir { padding-left: var(--space-5); }
  .os-intro, .os-hint, .os-rodape { font-size: var(--text-sm); color: var(--text-secondary); margin: 0 0 var(--space-3); }
  .os-hint { margin: 6px 0 0; font-size: 12px; }
  .os-item { width: 100%; display: flex; align-items: center; gap: var(--space-2); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); padding: 10px var(--space-3); margin: 6px 0; text-align: left; color: inherit; background: transparent; }
  .os-item.sel { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .os-item.bad { border-color: color-mix(in srgb, var(--error) 60%, transparent); }
  .os-item--novo { border-style: dashed; color: var(--text-secondary); }
  .os-skel-item { pointer-events: none; }
  .os-skel-bar, .os-skel-chip {
    display: block; height: 12px; border-radius: 6px;
    background: linear-gradient(90deg, color-mix(in srgb, var(--text-muted) 14%, transparent) 25%, color-mix(in srgb, var(--text-muted) 28%, transparent) 50%, color-mix(in srgb, var(--text-muted) 14%, transparent) 75%);
    background-size: 200% 100%;
    animation: os-shimmer 1.4s ease-in-out infinite;
  }
  .os-skel-bar--sub { height: 9px; margin-top: 6px; }
  .os-skel-chips { display: flex; gap: 6px; margin-top: 8px; }
  .os-skel-chip { width: 56px; height: 16px; border-radius: 5px; }
  @keyframes os-shimmer { from { background-position: 200% 0; } to { background-position: -200% 0; } }
  .os-item-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
  .os-nome { font-weight: 600; font-size: 15px; }
  .os-sess { font-family: ui-monospace, monospace; font-size: 11.5px; color: var(--text-muted); }
  .os-chips { display: flex; gap: 5px; flex-wrap: wrap; }
  .os-chip { font-size: 10.5px; padding: 2px 7px; border-radius: 5px; background: var(--surface-raised); color: var(--text-secondary); font-weight: 600; text-transform: uppercase; letter-spacing: .03em; }
  .os-chip--ok { color: var(--success); }
  .os-chip--off { color: var(--text-muted); }
  .os-chip--bad { color: var(--error); background: color-mix(in srgb, var(--error) 18%, transparent); }
  .os-papeis-grid { grid-template-columns: repeat(3, 1fr); }
  .os-papeis-grid .provider-tile { text-transform: capitalize; }
  .os-chip--edit { color: var(--accent); background: color-mix(in srgb, var(--accent) 16%, transparent); font-size: 0.7em; vertical-align: middle; }
  .os-arrow { color: var(--text-muted); }
  .os-h { margin: 0 0 4px; font-size: var(--text-lg); font-weight: 600; }
  .os-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: var(--space-3); }
  .os-cota { margin: 0; color: var(--success); }
  .os-cota--alta { color: #e3b341; }
  .os-agora { margin-top: var(--space-3); padding: 10px 12px; border-radius: var(--radius-md); font-size: var(--text-sm); background: var(--surface-raised); border: 1px solid var(--border-subtle); }
  .os-agora.bad { border-color: color-mix(in srgb, var(--error) 50%, transparent); background: color-mix(in srgb, var(--error) 8%, transparent); }
  .os-acao { margin-top: auto; padding-top: var(--space-3); }
  .os-primary { width: 100%; height: 48px; background: var(--accent); color: #fff; border-radius: var(--radius-md); font-weight: 600; font-size: var(--text-base); }
  .os-primary:disabled { opacity: .5; }
  .os-rodape { text-align: center; margin: 6px 0 0; font-size: 12px; }
  .os-erro { color: var(--error); font-size: var(--text-sm); margin: 0 0 var(--space-2); }
  .os-ok { color: var(--success); font-size: var(--text-sm); margin: 0 0 var(--space-2); }
  .os-ok--ruim { color: #e3b341; }
  .os-link { color: var(--accent); font-size: 12px; background: none; padding: 0; }
  .os-vazio { flex: 1; display: grid; place-items: center; color: var(--text-muted); font-size: var(--text-sm); padding: var(--space-6); }
  .os-back { color: var(--accent); font-size: var(--text-sm); margin-bottom: var(--space-2); padding: 0; background: none; }
</style>
