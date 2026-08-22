// @vitest-environment happy-dom
// O terminal do celular manda BYTES CRUS pro PTY (o desktop digita direto no xterm; aqui quem
// digita e o campo de texto e a barra de teclas). Estes testes travam o que a tela promete:
// as teclas de resgate viram a sequencia que uma tecla fisica mandaria, "envia ⏎" nao separa o
// texto do Enter em dois envios, e fechar a tela derruba o socket.
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
  sent: (string | ArrayBufferLike)[] = [];
  binaryType = '';
  readyState = 1;                              // OPEN: o TermSocket recusa enviar fora disso
  static readonly OPEN = 1;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onclose: ((e?: { reason?: string }) => void) | null = null;
  constructor(public url: string) { FakeWS.ultimo = this; FakeWS.abertos++; }
  send(d: string | ArrayBufferLike) { this.sent.push(d); }
  close() { FakeWS.fechados++; this.onclose?.(); }
}

// happy-dom nao roda rAF na mesma volta, e o componente faz probe + import dinamico do xterm antes
// de existir socket. Espera a CONDICAO, nao um numero fixo de quadros: contar quadros passava
// isolado e falhava na suite inteira (o import demora mais com o resto dos testes na frente).
async function ate(cond: () => boolean, quadros = 400): Promise<void> {
  for (let i = 0; i < quadros && !cond(); i++) {
    await tick();
    await new Promise((r) => requestAnimationFrame(() => r(null)));
  }
}
const socketPronto = () => ate(() => FakeWS.ultimo !== null);

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

  it('"envia ⏎" manda texto e Enter num ENVIO so, e limpa o campo', async () => {
    const t = montar();
    await socketPronto();
    const campo = document.querySelector<HTMLInputElement>('.tx-input')!;
    campo.value = 'oi';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.closest('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await tick();
    expect(enviados()).toEqual(['oi\r']);
    expect(campo.value).toBe('');
    unmount(t.comp);
  });

  it('conexao caida: NAO limpa o campo (o texto sumia sem nunca chegar no PTY)', async () => {
    const t = montar();
    await socketPronto();
    FakeWS.ultimo!.readyState = 3;                 // CLOSED: o TermSocket.send vira no-op
    const campo = document.querySelector<HTMLInputElement>('.tx-input')!;
    campo.value = 'comando importante';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.closest('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await tick();
    expect(enviados()).toEqual([]);
    expect(campo.value).toBe('comando importante');   // continua ali pra reenviar
    unmount(t.comp);
  });

  it('enviar SEM Enter nao acrescenta o \\r (picker/filtro submeteria antes da hora)', async () => {
    const t = montar();
    await socketPronto();
    const campo = document.querySelector<HTMLInputElement>('.tx-input')!;
    campo.value = 'filtro';
    campo.dispatchEvent(new Event('input'));
    await tick();
    tecla(m.term_enviar_sem_enter()).click();
    expect(enviados()).toEqual(['filtro']);
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
