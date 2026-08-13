import { describe, expect, it, beforeEach } from 'vitest';
import { overwriteGetLocale } from '../paraglide/runtime';
import * as m from '../paraglide/messages';

// Os grupos das acoes sao mensagens agora: o teste fixa o pt e usa as MESMAS mensagens
// que os componentes usam, senao a agregacao (action.group === group) nunca casa.
beforeEach(() => overwriteGetLocale(() => 'pt'));
import type { AggSession } from './types';
import {
  aggregateWorkspaceActions,
  filterWorkspaceItems,
  resolveWorkspaceChatTarget,
  workspaceSessionItems,
  workspaceSessionKey,
  type WorkspaceAction,
} from './workspaceCommands';

const items = [
  { key: 'archive', title: 'Arquivo', detail: 'Sessões encerradas', keywords: ['histórico'], group: 'Sessão' as const },
  { key: 'git', title: 'Git', detail: 'Branch e alterações', keywords: ['commit'], group: 'Ferramentas' as const },
];

describe('filterWorkspaceItems', () => {
  it('matches accents and keywords', () => {
    expect(filterWorkspaceItems(items, 'historico').map((item) => item.key)).toEqual(['archive']);
    expect(filterWorkspaceItems(items, 'commit').map((item) => item.key)).toEqual(['git']);
  });

  it('keeps the supplied group order', () => {
    const groupedItems = [
      { key: 'chat', title: 'Conversa', detail: 'Abrir conversa', keywords: [], group: m.nav_navegacao() },
      { key: 'archive', title: 'Arquivo', detail: 'Abrir arquivo', keywords: [], group: m.sessao_grupo() },
      { key: 'git', title: 'Git', detail: 'Abrir Git', keywords: [], group: m.lista_ferramentas() },
      { key: 'pair', title: 'Parear', detail: 'Parear sessão', keywords: [], group: m.lista_colaboracao() },
      { key: 'session', title: 'Projeto', detail: 'Abrir sessão', keywords: [], group: 'Sessões' },
    ];

    expect(filterWorkspaceItems(groupedItems, '').map((item) => item.group)).toEqual([
      m.nav_navegacao(),
      m.sessao_grupo(),
      m.lista_ferramentas(),
      m.lista_colaboracao(),
      'Sessões',
    ]);
  });

  it('preserves the original order when matches tie', () => {
    const tiedItems = [
      { key: 'first', title: 'Primeiro', detail: 'Abrir projeto', keywords: [], group: 'Sessões' as const },
      { key: 'second', title: 'Segundo', detail: 'Abrir projeto', keywords: [], group: 'Sessões' as const },
    ];

    expect(filterWorkspaceItems(tiedItems, 'projeto').map((item) => item.key)).toEqual(['first', 'second']);
  });
});

describe('workspaceSessionItems', () => {
  it('uses a server-aware key and exposes server/cwd as searchable text', () => {
    const rows: AggSession[] = [
      {
        name: 'api',
        cwd: '/workspace/checkout',
        state: 'idle',
        serverId: 'server-b',
        serverLabel: 'Produção',
        serverColor: '#22c55e',
      },
    ];

    const sessionItems = workspaceSessionItems(rows);

    expect(sessionItems[0]).toMatchObject({
      key: 'server-b::api',
      kind: 'session',
      title: 'api',
      detail: 'Produção · /workspace/checkout',
      group: 'Sessões',
    });
    expect(filterWorkspaceItems(sessionItems, 'producao')).toHaveLength(1);
    expect(filterWorkspaceItems(sessionItems, 'checkout')).toHaveLength(1);
  });
});

describe('resolveWorkspaceChatTarget', () => {
  const normalA = { serverId: 'server-a', name: 'api' };
  const overlayB = { serverId: 'server-b', name: 'worker' };

  it('uses the server B overlay while it is open', () => {
    expect(resolveWorkspaceChatTarget(normalA, overlayB)).toEqual(overlayB);
  });

  it('restores the last normal server A chat after the overlay closes', () => {
    expect(resolveWorkspaceChatTarget(normalA, null)).toEqual(normalA);
  });

  it('keeps homonymous sessions on different servers distinct', () => {
    expect(workspaceSessionKey(normalA)).toBe('server-a::api');
    expect(workspaceSessionKey({ serverId: 'server-b', name: 'api' })).toBe('server-b::api');
  });
});

describe('aggregateWorkspaceActions', () => {
  const action = (id: string, group: WorkspaceAction['group'], title = id): WorkspaceAction => ({
    id,
    title,
    detail: title,
    keywords: [],
    group,
    run: () => undefined,
  });

  it('deduplicates by id, orders shuffled groups and preserves order inside each group', () => {
    const first = action('first', m.lista_ferramentas());
    const navigation = action('navigation', m.nav_navegacao());
    const session = action('session', m.sessao_grupo());
    const second = action('second', m.lista_ferramentas());
    const collaboration = action('collaboration', m.lista_colaboracao());
    const replacement = action('first', m.lista_ferramentas(), 'substituída');

    expect(aggregateWorkspaceActions([
      first,
      navigation,
      session,
      second,
      collaboration,
      replacement,
    ])).toEqual([
      navigation,
      session,
      replacement,
      second,
      collaboration,
    ]);
  });
});
