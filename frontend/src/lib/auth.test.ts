import { describe, it, expect } from 'vitest';

// auth.ts toca localStorage no load (migrate()). vitest env=node nao tem -> stub minimo ANTES do
// import dinamico (top-level await roda apos o stub). migrate() so faz getItem -> null, sai cedo.
const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
const { mergeServers, parseServerPairing, onServersChanged, removeServer } = await import('./auth');

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

describe('parseServerPairing', () => {
  it('URL com ?token= -> origin + token', () => {
    expect(parseServerPairing('https://pc.ts.net/?token=abc123')).toEqual({
      base: 'https://pc.ts.net',
      token: 'abc123',
    });
  });

  it('?api= sobrepoe o origin (backend atras de proxy)', () => {
    expect(parseServerPairing('https://app.com/?api=https://backend:8765&token=xyz')).toEqual({
      base: 'https://backend:8765',
      token: 'xyz',
    });
  });

  it('espacos em volta sao ignorados', () => {
    expect(parseServerPairing('  https://pc.ts.net/?token=t9  ')).toEqual({
      base: 'https://pc.ts.net',
      token: 't9',
    });
  });

  it('token cru sem URL -> null (sem origem confiavel)', () => {
    expect(parseServerPairing('abc123')).toBeNull();
  });

  it('vazio -> null', () => {
    expect(parseServerPairing('')).toBeNull();
    expect(parseServerPairing('   ')).toBeNull();
  });
});

// notifyChanged nao e exportado: o gatilho publico mais barato e removeServer (unico mutador que
// notifica sem tocar cookie/crypto — o id fantasma nao casa com o ACTIVE_KEY, entao pula o syncCookie).
describe('onServersChanged / notifyChanged', () => {
  const fire = () => removeServer('ghost');

  it('multi-listener: TODOS os inscritos sao chamados', () => {
    // Era slot unico antes: o 2o consumidor clobberava o 1o calado.
    const calls: string[] = [];
    const un1 = onServersChanged(() => calls.push('a'));
    const un2 = onServersChanged(() => calls.push('b'));
    fire();
    expect(calls).toEqual(['a', 'b']);
    un1(); un2();
  });

  it('unsubscribe: o removido nao e mais chamado, o outro continua', () => {
    const calls: string[] = [];
    const un1 = onServersChanged(() => calls.push('a'));
    const un2 = onServersChanged(() => calls.push('b'));
    un1();
    fire();
    expect(calls).toEqual(['b']);
    un2();
  });

  it('unsubscribe DURANTE o notify nao quebra a iteracao', () => {
    // Por isso o loop itera uma copia do Set: mexer no Set original durante a iteracao pularia o 'b'.
    const calls: string[] = [];
    const un1 = onServersChanged(() => { calls.push('a'); un1(); un2(); });
    const un2 = onServersChanged(() => calls.push('b'));
    fire();
    expect(calls).toEqual(['a', 'b']);
    fire();
    expect(calls).toEqual(['a', 'b']);   // ambos saidos: 2o disparo nao chama ninguem
  });

  it('um listener que LANCA nao impede o seguinte', () => {
    // O do Board faz `new EventSource(url)` (lanca SyntaxError com baseUrl malformado do vault) e
    // matava CALADO o push do vault do App — a ordem e de insercao, entao a vitima dependia de timing.
    const calls: string[] = [];
    const un1 = onServersChanged(() => { throw new Error('EventSource explodiu'); });
    const un2 = onServersChanged(() => calls.push('sobrevivi'));
    expect(() => fire()).not.toThrow();
    expect(calls).toEqual(['sobrevivi']);
    un1(); un2();
  });
});
