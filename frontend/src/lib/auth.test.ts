import { describe, it, expect, vi } from 'vitest';
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
        addServer, updateServer, renameServer, listServers, selectServer, getActiveId, serverFingerprint, snapshotRemocao, removalStillMatches,
        addServerWithRollback } = await import('./auth');

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

// Round 4: add TRANSACIONAL — servidor EXISTENTE (mesma baseUrl) com token novo + probe rejeitado
// volta ao estado anterior (lista e ativo EXATOS); novo rejeitado não permanece; sucesso persiste.
describe('addServerWithRollback', () => {
  function reset() {
    store.clear();
    cookieJar = '';
    addServer('http://casa:8765', 'tok-casa', 'Casa');
    addServer('http://vps:8766', 'tok-vps', 'VPS');   // addServer deixa o ÚLTIMO como ativo
    return { casa: listServers()[0], vps: listServers()[1] };
  }

  it('probe rejeitado em servidor EXISTENTE restaura a entrada (token novo não permanece)', async () => {
    const { casa, vps } = reset();
    const probe = vi.fn(async () => { throw new Error('servidor fora do ar'); });
    await expect(addServerWithRollback('http://casa:8765', 'tok-novo', probe))
      .rejects.toThrow('servidor fora do ar');
    // entrada tocada volta EXATA; as outras intactas; ativo de volta
    expect(listServers()).toHaveLength(2);
    expect(listServers().find((s) => s.baseUrl === 'http://casa:8765')).toEqual(casa);
    expect(listServers().find((s) => s.baseUrl === 'http://vps:8766')).toEqual(vps);
    expect(getActiveId()).toBe(vps.id);
  });

  it('servidor NOVO rejeitado não permanece na lista nem troca o ativo', async () => {
    const { casa, vps } = reset();
    const probe = vi.fn(async () => { throw new Error('falhou'); });
    await expect(addServerWithRollback('http://novo:9999', 'tok-novo', probe))
      .rejects.toThrow('falhou');
    expect(listServers()).toHaveLength(2);
    expect(listServers().find((s) => s.baseUrl === 'http://novo:9999')).toBeUndefined();
    expect(listServers().find((s) => s.baseUrl === 'http://casa:8765')).toEqual(casa);
    expect(getActiveId()).toBe(vps.id);
  });

  it('sucesso persiste o novo estado e o ativo correto', async () => {
    reset();
    const probe = vi.fn(async () => []);
    const r = await addServerWithRollback('http://novo:9999', 'tok-novo', probe);
    expect(r.succeeded).toBe(true);
    const s = listServers().find((x) => x.id === r.id)!;
    expect(s.baseUrl).toBe('http://novo:9999');
    expect(s.token).toBe('tok-novo');
    expect(getActiveId()).toBe(r.id);
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it('entrada que não monta URL válida NÃO chama addServer (defesa em profundidade)', async () => {
    reset();
    const probe = vi.fn(async () => []);
    const antes = listServers().length;
    const r = await addServerWithRollback('ftp://torta', 'abc', probe);
    expect(r.succeeded).toBe(false);
    expect(probe).not.toHaveBeenCalled();
    expect(listServers()).toHaveLength(antes);
  });

  // Round 5: transações de add são FIFO — duas chamadas concorrentes viram sequenciais, e o
  // rollback da primeira NUNCA apaga o sucesso da segunda.
  it('serializa FIFO: a segunda não começa antes da primeira concluir; rollback não apaga sucesso seguinte', async () => {
    reset();
    let rejectBad!: (e: Error) => void;
    const bad = new Promise<void>((_res, rej) => { rejectBad = rej; });
    let resolveGood!: () => void;
    const good = new Promise<void>((res) => { resolveGood = res; });
    const probeBad = vi.fn(() => bad);
    const probeGood = vi.fn(() => good);
    const pBad = addServerWithRollback('http://bad:1', 'tok-bad', probeBad);
    const pGood = addServerWithRollback('http://good:2', 'tok-good', probeGood);
    await Promise.resolve(); await Promise.resolve();
    // primeira rodou; a segunda ainda NÃO (está na fila)
    expect(probeBad).toHaveBeenCalledTimes(1);
    expect(probeGood).not.toHaveBeenCalled();
    expect(listServers().find((s) => s.baseUrl === 'http://good:2')).toBeUndefined();
    // primeira rejeita: rollback + erro pro primeiro caller
    rejectBad(new Error('bad falhou'));
    await expect(pBad).rejects.toThrow('bad falhou');
    await Promise.resolve(); await Promise.resolve();
    // fila liberada: segunda começa e persiste
    expect(probeGood).toHaveBeenCalledTimes(1);
    resolveGood();
    await pGood;
    expect(listServers().find((s) => s.baseUrl === 'http://good:2')!.token).toBe('tok-good');
    // o rollback da primeira NÃO apagou o sucesso da segunda
    expect(listServers().find((s) => s.baseUrl === 'http://good:2')).toBeDefined();
    expect(listServers().find((s) => s.baseUrl === 'http://bad:1')).toBeUndefined();
  });

  it('após rejeição a fila segue livre: a próxima transação roda normalmente', async () => {
    reset();
    const probeFail = vi.fn(async () => { throw new Error('x'); });
    await expect(addServerWithRollback('http://bad:1', 't', probeFail)).rejects.toThrow('x');
    const probeOk = vi.fn(async () => []);
    const r = await addServerWithRollback('http://ok:2', 't', probeOk);
    expect(r.succeeded).toBe(true);
    expect(probeOk).toHaveBeenCalledTimes(1);
    expect(listServers().find((s) => s.baseUrl === 'http://ok:2')).toBeDefined();
  });

  // Round 7: rollback SCOPED à entrada que o add tocou. O snapshot antigo regravava a lista
  // INTEIRA na falha do probe e revertia calado mutações concorrentes em OUTRAS entradas —
  // remoção ou troca de token feita durante o probe pendente (outra view, sync do hub) voltava
  // como estava, sem mensagem.
  it('rollback não ressuscita servidor removido concorrentemente durante o probe', async () => {
    const { casa, vps } = reset();
    let rejectProbe!: (e: Error) => void;
    const pendente = new Promise<void>((_res, rej) => { rejectProbe = rej; });
    const p = addServerWithRollback('http://novo:9999', 'tok-novo', () => pendente);
    await Promise.resolve(); await Promise.resolve();
    // add rodou (novo na lista) e o probe está pendente; noutra view removem o vps
    expect(listServers().find((s) => s.baseUrl === 'http://novo:9999')).toBeDefined();
    removeServer(vps.id);
    rejectProbe(new Error('falhou'));
    await expect(p).rejects.toThrow('falhou');
    // vps CONTINUA removido — o rollback não regravou o snapshot antigo nem aponta o ativo pra ele
    expect(listServers().find((s) => s.id === vps.id)).toBeUndefined();
    expect(getActiveId()).not.toBe(vps.id);
    expect(getActiveId()).toBe(casa.id);
    // a CHAVE crua não pode ficar apontando pro id que o add criou (getActiveId mascara chave
    // stale caindo pro list[0] — a asserção de cima sozinha não pegaria a chave vazada)
    expect((globalThis as any).localStorage.getItem('cp_active')).toBeNull();
    // entrada nova do add não permanece
    expect(listServers().find((s) => s.baseUrl === 'http://novo:9999')).toBeUndefined();
  });

  it('rename concorrente na própria entrada NÃO impede a reversão do token', async () => {
    const { casa } = reset();
    let rejectProbe!: (e: Error) => void;
    const pendente = new Promise<void>((_res, rej) => { rejectProbe = rej; });
    // update em casa (mesma baseUrl já cadastrada) com probe pendente
    const p = addServerWithRollback('http://casa:8765', 'tok-casa-novo', () => pendente);
    await Promise.resolve(); await Promise.resolve();
    // durante o probe, OUTRA view renomeia a MESMA entrada que o add tocou
    renameServer(casa.id, 'Casa nova');
    rejectProbe(new Error('falhou'));
    await expect(p).rejects.toThrow('falhou');
    // o token que falhou NUNCA fica gravado; o label concorrente é preservado
    expect(listServers().find((s) => s.id === casa.id)!.token).toBe('tok-casa');
    expect(listServers().find((s) => s.id === casa.id)!.label).toBe('Casa nova');
  });

  it('rollback NÃO sobrescreve rotação de token na PRÓPRIA entrada durante o probe (round 2)', async () => {
    const { casa } = reset();
    let rejectProbe!: (e: Error) => void;
    const pendente = new Promise<void>((_res, rej) => { rejectProbe = rej; });
    // update em casa (mesma baseUrl já cadastrada) com probe pendente
    const p = addServerWithRollback('http://casa:8765', 'tok-casa-novo', () => pendente);
    await Promise.resolve(); await Promise.resolve();
    // durante o probe, OUTRA view roda a rotação de token DA MESMA entrada que o add tocou
    updateServer(casa.id, { token: 'tok-casa-rotacionado' });
    rejectProbe(new Error('falhou'));
    await expect(p).rejects.toThrow('falhou');
    // a rotação concorrente VENCE: o rollback não regrava o token pré-add por cima dela
    expect(listServers().find((s) => s.id === casa.id)!.token).toBe('tok-casa-rotacionado');
    expect(listServers().find((s) => s.id === casa.id)!.label).toBe('Casa');
  });

  it('rollback não desfaz token trocado concorrentemente em outro servidor', async () => {
    const { casa, vps } = reset();
    let rejectProbe!: (e: Error) => void;
    const pendente = new Promise<void>((_res, rej) => { rejectProbe = rej; });
    // update em casa (mesma baseUrl já cadastrada) com probe pendente
    const p = addServerWithRollback('http://casa:8765', 'tok-casa-novo', () => pendente);
    await Promise.resolve(); await Promise.resolve();
    // noutra view trocam o token do vps durante o probe
    updateServer(vps.id, { token: 'tok-vps-novo' });
    rejectProbe(new Error('falhou'));
    await expect(p).rejects.toThrow('falhou');
    // vps preserva a troca concorrente; casa (entrada tocada pelo add) volta ao token anterior;
    // ativo volta ao anterior (vps) em vez de apontar pro id que o add criou
    expect(listServers().find((s) => s.id === vps.id)!.token).toBe('tok-vps-novo');
    expect(listServers().find((s) => s.baseUrl === 'http://casa:8765')!.token).toBe('tok-casa');
    expect(getActiveId()).toBe(vps.id);
  });
});
