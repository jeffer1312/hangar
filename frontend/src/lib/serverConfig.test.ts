// Geração do load no criarConfigServidor (round 1 da 4b): resposta A pendente nunca pinta o alvo B
// ao trocar de alvo no meio; loading/erro também só do dono atual.
import { describe, it, expect, vi } from 'vitest';
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
