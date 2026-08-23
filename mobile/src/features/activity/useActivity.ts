// Hook incremental para derivar Activity a partir do fluxo de ChatEvent.
// Porte do padrão de Chat.svelte:831 — createActivityFolder() alimentado evento a evento.
import { useMemo, useRef } from 'react';
import { createActivityFolder } from '@hangar/core';
import type { Activity, ChatEvent } from '@hangar/core';

export function useActivity(events: ChatEvent[]): Activity {
  const folder = useRef(createActivityFolder());
  const visto = useRef(0);

  // precisa acontecer durante o render, antes do snapshot — é o que evita re-varrer tudo
  if (events.length < visto.current) {
    folder.current.reset(events);
    visto.current = events.length;
  } else if (events.length > visto.current) {
    for (let i = visto.current; i < events.length; i++) {
      const ev = events[i];
      if (ev) folder.current.push(ev);
    }
    visto.current = events.length;
  }

  return useMemo(() => folder.current.snapshot(), [events]);
}
