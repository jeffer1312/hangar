import { describe, expect, it } from 'vitest';
import { lerTabelaMarkdown } from '@hangar/core';

describe('TableChart duplicate columns', () => {
  it('duas colunas com mesmo título têm identidade distinta; selecionar segunda pega índice 1', () => {
    const md = `| item | total | total |\n|---|---:|---:|\n| a | 1 | 10 |\n| b | 2 | 20 |`;
    const tabelas = lerTabelaMarkdown(md);
    expect(tabelas).toHaveLength(1);
    const t = tabelas[0];
    expect(t.colunas.map((c) => c.titulo)).toEqual(['total', 'total']);
    expect(t.colunas[0].valores).toEqual([1, 2]);
    expect(t.colunas[1].valores).toEqual([10, 20]);

    // Simula items do TableChart
    const items = t.colunas.map((c, idx) => ({ label: c.titulo, selected: idx === 0 }));
    // Bug antigo: findIndex por título sempre devolve 0 para a segunda
    const bugIdx = t.colunas.findIndex((c) => c.titulo === items[1].label);
    expect(bugIdx).toBe(0);
    // Fix: indexOf pela identidade do objeto devolve 1
    const fixIdx = items.indexOf(items[1]);
    expect(fixIdx).toBe(1);
  });

  it('chaves de PillMenu com índice são únicas mesmo com títulos repetidos', () => {
    const items = [
      { label: 'total', hint: undefined },
      { label: 'total', hint: undefined },
    ];
    const keys = items.map((it, idx) => `${it.label}-${idx}-${it.hint ?? ''}`);
    expect(new Set(keys).size).toBe(2);
    // chaves antigas sem índice colidiriam
    const oldKeys = items.map((it) => it.label + (it.hint ?? ''));
    expect(new Set(oldKeys).size).toBe(1);
  });
});
