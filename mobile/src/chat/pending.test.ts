import { describe, test, expect } from 'vitest';
import { reconcilePending, type PendingMsg } from './pending';
import type { ChatEvent } from '@hangar/core';

function p(text: string, id = `pending-${text.slice(0, 5)}`): PendingMsg {
  return { id, text };
}
function incoming(text: string, id = 'a:1'): ChatEvent {
  return { kind: 'user_msg', id, text, ts: 1 } as ChatEvent;
}

describe('reconcilePending', () => {
  test('mesmo texto sai', () => {
    const pending = [p('oi')];
    expect(reconcilePending(pending, incoming('oi'))).toEqual([]);
  });

  test('texto com espaço/quebra diferente sai (trim + linha)', () => {
    expect(reconcilePending([p('oi')], incoming('  oi  '))).toEqual([]);
    expect(reconcilePending([p('oi')], incoming('oi\noutra'))).toEqual([]);
    expect(reconcilePending([p('oi ')], incoming('oi'))).toEqual([]);
  });

  test('multilinha do transcript cobre pendings de linhas individuais', () => {
    const pending = [p('msg1', 'p1'), p('msg2', 'p2')];
    // Claude funde "msg1\nmsg2" num único user_msg
    const res = reconcilePending(pending, incoming('msg1\nmsg2'));
    expect(res).toEqual([]);
  });

  test('multilinha que o Claude juntou com outra no meio do turno sai', () => {
    const pending = [p('T14 linhas\n001 a\n002 b', 'p1'), p('nada a ver\n001 a', 'p2')];
    const res = reconcilePending(pending, incoming('T14 linhas\n001 a\n002 b\nT14 linhas\n001 a\n002 b'));
    expect(res.map((x) => x.id)).toEqual(['p2']);
  });

  test('legenda sem marcador casa com pending que tinha marcador', () => {
    const pend = [p('foto — 📎 imagem: /tmp/x.jpg')];
    expect(reconcilePending(pend, incoming('foto'))).toEqual([]);
  });

  test('pendente sem relação fica, ordem preservada', () => {
    const pending = [p('a', '1'), p('b', '2'), p('c', '3')];
    const res = reconcilePending(pending, incoming('b'));
    expect(res.map((x) => x.id)).toEqual(['1', '3']);
  });

  test('incoming não-user_msg ou sem texto não remove nada', () => {
    const pending = [p('oi')];
    expect(reconcilePending(pending, { kind: 'assistant_msg', id: 'a:2', text: 'oi', ts: 1 } as ChatEvent)).toEqual(pending);
    expect(reconcilePending(pending, incoming(''))).toEqual(pending);
  });

  test('texto diferente não remove', () => {
    expect(reconcilePending([p('oi')], incoming('olá'))).toHaveLength(1);
  });

  test('vazio retorna vazio', () => {
    expect(reconcilePending([], incoming('oi'))).toEqual([]);
  });

  test('replay de queued e user_msg reconcilia eco sem alterar o rascunho ou confirmar envio', () => {
    const pending = [p('primeiro', 'p1'), p('edição nova', 'p2')];
    const queued = reconcilePending(pending, incoming('primeiro', 'queued-1'));
    const real = reconcilePending(queued, incoming('primeiro', 'transcript:1'));
    expect(real).toEqual([p('edição nova', 'p2')]);
    expect(reconcilePending(real, incoming('primeiro', 'transcript:1'))).toEqual(real);
  });
});
