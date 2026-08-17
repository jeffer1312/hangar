// Cliente de peers: as chamadas certas (método/caminho/corpo) e o self-heal de 401. O conteúdo
// mascarado em si é contrato do backend (test_peers_api.py) — aqui se prova a borda do cliente.
import { beforeEach, describe, expect, it, vi } from 'vitest';

const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
(globalThis as any).document = { cookie: '' };
(globalThis as any).window = { location: { origin: 'https://app.test' } };

// O servidor ativo resolve getBaseUrl/getToken (lib/auth). URL e token do app, não de peer.
const ativo = { id: 'a', label: 'Servidor A', baseUrl: 'https://a.test', token: 'token-a' };

const { listarPeers, gravarPeer, removerPeer, getIdentificador, setIdentificador } = await import('./peers');
const { listServers, getActiveId } = await import('./auth');

beforeEach(() => {
  vi.restoreAllMocks();
  store.set('cp_servers', JSON.stringify([ativo]));
  store.set('cp_active', ativo.id);
});

describe('cliente de peers', () => {
  it('listar usa o servidor ativo e o Bearer do app', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify([{ id: 'notebook', base_url: 'http://n:8765', token: '••••reto' }]), { status: 200 }),
    );
    const r = await listarPeers();
    expect(r[0].id).toBe('notebook');
    expect(fetchMock).toHaveBeenCalledWith('https://a.test/api/peers', expect.objectContaining({
      headers: expect.objectContaining({ Authorization: 'Bearer token-a' }),
    }));
  });

  it('gravar manda POST com corpo completo e devolve a lista nova', async () => {
    const payload = { id: 'notebook', base_url: 'http://n:8765', token: 'segredo' };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify([payload]), { status: 200 }),
    );
    const r = await gravarPeer(payload);
    expect(r).toHaveLength(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('https://a.test/api/peers');
    expect(init.method).toBe('POST');
    expect(JSON.parse(String(init.body))).toEqual(payload);
  });

  it('remover manda DELETE no caminho codificado', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify([]), { status: 200 }),
    );
    await removerPeer('notebook');
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('https://a.test/api/peers/notebook');
    expect(init.method).toBe('DELETE');
  });

  it('setIdentificador faz PUT e getIdentificador lê', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ identificador: 'casa' }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ identificador: '' }), { status: 200 }));
    const r = await setIdentificador('casa');
    expect(r).toEqual({ identificador: 'casa' });
    expect(await getIdentificador()).toEqual({ identificador: '' });
    const call = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(call[0]).toBe('https://a.test/api/peers/identificador');
    expect(call[1].method).toBe('PUT');
    expect(JSON.parse(String(call[1].body))).toEqual({ identificador: 'casa' });
  });

  it('erro 400 do backend vira Error com a mensagem do detail', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'identificador: use minúsculas…' }), { status: 400 }),
    );
    await expect(setIdentificador('Casa')).rejects.toThrow('identificador: use minúsculas…');
  });

  it('401 com token salvo limpa a credencial e recarrega (self-heal do apiFetch)', async () => {
    const reload = vi.fn();
    (globalThis as any).window.location.reload = reload;
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'token inválido' }), { status: 401 }),
    );
    await expect(listarPeers()).rejects.toThrow();
    expect(reload).toHaveBeenCalledTimes(1);
    expect(listServers()).toEqual([]);
    expect(getActiveId()).toBeNull();
  });
});