import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { View } from 'react-native';
import { anchorMoved, anchorNode, listenTourActions, registerAnchor, reportTourAction, unregisterAnchor, useAnchors } from './anchors';

const view = (name: string) => ({ name }) as unknown as View;

describe('peças do tour', () => {
  beforeEach(() => { useAnchors.setState({ onScreen: [], moved: 0 }); listenTourActions(null); });

  it('a peça entra na tela ao registrar e sai só quando o mesmo nó sai', () => {
    const a = view('a');
    const b = view('b');
    registerAnchor('chat-more', a);
    registerAnchor('chat-more', b);
    expect(useAnchors.getState().onScreen).toEqual(['chat-more']);
    unregisterAnchor('chat-more', a);
    expect(anchorNode('chat-more')).toBe(b);
    unregisterAnchor('chat-more', b);
    expect(useAnchors.getState().onScreen).toEqual([]);
    expect(anchorNode('chat-more')).toBeNull();
  });

  it('só peça registrada conta movimento', () => {
    anchorMoved('list-search');
    expect(useAnchors.getState().moved).toBe(0);
    registerAnchor('list-search', view('s'));
    anchorMoved('list-search');
    expect(useAnchors.getState().moved).toBe(1);
  });

  it('a ação feita na tela chega a quem assina', () => {
    const heard = vi.fn();
    reportTourAction('menu');
    listenTourActions(heard);
    reportTourAction('search');
    expect(heard).toHaveBeenCalledTimes(1);
    expect(heard).toHaveBeenCalledWith('search');
  });
});
