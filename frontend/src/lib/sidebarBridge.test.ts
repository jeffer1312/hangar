// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { sidebarBridge, type SidebarBridgeHandlers } from './sidebarBridge';
import type { AggSession } from './types';

const session = { name: 'x', serverId: 'a' } as unknown as AggSession;

const noop: SidebarBridgeHandlers = {
  openCreate: () => {},
  openSessionMenu: () => {},
  openKebab: () => {},
};

describe('sidebarBridge', () => {
  it('sem handler registrado é no-op', () => {
    expect(() => sidebarBridge.openCreate()).not.toThrow();
    expect(() => sidebarBridge.openSessionMenu(new MouseEvent('click'), session, 'a')).not.toThrow();
    expect(() => sidebarBridge.openKebab(new MouseEvent('click'))).not.toThrow();
  });

  it('após register delega as três operações', () => {
    const calls: string[] = [];
    const cleanup = sidebarBridge.register({
      openCreate: () => calls.push('create'),
      openSessionMenu: (event, s, serverId) => calls.push(`menu:${s.name}:${serverId}`),
      openKebab: (event) => calls.push('kebab'),
    });
    sidebarBridge.openCreate();
    sidebarBridge.openSessionMenu(new MouseEvent('click'), session, 'a');
    sidebarBridge.openKebab(new MouseEvent('click'));
    expect(calls).toEqual(['create', 'menu:x:a', 'kebab']);
    cleanup();
  });

  it('cleanup remove somente o mesmo handler', () => {
    const seen: string[] = [];
    const a = { ...noop, openCreate: () => seen.push('a') };
    const b = { ...noop, openCreate: () => seen.push('b') };
    const unregisterA = sidebarBridge.register(a);
    const unregisterB = sidebarBridge.register(b);
    unregisterA(); // handler velho: não pode desregistrar o atual
    sidebarBridge.openCreate();
    expect(seen).toEqual(['b']);
    unregisterB();
    sidebarBridge.openCreate(); // sem handler: no-op
    expect(seen).toEqual(['b']);
  });
});
