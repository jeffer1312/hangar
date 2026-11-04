// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, expect, it, vi } from 'vitest';
import { terminateCodexCommand, type CodexCommand } from '@hangar/core';
import { CodexCommands } from './CodexCommands';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
vi.mock('@hangar/core', async original => ({ ...await original<typeof import('@hangar/core')>(), terminateCodexCommand: vi.fn() }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
const chat = vi.hoisted(() => ({ commandOutput: {} as Record<string, string> }));
vi.mock('../../stores/chat', () => ({ chatStore: () => ({ use: (select: (state: typeof chat) => unknown) => select(chat) }) }));
const server = { id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'token-b' };
const command: CodexCommand = { id: 'exec-a', process_id: '4242', command: 'npm run dev', started_at: 1, background: true };
const roots: Root[] = [];
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  vi.clearAllMocks();
});
async function mount() {
  const target = document.createElement('div');
  const root = createRoot(target);
  roots.push(root);
  const render = (commands = [command], output: Record<string, string> = { 'exec-a': '<script>literal</script>' }, readOnly = false) => {
    chat.commandOutput = output;
    return act(async () => root.render(createElement(CodexCommands, { commands, server, name: 'sess', serverId: 'b', readOnly })));
  };
  await render();
  return { target, render };
}

it('mostra a saída literal atualizada e não remove o comando só porque o POST respondeu', async () => {
  vi.mocked(terminateCodexCommand).mockResolvedValue({ ok: true });
  const { target, render } = await mount();
  const buttons = () => [...target.querySelectorAll<HTMLButtonElement>('button')];
  await act(async () => buttons()[0].click());
  expect(target.textContent).toContain('<script>literal</script>');
  expect(target.querySelector('script')).toBeNull();
  await render([command], { 'exec-a': 'saída nova' });
  expect(target.textContent).toContain('saída nova');
  await act(async () => buttons()[1].click());
  expect(terminateCodexCommand).toHaveBeenCalledWith('sess', '4242', server);
  expect(target.textContent).toContain(command.command);
  expect(buttons()[1].disabled).toBe(false);
  await render([]);
  expect(target.textContent).toBe('');
});

it('libera cada pedido sem bloquear os outros comandos', async () => {
  let finishA!: (value: { ok: true }) => void;
  let finishB!: (value: { ok: true }) => void;
  vi.mocked(terminateCodexCommand)
    .mockImplementationOnce(() => new Promise(resolve => { finishA = resolve; }))
    .mockImplementationOnce(() => new Promise(resolve => { finishB = resolve; }));
  const { target, render } = await mount();
  await render([command, { ...command, id: 'exec-b', process_id: '4243', command: 'npm run watch' }]);
  const buttons = () => [...target.querySelectorAll<HTMLButtonElement>('button')].filter((_, index) => index % 2 === 1);
  await act(async () => buttons()[0].click());
  expect(buttons().map(button => button.disabled)).toEqual([true, false]);
  await act(async () => buttons()[1].click());
  expect(buttons().map(button => button.disabled)).toEqual([true, true]);
  await act(async () => finishA({ ok: true }));
  expect(buttons().map(button => button.disabled)).toEqual([false, true]);
  await act(async () => finishB({ ok: true }));
  expect(buttons().map(button => button.disabled)).toEqual([false, false]);
  expect(terminateCodexCommand).toHaveBeenCalledTimes(2);
});

it('mantém o comando e expõe a falha ao encerrar', async () => {
  vi.mocked(terminateCodexCommand).mockRejectedValue(new Error('processo não encontrado'));
  const { target } = await mount();
  await act(async () => target.querySelectorAll<HTMLButtonElement>('button')[1].click());
  expect(target.querySelector('[role="alert"]')?.textContent).toBe('processo não encontrado');
  expect(target.textContent).toContain(command.command);
  expect(target.querySelectorAll<HTMLButtonElement>('button')[1].disabled).toBe(false);
});

it('permite ler a saída sem oferecer Encerrar em sessão somente leitura', async () => {
  const { target, render } = await mount();
  await render([command], { 'exec-a': 'saída' }, true);
  expect(target.querySelectorAll('button')).toHaveLength(1);
  await act(async () => target.querySelector<HTMLButtonElement>('button')!.click());
  expect(target.textContent).toContain('saída');
  expect(terminateCodexCommand).not.toHaveBeenCalled();
});
