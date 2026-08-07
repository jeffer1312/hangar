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

const { TermSocket, termUrl, RESIZE_DEBOUNCE_MS } = await import('./term');

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

describe('termUrl', () => {
  it('cai em location.origin quando a base e vazia (front na mesma origem)', async () => {
    const auth = await import('./auth');
    vi.spyOn(auth, 'getBaseUrl').mockReturnValue('');
    vi.spyOn(auth, 'getToken').mockReturnValue('t');
    // Sem o fallback isto viraria `new WebSocket('/api/...')`, que levanta SyntaxError — e e o caso
    // do PWA servido pela VPS.
    expect(termUrl('s', 80, 24)).toMatch(/^wss?:\/\/.+\/api\/sessions\/s\/term\?/);
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
