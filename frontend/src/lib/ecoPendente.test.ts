import { describe, expect, test } from 'vitest';
import { ecoEntregue } from './ecoPendente';

describe('ecoEntregue', () => {
  test('texto igual, linha de uma mensagem fundida e legenda confirmam', () => {
    expect(ecoEntregue(['  oi  '])('oi')).toBe(true);
    expect(ecoEntregue(['msg1\nmsg2'])('msg2')).toBe(true);
    expect(ecoEntregue(['foto'])('foto — 📎 imagem: /tmp/x.jpg')).toBe(true);
  });

  test('multilinha que o Claude juntou com outra no meio do turno confirma', () => {
    expect(ecoEntregue(['T14 linhas\n001 a\n002 b\nT14 linhas\n001 a\n002 b'])('T14 linhas\n001 a\n002 b')).toBe(true);
  });

  test('pedaço de linha ou trecho sem relação não confirma', () => {
    expect(ecoEntregue(['T14 linhas\n001 a'])('T14 linhas\n001')).toBe(false);
    expect(ecoEntregue(['msg1\nmsg2'])('outra')).toBe(false);
  });
});
