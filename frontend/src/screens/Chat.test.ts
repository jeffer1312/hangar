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
import { filesStores } from '../lib/filesStore.svelte';
import { ctxPanel } from '../lib/ctxPanel.svelte';
import { overwriteGetLocale } from '../paraglide/runtime';

// Stub de componente Svelte 5: createRawSnippet é o padrão do DesktopShell.test.ts — uma classe
// com $destroy (padrão Svelte 4) quebra no mount do Svelte 5 com "cannot be invoked without new".
function stubDe() { return { default: createRawSnippet(() => ({ render: () => '<div />' })) }; }
// O FileViewer vira um stub com o botão .fechar REAL: é ele que o Chat foca ao abrir o visor.
function stubVisor() {
  return { default: createRawSnippet(() => ({ render: () => '<div class="visor" role="region" tabindex="-1"><button class="fechar">×</button></div>' })) };
}

vi.mock('../components/NavBar.svelte', stubDe);
vi.mock('../components/MessageList.svelte', stubDe);
vi.mock('../components/Composer.svelte', stubDe);
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

// API: só o que o mount do Chat toca precisa responder; o resto nunca chega a ser chamado
// com os filhos stubados.
vi.mock('../lib/api', () => ({
  getHistory: vi.fn(async () => ({ events: [], total: 0 })),
  openEventStream: vi.fn(() => ({
    onmessage: () => {}, onerror: () => {}, close: () => {},
    readyState: 0, addEventListener: () => {},
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
vi.mock('../lib/loop', () => ({ loopBadge: vi.fn(() => null), LOOP_TONE_COLOR: {} }));
vi.mock('../lib/statusline', () => ({ parseStatusLine: vi.fn(() => null) }));
vi.mock('../lib/history', () => ({ appendTail: vi.fn(), hasSeam: vi.fn(), prependOlder: vi.fn() }));
vi.mock('../lib/activity', () => ({
  createActivityFolder: vi.fn(() => ({
    snapshot: () => ({ tasks: [], inProgress: 0, running: 0, agents: [], writeEvents: [] }),
    push: () => {}, save: () => {}, attach: () => {},
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
});

describe('Chat — visor de arquivo (Task 11, B5: foco e inert)', () => {
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
});
