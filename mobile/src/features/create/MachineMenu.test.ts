import { describe, expect, it, vi } from 'vitest';

vi.mock('react-native-unistyles', () => ({ StyleSheet: { create: () => ({}) }, useUnistyles: () => ({ theme: {} }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/ProviderGlyph', () => ({ ProviderGlyph: () => null }));
vi.mock('../../ui/haptics', () => ({ haptic: { selection: () => {} } }));

import type { AccountBadge } from '@hangar/core';
import { accountGroups } from './MachineMenu';

const badge = (name: string): AccountBadge => ({ name, icon: 'credit-card', color: null });

describe('accountGroups', () => {
  it('keeps one item per agent in the desktop order, with API keys last', () => {
    const groups = accountGroups({
      'antigravity:/g': badge('Google (padrão)'), 'chave:openrouter': badge('OpenRouter'), 'codex:/b': badge('jeffer1312'),
      'claude:/b': badge('Felizardo 2'), 'codex:/a': badge('claude-200-2'), 'claude:/a': badge('Felizardo'),
    });
    expect(groups.map((g) => g.provider)).toEqual(['claude', 'codex', 'antigravity', 'chave']);
    expect(groups[0].names).toEqual(['Felizardo', 'Felizardo 2']);
    expect(groups[2].names).toEqual(['Google (padrão)']);
  });

  it('is empty without accounts', () => {
    expect(accountGroups(undefined)).toEqual([]);
  });
});
