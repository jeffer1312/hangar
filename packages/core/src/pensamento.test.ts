import { describe, it, expect } from 'vitest';
import { entraNoPensamento, ehBusca, resumoPensamento } from './pensamento';

describe('pensamento', () => {
  it('busca: só WebSearch/WebFetch/ToolSearch entram; tarefas nunca', () => {
    expect(entraNoPensamento('busca', 'WebSearch')).toBe(true);
    expect(entraNoPensamento('busca', 'ToolSearch')).toBe(true);
    expect(entraNoPensamento('busca', 'Bash')).toBe(false);
    expect(entraNoPensamento('tudo', 'Bash')).toBe(true);
    expect(entraNoPensamento('tudo', 'TaskCreate')).toBe(false);
    expect(entraNoPensamento('nada', 'WebSearch')).toBe(false);
  });
  it('ehBusca', () => { expect(ehBusca('WebFetch')).toBe(true); expect(ehBusca('Read')).toBe(false); });
  it('resumo é a primeira frase curta', () => {
    expect(resumoPensamento('Vou olhar o arquivo. Depois testo.')).toBe('Vou olhar o arquivo.');
    expect(resumoPensamento('x'.repeat(200) + '. fim')).toBe('x'.repeat(200) + '. fim');
  });
});
