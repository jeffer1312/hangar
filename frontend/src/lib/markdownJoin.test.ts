import { describe, it, expect } from 'vitest';
import { renderMarkdown } from './markdown';

describe('renderMarkdown — joinWrapped (markdown de ARQUIVO)', () => {
  it('junta linhas do mesmo paragrafo, sem colar lista/heading', () => {
    const src = '**Tarefa:** abrir a sessao do par\ndentro de um modal, inteira e usavel.\n\n- item um\n- item dois\n';
    const html = renderMarkdown(src, { joinWrapped: true });
    expect(html).toContain('dentro de um modal');
    expect(html.match(/<p/g)?.length ?? 0).toBe(1);   // um paragrafo, nao dois
    expect(html.match(/<li/g)?.length ?? 0).toBe(2);  // a lista continua com 2 itens
  });
  it('sem a opcao, o chat continua quebrando linha a linha', () => {
    const html = renderMarkdown('linha um\nlinha dois');
    expect(html.match(/<p/g)?.length ?? 0).toBe(2);
  });
});

describe('renderMarkdown — joinWrapped em listas', () => {
  it('junta a 2a linha DENTRO do item, sem quebrar a lista nem reiniciar a numeracao', () => {
    const src = '1. **Um.** primeira linha\n   continuacao do item um\n2. **Dois.** outro item\n';
    const html = renderMarkdown(src, { joinWrapped: true });
    expect(html.match(/<li/g)?.length ?? 0).toBe(2);
    expect(html).toContain('continuacao do item um');
    expect(html.match(/<ol/g)?.length ?? 0).toBe(1);
  });
  it('nao cola texto num heading nem numa cerca de codigo', () => {
    const html = renderMarkdown('## Titulo\ntexto embaixo\n', { joinWrapped: true });
    expect(html).toContain('<h2');
    expect(html).toContain('texto embaixo');
    expect(html).not.toContain('Titulo texto embaixo');
  });
});
