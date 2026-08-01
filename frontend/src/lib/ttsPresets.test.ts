import { describe, it, expect } from 'vitest';
import { ehInstrucaoDigitada, presetPadrao, PRESET_LER, PRESET_CODIGO, PRESET_FALA } from './ttsPresets';

describe('ehInstrucaoDigitada', () => {
  it('rejeita os presets (nao viram prefill)', () => {
    expect(ehInstrucaoDigitada(PRESET_LER)).toBe(false);
    expect(ehInstrucaoDigitada(PRESET_CODIGO)).toBe(false);
    expect(ehInstrucaoDigitada(PRESET_FALA)).toBe(false);
  });

  it('aceita instrucao livre digitada pelo usuario', () => {
    expect(ehInstrucaoDigitada('resume em uma frase')).toBe(true);
  });
});

describe('presetPadrao', () => {
  it('adapta pra fala por padrao, com ou sem codigo no alvo', () => {
    // O padrao da feature: ninguem digita nada e mesmo assim o texto chega falavel. "Explicar o
    // codigo" virou escolha explicita — quem seleciona um trecho com codigo quase sempre quer ouvir
    // o texto, e so as vezes a explicacao da logica.
    expect(presetPadrao(false)).toBe(PRESET_FALA);
    expect(presetPadrao(true)).toBe(PRESET_FALA);
  });
});

describe('PRESET_FALA', () => {
  it('proibe resumo de forma explicita', () => {
    // A parte mais facil de o modelo desobedecer: quem manda ouvir um plano quer o plano inteiro,
    // nao a ideia geral dele.
    expect(PRESET_FALA).toMatch(/NÃO é resumo/);
    expect(PRESET_FALA).toMatch(/TODA a informação/);
  });
});
