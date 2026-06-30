import { describe, it, expect } from 'vitest';

// auth.ts toca localStorage no load (migrate()). vitest env=node nao tem -> stub minimo ANTES do
// import dinamico (top-level await roda apos o stub). migrate() so faz getItem -> null, sai cedo.
const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
const { mergeServers } = await import('./auth');

const S = (id: string, baseUrl: string, token = 't') => ({ id, label: id, baseUrl, token });

describe('mergeServers', () => {
  it('vault vazio -> sobe a lista local inteira', () => {
    const local = [S('a', 'http://casa:8765'), S('b', 'http://vps:8765')];
    expect(mergeServers([], local)).toEqual(local);
  });

  it('acrescenta locais que o hub ainda nao tem', () => {
    const remote = [S('a', 'http://casa:8765')];
    const local = [S('a2', 'http://casa:8765'), S('b', 'http://vps:8765')];
    const out = mergeServers(remote, local);
    expect(out.map((s) => s.baseUrl)).toEqual(['http://casa:8765', 'http://vps:8765']);
  });

  it('remote tem precedencia em duplicata (mesma baseUrl normalizada, barra final ignorada)', () => {
    const remote = [S('R', 'http://casa:8765')];
    const local = [S('L', 'http://casa:8765/')];
    const out = mergeServers(remote, local);
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe('R');
  });

  it('mantem servers do hub que o navegador nao tem', () => {
    const remote = [S('a', 'http://casa:8765'), S('b', 'http://vps:8765')];
    expect(mergeServers(remote, [])).toEqual(remote);
  });
});
