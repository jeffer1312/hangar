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
  import { providerName, SESSION_PROVIDERS, type ModelOption } from '@hangar/core';
  import { untrack } from 'svelte';
  import { createQuery } from '@tanstack/svelte-query';
  import { comecarOrq, postOrqPapeis, removerPapel } from '@hangar/core';
  import { clienteQuery, motores, orqGrupo, orqPolitica } from '../lib/queries';
  import { quotaFeed } from '../lib/quotaFeed.svelte';
  import { segredos } from '../lib/segredos.svelte';
  import SessionOpeningFields from './SessionOpeningFields.svelte';
  import {
    agruparPorPapel, casarViva, contasEmUso, contasLiberadas, estadoDoPapel, etapasDoTime, faixaDe,
    modelosLiberados, mudancasDe, politicaDe, rotuloModelo,
    type AberturaPapel, type CampoMudado, type ModoPapel, type OrqGrupo, type OrqPolitica, type Papel, type Provider,
  } from '@hangar/core';
  import type { SessionInfo } from '@hangar/core';

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

  const PROVIDERS = SESSION_PROVIDERS;

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
  let fHeadless = $state(false);
  let fPermissao = $state('');
  let fMotor = $state('');
  let fJev = $state(false);
  let fSubagente = $state('');
  let fPerfil = $state('');
  let fJanela = $state('');
  const JANELAS = ['30', '40', '60', '70', '80'];

  const qMotores = createQuery(() => ({ ...motores(null), enabled: open }), () => clienteQuery);
  const listaMotores = $derived(qMotores.data?.motores ?? {});

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
  // Pi manda o contexto em texto ("200k") e os outros em número: o campo de modelo lê os dois.
  const modelos = $derived<ModelOption[]>(modelosLiberados(invConta, polConta).map((x) => ({
    id: x.id, name: x.name, efforts: x.efforts,
    ...(typeof x.context_length === 'number' ? { context_length: x.context_length }
      : x.context_length ? { context: x.context_length } : {}),
  })));
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

  // Já ligado sem chave no servidor: o interruptor continua na tela, senão não haveria como desligar.
  const temJev = $derived(segredos.temChave('jev_api_key') || fJev);

  const aberturaDe = (p: Papel | null | undefined): AberturaPapel => ({
    headless: !!p?.headless, permissao: p?.permissao ?? '', motor: p?.motor ?? '',
    jev: !!p?.jev, subagente: p?.subagente ?? '', perfil: p?.perfil ?? '',
  });
  /**
   * O que o formulário grava: só os valores que valem pro provider escolhido. Filtrar aqui, e não
   * só na tela, garante que nenhum caminho de gravação mande um motor pro codex.
   */
  function abertura(): AberturaPapel {
    const claude = fProvider === 'claude';
    const headless = (claude || fProvider === 'codex') && fHeadless;
    return {
      headless,
      permissao: claude || headless ? fPermissao : '',
      motor: claude ? fMotor : '',
      jev: fJev,
      subagente: claude && !fMotor ? fSubagente : '',
      perfil: fProvider === 'omp' ? fPerfil.trim() : '',
    };
  }

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
  type Rascunho = { papel: string; sessao: string; provider: Provider; conta: string; modelo: string; esforco: string; vez: string; janela: string } & AberturaPapel;
  let rascunhos = $state<Record<string, Rascunho>>({});
  // Chave papel+vez: num papel que reveza, chavear só pelo nome faria o rascunho da 2ª conta
  // sobrescrever o da 1ª, e salvar mandaria uma linha só.
  const chaveDe = (i: number | 'novo') =>
    (i === 'novo' ? 'novo' : `${papeis[i]?.papel ?? ''}::${papeis[i]?.vez ?? ''}`);

  function guardarRascunho() {
    if (sel === null) return;
    const k = chaveDe(sel);
    const orig = sel === 'novo' ? null : papeis[sel];
    const papelNome = fPapel.trim();
    const r: Rascunho = { papel: papelNome, sessao: (fSessao.trim() || (papelNome ? sessaoDerivada(papelNome) : '')), provider: fProvider, conta: fConta, modelo: fModelo, esforco: fEsforco, vez: fVez, janela: fJanela, ...abertura() };
    const a = aberturaDe(orig);
    const igual = !!orig && orig.sessao === r.sessao && (orig.provider || 'claude') === r.provider
      && orig.conta === r.conta && orig.modelo === r.modelo && orig.esforco === r.esforco
      && (orig.vez ?? '') === r.vez && (orig.janela ?? '') === r.janela
      && (Object.keys(a) as (keyof AberturaPapel)[]).every((c) => a[c] === r[c]);
    if (igual || (sel === 'novo' && !r.papel)) delete rascunhos[k]; else rascunhos[k] = r;
  }
  $effect(() => {
    void [fPapel, fSessao, fProvider, fConta, fModelo, fEsforco, fVez, fJanela, fHeadless, fPermissao, fMotor, fJev, fSubagente, fPerfil, sel];
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
    fJanela = r?.janela ?? p?.janela ?? '';
    const a = r ?? aberturaDe(p);
    fHeadless = a.headless; fPermissao = a.permissao; fMotor = a.motor; fJev = a.jev; fSubagente = a.subagente;
    fPerfil = a.perfil;
    // Papel novo nasce no padrão do servidor, como a folha de nova sessão.
    if (i === 'novo' && !r) fJev = segredos.ligado('jev_padrao');
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
      conta: l.conta, modelo: l.modelo, esforco: l.esforco, vez, janela: l.janela ?? '', ...aberturaDe(l),
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
      janela: l.janela ?? '', ...aberturaDe(l),
    }));
    const base = linhas[0] ?? null;
    itens.push({
      papel: fPapel.trim() || base?.papel || '', sessao: base?.sessao ?? fSessao,
      provider: (base?.provider || fProvider) as Provider, conta: '', modelo: '', esforco: '',
      vez: String(Math.max(n, itens.length + 1)), janela: base?.janela ?? '', ...aberturaDe(null),
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
    // O Jev vale em qualquer provider; o resto da abertura é por provider e volta ao padrão.
    fHeadless = false; fPermissao = ''; fMotor = ''; fSubagente = ''; fPerfil = '';
  }
  // Conta travada: o modelo é o primeiro liberado, sem escolha.
  $effect(() => { if (contaTravada && modelos[0]) fModelo = modelos[0].id; });

  // ── Lista em etapas ────────────────────────────────────────────────────────
  const chaveLinha = (p: Papel) => `${p.papel}::${p.vez ?? ''}`;
  /** A linha como vai ficar: com o rascunho por cima, para a lista mostrar a edição pendente. */
  const vista = (p: Papel): Papel => {
    const r = rascunhos[chaveLinha(p)];
    return r ? { ...p, ...r } : p;
  };
  const etapas = $derived(etapasDoTime(papeis));
  const usoContas = $derived(contasEmUso(papeis.map(vista)));
  const FINALIDADE: Record<string, () => string> = {
    'árbitro': m.orqcfg_fim_arbitro, executor: m.orqcfg_fim_executor, revisor: m.orqcfg_fim_revisor,
    'revisão final': m.orqcfg_fim_revisao_final, retrospectiva: m.orqcfg_fim_retrospectiva,
    'par de research': m.orqcfg_fim_research,
  };
  const finalidade = (base: string) => FINALIDADE[base.toLowerCase()]?.() ?? null;

  const invDe = (p: Papel) =>
    politica?.inventario.find((i) => i.provider === (p.provider || 'claude') && i.conta === p.conta) ?? null;
  /** Apelido da conta quando a política dá um ("Rafael e Viana"); senão o id. */
  const rotuloConta = (conta: string) =>
    politica?.inventario.find((i) => i.conta === conta && i.apelido)?.apelido || conta;
  /** Nome legível do modelo ("Opus (1M context)") quando o inventário da conta conhece o id. */
  const nomeModelo = (p: Papel) =>
    p.modelo ? (invDe(p)?.modelos.find((x) => x.id === p.modelo)?.name ?? rotuloModelo(p.modelo)) : m.criar_padrao();
  /** Pior janela de cota da conta, em %; null = sem leitura. */
  function cotaDe(provider: string, conta: string, idCota?: string | null): number | null {
    const id = idCota ?? politica?.inventario.find((i) => i.provider === provider && i.conta === conta)?.id_cota ?? null;
    const c = id ? quotaFeed.contas.find((x) => x.id === id) : null;
    return c?.janelas.length ? Math.max(...c.janelas.map((j) => j.pct)) : null;
  }
  const COTA_TROCA = 90;
  /** Conta liberada do mesmo provider com mais folga, para a troca rápida. Só com leitura de cota:
   * trocar às cegas pode mandar o papel para outra conta tão cheia quanto. */
  function contaComFolga(p: Papel): { conta: string; pct: number } | null {
    if (!politica) return null;
    const prov = (p.provider || 'claude') as Provider;
    return contasLiberadas(politica.politica, politica.inventario, prov)
      .filter((c) => c.conta !== p.conta)
      .map((c) => ({ conta: c.conta, pct: cotaDe(prov, c.conta) }))
      .filter((c): c is { conta: string; pct: number } => c.pct !== null && c.pct < COTA_TROCA - 20)
      .sort((a, b) => a.pct - b.pct)[0] ?? null;
  }

  // Andamento: o plano aparece no SessionInfo de quem o executa. O árbitro vem primeiro.
  const vivasDoTime = $derived(etapas.flatMap((e) => e.linhas.map((l) => casarViva(l.sessao, sessoes))).filter((s): s is SessionInfo => !!s));
  const andamento = $derived.by(() => {
    const s = vivasDoTime.find((v) => v.plan_task_total);
    return s?.plan_task && s.plan_task_total ? { t: s.plan_task, total: s.plan_task_total } : null;
  });

  // Troca rápida de conta e modelo, aberta na própria linha.
  let rapida = $state<string | null>(null);
  function editarRapido(p: Papel, mud: { conta?: string; modelo?: string }) {
    const k = chaveLinha(p);
    const atual = vista(p);
    const prov = (atual.provider || 'claude') as Provider;
    const r: Rascunho = {
      papel: atual.papel, sessao: atual.sessao, provider: prov,
      conta: mud.conta ?? atual.conta,
      modelo: mud.conta !== undefined ? modeloQueSegue(prov, mud.conta, atual.modelo) : (mud.modelo ?? atual.modelo),
      esforco: atual.esforco, vez: atual.vez ?? '', janela: atual.janela ?? '', ...aberturaDe(atual),
    };
    if (mudancasDe(p, r).length) rascunhos[k] = r; else delete rascunhos[k];
    const i = papeis.indexOf(p);
    if (sel === i) escolher(i);
  }

  const ROTULO_CAMPO: Record<CampoMudado, () => string> = {
    provider: m.comum_provider, conta: m.orqcfg_conta, modelo: m.composer_modelo, esforco: m.composer_esforco,
    janela: m.orqcfg_janela, headless: m.criar_modo_exec, permissao: m.criar_permissao, motor: m.comum_motor,
    subagente: m.criar_subagente, jev: m.criar_jev, perfil: m.criar_perfil_omp, sessao: m.orqcfg_campo_sessao,
  };
  function valorCampo(c: CampoMudado, v: string): string {
    if (c === 'headless') return v ? m.criar_modo_exec_headless() : m.criar_modo_exec_tmux();
    if (c === 'jev') return v ? m.orqcfg_ligado() : m.orqcfg_desligado();
    if (c === 'janela' && v) return `${v}%`;
    if (c === 'modelo' && v) return rotuloModelo(v);
    if (c === 'conta' && v) return rotuloConta(v);
    return v || m.criar_padrao();
  }
  const pendentes = $derived(Object.entries(rascunhos).map(([k, r]) => {
    const orig = papeis.find((p) => chaveLinha(p) === k) ?? null;
    return { k, papel: r.papel || m.orqcfg_novo_papel(), novo: !orig, mudancas: mudancasDe(orig, r) };
  }).filter((x) => x.novo || x.mudancas.length));

  function descartar() {
    rascunhos = {};
    if (sel !== null) escolher(sel);
  }

  /** Grava linhas no contrato e põe o resultado no cache. Serve ao salvar e à troca rápida.
   * Devolve o índice da última linha gravada, ou -1 se não gravou. */
  async function gravar(itens: Rascunho[], avisar: boolean): Promise<number> {
    if (!grupo || !itens.length) return -1;
    salvando = true; erro = ''; aviso = ''; conflito = false;
    try {
      const r = await postOrqPapeis(sessionName, { papeis: itens, mtime: grupo.mtime, avisar });
      const lista = [...grupo.papeis];
      let ultimo = -1;
      for (const p of r.papeis) {
        const novo: Papel = { ...p, viva: casarViva(p.sessao, sessoes)?.name ?? null };
        // Papel + vez: numa etapa com rodízio, casar só pelo nome sobrescreveria a outra conta.
        const i = lista.findIndex((x) => chaveLinha(x).toLowerCase() === chaveLinha(novo).toLowerCase());
        if (i >= 0) lista[i] = { ...lista[i], ...novo }; else lista.push(novo);
        ultimo = i >= 0 ? i : lista.length - 1;
      }
      // Escreve no cache, não num state local: o painel reabre com o que foi salvo, sem esperar
      // uma releitura do disco que já sabemos como terminaria.
      // `arbitro` só é reescrito quando houve aviso: salvando sem avisar o backend devolve null
      // (não foi procurar quem é), e escrever esse null apagaria o árbitro que a tela já conhecia.
      clienteQuery.setQueryData(orqGrupo(sessionName).queryKey,
        { ...grupo, mtime: r.mtime, papeis: lista, arbitro: avisar ? r.arbitro : grupo.arbitro });
      const arb = r.arbitro ?? '';
      avisoRuim = r.aviso === 'falhou' || r.aviso === 'sem_arbitro';
      aviso = r.aviso === 'nao_avisado' ? m.orqcfg_aviso_salvo_sem_avisar()
        : r.aviso === 'enviado' ? m.orqcfg_aviso_enviado({ arbitro: arb })
        : r.aviso === 'enfileirado' ? m.orqcfg_aviso_enfileirado({ arbitro: arb })
        : r.aviso === 'sem_arbitro' ? m.orqcfg_aviso_sem_arbitro()
        : m.orqcfg_aviso_falhou({ erro: r.erro ?? '' });
      return ultimo;
    } catch (e) {
      const err = e as Error & { status?: number };
      if (err.status === 409) conflito = true; else erro = err.message;
      return -1;
    } finally {
      salvando = false;
    }
  }

  /** O modelo só acompanha a troca de conta se a conta nova o libera; senão fica no padrão dela. */
  function modeloQueSegue(prov: Provider, conta: string, modelo: string): string {
    const inv = politica?.inventario.find((i) => i.provider === prov && i.conta === conta) ?? null;
    const pol = politica ? politicaDe(politica.politica, prov, conta, politica.inventario) : null;
    const libera = !!pol?.modelos.includes('*') || modelosLiberados(inv, pol).some((x) => x.id === modelo);
    return libera ? modelo : '';
  }

  /** Cota no limite: troca a conta da linha pela de mais folga e já avisa o árbitro. */
  async function trocarAgora(p: Papel, conta: string) {
    const atual = vista(p);
    const prov = (atual.provider || 'claude') as Provider;
    const modelo = modeloQueSegue(prov, conta, atual.modelo);
    const aberta = sel === papeis.indexOf(p);
    const i = await gravar([{ papel: atual.papel, sessao: atual.sessao, provider: prov, conta, modelo, esforco: atual.esforco,
      vez: atual.vez ?? '', janela: atual.janela ?? '', ...aberturaDe(atual) }], true);
    if (i < 0) return;
    delete rascunhos[chaveLinha(p)];
    if (aberta) escolher(i);
  }

  // `avisar=false`: grava o contrato e volta pra lista pra continuar montando o time. O recado ao
  // árbitro sai uma vez, no fim — antes, cada papel salvo acordava ele com meia configuração.
  async function salvar(avisar = true) {
    guardarRascunho();
    const itens = Object.values(rascunhos).filter((r) => r.papel && r.conta);
    const ultimo = await gravar(itens, avisar);
    if (ultimo < 0) return;
    rascunhos = {};
    // Salvou sem avisar = ainda está montando o time: volta pra lista, pronto pro próximo papel.
    sel = avisar ? ultimo : null;
  }
</script>

{#snippet abas()}
  <div class="os-tabs" role="tablist">
    <button type="button" role="tab" class="os-tab" class:on={aba === 'papeis'} aria-selected={aba === 'papeis'} onclick={() => (aba = 'papeis')}>{m.orqcfg_aba_papeis()}</button>
    <button type="button" role="tab" class="os-tab" class:on={aba === 'contas'} aria-selected={aba === 'contas'} onclick={() => (aba = 'contas')}>{m.orqcfg_aba_contas()}</button>
  </div>
{/snippet}

<!-- Uma linha do contrato. Com título é etapa de uma linha só; sem, é uma faixa (ou vez do
     rodízio) dentro da etapa. -->
{#snippet celula(orig: Papel, titulo: string | null, sub: string | null)}
  {@const p = vista(orig)}
  {@const i = papeis.indexOf(orig)}
  {@const k = chaveLinha(orig)}
  {@const viva = casarViva(p.sessao, sessoes)}
  {@const st = estadoDoPapel(p, viva)}
  {@const prov = (p.provider || 'claude') as Provider}
  {@const pct = cotaDe(prov, p.conta, orig.conta === p.conta ? orig.id_cota : null)}
  {@const divide = usoContas.get(`${prov}::${p.conta}`) ?? 0}
  {@const folga = pct !== null && pct >= COTA_TROCA ? contaComFolga(p) : null}
  {@const fx = faixaDe(p.papel).faixa}
  {@const inv = politica?.inventario.find((x) => x.provider === prov && x.conta === p.conta) ?? null}
  {@const pol = politica ? politicaDe(politica.politica, prov, p.conta, politica.inventario) : null}
  <div class="os-cel" class:em-faixa={!titulo} class:sel={sel === i}>
    <button type="button" class="os-cel-abrir" onclick={() => escolher(i)}>
      <span class="os-oque">
        {titulo ?? (fx ? m.orqcfg_faixa_n({ f: fx }) : p.vez ? m.orqcfg_vez_n({ n: p.vez }) : p.papel)}
        {#if rascunhos[k]}<span class="os-chip os-chip--edit">{m.orqcfg_editado()}</span>{/if}
      </span>
      <span class="os-quem">
        <!-- Cada pedaço não quebra por dentro: a linha só dobra entre um e outro. -->
        <span class="os-seg" title={p.conta}>{sub ? `${sub} · ` : ''}{p.conta ? rotuloConta(p.conta) : m.orqcfg_fila_sem_conta()}</span>{#if pct !== null}{' '}<span class="os-seg">{'· '}<span class:os-alta={pct >= 80}>{m.orqcfg_cota_pct({ pct: Math.round(pct) })}</span></span>{/if}{#if divide > 1}{' '}<span class="os-seg">{'· '}<span class="os-divide" title={m.orqcfg_conta_dividida_ajuda()}>{m.orqcfg_conta_dividida({ n: divide })}</span></span>{/if}
      </span>
    </button>
    <button type="button" class="os-modelo" aria-expanded={rapida === k} title={m.orqcfg_trocar_rapido()}
            onclick={() => (rapida = rapida === k ? null : k)}>
      <ProviderGlyph provider={prov} size={13} />{nomeModelo(p)}{#if p.esforco}<i>{p.esforco}</i>{/if}
    </button>
    <span class="os-est" class:viva={st.viva} class:trab={viva?.state === 'working'} class:bad={st.divergente} aria-hidden="true">
      {st.divergente ? '!' : st.viva ? '●' : '○'}
    </span>
    <span class="sr-only">{viva?.state === 'working' ? m.orqcfg_trabalhando() : st.viva ? m.orqcfg_viva() : m.orqcfg_nao_aberta()}</span>
    {#if st.divergente}
      <span class="os-aviso">{m.orqcfg_rodando_em({ v: [st.conta === 'divergente' ? st.contaMedida : null, st.modelo === 'divergente' ? st.modeloMedido : null, st.esforco === 'divergente' ? st.esforcoMedido : null].filter(Boolean).join(' · ') })}</span>
    {/if}
    {#if folga}
      <button type="button" class="os-trocar" disabled={salvando} onclick={() => trocarAgora(orig, folga.conta)}>
        {m.orqcfg_trocar_para({ conta: folga.conta, pct: Math.round(folga.pct) })}
      </button>
    {/if}
    {#if rapida === k}
      <div class="os-rapida">
        <Select ariaLabel={m.orqcfg_conta()} class="field-input" value={p.conta}
          opcoes={politica ? contasLiberadas(politica.politica, politica.inventario, prov).map((c) => {
            const q = cotaDe(prov, c.conta);
            return { value: c.conta, label: c.apelido || c.conta, hint: q !== null ? m.orqcfg_cota_pct({ pct: Math.round(q) }) : undefined };
          }) : []}
          onchange={(v) => editarRapido(orig, { conta: v })} />
        <Select ariaLabel={m.composer_modelo()} class="field-input" value={p.modelo} disabled={!!pol && !pol.trocar}
          opcoes={[{ value: '', label: m.criar_padrao() },
                   // O modelo gravado pode não estar no catálogo da conta: fica na lista com o
                   // rótulo curto, senão o campo mostraria o id cru sem opção correspondente.
                   ...(p.modelo && !modelosLiberados(inv, pol).some((x) => x.id === p.modelo) ? [{ value: p.modelo, label: rotuloModelo(p.modelo) }] : []),
                   ...modelosLiberados(inv, pol).map((x) => ({ value: x.id, label: x.name ?? x.id }))]}
          onchange={(v) => editarRapido(orig, { modelo: v })} />
      </div>
    {/if}
  </div>
{/snippet}

<!-- Mudanças pendentes: o que vai ser gravado, em palavras, antes de acordar o árbitro. -->
{#snippet barra()}
  {#if pendentes.length || erro || erroCarga || aviso || conflito}
    <div class="os-barra">
      {#if conflito}
        <p class="os-erro" role="alert">{m.orqcfg_arquivo_mudou()} <button type="button" class="os-link" onclick={carregar}>{m.orqcfg_recarregar()}</button></p>
      {:else if erro || erroCarga}<p class="os-erro" role="alert">{erro || erroCarga}</p>
      {:else if aviso && !pendentes.length}<p class="os-ok" class:os-ok--ruim={avisoRuim} role="status">{aviso}</p>{/if}
      {#if pendentes.length}
        <p class="os-barra-tit">{pendentes.length === 1 ? m.orqcfg_mudanca_1() : m.orqcfg_mudancas_n({ n: pendentes.length })}</p>
        <ul class="os-barra-lista">
          {#each pendentes as x (x.k)}
            <li><b>{x.papel}</b>{#if x.novo} · {m.orqcfg_mudanca_novo()}{:else}: {x.mudancas.map((c) => `${ROTULO_CAMPO[c.campo]().toLowerCase()} ${valorCampo(c.campo, c.de)} → ${valorCampo(c.campo, c.para)}`).join('; ')}{/if}</li>
          {/each}
        </ul>
        <!-- Dois caminhos porque são dois momentos: montar o time (salva sem acordar ninguém) e
             fechar a configuração (salva e avisa o árbitro, uma vez só). -->
        <div class="os-botoes">
          <button type="button" class="os-link" onclick={descartar} disabled={salvando}>{m.orqcfg_descartar()}</button>
          <button type="button" class="os-secundario" onclick={() => salvar(false)} disabled={salvando || conflito}>{m.orqcfg_salvar_continuar()}</button>
          <button type="button" class="os-primary" onclick={() => salvar(true)} disabled={salvando || conflito}>
            {salvando ? m.orqcfg_salvando() : m.orqcfg_salvar_avisar()}
          </button>
        </div>
        <p class="os-rodape">{m.orqcfg_rodape_papel({ arquivo: grupo?.arquivo?.split('/').pop() ?? '' })}</p>
      {/if}
    </div>
  {/if}
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
    {#if andamento}<p class="os-andamento">{m.orqcfg_andamento({ t: andamento.t, total: andamento.total })}</p>{/if}
    {#if papeis.length}
      <!-- Fica no fim da lista, não no formulário: começar é ação do GRUPO, e o formulário edita
           um papel. Sem plano o backend recusa com o motivo, que aparece aqui em cima. -->
      <button type="button" class="os-comecar" onclick={comecar} disabled={comecando}>
        {comecando ? m.orqcfg_comecando() : m.orqcfg_comecar()}
      </button>
    {/if}
    <!-- Uma etapa por papel-base, na ordem em que o trabalho acontece. Faixas ("executor faixa A")
         e rodízio viram linhas dentro da etapa: soltos, o mesmo papel aparecia duas ou três vezes.
         O que a lista mostra é para que serve cada etapa e com que modelo roda; o resto fica no
         formulário do papel. -->
    <ol class="os-etapas">
      {#each etapas as e, n (e.base)}
        {@const fim = finalidade(e.base)}
        {@const ativa = e.linhas.some((l) => casarViva(l.sessao, sessoes)?.state === 'working')}
        <li class="os-etapa" class:ativa>
          <span class="os-num" aria-hidden="true">{n + 1}</span>
          <div class="os-etapa-corpo">
            {#if e.linhas.length > 1}
              <div class="os-etapa-cab">
                <span class="os-oque">{fim ?? e.base}</span>
                <span class="os-quem">{fim ? `${e.base} · ` : ''}{e.linhas.some((l) => faixaDe(l.papel).faixa)
                  ? m.orqcfg_faixas_n({ n: e.linhas.length }) : m.orqcfg_modo_reveza_resumo({ n: e.linhas.length })}</span>
              </div>
              <div class="os-faixas">
                {#each e.linhas as p (chaveLinha(p))}{@render celula(p, null, null)}{/each}
              </div>
            {:else}
              {@render celula(e.linhas[0], fim ?? e.base, fim ? e.base : null)}
            {/if}
          </div>
        </li>
      {/each}
    </ol>
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
      <span class="field-label">{m.comum_provider()}</span>
      <div class="provider-grid" role="group" aria-label={m.criar_provider_aria()}>
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

    <!-- As mesmas opções da folha "Nova sessão": é com elas que o árbitro abre a sessão do papel. -->
    <SessionOpeningFields provider={fProvider} models={modelos} engines={listaMotores} idPrefix="orq-"
      reducedList={!!invConta?.reduced} modelLocked={contaTravada} showJev={temJev}
      bind:headless={fHeadless} bind:model={fModelo} bind:effort={fEsforco} bind:permission={fPermissao}
      bind:engine={fMotor} bind:subagent={fSubagente} bind:jev={fJev} bind:ompProfile={fPerfil}>
      {#snippet afterChoices()}
        <div class="os-grid">
          <div class="field">
            <label class="field-label" for="orq-janela">{m.orqcfg_janela()}</label>
            <Select id="orq-janela" class="field-input" ariaLabel={m.orqcfg_janela()} value={fJanela}
              opcoes={[{ value: '', label: m.orqcfg_janela_padrao({ pct: '50' }) }, ...JANELAS.map((n) => ({ value: n, label: `${n}%` }))]}
              onchange={(v) => (fJanela = v)} />
            <p class="os-hint">{m.orqcfg_janela_ajuda()}</p>
          </div>
          <div class="field">
            <span class="field-label">{m.orqcfg_cota()}</span>
            <p class="field-input os-cota" class:os-cota--alta={(cotaConta?.pct ?? 0) >= 80}>
              {cotaConta ? m.orqcfg_cota_usada({ pct: Math.round(cotaConta.pct), janela: cotaConta.rotulo }) : m.orqcfg_cota_sem()}
            </p>
          </div>
        </div>
      {/snippet}
    </SessionOpeningFields>

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
        {@render barra()}
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
    {@render barra()}
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
  /* relative: o .sr-only de cada item é absoluto, e só o overflow do bloco de contenção o corta
     (mesma regra do .ed-split em EditDiff.svelte). */
  .os-pane { position: relative; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; }
  .os-pane > :global(*) { flex: none; }
  .os-esq { padding-right: var(--space-5); border-right: 1px solid var(--border-subtle); }
  .os-dir { padding-left: var(--space-5); }
  .os-intro, .os-hint, .os-rodape { font-size: var(--text-sm); color: var(--text-secondary); margin: 0 0 var(--space-3); }
  .os-hint { margin: 6px 0 0; font-size: 12px; }
  .os-item { width: 100%; display: block; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); padding: 10px var(--space-3); margin: 6px 0; text-align: left; color: inherit; background: transparent; }
  .os-item.sel { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 10%, transparent); }
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
  .os-nome { font-weight: 600; font-size: 15px; flex: 1; min-width: 0; }
  .os-chip { font-size: 10.5px; padding: 2px 7px; border-radius: 5px; background: var(--surface-raised); color: var(--text-secondary); font-weight: 600; text-transform: uppercase; letter-spacing: .03em; }
  .os-papeis-grid { grid-template-columns: repeat(3, 1fr); }
  .os-papeis-grid .provider-tile { text-transform: capitalize; }
  .os-chip--edit { color: var(--accent); background: color-mix(in srgb, var(--accent) 16%, transparent); font-size: 0.7em; vertical-align: middle; margin-left: 4px; }

  /* ── Etapas: o fio liga os números, porque é uma sequência e não uma lista solta ── */
  .os-andamento { margin: calc(-1 * var(--space-2)) 0 var(--space-3); font-size: 12.5px; color: var(--accent); font-weight: 600; }
  .os-etapas { list-style: none; margin: var(--space-2) 0 0; padding: 0; position: relative; container-type: inline-size; }
  .os-etapas::before { content: ""; position: absolute; left: 12px; top: 16px; bottom: 16px; width: 1px; background: var(--border-default); }
  .os-etapa { position: relative; display: grid; grid-template-columns: 25px minmax(0, 1fr); gap: 10px; padding: 4px 0 10px; }
  .os-num { width: 25px; height: 25px; margin-top: 6px; border-radius: 50%; display: grid; place-items: center; font-size: 11.5px; font-weight: 700;
            background: var(--bg-surface); color: var(--text-secondary); border: 1px solid var(--border-default); z-index: 1; }
  .os-etapa.ativa .os-num { color: var(--accent); border-color: var(--accent); background: var(--accent-dim); }
  .os-etapa-cab { display: flex; flex-direction: column; padding: 6px 0 6px; }
  .os-oque { font-weight: 600; font-size: 14.5px; line-height: 1.3; color: var(--text-primary); }
  .os-quem { font-size: 12px; color: var(--text-muted); line-height: 1.4; }
  .os-seg { white-space: nowrap; }
  .os-cel-abrir > span { max-width: 100%; }
  .os-alta { color: #e3b341; }
  .os-divide { color: #e3b341; border-bottom: 1px dotted currentColor; cursor: help; }

  /* Célula = uma linha do contrato. Etapa de uma linha: título à esquerda, modelo à direita. */
  .os-cel { display: grid; grid-template-columns: minmax(0, 1fr) auto 16px; align-items: center; column-gap: 8px; row-gap: 4px;
            padding: 5px 8px; margin: 0 -8px; border-radius: var(--radius-md); border: 1px solid transparent; }
  .os-cel:hover { background: color-mix(in srgb, var(--text-primary) 3%, transparent); }
  .os-cel.sel { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 8%, transparent); }
  .os-cel-abrir { display: flex; flex-direction: column; align-items: flex-start; min-width: 0; text-align: left; background: none; color: inherit; padding: 0; }
  .os-modelo { display: inline-flex; align-items: center; gap: 6px; height: 26px; min-height: 0; padding: 0 10px; border-radius: 999px; background: var(--surface-raised);
               border: 1px solid var(--border-subtle); color: var(--text-primary); font-size: 12.5px; white-space: nowrap; }
  .os-modelo:hover, .os-modelo[aria-expanded="true"] { border-color: var(--accent); }
  .os-modelo i { font-style: normal; color: var(--text-muted); }
  .os-est { text-align: center; color: var(--text-muted); font-size: 12px; }
  .os-est.viva { color: var(--success); }
  .os-est.trab { color: var(--accent); }
  .os-est.bad { color: var(--error); font-weight: 700; }
  .os-aviso, .os-trocar, .os-rapida { grid-column: 1 / -1; }
  .os-aviso { font-size: 12px; color: color-mix(in srgb, var(--error) 80%, var(--text-primary)); }
  .os-trocar { justify-self: start; font-size: 12px; font-weight: 600; color: var(--accent); background: none; padding: 0; }
  .os-trocar:disabled { opacity: .5; }
  .os-rapida { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: var(--space-2); padding: 4px 0 2px; }

  /* Faixas: caixas lado a lado; o modelo desce para baixo do rótulo. */
  .os-faixas { display: grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: 8px; }
  .os-cel.em-faixa { margin: 0; padding: 8px 10px; border-color: var(--border-subtle); grid-template-columns: minmax(0, 1fr) 16px; }
  .os-cel.em-faixa.sel { border-color: var(--accent); }
  .os-cel.em-faixa .os-oque { font-size: 11px; font-weight: 700; letter-spacing: .05em; text-transform: uppercase; color: var(--accent); }
  .os-cel.em-faixa .os-modelo { grid-column: 1 / -1; grid-row: 2; justify-self: start; }
  .os-cel.em-faixa .os-est { grid-column: 2; grid-row: 1; }
  /* Painel estreito (celular): o chip do modelo desce para baixo do título. */
  @container (max-width: 380px) {
    .os-cel:not(.em-faixa) { grid-template-columns: minmax(0, 1fr) 16px; }
    .os-cel:not(.em-faixa) .os-modelo { grid-column: 1 / -1; grid-row: 2; justify-self: start; }
    .os-cel:not(.em-faixa) .os-est { grid-column: 2; grid-row: 1; }
  }

  /* ── Mudanças pendentes ── */
  .os-barra { position: sticky; bottom: 0; z-index: 2; margin-top: var(--space-3); padding: var(--space-3); border-radius: var(--radius-md);
              border: 1px solid var(--border-default); background: var(--bg-surface); }
  .os-barra-tit { margin: 0 0 4px; font-weight: 600; font-size: var(--text-sm); }
  .os-barra-lista { margin: 0 0 var(--space-3); padding-left: 18px; font-size: 12.5px; color: var(--text-secondary); line-height: 1.5; }
  .os-barra-lista b { color: var(--text-primary); font-weight: 600; }
  .os-barra .os-botoes { align-items: center; }
  .os-barra .os-link { margin-right: auto; }
  .os-h { margin: 0 0 4px; font-size: var(--text-lg); font-weight: 600; }
  .os-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: var(--space-3); }
  .os-cota { margin: 0; color: var(--success); }
  .os-cota--alta { color: #e3b341; }
  .os-agora { margin-top: var(--space-3); padding: 10px 12px; border-radius: var(--radius-md); font-size: var(--text-sm); background: var(--surface-raised); border: 1px solid var(--border-subtle); }
  .os-agora.bad { border-color: color-mix(in srgb, var(--error) 50%, transparent); background: color-mix(in srgb, var(--error) 8%, transparent); }
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
