// @vitest-environment happy-dom
import { afterEach, expect, it } from 'vitest';
import { flushSync, mount, unmount, tick } from 'svelte';
import type { OrqConductor as Data } from '@hangar/core';
import * as m from '../paraglide/messages';
import OrqConductor from './OrqConductor.svelte';

const DATA: Data = {
  watchdog: { alive: false, last_cycle: null, since: null, restarts: null, unit: null, unit_state: null,
    arbiter: null, watching: [], source: 'unavailable' },
  truncated: false, skipped: 0, feed: [
    { id: 'r2', ts: '2026-09-25T10:01:00-03:00', kind: 'alarm', text: 'rev1-parada', task: 1, jev: null },
    { id: 'e1', ts: '2026-09-25T10:00:00-03:00', kind: 'event', text: 'task_inicio T1', task: 1, jev: null },
  ],
};

let comp: ReturnType<typeof mount> | null = null;
afterEach(async () => { if (comp) await unmount(comp); comp = null; document.body.innerHTML = ''; });

it('erro na revalidação mantém o feed montado, com o filtro, e avisa em cima', async () => {
  const props = $state({ conductor: DATA as Data | null, error: '' });
  const alvo = document.body.appendChild(document.createElement('div'));
  comp = mount(OrqConductor, { target: alvo, props });
  await tick();
  alvo.querySelector<HTMLButtonElement>('[data-kind="alarm"]')!.click();
  await tick();
  const feed = alvo.querySelector('.feed');
  expect(alvo.querySelectorAll('.item')).toHaveLength(1);

  props.error = 'boom';
  flushSync();
  expect(alvo.querySelector('.feed')).toBe(feed);
  expect(alvo.querySelectorAll('.item')).toHaveLength(1);
  expect(alvo.querySelector('.warn')!.textContent).toBe(m.orq_conductor_error({ error: 'boom' }));
});
