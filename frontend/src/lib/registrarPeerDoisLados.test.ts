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

vi.mock('./peers', () => ({
  gravarPeer: vi.fn(),
  listarPeers: vi.fn(),
  checkPeer: vi.fn(),
}));

const { registrarPeerDoisLados } = await import('./registrarPeerDoisLados');
const peers = await import('./peers');
const peersMock = vi.mocked(peers);

const DONO: Server = { id: 'srv-casa', label: 'casa', baseUrl: 'http://casa:8765', token: 'tcasa' } as Server;
const NOTEBOOK = { id: 'notebook', base_url: 'http://notebook:8765', token: 'tnot' };

beforeEach(() => { vi.clearAllMocks(); store.clear(); });

describe('registrarPeerDoisLados — os dois lados, sucesso e falha nomeada', () => {
  it('os dois gravam e os dois testes passam → sucesso silencioso, dois selos ok', async () => {
    peersMock.gravarPeer.mockResolvedValueOnce([
      { id: 'notebook', base_url: 'http://notebook:8765', token: '•••' },
    ]);   // grava no dono
    peersMock.gravarPeer.mockResolvedValueOnce([
      { id: 'notebook', base_url: 'http://notebook:8765', token: 'tnot' },
    ]);   // grava no peer (volta)
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });    // ida
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });    // volta
    const r = await registrarPeerDoisLados(DONO, NOTEBOOK);
    expect(r.ok).toBe(true);
    expect(r.lados).toEqual([
      { lado: 'ida', estado: 'ok' },
      { lado: 'volta', estado: 'ok' },
    ]);
    // gravação no DONO (o alvo da aba) e no PEER (A em B, B em A)
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(DONO, NOTEBOOK);
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(
      expect.objectContaining({ baseUrl: 'http://notebook:8765', token: 'tnot' }),
      NOTEBOOK,
    );
  });

  it('um lado falha → ok=false, nomeia o lado e o estado; o outro registrado NÃO vira silêncio', async () => {
    peersMock.gravarPeer.mockResolvedValueOnce([NOTEBOOK]);
    peersMock.gravarPeer.mockResolvedValueOnce([NOTEBOOK]);   // volta grava
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

  it('sem credencial salva do peer, o lado volta fica nao_configurado (não é defeito)', async () => {
    peersMock.gravarPeer.mockResolvedValueOnce([]);   // dono não guardou credencial do peer
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });    // ida
    const r = await registrarPeerDoisLados(DONO, NOTEBOOK);
    expect(r.ok).toBe(false);   // sem os dois lados não é sucesso
    expect(r.lados).toEqual([
      { lado: 'ida', estado: 'ok' },
      { lado: 'volta', estado: 'nao_configurado' },
    ]);
    expect(peersMock.gravarPeer).toHaveBeenCalledTimes(1);
  });

  it('dono null (modo global) grava no servidor ATIVO e testa a ida', async () => {
    peersMock.gravarPeer.mockImplementation(async () => [NOTEBOOK] as never);
    peersMock.checkPeer.mockImplementation(async () => ({ estado: 'ok' }) as never);
    const r = await registrarPeerDoisLados(null, NOTEBOOK);
    expect(r.ok).toBe(true);   // grava no ativo e o lado volta é o próprio peer
    expect(r.lados).toEqual([
      { lado: 'ida', estado: 'ok' },
      { lado: 'volta', estado: 'ok' },
    ]);
    // modo global: grava no ativo (cliente usa getToken/getBaseUrl com alvo null)
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(null, NOTEBOOK);
    expect(peersMock.checkPeer).toHaveBeenCalledWith(null, 'http://notebook:8765', 'notebook');
  });
});
