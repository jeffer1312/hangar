import { describe, it, expect } from 'vitest';
import { ehInstrucaoDigitada, presetPadrao, PRESET_LER, PRESET_CODIGO } from './ttsPresets';

describe('ehInstrucaoDigitada', () => {
  it('rejeita os presets (nao viram prefill)', () => {
    expect(ehInstrucaoDigitada(PRESET_LER)).toBe(false);
    expect(ehInstrucaoDigitada(PRESET_CODIGO)).toBe(false);
  });

  it('aceita instrucao livre digitada pelo usuario', () => {
    expect(ehInstrucaoDigitada('resume em uma frase')).toBe(true);
  });
});

describe('presetPadrao', () => {
  it('com codigo no alvo, o padrao explica em vez de ler', () => {
    expect(presetPadrao(true)).toBe(PRESET_CODIGO);
  });

  it('sem codigo, o padrao le como esta (sem chamar a Groq)', () => {
    expect(presetPadrao(false)).toBe(PRESET_LER);
  });
});
