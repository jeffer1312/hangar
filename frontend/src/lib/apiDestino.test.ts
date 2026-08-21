// @vitest-environment happy-dom
// DESTINO das funções que recebem um servidor: `api.ts` de verdade, `fetch` espionado. É o único
// jeito de provar PARA ONDE a chamada vai. A suíte que espiona `api.openShell` prova que o botão
// chama a função — nunca o destino do POST: medido em 18/08/2026, mutar o corpo de `openShell` de
// volta pro caminho global (servidor ATIVO) deixava a suíte inteira verde E o `check` em 0 erros.
// É a mesma classe que deixou a aba Contas agir no servidor errado passar por cinco portões.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import type { Server } from './auth';
import {
  openShell, openNativeTerminal, getConfigForServer, patchConfigForServer,
  getEnginesForServer, getConfig,
  configureApi,
} from '@hangar/core';

const SRV_A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a.local:8765', token: 't-a' };
const SRV_B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b.local:8765', token: 't-b' };

let chamadas: { url: string; token: string | undefined; metodo: string }[] = [];

beforeEach(() => {
  chamadas = [];
  // O ATIVO é o A. Toda função abaixo recebe o B: se alguma sair pelo caminho global, a URL
  // aparece com o host do A e o teste cai.
  localStorage.setItem('cp_servers', JSON.stringify([SRV_A, SRV_B]));
  localStorage.setItem('cp_active', 'srv-a');
  configureApi({
    getBaseUrl: () => SRV_A.baseUrl,
    getToken: () => SRV_A.token,
    onUnauthorized: () => {},
    origin: 'http://a.local:8765',
    createEventSource: () => ({ addEventListener() {}, removeEventListener() {}, close() {}, onerror: null, onopen: null, readyState: 0 }) as unknown as import('@hangar/core').EventSourceLike,
  });
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
    const h = new Headers(init?.headers as HeadersInit);
    chamadas.push({
      url: String(input),
      token: h.get('Authorization') ?? undefined,
      metodo: String(init?.method ?? 'GET'),
    });
    return { ok: true, status: 200, json: async () => ({ ok: true, shell: 'term-s' }) } as Response;
  });
});
afterEach(() => vi.restoreAllMocks());

describe('api — a chamada vai pro servidor DONO, não pro ativo', () => {
  it('openShell fala com o servidor recebido, com a credencial dele', async () => {
    await openShell(SRV_B, 'minha-sessao');
    expect(chamadas).toHaveLength(1);
    expect(chamadas[0].url).toBe('http://b.local:8765/api/sessions/minha-sessao/shell');
    expect(chamadas[0].token).toBe('Bearer t-b');
    expect(chamadas[0].metodo).toBe('POST');
  });

  it('openNativeTerminal idem', async () => {
    await openNativeTerminal(SRV_B, 'minha-sessao');
    expect(chamadas[0].url).toBe('http://b.local:8765/api/sessions/minha-sessao/open-terminal');
    expect(chamadas[0].token).toBe('Bearer t-b');
    expect(chamadas[0].metodo).toBe('POST');
  });

  it('o nome da sessão vai codificado (nome com espaço/barra não quebra a rota)', async () => {
    await openShell(SRV_B, 'a b/c');
    expect(chamadas[0].url).toBe('http://b.local:8765/api/sessions/a%20b%2Fc/shell');
  });

  // Varredura: as outras funções *ForServer do api.ts respondem à mesma pergunta. Sem isto, a
  // próxima que nascer copiando o padrão pode sair pelo ativo e nenhum teste vê.
  it.each([
    ['getConfigForServer', () => getConfigForServer(SRV_B), 'http://b.local:8765/api/config'],
    ['patchConfigForServer', () => patchConfigForServer(SRV_B, { x: 1 }), 'http://b.local:8765/api/config'],
    ['getEnginesForServer', () => getEnginesForServer(SRV_B), 'http://b.local:8765/api/engines'],
  ])('%s fala com o servidor recebido', async (_nome, chamar, esperada) => {
    await chamar();
    expect(chamadas[0].url).toBe(esperada);
    expect(chamadas[0].token).toBe('Bearer t-b');
  });

  // CONTROLE: quem NÃO recebe servidor continua saindo pelo ativo. Sem esta linha, um teste que
  // ficasse verde por acidente (tudo apontando pro B) passaria despercebido.
  it('quem não recebe servidor sai pelo ATIVO', async () => {
    await getConfig();
    expect(chamadas[0].url).toBe('http://a.local:8765/api/config');
    expect(chamadas[0].token).toBe('Bearer t-a');
  });
});
