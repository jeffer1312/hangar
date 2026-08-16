// @vitest-environment happy-dom
// Task 12: o contrato da aba Arquivos no celular — a fileira ganha a aba, o nivel 0 hospeda o
// FilesPanel (desktop=false), o clique no arquivo sobe o nivel (o arquivo vira o degrau do
// drill-down) e fechar/trocar de aba limpa a selecao. As abas irma sao stub: o contrato testado
// e do GitTabs; o FilesPanel e o FileViewer reais (com a API mockada) exercitam o fluxo de ponta.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import { createRawSnippet } from 'svelte';
import GitTabs from './GitTabs.svelte';
import { createGitStore } from '../../lib/gitStore.svelte';
import { filesStores } from '../../lib/filesStore.svelte';
import { overwriteGetLocale } from '../../paraglide/runtime';

vi.mock('../../lib/api', () => ({
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
}));

vi.mock('../../lib/auth', () => ({
  listServers: vi.fn(() => []),
  getActiveId: vi.fn(() => 'srv-test'),
}));

// Stub de componente Svelte 5: mesmo padrao do Chat.test.ts/DesktopShell.test.ts.
function stubDe() { return { default: createRawSnippet(() => ({ render: () => '<div />' })) }; }
vi.mock('./GitChangesTab.svelte', stubDe);
vi.mock('./GitHistoryTab.svelte', stubDe);
vi.mock('./GitBranchesTab.svelte', stubDe);
vi.mock('./GitStatusBar.svelte', stubDe);
vi.mock('./RepoMenu.svelte', stubDe);

function montar() {
  const git = createGitStore('sess');
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(GitTabs, { target: el, props: { git, desktop: false, onClose: vi.fn() } });
  return { el, comp: comp as never };
}

const clica = (alvo: Element | null | undefined, evento = 'click') => {
  alvo?.dispatchEvent(new MouseEvent(evento, { bubbles: true }));
};

beforeEach(() => {
  overwriteGetLocale(() => 'pt');
  document.body.innerHTML = '';
  // o registry do FilesStore persiste no modulo entre testes: zera a selecao da chave usada
  const s = filesStores.retain('srv-test::sess', 'sess');
  s.selecionado = null;
  filesStores.release('srv-test::sess');
});

describe('GitTabs — aba Arquivos no celular (Task 12)', () => {
  it('aba files: arvore no nivel 0, clique no arquivo sobe o nivel e fechar volta', async () => {
    const { el, comp } = montar();
    await tick();
    await tick();
    // a fileira tem as 4 abas, com Arquivos entre Mudancas e Historico. O GIT_TABS resolve os
    // rotulos na importacao: o teste os ve no idioma inicial (ingles, mesmo padrao do
    // gitTabs.test.ts)
    const abas = [...el.querySelectorAll('[role=tab]')].map((t) => t.textContent);
    expect(abas[0]?.startsWith('Changes')).toBe(true);
    expect(abas[1]).toBe('Files');
    expect(abas[2]?.startsWith('History')).toBe(true);

    clica([...el.querySelectorAll('[role=tab]')].find((t) => t.textContent === 'Files'));
    await tick();
    await tick();
    const painel = el.querySelector('.files-panel');
    expect(painel).not.toBeNull();
    expect(painel?.classList.contains('mobile')).toBe(true);   // desktop=false no celular

    // clique no arquivo da arvore -> o nivel sobe e o FileViewer substitui o painel
    clica(el.querySelector('.files-panel .arvore .no'));
    await tick();
    await tick();
    await tick();
    expect(el.querySelector('.files-panel')).toBeNull();
    const visor = el.querySelector('.visor');
    expect(visor).not.toBeNull();

    // fechar pelo proprio FileViewer (o ×) -> volta a arvore
    clica(visor?.querySelector('.fechar'));
    await tick();
    await tick();
    expect(el.querySelector('.files-panel')).not.toBeNull();
    expect(el.querySelector('.visor')).toBeNull();
    unmount(comp);
  });

  it('aba files: trocar de aba com arquivo aberto limpa a selecao e o nivel', async () => {
    const { el, comp } = montar();
    await tick();
    await tick();
    clica([...el.querySelectorAll('[role=tab]')].find((t) => t.textContent === 'Files'));
    await tick();
    await tick();
    clica(el.querySelector('.files-panel .arvore .no'));
    await tick();
    await tick();
    await tick();
    expect(el.querySelector('.visor')).not.toBeNull();

    // troca para Mudancas com o arquivo aberto: a selecao some (senao a aba voltaria com o
    // arquivo herdado — a mesma regra do diff aberto que a Task 3 pagou)
    clica([...el.querySelectorAll('[role=tab]')].find((t) => t.textContent?.startsWith('Changes')));
    await tick();
    await tick();
    const store = filesStores.retain('srv-test::sess', 'sess');
    expect(store.selecionado).toBeNull();

    // volta para Arquivos: arvore no nivel 0, nao o arquivo herdado
    clica([...el.querySelectorAll('[role=tab]')].find((t) => t.textContent === 'Files'));
    await tick();
    await tick();
    expect(el.querySelector('.files-panel')).not.toBeNull();
    expect(el.querySelector('.visor')).toBeNull();
    unmount(comp);
  });
});
