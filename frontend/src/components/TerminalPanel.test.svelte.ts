// @vitest-environment happy-dom
// Correção da Task 2 (round REPROVA, parecer task-2-a8e13182.md): três bloqueadores no
// TerminalPanel que nenhum teste de helper cobria — a mutação da revisão (passar servidor falso ao
// caller do Shell) deixava `term.test.ts` 11/11 verde, e o fluxo assíncrono do painel ficava
// invisível.
//
// B1 — o Shell relia o servidor ATIVO depois do await do POST: trocando o ativo no meio, o POST
//       criava term-<nome> em A e o socket abria em B. Corrigido capturando o servidor no clique;
//       a Task G1 (desc-shell-srv) trocou a ORIGEM do valor: o capturado passou a ser o servidor DA
//       SESSAO (servidorDe(connKey), o mesmo da aba attach), nunca o ativo — com sessão em B e
//       ativo em A, o POST /shell saía pra A (404, "não aparece nada").
// B2 — o erro de attach desmontava o host (`bind:this`), o $effect relia `host` null e a troca de
//       DOM reexecutava o efeito até `effect_update_depth_exceeded` (tela travada); e a mensagem
//       não era anunciada ao leitor de tela (WCAG 4.1.3, sem role="alert").
// B3 — o painel não assinava `onServersChanged`; trocar/remover servidor com o painel aberto
//       deixava socket + credencial velhos vivos.
//
// Sem rede/processo real: fetchSessionsForServer é stubbed e o WebSocket é um fake (mesma regra do
// grupo — nenhum teste toca rede; a prova é a URL que o socket monta). O POST /shell tem o DESTINO
// provado pelo spy do openShell (recebe o servidor da sessão), no molde do ContasRotaAlvo.test.ts.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import TerminalPanel from './TerminalPanel.svelte';
import * as auth from '../lib/auth';
import * as api from '@hangar/core';
import * as term from '../lib/term';

// ── dublês de I/O ────────────────────────────────────────────────────────────

class FakeWS {
  static ultimo: FakeWS | null = null;
  static todos: FakeWS[] = [];
  sent: (string | ArrayBufferLike)[] = [];
  binaryType = '';
  onmessage: ((e: MessageEvent) => void) | null = null;
  onclose: ((e?: { reason?: string }) => void) | null = null;
  constructor(public url: string) { FakeWS.todos.push(this); FakeWS.ultimo = this; }
  send(d: string | ArrayBufferLike) { this.sent.push(d); }
  close() { this.onclose?.(); }
}

// happy-dom não roda rAF na mesma volta; o TerminalPanel usa import dinâmico + várias awaits.
async function frames(n = 8): Promise<void> {
  for (let i = 0; i < n; i++) {
    await tick();
    await new Promise((r) => requestAnimationFrame(() => r(null)));
  }
}

const sess = (name: string) => ({ name, state: 'idle' as const, jsonl: `/j/${name}.jsonl` });
const srvA = { id: 'a', label: 'A', baseUrl: 'http://a', token: 'ta' };
const srvB = { id: 'b', label: 'B', baseUrl: 'http://b', token: 'tb' };

let resolveShell: ((r: { ok: true; shell: string }) => void) | null = null;
let fetchSessoes: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.stubGlobal('WebSocket', FakeWS as never);
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal('cancelAnimationFrame', (id: unknown) => clearTimeout(id as ReturnType<typeof setTimeout>));
  // localStorage limpo + dois servidores; A é o ATIVO.
  localStorage.clear();
  auth.addServer(srvA.baseUrl, srvA.token, srvA.label);
  auth.addServer(srvB.baseUrl, srvB.token, srvB.label);
  // Os ids são gerados (srv-xxxxx), não 'a'/'b': seleciona o de baseUrl http://a.
  const idA = auth.listServers().find((s) => s.baseUrl === srvA.baseUrl)!.id;
  auth.selectServer(idA);
  // fetchSessionsForServer (usado pelo probe) fake; openShell (POST) fake e adiado; o nativo
  // (POST /open-terminal) fake imediato — o DESTINO dos dois é provado pelo spy, no molde do
  // ContasRotaAlvo.test.ts (o POST da sessão de B tem que sair para o baseUrl de B).
  fetchSessoes = vi.fn<(s: import('../lib/auth').Server) => Promise<ReturnType<typeof sess>[]>>()
    .mockResolvedValue([sess('s')]);
  vi.spyOn(api, 'fetchSessionsForServer').mockImplementation(fetchSessoes as never);
  vi.spyOn(api, 'openShell').mockImplementation(() =>
    new Promise((r) => { resolveShell = r; }),
  );
  vi.spyOn(api, 'openNativeTerminal').mockResolvedValue({ ok: true } as never);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  FakeWS.todos = [];
  FakeWS.ultimo = null;
});

// ── helpers ──────────────────────────────────────────────────────────────────

interface PanelProps {
  sessionName: string; connKey: string; open: boolean;
  onClose: () => void; onMaximizar?: (v: boolean) => void;
}

function montar(props: Partial<PanelProps> = {}) {
  // connKey real: o DesktopShell usa o id do servidor na lista (workspaceSessionKey).
  const idB = auth.listServers().find((s) => s.baseUrl === srvB.baseUrl)!.id;
  const estado = $state<PanelProps>({
    sessionName: 's', connKey: `${idB}::s`, open: true,
    onClose: vi.fn(), onMaximizar: vi.fn(),
    ...props,
  });
  const alvo = document.createElement('div');
  document.body.appendChild(alvo);
  const comp = mount(TerminalPanel, { target: alvo, props: estado });
  return { comp, estado };
}

// ── testes ───────────────────────────────────────────────────────────────────

describe('TerminalPanel — bloqueadores da Task 2 e destino da Task G1', () => {
  it('B1: o socket do Shell usa o servidor da SESSAO (capturado no clique), mesmo trocando o ativo no meio', async () => {
    const { comp } = montar();
    await frames();
    const idB = auth.listServers().find((s) => s.baseUrl === srvB.baseUrl)!.id;
    // B1: abre a aba Shell: POST /shell fica PENDENTE
    (document.querySelector('.tp-aba:nth-child(2)') as HTMLButtonElement).click();
    await frames(2);
    // enquanto o POST espera, o usuário troca o ativo para B (o da sessão, que já é B)
    auth.selectServer(idB);
    // POST resolve
    resolveShell!({ ok: true, shell: 'term-s' });
    // espera o socket do shell nascer (na suite cheia os imports dinamicos sao mais lentos que
    // os frames fixos — poll por ate 2s)
    let url = '';
    for (let i = 0; i < 20; i++) {
      url = FakeWS.ultimo?.url ?? '';
      if (url.includes('/api/sessions/term-s/term?')) break;
      await new Promise((r) => setTimeout(r, 100));
      await tick();
    }
    // o attach (s/term) nasce primeiro; o do shell (term-s/term) e o ultimo
    expect(url).toContain('/api/sessions/term-s/term?');
    // O socket deve usar o servidor da SESSAO (B) — não o ativo nem o trocado no meio.
    expect(url).toContain(`ws://${srvB.baseUrl.replace('http://', '')}/api/sessions/term-s/term?`);
    expect(url).toContain('token=tb');
    expect(url).not.toContain('ws://a');
    unmount(comp);
  });

  // Task G1 (desc-shell-srv): sessao em B com o ativo em A — o POST /shell e o WebSocket tem de
  // sair com o baseUrl de B (o servidor da sessao), nao do ativo. Antes: POST ia pra A, o A
  // respondia 404 (sessao inexistente) e a aba mostrava "sessao não encontrada".
  it('G1: sessao em B com ativo em A — o POST /shell sai para o servidor da SESSAO (B)', async () => {
    const { comp } = montar();   // connKey 'b::s' (sessao de B), ativo = A (beforeEach)
    await frames();
    (document.querySelector('.tp-aba:nth-child(2)') as HTMLButtonElement).click();
    await frames(2);
    // O spy do openShell recebeu o servidor DA SESSÃO como alvo — nunca o ativo.
    expect(api.openShell).toHaveBeenCalledWith(
      expect.objectContaining({ baseUrl: srvB.baseUrl, token: 'tb' }), 's');
    resolveShell!({ ok: true, shell: 'term-s' });
    let url = '';
    for (let i = 0; i < 20; i++) {
      url = FakeWS.ultimo?.url ?? '';
      if (url.includes('/api/sessions/term-s/term?')) break;
      await new Promise((r) => setTimeout(r, 100));
      await tick();
    }
    expect(url).toContain(`ws://${srvB.baseUrl.replace('http://', '')}/api/sessions/term-s/term?`);
    expect(url).toContain('token=tb');
    expect(url).not.toContain('ws://a');
    unmount(comp);
  });

  // Mesma regra do POST /shell: o terminal nativo é da SESSÃO — o POST /open-terminal tem de sair
  // para o servidor dela, não para o ativo (senão abriria janela na máquina errada).
  it('G1: o POST /open-terminal sai para o servidor da SESSAO (B), nao para o ativo (A)', async () => {
    const { comp } = montar();   // connKey 'b::s', ativo = A
    await frames();
    // primeiro botão FILHO DIRETO da barra = ↗ (as abas vivem dentro de .tp-abas)
    (document.querySelectorAll('.tp-bar > button')[0] as HTMLButtonElement).click();
    expect(api.openNativeTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ baseUrl: srvB.baseUrl, token: 'tb' }), 's');
    unmount(comp);
  });

  it('B2: erro de attach NÃO desmonta o host, aparece com role=alert e não estoura profundidade de efeito', async () => {
    const { comp, estado } = montar();
    // probe devolve lista SEM a sessão -> anexoErro. connKey 'b::s' resolve para o servidor B.
    fetchSessoes.mockResolvedValue([sess('outra')]);
    await frames(4);
    const msg = document.querySelector('[role="alert"] .tp-erro')?.textContent ?? '';
    // aceita os dois idiomas: o locale default do app e inglês no happy-dom
    expect(msg).toMatch(/não encontrada|not found|no longer exists/i);
    const alerta = document.querySelector('[role="alert"]');
    expect(alerta).not.toBeNull();
    // o host NÃO sumiu (fica montado, mesmo com o erro por cima)
    expect(document.querySelector('.tp-screen:not(.tp-status)')).not.toBeNull();
    unmount(comp);
  });

  it('B3: trocar o token do servidor fecha o socket velho e reconecta com o token NOVO', async () => {
    const { comp } = montar();
    // connKey 'b::s' -> servidor B; o socket deve nascer com o token de B.
    await frames(4);
    const urlInicial = FakeWS.ultimo?.url ?? '';
    expect(urlInicial).toContain('token=tb');
    // atualiza o token do servidor B (o da sessão anexada)
    const idB = auth.listServers().find((s) => s.baseUrl === srvB.baseUrl)!.id;
    auth.updateServer(idB, { token: 'tb-novo' });
    await frames(4);
    expect(FakeWS.ultimo?.url).toContain('token=tb-novo');
    unmount(comp);
  });
});
