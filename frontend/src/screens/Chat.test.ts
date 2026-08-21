// @vitest-environment happy-dom
// Parecer Task 11, B5: com o visor de arquivo aberto, o underlay da conversa fica `inert`
// (Tab não alcança mensagens/composer escondidos sob o arquivo) e o foco tem dono: entra no
// Fechar do visor e volta ao elemento que abriu quando fecha. Montar o Chat inteiro pediria
// dezenas de módulos reais (SSE, composer, sheets); os filhos pesados viram stub vazio e o
// teste cobre o CONTRATO do Chat (underlay + foco), não o conteúdo deles.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import { createRawSnippet } from 'svelte';
import Chat from './Chat.svelte';
import { chamadasFocus } from './ComposerStub.svelte';
import { filesStores } from '../lib/filesStore.svelte';
import { ctxPanel } from '../lib/ctxPanel.svelte';
import { overwriteGetLocale } from '../paraglide/runtime';
import GitTabs from '../components/git/GitTabs.svelte';
import { createGitStore } from '../lib/gitStore.svelte';

// Stub de componente Svelte 5: createRawSnippet é o padrão do DesktopShell.test.ts — uma classe
// com $destroy (padrão Svelte 4) quebra no mount do Svelte 5 com "cannot be invoked without new".
function stubDe() { return { default: createRawSnippet(() => ({ render: () => '<div />' })) }; }
// O FileViewer vira um stub com o botão .fechar REAL: é ele que o Chat foca ao abrir o visor.
function stubVisor() {
  return { default: createRawSnippet(() => ({ render: () => '<div class="visor" role="region" tabindex="-1"><button class="fechar">×</button></div>' })) };
}

vi.mock('../components/NavBar.svelte', stubDe);
vi.mock('../components/MessageList.svelte', stubDe);
vi.mock('../components/Composer.svelte', async () => ({
  default: (await import('./ComposerStub.svelte')).default,
}));
vi.mock('../components/SessionSwitcherSheet.svelte', stubDe);
vi.mock('../components/CreateSessionSheet.svelte', stubDe);
vi.mock('../components/UsageSheet.svelte', stubDe);
vi.mock('../components/Git.svelte', stubDe);
vi.mock('../components/PreviewSheet.svelte', stubDe);
vi.mock('../components/ActivitySheet.svelte', stubDe);
vi.mock('../components/TerminalMirror.svelte', stubDe);
vi.mock('../components/AskQuestionSheet.svelte', stubDe);
vi.mock('../components/RunSheet.svelte', stubDe);
vi.mock('../components/MoreSheet.svelte', stubDe);
vi.mock('../components/AttachmentsSheet.svelte', stubDe);
vi.mock('../components/CodexLimitsSheet.svelte', stubDe);
vi.mock('../components/ForwardSheet.svelte', stubDe);
vi.mock('../components/PairSheet.svelte', stubDe);
vi.mock('../components/PairChatModal.svelte', stubDe);
vi.mock('../components/LoopSheet.svelte', stubDe);
vi.mock('../components/DesktopSessionContext.svelte', stubDe);
vi.mock('../components/files/FileViewer.svelte', stubVisor);

// Controle do SSE mockado: o Chat conecta via openEventStream().addEventListener('state', ...)
// — para montar uma sessao morta (B5 rodada 4) o teste precisa disparar o evento na mao.
const sseCtl = vi.hoisted(() => {
  const handlers = new Map<string, (e: MessageEvent) => void>();
  return {
    handlers,
    state: (dados: unknown) => {
      const h = handlers.get('state');
      if (h) h({ data: JSON.stringify(dados) } as MessageEvent);
    },
  };
});

// API: só o que o mount do Chat toca precisa responder; o resto nunca chega a ser chamado
// com os filhos stubados.
vi.mock('../lib/api', () => ({
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  isTimeoutError: vi.fn(() => false),
  errorDetail: vi.fn(async () => ''),
  getHistory: vi.fn(async () => []),
  openEventStream: vi.fn(() => ({
    onmessage: () => {}, onerror: () => {}, close: () => {},
    readyState: 0,
    addEventListener: (tipo: string, h: (e: MessageEvent) => void) => { sseCtl.handlers.set(tipo, h); },
  })),
  // o teste integrado do B1 (rodada 5) monta o GitTabs real: as funcoes do FilesPanel
  listFiles: vi.fn(async () => ({
    entries: [{ name: 'a.txt', path: 'a.txt', is_dir: false, size: 1, changed: null, add: 0, del: 0 }],
    truncated: false,
  })),
  readFile: vi.fn(async () => ({ path: 'a.txt', text: 'A', size: 1, truncated: false })),
  searchFiles: vi.fn(async () => ({ hits: [], truncated: false, mode: 'names' })),
  pathDiff: vi.fn(async () => ({
    path: 'a.txt', diff: '', truncated: false,
    escopo_pedido: 'branch', escopo_usado: 'branch', base: null, motivo: null,
  })),
  getSessions: vi.fn(async () => []),
  getRunners: vi.fn(async () => ({ running: false })),
  getPlan: vi.fn(async () => null),
  getWorkflows: vi.fn(async () => []),
  sendInput: vi.fn(),
  steerSession: vi.fn(),
  broadcast: vi.fn(),
  selectOption: vi.fn(),
  interrupt: vi.fn(),
  createSession: vi.fn(),
  answerQuestions: vi.fn(),
  isAbortError: vi.fn(() => false),
}));
vi.mock('../lib/auth', () => ({
  listServers: vi.fn(() => [{ id: 'srv-test', label: 'T', baseUrl: 'http://x', token: 't' }]),
  getActiveId: vi.fn(() => 'srv-test'),
}));
vi.mock('../lib/ttsPlayer.svelte', () => ({ ttsPlayer: { active: false, loading: false } }));
vi.mock('../lib/ouvir', () => ({ ouvirTexto: vi.fn() }));
vi.mock('../lib/speakable', () => ({ textoFalavelComCodigo: vi.fn(() => '') }));
vi.mock('@hangar/core', async (importOriginal) => ({ ...(await importOriginal<typeof import('@hangar/core')>()), loopBadge: vi.fn(() => null), LOOP_TONE_COLOR: {}, appendTail: vi.fn(), hasSeam: vi.fn(), prependOlder: vi.fn() }));
vi.mock('../lib/statusline', () => ({ parseStatusLine: vi.fn(() => null) }));
vi.mock('../lib/activity', () => ({
  createActivityFolder: vi.fn(() => ({
    snapshot: () => ({ tasks: [], inProgress: 0, running: 0, agents: [], writeEvents: [] }),
    push: () => {}, save: () => {}, attach: () => {}, reset: () => {},
  })),
}));
vi.mock('../lib/workspaceCommands', () => ({}) );
vi.mock('../lib/errosApi', () => ({ formataErro: vi.fn(() => '') }));

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(Chat, {
    target: el,
    props: {
      sessionName: 'sess',
      onBack: vi.fn(),
      onNavigateToChat: vi.fn(),
      desktop: true,
      showContextPanel: true,
    },
  });
  return { el, comp: comp as never };
}

beforeEach(() => {
  overwriteGetLocale(() => 'pt');
  ctxPanel.recolhido = false;   // vive no módulo e persiste entre testes
  ctxPanel.aba = 'contexto';
  document.body.innerHTML = '';
  // o registry do FilesStore vive no modulo e persiste entre testes: zera a selecao da chave
  // usada (o GitTabs e o Chat compartilham serverId::sessionName nos testes integrados)
  const s = filesStores.retain('srv-test::sess', 'sess');
  s.selecionado = null;
  filesStores.release('srv-test::sess');
});

// Stub do matchMedia com ouvintes controlados na mao (ver nota no describe do visor).
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

describe('Chat — visor de arquivo (Task 11, B5: foco e inert)', () => {
  // O happy-dom 20 NAO notifica listeners de matchMedia criados depois do primeiro setViewport
  // (medido 16/08/2026: o change nao dispara nesse fluxo) — o resize do B5 precisa de um stub
  // que dispare os ouvintes na mao. No navegador real o evento existe; a prova viva cobre.
  let mq: ReturnType<typeof stubMatchMedia>;
  beforeEach(() => {
    mq = stubMatchMedia();
    mq.set('(min-width: 768px)', true);
    mq.set('(min-width: 1280px)', true);   // o visor so existe em desktop largo (B5)
  });

  it('underlay da conversa fica inert com o visor aberto e volta sem ele', async () => {
    const t = montar();
    await tick();
    await tick();
    const store = filesStores.retain('srv-test::sess', 'sess');
    expect(store.selecionado).toBeNull();
    const underlay = t.el.querySelector<HTMLElement>('.chat-underlay');
    expect(underlay).not.toBeNull();
    expect(underlay?.hasAttribute('inert')).toBe(false);
    store.selecionado = 'a.ts';
    await tick();
    await tick();
    expect(underlay?.hasAttribute('inert')).toBe(true);
    store.selecionado = null;
    await tick();
    await tick();
    expect(underlay?.hasAttribute('inert')).toBe(false);
    unmount(t.comp);
  });

  it('abrir move o foco pro Fechar do visor; fechar devolve ao elemento que abriu', async () => {
    const t = montar();
    await tick();
    await tick();
    const store = filesStores.retain('srv-test::sess', 'sess');
    // o "elemento que abriu": um botão da árvore com foco (o clique do FileTree deixa o foco nele)
    const origem = document.createElement('button');
    origem.className = 'no';
    t.el.appendChild(origem);
    origem.focus();
    expect(document.activeElement).toBe(origem);
    store.selecionado = 'a.ts';
    await tick();
    await tick();
    const fechar = t.el.querySelector<HTMLElement>('.arq-visor .fechar');
    expect(fechar).not.toBeNull();
    expect(document.activeElement).toBe(fechar);   // foco entrou no visor
    // Fecha pelo Esc (onGlobalKey do Chat — o mesmo fecharVisor do × e do "voltar")
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await tick();
    await tick();
    expect(document.activeElement).toBe(origem);   // foco voltou pra quem abriu
    unmount(t.comp);
  });

  // Parecer rodada 3, B5: encolher para <1280px com o visor aberto nao pode deixar a conversa
  // inerte com o arquivo invisivel — a selecao e limpa, o inert sai e o foco volta a um
  // controle vivo. Voltar a 1440px NAO remonta o visor sozinho.
  it('resize para desktop estreito limpa o visor e libera a conversa', async () => {
    const t = montar();
    await tick();
    await tick();
    const store = filesStores.retain('srv-test::sess', 'sess');
    store.selecionado = 'a.ts';
    await tick();
    await tick();
    const underlay = t.el.querySelector<HTMLElement>('.chat-underlay');
    expect(underlay?.hasAttribute('inert')).toBe(true);   // largo: visor abre e inerta a conversa
    mq.set('(min-width: 1280px)', false);   // encolheu p/ desktop estreito
    await tick();
    await tick();
    expect(store.selecionado).toBeNull();                  // selecao limpa
    expect(underlay?.hasAttribute('inert')).toBe(false);   // conversa volta operavel
    expect(t.el.querySelector('.arq-visor')).toBeNull();   // visor desmontado
    // foco devolvido ao Composer (o stub foca o proprio botao, que representa o textarea)
    expect(document.activeElement).toBe(t.el.querySelector('.composer-stub'));
    mq.set('(min-width: 1280px)', true);   // volta a largo
    await tick();
    await tick();
    expect(t.el.querySelector('.arq-visor')).toBeNull();   // so monta com nova selecao
    unmount(t.comp);
  });

  // Parecer rodada 4, B5: sessao morta nao tem Composer (o bottom-dock vira o .dead-footer com
  // o botao voltar) — o resize tem que devolver o foco a um controle que o usuario VE.
  it('resize com sessao morta devolve o foco ao botao voltar', async () => {
    const t = montar();
    await tick();
    await tick();
    await import('../lib/api');   // estabiliza o mock do SSE: sem o import explicito, o registro
    // dos handlers pode nao estar visivel ao sseCtl na suite completa (medido: falha intermitente).
    sseCtl.state({ state: 'dead' });   // a sessao encerrou
    await tick();
    await tick();
    const voltar = t.el.querySelector<HTMLElement>('.dead-footer .back-btn');
    expect(voltar).not.toBeNull();     // footer morto no lugar do composer
    const store = filesStores.retain('srv-test::sess', 'sess');
    store.selecionado = 'a.ts';
    await tick();
    await tick();
    expect(t.el.querySelector('.arq-visor')).not.toBeNull();   // visor abre mesmo assim
    mq.set('(min-width: 1280px)', false);   // encolheu p/ desktop estreito
    await tick();
    await tick();
    expect(store.selecionado).toBeNull();
    expect(document.activeElement).toBe(voltar);   // foco no botao vivo, nao no body
    unmount(t.comp);
  });

  // Parecer rodada 4, B2: janela JA estreita (1024px) — a selecao escrita por OUTRO host (o
  // Git do desktop estreito compartilha o MESMO FilesStore) nao pode ser limpa pelo guard de
  // resize do Chat: sem o guard de transicao, o drill-down do Git morreria no mesmo flush.
  it('janela ja estreita: selecao escrita por outro host nao e limpa', async () => {
    mq.set('(min-width: 1280px)', false);   // monta direto em desktop estreito
    const t = montar();
    await tick();
    await tick();
    const store = filesStores.retain('srv-test::sess', 'sess');
    store.selecionado = 'a.ts';   // escrito pelo GitTabs (mesma chave serverId::sessionName)
    await tick();
    await tick();
    expect(store.selecionado).toBe('a.ts');   // nada limpou no meio
    unmount(t.comp);
  });

  // Parecer rodada 5, B1: Git aberto com contexto oculto + resize largo->estreito — o Chat
  // compartilha o MESMO FilesStore e nao pode roubar a selecao nem o foco do modal (o visor do
  // Git sobrevive por abridorPath, e o foco tem que ficar DENTRO do .sheet, nao no Composer
  // atras).
  it('resize com Git aberto e contexto oculto nao rouba selecao nem foco do modal', async () => {
    ctxPanel.recolhido = true;   // contexto oculto: filesInContext false, o Git tem a aba
    const t = montar();          // Chat a 1440 (beforeEach liga o breakpoint largo)
    await tick();
    await tick();
    // GitTabs na MESMA chave do Chat, como a Sidebar (filesInContext=false)
    const git = createGitStore('sess');
    const gEl = document.createElement('div');
    document.body.appendChild(gEl);
    const gComp = mount(GitTabs, {
      target: gEl,
      props: { git, desktop: true, filesInContext: false, onClose: vi.fn() },
    });
    await tick();
    await tick();
    [...gEl.querySelectorAll('[role=tab]')].find((x) => x.textContent === 'Files')
      ?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await tick();
    await tick();
    await vi.waitFor(() => expect(gEl.querySelector('.files-panel .arvore .no')).not.toBeNull());
    (gEl.querySelector('.files-panel .arvore .no') as HTMLElement | null)
      ?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await tick();
    await tick();
    await tick();
    expect(gEl.querySelector('.visor')).not.toBeNull();   // visor do Git aberto
    // resize para 1024 (transicao largo->estreito)
    mq.set('(min-width: 1280px)', false);
    await tick();
    await tick();
    await tick();
    const store = filesStores.retain('srv-test::sess', 'sess');
    expect(store.selecionado).toBe('a.txt');              // o Chat nao limpou a selecao do Git
    expect(gEl.querySelector('.visor')).not.toBeNull();   // o visor continua
    expect(gEl.contains(document.activeElement)).toBe(true);   // foco dentro do modal
    unmount(gComp as never);
    unmount(t.comp);
  });

  // Parecer rodada 6, B1: visor do Chat + Git aberto POR CIMA + resize — a limpeza da selecao
  // continua (o visor do Chat fica sob display:none e precisa sair), mas o foco NAO pode viajar
  // para o Composer atras do modal aberto (role=dialog do BottomSheet). O Git e o REAL via
  // vi.importActual — o mock do arquivo e um stub sem o BottomSheet.
  it('resize com modal aberto por cima nao joga o foco para o Composer atras', async () => {
    const { default: GitReal } = await vi.importActual<typeof import('../components/Git.svelte')>('../components/Git.svelte');
    const t = montar();   // Chat a 1440, contexto visivel (beforeEach)
    await tick();
    await tick();
    const store = filesStores.retain('srv-test::sess', 'sess');
    store.selecionado = 'a.ts';   // o visor do Chat abre
    await tick();
    await tick();
    expect(t.el.querySelector('.arq-visor')).not.toBeNull();
    // o Git real (que traz o BottomSheet com role=dialog) aberto por cima
    const git = createGitStore('sess');
    const gEl = document.createElement('div');
    document.body.appendChild(gEl);
    const gComp = mount(GitReal, {
      target: gEl,
      props: { open: true, sessionName: 'sess', desktop: true, filesInContext: true, onClose: vi.fn() },
    });
    await tick();
    await tick();
    expect(document.querySelector('[role="dialog"]:not(.board-overlay)')).not.toBeNull();
    // resize para 1024: a limpeza do visor do Chat roda, mas o foco nao sai para o composer
    mq.set('(min-width: 1280px)', false);
    await tick();
    await tick();
    await tick();
    expect(store.selecionado).toBeNull();                        // a limpeza continua
    expect(t.el.querySelector('.arq-visor')).toBeNull();         // o visor do Chat desmontou
    const underlay = t.el.querySelector<HTMLElement>('.chat-underlay');
    expect(underlay?.hasAttribute('inert')).toBe(false);         // o inert saiu
    expect(document.querySelector('[role="dialog"]:not(.board-overlay)')).not.toBeNull();
    expect(document.activeElement).not.toBe(t.el.querySelector('.composer-stub'));   // foco nao viajou
    unmount(gComp as never);
    unmount(t.comp);
  });
});
