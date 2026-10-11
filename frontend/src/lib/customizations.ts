// Plugins e skills de UMA sessão nova (create/customizations.rs): o backend recebe só a diferença
// do padrão do catálogo (ClaudeCustomizationBody, corpo estrito).
import type { ClaudeCustomizationDelta, ClaudeCustomizations, ClaudePlugin, ClaudeSkill } from '@hangar/core';

export const emptyDelta = (): ClaudeCustomizationDelta => ({ plugins: {}, skills: {}, blocked_skills: [] });

export const isEmptyDelta = (d: ClaudeCustomizationDelta) =>
  !Object.keys(d.plugins).length && !Object.keys(d.skills).length && !d.blocked_skills.length;

export const pluginEnabled = (d: ClaudeCustomizationDelta, p: ClaudePlugin) => d.plugins[p.id] ?? p.enabled;
export const looseSkillEnabled = (d: ClaudeCustomizationDelta, s: ClaudeSkill) => !s.blocked && (d.skills[s.name] ?? s.enabled);
export const pluginSkillEnabled = (d: ClaudeCustomizationDelta, s: ClaudeSkill) =>
  s.enabled && !s.blocked && !d.blocked_skills.includes(s.name);

export function setPlugin(d: ClaudeCustomizationDelta, c: ClaudeCustomizations, id: string, on: boolean): ClaudeCustomizationDelta {
  const plugin = c.plugins.find((p) => p.id === id);
  if (!plugin) return d;
  const plugins = { ...d.plugins };
  if (on === plugin.enabled) delete plugins[id]; else plugins[id] = on;
  return { ...d, plugins };
}

export function setLooseSkill(d: ClaudeCustomizationDelta, c: ClaudeCustomizations, name: string, on: boolean): ClaudeCustomizationDelta {
  const skill = c.skills.find((s) => s.name === name && !s.blocked);
  if (!skill) return d;
  const skills = { ...d.skills };
  if (on === skill.enabled) delete skills[name]; else skills[name] = on;
  return { ...d, skills };
}

// Skill de plugin não tem chave própria: desligar é bloquear, e só com o plugin ligado.
export function setPluginSkill(d: ClaudeCustomizationDelta, c: ClaudeCustomizations, pluginId: string, name: string, on: boolean): ClaudeCustomizationDelta {
  const plugin = c.plugins.find((p) => p.id === pluginId);
  if (!plugin || !pluginEnabled(d, plugin) || !plugin.skills.some((s) => s.name === name && s.enabled && !s.blocked)) return d;
  const blocked = d.blocked_skills.filter((n) => n !== name);
  return { ...d, blocked_skills: on ? blocked : [...blocked, name].sort() };
}

export const deltaCount = (d: ClaudeCustomizationDelta) =>
  Object.keys(d.plugins).length + Object.keys(d.skills).length + d.blocked_skills.length;
