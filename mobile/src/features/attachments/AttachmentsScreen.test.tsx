// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, type ChatEvent, type Server, type UploadFile } from '@hangar/core';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const h = vi.hoisted(() => ({ list: vi.fn(), resolve: vi.fn(), dictate: vi.fn(), back: vi.fn(), push: vi.fn(), toast: vi.fn(),
  audio: vi.fn(), doc: vi.fn(), lightbox: vi.fn(), download: vi.fn(), deliverLocal: vi.fn(), kept: vi.fn(),
  events: [] as ChatEvent[] }));
const servers = vi.hoisted(() => ({ ready: true, activeId: 's2', servers: [
  { id: 's1', label: 'A', baseUrl: 'https://a.test', token: 'token-a' },
  { id: 's2', label: 'B', baseUrl: 'https://b.test', token: 'token-b' },
] as Server[] }));
vi.mock('expo-router', () => ({
  useLocalSearchParams: () => ({ server: 's1', name: 'sess' }),
  useRouter: () => ({ push: h.push, back: h.back }),
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  listUploads: h.list,
  resolverCitados: h.resolve,
}));
vi.mock('../../stores/servers', () => ({
  useServers: Object.assign((select: (s: typeof servers) => unknown) => select(servers), { getState: () => servers }),
}));
vi.mock('../../stores/chat', () => ({
  chatStore: () => ({ use: (select: (s: { events: ChatEvent[] }) => unknown) => select({ events: h.events }) }),
}));
vi.mock('../../stores/sessions', () => {
  const state = { rows: [{ serverId: 's1', name: 'sess', jsonl: '/t/a.jsonl' }], byServerRecord: {} };
  return { useSessions: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) };
});
vi.mock('../ditado/ditadoEstiloStore', () => ({ dictationStyleStore: (server: Server) => ({
  getState: () => ({ pronto: true, valor: server.id === 's1' ? 'prosa' : 'briefing' }),
}) }));
vi.mock('../ditado/dictationRun', () => ({
  dictateUpload: h.dictate,
  DictationError: class extends Error {
    constructor(message: string, readonly lost: boolean, readonly storageIssue: string) { super(message); }
  },
}));
vi.mock('../../chat/AudioChip', () => ({ AudioChip: (props: { name: string; uri: string; headers?: Record<string, string>; meta?: string }) => {
  h.audio(props);
  return createElement('span', null, `audio:${props.name}|${props.meta ?? ''}`);
} }));
type LightboxMockProps = { visible: boolean; uri: string; headers?: Record<string, string>; filename: string; kind?: string;
  counter?: string; onStep?: (dir: 1 | -1) => void; details?: string; onShowInChat?: () => void };
vi.mock('./Lightbox', () => ({ Lightbox: (props: LightboxMockProps) => {
  h.lightbox(props);
  return props.visible ? createElement('div', { 'data-lightbox': props.kind ?? 'image' }, `${props.counter ?? ''}|${props.filename}`) : null;
} }));
vi.mock('./mediaCache', () => ({ downloadKept: h.download, deliverLocalFile: h.deliverLocal, keptCopy: h.kept }));
vi.mock('./DocumentViewer', () => ({
  DocumentViewer: ({ doc }: { doc: { kind: string; name: string; uri: string; headers?: Record<string, string> } | null }) => {
    h.doc(doc);
    return doc ? createElement('div', { 'data-doc': doc.kind }, doc.name) : null;
  },
}));
vi.mock('../../ui/Toast', () => ({ toast: { erro: h.toast, ok: () => {} } }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../theme/superficie', () => ({ superficie: () => '#eee' }));
// Cada chave devolve o próprio nome, com os parâmetros depois de ":".
vi.mock('../../paraglide/messages', () => Object.fromEntries(
  ('comum_carregando anexos_erro_listar anexos_nenhum lista_tentar_novamente composer_transcrever_de_novo '
    + 'sessao_expirada chat_servidor_removido composer_falha_transcricao file_card_empty file_card_no_app file_card_missing '
    + 'file_card_failed file_card_open file_card_download file_card_downloading anexos_ver anexos_tocar anexos_visualizar '
    + 'anexos_imagem_enviada anexos_sem_expiracao anexos_vencido anexos_expira_h anexos_expira_d media_title media_tab_media '
    + 'media_tab_files media_tab_audio media_filter_all media_filter_agent media_filter_user media_origin_agent media_origin_user '
    + 'media_empty media_day_today media_day_yesterday media_day_undated media_when_today media_when_yesterday media_when_date '
    + 'media_folder_uploads media_cancel_download media_again media_downloading_pct media_downloading_pct_of media_downloaded '
    + 'media_download_failed gallery_counter')
    .split(' ').map((k) => [k, (p?: Record<string, unknown>) => (p ? `${k}:${Object.values(p).join(',')}` : k)]),
));

import AttachmentsScreen from '../../../app/s/[server]/[name]/attachments';
import { useChatJump } from '../../stores/chatJump';

const file = (filename: string, mtime = 0): UploadFile => ({ filename, size: 10, mtime, expires_in_days: null });
async function render() {
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(createElement(AttachmentsScreen)));
  return { container, root };
}
const find = (c: HTMLElement, label: string) => [...c.querySelectorAll<HTMLElement>('[aria-label], button, [role="tab"], [role="button"]')]
  .find((el) => el.getAttribute('aria-label') === label || el.textContent === label);
const click = async (c: HTMLElement, label: string) => {
  const el = find(c, label);
  if (!el) throw new Error(`missing: ${label}`);
  await act(async () => el.click());
};
const lastLightbox = () => h.lightbox.mock.lastCall![0] as LightboxMockProps;

describe('media and files', () => {
  beforeEach(() => {
    vi.clearAllMocks(); h.list.mockReset(); h.dictate.mockReset(); h.download.mockReset(); h.deliverLocal.mockReset();
    h.kept.mockReturnValue(null);
    h.resolve.mockResolvedValue({ ok: {}, faltam: [] });
    h.events = [];
    servers.ready = true;
    servers.servers = [
      { id: 's1', label: 'A', baseUrl: 'https://a.test', token: 'token-a' },
      { id: 's2', label: 'B', baseUrl: 'https://b.test', token: 'token-b' },
    ];
    useChatJump.setState({ request: null });
    configureApi({ getBaseUrl: () => 'https://b.test', getToken: () => 'token-b', onUnauthorized: () => {},
      origin: null, createEventSource: () => { throw new Error('unexpected SSE'); } });
  });

  it('loading, empty, error with retry and list', async () => {
    let finish!: (v: { files: UploadFile[] }) => void;
    h.list.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const first = await render();
    expect(first.container.textContent).toContain('comum_carregando');
    await act(async () => finish({ files: [] }));
    expect(first.container.textContent).toContain('anexos_nenhum');
    act(() => first.root.unmount());

    h.list.mockRejectedValueOnce(new Error('caiu')).mockResolvedValueOnce({ files: [file('foto.png')] });
    const second = await render();
    expect(second.container.textContent).toContain('anexos_erro_listar caiu');
    await click(second.container, 'lista_tentar_novamente');
    expect(find(second.container, 'anexos_ver:foto.png')).toBeDefined();
    expect(second.container.textContent).toContain('media_title');
    act(() => second.root.unmount());
  });

  it('audio plays in the list and "transcribe again" sends to the chat and goes back', async () => {
    h.list.mockResolvedValueOnce({ files: [file('ditado-1.m4a'), file('clip.mp4')] });
    h.dictate.mockReturnValueOnce(Promise.resolve());
    const { container, root } = await render();
    await click(container, 'media_tab_audio:1');
    expect(container.textContent).toContain('audio:ditado-1.m4a');
    await click(container, 'composer_transcrever_de_novo: ditado-1.m4a');
    expect(h.dictate).toHaveBeenCalledExactlyOnceWith(servers.servers[0], 's1', 'sess', '/t/a.jsonl', 'ditado-1.m4a', 'prosa');
    expect(h.back).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it('a failure not kept in the chat becomes a toast; a kept one does not', async () => {
    const { DictationError } = await import('../ditado/dictationRun');
    h.list.mockResolvedValue({ files: [file('ditado-1.m4a')] });
    h.dictate
      .mockRejectedValueOnce(new DictationError('502: fora', false, ''))
      .mockRejectedValueOnce(new DictationError('502: fora', true, ''))
      .mockRejectedValueOnce(new DictationError('502: fora', false, 'disco cheio'));
    const { container, root } = await render();
    await click(container, 'media_tab_audio:1');
    const botao = () => click(container, 'composer_transcrever_de_novo: ditado-1.m4a');
    await botao();
    expect(h.toast).not.toHaveBeenCalled();
    await botao();
    expect(h.toast).toHaveBeenLastCalledWith('502: fora');
    await botao();
    expect(h.toast).toHaveBeenLastCalledWith('502: fora (disco cheio)');
    act(() => root.unmount());
  });

  it('audio cited by the agent plays but has no "transcribe again"', async () => {
    h.list.mockResolvedValueOnce({ files: [] });
    h.events = [{ kind: 'assistant_msg', id: 'a1', text: 'narration at /tmp/n.mp3', ts: 10 }];
    const { container, root } = await render();
    await click(container, 'media_tab_audio:1');
    expect(container.textContent).toContain('audio:n.mp3');
    expect(find(container, 'composer_transcrever_de_novo: n.mp3')).toBeUndefined();
    act(() => root.unmount());
  });

  it('video plays in the media viewer, not in the document viewer', async () => {
    h.list.mockResolvedValueOnce({ files: [file('clip.mp4')] });
    const { container, root } = await render();
    await click(container, 'anexos_tocar:clip.mp4');
    expect(container.querySelector('[data-lightbox="video"]')?.textContent).toBe('|clip.mp4');
    expect(container.querySelector('[data-doc]')).toBeNull();
    act(() => root.unmount());
  });

  it('photos and videos form one gallery, newest first, like the grid', async () => {
    h.list.mockResolvedValueOnce({ files: [file('a.png', 3), file('relatorio.pdf', 4), file('clip.mp4', 2), file('b.jpg', 1)] });
    const { container, root } = await render();
    await click(container, 'anexos_tocar:clip.mp4');
    expect(container.querySelector('[data-lightbox]')?.textContent).toBe('gallery_counter:2,3|clip.mp4');
    await act(async () => lastLightbox().onStep!(1));
    expect(container.querySelector('[data-lightbox="image"]')?.textContent).toBe('gallery_counter:3,3|b.jpg');
    expect(lastLightbox()).toMatchObject({ uri: 'https://a.test/api/sessions/sess/uploads/b.jpg', headers: { Authorization: 'Bearer token-a' } });
    await act(async () => lastLightbox().onStep!(1));
    expect(container.querySelector('[data-lightbox]')?.textContent).toBe('gallery_counter:3,3|b.jpg');
    await act(async () => lastLightbox().onStep!(-1));
    await act(async () => lastLightbox().onStep!(-1));
    expect(container.querySelector('[data-lightbox]')?.textContent).toBe('gallery_counter:1,3|a.png');
    // Upload fora do histórico carregado não tem mensagem para onde ir.
    expect(lastLightbox().onShowInChat).toBeUndefined();
    act(() => root.unmount());
  });

  it('cited media and files come from the conversation, with origin, filter and "in chat"', async () => {
    h.list.mockResolvedValueOnce({ files: [] });
    h.resolve.mockResolvedValue({ ok: { '/tmp/out/r.pdf': { relativo: null, real: '/tmp/out/r.pdf', size: 2048 } }, faltam: [] });
    h.events = [
      { kind: 'user_msg', id: 'u1', text: 'ok', ts: 5 },
      { kind: 'assistant_msg', id: 'a1', text: 'shot at /tmp/shot.png and the report /tmp/out/r.pdf', ts: 10 },
    ];
    const { container, root } = await render();
    expect(h.resolve).toHaveBeenCalledWith('sess', ['/tmp/shot.png', '/tmp/out/r.pdf'], servers.servers[0]);
    expect(container.textContent).toContain('media_origin_agent');
    await click(container, 'media_filter_user');
    expect(container.textContent).toContain('media_empty');
    await click(container, 'media_filter_all');
    await click(container, 'anexos_ver:shot.png');
    expect(lastLightbox()).toMatchObject({ uri: 'https://a.test/api/sessions/sess/file?path=%2Ftmp%2Fshot.png', visible: true });
    await act(async () => lastLightbox().onShowInChat!());
    expect(useChatJump.getState().request).toMatchObject({ route: 's1::sess', eventId: 'a1' });
    expect(h.back).toHaveBeenCalledTimes(1);
    await click(container, 'media_tab_files:1');
    expect(container.textContent).toContain('r.pdf');
    expect(container.textContent).toContain('2 KB · /tmp/out · media_origin_agent');
    act(() => root.unmount());
  });

  it.each(['notas.docx', 'tabela.xlsx', 'pacote.zip', 'dados.csv', 'leia.txt'])('%s downloads, then goes to the system', async (filename) => {
    h.list.mockResolvedValueOnce({ files: [file(filename)] });
    const done = { uri: 'file:///cache/x', size: 10 };
    h.download.mockResolvedValueOnce(done);
    h.deliverLocal.mockResolvedValueOnce('opened');
    const { container, root } = await render();
    await click(container, 'media_tab_files:1');
    await click(container, `file_card_download: ${filename}`);
    expect(h.download).toHaveBeenCalledExactlyOnceWith(`s1::sess::upload:${filename}`,
      `https://a.test/api/sessions/sess/uploads/${filename}`, { Authorization: 'Bearer token-a' }, filename,
      expect.any(Function), expect.any(AbortSignal));
    expect(h.deliverLocal).toHaveBeenCalledExactlyOnceWith(done, filename);
    expect(container.querySelector('[data-doc]')).toBeNull();
    expect(h.toast).not.toHaveBeenCalled();
    // Baixado: o próximo toque abre a cópia, sem baixar de novo.
    await click(container, `file_card_open: ${filename}`);
    expect(h.download).toHaveBeenCalledTimes(1);
    expect(h.deliverLocal).toHaveBeenCalledTimes(2);
    act(() => root.unmount());
  });

  it('an already downloaded file opens without the network', async () => {
    h.list.mockResolvedValueOnce({ files: [file('pacote.zip')] });
    const kept = { uri: 'file:///cache/kept', size: 5 };
    h.kept.mockReturnValue(kept);
    h.deliverLocal.mockResolvedValueOnce('opened');
    const { container, root } = await render();
    await click(container, 'media_tab_files:1');
    await click(container, 'file_card_open: pacote.zip');
    expect(h.download).not.toHaveBeenCalled();
    expect(h.deliverLocal).toHaveBeenCalledWith(kept, 'pacote.zip');
    act(() => root.unmount());
  });

  it('a file that does not open says why, and a failed download offers "again"', async () => {
    h.list.mockResolvedValue({ files: [file('pacote.rar')] });
    h.download.mockResolvedValueOnce({ uri: 'file:///x', size: 3 }).mockRejectedValueOnce(new Error('server returned HTTP 404'));
    h.deliverLocal.mockResolvedValueOnce('no-app');
    const { container, root } = await render();
    await click(container, 'media_tab_files:1');
    await click(container, 'file_card_download: pacote.rar');
    expect(h.toast).toHaveBeenLastCalledWith('pacote.rar: file_card_no_app');
    act(() => root.unmount());

    h.kept.mockReturnValue(null);
    const again = await render();
    await click(again.container, 'media_tab_files:1');
    await click(again.container, 'file_card_download: pacote.rar');
    expect(again.container.textContent).toContain('media_download_failed:file_card_missing');
    expect(find(again.container, 'media_again: pacote.rar')).toBeDefined();
    act(() => again.root.unmount());
  });

  it('servidor sem token avisa sessão expirada e não abre nada', async () => {
    servers.servers = [{ ...servers.servers[0], token: '' }, servers.servers[1]];
    h.list.mockResolvedValueOnce({ files: [file('pacote.zip'), file('relatorio.pdf'), file('clip.mp4')] });
    const { container, root } = await render();
    await click(container, 'anexos_tocar:clip.mp4');
    expect(h.toast).toHaveBeenLastCalledWith('sessao_expirada');
    await click(container, 'media_tab_files:2');
    for (const label of ['file_card_download: pacote.zip', 'anexos_visualizar:relatorio.pdf']) {
      h.toast.mockClear();
      await click(container, label);
      expect(h.toast).toHaveBeenLastCalledWith('sessao_expirada');
    }
    expect(h.download).not.toHaveBeenCalled();
    expect(container.querySelector('[data-doc]')).toBeNull();
    expect(container.querySelector('[data-lightbox]')).toBeNull();
    act(() => root.unmount());
  });

  it('dois toques seguidos baixam o documento uma vez só', async () => {
    h.list.mockResolvedValueOnce({ files: [file('pacote.zip')] });
    let finish!: (v: { uri: string; size: number }) => void;
    h.download.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    h.deliverLocal.mockResolvedValueOnce('opened');
    const { container, root } = await render();
    await click(container, 'media_tab_files:1');
    const button = find(container, 'file_card_download: pacote.zip')!;
    await act(async () => {
      button.click();
      button.click();
    });
    expect(h.download).toHaveBeenCalledTimes(1);
    await act(async () => finish({ uri: 'file:///cache/x', size: 10 }));
    expect(h.deliverLocal).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it('erro com status 404 tipado diz que o arquivo sumiu', async () => {
    h.list.mockResolvedValueOnce({ files: [file('pacote.rar')] });
    h.download.mockRejectedValueOnce(Object.assign(new Error('falhou'), { status: 404 }));
    const { container, root } = await render();
    await click(container, 'media_tab_files:1');
    await click(container, 'file_card_download: pacote.rar');
    expect(container.textContent).toContain('media_download_failed:file_card_missing');
    act(() => root.unmount());
  });

  it.each([
    ['relatório final.pdf', 'pdf', 'relat%C3%B3rio%20final.pdf'],
    ['página final.html', 'html', 'p%C3%A1gina%20final.html'],
  ])('%s opens the upload in the viewer, on the chat server', async (filename, kind, encoded) => {
    h.list.mockResolvedValueOnce({ files: [file(filename)] });
    const { container, root } = await render();
    await click(container, 'media_tab_files:1');
    await click(container, `anexos_visualizar:${filename}`);
    expect(h.doc).toHaveBeenLastCalledWith({
      uri: `https://a.test/api/sessions/sess/uploads/${encoded}`,
      headers: { Authorization: 'Bearer token-a' }, name: filename, kind,
    });
    expect(h.push).not.toHaveBeenCalled();
    expect(servers.activeId).toBe('s2');
    act(() => root.unmount());
  });

  it('list and media of the route stay on A with B active', async () => {
    h.list.mockResolvedValueOnce({ files: [file('ditado.m4a'), file('clip.mp4')] });
    const { container, root } = await render();
    expect(h.list).toHaveBeenCalledExactlyOnceWith('sess', servers.servers[0]);
    expect(servers.activeId).toBe('s2');
    await click(container, 'anexos_tocar:clip.mp4');
    expect(lastLightbox()).toMatchObject({
      uri: 'https://a.test/api/sessions/sess/uploads/clip.mp4', headers: { Authorization: 'Bearer token-a' }, kind: 'video',
    });
    await click(container, 'media_tab_audio:1');
    expect(h.audio).toHaveBeenCalledWith(expect.objectContaining({
      uri: 'https://a.test/api/sessions/sess/uploads/ditado.m4a', headers: { Authorization: 'Bearer token-a' },
    }));
    servers.servers = servers.servers.filter((s) => s.id !== 's1');
    await act(async () => root.render(createElement(AttachmentsScreen)));
    expect(container.textContent).toContain('chat_servidor_removido');
    expect(container.querySelector('[data-doc]')).toBeNull();
    expect(container.querySelector('[data-lightbox]')).toBeNull();
    expect(h.list).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it('without the route server it neither lists nor mounts media of the active one', async () => {
    servers.servers = servers.servers.filter((s) => s.id !== 's1');
    const { container, root } = await render();
    expect(container.textContent).toContain('chat_servidor_removido');
    expect(h.list).not.toHaveBeenCalled();
    expect(h.audio).not.toHaveBeenCalled();
    expect(h.doc).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it('dictation still unused in the chat: the refusal shows and the screen stays', async () => {
    h.list.mockResolvedValueOnce({ files: [file('ditado-1.m4a')] });
    h.dictate.mockImplementationOnce(() => { throw new Error('composer_aguarde_transcricao'); });
    const { container, root } = await render();
    await click(container, 'media_tab_audio:1');
    await click(container, 'composer_transcrever_de_novo: ditado-1.m4a');
    expect(h.toast).toHaveBeenCalledWith('composer_aguarde_transcricao');
    expect(h.back).not.toHaveBeenCalled();
    act(() => root.unmount());
  });
});
