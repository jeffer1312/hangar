// @vitest-environment happy-dom
// Follow-up visual, contrato DesktopShell: (1) o empty state compensa EXATAMENTE metade da altura
// da barra de abas quando ela existe (barraRecolhida && !terminalMaximizado) e não compensa sem
// ela; (2) o toggle de contexto da barra fica DESABILITADO quando não há painel montado (sem
// sessão aberta) — decisão do usuário. Sidebar e SessionTabs REAIS; Chat/Board/Canvas/terminal
// stubados.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick, createRawSnippet } from 'svelte';
import { overwriteGetLocale } from '../paraglide/runtime';
import DesktopShell from './DesktopShell.svelte';
import { sidebarPin } from '../lib/sidebarPin.svelte';
import { navMode } from '../lib/navMode.svelte';

function stubDe() { return { default: createRawSnippet(() => ({ render: () => '<div />' })) }; }

vi.mock('../lib/api', () => ({
  getConfig: vi.fn(async () => ({})),
  createSession: vi.fn(), deleteSession: vi.fn(),
  renameSession: vi.fn(async (_old: string, nv: string) => ({ ok: true, name: nv })),
  gitAction: vi.fn(), checkoutBranch: vi.fn(), resumeSession: vi.fn(),
  broadcast: vi.fn(), getHistoryTailForServer: vi.fn(async () => []),
  getPushSettings: vi.fn(async () => ({ muted: [] })),
  setSessionMute: vi.fn(), getBranches: vi.fn(), openEditor: vi.fn(),
  setThenLink: vi.fn(), clearThenLink: vi.fn(),
}));
vi.mock('../lib/auth', () => ({
  getActiveId: vi.fn(() => 'srv-a'),
  selectServer: vi.fn(() => true),
  serverColor: () => '#fff',
  withServer: vi.fn(async (_id: string, fn: () => Promise<unknown>) => fn()),
  listServers: vi.fn(() => []),
  removeServer: vi.fn(),
  addServerWithRollback: vi.fn(),
  renameServer: vi.fn(),
  updateServer: vi.fn(() => true),
  validarPareamento: vi.fn(),
  onServersChanged: vi.fn(() => () => {}),
  snapshotRemocao: vi.fn(() => null),
  removalStillMatches: vi.fn(() => null),
}));
vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: {
    retain: vi.fn(), release: vi.fn(), refreshServers: vi.fn(), reconnect: vi.fn(),
    byServer: [], rows: [], servers: [], loading: false,
  },
}));
vi.mock('../lib/format', () => ({
  rotuloEstado: () => '', stateColors: {}, countAwaiting: () => 0,
  groupSelectedByServer: () => [], initials: (n: string) => n.slice(0, 2),
  projectKey: () => '', projectLabel: () => '', effectiveGroupBy: () => 'server',
  fmtWhen: () => '', sortSessions: (s: unknown[]) => s, latestAssistantEvent: () => null,
  clusterByPair: (s: unknown[]) => s.map((x) => ({ session: x })),
  untrackedReason: () => '', providerName: () => 'claude', providerTag: () => null,
}));
vi.mock('../lib/badge', () => ({ updateBadge: vi.fn() }));
vi.mock('../lib/loop', () => ({ loopBadge: () => null, LOOP_TONE_COLOR: {} }));
vi.mock('../lib/plan', () => ({ planBadge: () => null }));
vi.mock('../lib/sidebarPrefs.svelte', () => ({ sidebarPrefs: { height: 'content' } }));
vi.mock('../lib/configNav', () => ({ abrirConfig: vi.fn() }));

// Telas pesadas stubadas; Sidebar + SessionTabs REAIS; Chat e TerminalPanel sao SPIES (REV G1)
// — o teste da rota legada precisa que o Chat chame onOpenTerminalPanel num clique e que o
// TerminalPanel grave o propro open, que o stub vazio nao faz.
vi.mock('../screens/Chat.svelte', async () => ({ default: (await import('./__ChatSpy.svelte')).default }));
vi.mock('../screens/Board.svelte', stubDe);
vi.mock('../screens/Canvas.svelte', stubDe);
vi.mock('./TerminalPanel.svelte', async () => ({ default: (await import('./__TpSpy.svelte')).default }));
vi.mock('./WorkspaceCommandPalette.svelte', stubDe);
vi.mock('./WorkspaceAttentionStrip.svelte', stubDe);
// Internos do Sidebar real
vi.mock('./CreateSessionSheet.svelte', stubDe);
vi.mock('./SessionContextMenu.svelte', stubDe);
vi.mock('./Git.svelte', stubDe);
vi.mock('./LoopSheet.svelte', stubDe);
vi.mock('./SessionSwitcherSheet.svelte', stubDe);
vi.mock('./HoverPreview.svelte', stubDe);
vi.mock('./PlanBar.svelte', stubDe);
vi.mock('./WorkspaceNav.svelte', stubDe);

function montar(currentSession: string | null) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(DesktopShell, {
    target: el,
    props: {
      currentSession,
      currentKey: currentSession ? `srv-a::${currentSession}` : null,
      view: 'chat',
      overlaySession: null,
      onOpenBoardSession: vi.fn(),
      onOpenCanvasSession: vi.fn(),
      onCloseOverlay: vi.fn(),
      onToggleBoard: vi.fn(),
      onToggleCanvas: vi.fn(),
      onNavigateToChat: vi.fn(),
      onCompare: vi.fn(),
    },
  });
  return { el, comp: comp as never };
}

beforeEach(() => {
  overwriteGetLocale(() => 'pt');   // textos dos estados sao mensagens agora
  sidebarPin.setForced(null);
  sidebarPin.setUser(false);   // sidebar expandida por padrão (sem barra de abas)
  navMode.mode = 'tabs';       // barra de abas ligada por padrão NOS TESTES (modo explícito)
  document.body.innerHTML = '';
});

describe('DesktopShell — empty state compensa a faixa (follow-up visual)', () => {
  // A barra do topo é PERMANENTE desde 10/08/2026 (decisão do usuário): ela não depende mais de a
  // sidebar estar recolhida nem do modo de navegação. O que o modo decide é só ONDE ficam as
  // sessões — tira de abas no topo, ou lista na barra lateral. Os testes abaixo travam essas duas
  // coisas separadamente, porque foi juntá-las que produziu a lista duplicada em tela.
  it('a barra do topo existe mesmo com a sidebar expandida', async () => {
    const t = montar(null);
    await tick();
    expect(document.querySelector('.tabs-bar')).not.toBeNull();
    const empty = document.querySelector<HTMLElement>('.desktop-empty')!;
    expect(empty).not.toBeNull();
    expect(empty.classList.contains('compensa-faixa')).toBe(true);
    unmount(t.comp);
  });

  it('com barra de abas (sidebar recolhida): empty COM compensação', async () => {
    const t = montar(null);
    await tick();
    // Override do board é o mesmo caminho do pin: setForced DEPOIS do mount (effect do Sidebar)
    sidebarPin.setForced(true);
    await tick();
    expect(document.querySelector('.tabs-bar')).not.toBeNull();
    const empty = document.querySelector<HTMLElement>('.desktop-empty')!;
    expect(empty.classList.contains('compensa-faixa')).toBe(true);
    // O translateY(-22px) exato é validado no browser (happy-dom não injeta o CSS escopado);
    // a classe é o contrato determinístico que liga a compensação à presença da barra.
    unmount(t.comp);
  });

  it('toggle do contexto DESABILITADO sem sessão aberta (sem painel montado)', async () => {
    const t = montar(null);
    await tick();
    sidebarPin.setForced(true);
    await tick();
    const ctx = document.querySelector<HTMLButtonElement>('.tab-ctx')!;
    expect(ctx).not.toBeNull();
    expect(ctx.disabled).toBe(true);
    expect(ctx.title).toBe('Sem painel de contexto aberto');
    unmount(t.comp);
  });

  it('toggle do contexto HABILITADO com sessão aberta (painel montado)', async () => {
    const t = montar('sess-1');
    await tick();
    sidebarPin.setForced(true);
    await tick();
    const ctx = document.querySelector<HTMLButtonElement>('.tab-ctx')!;
    expect(ctx.disabled).toBe(false);
    unmount(t.comp);
  });

  it('modo RAIL: a barra existe, mas SEM a tira de sessões (elas ficam no trilho)', async () => {
    navMode.mode = 'rail';
    const t = montar(null);
    await tick();
    sidebarPin.setForced(true);
    await tick();
    expect(document.querySelector('.tabs-bar')).not.toBeNull();
    expect(document.querySelector('.tab-ctx')).not.toBeNull();
    // A tira some; o espaçador entra no lugar dela pra as ações seguirem à direita.
    expect(document.querySelector('.tabs-strip')).toBeNull();
    expect(document.querySelector('.tabs-vazio')).not.toBeNull();
    // E a barra lateral continua visível: é onde as sessões estão neste modo.
    expect(document.querySelector('.sidebar-wrap.oculta')).toBeNull();
    unmount(t.comp);
  });

  it('modo ABAS: a tira aparece e a barra lateral é escondida (lista num lugar só)', async () => {
    navMode.mode = 'tabs';
    const t = montar(null);
    await tick();
    expect(document.querySelector('.tabs-strip')).not.toBeNull();
    // Escondida, NÃO desmontada: a barra do topo delega +/⋯/menu pra dentro da Sidebar via bridge.
    expect(document.querySelector('.sidebar-wrap.oculta')).not.toBeNull();
    unmount(t.comp);
  });

  it('empty state cita a faixa no modo tabs e a barra no modo rail', async () => {
    // modo tabs (padrão dos testes): sidebar recolhida -> a faixa de abas existe e a dica a cita
    const t1 = montar(null);
    await tick();
    sidebarPin.setForced(true);
    await tick();
    expect(document.querySelector<HTMLElement>('.empty-sub')!.textContent).toContain('na faixa de abas');
    unmount(t1.comp);

    // modo rail: as sessões ficam no trilho -> a dica fala da barra lateral (a faixa segue lá)
    navMode.mode = 'rail';
    const t2 = montar(null);
    await tick();
    sidebarPin.setForced(true);
    await tick();
    expect(document.querySelector<HTMLElement>('.empty-sub')!.textContent).toContain('na barra lateral');
    unmount(t2.comp);
  });

  // Parecer Task 11, B6: com o visor de arquivo aberto DENTRO do overlay (board/canvas), o
  // primeiro Escape fecha o visor (dono do Esc é o Chat) e NÃO o overlay — o guard de captura
  // do DesktopShell procura o [data-arq-visor] visível e devolve. O segundo Escape (visão
  // sem o visor) fecha o overlay.
  it('Escape com visor aberto dentro do overlay NAO fecha o overlay (B6)', async () => {
    const onCloseOverlay = vi.fn();
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(DesktopShell, {
      target: el,
      props: {
        currentSession: null,
        currentKey: null,
        view: 'chat',
        overlaySession: { name: 'sess', serverId: 'srv-a' },   // board/canvas aberto
        onOpenBoardSession: vi.fn(),
        onOpenCanvasSession: vi.fn(),
        onCloseOverlay,
        onToggleBoard: vi.fn(),
        onToggleCanvas: vi.fn(),
        onNavigateToChat: vi.fn(),
        onCompare: vi.fn(),
      },
    });
    await tick();
    // o visor do arquivo monta DENTRO do Chat do overlay (stubado aqui) — injeta o marcador
    // real que o guard procura, com caixa visível
    const visor = document.createElement('div');
    visor.setAttribute('data-arq-visor', '');
    visor.style.width = '100px';
    visor.style.height = '100px';
    document.body.appendChild(visor);
    // dispara no body (sobe ate a window na captura): o guard do DesktopShell chama
    // e.target.closest, e o target de um evento na window crua nao tem closest
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await tick();
    expect(onCloseOverlay).not.toHaveBeenCalled();   // 1o Esc: fecha o visor, nao o overlay
    visor.remove();                                   // o visor fechou (Chat limpou selecionado)
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await tick();
    expect(onCloseOverlay).toHaveBeenCalledTimes(1);  // 2o Esc: fecha o overlay
    unmount(comp as never);
  });

  // ── REV G1 (round REPROVA): painel de terminal x rota sem serverId ─────────────────────────
  // App.svelte:439 monta currentKey = (route.serverId ?? '') + '::' + nome => "::s" na forma
  // legada #/chat/<nome> (aceita de proposito, route.ts:30). O serverId vira a string VAZIA — e
  // isso tragava o bloqueador: sessoesNaTela caia no ativo (||) mas terminalKey nao (??), e o
  // $effect de fechamento derrubava o painel no MESMO flush em que ele abria (sem a correcao,
  // este teste reporta open:false). Chat/TerminalPanel sao spies; getActiveId() fixo em 'srv-a'.
  function montarKey(currentKey: string | null) {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(DesktopShell, {
      target: el,
      props: {
        currentSession: 's',
        currentKey,
        view: 'chat',
        overlaySession: null,
        onOpenBoardSession: vi.fn(), onOpenCanvasSession: vi.fn(), onCloseOverlay: vi.fn(),
        onToggleBoard: vi.fn(), onToggleCanvas: vi.fn(), onNavigateToChat: vi.fn(), onCompare: vi.fn(),
      },
    });
    return { el, comp: comp as never };
  }

  async function abrirEObservar(currentKey: string | null) {
    const g = globalThis as Record<string, unknown>;
    g.__tpOpen = undefined; g.__tpKey = undefined;
    const { comp } = montarKey(currentKey);
    await tick(); await tick();
    (document.querySelector('[data-spy="abrir-term"]') as HTMLButtonElement).click();
    await tick(); await tick(); await tick();
    const r = { open: g.__tpOpen, key: g.__tpKey };
    unmount(comp);
    document.body.innerHTML = '';
    return r;
  }

  it('REV G1: rota COM servidor (#/chat/srv-a/s): o painel abre e fica aberto', async () => {
    const r = await abrirEObservar('srv-a::s');
    expect(r.open).toBe(true);
  });

  it('REV G1: rota LEGADA (#/chat/s): o painel abre e fica aberto, connKey no ativo (dono da sessao)', async () => {
    const r = await abrirEObservar('::s');
    expect(r.open).toBe(true);
    expect(r.key).toBe('srv-a::s');
  });

  // ── REV G1: o efeito de fechamento quebrou o split? ────────────────────────────────────────
  // Principal em srv-b (rota), ATIVO = srv-a (mock fixo): os dois abrem o terminal, nenhum fecha.
  it('REV G1: principal em srv-b (rota) + split no ativo: os dois abrem o terminal e nenhum fecha', async () => {
    const g = globalThis as Record<string, unknown>;
    g.__tpOpen = undefined; g.__tpKey = undefined;
    const { comp } = montarKey('srv-b::s');
    await tick(); await tick();
    (document.querySelector('[data-spy="split"]') as HTMLButtonElement).click();
    await tick(); await tick();
    const botoes = () => Array.from(document.querySelectorAll('[data-spy="abrir-term"]')) as HTMLButtonElement[];

    // terminal da PRINCIPAL (sessao de srv-b, ativo srv-a)
    botoes().find((b) => b.dataset.nome === 's')!.click();
    await tick(); await tick(); await tick();
    expect(g.__tpOpen).toBe(true);
    expect(g.__tpKey).toBe('srv-b::s');

    // terminal do SPLIT (segue o ativo, como o Chat do split)
    botoes().find((b) => b.dataset.nome === 'split-s')!.click();
    await tick(); await tick(); await tick();
    expect(g.__tpOpen).toBe(true);
    expect(g.__tpKey).toBe('srv-a::split-s');

    unmount(comp);
    document.body.innerHTML = '';
  });
});
