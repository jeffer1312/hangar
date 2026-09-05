import { describe, it, expect } from 'vitest';
import { toolIcon } from './toolIcon';

describe('toolIcon', () => {
  it('mapeia as ferramentas conhecidas e cai em Wrench', () => {
    expect(toolIcon('Bash')).toBe('Terminal');
    expect(toolIcon('Read')).toBe('Eye');
    expect(toolIcon('Edit')).toBe('Pencil');
    expect(toolIcon('Write')).toBe('FilePlus');
    expect(toolIcon('Grep')).toBe('Search');
    expect(toolIcon('WebSearch')).toBe('Globe');
    expect(toolIcon('Agent')).toBe('Bot');
    expect(toolIcon('Qualquer')).toBe('Wrench');
    expect(toolIcon(null)).toBe('Wrench');
  });
});
