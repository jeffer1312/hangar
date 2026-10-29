// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import { terminateCodexCommand } from '@hangar/core';
import CodexCommands from './CodexCommands.svelte';
import * as m from '../paraglide/messages';

const server = vi.hoisted(() => ({ id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'token-b' }));
vi.mock('../lib/sessionServer', () => ({ useSessionServer: () => () => server }));
vi.mock('@hangar/core', async original => ({ ...await original<typeof import('@hangar/core')>(), terminateCodexCommand: vi.fn() }));
afterEach(() => { vi.clearAllMocks(); document.body.innerHTML = ''; });

it('abre saída literal, envia o process_id e aguarda o state remover o comando', async () => {
  vi.mocked(terminateCodexCommand).mockResolvedValue({ ok: true });
  const target = document.createElement('div');
  document.body.append(target);
  const command = { id: 'exec-a', process_id: '4242', command: 'npm run dev', started_at: 1, background: true };
  const component = mount(CodexCommands, { target, props: { commands: [command], output: { 'exec-a': '<script>literal</script>' }, sessionName: 'sess' } });
  try {
    target.querySelector<HTMLButtonElement>('.open')!.click();
    await tick();
    expect(target.querySelector('pre')?.textContent).toBe('<script>literal</script>');
    expect(target.querySelector('script')).toBeNull();
    target.querySelector<HTMLButtonElement>('.stop')!.click();
    await tick(); await Promise.resolve(); await tick();
    expect(terminateCodexCommand).toHaveBeenCalledWith('sess', '4242', server);
    expect(target.textContent).toContain(command.command);
    expect(target.querySelector<HTMLButtonElement>('.stop')?.disabled).toBe(false);
    expect(target.querySelector('.stop')?.textContent).toContain(m.codex_command_stop());
  } finally { await unmount(component); }
});

it('aguarda cada pedido sem bloquear o encerramento dos outros comandos', async () => {
  let finishA!: (value: { ok: true }) => void;
  let finishB!: (value: { ok: true }) => void;
  vi.mocked(terminateCodexCommand)
    .mockImplementationOnce(() => new Promise(resolve => { finishA = resolve; }))
    .mockImplementationOnce(() => new Promise(resolve => { finishB = resolve; }));
  const target = document.createElement('div');
  const commands = [
    { id: 'exec-a', process_id: '4242', command: 'npm run dev', started_at: 1, background: true },
    { id: 'exec-b', process_id: '4243', command: 'npm run watch', started_at: 2, background: true },
  ];
  const component = mount(CodexCommands, { target, props: { commands, output: {}, sessionName: 'sess' } });
  const buttons = () => [...target.querySelectorAll<HTMLButtonElement>('.stop')];
  try {
    buttons()[0].click(); await tick();
    expect(buttons().map(button => button.disabled)).toEqual([true, false]);
    buttons()[1].click(); await tick();
    expect(buttons().map(button => button.disabled)).toEqual([true, true]);
    finishA({ ok: true }); await Promise.resolve(); await tick();
    expect(buttons().map(button => button.disabled)).toEqual([false, true]);
    finishB({ ok: true }); await Promise.resolve(); await tick();
    expect(buttons().map(button => button.disabled)).toEqual([false, false]);
    expect(terminateCodexCommand).toHaveBeenCalledTimes(2);
  } finally { await unmount(component); }
});

it('mostra a recusa e permite tentar novamente', async () => {
  vi.mocked(terminateCodexCommand).mockRejectedValue(new Error('processo não encontrado'));
  const target = document.createElement('div');
  const component = mount(CodexCommands, { target, props: {
    commands: [{ id: 'exec-a', process_id: '4242', command: 'npm run dev', started_at: 1, background: true }],
    output: {}, sessionName: 'sess',
  } });
  try {
    target.querySelector<HTMLButtonElement>('.stop')!.click();
    await tick(); await Promise.resolve(); await tick();
    expect(target.querySelector('[role="alert"]')?.textContent).toBe('processo não encontrado');
    expect(target.querySelector<HTMLButtonElement>('.stop')?.disabled).toBe(false);
  } finally { await unmount(component); }
});
