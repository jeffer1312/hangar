import { describe, expect, it } from 'vitest';
import { emptyDelta, isEmptyDelta, setLooseSkill, setPlugin, setPluginSkill } from './customizations';

const catalog = {
  plugins: [{ id: 'p', name: 'p', enabled: true, skills: [{ name: 'p:s', enabled: true }] }],
  skills: [{ name: 'solta', enabled: true }],
};

describe('delta de plugins e skills', () => {
  it('guarda só o que difere do padrão do catálogo', () => {
    const off = setPlugin(emptyDelta(), catalog, 'p', false);
    expect(off.plugins).toEqual({ p: false });
    expect(isEmptyDelta(setPlugin(off, catalog, 'p', true))).toBe(true);
    expect(setLooseSkill(emptyDelta(), catalog, 'solta', false).skills).toEqual({ solta: false });
  });

  it('skill de plugin desliga por blocked_skills, e só com o plugin ligado', () => {
    const d = setPluginSkill(emptyDelta(), catalog, 'p', 'p:s', false);
    expect(d.blocked_skills).toEqual(['p:s']);
    expect(d.skills).toEqual({});
    expect(isEmptyDelta(setPluginSkill(d, catalog, 'p', 'p:s', true))).toBe(true);
    const pluginOff = setPlugin(emptyDelta(), catalog, 'p', false);
    expect(setPluginSkill(pluginOff, catalog, 'p', 'p:s', false).blocked_skills).toEqual([]);
  });
});
