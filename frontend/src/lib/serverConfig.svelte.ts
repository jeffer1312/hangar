// DONO do estado das Configuracoes do servidor. Mora aqui, e nao dentro do ServerSettings, porque o
// componente e destruido em dois caminhos normais: ir pra Aparencia/Motores (troca o ramo do {#if}) e
// atravessar os 820px (o corpo do modal remonta). Com o estado no componente, o rascunho sumiria nos
// dois — e rascunho unico foi a decisao explicita do usuario.
import {
  getConfig, getConfigForServer, patchConfig, patchConfigForServer, type CampoConfig,
} from './api';
import type { Server } from './auth';

export type ValorCampo = string | number | boolean;

export function criarConfigServidor(alvo: () => Server | null) {
  // Geração de load/save: trocar de alvo ou tela no meio invalida a resposta pendente (ver
  // carregar/salvar) — só o dono atual pinta campos/rascunho/salvo/erro/salvando.
  let geracao = 0;
  let timerSalvo: ReturnType<typeof setTimeout> | null = null;
  let campos = $state<Record<string, CampoConfig>>({});
  let leitura = $state<Record<string, string | number | boolean>>({});
  let rascunho = $state<Record<string, ValorCampo>>({});
  let carregando = $state(false);
  let salvando = $state(false);
  let erro = $state('');
  let salvo = $state(false);

  async function carregar() {
    const mine = ++geracao;
    carregando = true;
    erro = '';
    try {
      const s = alvo();
      const c = s ? await getConfigForServer(s) : await getConfig();
      if (mine !== geracao) return;   // alvo mudou no meio: resposta A não pinta o alvo B
      campos = c.campos;
      leitura = c.somente_leitura;
      rascunho = {};
    } catch (e) {
      if (mine !== geracao) return;
      erro = e instanceof Error ? e.message : 'Falha ao carregar';
    } finally {
      if (mine === geracao) carregando = false;
    }
  }

  async function salvar() {
    if (!Object.keys(rascunho).length) return;
    const mine = ++geracao;               // invalida load E save anteriores: só o dono atual pinta
    const s = alvo();                     // snapshot do alvo
    const mudancas = { ...rascunho };     // snapshot do rascunho
    salvando = true;
    erro = '';
    salvo = false;
    limparTimerSalvo();
    try {
      // O rascunho vai INTEIRO: o POST /api/config aceita varias chaves num corpo so, e e por isso
      // que um Salvar em qualquer das tres telas grava o que foi mexido nas outras.
      const r = s ? await patchConfigForServer(s, mudancas) : await patchConfig(mudancas);
      if (mine !== geracao) return;       // operacao atual tomou a frente: nao pinta nada
      campos = r.campos;
      rascunho = {};
      salvo = true;
      // Timer do "salvo": o callback só age se esta operacao ainda e a dona — um save posterior
      // limpa o timer anterior, e um timer velho nunca derruba o salvo do novo.
      timerSalvo = setTimeout(() => { if (mine === geracao) salvo = false; }, 2500);
    } catch (e) {
      if (mine !== geracao) return;
      // Erro de validacao do servidor aparece como veio ("upload_retention_days: esperado numero").
      erro = e instanceof Error ? e.message : 'Falha ao salvar';
    } finally {
      if (mine === geracao) salvando = false;
    }
  }

  // Invalida operacao pendente SEM nova chamada (ex: entrar na tela Servidores, que tem controller
  // proprio — a resposta de um load/save antigo nao pinta quando a tela voltar). Zera também os
  // flags: a operacao invalidada nunca mais limpa o que ja morreu aqui.
  function invalidar() {
    geracao++;
    limparTimerSalvo();
    carregando = false;
    salvando = false;
  }
  function limparTimerSalvo() {
    if (timerSalvo) { clearTimeout(timerSalvo); timerSalvo = null; }
  }

  return {
    get campos() { return campos; },
    get leitura() { return leitura; },
    get carregando() { return carregando; },
    get salvando() { return salvando; },
    get erro() { return erro; },
    get salvo() { return salvo; },
    get temMudanca() { return Object.keys(rascunho).length > 0; },
    valorAtual(chave: string): ValorCampo {
      if (chave in rascunho) return rascunho[chave];
      return campos[chave]?.valor ?? '';
    },
    // Segredo NUNCA mostra o valor vindo do servidor (e a mascara, gsk_XXXX...): so o que foi
    // digitado nesta sessao. Editar em cima da mascara manda a mascara de volta como override real.
    rascunhoDe(chave: string): string { return (rascunho[chave] as string) ?? ''; },
    setRascunho(chave: string, valor: ValorCampo) { rascunho[chave] = valor; },
    carregar,
    salvar,
    invalidar,
  };
}

export type ConfigServidorStore = ReturnType<typeof criarConfigServidor>;
