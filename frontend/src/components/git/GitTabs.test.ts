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
import { readFile } from '../../lib/api';
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

function montar(desktop = false, filesInContext = false) {
  const git = createGitStore('sess');
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(GitTabs, { target: el, props: { git, desktop, filesInContext, onClose: vi.fn() } });
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

  // Parecer rodada 2, B1: com um painel de contexto VISIVEL o Git desktop nao oferece a aba
  // Arquivos (o DesktopSessionContext ja a hospeda) — a fileira e filtrada e o FilesPanel
  // mobile nunca monta no modal.
  it('desktop com contexto visivel: sem a aba Arquivos e sem o FilesPanel', async () => {
    const { el, comp } = montar(true, true);
    await tick();
    await tick();
    const abas = [...el.querySelectorAll('[role=tab]')].map((t) => t.textContent);
    expect(abas.some((t) => t === 'Files')).toBe(false);
    expect(abas).toHaveLength(3);
    expect(el.querySelector('.files-panel')).toBeNull();
    unmount(comp);
  });

  // Parecer rodada 3, B1: SEM painel de contexto visivel (Sidebar, split, desktop estreito
  // 820-1279px) o Git desktop PRECISA oferecer a aba — e o unico host possivel.
  it('desktop sem contexto visivel: a aba Arquivos existe e monta o painel', async () => {
    const { el, comp } = montar(true, false);
    await tick();
    await tick();
    const abas = [...el.querySelectorAll('[role=tab]')].map((t) => t.textContent);
    expect(abas.some((t) => t === 'Files')).toBe(true);
    expect(abas).toHaveLength(4);
    clica([...el.querySelectorAll('[role=tab]')].find((t) => t.textContent === 'Files'));
    await tick();
    await tick();
    expect(el.querySelector('.files-panel')).not.toBeNull();
    unmount(comp);
  });

  // Parecer rodada 3, B2: ativacao por toque/AT deixa o foco no body — o abridor so conta se
  // for linha de arquivo; o fechamento devolve o foco a linha por data-path e nunca ao body.
  it('abrir com o foco no body e fechar devolve a linha que abriu', async () => {
    const { el, comp } = montar();
    await tick();
    await tick();
    clica([...el.querySelectorAll('[role=tab]')].find((t) => t.textContent === 'Files'));
    await tick();
    await tick();
    document.body.focus();   // toque/AT: o DOM focado no body, nao na linha
    expect(document.activeElement).toBe(document.body);
    const linha = el.querySelector<HTMLElement>('.files-panel .arvore .no');
    clica(linha);   // ativa a linha (Enter/toque) com o body focado
    await tick();
    await tick();
    await tick();
    expect(el.querySelector('.visor')).not.toBeNull();
    expect(document.activeElement).not.toBe(document.body);   // foco entrou no visor
    clica(el.querySelector('.visor .fechar'));
    await tick();
    await tick();
    await tick();
    expect(el.querySelector('.files-panel')).not.toBeNull();
    const linhaVolta = [...el.querySelectorAll<HTMLElement>('.files-panel .arvore .no')]
      .find((n) => n.dataset.path === 'a.txt');
    expect(document.activeElement).toBe(linhaVolta);   // linha restaurada, nunca o body
    unmount(comp);
  });

  // Parecer rodada 2, B4: dentro do modal Git nao ha conversa atras — o rotulo do voltar nao
  // pode dizer "voltar a conversa" (o default do FileViewer fica para o host desktop do Chat).
  it('mobile: o rotulo do voltar nao menciona conversa', async () => {
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
    const texto = el.querySelector('.visor .voltar')?.textContent ?? '';
    expect(/conversa|conversation/i.test(texto)).toBe(false);
    expect(texto).toContain('Voltar');   // m.comum_voltar em pt (overwriteGetLocale)
    unmount(comp);
  });

  // Parecer rodada 3, B3: leitura que falha (binario 415) NAO desmonta o visor para a arvore
  // silenciosamente normal — o erro do store aparece no visor, anunciado, e o usuario pode
  // fechar sabendo qual arquivo falhou.
  it('readFile rejeitado (415) mantem o visor com o erro anunciado', async () => {
    const { el, comp } = montar();
    await tick();
    await tick();
    clica([...el.querySelectorAll('[role=tab]')].find((t) => t.textContent === 'Files'));
    await tick();
    await tick();
    vi.mocked(readFile).mockRejectedValueOnce(
      Object.assign(new Error('arquivo binario'), { status: 415 }));
    clica(el.querySelector('.files-panel .arvore .no'));
    await tick();
    await tick();
    await tick();
    const visor = el.querySelector('.visor');
    expect(visor).not.toBeNull();   // o visor fica com o arquivo que falhou
    const alerta = visor?.querySelector('[role="alert"]');
    expect(alerta).not.toBeNull();
    expect(alerta?.textContent).toContain('arquivo binario');
    clica(visor?.querySelector('.fechar'));
    await tick();
    await tick();
    await tick();
    expect(el.querySelector('.files-panel')).not.toBeNull();
    expect(el.querySelector('.visor')).toBeNull();
    unmount(comp);
  });

  // variante 404 (linha fantasma): o mesmo contrato — erro visivel e saida possivel.
  it('readFile rejeitado (404) mantem o visor com o erro anunciado', async () => {
    const { el, comp } = montar();
    await tick();
    await tick();
    clica([...el.querySelectorAll('[role=tab]')].find((t) => t.textContent === 'Files'));
    await tick();
    await tick();
    vi.mocked(readFile).mockRejectedValueOnce(
      Object.assign(new Error('sumiu'), { status: 404 }));
    clica(el.querySelector('.files-panel .arvore .no'));
    for (let i = 0; i < 6; i++) await tick();   // o ramo 404 roda recarregar + _listar antes do erro
    const visor = el.querySelector('.visor');
    expect(visor).not.toBeNull();
    const alerta = visor?.querySelector('[role="alert"]');
    expect(alerta).not.toBeNull();
    expect(alerta?.textContent).toContain('não existe mais');   // m.erro_arq_inexistente em pt
    unmount(comp);
  });

  // Parecer rodada 2, B3: o botao "mostrar tudo" do aviso de filtro tem alvo de toque no
  // celular — a regra compacta do mock (17px) nao pode valer na hospedagem mobile. O CSS do
  // componente nao e computado no happy-dom (nem injetado no <style> — medido), entao o teste
  // prova a ESTRUTURA (botao presente, classe mobile aplicada) e o RETANGULO e medido ao vivo
  // no navegador (mesmo metodo do revisor; colado no relato).
  it('mobile: o botao mostrar tudo existe na hospedagem mobile', async () => {
    const { el, comp } = montar();
    await tick();
    await tick();
    clica([...el.querySelectorAll('[role=tab]')].find((t) => t.textContent === 'Files'));
    await tick();
    await tick();
    const painel = el.querySelector('.files-panel');
    expect(painel?.classList.contains('mobile')).toBe(true);
    const botao = el.querySelector<HTMLElement>('.files-panel .filtro-aviso button');
    expect(botao).not.toBeNull();   // o filtro vem ligado (padrao) e o aviso aparece
    unmount(comp);
  });
});
