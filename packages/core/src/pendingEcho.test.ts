import { describe, expect, test } from 'vitest';
import { pendingEchoDelivered } from './pendingEcho';

const msgs = (...texts: string[]) => texts.map((text, i) => ({ id: `u${i}`, text }));

describe('pendingEchoDelivered', () => {
  test('texto igual, linha de uma mensagem fundida e legenda confirmam', () => {
    expect(pendingEchoDelivered(msgs('  oi  '))('oi')).toBe(true);
    expect(pendingEchoDelivered(msgs('msg1\nmsg2'))('msg2')).toBe(true);
    expect(pendingEchoDelivered(msgs('foto'))('foto — 📎 imagem: /tmp/x.jpg')).toBe(true);
  });

  test('multilinha que o Claude juntou com outra no meio do turno confirma', () => {
    expect(pendingEchoDelivered(msgs('T14 linhas\n001 a\n002 b\nT14 linhas\n001 a\n002 b'))('T14 linhas\n001 a\n002 b')).toBe(true);
  });

  test('trecho de uma mensagem anterior ao envio não confirma; a que chegou depois confirma', () => {
    const antes = msgs('lista\n- a\n- b\nfim');
    expect(pendingEchoDelivered(antes)('- a\n- b', 'u0')).toBe(false);
    expect(pendingEchoDelivered([...antes, { id: 'u1', text: 'outra\n- a\n- b' }])('- a\n- b', 'u0')).toBe(true);
  });

  test('pedaço de linha ou trecho sem relação não confirma', () => {
    expect(pendingEchoDelivered(msgs('T14 linhas\n001 a'))('T14 linhas\n001')).toBe(false);
    expect(pendingEchoDelivered(msgs('msg1\nmsg2'))('outra')).toBe(false);
  });
});
