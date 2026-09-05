import { describe, it, expect } from 'vitest';
import { agruparConversa } from './toolGroups';
import type { ChatEvent } from './types';

const ev = (kind: ChatEvent['kind'], id: string, extra: Partial<ChatEvent> = {}): ChatEvent => ({ kind, id, ...extra });
const tool = (id: string, name = 'Bash') => ev('tool_use', id, { tool_name: name, tool_use_id: id });
const semPensamento = () => false;
const soBusca = (n?: string | null) => n === 'WebSearch' || n === 'WebFetch' || n === 'ToolSearch';

describe('agruparConversa', () => {
  it('1 ou 2 ferramentas seguidas ficam soltas; 3+ viram grupo', () => {
    const a = agruparConversa([tool('a'), tool('b'), ev('assistant_msg', 'm', { text: 'x' })], { entraNoPensamento: semPensamento });
    expect(a.map((i) => i.type)).toEqual(['tool', 'tool', 'event']);
    const b = agruparConversa([tool('a'), tool('b'), tool('c')], { entraNoPensamento: semPensamento });
    expect(b).toEqual([{ type: 'group', id: 'g-a', tools: [tool('a'), tool('b'), tool('c')] }]);
  });

  it('tool_result nunca vira item', () => {
    const r = agruparConversa([tool('a'), ev('tool_result', 'r', { tool_use_id: 'a' })], { entraNoPensamento: semPensamento });
    expect(r.map((i) => i.id)).toEqual(['a']);
  });

  it('pensamentos consecutivos e a busca entre eles viram um bloco; Bash fecha o bloco', () => {
    const r = agruparConversa(
      [ev('thinking', 'p1', { text: 'hm' }), tool('s', 'WebSearch'), ev('thinking', 'p2'), tool('b'), ev('thinking', 'p3')],
      { entraNoPensamento: soBusca },
    );
    expect(r.map((i) => [i.type, i.id])).toEqual([['pensamento', 'p-p1'], ['tool', 'b'], ['pensamento', 'p-p3']]);
    expect((r[0] as { eventos: ChatEvent[] }).eventos.map((e) => e.id)).toEqual(['p1', 's', 'p2']);
  });

  it('busca sem pensamento aberto antes continua card solto', () => {
    const r = agruparConversa([tool('s', 'WebSearch')], { entraNoPensamento: soBusca });
    expect(r[0].type).toBe('tool');
  });

  it('ids repetidos ganham sufixo em vez de derrubar a lista', () => {
    const r = agruparConversa([ev('user_msg', 'q', { text: 'a' }), ev('user_msg', 'q', { text: 'b' })], { entraNoPensamento: semPensamento });
    expect(new Set(r.map((i) => i.id)).size).toBe(2);
  });
});
