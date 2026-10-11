// @vitest-environment happy-dom
// @vitest-environment-options {"settings":{"navigation":{"disableChildFrameNavigation":true}}}
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import { configureApi, parseFilePaths } from '@hangar/core';
import FileAttachment from './FileAttachment.svelte';
import { saveFile } from '../lib/saveFile';
import * as m from '../paraglide/messages';

vi.mock('../lib/saveFile', () => ({ saveFile: vi.fn(async () => 'saved') }));

let component: ReturnType<typeof mount> | undefined;
let target: HTMLDivElement;

beforeEach(() => {
  configureApi({ getBaseUrl: () => 'https://desktop.example.ts.net', getToken: () => 'test-token',
    onUnauthorized: () => {}, origin: 'https://pwa.example',
    createEventSource: () => { throw new Error('Unexpected stream'); } });
  target = document.createElement('div');
  document.body.append(target);
  vi.mocked(saveFile).mockClear();
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  target.remove();
});

describe('documentos citados', () => {
  it('DOCX oferece download autenticado no servidor da conversa, sem editor ou iframe', async () => {
    component = mount(FileAttachment, { target, props: {
      sessionName: 'sessão', refs: parseFilePaths('/tmp/Relatório final.docx'),
    } });
    await tick();
    const buttons = target.querySelectorAll<HTMLButtonElement>('button');
    expect(buttons).toHaveLength(1);
    expect(buttons[0].getAttribute('aria-label')).toBe(m.anexos_baixar({ nome: 'Relatório final.docx' }));
    expect(target.querySelector('iframe')).toBeNull();
    buttons[0].click();
    await tick();
    const [href, name] = vi.mocked(saveFile).mock.calls[0];
    const url = new URL(href);
    expect(url.origin).toBe('https://desktop.example.ts.net');
    expect(url.searchParams.get('download')).toBe('1');
    expect(url.searchParams.get('token')).toBe('test-token');
    expect(url.searchParams.get('path')).toBe('/tmp/Relatório final.docx');
    expect(name).toBe('Relatório final.docx');
  });

  it('PDF mantém a prévia e permite baixar tanto no cartão quanto no visor', async () => {
    component = mount(FileAttachment, { target, props: {
      sessionName: 's', refs: parseFilePaths('/tmp/relatorio.pdf'),
    } });
    await tick();
    target.querySelector<HTMLButtonElement>('button.att-download')!.click();
    await tick();
    expect(vi.mocked(saveFile).mock.calls[0]?.[0]).toContain('download=1');
    target.querySelector<HTMLButtonElement>('button.att-chip')!.click();
    await tick();
    const dialog = document.querySelector('[role="dialog"]')!;
    expect(dialog.querySelector('iframe')?.getAttribute('src')).not.toContain('download=');
    const label = m.anexos_baixar({ nome: 'relatorio.pdf' });
    dialog.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!.click();
    await tick();
    expect(vi.mocked(saveFile).mock.calls[1]?.[0]).toContain('download=1');
  });
});
