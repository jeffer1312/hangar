import { describe, expect, test } from 'vitest';
import { ecoEntregue } from './ecoPendente';

const msgs = (...texts: string[]) => texts.map((text, i) => ({ id: `u${i}`, text }));

describe('ecoEntregue', () => {
  test('texto igual, linha de uma mensagem fundida e legenda confirmam', () => {
    expect(ecoEntregue(msgs('  oi  '))('oi')).toBe(true);
    expect(ecoEntregue(msgs('msg1\nmsg2'))('msg2')).toBe(true);
    expect(ecoEntregue(msgs('foto'))('foto — 📎 imagem: /tmp/x.jpg')).toBe(true);
  });

  test('multilinha que o Claude juntou com outra no meio do turno confirma', () => {
    expect(ecoEntregue(msgs('T14 linhas\n001 a\n002 b\nT14 linhas\n001 a\n002 b'))('T14 linhas\n001 a\n002 b')).toBe(true);
  });

  test('trecho de uma mensagem anterior ao envio não confirma; a que chegou depois confirma', () => {
    const antes = msgs('lista\n- a\n- b\nfim');
    expect(ecoEntregue(antes)('- a\n- b', 'u0')).toBe(false);
    expect(ecoEntregue([...antes, { id: 'u1', text: 'outra\n- a\n- b' }])('- a\n- b', 'u0')).toBe(true);
  });

  test('pedaço de linha ou trecho sem relação não confirma', () => {
    expect(ecoEntregue(msgs('T14 linhas\n001 a'))('T14 linhas\n001')).toBe(false);
    expect(ecoEntregue(msgs('msg1\nmsg2'))('outra')).toBe(false);
  });
});
