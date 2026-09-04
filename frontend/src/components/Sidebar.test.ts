// @vitest-environment happy-dom
// A Sidebar monta uma árvore pesada; os componentes de trabalho (sheets, menus, git, loop) viram
// stubs. Cobertura do fluxo de adicionar servidor (que vivia aqui desde a round 5) migrou pra
// ServidoresSettings na Task 4b/4c — ela mora em ServidoresSettings.test.ts.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick, createRawSnippet } from 'svelte';
import Sidebar from './Sidebar.svelte';
import * as auth from '../lib/auth';
import { abrirConfig } from '../lib/configNav';
import { overwriteGetLocale } from '../paraglide/runtime';

// Snippet criado DENTRO de cada factory: vi.mock é hoisted; function declaration é hoisted também
// (const top-level cairia em TDZ quando a factory rodasse).
function stubDe() { return { default: createRawSnippet(() => ({ render: () => '<div />' })) }; }

// O store mockado é objeto PLAIN (sem reatividade Svelte): os deriveds do Sidebar leem as arrays
// uma vez, no mount. Por isso os testes preenchem as arrays ANTES de montar — vi.hoisted pra o
// teste alcançar a MESMA referência que a factory do mock retornou.
const storeState = vi.hoisted(() => ({
  byServer: [] as unknown[],
  rows: [] as unknown[],
  servers: [] as unknown[],
}));

vi.mock('../lib/api', () => ({
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  isTimeoutError: vi.fn(() => false),
  isAbortError: vi.fn(() => false),
  errorDetail: vi.fn(async () => ''),
  createSession: vi.fn(), deleteSession: vi.fn(),
  // Contrato real do renameSession: devolve { ok, name } (o doRename lê r.name).
  renameSession: vi.fn(async (_old: string, nv: string) => ({ ok: true, name: nv })),
  gitAction: vi.fn(), checkoutBranch: vi.fn(), resumeSession: vi.fn(),
  broadcast: vi.fn(), getHistoryTailForServer: vi.fn(async () => []),
  // Imports do SessionContextMenu REAL (não stubado): o onMount chama getPushSettings.
  getPushSettings: vi.fn(async () => ({ muted: [] })),
  setSessionMute: vi.fn(), openEditor: vi.fn(),
  setThenLink: vi.fn(), clearThenLink: vi.fn(),
  // Git REAL montado nos testes de filesInContext (abaixo): o store faz refresh no load.
  getBranches: vi.fn(async () => ({ branches: [], current: null, remotes: [], dirty: false })),
  getChangedFiles: vi.fn(async () => ({ files: [], sequencer: null })),
  getGitLog: vi.fn(async () => ({ commits: [], truncated: false })),
  listFiles: vi.fn(async () => ({ entries: [], truncated: false })),
  readFile: vi.fn(async () => ({ path: 'a.txt', text: 'A', size: 1, truncated: false, digest: 'abc' })),
  searchFiles: vi.fn(async () => ({ hits: [], truncated: false, mode: 'names' })),
  pathDiff: vi.fn(async () => ({
    path: 'a.txt', diff: '', truncated: false,
    escopo_pedido: 'branch', escopo_usado: 'branch', base: null, motivo: null,
  })),
}));
vi.mock('../lib/auth', () => ({
  getActiveId: vi.fn(() => null),
  selectServer: vi.fn(() => true),
  serverColor: () => '#fff',
  withServer: vi.fn(async (_id: string, fn: () => Promise<unknown>) => fn()),
}));
vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: {
    retain: vi.fn(), release: vi.fn(),
    markDeleting: vi.fn(), unmarkDeleting: vi.fn(),
    byServer: storeState.byServer, rows: storeState.rows, servers: storeState.servers,
    loading: false,
  },
}));
vi.mock('../lib/format', () => ({
  // Rótulo REAL de estado (agora funcao): o trilho original anuncia estado no aria-label/title.
  rotuloEstado: (s: string) => ({ working: 'em execução', idle: 'pronto', awaiting_input: 'aguardando', dead: 'encerrado' })[s] ?? '',
  stateColors: {}, countAwaiting: () => 0,
  groupSelectedByServer: () => [], initials: (n: string) => n.slice(0, 2),
  projectKey: () => '', projectLabel: () => '', effectiveGroupBy: () => 'server',
  fmtWhen: () => '', sortSessions: (s: unknown[]) => s, latestAssistantEvent: () => null,
  // Mesmo shape do real: itens do cluster são {session} (ou {kind:'header',...}); o template lê
  // item.session — sessão crua no lugar certo quebraria na chave do each.
  clusterByPair: (s: unknown[]) => s.map((x) => ({ session: x })),
  untrackedReason: () => '', providerName: () => 'claude',
  providerTag: () => null,
  cwdParts: (c: string | undefined) => ({ prefix: '', base: c ?? '' }),
}));
vi.mock('../lib/badge', () => ({ updateBadge: vi.fn() }));
vi.mock('../lib/loop', () => ({ loopBadge: () => null, LOOP_TONE_COLOR: {} }));
vi.mock('../lib/plan', () => ({ planBadge: () => null }));
vi.mock('../lib/sidebarPrefs.svelte', () => ({ sidebarPrefs: { height: 'content' } }));
vi.mock('../lib/configNav', () => ({ abrirConfig: vi.fn() }));

// Dublê que PUBLICA as props, não o stub cego: é o que deixa afirmar em que modo a folha abriu
// (normal x bastão). Renderiza uma única div, como o stub — os casos que contam markup não mudam.
vi.mock('./CreateSessionSheet.svelte', async () => ({
  default: (await import('./CreateSessionSheet.spy.svelte')).default,
}));
// SessionContextMenu NÃO é stubado: o teste clica no botão Renomear REAL do menu (mesmo caminho
// que as abas usam — bridge -> openMenu -> Rename). Stub de raw snippet não repassava o clique
// pro listener do setup no happy-dom (o nó é substituído no flush do Svelte).
// Git REAL (não stubado) nos testes de filesInContext: o alvo é a fileira .gt-tab que o GitTabs
// renderiza a partir da prop filesInContext da Sidebar. As abas irmãs viram stub (mesmo padrão
// do GitTabs.test.ts); FilesPanel/FileViewer reais, com a API mockada acima.
vi.mock('./git/GitChangesTab.svelte', stubDe);
vi.mock('./git/GitHistoryTab.svelte', stubDe);
vi.mock('./git/GitBranchesTab.svelte', stubDe);
vi.mock('./git/GitStatusBar.svelte', stubDe);
vi.mock('./git/RepoMenu.svelte', stubDe);
vi.mock('./LoopSheet.svelte', stubDe);
vi.mock('./SessionSwitcherSheet.svelte', stubDe);
vi.mock('./HoverPreview.svelte', stubDe);
vi.mock('./PlanBar.svelte', stubDe);
vi.mock('./WorkspaceNav.svelte', stubDe);

const authMock = vi.mocked(auth);
const navMock = vi.mocked(abrirConfig);

// sidebarPin é o módulo REAL (só o store de sessões é mockado): o pin decide se a <aside> monta.
// beforeEach garante sidebar EXPANDIDA por padrão — o teste do recolhido seta e limpa o dele.
import { sidebarPin } from '../lib/sidebarPin.svelte';
import { sidebarBridge } from '../lib/sidebarBridge';
import { navMode } from '../lib/navMode.svelte';
import { ctxPanel } from '../lib/ctxPanel.svelte';
import * as api from '../lib/api';
import * as m from '../paraglide/messages';
import type { AggSession } from '../lib/types';

// O body-scroll-lock do bits-ui agenda um cleanup de 24ms ao desmontar um dialog/sheet; sem
// esperar, o timer dispara DEPOIS do teardown do happy-dom ("document is not defined" — erro
// não tratado que faz a suite inteira sair com exit 1 mesmo com todos os testes verdes).
afterEach(() => new Promise((r) => setTimeout(r, 30)));

beforeEach(() => {
  overwriteGetLocale(() => 'pt');   // textos dos menus e dialogs sao mensagens agora
  vi.clearAllMocks();   // contagens de chamada não vazam entre testes
  sidebarPin.setUser(false);   // pin do usuário: expandido (o persistido '0' não vaza do teste anterior)
  navMode.mode = 'tabs';       // modo abas nos testes existentes (a aside some com o pin)
  ctxPanel.recolhido = false;  // painel de contexto aberto por padrão
});

function montar(over: { ctxDisponivel?: boolean } = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(Sidebar, {
    target: el,
    props: {
      currentSession: null,
      onSelect: vi.fn(),
      onCompare: vi.fn(),
      boardActive: false,
      canvasActive: false,
      orqActive: false,
      view: 'chat',
      onSelectView: vi.fn(),
      onOpenCommand: vi.fn(),
      onCollapsedChange: vi.fn(),
      overlaySession: null,
      ...(over.ctxDisponivel !== undefined ? { ctxDisponivel: over.ctxDisponivel } : {}),
    },
  });
  return { el, comp: comp as never };
}

describe('Sidebar — a engrenagem MUDOU pra barra do topo (10/08/2026)', () => {
  // Plano A da reorganização: a barra do topo virou permanente, então os comandos do app moram nela
  // — engrenagem e "mais opções" saíram do rodapé do trilho, e só o "+ Nova" ficou nos dois lugares
  // (pedido do usuário). O teste vive aqui pra travar a AUSÊNCIA: se a engrenagem voltar pro rodapé
  // sem alguém tirar a da barra, o mesmo comando fica em dois lugares na mesma tela outra vez.
  // A presença dela (com o ponto do servidor ativo) é coberta em SessionTabs.test.ts.
  it('o rodapé não tem mais engrenagem nem kebab; o + Nova continua', async () => {
    const t = montar();
    await tick();
    expect(document.querySelector('.acct-btn')).toBeNull();
    expect(document.querySelector('.kebab-btn')).toBeNull();
    expect(document.querySelector('.cta-new')).not.toBeNull();
    unmount(t.comp);
  });
});

describe('Sidebar — renomear com a sidebar recolhida (round 7)', () => {
  // Uma sessão pra linha existir no store; a sessão entra ANTES do mount (o mock do store é plain,
  // sem reatividade — os deriveds leem uma vez).
  function comUmaSessao() {
    // servers NÃO pode ficar vazio: o derived `groups` retorna [] cedo quando servers.length === 0
    storeState.servers.length = 0;
    storeState.servers.push({ id: 'srv-a', label: 'Servidor A', baseUrl: 'http://a', token: 'x' });
    storeState.byServer.length = 0;
    storeState.byServer.push({
      server: { id: 'srv-a', label: 'Servidor A' },
      sessions: [{ name: 'sess-1', serverId: 'srv-a', state: 'idle' }],
      error: null, loaded: true,
    });
  }
  function abrirMenuDaAba() {
    // Caminho REAL das abas: sidebarBridge.openSessionMenu -> openMenu do Sidebar -> menu monta.
    sidebarBridge.openSessionMenu(
      new MouseEvent('contextmenu', { clientX: 5, clientY: 5 }),
      { name: 'sess-1', serverId: 'srv-a' } as unknown as AggSession,
      'srv-a',
    );
  }

  it('RECOLHIDA: bridge + Rename do menu abrem diálogo acessível e renomeiam de verdade', async () => {
    comUmaSessao();
    const t = montar();
    await tick();
    // override do board/canvas DEPOIS do mount: o $effect do Sidebar (overviewActive) roda setForced
    // no mount e limparia o override pré-montado — no app real o board já chega no mount com o
    // override ligado, o que o teste reproduz aqui na sequência.
    sidebarPin.setForced(true);
    await tick();
    expect(document.querySelector('.sidebar')).toBeNull();   // aside realmente ausente
    abrirMenuDaAba();
    await tick();
    // Botão REAL do SessionContextMenu (não stubado)
    const renameBtn = [...document.querySelectorAll<HTMLButtonElement>('.ctx-menu button')]
      .find((b) => b.textContent?.trim() === 'Renomear');
    expect(renameBtn).not.toBeNull();
    renameBtn!.click();
    await tick();
    // SEM input inline (não existe fora da aside): o diálogo é o único caminho
    expect(document.querySelector('.sess-edit')).toBeNull();
    const dialog = document.querySelector<HTMLElement>('.confirm-card');
    expect(dialog).not.toBeNull();
    const input = document.querySelector<HTMLInputElement>('input[aria-label="Novo nome da sessão"]');
    expect(input).not.toBeNull();
    // conclui o rename: digita e confirma
    input!.value = 'sess-novo';
    input!.dispatchEvent(new Event('input'));
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-primary')!.click();
    await tick(); await tick();   // fechamento é assíncrono (espera o doRename resolver — round 2)
    expect(api.renameSession).toHaveBeenCalledWith('sess-1', 'sess-novo');
    expect(document.querySelector('.confirm-card')).toBeNull();   // fechou sozinho após confirmar
    sidebarPin.setForced(null);
    unmount(t.comp);
  });

  it('EXPANDIDA: Rename do menu segue editando INLINE (sem diálogo)', async () => {
    comUmaSessao();
    const t = montar();
    await tick();
    expect(document.querySelector('.sidebar')).not.toBeNull();
    abrirMenuDaAba();
    await tick();
    const renameBtn = [...document.querySelectorAll<HTMLButtonElement>('.ctx-menu button')]
      .find((b) => b.textContent?.trim() === 'Renomear');
    expect(renameBtn).not.toBeNull();
    renameBtn!.click();
    await tick();
    expect(document.querySelector('.confirm-card')).toBeNull();   // nenhum diálogo
    expect(document.querySelector('.sess-edit')).not.toBeNull();  // input inline na linha
    unmount(t.comp);
  });

  // Round 2: foco/teclado de VERDADE no diálogo (não input.value + clique no botão).
  function abrirDialogoRename() {
    abrirMenuDaAba();
    return tick().then(() => {
      const renameBtn = [...document.querySelectorAll<HTMLButtonElement>('.ctx-menu button')]
        .find((b) => b.textContent?.trim() === 'Renomear')!;
      renameBtn.click();
      return tick();
    });
  }

  it('RECOLHIDA: diálogo foca o input sozinho (pós-tick); digitar + Enter renomeia sem acionar Cancelar', async () => {
    comUmaSessao();
    const t = montar();
    await tick();
    sidebarPin.setForced(true);
    await tick();
    await abrirDialogoRename();
    await tick();
    const input = document.querySelector<HTMLInputElement>('input[aria-label="Novo nome da sessão"]')!;
    // Foco inicial explícito no CAMPO — não no safeButton (Cancelar) do ConfirmDialog
    expect(document.activeElement).toBe(input);
    // Digitação real (valor + evento de input do bind:value)
    input.value = 'sess-novo';
    input.dispatchEvent(new Event('input'));
    await tick();
    // Enter no campo (teclado) renomeia — e não dispara Cancelar
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick(); await tick();   // fechamento assíncrono (espera o doRename resolver)
    expect(api.renameSession).toHaveBeenCalledWith('sess-1', 'sess-novo');
    expect(document.querySelector('.confirm-card')).toBeNull();   // fechou no sucesso
    sidebarPin.setForced(null);
    unmount(t.comp);
  });

  it('RECOLHIDA: valor que trima pro nome atual mantém Renomear desabilitado (sem no-op que fecha)', async () => {
    comUmaSessao();
    const t = montar();
    await tick();
    sidebarPin.setForced(true);
    await tick();
    await abrirDialogoRename();
    await tick();
    const input = document.querySelector<HTMLInputElement>('input[aria-label="Novo nome da sessão"]')!;
    input.value = '  sess-1  ';   // trim() === nome atual (só espaços a mais)
    input.dispatchEvent(new Event('input'));
    await tick();
    const confirmBtn = document.querySelector<HTMLButtonElement>('.confirm-card .c-primary')!;
    expect(confirmBtn.disabled).toBe(true);
    // Enter com o mesmo valor também não fecha em no-op
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(api.renameSession).not.toHaveBeenCalled();
    expect(document.querySelector('.confirm-card')).not.toBeNull();
    sidebarPin.setForced(null);
    unmount(t.comp);
  });

  it('RECOLHIDA: falha do renameSession mantém o diálogo aberto com erro role=alert ligado ao campo', async () => {
    comUmaSessao();
    vi.mocked(api.renameSession).mockRejectedValueOnce(new Error('tmux falhou'));
    const t = montar();
    await tick();
    sidebarPin.setForced(true);
    await tick();
    await abrirDialogoRename();
    await tick();
    const input = document.querySelector<HTMLInputElement>('input[aria-label="Novo nome da sessão"]')!;
    input.value = 'sess-novo';
    input.dispatchEvent(new Event('input'));
    await tick();
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick(); await tick();
    // Nada de sucesso falso: diálogo segue aberto, erro visível e associado ao campo
    expect(document.querySelector('.confirm-card')).not.toBeNull();
    const err = document.querySelector<HTMLElement>('#rename-dialog-err');
    expect(err?.innerText).toContain('tmux falhou');
    expect(err?.getAttribute('role')).toBe('alert');
    expect(input.getAttribute('aria-invalid')).toBe('true');
    expect(input.getAttribute('aria-describedby')).toContain('rename-dialog-err');
    // retry possível: Enter de novo dispara nova tentativa (o diálogo não travou)
    const retrySpy = vi.mocked(api.renameSession).mockResolvedValueOnce({ ok: true, name: 'sess-novo' });
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick(); await tick();
    expect(retrySpy).toHaveBeenCalledWith('sess-1', 'sess-novo');
    sidebarPin.setForced(null);
    unmount(t.comp);
  });

  it('RECOLHIDA no modo RAIL (padrão): aside monta o trilho original (iniciais, sem reconstrução) com rodapé e toggle do painel', async () => {
    comUmaSessao();
    navMode.mode = 'rail';
    const t = montar();
    await tick();
    sidebarPin.setForced(true);
    await tick();
    // A aside NÃO sai do DOM: vira o trilho (56px) com o desenho ORIGINAL (classe collapsed)
    const aside = document.querySelector<HTMLElement>('.sidebar');
    expect(aside).not.toBeNull();
    expect(aside?.classList.contains('collapsed')).toBe(true);
    expect(aside?.classList.contains('rail')).toBe(false);
    // Iniciais com anel de estado — nada da reconstrução da Task 5
    expect(aside?.querySelector('.initials')).not.toBeNull();
    expect(aside?.querySelector('.rail-iniciais')).toBeNull();
    expect(aside?.querySelector('.rail-state-dot')).toBeNull();
    // Rodapé sem o toggle do painel de contexto: ele vive só na barra superior (SessionTabs)
    expect(aside?.querySelector('.rail-ctx')).toBeNull();
    sidebarPin.setForced(null);
    unmount(t.comp);
  });

  it('RECOLHIDA: sucesso do rename pede foco na aba recriada (focusTab com a chave nova)', async () => {
    comUmaSessao();
    const focusSpy = vi.fn();
    const un = sidebarBridge.registerTabFocus({ focusTab: focusSpy });
    const t = montar();
    await tick();
    sidebarPin.setForced(true);
    await tick();
    await abrirDialogoRename();
    await tick();
    const input = document.querySelector<HTMLInputElement>('input[aria-label="Novo nome da sessão"]')!;
    input.value = 'sess-novo';
    input.dispatchEvent(new Event('input'));
    await tick();
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick(); await tick();
    // A aba antiga (keyed por nome) será destruída — o foco é delegado à aba recriada, quando o
    // modelo (SSE) refletir o novo nome. A cadeia completa (espera + foco conectado) está coberta
    // no SessionTabs.test.ts; aqui provamos a chamada com a chave certa.
    expect(focusSpy).toHaveBeenCalledWith('srv-a::sess-novo');
    un();
    sidebarPin.setForced(null);
    unmount(t.comp);
  });
});

describe('Sidebar — trilho original no modo rail', () => {
  // Preenche servers + byServer com 1 grupo por servidor, na ordem dos specs. O mock do store é
  // plain: os deriveds do Sidebar leem as arrays uma vez, no mount (mesmo padrão de comUmaSessao).
  function comStore(specs: Array<{ id: string; label: string; sessions: unknown[] }>) {
    storeState.servers.length = 0;
    storeState.byServer.length = 0;
    for (const sp of specs) {
      storeState.servers.push({ id: sp.id, label: sp.label, baseUrl: 'http://' + sp.id, token: 'x' });
      storeState.byServer.push({
        server: { id: sp.id, label: sp.label },
        sessions: sp.sessions,
        error: null, loaded: true,
      });
    }
  }
  const sess = (name: string, serverId: string, state: string, extra: Record<string, unknown> = {}) =>
    ({ name, serverId, state, ...extra });

  it('RAIL: não renderiza WorkspaceNav, filtro nem cabeçalho de grupo', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(true);   // pin recolhido -> trilho
    comStore([
      { id: 'srv-a', label: 'Servidor A', sessions: [1, 2, 3, 4].map((i) => sess(`sess-${i}`, 'srv-a', 'idle')) },
      { id: 'srv-b', label: 'Servidor B', sessions: [5, 6, 7, 8].map((i) => sess(`sess-${i}`, 'srv-b', 'idle')) },
    ]);
    const t = montar();
    await tick();
    expect(document.querySelector('.side-views')).toBeNull();
    expect(document.querySelector('.filter-input')).toBeNull();
    expect(document.querySelector('.grp-head-row')).toBeNull();
    unmount(t.comp);
  });

  it('RAIL: rodapé sem rótulos de texto', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(true);
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] }]);
    const t = montar();
    await tick();
    expect(document.querySelector('.fold-label')).toBeNull();
    expect(document.querySelector('.cta-new')!.textContent!.trim()).toBe('');
    unmount(t.comp);
  });

  it('RAIL: homônimas cross-server têm aria-label completo e distinto', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(true);
    comStore([
      { id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] },
      { id: 'srv-b', label: 'Servidor B', sessions: [sess('hangar', 'srv-b', 'idle')] },
    ]);
    const t = montar();
    await tick();
    const labels = [...document.querySelectorAll('.sess-main')].map((b) => b.getAttribute('aria-label'));
    expect(labels).toEqual([
      'hangar · Servidor A · pronto',
      'hangar · Servidor B · pronto',
    ]);
    unmount(t.comp);
  });

  it('RAIL: title repete nome, servidor e estado', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(true);
    comStore([
      { id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] },
      { id: 'srv-b', label: 'Servidor B', sessions: [sess('hangar', 'srv-b', 'idle')] },
    ]);
    const t = montar();
    await tick();
    expect(document.querySelector('.sess-main')!.getAttribute('title')).toBe('hangar · Servidor A · pronto');
    unmount(t.comp);
  });

  it('RAIL: estado em execução aparece no rótulo e na classe do anel', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(true);
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'working')] }]);
    const t = montar();
    await tick();
    expect(document.querySelector('.sess-main')!.getAttribute('aria-label')).toBe('hangar · Servidor A · em execução');
    // O estado saiu do anel das iniciais e virou marca própria no canto: trabalhando é a marca
    // animada, o resto é o ponto.
    expect(document.querySelector('.estado-marca')).not.toBeNull();
    expect(document.querySelector('.estado-ponto')).toBeNull();
    unmount(t.comp);
  });

  it('RAIL: sessão travada anuncia o aviso', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(true);
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle', { stalled: true })] }]);
    const t = montar();
    await tick();
    expect(document.querySelector('.sess-main')!.getAttribute('aria-label')).toBe('hangar · Servidor A · pode estar travada');
    const ponto = document.querySelector<HTMLElement>('.estado-ponto')!;
    expect(ponto).not.toBeNull();
    expect(ponto.getAttribute('style')).toContain('--warning');
    expect(document.querySelector('.estado-marca')).toBeNull();
    unmount(t.comp);
  });

  it('RAIL: não sobrou markup da reconstrução', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(true);
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] }]);
    const t = montar();
    await tick();
    expect(document.querySelector('.rail-iniciais')).toBeNull();
    expect(document.querySelector('.rail-state-dot')).toBeNull();
    unmount(t.comp);
  });

  it('EXPANDIDA: aria-label não é imposto e o nome visível continua', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(false);   // expandida
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] }]);
    const t = montar();
    await tick();
    expect(document.querySelector('.sess-main')!.getAttribute('aria-label')).toBeNull();
    expect(document.querySelector('.sess-name')!.textContent).toContain('hangar');
    unmount(t.comp);
  });

  it('EXPANDIDA: WorkspaceNav e filtro continuam presentes', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(false);
    comStore([
      { id: 'srv-a', label: 'Servidor A', sessions: [1, 2, 3, 4].map((i) => sess(`sess-${i}`, 'srv-a', 'idle')) },
      { id: 'srv-b', label: 'Servidor B', sessions: [5, 6, 7, 8].map((i) => sess(`sess-${i}`, 'srv-b', 'idle')) },
    ]);
    const t = montar();
    await tick();
    expect(document.querySelector('.side-views')).not.toBeNull();
    expect(document.querySelector('.filter-input')).not.toBeNull();
    unmount(t.comp);
  });

  it('TABS recolhido: a aside não é montada', async () => {
    navMode.mode = 'tabs';
    sidebarPin.setUser(true);
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] }]);
    const t = montar();
    await tick();
    expect(document.querySelector('.sidebar')).toBeNull();
    unmount(t.comp);
  });

  it('sem hover-expansion: mouseenter na aside não expande', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(true);
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] }]);
    const t = montar();
    await tick();
    document.querySelector('.sidebar')!.dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));
    await tick();
    expect(document.querySelector('.side-views')).toBeNull();
    unmount(t.comp);
  });

  it('fold sob override do Board/Canvas não muda preferred', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(false);   // preferência do usuário: expandida
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] }]);
    const t = montar();
    await tick();
    // override DEPOIS do mount (o $effect do mount limparia um override pré-montado)
    sidebarPin.setForced(true);
    await tick();
    const fold = document.querySelector<HTMLButtonElement>('.fold-btn')!;
    expect(fold.disabled).toBe(true);
    fold.click();
    await tick();
    expect(sidebarPin.preferred).toBe(false);
    expect(localStorage.getItem('cp_sidebar_collapsed')).not.toBe('1');
    sidebarPin.setForced(null);
    unmount(t.comp);
  });

  it('fold sem override continua alternando preferred', async () => {
    navMode.mode = 'rail';
    sidebarPin.setUser(false);
    sidebarPin.setForced(null);
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] }]);
    const t = montar();
    await tick();
    const fold = document.querySelector<HTMLButtonElement>('.fold-btn')!;
    expect(fold.disabled).toBe(false);
    fold.click();
    await tick();
    expect(sidebarPin.preferred).toBe(true);
    sidebarPin.setForced(null);
    unmount(t.comp);
  });



  it('fold sob override fica desabilitado no atributo', async () => {
    // sidebarPin.setUser(false) + setForced(true) = Quadro/Canvas forçando o recolhimento
    navMode.mode = 'rail';
    sidebarPin.setUser(false);   // pin do usuário: expandida
    comStore([{ id: 'srv-a', label: 'Servidor A', sessions: [sess('hangar', 'srv-a', 'idle')] }]);
    const t = montar();
    await tick();
    // override DEPOIS do mount (o $effect do mount limparia um override pré-montado)
    sidebarPin.setForced(true);
    await tick();
    expect(document.querySelector('.fold-btn:not(.rail-ctx)')!.hasAttribute('disabled')).toBe(true);
    sidebarPin.setForced(null);
    unmount(t.comp);
  });

});

// Task 15, item 1 + REGISTRADO 1 do parecer da Task 14 (arv-review19): das TRÊS expressões de
// filesInContext (Chat, Sidebar, SessionList), a da Sidebar era a única sem teste — e é onde o
// visor fantasma do Quadro nasceu. A fileira .gt-tab é o contrato do GitTabs (coberto lá com 7
// casos); aqui o alvo é a DERIVADA da Sidebar alimentando o Git REAL (que monta o GitTabs real).
// O visor só existe em desktop largo e com o painel de contexto visível.
function stubMatchMedia() {
  const estados = new Map<string, boolean>();
  const ouvintes: Array<{ query: string; on: () => void }> = [];
  window.matchMedia = ((query: string) => ({
    get matches() {
      return estados.get(query) ?? false;
    },
    addEventListener: (_tipo: string, on: () => void) => ouvintes.push({ query, on }),
    removeEventListener: () => {},
  })) as never;
  return {
    set(query: string, valor: boolean) {
      estados.set(query, valor);
      for (const o of ouvintes) if (o.query === query) o.on();
    },
  };
}

describe('Sidebar — filesInContext (Task 14/15): o Git do menu e a sessão hospedeira do visor', () => {
  let mq: ReturnType<typeof stubMatchMedia>;
  beforeEach(() => {
    mq = stubMatchMedia();
    mq.set('(min-width: 820px)', true);
    mq.set('(min-width: 1280px)', true);   // o visor só existe em desktop largo
  });

  function comSessao() {
    // Mock do store é plain: os deriveds leem as arrays uma vez, no mount (mesmo padrão
    // do comUmaSessao do describe de renomear — comStore/sess são escopados ao describe deles).
    // cwd presente de propósito: o item Git do SessionContextMenu só renderiza com `{#if cwd}`.
    storeState.servers.length = 0;
    storeState.servers.push({ id: 'srv-a', label: 'Servidor A', baseUrl: 'http://a', token: 'x' });
    storeState.byServer.length = 0;
    storeState.byServer.push({
      server: { id: 'srv-a', label: 'Servidor A' },
      sessions: [{ name: 'sess-1', serverId: 'srv-a', state: 'idle', cwd: '/repo/x' }],
      error: null, loaded: true,
    });
  }
  function montarCom(over: Record<string, unknown>) {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(Sidebar, {
      target: el,
      props: {
        currentSession: null,
        onSelect: vi.fn(),
        onCompare: vi.fn(),
        boardActive: false,
        canvasActive: false,
        orqActive: false,
        view: 'chat',
        onSelectView: vi.fn(),
        onOpenCommand: vi.fn(),
        onCollapsedChange: vi.fn(),
        ctxDisponivel: true,
        overlaySession: null,
        ...over,
      },
    });
    return { el, comp: comp as never };
  }
  async function abrirGitDaSessao() {
    sidebarBridge.openSessionMenu(
      new MouseEvent('contextmenu', { clientX: 5, clientY: 5 }),
      { name: 'sess-1', serverId: 'srv-a', cwd: '/repo/x' } as unknown as AggSession,
      'srv-a',
    );
    await tick();
    const gitBtn = [...document.querySelectorAll<HTMLButtonElement>('.ctx-menu button')]
      .find((b) => b.textContent?.trim().startsWith('Git') && !b.textContent?.includes('pull'));
    expect(gitBtn).not.toBeNull();
    gitBtn!.click();
    for (let i = 0; i < 5; i++) await tick();   // monta BottomSheet + GitTabs + load do store
  }
  // GIT_TABS avalia as labels no import do módulo (locale default do happy-dom = en), antes do
  // overwriteGetLocale — por isso o rótulo sai 'Files' e não 'Arquivos'. Mesmo padrão do
  // GitTabs.test.ts, que compara com 'Files'. O alvo aqui é a DERIVADA (presença da aba), não a
  // tradução — a paridade pt/en das mensagens é coberta pelos testes de i18n.
  const abas = () => [...document.querySelectorAll<HTMLElement>('.gt-tab')].map((b) => b.textContent?.trim() ?? '');
  const temArquivos = () => abas().includes('Files');
  it('painel de contexto aberto: o Git da MESMA sessão NÃO oferece a aba Arquivos', async () => {
    comSessao();
    const t = montarCom({ currentSession: 'sess-1' });
    await tick();
    await abrirGitDaSessao();
    expect(temArquivos()).toBe(false);
    expect(abas()).toEqual(expect.not.arrayContaining([expect.stringContaining('Arquivos')]));
    unmount(t.comp);
  });

  it('painel recolhido: a aba Arquivos VOLTA (o Chat não hospeda o visor)', async () => {
    comSessao();
    const t = montarCom({ currentSession: 'sess-1' });
    await tick();
    ctxPanel.recolhido = true;
    await tick();
    await abrirGitDaSessao();
    expect(temArquivos()).toBe(true);
    unmount(t.comp);
  });

  it('Git de OUTRA sessão (currentSession diferente): a aba Arquivos aparece', async () => {
    comSessao();
    const t = montarCom({ currentSession: 'outra' });
    await tick();
    await abrirGitDaSessao();
    expect(temArquivos()).toBe(true);
    unmount(t.comp);
  });

  it('QUADRO com overlay da MESMA sessão: sem a aba (o Chat do overlay hospeda o visor)', async () => {
    comSessao();
    const t = montarCom({ boardActive: true, canvasActive: false, view: 'board',
      overlaySession: { name: 'sess-1', serverId: 'srv-a' } });
    await tick();
    await abrirGitDaSessao();
    expect(temArquivos()).toBe(false);
    unmount(t.comp);
  });

  it('QUADRO com overlay de OUTRA sessão: com a aba (o overlay não hospeda esta sessão)', async () => {
    comSessao();
    const t = montarCom({ boardActive: true, canvasActive: false, view: 'board',
      overlaySession: { name: 'outra', serverId: 'srv-a' } });
    await tick();
    await abrirGitDaSessao();
    expect(temArquivos()).toBe(true);
    unmount(t.comp);
  });
});

describe('Sidebar — passagem de bastão: em que MODO a folha de criar abre', () => {
  // O defeito que isto trava: `bastaoAlvo` sobrevive à sessão em que foi escrito. Sem o
  // `abrirCriar()` zerando, o "+ Nova" clicado depois de uma passagem abriria a folha ainda em
  // modo bastão — criando, com o dossiê de outra sessão, algo que a pessoa pediu do zero.
  function comUmaSessao() {
    storeState.servers.length = 0;
    storeState.servers.push({ id: 'srv-a', label: 'Servidor A', baseUrl: 'http://a', token: 'x' });
    storeState.byServer.length = 0;
    storeState.byServer.push({
      server: { id: 'srv-a', label: 'Servidor A' },
      sessions: [{ name: 'sess-1', serverId: 'srv-a', state: 'idle', cwd: '/tmp/proj' }],
      error: null, loaded: true,
    });
  }
  const modo = () => document.querySelector('[data-testid="create-sheet"]')?.getAttribute('data-bastao');

  async function abrirMenuEPassarBastao() {
    sidebarBridge.openSessionMenu(
      new MouseEvent('contextmenu', { clientX: 5, clientY: 5 }),
      { name: 'sess-1', serverId: 'srv-a', cwd: '/tmp/proj' } as unknown as AggSession,
      'srv-a',
    );
    await tick();
    const item = [...document.querySelectorAll<HTMLButtonElement>('.ctx-menu button')]
      // `includes` e não igualdade: o item carrega o chevron "›" (abre outra superfície), igual
      // aos de Git e Loop — o texto do botão é rótulo + chevron.
      .find((b) => b.textContent?.includes(m.bastao_menu()));
    expect(item, 'item "continuar em outra conta" no menu "⋯"').not.toBeUndefined();
    item!.click();
    await tick();
  }

  it('o item do menu abre em modo bastão; o "+ Nova" seguinte volta ao normal', async () => {
    comUmaSessao();
    const t = montar();
    await tick();
    expect(modo()).toBe('');                       // nasce em modo normal

    await abrirMenuEPassarBastao();
    expect(modo()).toBe('sess-1');                 // a folha recebeu a origem

    // "+ Nova" do rodapé: o MESMO caminho que a pessoa usa depois de passar o bastão.
    document.querySelector<HTMLButtonElement>('.cta-new')!.click();
    await tick();
    expect(modo()).toBe('');
    unmount(t.comp);
  });

  it('o "+ Nova" da barra de abas (bridge) também zera o modo', async () => {
    comUmaSessao();
    const t = montar();
    await tick();
    await abrirMenuEPassarBastao();
    expect(modo()).toBe('sess-1');
    sidebarBridge.openCreate();                    // caminho das abas, com a sidebar recolhida
    await tick();
    expect(modo()).toBe('');
    unmount(t.comp);
  });
});
