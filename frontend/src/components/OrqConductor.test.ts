// @vitest-environment happy-dom
import { afterEach, expect, it } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import type { OrqConductor as Data, OrqWatchdog } from '@hangar/core';
import * as m from '../paraglide/messages';
import OrqConductor from './OrqConductor.svelte';

const WD: OrqWatchdog = { alive: false, last_cycle: null, since: null, restarts: null, unit: null,
  unit_state: null, arbiter: null, watching: [], source: 'unavailable' };
const JEV = { mode: 'shadow', choice: 'nothing', p: 0.97,
  veto: { context: 0.1, user: 0.2, problem: 0.55, deviation: 0.1 }, held: ['problem'],
  would_drop: false, error: null };
const DATA: Data = { watchdog: WD, truncated: false, skipped: 2, feed: [
  { id: 'r3', ts: '2026-09-25T10:02:00-03:00', kind: 'alarm', text: 'rev1-parada', task: 1, jev: null },
  { id: 'r2', ts: '2026-09-25T10:01:00-03:00', kind: 'woke', text: 'use `orq notify` aqui', task: 2, jev: JEV },
  { id: 'e1', ts: '2026-09-25T10:00:00-03:00', kind: 'event', text: 'task_inicio T1', task: 1, jev: null },
] };

let alvo: HTMLElement;
let comp: ReturnType<typeof mount> | null = null;
async function montar(props: { conductor: Data | null; error: string }) {
  if (comp) await unmount(comp);
  document.body.innerHTML = '';
  alvo = document.body.appendChild(document.createElement('div'));
  comp = mount(OrqConductor, { target: alvo, props });
  await tick();
}
afterEach(async () => { if (comp) await unmount(comp); comp = null; document.body.innerHTML = ''; });

it('carregando, erro e vazio têm texto próprio', async () => {
  await montar({ conductor: null, error: '' });
  expect(alvo.textContent).toContain(m.orq_conductor_loading());
  await montar({ conductor: null, error: 'boom' });
  expect(alvo.textContent).toContain(m.orq_conductor_error({ error: 'boom' }));
  await montar({ conductor: { ...DATA, feed: [], skipped: 0 }, error: '' });
  expect(alvo.textContent).toContain(m.orq_conductor_empty());
  expect(alvo.textContent).toContain(m.orq_conductor_unavailable());
  // Só eventos, nenhum recado: a frase de vazio continua, com os eventos embaixo.
  await montar({ conductor: { ...DATA, feed: [DATA.feed[2]] }, error: '' });
  expect(alvo.textContent).toContain(m.orq_conductor_empty());
  expect(alvo.querySelectorAll('.item')).toHaveLength(1);
});

it('filtra por tipo e por Task', async () => {
  await montar({ conductor: DATA, error: '' });
  expect(alvo.querySelectorAll('.item')).toHaveLength(3);
  alvo.querySelector<HTMLButtonElement>('[data-kind="alarm"]')!.click();
  await tick();
  expect(alvo.querySelectorAll('.item')).toHaveLength(1);
  alvo.querySelector<HTMLButtonElement>('[data-kind="all"]')!.click();
  await tick();
  const sel = alvo.querySelector('select')!;
  sel.selectedIndex = 1;   // T1
  sel.dispatchEvent(new Event('change', { bubbles: true }));
  await tick();
  expect([...alvo.querySelectorAll('.item .text')].map((e) => e.textContent)).toEqual(['rev1-parada', 'task_inicio T1']);
  expect(alvo.textContent).toContain(m.orq_conductor_skipped({ n: 2 }));
});

it('o item do Jev abre escolha, certeza e vetos com o que segurou destacado; crase fica crase', async () => {
  await montar({ conductor: DATA, error: '' });
  expect(alvo.querySelector('code')).toBeNull();
  expect(alvo.textContent).toContain('use `orq notify` aqui');
  alvo.querySelector<HTMLButtonElement>('.jev-toggle')!.click();
  await tick();
  expect(alvo.textContent).toContain('nothing');
  expect(alvo.textContent).toContain('0.97');
  expect(alvo.querySelectorAll('.vetoes li')).toHaveLength(4);
  expect(alvo.querySelector('.vetoes .held')!.textContent).toContain('problem');
});
