// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest';
import { mount, unmount } from 'svelte';
import * as m from '../paraglide/messages';
import StateChip from './StateChip.svelte';

function monta(props: { state: 'working' | 'idle'; limited?: boolean; limitReset?: string | null }) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const c = mount(StateChip, { target, props });
  return { target, fim: () => { unmount(c); target.remove(); } };
}

describe('StateChip — limite de uso', () => {
  it('parada no limite: texto com a hora de volta e cor própria, mesmo com state=working', () => {
    const { target, fim } = monta({ state: 'working', limited: true, limitReset: '9:10pm' });
    const chip = target.querySelector('.chip')!;
    expect(chip.textContent).toBe(m.estado_limite_volta({ n: '9:10pm' }));
    expect(chip.classList.contains('limite')).toBe(true);
    expect(chip.classList.contains('working')).toBe(false);
    fim();
  });

  it('sem limite segue o estado de sempre', () => {
    const { target, fim } = monta({ state: 'working' });
    const chip = target.querySelector('.chip')!;
    expect(chip.textContent).toBe(m.estado_em_execucao());
    expect(chip.classList.contains('working')).toBe(true);
    fim();
  });
});
