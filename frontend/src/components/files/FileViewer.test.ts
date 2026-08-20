// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount } from 'svelte';
import FileViewer from './FileViewer.svelte';
import * as m from '../../paraglide/messages';
import { overwriteGetLocale } from '../../paraglide/runtime';

// Todos os props obrigatórios entram no base como null/vi.fn() (padrão do BottomSheet.test):
// o spread de `props` só adiciona, e o svelte-check fecha o merge sem cast.
const base = { path: 'a.py', diff: null, conteudo: null, loading: false, onEscopo: vi.fn(), onFechar: vi.fn() };

function montar(props: Record<string, unknown>) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return { el, comp: mount(FileViewer, { target: el, props: { ...base, ...props } }) };
}

describe('FileViewer', () => {
  beforeEach(() => overwriteGetLocale(() => 'pt'));

  it('sem mudanca, monta o editor com o arquivo', () => {
    // O conteúdo agora é desenhado pelo CodeMirror (import dinâmico), então o que este nível
    // decide — e o que cabe afirmar aqui — é montar a caixa do editor em vez do <pre>.
    const { el, comp } = montar({
      diff: null,
      conteudo: { path: 'a.py', text: 'print(1)\n', size: 9, truncated: false, digest: 'abc' },
    });
    expect(el.querySelector('.editor')).not.toBeNull();
    unmount(comp);
  });

  it('arquivo cortado avisa, e ainda assim mostra o editor', () => {
    // Numeração e dobra passaram a ser do CodeMirror. O que continua sendo decisão desta tela é
    // avisar do corte sem esconder o que deu pra ler.
    const { el, comp } = montar({
      diff: null,
      conteudo: { path: 'a.py', text: 'a = 1\n', size: 999999, truncated: true, digest: null },
    });
    expect(el.textContent).toContain(m.arq_arquivo_cortado());
    expect(el.querySelector('.editor')).not.toBeNull();
    unmount(comp);
  });

  it('pluraliza a meta: 1 linha no singular', () => {
    // A meta (tamanho + linhas) saiu da sub-barra e vive no title da aba: o desenho aprovado
    // deixa ali só a pasta e os números do diff. A regra do plural continua valendo.
    const { el, comp } = montar({
      diff: null,
      conteudo: { path: 'a.py', text: 'print(1)\n', size: 9, truncated: false, digest: 'abc' },
    });
    const titulo = el.querySelector('.aba-nome')?.getAttribute('title') ?? '';
    expect(titulo).toContain('1 linha');
    expect(titulo).not.toContain('1 linhas');
    unmount(comp);
  });

  it('escopo que caiu aparece desabilitado, com o rotulo certo e o motivo visivel', () => {
    const { el, comp } = montar({
      conteudo: null,
      diff: {
        path: 'a.py', diff: '@@ -1 +1 @@\n-a\n+b\n', truncated: false,
        escopo_pedido: 'branch', escopo_usado: 'nao_commitado',
        base: null, motivo: 'arq_motivo_sem_commit_proprio',
      },
    });
    const b = el.querySelector('.escopo') as HTMLButtonElement;
    expect(b.disabled).toBe(true);
    expect(b.textContent).toContain(m.arq_escopo_nao_commitado());   // o que ESTÁ na tela
    // motivo legível, fora do title — o código chega do backend e vira texto pela via dos erros
    expect(el.textContent).toContain(m.arq_motivo_sem_commit_proprio());
    unmount(comp);
  });

  it('motivo desconhecido nao mostra o codigo cru nem quebra', () => {
    const { el, comp } = montar({
      conteudo: null,
      diff: {
        path: 'a.py', diff: '@@ -1 +1 @@\n-a\n+b\n', truncated: false,
        escopo_pedido: 'branch', escopo_usado: 'nao_commitado',
        base: null, motivo: 'arq_motivo_que_nao_existe_ainda',
      },
    });
    expect((el.querySelector('.escopo') as HTMLButtonElement).disabled).toBe(true);
    expect(el.textContent).not.toContain('arq_motivo_que_nao_existe_ainda');
    expect(el.querySelector('.motivo')).toBeNull();
    unmount(comp);
  });

  it('o motivo de base desconhecida tambem vira texto', () => {
    const { el, comp } = montar({
      conteudo: null,
      diff: { path: 'a.py', diff: '@@ -1 +1 @@\n-a\n+b\n', truncated: false,
              escopo_pedido: 'branch', escopo_usado: 'nao_commitado',
              base: null, motivo: 'arq_motivo_sem_base_conhecida' },
    });
    expect(el.textContent).toContain(m.arq_motivo_sem_base_conhecida());
    unmount(comp);
  });

  it('diff cortado mostra o aviso', () => {
    const { el, comp } = montar({
      conteudo: null,
      diff: {
        path: 'a.py', diff: '@@ -1 +1 @@\n+x\n', truncated: true,
        escopo_pedido: 'branch', escopo_usado: 'branch', base: 'abc1234', motivo: null,
      },
    });
    expect(el.textContent).toContain('200 KB');
    unmount(comp);
  });

  it('diff vazio no escopo mostra o ARQUIVO, nao "sem diferencas"', () => {
    const { el, comp } = montar({
      diff: { path: 'a.py', diff: '', truncated: false, escopo_pedido: 'branch',
              escopo_usado: 'branch', base: 'abc1234', motivo: null, original: 'print(1)\n' },
      conteudo: { path: 'a.py', text: 'print(1)\n', size: 9, truncated: false, digest: 'abc' },
    });
    expect(el.querySelector('.editor')).not.toBeNull();
    expect(el.textContent).not.toContain(m.git_sem_diferencas());
    unmount(comp);
  });

  it('nao pisca "sem diferencas" antes de carregar o diff', () => {
    const { el, comp } = montar({
      diff: { path: 'a.py', diff: '@@ -1 +1 @@\n-a\n+b\n', truncated: false,
              escopo_pedido: 'branch', escopo_usado: 'branch', base: 'abc1234', motivo: null },
      conteudo: null,
    });
    // primeiro quadro, SEM await
    expect(el.textContent).not.toContain(m.git_sem_diferencas());
    expect(el.textContent).toContain(m.git_diff_carregando());
    unmount(comp);
  });

  it('carregando sem dados ainda mostra a carga, nao "sem diferencas"', () => {
    const { el, comp } = montar({ loading: true });
    expect(el.textContent).not.toContain(m.git_sem_diferencas());
    expect(el.textContent).toContain(m.git_diff_carregando());
    unmount(comp);
  });

  it('nao mostra o conteudo do arquivo anterior sob o nome do novo', () => {
    const { el, comp } = montar({
      path: 'b.py', loading: true, diff: null,
      conteudo: { path: 'a.py', text: 'CONTEUDO DE A', size: 13, truncated: false, digest: 'abc' },
    });
    expect(el.textContent).not.toContain('CONTEUDO DE A');
    expect(el.textContent).toContain(m.git_diff_carregando());
    unmount(comp);
  });

  it('nao mostra o diff do arquivo anterior sob o nome do novo', () => {
    const { el, comp } = montar({
      path: 'b.py', loading: true, conteudo: null,
      diff: { path: 'a.py', diff: '@@ -1 +1 @@\n-VELHO\n', truncated: false,
              escopo_pedido: 'branch', escopo_usado: 'branch', base: 'abc1234', motivo: null },
    });
    expect(el.textContent).not.toContain('VELHO');
    expect(el.textContent).not.toContain(m.arq_escopo_branch());
    expect(el.textContent).toContain(m.git_diff_carregando());
    unmount(comp);
  });
});
