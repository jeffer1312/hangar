import { beforeEach, expect, it, vi } from 'vitest';

const disk = vi.hoisted(() => new Map<string, string>());
const storage = vi.hoisted(() => ({ fail: false }));
vi.mock('../stores/prefs', () => ({ prefs: {
  getString: (key: string) => disk.get(key),
  set: (key: string, value: string) => { if (storage.fail) throw Error('storage-failed'); disk.set(key, value); },
} }));

beforeEach(() => { disk.clear(); storage.fail = false; vi.resetModules(); });

async function boot() {
  const errors = await import('./fatalErrors');
  let handler = (_error: unknown, _fatal?: boolean) => {};
  let decorator: ((data: any) => any) | undefined;
  let listener: ((data: any) => void) | undefined;
  const original = vi.fn((_error: unknown, _fatal?: boolean) => {});
  const runtime = {
    errorUtils: { getGlobalHandler: () => original, setGlobalHandler: (next: typeof handler) => { handler = next; } },
    exceptions: { unstable_setExceptionDecorator: (next: typeof decorator) => { decorator = next; } },
    registerExceptionListener: (next: typeof listener) => { listener = next; },
    metadata: { platform: 'ios', version: '0.1.0', runtime: 'native-2', updateId: 'ota-one' },
  };
  errors.installFatalErrorCapture(runtime);
  return { errors, runtime, original, report: (error: unknown, fatal = true) => handler(error, fatal),
    decorate: (data: any) => decorator?.(data), native: (data: any) => listener?.(data) };
}

it('grava antes de encadear o fatal original e recupera após reiniciar o JS', async () => {
  const app = await boot();
  app.errors.setFatalErrorRoute(['s', '[server]', '[name]', 'activity']);
  const error = Object.assign(new TypeError("Cannot read properties of undefined (reading 'map')"), {
    stack: 'TypeError: private-message\n    at SubagentLive (file:///private/app/main.jsbundle:1:9234)',
    componentStack: '\n    at Activity (file:///private/app/main.jsbundle:1:4567)',
  });
  app.original.mockImplementation(() => {
    expect(app.errors.readFatalError()?.route).toBe('/s/[server]/[name]/activity');
    throw Error('native-abort');
  });
  expect(() => app.report(error)).toThrow('native-abort');
  expect(app.original).toHaveBeenCalledWith(error, true);
  vi.resetModules();
  const recovered = (await import('./fatalErrors')).readFatalError();
  expect(recovered).toMatchObject({ name: 'TypeError', metadata: { runtime: 'native-2', updateId: 'ota-one' },
    stack: '    at SubagentLive (main.jsbundle:1:9234)', componentStack: '    at Activity (main.jsbundle:1:4567)' });
  expect(recovered?.message).toContain('Cannot read properties of undefined');
  expect(JSON.stringify(recovered)).not.toContain('private-message');
});

it('captura fatal do React e da esteira nativa sem alterar o relatório original', async () => {
  const app = await boot();
  const data = { isFatal: true, name: 'Error', message: 'Maximum update depth exceeded.',
    componentStack: '\n    at Activity (main.jsbundle:1:45)',
    stack: [{ methodName: 'Activity', file: 'file:///private/main.jsbundle', lineNumber: 1, column: 45 }] };
  expect(app.decorate(data)).toBe(data);
  expect(app.errors.readFatalError()?.message).toBe('Maximum update depth exceeded.');
  vi.resetModules();
  const second = await boot();
  second.native(data);
  expect(second.errors.readFatalError()?.stack).toContain('Activity@main.jsbundle:1:45');
});

it('ignora erro não fatal e instala uma vez', async () => {
  const app = await boot();
  const set = vi.spyOn(app.runtime.errorUtils, 'setGlobalHandler');
  app.errors.installFatalErrorCapture(app.runtime);
  expect(set).not.toHaveBeenCalled();
  app.report(Error('nonfatal'), false);
  expect(app.original).toHaveBeenCalledOnce();
  expect(app.errors.readFatalError()).toBeNull();
});

it('preserva o original mesmo quando o armazenamento falha', async () => {
  const app = await boot();
  storage.fail = true;
  const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
  try {
    app.report(Error('failure'));
    expect(app.original).toHaveBeenCalledOnce();
    expect(warn).toHaveBeenCalledWith('fatal_error_capture_failed');
  } finally { warn.mockRestore(); }
});

it('remove credenciais, caminho privado, query, dados interpolados e mensagem desconhecida', async () => {
  const app = await boot();
  const error = Error('Cannot read "texto privado da conversa" token=super-secret https://server.test/s/private?token=other-secret');
  error.stack = `${error.message}\nfn@https://server.test/private/index.bundle?token=other-secret:1:99\nBearer super-secret`;
  app.report(error);
  const text = JSON.stringify(app.errors.readFatalError());
  for (const secret of ['texto privado', 'super-secret', 'other-secret', 'server.test', '/private/']) expect(text).not.toContain(secret);
  expect(text).toContain('index.bundle:1:99');
  vi.resetModules();
  const next = await boot();
  next.report(Error('texto livre de uma conversa'));
  expect(next.errors.readFatalError()?.message).toBe('[redacted]');
});

it('reenvia ao diário após reiniciar e só confirma após aceitação, sem apagar o relatório', async () => {
  const app = await boot();
  app.report(new TypeError('Cannot read property of undefined'));
  vi.resetModules();
  const recovered = await import('./fatalErrors');
  const send = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
  await recovered.replayFatalError(send);
  await recovered.replayFatalError(send);
  await recovered.replayFatalError(send);
  expect(send).toHaveBeenCalledTimes(2);
  expect(send.mock.calls[0][0]).toMatchObject({ name: 'TypeError', route: '/startup' });
  expect(recovered.readFatalError()).not.toBeNull();
});

it('registra um novo fatal mesmo depois de outro erro fatal tratado pelo ambiente de desenvolvimento', async () => {
  const app = await boot();
  app.report(new TypeError('Cannot read property of undefined'));
  app.errors.setFatalErrorRoute(['config', 'sobre']);
  app.report(new RangeError('Maximum call stack size exceeded'));
  expect(app.errors.readFatalError()).toMatchObject({ name: 'RangeError', route: '/config/sobre' });
});

it('não vaza dados anexados depois de posições da pilha ou de um diagnóstico conhecido', async () => {
  const app = await boot();
  const error = Error('Cannot read properties of undefined (reading \'map\'): texto da conversa');
  error.stack = 'Error: texto da conversa\n    at Activity (main.jsbundle:1:99) texto da conversa\nfn@main.jsbundle:1:40 token=secret';
  app.report(error);
  const report = JSON.stringify(app.errors.readFatalError());
  expect(report).not.toContain('texto da conversa');
  expect(report).not.toContain('secret');
  expect(report).toContain('main.jsbundle:1:99');
});

it('não confirma um erro novo enquanto envia o relatório anterior', async () => {
  const app = await boot();
  app.report(new TypeError('Cannot read property of undefined'));
  let done!: (ok: boolean) => void;
  const send = vi.fn(() => new Promise<boolean>(resolve => { done = resolve; }));
  const pending = app.errors.replayFatalError(send);
  await app.errors.replayFatalError(send);
  const saved = JSON.parse(disk.get('diagnostics.lastFatalError')!);
  saved.report.timestamp = '2099-01-01T00:00:00Z';
  disk.set('diagnostics.lastFatalError', JSON.stringify(saved));
  done(true); await pending;
  expect(send).toHaveBeenCalledOnce();
  expect(JSON.parse(disk.get('diagnostics.lastFatalError')!).pending).toBe(true);
});

it.each([
  ['ReferenceError', "Property 'private-key' doesn't exist", 'Property [redacted] doesn\'t exist'],
  ['RangeError', 'Out of memory', 'Out of memory'],
  ['TypeError', 'target.render is not a function (it is undefined)', '[redacted] is not a function'],
])('preserva a causa de %s do Hermes sem o valor variável', async (name, message, expected) => {
  const app = await boot();
  app.report(Object.assign(Error(message), { name }));
  expect(app.errors.readFatalError()?.message).toBe(expected);
});

it('preserva posições do bytecode Hermes sem nome de arquivo', async () => {
  const app = await boot();
  const error = Error('Out of memory');
  error.stack = 'RangeError: private-text\nSubagentLive@1:9234\nglobal code@1:4567';
  app.report(error);
  expect(app.errors.readFatalError()?.stack).toBe('SubagentLive@1:9234\nglobal code@1:4567');
});

it('preserva a pilha do bundle de OTA, que no iOS mora em Application Support', async () => {
  const app = await boot();
  const ota = '/var/mobile/Containers/Data/Application/AB12/Library/Application Support/.expo-internal/01a1748f.hbc';
  const error = Error('Out of memory');
  error.stack = `Error: private-text\n    at Chat (address at ${ota}:1:9234)\nSubagentLive@${ota}:1:4567\n    at Activity (file://${ota}:1:45)`;
  app.report(error);
  const report = app.errors.readFatalError();
  expect(report?.stack).toBe('    at Chat (address at 01a1748f.hbc:1:9234)\nSubagentLive@01a1748f.hbc:1:4567\n    at Activity (01a1748f.hbc:1:45)');
  expect(JSON.stringify(report)).not.toContain('Containers');
});

it.each(['Rendered more hooks texto da conversa.', "Couldn't find the header height texto da conversa.", 'Invalid hook call texto da conversa.'])('não aceita texto livre depois de um prefixo conhecido: %s', async message => {
  const app = await boot();
  app.report(Error(message));
  expect(app.errors.readFatalError()?.message).toBe('[redacted]');
});

it('avisa uma vez por erro e de novo quando surge outro', async () => {
  const app = await boot();
  expect(app.errors.takeFatalErrorNotice()).toBe(false);
  app.decorate({ isFatal: true, name: 'Error', message: 'Out of memory' });
  expect(app.errors.takeFatalErrorNotice()).toBe(true);
  expect(app.errors.takeFatalErrorNotice()).toBe(false);
  expect(app.errors.readFatalError()?.message).toBe('Out of memory');
  app.decorate({ isFatal: true, name: 'Error', message: 'Maximum update depth exceeded.' });
  expect(app.errors.takeFatalErrorNotice()).toBe(true);
});
