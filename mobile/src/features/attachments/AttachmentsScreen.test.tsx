// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, type Server, type UploadFile } from '@hangar/core';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const h = vi.hoisted(() => ({ list: vi.fn(), dictate: vi.fn(), back: vi.fn(), push: vi.fn(), toast: vi.fn(),
  audio: vi.fn(), card: vi.fn(), doc: vi.fn(), lightbox: vi.fn(), deliver: vi.fn() }));
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
}));
vi.mock('../../stores/servers', () => ({
  useServers: Object.assign((select: (s: typeof servers) => unknown) => select(servers), { getState: () => servers }),
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
vi.mock('../../chat/AudioChip', () => ({ AudioChip: (props: { name: string; uri: string; headers?: Record<string, string> }) => {
  h.audio(props);
  return createElement('span', null, `audio:${props.name}`);
} }));
vi.mock('./AttachmentCard', () => ({
  AttachmentCard: (props: { file: UploadFile; serverId: string; onPress: () => void }) => {
    h.card(props);
    return createElement('button', { 'aria-label': props.file.filename, onClick: props.onPress }, props.file.filename);
  },
}));
type LightboxMockProps = { visible: boolean; uri: string; headers?: Record<string, string>; filename: string; kind?: string;
  counter?: string; onStep?: (dir: 1 | -1) => void };
vi.mock('./Lightbox', () => ({ Lightbox: (props: LightboxMockProps) => {
  h.lightbox(props);
  return props.visible ? createElement('div', { 'data-lightbox': props.kind ?? 'image' }, `${props.counter ?? ''}|${props.filename}`) : null;
} }));
vi.mock('./mediaCache', () => ({ deliverFile: h.deliver }));
vi.mock('./DocumentViewer', () => ({
  DocumentViewer: ({ doc }: { doc: { kind: string; name: string; uri: string; headers?: Record<string, string> } | null }) => {
    h.doc(doc);
    return doc ? createElement('div', { 'data-doc': doc.kind }, doc.name) : null;
  },
}));
vi.mock('../../ui/Toast', () => ({ toast: { erro: h.toast, ok: () => {} } }));
vi.mock('../../theme/superficie', () => ({ superficie: () => '#eee' }));
vi.mock('../../paraglide/messages', () => Object.fromEntries(
  ('comum_carregando anexos_erro_listar anexos_nenhum lista_tentar_novamente ctx_anexos composer_transcrever_de_novo '
    + 'arq_sessao_encerrada sessao_expirada anexos_fechar_visualizacao arquivo_carregar_erro chat_servidor_removido composer_falha_transcricao '
    + 'file_card_empty file_card_no_app file_card_missing file_card_failed')
    .split(' ').map((k) => [k, () => k]),
));

import AttachmentsScreen from '../../../app/s/[server]/[name]/attachments';

const file = (filename: string): UploadFile => ({ filename, size: 10, mtime: 0, expires_in_days: null });
async function render() {
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(createElement(AttachmentsScreen)));
  return { container, root };
}
const byLabel = (c: HTMLElement, label: string) => [...c.querySelectorAll('button')]
  .find((el) => el.textContent === label || el.getAttribute('aria-label') === label) as HTMLButtonElement | undefined;

describe('galeria de anexos', () => {
  beforeEach(() => {
    vi.clearAllMocks(); h.list.mockReset(); h.dictate.mockReset();
    servers.ready = true;
    servers.servers = [
      { id: 's1', label: 'A', baseUrl: 'https://a.test', token: 'token-a' },
      { id: 's2', label: 'B', baseUrl: 'https://b.test', token: 'token-b' },
    ];
    configureApi({ getBaseUrl: () => 'https://b.test', getToken: () => 'token-b', onUnauthorized: () => {},
      origin: null, createEventSource: () => { throw new Error('unexpected SSE'); } });
  });

  it('carregando, vazio, erro com tentar de novo e lista', async () => {
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
    await act(async () => byLabel(second.container, 'lista_tentar_novamente')!.click());
    expect(byLabel(second.container, 'foto.png')).toBeDefined();
    act(() => second.root.unmount());
  });

  it('áudio toca na lista e "Transcrever de novo" manda à conversa e volta', async () => {
    h.list.mockResolvedValueOnce({ files: [file('ditado-1.m4a'), file('clip.mp4')] });
    h.dictate.mockReturnValueOnce(Promise.resolve());
    const { container, root } = await render();
    expect(container.textContent).toContain('audio:ditado-1.m4a');
    await act(async () => byLabel(container, 'composer_transcrever_de_novo: ditado-1.m4a')!.click());
    expect(h.dictate).toHaveBeenCalledExactlyOnceWith(servers.servers[0], 's1', 'sess', '/t/a.jsonl', 'ditado-1.m4a', 'prosa');
    expect(h.back).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it('falha que não ficou guardada na conversa vira toast; a guardada não', async () => {
    const { DictationError } = await import('../ditado/dictationRun');
    h.list.mockResolvedValue({ files: [file('ditado-1.m4a')] });
    h.dictate
      .mockRejectedValueOnce(new DictationError('502: fora', false, ''))
      .mockRejectedValueOnce(new DictationError('502: fora', true, ''))
      .mockRejectedValueOnce(new DictationError('502: fora', false, 'disco cheio'));
    const { container, root } = await render();
    const botao = () => byLabel(container, 'composer_transcrever_de_novo: ditado-1.m4a')!;
    await act(async () => botao().click());
    expect(h.toast).not.toHaveBeenCalled();
    await act(async () => botao().click());
    expect(h.toast).toHaveBeenLastCalledWith('502: fora');
    await act(async () => botao().click());
    expect(h.toast).toHaveBeenLastCalledWith('502: fora (disco cheio)');
    act(() => root.unmount());
  });

  it('vídeo toca no visor de mídia (que baixa antes), não no WebView com o header', async () => {
    h.list.mockResolvedValueOnce({ files: [file('clip.mp4')] });
    const { container, root } = await render();
    await act(async () => byLabel(container, 'clip.mp4')!.click());
    expect(container.querySelector('[data-lightbox="video"]')?.textContent).toBe('|clip.mp4');
    expect(container.querySelector('[data-doc]')).toBeNull();
    act(() => root.unmount());
  });

  it('fotos e vídeos da lista formam uma galeria no visor', async () => {
    h.list.mockResolvedValueOnce({ files: [file('a.png'), file('relatorio.pdf'), file('clip.mp4'), file('b.jpg')] });
    const { container, root } = await render();
    await act(async () => byLabel(container, 'clip.mp4')!.click());
    expect(container.querySelector('[data-lightbox]')?.textContent).toBe('2/3|clip.mp4');
    const step = () => (h.lightbox.mock.lastCall![0] as LightboxMockProps).onStep!;
    await act(async () => step()(1));
    expect(container.querySelector('[data-lightbox="image"]')?.textContent).toBe('3/3|b.jpg');
    expect(h.lightbox).toHaveBeenLastCalledWith(expect.objectContaining({
      uri: 'https://a.test/api/sessions/sess/uploads/b.jpg', headers: { Authorization: 'Bearer token-a' },
    }));
    await act(async () => step()(1));
    expect(container.querySelector('[data-lightbox]')?.textContent).toBe('3/3|b.jpg');
    await act(async () => { step()(-1); });
    await act(async () => { step()(-1); });
    expect(container.querySelector('[data-lightbox]')?.textContent).toBe('1/3|a.png');
    act(() => root.unmount());
  });

  it.each(['notas.docx', 'tabela.xlsx', 'pacote.zip', 'dados.csv', 'leia.txt'])('%s vai ao sistema como o card da conversa, não ao WebView', async (filename) => {
    h.list.mockResolvedValueOnce({ files: [file(filename)] });
    h.deliver.mockResolvedValueOnce('opened');
    const { container, root } = await render();
    await act(async () => byLabel(container, filename)!.click());
    expect(h.deliver).toHaveBeenCalledExactlyOnceWith(
      `https://a.test/api/sessions/sess/uploads/${filename}`, { Authorization: 'Bearer token-a' }, filename);
    expect(container.querySelector('[data-doc]')).toBeNull();
    expect(h.toast).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it('documento que não abre avisa o motivo', async () => {
    h.list.mockResolvedValue({ files: [file('pacote.rar')] });
    h.deliver.mockResolvedValueOnce('no-app').mockRejectedValueOnce(new Error('response has status: 404'));
    const { container, root } = await render();
    await act(async () => byLabel(container, 'pacote.rar')!.click());
    expect(h.toast).toHaveBeenLastCalledWith('pacote.rar: file_card_no_app');
    // O fim da entrega libera o cartão para o próximo toque.
    await act(async () => {});
    await act(async () => byLabel(container, 'pacote.rar')!.click());
    expect(h.toast).toHaveBeenLastCalledWith('pacote.rar: file_card_missing');
    act(() => root.unmount());
  });

  it('servidor sem token avisa sessão expirada e não abre nada', async () => {
    servers.servers = [{ ...servers.servers[0], token: '' }, servers.servers[1]];
    h.list.mockResolvedValueOnce({ files: [file('pacote.zip'), file('relatorio.pdf'), file('clip.mp4')] });
    const { container, root } = await render();
    for (const name of ['pacote.zip', 'relatorio.pdf', 'clip.mp4']) {
      await act(async () => byLabel(container, name)!.click());
      expect(h.toast).toHaveBeenLastCalledWith('sessao_expirada');
    }
    expect(h.deliver).not.toHaveBeenCalled();
    expect(container.querySelector('[data-doc]')).toBeNull();
    expect(container.querySelector('[data-lightbox]')).toBeNull();
    act(() => root.unmount());
  });

  it('dois toques seguidos entregam o documento uma vez só', async () => {
    h.list.mockResolvedValueOnce({ files: [file('pacote.zip')] });
    let finish!: (v: string) => void;
    h.deliver.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const { container, root } = await render();
    await act(async () => {
      byLabel(container, 'pacote.zip')!.click();
      byLabel(container, 'pacote.zip')!.click();
    });
    expect(h.deliver).toHaveBeenCalledTimes(1);
    await act(async () => finish('opened'));
    act(() => root.unmount());
  });

  it('erro com status 404 tipado diz que o arquivo sumiu', async () => {
    h.list.mockResolvedValueOnce({ files: [file('pacote.rar')] });
    h.deliver.mockRejectedValueOnce(Object.assign(new Error('falhou'), { status: 404 }));
    const { container, root } = await render();
    await act(async () => byLabel(container, 'pacote.rar')!.click());
    expect(h.toast).toHaveBeenLastCalledWith('pacote.rar: file_card_missing');
    act(() => root.unmount());
  });

  it.each([
    ['relatório final.pdf', 'pdf', 'relat%C3%B3rio%20final.pdf'],
    ['página final.html', 'html', 'p%C3%A1gina%20final.html'],
  ])('%s abre o upload no visor, no servidor da conversa', async (filename, kind, encoded) => {
    h.list.mockResolvedValueOnce({ files: [file(filename)] });
    const { container, root } = await render();
    await act(async () => byLabel(container, filename)!.click());
    expect(h.doc).toHaveBeenLastCalledWith({
      uri: `https://a.test/api/sessions/sess/uploads/${encoded}`,
      headers: { Authorization: 'Bearer token-a' }, name: filename, kind,
    });
    expect(h.push).not.toHaveBeenCalled();
    expect(servers.activeId).toBe('s2');
    act(() => root.unmount());
  });

  it('lista e mídia da rota ficam em A com B ativo', async () => {
    h.list.mockResolvedValueOnce({ files: [file('ditado.m4a'), file('clip.mp4')] });
    const { container, root } = await render();
    expect(h.list).toHaveBeenCalledExactlyOnceWith('sess', servers.servers[0]);
    expect(servers.activeId).toBe('s2');
    expect(h.audio).toHaveBeenCalledWith(expect.objectContaining({
      uri: 'https://a.test/api/sessions/sess/uploads/ditado.m4a', headers: { Authorization: 'Bearer token-a' },
    }));
    expect(h.card).toHaveBeenCalledWith(expect.objectContaining({ serverId: 's1' }));
    await act(async () => byLabel(container, 'clip.mp4')!.click());
    expect(h.lightbox).toHaveBeenLastCalledWith(expect.objectContaining({
      uri: 'https://a.test/api/sessions/sess/uploads/clip.mp4', headers: { Authorization: 'Bearer token-a' }, kind: 'video',
    }));
    servers.servers = servers.servers.filter((s) => s.id !== 's1');
    await act(async () => root.render(createElement(AttachmentsScreen)));
    expect(container.textContent).toContain('chat_servidor_removido');
    expect(container.querySelector('[data-doc]')).toBeNull();
    expect(container.querySelector('[data-lightbox]')).toBeNull();
    expect(h.list).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it('sem servidor da rota não lista nem monta mídia do ativo', async () => {
    servers.servers = servers.servers.filter((s) => s.id !== 's1');
    const { container, root } = await render();
    expect(container.textContent).toContain('chat_servidor_removido');
    expect(h.list).not.toHaveBeenCalled();
    expect(h.audio).not.toHaveBeenCalled();
    expect(h.card).not.toHaveBeenCalled();
    expect(h.doc).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it('ditado da conversa ainda não usado: a recusa aparece e a galeria fica', async () => {
    h.list.mockResolvedValueOnce({ files: [file('ditado-1.m4a')] });
    h.dictate.mockImplementationOnce(() => { throw new Error('composer_aguarde_transcricao'); });
    const { container, root } = await render();
    await act(async () => byLabel(container, 'composer_transcrever_de_novo: ditado-1.m4a')!.click());
    expect(h.toast).toHaveBeenCalledWith('composer_aguarde_transcricao');
    expect(h.back).not.toHaveBeenCalled();
    act(() => root.unmount());
  });
});
