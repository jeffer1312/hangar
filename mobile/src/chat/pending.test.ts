import { describe, test, expect } from 'vitest';
import { bindQueued, reconcilePending, settleQueued, type PendingMsg } from './pending';
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

  test('na reconstrução, trecho de mensagem anterior ao envio não confirma; a que chegou depois confirma', () => {
    const antes = incoming('lista\n- a\n- b\nfim', 'u0');
    const pend = [{ id: 'p1', text: '- a\n- b', afterId: 'u0' }];
    const order = new Map([['u0', 0], ['u1', 1]]);
    expect(reconcilePending(pend, antes, order)).toEqual(pend);
    expect(reconcilePending(pend, incoming('outra\n- a\n- b', 'u1'), order)).toEqual([]);
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

describe('eco que passa pela fila', () => {
  test('a bolha queued- prende o eco que já está no destino, sem tirá-lo', () => {
    const solid = { ...p('oi', 'p1'), solid: true };
    const bound = bindQueued([solid, p('outra', 'p2')], incoming('oi', 'queued-7'));
    expect(bound).toEqual([{ ...solid, entry: 'queued-7' }, p('outra', 'p2')]);
    // A mensagem real continua tirando o eco, preso ou não.
    expect(reconcilePending(bound, incoming('oi', 'transcript:1'))).toEqual([p('outra', 'p2')]);
  });

  test('eco esmaecido, eco já preso e evento que não é da fila ficam com a regra de sempre', () => {
    const dim = [p('oi')];
    expect(bindQueued(dim, incoming('oi', 'queued-7'))).toBe(dim);
    const taken = [{ ...p('oi'), solid: true, entry: 'queued-1' }];
    expect(bindQueued(taken, incoming('oi', 'queued-7'))).toBe(taken);
    const solid = [{ ...p('oi'), solid: true }];
    expect(bindQueued(solid, incoming('oi', 'transcript:1'))).toBe(solid);
  });

  test('duas mensagens iguais: cada bolha da fila prende um eco', () => {
    const twice = [{ ...p('oi', 'p1'), queue: true }, { ...p('oi', 'p2'), queue: true }];
    const first = bindQueued(twice, incoming('oi', 'queued-1'));
    expect(bindQueued(first, incoming('oi', 'queued-2')).map((x) => x.entry)).toEqual(['queued-1', 'queued-2']);
  });

  test('o eco da fila sai quando o servidor lista a entrada; o da conversa fica', () => {
    const queue = { ...p('na fila', 'p1'), queue: true };
    const solid = { ...p('na conversa', 'p2'), solid: true, entry: 'queued-2' };
    expect(settleQueued([queue, solid], [{ id: '1', text: ' na fila ' }, { id: '2', text: 'na conversa' }], [])).toEqual([solid]);
    const same = [queue, solid];
    expect(settleQueued(same, [{ id: '9', text: 'outra' }], [])).toBe(same);
  });

  test('duas mensagens iguais: cada entrada tira um eco só, e a que já estava no painel não tira nenhum', () => {
    const first = { ...p('ok', 'p1'), queue: true, known: [] };
    const second = { ...p('ok', 'p2'), queue: true, known: [] };
    expect(settleQueued([first, second], [{ id: '1', text: 'ok' }], [])).toEqual([second]);
    // Enviada com um "ok" antigo ainda esperando: a entrada antiga não é a dela.
    const later = { ...p('ok', 'p3'), queue: true, known: ['1'] };
    const kept = [later];
    expect(settleQueued(kept, [{ id: '1', text: 'ok' }], [])).toBe(kept);
    expect(settleQueued(kept, [{ id: '1', text: 'ok' }, { id: '2', text: 'ok' }], [])).toEqual([]);
  });

  test('o eco do painel preso a uma bolha sai quando a entrada dela é listada, mesmo com outro texto', () => {
    const bound = { ...p('com  espaço', 'p1'), queue: true, entry: 'queued-5' };
    expect(settleQueued([bound], [{ id: '5', text: 'com espaço' }], [])).toEqual([]);
  });

  test('entrada apagada leva o eco preso a ela', () => {
    const solid = { ...p('oi'), solid: true, entry: 'queued-2' };
    expect(settleQueued([solid], [], ['queued-2'])).toEqual([]);
  });
});
