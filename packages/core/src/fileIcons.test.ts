import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { ICONES } from './fileIcons.generated';
import lista from './fileIcons.lista.json';
import { nomeIcone, svgIcone } from './fileIcons';

describe('fileIcons', () => {
  it('o gerado cobre a lista inteira (rode `npm run icons` se falhar)', () => {
    for (const n of lista as string[]) expect(ICONES[n], n).toBeTruthy();
    expect(Object.keys(ICONES).length).toBe((lista as string[]).length);
  });

  it('cada svg do tema vem limpo: sem script nem handler inline', () => {
    for (const [n, svg] of Object.entries(ICONES)) {
      expect(svg.startsWith('<svg'), n).toBe(true);
      expect(/<script|\son[a-z]+\s*=|javascript:/i.test(svg), n).toBe(false);
    }
  });

  it('mapeia extensão, nome especial, prefixo e pasta', () => {
    expect(nomeIcone('Folha.svelte', false)).toBe('svelte');
    expect(nomeIcone('x.ts', false)).toBe('typescript');
    expect(nomeIcone('package.json', false)).toBe('nodejs');
    expect(nomeIcone('.env.production', false)).toBe('tune');
    expect(nomeIcone('README.md', false)).toBe('readme');
    expect(nomeIcone('tsconfig.app.json', false)).toBe('tsconfig');
    expect(nomeIcone('docker-compose.dev.yaml', false)).toBe('docker');
    expect(nomeIcone('Dockerfile', false)).toBe('docker');
    expect(nomeIcone('CLAUDE.md', false)).toBe('markdown');
    expect(nomeIcone('sem-extensao', false)).toBe('document');
    expect(nomeIcone('src', true)).toBe('folder-src');
    expect(nomeIcone('src', true, true)).toBe('folder-src-open');
    expect(nomeIcone('qualquer', true, true)).toBe('folder-open');
  });

  it('todo nome que o mapa devolve existe no gerado', () => {
    const nomes = ['a.svelte', 'a.ts', 'a.tsx', 'a.py', 'a.cs', 'a.dart', 'a.pas', 'a.md', 'a.json', 'a.css',
      'a.html', 'a.xml', 'a.sql', 'a.sh', 'a.ps1', 'a.yaml', 'a.toml', 'a.png', 'a.pdf', 'a.txt', 'a.zip',
      'a.woff2', 'a.mp4', 'a.mp3', '.env', 'package.json', 'package-lock.json', 'README.md', 'tsconfig.json',
      'vite.config.ts', 'biome.json', '.gitignore', 'Dockerfile', '.editorconfig', 'vitest.config.ts', 'jest.config.js'];
    for (const n of nomes) expect(ICONES[nomeIcone(n, false)], n).toBeTruthy();
    for (const p of ['src', 'pages', 'components', 'lib', 'tests', 'docs', 'node_modules', '.github', '.vscode',
      'scripts', 'public', 'dist', 'api', 'images', 'config', '.git', 'zzz']) {
      expect(ICONES[nomeIcone(p, true)], p).toBeTruthy();
      expect(ICONES[nomeIcone(p, true, true)], p + ' open').toBeTruthy();
    }
    expect(svgIcone('x.desconhecido', false)).toBe(ICONES.document);
  });

  it('a fonte declara a licença MIT do tema', () => {
    const src = readFileSync(new URL('./fileIcons.generated.ts', import.meta.url), 'utf8');
    expect(src).toMatch(/MIT License/);
  });
});
