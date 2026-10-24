// @vitest-environment happy-dom
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, statSync, symlinkSync, watch, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import path from 'node:path';
import { expect, it, vi } from 'vitest';

const require = createRequire(import.meta.url);
const builderPath = require.resolve('../../../scripts/build-terminal-document.cjs');
const { generateTerminalDocument, watchTerminalDocument } = require(builderPath);

it('regenera documento ao mudar JS/CSS, sem regravar conteúdo igual, e encerra observação', async () => {
  const terminal = mkdtempSync(path.join(tmpdir(), 'hangar-terminal-document-'));
  const output = path.join(terminal, 'terminalDocument.generated.json');
  let observer: ReturnType<typeof watch> | undefined;
  let outputObserver: ReturnType<typeof watch> | undefined;
  try {
    for (const file of ['terminalPage.js', 'terminal.css']) copyFileSync(new URL(file, import.meta.url), path.join(terminal, file));
    expect(watchTerminalDocument).toBeTypeOf('function');
    generateTerminalDocument(terminal);
    expect(readFileSync(output, 'utf8')).toContain('__hangarTerminalReceive');
    observer = watchTerminalDocument(terminal);
    outputObserver = watch(output, { persistent: false });
    for (const [index, file] of ['terminalPage.js', 'terminal.css'].entries()) {
      const marker = `source-change-${index}`;
      const changed = new Promise<void>(resolve => {
        const onChange = () => {
          if (!readFileSync(output, 'utf8').includes(marker)) return;
          outputObserver!.off('change', onChange);
          resolve();
        };
        outputObserver!.on('change', onChange);
      });
      const source = path.join(terminal, file);
      const target = index === 0 ? source : `${source}.tmp`;
      writeFileSync(target, `${readFileSync(source, 'utf8')}\n/* ${marker} */\n`);
      if (target !== source) renameSync(target, source);
      await changed;
      expect(readFileSync(output, 'utf8').includes(marker)).toBe(true);
      const modified = statSync(output).mtimeMs;
      generateTerminalDocument(terminal);
      expect(statSync(output).mtimeMs).toBe(modified);
    }
    const beforeClose = readFileSync(output, 'utf8');
    const closed = once(observer!, 'close');
    observer!.close();
    await closed;
    const sourceObserver = watch(path.join(terminal, 'terminalPage.js'), { persistent: false });
    try {
      const changed = once(sourceObserver, 'change');
      writeFileSync(path.join(terminal, 'terminalPage.js'), '/* after-close */');
      await changed;
      expect(readFileSync(output, 'utf8')).toBe(beforeClose);
    } finally { sourceObserver.close(); }
  } finally {
    observer?.close();
    outputObserver?.close();
    rmSync(terminal, { recursive: true, force: true });
  }
});

it('observador não prende processo e falhas reais de geração/observação aparecem', () => {
  expect(watchTerminalDocument).toBeTypeOf('function');
  const terminal = mkdtempSync(path.join(tmpdir(), 'hangar-terminal-watch-'));
  try {
    const result = spawnSync(process.execPath, ['-e', `require(${JSON.stringify(builderPath)}).watchTerminalDocument(${JSON.stringify(terminal)})`],
      { encoding: 'utf8', timeout: 5000 });
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(0);
    expect(() => watchTerminalDocument(path.join(terminal, 'missing'))).toThrow(/ENOENT/);
    const failure = spawnSync(process.execPath, ['-e', `
      const fs = require('node:fs');
      require(${JSON.stringify(builderPath)}).watchTerminalDocument(${JSON.stringify(terminal)}).ref();
      fs.writeFileSync(${JSON.stringify(path.join(terminal, 'terminalPage.js'))}, '/* changed */');
    `], { encoding: 'utf8', timeout: 5000 });
    expect(failure.error).toBeUndefined();
    expect(failure.status).not.toBe(0);
    expect(failure.stderr).toContain('ENOENT');
    expect(failure.stderr).toContain('terminal.css');
  } finally { rmSync(terminal, { recursive: true, force: true }); }
});

it('preparação i18n anterior ao tsc gera documento do zero sem Metro/Vitest', () => {
  const fixture = mkdtempSync(path.join(tmpdir(), 'hangar-terminal-prepare-'));
  const mobile = path.join(fixture, 'mobile');
  const terminal = path.join(mobile, 'src/features/terminal');
  try {
    mkdirSync(terminal, { recursive: true });
    mkdirSync(path.join(mobile, 'scripts'));
    const mobileDirectory = path.resolve(path.dirname(builderPath), '..');
    const { scripts } = JSON.parse(readFileSync(path.join(mobileDirectory, 'package.json'), 'utf8'));
    writeFileSync(path.join(mobile, 'package.json'), JSON.stringify({ private: true, scripts }));
    copyFileSync(builderPath, path.join(mobile, 'scripts/build-terminal-document.cjs'));
    for (const file of ['terminalPage.js', 'terminal.css']) copyFileSync(new URL(file, import.meta.url), path.join(terminal, file));
    symlinkSync(path.join(mobileDirectory, 'node_modules'), path.join(mobile, 'node_modules'), 'dir');
    symlinkSync(path.resolve(mobileDirectory, '../messages'), path.join(fixture, 'messages'), 'dir');
    cpSync(path.resolve(mobileDirectory, '../project.inlang'), path.join(fixture, 'project.inlang'), { recursive: true });
    mkdirSync(path.join(fixture, 'scripts'));
    copyFileSync(path.resolve(mobileDirectory, '../scripts/i18n-compile.mjs'), path.join(fixture, 'scripts/i18n-compile.mjs'));
    const output = path.join(terminal, 'terminalDocument.generated.json');
    expect(existsSync(output)).toBe(false);
    const result = spawnSync('npm', ['run', 'i18n:compile'], { cwd: mobile, encoding: 'utf8', timeout: 20000 });
    expect(result.error).toBeUndefined();
    expect(result.status, result.stderr).toBe(0);
    expect(existsSync(output)).toBe(true);
    expect(readFileSync(output, 'utf8').includes('__hangarTerminalReceive')).toBe(true);
  } finally { rmSync(fixture, { recursive: true, force: true }); }
}, 30000);

it('gera HTML autocontido com UMD/CSS/licenças instalados, CSP sem rede e runtime próprio', () => {
  const html = generateTerminalDocument();
  expect(html).toContain('<!doctype html>');
  const document = new DOMParser().parseFromString(html, 'text/html');
  const csp = document.querySelector('meta[http-equiv="Content-Security-Policy"]')!.getAttribute('content');
  for (const rule of ["default-src 'none'", "script-src 'unsafe-inline'", "style-src 'unsafe-inline'",
    "connect-src 'none'", "frame-src 'none'", "form-action 'none'", "base-uri 'none'"]) expect(csp).toContain(rule);
  expect(document.querySelectorAll('script')).toHaveLength(3);
  expect(document.querySelectorAll('script[src],link,iframe,form,base')).toHaveLength(0);
  for (const [index, name] of ['@xterm/xterm', '@xterm/addon-fit'].entries()) {
    const main = require.resolve(name);
    const license = readFileSync(path.resolve(path.dirname(main), '../LICENSE'), 'utf8');
    const script = document.querySelectorAll('script')[index].textContent!;
    expect(script).toContain(license.trim());
    expect(script).toContain(readFileSync(main, 'utf8').replace(/<\/script/gi, '<\\/script'));
  }
  expect(document.querySelector('#terminal.terminal-host')).not.toBeNull();
  expect(document.querySelectorAll('script')[2].textContent).toContain('__hangarTerminalReceive');
  expect(document.querySelector('style')!.textContent).toContain('.xterm-accessibility-tree');
  expect(html).not.toContain('injectedObjectJson');
  const generated = require.resolve('./terminalDocument.generated.json');
  expect(readFileSync(generated, 'utf8')).toBe(`${JSON.stringify(html)}\n`);
  const modified = statSync(generated).mtimeMs;
  expect(generateTerminalDocument()).toBe(html);
  expect(statSync(generated).mtimeMs).toBe(modified);
});

it('prova serve o documento exato sem abrir porta e mantém a ponte fora do HTML de produção', () => {
  const { createProofServer } = require('../../../scripts/terminal-dom-proof.mjs');
  const html = generateTerminalDocument();
  const server = createProofServer(html);
  expect(server).not.toBeNull();
  expect(server.listening).toBe(false);
  const request = (url: string, method = 'GET') => {
    const response = { setHeader: vi.fn(), writeHead: vi.fn(), end: vi.fn() };
    server.emit('request', { url, method }, response);
    return response;
  };
  const raw = request('/document');
  expect(raw.writeHead).toHaveBeenCalledWith(200, { 'Content-Type': 'text/html; charset=utf-8' });
  expect(raw.end).toHaveBeenCalledWith(html);
  const proof = request('/ready');
  const document = new DOMParser().parseFromString(proof.end.mock.calls[0][0], 'text/html');
  expect(document.querySelector('iframe')).not.toBeNull();
  expect(JSON.parse(document.querySelector('#terminal-document')!.textContent!)).toBe(html);
  expect(document.querySelectorAll('script[src],link[src]')).toHaveLength(0);
  expect(request('/document', 'HEAD').end).toHaveBeenCalledWith(undefined);
  for (const url of ['/../secret', '/%2e%2e/secret', '/a\\secret', '/%00']) {
    expect(request(url).writeHead).toHaveBeenCalledWith(403, expect.anything());
  }
  expect(request('/unknown').writeHead).toHaveBeenCalledWith(404, expect.anything());
  expect(request('/ready', 'POST').writeHead).toHaveBeenCalledWith(405, expect.anything());
  server.close();
});
