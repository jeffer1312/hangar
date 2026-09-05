import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// term.ts importa auth.ts, que toca localStorage/window.location no load via migrate(); vitest
// env=node nao tem nenhum dos dois. Stub minimo ANTES do import dinamico — mesmo padrao de
// sessions.test.ts/auth.test.ts (import estatico rodaria migrate() antes do stub existir).
// `location` global tambem precisa existir a parte de `window.location`: termUrl usa o identificador
// bare `location.origin` (auth.ts usa `window.location.origin`), e node nao aliasa um pro outro.
const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
(globalThis as any).location = { origin: 'http://casa:8765' };
(globalThis as any).window = { location: (globalThis as any).location };

const { TermSocket, termUrlForServer, sessionExistsOnServer, RESIZE_DEBOUNCE_MS } = await import('./term');
// term.ts só importa TIPO de auth.ts, então auth (e as 2400 mensagens do paraglide do front) carregaria
// a frio dentro do primeiro teste e estouraria os 5s. Carrega aqui, na coleta.
await import('./auth');

class FakeWS {
  static ultimo: FakeWS;
  sent: (string | ArrayBufferLike)[] = [];
  binaryType = '';
  onmessage: ((e: MessageEvent) => void) | null = null;
  // Aceita (ou nao) um CloseEvent: o navegador SEMPRE passa um, mas o `close()` daqui embaixo
  // chama sem argumento — e e justamente esse caso que a guarda `e?.reason` do term.ts cobre.
  onclose: ((e?: { reason?: string }) => void) | null = null;
  constructor(public url: string) { FakeWS.ultimo = this; }
  send(d: string | ArrayBufferLike) { this.sent.push(d); }
  close() { this.onclose?.(); }
}

beforeEach(() => { vi.stubGlobal('WebSocket', FakeWS as never); vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

// Dois servidores, A o ATIVO e B fora da listagem ativa: o endereco do terminal de uma sessao de B
// TEM que ser o de B (baseUrl + token), nunca o do ativo. E a regressao que esta Task conserta —
// antes, termUrl() montava com getBaseUrl()/getToken(), e o painel da sessao de B anexava no
// terminal da homonima do servidor A (ou caia nele, em silencio).
describe('termUrlForServer', () => {
  it('cai em location.origin quando o baseUrl do servidor e vazio (front na mesma origem)', async () => {
    // Mesmo caso do PWA servido pela propria origem: o servidor guarda baseUrl vazio e o app e
    // carregado de onde o backend responde — sem o fallback, `new WebSocket('/api/...')` vira
    // SyntaxError. Regressao que morava no termUrl antigo (apagado no Step 7) e que nao pode ir
    // junto com ele.
    const auth = await import('./auth');
    vi.spyOn(auth, 'getBaseUrl').mockReturnValue('');
    vi.spyOn(auth, 'getToken').mockReturnValue('t');
    const mesmaOrigem = { id: 'l', label: 'local', baseUrl: '', token: 't' };
    expect(termUrlForServer(mesmaOrigem, 's', 80, 24)).toMatch(/^wss?:\/\/.+\/api\/sessions\/s\/term\?/);
  });

  it('usa o endereco e a credencial do servidor EXPLICITO, mesmo com outro ativo', async () => {
    const auth = await import('./auth');
    // O ativo e A: se a funcao lesse o servidor ativo em vez do argumento, a URL sairia com o
    // endereco/credencial de A e este teste acusaria.
    vi.spyOn(auth, 'getBaseUrl').mockReturnValue('http://a');
    vi.spyOn(auth, 'getToken').mockReturnValue('ta');
    const srvB = { id: 'b', label: 'servidor B', baseUrl: 'http://b', token: 'tb' };
    const url = termUrlForServer(srvB, 'sess', 80, 24);
    expect(url.startsWith('ws://b/api/sessions/sess/term?')).toBe(true);
    expect(url).toContain('token=tb');
    expect(url).toContain('cols=80');
    expect(url).toContain('rows=24');
    expect(url).not.toContain('ws://a');
    expect(url).not.toContain('token=ta');
  });

  it('sessao homonima nos dois servidores: o endereco e o de B, nao o do ativo', async () => {
    const auth = await import('./auth');
    vi.spyOn(auth, 'getBaseUrl').mockReturnValue('http://a');
    vi.spyOn(auth, 'getToken').mockReturnValue('ta');
    // Mesmo nome nos dois — o caso que mordia: `termUrl` (servidor ativo) montava a URL de A e o
    // painel abria o terminal errado sem nenhum aviso.
    const srvB = { id: 'b', label: 'servidor B', baseUrl: 'http://b', token: 'tb' };
    const url = termUrlForServer(srvB, 'hom', 80, 24);
    expect(url).toContain('ws://b/api/sessions/hom/term?');
    expect(url).toContain('token=tb');
    expect(url).not.toContain('ws://a');
  });
});

describe('sessionExistsOnServer', () => {
  const sess = (name: string) => ({ name, state: 'idle' as const, jsonl: `/j/${name}.jsonl` });
  const srvB = { id: 'b', label: 'servidor B', baseUrl: 'http://b', token: 'tb' };

  it('verdadeiro quando a sessao esta na lista do servidor', async () => {
    const api = await import('@hangar/core');
    vi.spyOn(api, 'fetchSessionsForServer').mockResolvedValue([sess('x'), sess('y')]);
    expect(await sessionExistsOnServer(srvB, 'x')).toBe(true);
  });

  it('falso quando a sessao nao existe naquele servidor — o caso do erro visivel', async () => {
    const api = await import('@hangar/core');
    vi.spyOn(api, 'fetchSessionsForServer').mockResolvedValue([sess('outra')]);
    // A homonima pode existir em OUTRO servidor; o probe pergunta so ao B, e so ele decide.
    expect(await sessionExistsOnServer(srvB, 'x')).toBe(false);
  });

  it('propaga a falha de rede — quem chama decide o que mostrar', async () => {
    const api = await import('@hangar/core');
    vi.spyOn(api, 'fetchSessionsForServer').mockRejectedValue(new Error('servidor fora do ar'));
    await expect(sessionExistsOnServer(srvB, 'x')).rejects.toThrow('servidor fora do ar');
  });
});

describe('TermSocket', () => {
  it('manda dados como quadro BINARIO', () => {
    const s = new TermSocket('ws://x', { data: () => {}, close: () => {} });
    s.send(new Uint8Array([65, 66]));
    expect(FakeWS.ultimo.sent).toHaveLength(1);
    expect(typeof FakeWS.ultimo.sent[0]).not.toBe('string');
  });

  it('manda resize como quadro de TEXTO em json', () => {
    const s = new TermSocket('ws://x', { data: () => {}, close: () => {} });
    s.resize(100, 30);
    vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);
    expect(FakeWS.ultimo.sent).toEqual([JSON.stringify({ t: 'resize', cols: 100, rows: 30 })]);
  });

  it('repassa o motivo do fechamento mandado pelo backend', () => {
    let motivo: string | undefined = 'nao chamou';
    new TermSocket('ws://x', { data: () => {}, close: (m) => { motivo = m; } });
    FakeWS.ultimo.onclose?.({ reason: 'outra conexao assumiu' });
    expect(motivo).toBe('outra conexao assumiu');
  });

  it('fechamento sem motivo chega como undefined (o caso do handshake recusado)', () => {
    let chamou = false;
    let motivo: string | undefined = 'nao chamou';
    const s = new TermSocket('ws://x', { data: () => {}, close: (m) => { chamou = true; motivo = m; } });
    s.close();
    expect(chamou).toBe(true);
    expect(motivo).toBeUndefined();
  });

  it('coalesce resizes seguidos num so, com o ultimo valor', () => {
    const s = new TermSocket('ws://x', { data: () => {}, close: () => {} });
    s.resize(90, 20); s.resize(100, 25); s.resize(110, 30);
    vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);
    expect(FakeWS.ultimo.sent).toEqual([JSON.stringify({ t: 'resize', cols: 110, rows: 30 })]);
  });
});
