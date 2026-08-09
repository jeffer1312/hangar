// DONO do estado das Configuracoes do servidor. Mora aqui, e nao dentro do ServerSettings, porque o
// componente e destruido em dois caminhos normais: ir pra Aparencia/Motores (troca o ramo do {#if}) e
// atravessar os 820px (o corpo do modal remonta). Com o estado no componente, o rascunho sumiria nos
// dois — e rascunho unico foi a decisao explicita do usuario.
import {
  getConfig, getConfigForServer, patchConfig, patchConfigForServer, type CampoConfig,
} from './api';
import { serverIdentidade, type Server } from './auth';

export type ValorCampo = string | number | boolean;

// `identidade` (opcional) é a string que identifica o alvo sendo editado. O default deriva do `alvo`,
// mas quem precisa de precisão (SettingsModal, via App) passa a identidade EXPLÍCITA: no modo global
// `alvo` é null e ainda assim o servidor editado é o ATIVO — só o App resolve isso.
export function criarConfigServidor(alvo: () => Server | null, identidade?: () => string) {
  const donoDe = () => (identidade ? identidade() : serverIdentidade(alvo()));

  // Geração de load/save: trocar de alvo ou tela no meio invalida a resposta pendente (ver
  // carregar/salvar) — só o dono atual pinta campos/rascunho/salvo/erro/salvando.
  let geracao = 0;
  let ultimoDono = '';
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
    const dono = donoDe();
    // Troca REAL de identidade (outro servidor, ou o MESMO id com base/token/label mudados): o
    // rascunho do alvo anterior morre. Recarregar o MESMO alvo (voltar de Servidores, por exemplo)
    // PRESERVA o rascunho único — decisão do usuário, e é o que o round 4 corrigiu (antes um reload
    // de alvo igual apagava o draft).
    const trocouDono = dono !== ultimoDono;
    ultimoDono = dono;
    // Estado da operação ANTERIOR (outro alvo) morre aqui, ANTES do await: campos/leitura/erro de
    // A nunca pintam na tela de B (nem quando o load de B falha), e flags de A não travam B. O
    // rascunho morre NA TROCA REAL — mesmo que o load do B falhe, draft de A não sobrevive ao
    // handoff; recarregar o MESMO alvo preserva (trocouDono false).
    if (trocouDono) rascunho = {};
    campos = {};
    leitura = {};
    erro = '';
    salvo = false;
    salvando = false;
    limparTimerSalvo();
    carregando = true;
    try {
      const s = alvo();
      const c = s ? await getConfigForServer(s) : await getConfig();
      if (mine !== geracao || donoDe() !== dono) return;   // alvo mudou no meio: resposta A não pinta B
      campos = c.campos;
      leitura = c.somente_leitura;
    } catch (e) {
      if (mine !== geracao || donoDe() !== dono) return;
      erro = e instanceof Error ? e.message : 'Falha ao carregar';
    } finally {
      if (mine === geracao) carregando = false;
    }
  }

  async function salvar() {
    if (salvando) return;                 // duplo clique antes da primeira resposta: UM POST
    if (!Object.keys(rascunho).length) return;
    const mine = ++geracao;               // invalida load E save anteriores: só o dono atual pinta
    const dono = donoDe();
    ultimoDono = dono;                    // assume ownership do alvo corrente
    const s = alvo();                     // caminho da API (global vs ForServer)
    const enviado = { ...rascunho };      // snapshot do rascunho no instante do POST
    salvando = true;
    erro = '';
    salvo = false;
    carregando = false;                   // um load pendente não pode mais pintar por cima deste save
    limparTimerSalvo();
    try {
      // O rascunho vai INTEIRO: o POST /api/config aceita varias chaves num corpo so, e e por isso
      // que um Salvar em qualquer das tres telas grava o que foi mexido nas outras.
      const r = s ? await patchConfigForServer(s, enviado) : await patchConfig(enviado);
      if (mine !== geracao || donoDe() !== dono) return;   // operacao atual tomou a frente: nao pinta nada
      campos = r.campos;
      // Apaga SÓ as chaves cujo valor ATUAL ainda é o enviado. Edição feita DURANTE o POST (mesma
      // chave, valor novo) continua no rascunho e vai no PRÓXIMO Save — a resposta não a clobbera.
      for (const k of Object.keys(enviado)) {
        if (Object.is(rascunho[k], enviado[k])) delete rascunho[k];
      }
      salvo = true;
      // Timer do "salvo": o callback só age se esta operacao ainda e a dona — um save posterior
      // limpa o timer anterior, e um timer velho nunca derruba o salvo do novo.
      timerSalvo = setTimeout(() => { if (mine === geracao) salvo = false; }, 2500);
    } catch (e) {
      if (mine !== geracao || donoDe() !== dono) return;
      // Erro de validacao do servidor aparece como veio ("upload_retention_days: esperado numero").
      // O rascunho fica INTACTO: reject/timeout deixa salvando=false, erro visível e retry possível.
      erro = e instanceof Error ? e.message : 'Falha ao salvar';
    } finally {
      if (mine === geracao) salvando = false;
    }
  }

  // Invalida operacao pendente SEM nova chamada (ex: entrar na tela Servidores, que tem controller
  // proprio — a resposta de um load/save antigo nao pinta quando a tela voltar). Zera também os
  // flags: a operacao invalidada nunca mais limpa o que ja morreu aqui. NÃO mexe no ultimoDono: quem
  // volta pro MESMO alvo depois preserva o rascunho.
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
