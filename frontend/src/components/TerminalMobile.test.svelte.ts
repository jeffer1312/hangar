// @vitest-environment happy-dom
// O terminal do celular manda BYTES CRUS pro PTY: quem digita e o proprio xterm, e a barra de baixo
// so tem o que teclado de celular nao tem. Estes testes travam o que a tela promete: a tecla vira a
// sequencia que uma tecla fisica mandaria, arrastar o dedo numa TUI de tela alternada vira
// PageUp/PageDown (ali nao existe scrollback pra rolar), e fechar a tela derruba o socket.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import TerminalMobile from './TerminalMobile.svelte';
import * as auth from '../lib/auth';
import * as api from '../lib/api';
import * as m from '../paraglide/messages';

class FakeWS {
  static ultimo: FakeWS | null = null;
  static abertos = 0;
  static fechados = 0;
  // Deixa um teste segurar o handshake pra valer (o socket existe, o cano ainda nao abriu).
  static autoAbrir = true;
  sent: (string | ArrayBufferLike)[] = [];
  binaryType = '';
  readyState = 1;                              // OPEN: o TermSocket recusa enviar fora disso
  static readonly OPEN = 1;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onclose: ((e?: { reason?: string }) => void) | null = null;
  onopen: (() => void) | null = null;
  constructor(public url: string) {
    FakeWS.ultimo = this;
    FakeWS.abertos++;
    // O handshake real leva um tempo; aqui o `onopen` sai no proximo tick, que e o que separa
    // "socket criado" de "cano aberto" — a barra de teclas so libera no segundo.
    if (FakeWS.autoAbrir) setTimeout(() => this.onopen?.(), 0);
  }
  send(d: string | ArrayBufferLike) { this.sent.push(d); }
  close() { FakeWS.fechados++; this.onclose?.(); }
}

// happy-dom nao roda rAF na mesma volta, e o componente faz probe + import dinamico do xterm antes
// de existir socket. Espera a CONDICAO, nao um numero fixo de quadros: contar quadros passava
// isolado e falhava na suite inteira (o import demora mais com o resto dos testes na frente).
async function ate(cond: () => boolean, limite = 400): Promise<void> {
  for (let i = 0; i < limite && !cond(); i++) {
    await tick();
    await new Promise((r) => requestAnimationFrame(() => r(null)));
  }
}
const quadros = (n: number) => ate(() => false, n);
// "pronto" aqui e o cano ABERTO: a barra de teclas fica desabilitada ate o onopen chegar.
const socketPronto = () => ate(() =>
  FakeWS.ultimo !== null && document.querySelector('.tx-key:not([disabled])') !== null);

const dec = new TextDecoder();
const enviados = () => (FakeWS.ultimo?.sent ?? []).map((b) =>
  typeof b === 'string' ? b : dec.decode(new Uint8Array(b as ArrayBuffer)));

const srv = { baseUrl: 'http://a', token: 'ta', label: 'A' };

beforeEach(() => {
  vi.stubGlobal('WebSocket', FakeWS as never);
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal('cancelAnimationFrame', (id: unknown) => clearTimeout(id as ReturnType<typeof setTimeout>));
  localStorage.clear();
  auth.addServer(srv.baseUrl, srv.token, srv.label);
  auth.selectServer(auth.listServers()[0].id);
  vi.spyOn(api, 'fetchSessionsForServer').mockResolvedValue(
    [{ name: 's', state: 'idle', jsonl: '/j/s.jsonl' }] as never,
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  FakeWS.ultimo = null;
  FakeWS.abertos = 0;
  FakeWS.fechados = 0;
  FakeWS.autoAbrir = true;
});

function montar(props: Record<string, unknown> = {}) {
  const estado = $state({ open: true, sessionName: 's', onClose: vi.fn(), ...props });
  const alvo = document.createElement('div');
  document.body.appendChild(alvo);
  const comp = mount(TerminalMobile, { target: alvo, props: estado });
  return { comp, estado };
}

const tecla = (rotulo: string) =>
  [...document.querySelectorAll<HTMLButtonElement>('.tx-key')]
    .find((b) => b.textContent?.trim() === rotulo || b.getAttribute('aria-label') === rotulo)!;

// Bytes do PTY chegando: e por aqui que o programa entra em tela alternada (o mesmo `\x1b[?1049h`
// que o Claude Code, o vim e o less mandam ao abrir).
function doPty(texto: string) {
  const bytes = new TextEncoder().encode(texto);
  FakeWS.ultimo!.onmessage!({ data: bytes.buffer } as MessageEvent);
}

// happy-dom nao tem TouchEvent; o componente so le `touches[0].clientY`, entao um Event com essa
// propriedade e o suficiente — e mantem o teste preso ao que o codigo REALMENTE usa.
function arrastar(alvo: Element, ys: number[]) {
  const inicio = new Event('touchstart', { bubbles: true });
  Object.defineProperty(inicio, 'touches', { value: [{ clientY: ys[0] }] });
  alvo.dispatchEvent(inicio);
  for (const y of ys.slice(1)) {
    const mv = new Event('touchmove', { bubbles: true, cancelable: true });
    Object.defineProperty(mv, 'touches', { value: [{ clientY: y }] });
    alvo.dispatchEvent(mv);
  }
}

describe('TerminalMobile', () => {
  it('conecta no servidor ATIVO, com o tamanho do terminal na URL', async () => {
    const t = montar();
    await socketPronto();
    expect(FakeWS.ultimo).not.toBeNull();
    const u = new URL(FakeWS.ultimo!.url);
    expect(u.protocol).toBe('ws:');            // http:// -> ws:// (termUrlForServer)
    expect(u.pathname).toBe('/api/sessions/s/term');
    expect(u.searchParams.get('token')).toBe('ta');
    expect(Number(u.searchParams.get('cols'))).toBeGreaterThan(0);
    expect(Number(u.searchParams.get('rows'))).toBeGreaterThan(0);
    unmount(t.comp);
  });

  it('tecla de resgate vira a sequencia crua que o tmux entende', async () => {
    const t = montar();
    await socketPronto();
    tecla('Esc').click();
    tecla(m.term_seta_cima()).click();
    tecla(m.term_rolar_cima_aria()).click();
    // \x1b[A e nao 'Up': do outro lado esta o `tmux attach`, que parseia a entrada como terminal de
    // verdade — mandar nome de tecla escreveria a palavra na tela.
    expect(enviados()).toEqual(['\x1b', '\x1b[A', '\x1b[5~']);
    unmount(t.comp);
  });

  it('nao ha campo de texto: quem digita e o proprio terminal', async () => {
    const t = montar();
    await socketPronto();
    expect(document.querySelector('.tx-input')).toBeNull();
    unmount(t.comp);
  });

  it('arrastar o dedo numa TUI de tela alternada vira PageUp/PageDown', async () => {
    const t = montar();
    await socketPronto();
    doPty('\x1b[?1049h');                 // o programa entra em tela alternada
    await quadros(20);                    // o xterm processa a escrita fora da volta atual
    const tela = document.querySelector('.tx-screen')!;
    // Tela alternada = sem scrollback: arrastar o viewport nao rolaria nada, quem guarda o passado
    // e o programa. O passo e 1/3 da altura, com piso de 40px (happy-dom reporta altura 0).
    arrastar(tela, [400, 300, 200]);      // dedo SOBE = avanca (PageDown)
    expect(enviados().length).toBeGreaterThan(0);
    expect(new Set(enviados())).toEqual(new Set(['\x1b[6~']));
    const ate_aqui = enviados().length;
    arrastar(tela, [0, 100, 200]);        // dedo DESCE = volta pro passado (PageUp)
    expect(new Set(enviados().slice(ate_aqui))).toEqual(new Set(['\x1b[5~']));
    unmount(t.comp);
  });

  it('um salto grande num toque so nao despacha uma rajada de teclas', async () => {
    const t = montar();
    await socketPronto();
    doPty('\x1b[?1049h');
    await quadros(20);
    // Aba que volta do segundo plano, gesto atropelado: sem teto, 4000px / passo de 40px viravam
    // 100 PageUp de uma vez na TUI.
    arrastar(document.querySelector('.tx-screen')!, [0, 4000]);
    expect(enviados().length).toBeLessThanOrEqual(2);
    unmount(t.comp);
  });

  it('cano fechado: o arrasto nao e engolido (deixa a rolagem nativa passar)', async () => {
    FakeWS.autoAbrir = false;
    const t = montar();
    await ate(() => FakeWS.ultimo !== null);
    const tela = document.querySelector('.tx-screen')!;
    const mv = new Event('touchmove', { bubbles: true, cancelable: true });
    Object.defineProperty(mv, 'touches', { value: [{ clientY: 100 }] });
    const ini = new Event('touchstart', { bubbles: true });
    Object.defineProperty(ini, 'touches', { value: [{ clientY: 400 }] });
    tela.dispatchEvent(ini);
    tela.dispatchEvent(mv);
    // preventDefault com a tecla nao saindo mataria a rolagem nativa em troca de nada.
    expect(mv.defaultPrevented).toBe(false);
    expect(enviados()).toEqual([]);
    unmount(t.comp);
  });

  it('com scrollback de verdade (buffer normal) o arrasto NAO e sequestrado', async () => {
    const t = montar();
    await socketPronto();
    // Shell comum: o viewport do xterm tem historico proprio e rola nativo, com a inercia do
    // sistema — traduzir pra PageUp ali roubaria o gesto e mandaria tecla pro programa errado.
    const tela = document.querySelector('.tx-screen')!;
    arrastar(tela, [400, 200, 0]);
    expect(enviados()).toEqual([]);
    unmount(t.comp);
  });

  it('enquanto o cano nao abriu, a barra de teclas fica desabilitada', async () => {
    FakeWS.autoAbrir = false;                    // segura o handshake
    const t = montar();
    await ate(() => FakeWS.ultimo !== null);
    // Janela do handshake: o send seria no-op e a barra nao tem eco pra denunciar — botao apertavel
    // ali e um controle que mente.
    expect(tecla('Esc').disabled).toBe(true);
    tecla('Esc').click();
    expect(enviados()).toEqual([]);
    FakeWS.ultimo!.onopen!();                    // cano abre
    await tick();
    expect(tecla('Esc').disabled).toBe(false);
    unmount(t.comp);
  });

  it('fechar a tela derruba o socket (o tmux volta ao tamanho de antes)', async () => {
    const t = montar();
    await socketPronto();
    expect(FakeWS.abertos).toBe(1);
    t.estado.open = false;
    await ate(() => FakeWS.fechados > 0);
    expect(FakeWS.fechados).toBe(1);
    unmount(t.comp);
  });

  it('sessao que nao existe no servidor: recusa com texto, sem abrir socket', async () => {
    vi.spyOn(api, 'fetchSessionsForServer').mockResolvedValue([] as never);
    const t = montar();
    await ate(() => document.querySelector('.tx-caiu') !== null);
    expect(FakeWS.ultimo).toBeNull();
    expect(document.querySelector('.tx-caiu')?.textContent).toContain(m.erro_sessao_inexistente());
    unmount(t.comp);
  });
});
