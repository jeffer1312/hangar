import { describe, it, expect } from 'vitest';
import type { Server } from './auth';

// auth.ts toca localStorage no load (migrate()). vitest env=node nao tem -> stub minimo ANTES do
// import dinamico (top-level await roda apos o stub). migrate() so faz getItem -> null, sai cedo.
const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
// document.cookie: syncCookie() escreve nele. Os testes antigos desviavam do syncCookie de
// proposito; updateServer PRECISA dele, porque re-sincronizar o cookie do servidor ativo (e so
// dele) e exatamente a parte que da errado calada.
let cookieJar = '';
(globalThis as any).document = {
  get cookie() { return cookieJar; },
  set cookie(v: string) { cookieJar = v; },
};
// window.location.origin: updateServer decide o resync de cookie tambem por "same-origin", porque e
// assim que o openSessionsStream (api.ts) autentica o servidor que hospeda o proprio PWA.
(globalThis as any).window = { location: { origin: 'http://casa:8765' } };

const { mergeServers, validarPareamento, onServersChanged, removeServer,
        addServer, updateServer, listServers, selectServer, serverFingerprint, snapshotRemocao, removalStillMatches } = await import('./auth');

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

describe('parseServerPairing removido (round 4 da 4b)', () => {
  it('função não existe mais — a validação tem UMA entrada (validarPareamento)', async () => {
    const auth = await import('./auth');
    expect((auth as unknown as { parseServerPairing?: unknown }).parseServerPairing).toBeUndefined();
  });

  it('validarPareamento é pura: não toca storage nem cookie', () => {
    store.clear();
    cookieJar = '';
    store.set('cp_token', 'guardar-isto');
    validarPareamento('https://pc.ts.net/?token=abc');
    validarPareamento('   ');
    expect(store.get('cp_token')).toBe('guardar-isto');
    expect(cookieJar).toBe('');
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

describe('updateServer', () => {
  // Cada teste parte de uma lista limpa: os mutadores gravam no mesmo localStorage stubado.
  function reset() {
    store.clear();
    cookieJar = '';
    const a = addServer('http://casa:8765', 'tok-casa', 'Casa');
    const b = addServer('http://vps:8766', 'tok-vps', 'VPS');
    return { a: a.id, b: b.id };
  }

  it('troca o token PRESERVANDO id, label e baseUrl', () => {
    // O ponto da feature: remover+re-parear perdia label e posição. Editar tem que manter tudo.
    const { a } = reset();
    expect(updateServer(a, { token: 'tok-novo' })).toBe(true);
    const s = listServers().find((x) => x.id === a)!;
    expect(s.token).toBe('tok-novo');
    expect(s.id).toBe(a);
    expect(s.label).toBe('Casa');
    expect(s.baseUrl).toBe('http://casa:8765');
  });

  it('editar o ATIVO remoto não põe a chave dele no cookie desta origem', () => {
    // O cookie é do servidor DA ORIGEM, não do ativo: o remoto autentica o SSE por ?token= na URL.
    // Antes valia "ativo OU same-origin", e editar o remoto ativo gravava o token de OUTRA máquina
    // como cookie de primeira parte aqui — que agora, com Max-Age de um ano, ficaria guardado.
    const { b } = reset();          // addServer deixa o ÚLTIMO (vps, remoto) como ativo
    cookieJar = '';
    updateServer(b, { token: 'tok-vps-novo' });
    expect(cookieJar).not.toContain('tok-vps-novo');
    expect(cookieJar).toContain('tok-casa');   // segue o da origem, que é quem precisa do cookie
  });

  it('campo vazio mantém o valor atual em vez de apagar', () => {
    // Token em branco desautenticaria o servidor sem avisar — e branco é o que sobra quando o
    // usuário abre o campo e clica fora.
    const { a } = reset();
    updateServer(a, { token: '   ' });
    expect(listServers().find((x) => x.id === a)!.token).toBe('tok-casa');
  });

  it('troca baseUrl junto quando vem de URL de pareamento', () => {
    const { a } = reset();
    updateServer(a, { token: 'tok-novo', baseUrl: 'http://casa-nova:8765' });
    const s = listServers().find((x) => x.id === a)!;
    expect(s.baseUrl).toBe('http://casa-nova:8765');
    expect(s.label).toBe('Casa');      // label custom sobrevive à troca de URL
  });

  it('id inexistente devolve false e não cria entrada', () => {
    reset();
    const antes = listServers().length;
    expect(updateServer('fantasma', { token: 'x' })).toBe(false);
    expect(listServers()).toHaveLength(antes);
  });

  // Round 4: remoção com confirmação velha NUNCA remove entidade nova — fingerprint (id+label+base+
  // token) + revision (versão da lista no momento do clique) precisam casar com o snapshot do diálogo.
  describe('remoção com fingerprint + revision (round 4)', () => {
    const S1: Server = { id: 's1', label: 'Casa', baseUrl: 'http://casa:8765', token: 't1' };
    const S2: Server = { id: 's2', label: 'VPS', baseUrl: 'http://vps:8766', token: 't2' };

    it('serverFingerprint é JSON estável de id+label+baseUrl+token', () => {
      expect(serverFingerprint(S1)).toBe(JSON.stringify(['s1', 'Casa', 'http://casa:8765', 't1']));
      expect(serverFingerprint(S1)).not.toBe(serverFingerprint({ ...S1, token: 't1-novo' }));
    });

    it('snapshotRemocao captura id+fingerprint+revision; null quando não existe', () => {
      expect(snapshotRemocao(S1, 3)).toEqual({ id: 's1', fingerprint: serverFingerprint(S1), revision: 3 });
      expect(snapshotRemocao(undefined, 3)).toBeNull();
    });

    it('inalterado (mesma fingerprint+revision) pode remover', () => {
      expect(removalStillMatches(snapshotRemocao(S1, 1)!, [S1, S2], 1)).toBeNull();
    });

    it('ausente → motivo "removido"', () => {
      expect(removalStillMatches(snapshotRemocao(S1, 1)!, [S2], 1)).toMatch(/removido/);
    });

    it('revision mudou → motivo "mudou", mesmo com fingerprint igual', () => {
      expect(removalStillMatches(snapshotRemocao(S1, 1)!, [S1], 2)).toMatch(/mudou/);
    });

    it('fingerprint mudou (token/label/base isoladamente) → motivo "mudou"', () => {
      expect(removalStillMatches(snapshotRemocao(S1, 1)!, [{ ...S1, token: 't1-novo' }], 1)).toMatch(/mudou/);
      expect(removalStillMatches(snapshotRemocao(S1, 1)!, [{ ...S1, label: 'Outra' }], 1)).toMatch(/mudou/);
      expect(removalStillMatches(snapshotRemocao(S1, 1)!, [{ ...S1, baseUrl: 'http://nova:9999' }], 1)).toMatch(/mudou/);
    });

    it('reintroduzida com a MESMA entidade (fingerprint+revision iguais) pode remover', () => {
      // sync removeu e re-adicionou a mesma entidade antes do clique; a lista relida é idêntica
      // ao snapshot → a remoção continua válida (não é a entidade que mudou).
      expect(removalStillMatches(snapshotRemocao(S1, 1)!, [S1], 1)).toBeNull();
    });
  });

  it('re-sincroniza o cookie do servidor SAME-ORIGIN mesmo quando ele NAO e o ativo', () => {
    // openSessionsStream autentica o same-origin PELO COOKIE (withCredentials) e o cross-origin por
    // ?token= na URL. So "e o ativo" deixava um buraco: com o servidor que hospeda o PWA fora do
    // ativo, trocar o token dele gravava no storage e o reconnect reabria o SSE com o cookie VELHO.
    store.clear();
    cookieJar = '';
    const casa = addServer('http://casa:8765', 'tok-casa', 'Casa');   // = window.location.origin
    const vps = addServer('http://vps:8766', 'tok-vps', 'VPS');       // addServer deixa este ATIVO
    expect(vps.id).toBeTruthy();

    cookieJar = '';
    updateServer(casa.id, { token: 'tok-casa-novo' });
    expect(cookieJar).toContain('tok-casa-novo');
  });

  it('NAO mexe no cookie de servidor cross-origin que nao e o ativo (esse vai por ?token=)', () => {
    store.clear();
    cookieJar = '';
    const vps = addServer('http://vps:8766', 'tok-vps', 'VPS');
    addServer('http://casa:8765', 'tok-casa', 'Casa');                // este vira o ativo

    cookieJar = '';
    updateServer(vps.id, { token: 'tok-vps-novo' });
    // O cookie é reescrito (idempotente) com o token DA ORIGEM; o do remoto nunca entra aqui.
    expect(cookieJar).not.toContain('tok-vps-novo');
    expect(cookieJar).toContain('tok-casa');
  });
});
