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
  let campos = $state<Record<string, CampoConfig>>({});
  let leitura = $state<Record<string, string | number | boolean>>({});
  let rascunho = $state<Record<string, ValorCampo>>({});
  let carregando = $state(false);
  let salvando = $state(false);
  let erro = $state('');
  let salvo = $state(false);

  async function carregar() {
    carregando = true;
    erro = '';
    try {
      const s = alvo();
      const c = s ? await getConfigForServer(s) : await getConfig();
      campos = c.campos;
      leitura = c.somente_leitura;
      rascunho = {};
    } catch (e) {
      erro = e instanceof Error ? e.message : 'Falha ao carregar';
    } finally {
      carregando = false;
    }
  }

  async function salvar() {
    if (!Object.keys(rascunho).length) return;
    salvando = true;
    erro = '';
    salvo = false;
    try {
      const s = alvo();
      // O rascunho vai INTEIRO: o POST /api/config aceita varias chaves num corpo so, e e por isso
      // que um Salvar em qualquer das tres telas grava o que foi mexido nas outras.
      const r = s ? await patchConfigForServer(s, rascunho) : await patchConfig(rascunho);
      campos = r.campos;
      rascunho = {};
      salvo = true;
      setTimeout(() => (salvo = false), 2500);
    } catch (e) {
      // Erro de validacao do servidor aparece como veio ("upload_retention_days: esperado numero").
      erro = e instanceof Error ? e.message : 'Falha ao salvar';
    } finally {
      salvando = false;
    }
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
  };
}

export type ConfigServidorStore = ReturnType<typeof criarConfigServidor>;
