// @vitest-environment happy-dom
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import Composer from './Composer.svelte';
import { configureApi, transcribeUploaded, uploadFile } from '@hangar/core';
import { dictations } from '../lib/dictationStore.svelte';
import { overwriteGetLocale } from '../paraglide/runtime';

vi.hoisted(() => {
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key), clear: () => values.clear(),
    key: (index: number) => [...values.keys()][index] ?? null, get length() { return values.size; },
  } });
});

vi.mock('@hangar/core', async (original) => ({
  ...(await original<typeof import('@hangar/core')>()),
  getPermissionModes: vi.fn(async () => ({ current: 'plan', modes: ['plan'] })),
  getCommands: vi.fn(async () => []), getModelOptions: vi.fn(async () => []),
  uploadFile: vi.fn(), transcribeUploaded: vi.fn(),
}));
vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: { epoca: () => 0, retain: vi.fn(), release: vi.fn(), sessionsForServer: () => [] },
}));
vi.mock('../lib/imagePrep', () => ({ prepareImage: async (file: File) => file }));

class Recorder {
  static last: Recorder;
  state = 'inactive'; mimeType = 'audio/webm';
  ondataavailable?: (event: { data: Blob }) => void;
  onstop?: () => void; onerror?: () => void;
  stop = vi.fn(() => { this.state = 'inactive'; });
  constructor() { Recorder.last = this; }
  start() { this.state = 'recording'; }
  finish() {
    this.ondataavailable?.({ data: new Blob(['último trecho'], { type: this.mimeType }) });
    this.onstop?.();
  }
}
const stopTrack = vi.fn();
const stream = { getTracks: () => [{ stop: stopTrack }] } as unknown as MediaStream;
let app: ReturnType<typeof mount> | undefined;
let target: HTMLDivElement;
let resolveText: (value: { path: string; text: string; raw: string }) => void;
let rejectText: (error: Error) => void;

beforeEach(() => {
  overwriteGetLocale(() => 'pt');
  localStorage.clear(); dictations._resetForTests(); vi.clearAllMocks();
  vi.stubGlobal('MediaRecorder', Recorder);
  Object.defineProperty(navigator, 'mediaDevices', { configurable: true, value: { getUserMedia: vi.fn(async () => stream) } });
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{}', { status: 200 }));
  vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:fixture');
  vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
  configureApi({ getBaseUrl: () => 'http://fixture', getToken: () => 'fixture', onUnauthorized: () => {}, origin: null,
    createEventSource: () => { throw new Error('SSE inesperado'); } });
  vi.mocked(uploadFile).mockImplementation(async (_name, file) => ({ path: `/uploads/${file.name}` }));
  vi.mocked(transcribeUploaded).mockReturnValue(new Promise((resolve, reject) => { resolveText = resolve; rejectText = reject; }));
  target = document.createElement('div'); document.body.appendChild(target);
});
afterEach(async () => { if (app) await unmount(app); app = undefined; target.remove(); dictations._resetForTests(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function render(draft = '') {
  const props = $state({ sessionName: 'recording-send', sessionState: 'idle' as const, status: null,
    sessionJsonl: 'transcript', inputText: draft, pairPeers: ['par'] as string[] | null, sendToPair: false, onSend: vi.fn(), onCommand: vi.fn(),
    onInterrupt: vi.fn(), onOpenGit: vi.fn(), onOpenPreview: vi.fn() });
  app = mount(Composer, { target, props }); return props;
}
async function start() {
  await tick(); target.querySelector<HTMLButtonElement>('.mic-btn')!.click();
  await vi.waitFor(() => expect(Recorder.last.state).toBe('recording'));
  await tick();
}
function send() { target.querySelector<HTMLButtonElement>('.send-btn')!.click(); }

describe('envio encerra o microfone no PWA', () => {
  it.each(['', 'texto digitado'])('espera o último trecho e envia uma vez com o rascunho %j', async (draft) => {
    const props = render(draft); await start(); send(); send();
    expect(Recorder.last.stop).toHaveBeenCalledTimes(1);
    expect(props.onSend).not.toHaveBeenCalled();
    Recorder.last.finish();
    await vi.waitFor(() => expect(transcribeUploaded).toHaveBeenCalledOnce());
    expect(stopTrack).toHaveBeenCalledOnce();
    const file = vi.mocked(uploadFile).mock.calls[0][1];
    expect(file.size).toBe(new Blob(['último trecho']).size);
    resolveText({ path: '/uploads/audio.webm', text: 'fala transcrita', raw: 'fala transcrita' });
    await vi.waitFor(() => expect(props.onSend).toHaveBeenCalledExactlyOnceWith(draft ? `${draft} fala transcrita` : 'fala transcrita', false));
  });

  it('imagem e texto seguem juntos depois da transcrição', async () => {
    const props = render('legenda'); await tick();
    const event = new Event('paste', { bubbles: true, cancelable: true });
    const file = new File(['imagem'], 'foto.png', { type: 'image/png' });
    Object.defineProperty(event, 'clipboardData', { value: { items: [{ kind: 'file', type: file.type, getAsFile: () => file }] } });
    target.querySelector('textarea')!.dispatchEvent(event); await tick();
    await start(); send(); expect(Recorder.last.stop).toHaveBeenCalledOnce();
    Recorder.last.finish(); await vi.waitFor(() => expect(transcribeUploaded).toHaveBeenCalledOnce());
    resolveText({ path: '/uploads/audio.webm', text: 'fala transcrita', raw: 'fala transcrita' });
    await vi.waitFor(() => expect(props.onSend).toHaveBeenCalledOnce());
    expect(props.onSend.mock.calls[0][0]).toContain('legenda fala transcrita');
    expect(props.onSend.mock.calls[0][0]).toContain('/uploads/foto.png');
  });

  it('falha mantém o rascunho e não envia mensagem incompleta', async () => {
    const props = render('rascunho'); await start(); send();
    expect(Recorder.last.stop).toHaveBeenCalledOnce(); Recorder.last.finish();
    await vi.waitFor(() => expect(transcribeUploaded).toHaveBeenCalledOnce());
    rejectText(new Error('serviço indisponível'));
    await vi.waitFor(() => expect(target.textContent).toContain('serviço indisponível'));
    expect(props.onSend).not.toHaveBeenCalled(); expect(target.querySelector('textarea')!.value).toBe('rascunho');
  });

  it('espera uma transcrição já em andamento ao enviar um arquivo', async () => {
    const props = render('legenda'); await tick();
    const event = new Event('paste', { bubbles: true, cancelable: true });
    const file = new File(['arquivo'], 'nota.txt', { type: 'text/plain' });
    Object.defineProperty(event, 'clipboardData', { value: { items: [{ kind: 'file', type: file.type, getAsFile: () => file }] } });
    target.querySelector('textarea')!.dispatchEvent(event); await tick();
    dictations.start({ serverId: '', name: 'recording-send', jsonl: 'transcript', server: undefined,
      file: new File(['fala'], 'fala.m4a', { type: 'audio/mp4' }), opts: { ditado: true, autoEnvio: false } });
    await vi.waitFor(() => expect(transcribeUploaded).toHaveBeenCalledOnce()); await tick(); send();
    expect(props.onSend).not.toHaveBeenCalled();
    resolveText({ path: '/uploads/fala.m4a', text: 'fala transcrita', raw: 'fala transcrita' });
    await vi.waitFor(() => expect(props.onSend).toHaveBeenCalledOnce());
    expect(props.onSend.mock.calls[0][0]).toContain('legenda fala transcrita');
    expect(props.onSend.mock.calls[0][0]).toContain('/uploads/nota.txt');
  });

  it('edição durante a espera cancela o envio e conserva a transcrição no campo', async () => {
    const props = render('rascunho'); await start(); send(); Recorder.last.finish();
    await vi.waitFor(() => expect(transcribeUploaded).toHaveBeenCalledOnce());
    props.inputText = 'texto editado'; await tick();
    resolveText({ path: '/uploads/audio.webm', text: 'fala transcrita', raw: 'fala transcrita' });
    await vi.waitFor(() => expect(target.textContent).toContain('Confira o texto antes de enviar'));
    expect(props.onSend).not.toHaveBeenCalled(); expect(target.querySelector('textarea')!.value).toContain('fala transcrita');
    expect(target.querySelector('textarea')!.value).toContain('texto editado');
  });

  it('envio invalida uma permissão de microfone que chegar atrasada', async () => {
    let allow!: (value: MediaStream) => void;
    vi.mocked(navigator.mediaDevices.getUserMedia).mockReturnValue(new Promise((resolve) => { allow = resolve; }));
    const props = render('mensagem'); await tick();
    target.querySelector<HTMLButtonElement>('.mic-btn')!.click(); await tick(); send();
    await vi.waitFor(() => expect(props.onSend).toHaveBeenCalledExactlyOnceWith('mensagem', false));
    allow(stream); await vi.waitFor(() => expect(stopTrack).toHaveBeenCalledOnce());
    expect(target.querySelector('.mic-btn--recording')).toBeNull();
  });

  it.each(['toggle', 'members'])('mudança de destinatários %s cancela o envio pendente', async (change) => {
    const props = render('rascunho'); await start(); send(); Recorder.last.finish();
    await vi.waitFor(() => expect(transcribeUploaded).toHaveBeenCalledOnce());
    if (change === 'toggle') props.sendToPair = true;
    else props.pairPeers = ['outro par'];
    await tick();
    resolveText({ path: '/uploads/audio.webm', text: 'fala transcrita', raw: 'fala transcrita' });
    await vi.waitFor(() => expect(target.textContent).toContain('Confira o texto antes de enviar'));
    expect(props.onSend).not.toHaveBeenCalled();
  });
});
