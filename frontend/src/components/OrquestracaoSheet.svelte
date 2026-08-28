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
  import { comecarOrq, postOrqPapeis, removerPapel } from '../lib/api';
  import { clienteQuery, orqGrupo, orqPolitica } from '../lib/queries';
  import { quotaFeed } from '../lib/quotaFeed.svelte';
  import {
    agruparPorPapel, casarViva, contasLiberadas, estadoDoPapel, modelosLiberados, politicaDe,
    type ModoPapel, type OrqGrupo, type OrqPolitica, type Papel, type Provider,
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
  let fVez = $state('');

  const papeis = $derived(grupo?.papeis ?? []);
  const grupos = $derived(agruparPorPapel(papeis));
  // Papéis da skill orquestrar; um que já está no contrato não se repete.
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
  type Rascunho = { papel: string; sessao: string; provider: Provider; conta: string; modelo: string; esforco: string; vez: string };
  let rascunhos = $state<Record<string, Rascunho>>({});
  const nRascunhos = $derived(Object.keys(rascunhos).length);
  // Chave papel+vez: num papel que reveza, chavear só pelo nome faria o rascunho da 2ª conta
  // sobrescrever o da 1ª, e salvar mandaria uma linha só.
  const chaveDe = (i: number | 'novo') =>
    (i === 'novo' ? 'novo' : `${papeis[i]?.papel ?? ''}::${papeis[i]?.vez ?? ''}`);

  function guardarRascunho() {
    if (sel === null) return;
    const k = chaveDe(sel);
    const orig = sel === 'novo' ? null : papeis[sel];
    const papelNome = fPapel.trim();
    const r: Rascunho = { papel: papelNome, sessao: (fSessao.trim() || (papelNome ? sessaoDerivada(papelNome) : '')), provider: fProvider, conta: fConta, modelo: fModelo, esforco: fEsforco, vez: fVez };
    const igual = !!orig && orig.sessao === r.sessao && (orig.provider || 'claude') === r.provider
      && orig.conta === r.conta && orig.modelo === r.modelo && orig.esforco === r.esforco
      && (orig.vez ?? '') === r.vez;
    if (igual || (sel === 'novo' && !r.papel)) delete rascunhos[k]; else rascunhos[k] = r;
  }
  $effect(() => {
    void [fPapel, fSessao, fProvider, fConta, fModelo, fEsforco, fVez, sel];
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
    fVez = r?.vez ?? p?.vez ?? '';
  }

  // ── Rodízio ────────────────────────────────────────────────────────────────
  // O grupo do papel aberto: é dele que saem o modo e a fila de contas.
  const grupoSel = $derived(typeof sel === 'number'
    ? grupos.find((g) => g.linhas.some((l) => papeis.indexOf(l) === sel)) ?? null : null);
  const modoSel = $derived<ModoPapel>(fVez ? 'reveza' : 'unica');

  /**
   * Troca o modo do papel inteiro. Mexe em TODAS as linhas dele, não só na aberta: um papel com
   * metade das linhas numeradas e metade em `par` não significa nada, e o backend leria isso como
   * paralelo por causa de uma linha só.
   */
  function trocarModo(novo: ModoPapel) {
    const linhas = grupoSel?.linhas ?? [];
    if (novo === 'reveza') {
      // Renumera na ordem em que as linhas já estão: a ordem da tabela É a ordem do rodízio.
      linhas.forEach((l, n) => {
        if (papeis.indexOf(l) === sel) fVez = String(n + 1);
        else rascunhoDe(l, String(n + 1));
      });
      if (!linhas.length) fVez = '1';
    } else {
      // Volta pra conta única: só a linha aberta sobrevive. As outras saem pelo ✕ da fila, uma a
      // uma — apagar várias por baixo de um clique em "Uma conta" seria destruição sem pedido.
      fVez = '';
    }
  }

  /**
   * Descarta os rascunhos de UM papel. `adicionarConta` e `removerLinha` gravam só as linhas do
   * papel aberto, então zerar `rascunhos` inteiro apagava, sem aviso nenhum, a edição pendente de
   * outro papel que o usuário tinha deixado pra salvar depois.
   */
  function limparRascunhosDe(papel: string) {
    const pref = `${papel}::`;
    for (const k of Object.keys(rascunhos)) if (k.startsWith(pref)) delete rascunhos[k];
  }

  function rascunhoDe(l: Papel, vez: string) {
    rascunhos[`${l.papel}::${l.vez ?? ''}`] = {
      papel: l.papel, sessao: l.sessao, provider: (l.provider || 'claude') as Provider,
      conta: l.conta, modelo: l.modelo, esforco: l.esforco, vez,
    };
  }

  /** Cria a próxima linha do rodízio no contrato (grava sem avisar) e abre ela pra edição. */
  async function adicionarConta() {
    if (!grupo || salvando) return;
    const linhas = grupoSel?.linhas ?? [];
    const usadas = new Set(linhas.map((l) => (l.vez ?? '').trim()));
    let n = 1;
    while (usadas.has(String(n))) n++;
    // A primeira conta pode estar sem `vez` (papel que era único): ela vira a vez 1 na mesma
    // gravação, senão o contrato ficaria com uma linha sem número e outra numerada.
    const itens = linhas.map((l, idx) => ({
      papel: l.papel, sessao: l.sessao, provider: l.provider || 'claude',
      conta: l.conta, modelo: l.modelo, esforco: l.esforco, vez: (l.vez ?? '').trim() || String(idx + 1),
    }));
    const base = linhas[0] ?? null;
    itens.push({
      papel: fPapel.trim() || base?.papel || '', sessao: base?.sessao ?? fSessao,
      provider: (base?.provider || fProvider) as Provider, conta: '', modelo: '', esforco: '',
      vez: String(Math.max(n, itens.length + 1)),
    });
    salvando = true; erro = ''; aviso = '';
    try {
      const r = await postOrqPapeis(sessionName, { papeis: itens, mtime: grupo.mtime, avisar: false });
      clienteQuery.setQueryData(orqGrupo(sessionName).queryKey, { ...grupo, mtime: r.mtime, papeis: r.papeis.map((x) => ({ ...x, viva: null })) });
      limparRascunhosDe(itens[0].papel);
      sel = null;
    } catch (e) {
      const err = e as Error & { status?: number };
      if (err.status === 409) conflito = true; else erro = err.message;
    } finally {
      salvando = false;
    }
  }

  // Começar a orquestração: acorda ESTA sessão como árbitra. O 409 do backend explica o que falta
  // (grupo, papéis ou plano), e é ele que aparece na tela — um botão que não faz nada e não diz
  // por quê é o pior desfecho possível aqui.
  let comecando = $state(false);
  async function comecar() {
    if (comecando) return;
    comecando = true; erro = ''; aviso = ''; avisoRuim = false;
    try {
      const r = await comecarOrq(sessionName);
      aviso = r.entregue ? m.orqcfg_comecou({ plano: r.plano }) : m.orqcfg_comecou_fila({ plano: r.plano });
      onClose();
    } catch (e) {
      erro = (e as Error).message;
    } finally {
      comecando = false;
    }
  }

  /** Tira uma conta da fila (ou o papel inteiro, quando é a única linha). */
  async function removerLinha(l: Papel) {
    if (!grupo || salvando) return;
    salvando = true; erro = ''; aviso = '';
    try {
      const r = await removerPapel(sessionName, { papel: l.papel, vez: l.vez ?? '', mtime: grupo.mtime });
      clienteQuery.setQueryData(orqGrupo(sessionName).queryKey, { ...grupo, mtime: r.mtime, papeis: r.papeis.map((x) => ({ ...x, viva: null })) });
      limparRascunhosDe(l.papel);
      sel = null;
    } catch (e) {
      const err = e as Error & { status?: number };
      if (err.status === 409) conflito = true; else erro = err.message;
    } finally {
      salvando = false;
    }
  }
  function trocarProvider(p: Provider) {
    fProvider = p;
    fConta = (politica?.politica.find((c) => c.provider === p)?.conta) ?? '';
    fModelo = ''; fEsforco = '';
  }
  // Conta travada: o modelo é o primeiro liberado, sem escolha.
  $effect(() => { if (contaTravada && modelos[0]) fModelo = modelos[0].id; });

  // `avisar=false`: grava o contrato e volta pra lista pra continuar montando o time. O recado ao
  // árbitro sai uma vez, no fim — antes, cada papel salvo acordava ele com meia configuração.
  async function salvar(avisar = true) {
    guardarRascunho();
    const itens = Object.values(rascunhos).filter((r) => r.papel && r.conta);
    if (!grupo || !itens.length) return;
    salvando = true; erro = ''; aviso = ''; conflito = false;
    try {
      const r = await postOrqPapeis(sessionName, { papeis: itens, mtime: grupo.mtime, avisar });
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
      // `arbitro` só é reescrito quando houve aviso: salvando sem avisar o backend devolve null
      // (não foi procurar quem é), e escrever esse null apagaria o árbitro que a tela já conhecia.
      clienteQuery.setQueryData(orqGrupo(sessionName).queryKey,
        { ...grupo, mtime: r.mtime, papeis: lista, arbitro: avisar ? r.arbitro : grupo.arbitro });
      rascunhos = {};
      // Salvou sem avisar = ainda está montando o time: volta pra lista, pronto pro próximo papel.
      sel = avisar ? (typeof ultimo === 'number' ? ultimo : sel) : null;
      const arb = r.arbitro ?? '';
      avisoRuim = r.aviso === 'falhou' || r.aviso === 'sem_arbitro';
      aviso = r.aviso === 'nao_avisado' ? m.orqcfg_aviso_salvo_sem_avisar()
        : r.aviso === 'enviado' ? m.orqcfg_aviso_enviado({ arbitro: arb })
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
        <!-- Duas barras, na altura exata do card real (nome + linha de configuração): o esqueleto
             tem de nascer do tamanho do que vai chegar, senão a lista pula quando o dado entra. -->
        <div class="os-item os-skel-item">
          <span class="os-skel-bar" style="width: 38%"></span>
          <span class="os-skel-bar os-skel-bar--sub" style="width: 74%"></span>
        </div>
      {/each}
    </div>
  {:else if grupo}
    <p class="os-intro">{papeis.length ? m.orqcfg_papeis_intro() : m.orqcfg_sem_papeis()}</p>
    {#if papeis.length}
      <!-- Fica no fim da lista, não no formulário: começar é ação do GRUPO, e o formulário edita
           um papel. Sem plano o backend recusa com o motivo, que aparece aqui em cima. -->
      <button type="button" class="os-comecar" onclick={comecar} disabled={comecando}>
        {comecando ? m.orqcfg_comecando() : m.orqcfg_comecar()}
      </button>
    {/if}
    <!-- Chave com o índice: dois papéis de mesmo nome (contrato editado à mão) derrubavam a lista
         inteira, sem erro na tela (medido em 26/08/2026). -->
    <!-- Agrupado por papel: com rodízio o contrato tem uma linha por conta, e mostrá-las soltas
         faria o mesmo papel aparecer três vezes na lista — que é exatamente a gambiarra
         ("revisor A", "revisor B") que o rodízio veio substituir. -->
    {#each grupos as g (g.papel)}
      {#if g.modo !== 'unica'}
        <div class="os-grupo">
          <span class="os-grupo-nome">{g.papel}</span>
          <span class="os-grupo-modo">{m.orqcfg_modo_reveza_resumo({ n: g.linhas.length })}</span>
        </div>
      {/if}
    {#each g.linhas as p (`${p.papel}#${p.vez ?? ''}`)}
      {@const i = papeis.indexOf(p)}
      {@const viva = casarViva(p.sessao, sessoes)}
      {@const st = estadoDoPapel(p, viva)}
      <!-- Estado virou a bolinha ao lado do nome, e a configuração virou UMA linha em mono. Antes
           eram quatro chips de mesmo peso (estado, conta, modelo, divergência): tudo com o mesmo
           destaque é nada com destaque, e o card gastava quatro linhas por papel. Chip agora
           significa exceção — só a divergência tem um, e é por isso que ela finalmente é lida. -->
      <button type="button" class="os-item" class:sel={sel === i} class:bad={st.divergente} class:os-item--fila={g.modo !== 'unica'} onclick={() => escolher(i)}>
        <span class="os-item-topo">
          <span class="os-dot" class:on={st.viva} aria-hidden="true"></span>
          <!-- Dentro de um papel que reveza, repetir o nome em cada linha só faria ruído: o nome já
               está no cabeçalho do grupo, e o que distingue a linha é de quem é a vez. -->
          <!-- Chave composta, igual ao resto do arquivo: com `rascunhos[p.papel]` este chip nunca
               mais aparecia (nenhuma chave é o nome puro desde que a fila existe), e o usuário
               perdia o único sinal de que tinha edição não salva. -->
          <span class="os-nome">{g.modo === 'unica' ? p.papel : m.orqcfg_vez_n({ n: p.vez || '—' })}{#if rascunhos[`${p.papel}::${p.vez ?? ''}`]} <span class="os-chip os-chip--edit">{m.orqcfg_editado()}</span>{/if}</span>
          <span class="os-arrow" aria-hidden="true">›</span>
        </span>
        <!-- O estado continua no acessível: a bolinha é decorativa, quem lê tela ouve o texto. -->
        <span class="sr-only">{st.viva ? m.orqcfg_viva() : m.orqcfg_nao_aberta()}</span>
        <span class="os-cfg">
          <span class="os-sess">{viva?.name ?? p.sessao}</span> · {providerName(p.provider || 'claude')} · {p.conta}{p.modelo ? ` · ${p.modelo}` : ''}{p.esforco ? ` · ${p.esforco}` : ''}
        </span>
        {#if st.divergente}
          <span class="os-chip os-chip--bad">{m.orqcfg_rodando_em({ v: [st.conta === 'divergente' ? st.contaMedida : null, st.modelo === 'divergente' ? st.modeloMedido : null, st.esforco === 'divergente' ? st.esforcoMedido : null].filter(Boolean).join(' · ') })}</span>
        {/if}
      </button>
    {/each}
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

    {#if sel !== 'novo'}
      <!-- Modo do papel, nos mesmos tiles do "Onde roda" logo abaixo: é a mesma classe de escolha,
           e inventar um controle novo pra ela seria estranheza sem motivo. -->
      <div class="field">
        <span class="field-label">{m.orqcfg_modo_titulo()}</span>
        <div class="provider-grid os-modo" role="group" aria-label={m.orqcfg_modo_titulo()}>
          <!-- Só dois modos. "Rodar Tasks em paralelo" existe na skill e é outra coisa: Tasks
               independentes, uma worktree cada, cada uma com seu executor e seu revisor. Aquilo é
               decisão do PLANO, não da configuração de um papel — ver references/paralelo-worktree.md. -->
          {#each [['unica', m.orqcfg_modo_unica(), m.orqcfg_modo_unica_ajuda()], ['reveza', m.orqcfg_modo_reveza(), m.orqcfg_modo_reveza_ajuda()]] as [id, rotulo, ajuda] (id)}
            <!-- "Uma conta" fica travado enquanto a fila tem mais de uma linha: aceitar o clique
                 ali gravava uma linha SEM vez ao lado das numeradas, que o backend lê como uma
                 conta a MAIS (o rodízio virava %4), com as outras intactas e nenhum aviso. Esvazie
                 a fila pelo ✕ primeiro — remover conta é ação explícita, não efeito colateral. -->
            {@const travado = id === 'unica' && (grupoSel?.linhas.length ?? 0) > 1}
            <button type="button" class="provider-tile os-modo-tile" class:on={modoSel === id}
                    aria-pressed={modoSel === id} disabled={travado}
                    title={travado ? m.orqcfg_modo_unica_travada() : undefined}
                    onclick={() => trocarModo(id as ModoPapel)}>
              <span class="os-modo-nome">{rotulo}</span>
              <span class="os-modo-ajuda">{travado ? m.orqcfg_modo_unica_travada() : ajuda}</span>
            </button>
          {/each}
        </div>
      </div>

      {#if modoSel !== 'unica' && grupoSel}
        <div class="field">
          <span class="field-label">
            {m.orqcfg_fila_titulo()}
          </span>
          <div class="os-fila">
            {#each grupoSel.linhas as l, n (`${l.papel}#${l.vez ?? ''}`)}
              {@const idx = papeis.indexOf(l)}
              <div class="os-fila-linha" class:agora={idx === sel}>
                <span class="os-fila-n">{n + 1}</span>
                <button type="button" class="os-fila-abrir" onclick={() => escolher(idx)}>
                  {providerName(l.provider || 'claude')} · {l.conta || m.orqcfg_fila_sem_conta()}{l.modelo ? ` · ${l.modelo}` : ''}{l.esforco ? ` · ${l.esforco}` : ''}
                </button>
                <button type="button" class="os-fila-x" onclick={() => removerLinha(l)} disabled={salvando}
                        aria-label={m.orqcfg_fila_remover()} title={m.orqcfg_fila_remover()}>✕</button>
              </div>
            {/each}
            <button type="button" class="os-fila-add" onclick={adicionarConta} disabled={salvando}>
              + {m.orqcfg_fila_adicionar()}
            </button>
          </div>
        </div>
      {/if}
    {/if}

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
      <!-- Dois caminhos porque são dois momentos: montar o time (salva e volta pra lista, sem
           acordar ninguém) e fechar a configuração (salva e avisa o árbitro, uma vez só). -->
      <div class="os-botoes">
        <button type="button" class="os-secundario" onclick={() => salvar(false)} disabled={salvando || conflito || nRascunhos === 0}>
          {m.orqcfg_salvar_continuar()}
        </button>
        <button type="button" class="os-primary" onclick={() => salvar(true)} disabled={salvando || conflito || nRascunhos === 0}>
          {salvando ? m.orqcfg_salvando() : nRascunhos > 1 ? m.orqcfg_salvar_n({ n: nRascunhos }) : m.orqcfg_salvar_avisar()}
        </button>
      </div>
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
  .os-item { width: 100%; display: block; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); padding: 10px var(--space-3); margin: 6px 0; text-align: left; color: inherit; background: transparent; }
  .os-item.sel { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .os-item.bad { border-color: color-mix(in srgb, var(--error) 60%, transparent); }
  .os-item--novo { border-style: dashed; color: var(--text-secondary); }
  .os-skel-item { pointer-events: none; }
  .os-skel-bar {
    display: block; height: 12px; border-radius: 6px;
    background: linear-gradient(90deg, color-mix(in srgb, var(--text-muted) 14%, transparent) 25%, color-mix(in srgb, var(--text-muted) 28%, transparent) 50%, color-mix(in srgb, var(--text-muted) 14%, transparent) 75%);
    background-size: 200% 100%;
    animation: os-shimmer 1.4s ease-in-out infinite;
  }
  .os-skel-bar--sub { height: 9px; margin-top: 8px; }
  @keyframes os-shimmer { from { background-position: 200% 0; } to { background-position: -200% 0; } }
  .os-item-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
  .os-item-topo { display: flex; align-items: center; gap: var(--space-2); }
  /* Cabeçalho do papel que reveza: o nome sai dos cards e vem pra cá, uma vez só. */
  .os-comecar { width: 100%; height: 40px; margin-bottom: var(--space-2); border-radius: var(--radius-md); border: 1px solid var(--accent); background: color-mix(in srgb, var(--accent) 14%, transparent); color: var(--accent); font-weight: 600; font-size: var(--text-sm); }
  .os-comecar:disabled { opacity: .55; }
  .os-modo { grid-template-columns: repeat(3, 1fr); }
  .os-modo-tile { flex-direction: column; align-items: flex-start; gap: 2px; height: auto; padding: 9px 11px; }
  .os-modo-nome { font-weight: 600; font-size: var(--text-sm); }
  .os-modo-ajuda { font-size: 11px; color: var(--text-muted); line-height: 1.35; text-align: left; }
  /* `--surface-inset`: é área de conteúdo dentro do painel, então acompanha o véu do papel de parede. */
  .os-fila { border: 1px solid var(--border-subtle); border-radius: var(--radius-md); overflow: hidden; background: var(--surface-inset); }
  .os-fila-linha { display: flex; align-items: center; gap: var(--space-2); padding: 7px 10px; }
  .os-fila-linha + .os-fila-linha, .os-fila-add { border-top: 1px solid var(--border-subtle); }
  .os-fila-linha.agora { background: color-mix(in srgb, var(--accent) 12%, transparent); }
  .os-fila-n { font-family: ui-monospace, monospace; font-size: 11px; color: var(--text-muted); width: 14px; flex: none; }
  .os-fila-linha.agora .os-fila-n { color: var(--accent); }
  .os-fila-abrir { flex: 1; min-width: 0; text-align: left; background: none; color: inherit; font-family: ui-monospace, monospace; font-size: 11.5px; }
  .os-fila-x { color: var(--text-muted); background: none; padding: 2px 4px; }
  .os-fila-x:disabled { opacity: .4; }
  .os-fila-add { display: block; width: 100%; text-align: left; padding: 8px 10px; background: none; color: var(--accent); font-size: var(--text-sm); }
  .os-fila-add:disabled { opacity: .5; }
  .os-grupo { display: flex; align-items: baseline; gap: var(--space-2); margin: 12px 2px 2px; }
  .os-grupo-nome { font-weight: 600; font-size: 15px; }
  .os-grupo-modo { font-size: 11.5px; color: var(--accent); }
  /* Linha de uma fila: recuada e sem borda própria, pra ler como parte do papel acima. */
  .os-item--fila { margin-left: var(--space-3); border-color: transparent; }
  .os-item--fila.sel { border-color: var(--accent); }
  .os-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--text-muted); opacity: .45; flex: none; }
  .os-dot.on { background: var(--success); opacity: 1; box-shadow: 0 0 0 3px color-mix(in srgb, var(--success) 14%, transparent); }
  .os-nome { font-weight: 600; font-size: 15px; flex: 1; min-width: 0; }
  /* Uma linha só, em mono, com a sessão mais apagada que o resto: a configuração continua legível
     de relance sem disputar atenção com o nome do papel. */
  .os-cfg { display: block; font-family: ui-monospace, monospace; font-size: 11.5px; color: var(--text-muted); margin-top: 3px; line-height: 1.5; }
  .os-sess { color: color-mix(in srgb, var(--text-muted) 72%, transparent); }
  .os-chip { font-size: 10.5px; padding: 2px 7px; border-radius: 5px; background: var(--surface-raised); color: var(--text-secondary); font-weight: 600; text-transform: uppercase; letter-spacing: .03em; }
  .os-chip--bad { color: var(--error); background: color-mix(in srgb, var(--error) 18%, transparent); display: inline-block; margin-top: 6px; }
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
  .os-botoes { display: flex; gap: var(--space-2); }
  .os-primary { flex: 1 1 60%; height: 48px; background: var(--accent); color: #fff; border-radius: var(--radius-md); font-weight: 600; font-size: var(--text-base); }
  .os-primary:disabled { opacity: .5; }
  /* `--surface-raised`, não `--bg-elevated`: com papel de parede este botão precisa acompanhar o
     véu do painel, senão vira um retângulo chapado sobre a foto. */
  .os-secundario { flex: 1 1 40%; height: 48px; background: var(--surface-raised); color: var(--text); border: 1px solid var(--border); border-radius: var(--radius-md); font-weight: 600; font-size: var(--text-sm); }
  .os-secundario:disabled { opacity: .5; }
  .os-rodape { text-align: center; margin: 6px 0 0; font-size: 12px; }
  .os-erro { color: var(--error); font-size: var(--text-sm); margin: 0 0 var(--space-2); }
  .os-ok { color: var(--success); font-size: var(--text-sm); margin: 0 0 var(--space-2); }
  .os-ok--ruim { color: #e3b341; }
  .os-link { color: var(--accent); font-size: 12px; background: none; padding: 0; }
  .os-vazio { flex: 1; display: grid; place-items: center; color: var(--text-muted); font-size: var(--text-sm); padding: var(--space-6); }
  .os-back { color: var(--accent); font-size: var(--text-sm); margin-bottom: var(--space-2); padding: 0; background: none; }
</style>
