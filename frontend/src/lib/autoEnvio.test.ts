import { describe, expect, it } from 'vitest';
import { podeEnviarSozinho } from './autoEnvio';

const base = { motivo: 'silencio' as const, texto: 'usa o redis', aviso: null, rascunhoAntes: false };

describe('podeEnviarSozinho', () => {
  it('envia quando o silêncio encerrou e o texto veio limpo', () => {
    expect(podeEnviarSozinho(base)).toBe(true);
  });

  it('não envia quando quem encerrou foi o botão', () => {
    // Quem tocou no mic esta olhando a tela: aquela mensagem volta a ser revisada a mao.
    expect(podeEnviarSozinho({ ...base, motivo: 'botao' })).toBe(false);
  });

  it('não envia quando encerrou pelo teto de tempo', () => {
    // Tempo acabou nao e sinal de que a pessoa terminou de falar.
    expect(podeEnviarSozinho({ ...base, motivo: 'teto' })).toBe(false);
  });

  it('não envia quando a página escondeu', () => {
    expect(podeEnviarSozinho({ ...base, motivo: 'escondeu' })).toBe(false);
  });

  it('não envia com aviso da limpeza', () => {
    // O aviso mais grave: a limpeza mudou o sentido inventando palavra. Mandar isso sozinho seria
    // pior que nao ter a feature.
    expect(podeEnviarSozinho({ ...base, aviso: 'a limpeza mudou o sentido' })).toBe(false);
  });

  it('não envia por cima de rascunho que a pessoa digitou', () => {
    // O texto transcrito e ACRESCENTADO ao que ja estava no campo. Enviar sozinho levaria junto um
    // rascunho meio-escrito que ninguem revisou.
    expect(podeEnviarSozinho({ ...base, rascunhoAntes: true })).toBe(false);
  });

  it('não envia texto vazio ou só espaço', () => {
    expect(podeEnviarSozinho({ ...base, texto: '   ' })).toBe(false);
  });
});
