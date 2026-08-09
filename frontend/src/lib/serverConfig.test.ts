// Geração do load no criarConfigServidor (round 1 da 4b): resposta A pendente nunca pinta o alvo B
// ao trocar de alvo no meio; loading/erro também só do dono atual.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { criarConfigServidor } from './serverConfig.svelte';
import * as api from './api';
import type { Server } from './auth';

vi.mock('./api', () => ({
  getConfig: vi.fn(),
  getConfigForServer: vi.fn(),
  patchConfig: vi.fn(),
  patchConfigForServer: vi.fn(),
}));

const apiMock = vi.mocked(api);
const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' } as Server;
const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'y' } as Server;

function deferrada<T>() {
  let resolve!: (v: T) => void;
  const p = new Promise<T>((res) => { resolve = res; });
  return { p, resolve };
}

function payload(campos: Record<string, unknown>) {
  return { campos: campos as never, somente_leitura: {} as never };
}

beforeEach(() => { vi.useFakeTimers(); vi.clearAllMocks(); });
afterEach(() => { vi.useRealTimers(); });

describe('criarConfigServidor — ownership do salvar (round 2)', () => {
  it('save A tardio não pinta sobre o save B: campos, rascunho, salvo, erro, salvando e timer', async () => {
    let alvo: Server | null = A;
    const store = criarConfigServidor(() => alvo);
    // load A
    const la = deferrada<ReturnType<typeof payload>>();
    apiMock.getConfigForServer.mockReturnValueOnce(la.p as never);
    void store.carregar();
    la.resolve(payload({ chaveA: { valor: 'a' } }));
    await Promise.resolve(); await Promise.resolve();
    store.setRascunho('x', 1);
    const sa = deferrada<ReturnType<typeof payload>>();
    apiMock.patchConfigForServer.mockReturnValueOnce(sa.p as never);
    void store.salvar();                       // save A pendente
    expect(store.salvando).toBe(true);
    // troca pra B: carrega e salva
    alvo = B;
    const lb = deferrada<ReturnType<typeof payload>>();
    apiMock.getConfigForServer.mockReturnValueOnce(lb.p as never);
    void store.carregar();
    lb.resolve(payload({ chaveB: { valor: 'b' } }));
    await Promise.resolve(); await Promise.resolve();
    store.setRascunho('y', 2);
    const sb = deferrada<ReturnType<typeof payload>>();
    apiMock.patchConfigForServer.mockReturnValueOnce(sb.p as never);
    void store.salvar();                       // save B em voo
    sa.resolve(payload({ chaveA: { valor: 'A-TARDIA' } }));   // A resolve POR ÚLTIMO
    await Promise.resolve(); await Promise.resolve();
    // B continua dono: A não pintou NADA
    expect(store.campos).toEqual({ chaveB: { valor: 'b' } });
    expect(store.rascunhoDe('y')).toBe(2);     // rascunho do B intacto
    expect(store.salvo).toBe(false);
    expect(store.erro).toBe('');
    expect(store.salvando).toBe(true);         // B ainda em voo
    sb.resolve(payload({ chaveB: { valor: 'B-NOVO' } }));
    await Promise.resolve(); await Promise.resolve();
    expect(store.campos).toEqual({ chaveB: { valor: 'B-NOVO' } });
    expect(store.salvo).toBe(true);
    expect(store.salvando).toBe(false);
  });

  it('timer do "salvo" só derruba o da operação dona', async () => {
    let alvo: Server | null = A;
    const store = criarConfigServidor(() => alvo);
    apiMock.getConfigForServer.mockResolvedValue(payload({ chaveA: { valor: 'a' } }) as never);
    await store.carregar();
    apiMock.patchConfigForServer.mockResolvedValue(payload({ chaveA: { valor: 'a' } }) as never);
    store.setRascunho('x', 1);
    await store.salvar();                      // save 1: salvo=true, timer 1 agendado
    expect(store.salvo).toBe(true);
    await store.salvar();                      // save 2 limpa o timer 1
    expect(store.salvo).toBe(true);
    // sem o clear do timer 1, ele dispararia ~no mesmo instante e zeraria o salvo do save 2
    vi.advanceTimersByTime(100);
    expect(store.salvo).toBe(true);
    vi.advanceTimersByTime(2400);              // timer do save 2 estoura
    expect(store.salvo).toBe(false);
  });

  it('troca A→B com load que FALHA: estado de A limpo, erro de B, flags coerentes (round 3)', async () => {
    let alvo: Server | null = A;
    const store = criarConfigServidor(() => alvo);
    apiMock.getConfigForServer.mockResolvedValueOnce(payload({ chaveA: { valor: 'a' } }) as never);
    await store.carregar(true);
    store.setRascunho('x', 1);
    const sa = deferrada<ReturnType<typeof payload>>();
    apiMock.patchConfigForServer.mockReturnValueOnce(sa.p as never);
    void store.salvar();                       // save A pendente
    expect(store.salvando).toBe(true);
    // troca A→B: load de B FALHA
    alvo = B;
    apiMock.getConfigForServer.mockRejectedValueOnce(new Error('HTTP 500'));
    void store.carregar(true);
    await Promise.resolve(); await Promise.resolve();
    expect(store.campos).toEqual({});          // nada de A na tela de B
    expect(store.rascunhoDe('x')).toBe('');    // draft de A morreu na troca
    expect(store.erro).toBe('HTTP 500');       // erro do B, não do A
    expect(store.salvando).toBe(false);        // flags do A zerados
    expect(store.salvo).toBe(false);
    // A resolve POR ÚLTIMO: não pinta nada por cima de B
    sa.resolve(payload({ chaveA: { valor: 'A-TARDIA' } }));
    await Promise.resolve(); await Promise.resolve();
    expect(store.campos).toEqual({});
    expect(store.erro).toBe('HTTP 500');
  });

  it('salvar duplo antes da primeira resposta emite UM POST (round 3)', async () => {
    let alvo: Server | null = A;
    const store = criarConfigServidor(() => alvo);
    apiMock.getConfigForServer.mockResolvedValue(payload({ chaveA: { valor: 'a' } }) as never);
    await store.carregar(true);
    store.setRascunho('x', 1);
    const sa = deferrada<ReturnType<typeof payload>>();
    apiMock.patchConfigForServer.mockReturnValueOnce(sa.p as never);
    const p1 = store.salvar();
    const p2 = store.salvar();                 // duplo clique antes da resposta
    expect(apiMock.patchConfigForServer).toHaveBeenCalledTimes(1);
    sa.resolve(payload({ chaveA: { valor: 'a' } }) as never);
    await p1; await p2;
    expect(apiMock.patchConfigForServer).toHaveBeenCalledTimes(1);
  });

  it('invalidar() descarta resposta pendente sem nova chamada e zera flags', async () => {
    let alvo: Server | null = A;
    const store = criarConfigServidor(() => alvo);
    const la = deferrada<ReturnType<typeof payload>>();
    apiMock.getConfigForServer.mockReturnValueOnce(la.p as never);
    void store.carregar();
    expect(store.carregando).toBe(true);
    store.invalidar();                         // ex: entrar na tela Servidores
    expect(store.carregando).toBe(false);
    la.resolve(payload({ chaveA: { valor: 'x' } }));
    await Promise.resolve(); await Promise.resolve();
    expect(store.campos).toEqual({});          // resposta velha não pinta
    expect(store.carregando).toBe(false);
    expect(apiMock.getConfigForServer).toHaveBeenCalledTimes(1);   // sem chamada nova
  });
});

describe('criarConfigServidor — geração do carregar', () => {
  it('resposta A tardia não pinta o alvo B; B pinta o seu', async () => {
    let alvo: Server | null = A;
    const store = criarConfigServidor(() => alvo);
    const a = deferrada<ReturnType<typeof payload>>();
    apiMock.getConfigForServer.mockReturnValueOnce(a.p as never);
    void store.carregar();                    // load A pendente

    alvo = B;
    const b = deferrada<ReturnType<typeof payload>>();
    apiMock.getConfigForServer.mockReturnValueOnce(b.p as never);
    void store.carregar();                    // load B

    a.resolve(payload({ chaveA: { valor: 'de-A' } }));   // A responde DEPOIS de B começar
    await Promise.resolve(); await Promise.resolve();
    expect(store.campos).toEqual({});         // A não pintou
    expect(store.carregando).toBe(true);      // B ainda carrega

    b.resolve(payload({ chaveB: { valor: 'de-B' } }));
    await Promise.resolve(); await Promise.resolve();
    expect(store.campos).toEqual({ chaveB: { valor: 'de-B' } });
    expect(store.carregando).toBe(false);
    expect(apiMock.getConfigForServer).toHaveBeenCalledTimes(2);
  });

  it('erro da resposta A tardia não vira erro do alvo B', async () => {
    let alvo: Server | null = A;
    const store = criarConfigServidor(() => alvo);
    const a = deferrada<ReturnType<typeof payload>>();
    apiMock.getConfigForServer.mockReturnValueOnce(a.p as never);
    void store.carregar();

    alvo = B;
    const b = deferrada<ReturnType<typeof payload>>();
    apiMock.getConfigForServer.mockReturnValueOnce(b.p as never);
    void store.carregar();

    a.resolve(payload({ chaveA: { valor: 'x' } }));
    await Promise.resolve(); await Promise.resolve();
    expect(store.erro).toBe('');              // A não gravou erro nem campos

    b.resolve(payload({ chaveB: { valor: 'y' } }));
    await Promise.resolve(); await Promise.resolve();
    expect(store.campos).toEqual({ chaveB: { valor: 'y' } });
  });
});
