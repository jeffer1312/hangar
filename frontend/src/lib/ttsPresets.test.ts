import { describe, it, expect } from 'vitest';
import { ehInstrucaoDigitada, PRESET_LER, PRESET_CODIGO } from './ttsPresets';

describe('ehInstrucaoDigitada', () => {
  it('rejeita os presets (nao viram prefill)', () => {
    expect(ehInstrucaoDigitada(PRESET_LER)).toBe(false);
    expect(ehInstrucaoDigitada(PRESET_CODIGO)).toBe(false);
  });

  it('aceita instrucao livre digitada pelo usuario', () => {
    expect(ehInstrucaoDigitada('resume em uma frase')).toBe(true);
  });
});
