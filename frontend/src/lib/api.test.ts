import { beforeEach, describe, expect, it, vi } from 'vitest';

const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
(globalThis as any).document = { cookie: '' };
(globalThis as any).window = { location: { origin: 'https://app.test' } };

const { getConfigForServer, patchConfigForServer } = await import('./api');
const { listServers, getActiveId } = await import('./auth');
const server = { id: 'a', label: 'Servidor A', baseUrl: 'https://a.test', token: 'token-a' };

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('explicit server settings API', () => {
  it('usa base e token explícitos sem depender do servidor ativo', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ campos: {}, somente_leitura: {} }), { status: 200 }),
    );

    await getConfigForServer(server);

    expect(fetchMock).toHaveBeenCalledWith('https://a.test/api/config', expect.objectContaining({
      headers: expect.objectContaining({ Authorization: 'Bearer token-a' }),
    }));
  });

  it('401 explícito vira erro e não recarrega nem remove credencial global', async () => {
    // O cenário que importa: o servidor ATIVO é OUTRA máquina. Sem montá-lo, o teste passaria
    // mesmo que apiFetchForServer chamasse dropActiveServer — não haveria credencial pra derrubar.
    const outra = { id: 'b', label: 'Servidor B', baseUrl: 'https://b.test', token: 'token-b' };
    store.set('cp_servers', JSON.stringify([outra]));
    store.set('cp_active', outra.id);

    const reload = vi.fn();
    (globalThis as any).window.location.reload = reload;
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'token inválido' }), { status: 401 }),
    );

    await expect(patchConfigForServer(server, { automations: false }))
      .rejects.toThrow('401: token inválido');

    expect(reload).not.toHaveBeenCalled();
    expect(listServers()).toEqual([outra]);   // a credencial da outra máquina segue intacta
    expect(getActiveId()).toBe(outra.id);
  });
});
