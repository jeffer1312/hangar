// Registrar os DOIS lados de um peer (Task 8) — lógica pura. O par completo só vira sucesso
// quando os dois gravaram E os dois testes passaram; um lado falhar nomeia QUAL e não deixa o
// outro registrado em silêncio.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { Server } from './auth';

const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
(globalThis as any).document = { cookie: '' };
(globalThis as any).window = { location: { origin: 'https://app.test' } };

vi.mock('./auth', () => ({
  getBaseUrl: vi.fn(() => 'http://casa:8765'),
  getToken: vi.fn(() => 'tcasa'),
}));

vi.mock('./peers', () => ({
  gravarPeer: vi.fn(),
  checkPeer: vi.fn(),
  getIdentificador: vi.fn(async () => ({ identificador: 'srv-casa' })),
}));

const { registrarPeerDoisLados } = await import('./registrarPeerDoisLados');
const peers = await import('./peers');
const peersMock = vi.mocked(peers);

const DONO: Server = { id: 'srv-casa', label: 'casa', baseUrl: 'http://casa:8765', token: 'tcasa' } as Server;
const NOTEBOOK = { id: 'notebook', base_url: 'http://notebook:8765', token: 'tnot' };
// O peer (B) — o servidor EXPLÍCITO que a volta usa para falar com B e B com A.
const REMOTO_B = expect.objectContaining({ baseUrl: 'http://notebook:8765', token: 'tnot' });
// O DONO como B deve guardar: o id real do backend do dono + a credencial dele.
const DONO_NO_PEER = { id: 'srv-casa', base_url: 'http://casa:8765', token: 'tcasa' };

beforeEach(() => { vi.clearAllMocks(); store.clear(); });

describe('registrarPeerDoisLados — os dois lados, sucesso e falha nomeada', () => {
  it('grava o alvo no dono E o dono no peer; os dois testes passam → dois selos ok', async () => {
    peersMock.gravarPeer.mockResolvedValueOnce([NOTEBOOK]);   // grava no dono
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });    // ida (dono pergunta sobre o alvo)
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });    // volta (peer pergunta sobre o dono)
    const r = await registrarPeerDoisLados(DONO, NOTEBOOK);
    expect(r.ok).toBe(true);
    expect(r.lados).toEqual([
      { lado: 'ida', estado: 'ok' },
      { lado: 'volta', estado: 'ok' },
    ]);
    // A em B: grava o ALVO no dono, e o DONO no peer (A em B, B em A).
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(DONO, NOTEBOOK);
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(REMOTO_B, DONO_NO_PEER);
    // A ida pergunta ao dono pelo alvo; a volta pergunta ao PEER pelo DONO (não por ele mesmo).
    expect(peersMock.checkPeer).toHaveBeenCalledWith(DONO, 'http://notebook:8765', 'notebook');
    expect(peersMock.checkPeer).toHaveBeenCalledWith(REMOTO_B, 'http://casa:8765', 'srv-casa');
    // o identificador do dono veio do backend (CP_SERVER_ID), não do rótulo local
    expect(peersMock.getIdentificador).toHaveBeenCalledWith(DONO);
  });

  it('um lado falha → ok=false, nomeia o lado e o estado; o outro registrado NÃO vira silêncio', async () => {
    peersMock.gravarPeer.mockResolvedValueOnce([NOTEBOOK]);   // grava no dono
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });    // ida OK
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'recusou', motivo: 'credencial' });  // volta recusou
    const r = await registrarPeerDoisLados(DONO, NOTEBOOK);
    expect(r.ok).toBe(false);
    expect(r.lados).toContainEqual({ lado: 'ida', estado: 'ok' });
    expect(r.lados).toContainEqual({ lado: 'volta', estado: 'recusou', motivo: 'credencial' });
    // o lado que falhou não pode deixar o outro registrado em silêncio: ok=false
    expect(r.ok).toBe(false);
  });

  it('gravação do lado do dono falha → nem tenta o resto, ok=false com o lado falhou', async () => {
    peersMock.gravarPeer.mockRejectedValueOnce(new Error('Failed to fetch'));
    const r = await registrarPeerDoisLados(DONO, NOTEBOOK);
    expect(r.ok).toBe(false);
    expect(r.lados).toEqual([{ lado: 'ida', estado: 'falhou', motivo: 'Failed to fetch' }]);
    expect(peersMock.gravarPeer).toHaveBeenCalledTimes(1);   // nada mais rodou
    expect(peersMock.checkPeer).not.toHaveBeenCalled();
  });

  it('sem identificador do dono, a volta fica nao_configurado e NADA é gravado no peer', async () => {
    peersMock.getIdentificador.mockResolvedValueOnce({ identificador: '' });
    peersMock.gravarPeer.mockResolvedValueOnce([NOTEBOOK]);   // só a gravação no dono
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });    // ida
    const r = await registrarPeerDoisLados(DONO, NOTEBOOK);
    expect(r.ok).toBe(false);   // sem os dois lados não é sucesso
    expect(r.lados).toEqual([
      { lado: 'ida', estado: 'ok' },
      { lado: 'volta', estado: 'nao_configurado', motivo: 'identificador' },
    ]);
    expect(peersMock.gravarPeer).toHaveBeenCalledTimes(1);   // nada no peer (id vazio daria 400)
    expect(peersMock.checkPeer).toHaveBeenCalledTimes(1);    // só a ida rodou
  });

  it('dono null (modo global) grava no servidor ATIVO e a volta usa a credencial do ativo', async () => {
    peersMock.gravarPeer.mockImplementation(async () => [NOTEBOOK] as never);
    peersMock.checkPeer.mockImplementation(async () => ({ estado: 'ok' }) as never);
    const r = await registrarPeerDoisLados(null, NOTEBOOK);
    expect(r.ok).toBe(true);
    expect(r.lados).toEqual([
      { lado: 'ida', estado: 'ok' },
      { lado: 'volta', estado: 'ok' },
    ]);
    // modo global: grava no ativo (cliente usa getToken/getBaseUrl com alvo null)
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(null, NOTEBOOK);
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(REMOTO_B, DONO_NO_PEER);
    expect(peersMock.checkPeer).toHaveBeenCalledWith(null, 'http://notebook:8765', 'notebook');
    expect(peersMock.checkPeer).toHaveBeenCalledWith(REMOTO_B, 'http://casa:8765', 'srv-casa');
  });
});