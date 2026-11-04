// @vitest-environment happy-dom
import { expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import type { ChatEvent, StateEvent } from '@hangar/core';
import MessageList from './MessageList.svelte';
import * as m from '../paraglide/messages';

it.each(['hook_prompt', 'hook_failed'])('o título do aviso %s vem do tipo, com a causa preservada', async text => {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const detail = 'hook-detail-127';
  const component = mount(MessageList, { target, props: {
    events: [{ id: 'hook-1', kind: 'notice', text, hook_error: detail }], pending: [],
    sessionName: 's', dockH: 0, stateEvent: { session: 's', state: 'idle' },
    onSelectOption: () => {}, onCancel: () => {},
  } });
  try {
    await tick();
    const notice = target.querySelector('.notice-hook');
    expect(notice?.textContent).toContain(text === 'hook_failed' ? m.notice_hook_failed() : m.notice_hook_prompt());
    expect(notice?.textContent).toContain(detail);
    expect(notice?.textContent?.split(detail)).toHaveLength(2);
  } finally { await unmount(component); target.remove(); }
});

it('preserva o texto ao voltar, anima só o streaming novo e conclui inteiro ao parar', async () => {
  const frames = new Map<number, FrameRequestCallback>();
  let frameId = 0;
  vi.stubGlobal('requestAnimationFrame', (fn: FrameRequestCallback) => {
    frames.set(++frameId, fn);
    return frameId;
  });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  const target = document.createElement('div');
  document.body.appendChild(target);
  const recebido = 'Resposta já recebida.';
  const props = $state({
    events: [] as ChatEvent[], pending: [], sessionName: 's', dockH: 0,
    stateEvent: { session: 's', state: 'working' } as StateEvent,
    preview: recebido, previewMd: true,
    onSelectOption: () => {}, onCancel: () => {},
  });
  let comp = mount(MessageList, { target, props });
  const texto = () => target.querySelector('.assistant-msg .prose')?.textContent?.trim();
  try {
    await tick();
    expect(texto()).toBe(recebido);
    props.preview += ' Texto novo em streaming.'.repeat(10);
    await tick();
    expect(texto()).toBe(recebido);
    for (const agora of [1000, 1040, 1080]) {
      const pendentes = [...frames.values()];
      frames.clear();
      pendentes.forEach((fn) => fn(agora));
      await tick();
    }
    expect(texto()!.startsWith(recebido)).toBe(true);
    expect(texto()!.length).toBeGreaterThan(recebido.length);
    expect(texto()!.length).toBeLessThan(props.preview.length);

    await unmount(comp);
    comp = mount(MessageList, { target, props });
    await tick();
    expect(texto()).toBe(props.preview);

    props.preview += ' Mais conteúdo novo.';
    await tick();
    expect(texto()).not.toBe(props.preview);
    props.stateEvent.state = 'idle';
    await tick();
    expect(texto()).toBe(props.preview);
    props.preview += ' Último trecho.';
    await tick();
    expect(texto()).toBe(props.preview);

    props.events = [{ kind: 'assistant_msg', id: 'final', text: props.preview }];
    props.preview = '';
    await tick();
    expect(target.querySelectorAll('.assistant-msg')).toHaveLength(1);
    expect(texto()).toBe(props.events[0].text);
  } finally {
    await unmount(comp);
    target.remove();
    vi.unstubAllGlobals();
  }
});

it('prévia vivo (deltas do modelo) aparece como chega, sem máquina de escrever', async () => {
  vi.stubGlobal('requestAnimationFrame', () => 1);
  vi.stubGlobal('cancelAnimationFrame', () => {});
  const target = document.createElement('div');
  document.body.appendChild(target);
  const props = $state({
    events: [] as ChatEvent[], pending: [], sessionName: 's', dockH: 0,
    stateEvent: { session: 's', state: 'working' } as StateEvent,
    preview: 'Começo.', previewMd: true, previewVivo: true,
    onSelectOption: () => {}, onCancel: () => {},
  });
  const comp = mount(MessageList, { target, props });
  const texto = () => target.querySelector('.assistant-msg .prose')?.textContent?.trim();
  try {
    await tick();
    props.preview += ' Delta novo chegando do stream.'.repeat(10);
    await tick();
    expect(texto()).toBe(props.preview);   // sem frame nenhum rodado: nada ficou pra trás
  } finally {
    await unmount(comp);
    target.remove();
    vi.unstubAllGlobals();
  }
});
