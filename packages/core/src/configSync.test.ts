import { describe, expect, it } from 'vitest';
import { configSyncItemLabel, configSyncWarningText, diffManifests, type ConfigSyncManifest } from './configSync';
import { mensagemDeErro } from './errosApi';

const manifest = (hashes: Record<string, string>): ConfigSyncManifest => ({
  version: 1,
  machine: '',
  items: { claude_env: { ok: true, hashes, bytes: 0, warnings: [] } },
});

describe('diffManifests', () => {
  it('separa novo, alterado, igual e só no destino', () => {
    const d = diffManifests(manifest({ A: '1', B: '2', C: '3' }), manifest({ B: '2', C: 'x', D: '4' }), ['claude_env']);
    expect(d.claude_env).toEqual({ added: ['A'], changed: ['C'], same: ['B'], onlyTarget: ['D'] });
  });

  it('destino sem o item conta tudo como novo', () => {
    const d = diffManifests(manifest({ A: '1' }), { version: 1, machine: '', items: {} }, ['claude_env']);
    expect(d.claude_env).toEqual({ added: ['A'], changed: [], same: [], onlyTarget: [] });
  });
});

describe('textos', () => {
  it('aviso conhecido leva os parâmetros e o desconhecido cai no código', () => {
    const texto = configSyncWarningText({ code: 'config_sync_missing_program', params: { program: 'node', where: 'statusLine' } });
    expect(texto).toContain('node');
    expect(texto).toContain('statusLine');
    expect(configSyncWarningText({ code: 'config_sync_novo', params: {} })).toBe('config_sync_novo');
  });

  it('todo item tem rótulo próprio', () => {
    expect(configSyncItemLabel('claude_skills')).not.toBe('claude_skills');
  });

  it('erro HTTP da rota vira frase traduzida', () => {
    expect(mensagemDeErro('config_sync_busy')).toBeTruthy();
    expect(mensagemDeErro('config_sync_version', { version: '2' })).toBeTruthy();
  });
});
