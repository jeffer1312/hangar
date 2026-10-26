import { describe, expect, it } from 'vitest';
import { forClient } from './updateNotes';
import type { UpdateNotes } from './types';

const notes: UpdateNotes = {
  target: 'origin/main',
  warnings: [
    { id: 'w1', title: 'Migração', text: 'Apaga o cache antigo.' },
    { id: 'w2', title: 'Vazio', text: '   ' },
  ],
  news: [
    { id: 'a', title: 'Histórico', kind: 'new', text: 't', platforms: 'desktop', try_it: 'Aperte ↑', animation: 'historico-compositor' },
    { id: 'b', title: 'Folha', kind: 'improvement', text: 't', platforms: 'mobile', try_it: null, animation: null },
    { id: 'c', title: 'Geral', kind: 'improvement', text: 't', platforms: 'all', try_it: null, animation: null },
    { id: 'd', title: 'Conserto', kind: 'fix', text: 't', platforms: 'all', try_it: null, animation: 'x' },
    { id: 'e', title: 'Conserto desktop', kind: 'fix', text: 't', platforms: 'desktop', try_it: null, animation: null },
  ],
};

describe('forClient', () => {
  it('mobile keeps mobile and all, splits cards from fixes', () => {
    const r = forClient(notes, 'mobile');
    expect(r.cards.map((n) => n.id)).toEqual(['b', 'c']);
    expect(r.fixes.map((n) => n.id)).toEqual(['d']);
  });

  it('desktop keeps desktop and all, in order', () => {
    const r = forClient(notes, 'desktop');
    expect(r.cards.map((n) => n.id)).toEqual(['a', 'c']);
    expect(r.fixes.map((n) => n.id)).toEqual(['d', 'e']);
  });

  it('fix loses its animation', () => {
    expect(forClient(notes, 'mobile').fixes[0].animation).toBeNull();
  });

  it('keeps warning with blank text: the title alone still gates the update', () => {
    expect(forClient(notes, 'mobile').warnings.map((w) => w.id)).toEqual(['w1', 'w2']);
  });
});
