import { beforeEach, describe, expect, it, vi } from 'vitest';

const h = vi.hoisted(() => ({
  os: 'android',
  permission: 'granted',
  subscribe: vi.fn(),
  push: vi.fn(),
  getToken: vi.fn(async () => ({ data: 'ExponentPushToken[abc]' })),
  request: vi.fn(),
  channel: vi.fn(async () => null),
  handler: vi.fn(),
  clear: vi.fn(),
}));

vi.mock('react-native', () => ({ Platform: { get OS() { return h.os; } }, Linking: { openSettings: vi.fn() } }));
vi.mock('expo-router', () => ({ router: { push: h.push } }));
vi.mock('expo-constants', () => ({ default: { expoConfig: { extra: { eas: { projectId: 'proj-1' } } } } }));
vi.mock('expo-notifications', () => ({
  AndroidImportance: { HIGH: 4 },
  setNotificationChannelAsync: h.channel,
  getPermissionsAsync: vi.fn(async () => ({ status: h.permission })),
  requestPermissionsAsync: h.request.mockImplementation(async () => ({ status: h.permission })),
  getExpoPushTokenAsync: h.getToken,
  setNotificationHandler: h.handler,
  clearLastNotificationResponse: h.clear,
  DEFAULT_ACTION_IDENTIFIER: 'default-action',
}));
vi.mock('@hangar/core', () => ({
  subscribeExpoPushForServer: h.subscribe,
  isOwnMachine: (s: { invite?: unknown; ephemeral?: unknown; removed?: unknown }) => !s.invite && !s.ephemeral && !s.removed,
}));
vi.mock('../../paraglide/runtime', () => ({ getLocale: () => 'pt' }));
vi.mock('../../stores/servers', () => ({ useServers: vi.fn() }));

import { handleNotificationResponse, installNotificationHandler, openFromNotification, pushRegistrationKey, registerExpoPush, retryExpoPushIfDenied } from './expoPush';
import { openWhatsAppDraft, useWhatsAppTarget } from '../whatsapp/target';

const casa = { id: 'casa', label: 'Casa', baseUrl: 'https://casa.test', token: 't1' };
const work = { id: 'work', label: 'Trabalho', baseUrl: 'https://work.test', token: 't2' };

beforeEach(() => {
  h.os = 'android';
  h.permission = 'granted';
  h.subscribe.mockReset();
  h.push.mockReset();
  h.getToken.mockClear();
  h.request.mockClear();
  h.channel.mockClear();
  h.clear.mockClear();
});

describe('registerExpoPush', () => {
  it('iOS não pede permissão', async () => {
    h.os = 'ios';
    expect(await registerExpoPush([casa])).toBe('unsupported');
    expect(h.request).not.toHaveBeenCalled();
    expect(h.getToken).not.toHaveBeenCalled();
  });

  it('permissão negada não busca token', async () => {
    h.permission = 'denied';
    expect(await registerExpoPush([casa])).toBe('denied');
    expect(h.getToken).not.toHaveBeenCalled();
    expect(h.subscribe).not.toHaveBeenCalled();
  });

  it('um POST por servidor com o mesmo token e o id de cada um', async () => {
    expect(await registerExpoPush([casa, work])).toBe('registered');
    expect(h.getToken).toHaveBeenCalledWith({ projectId: 'proj-1' });
    expect(h.subscribe).toHaveBeenCalledTimes(2);
    expect(h.subscribe).toHaveBeenCalledWith(casa, 'ExponentPushToken[abc]', 'pt');
    expect(h.subscribe).toHaveBeenCalledWith(work, 'ExponentPushToken[abc]', 'pt');
  });

  it('falha num servidor não impede o outro, mas a rodada conta como falha', async () => {
    h.subscribe.mockImplementation(async (s: { id: string }) => { if (s.id === 'casa') throw new Error('404'); });
    expect(await registerExpoPush([casa, work])).toBe('failed');
    expect(h.subscribe).toHaveBeenCalledWith(work, 'ExponentPushToken[abc]', 'pt');
  });

  it('nenhum servidor aceitou (ou lista vazia) é falha, não registro', async () => {
    h.subscribe.mockRejectedValue(new Error('500'));
    expect(await registerExpoPush([casa, work])).toBe('failed');
    expect(await registerExpoPush([])).toBe('failed');
  });

  it('chamada no meio da pergunta não pergunta de novo e registra a lista nova no fim', async () => {
    h.permission = 'undetermined';
    let answer!: () => void;
    h.request.mockImplementationOnce(() => new Promise((r) => { answer = () => r({ status: 'granted' }); }));
    const first = registerExpoPush([casa]);
    const second = registerExpoPush([casa, work]);
    expect(second).toBe(first);
    await vi.waitFor(() => expect(answer).toBeDefined());
    h.permission = 'granted';
    answer();
    expect(await first).toBe('registered');
    expect(h.request).toHaveBeenCalledTimes(1);
    expect(h.subscribe).toHaveBeenCalledWith(work, 'ExponentPushToken[abc]', 'pt');
  });

  it('cria o canal Android default com importância alta', async () => {
    await registerExpoPush([casa]);
    expect(h.channel).toHaveBeenCalledWith('default', expect.objectContaining({ importance: 4 }));
  });
});

describe('openFromNotification', () => {
  it('abre a conversa da sessão', () => {
    openFromNotification({ server: 'casa', session: 'minha sessão' }, [casa]);
    expect(h.push).toHaveBeenCalledWith({ pathname: '/s/[server]/[name]', params: { server: 'casa', name: 'minha sessão' } });
  });

  it('push da lista agregada abre a lista', () => {
    openFromNotification({ server: 'casa' }, [casa]);
    expect(h.push).toHaveBeenCalledWith('/');
  });

  it('servidor fora da lista do aparelho abre a lista', () => {
    openFromNotification({ server: 'outro', session: 'x' }, [casa]);
    expect(h.push).toHaveBeenCalledWith('/');
  });

  it('rascunho de WhatsApp abre a folha dele na lista', () => {
    openWhatsAppDraft(null);
    openFromNotification({ server_id: 'casa', whatsapp_draft_id: 'd1' }, [casa]);
    expect(useWhatsAppTarget.getState().target).toEqual({ serverId: 'casa', draftId: 'd1' });
    expect(h.push).toHaveBeenCalledWith('/');
    openWhatsAppDraft(null);
  });

  it('rascunho de máquina fora da lista do aparelho não abre folha', () => {
    openWhatsAppDraft(null);
    openFromNotification({ server_id: 'outro', whatsapp_draft_id: 'd1' }, [casa]);
    expect(useWhatsAppTarget.getState().target).toBeNull();
    expect(h.push).toHaveBeenCalledWith('/');
  });
});

describe('permissão liberada nos ajustes', () => {
  it('registra de novo ao voltar quando a última tentativa foi negada', async () => {
    h.permission = 'denied';
    await registerExpoPush([casa]);
    h.permission = 'granted';
    expect(await retryExpoPushIfDenied([casa])).toBe('registered');
    expect(h.subscribe).toHaveBeenCalledWith(casa, 'ExponentPushToken[abc]', 'pt');
  });

  it('permissão ainda negada na volta não pergunta de novo', async () => {
    h.permission = 'denied';
    await registerExpoPush([casa]);
    h.request.mockClear();
    h.channel.mockClear();
    expect(await retryExpoPushIfDenied([casa])).toBe('denied');
    expect(h.request).not.toHaveBeenCalled();
    expect(h.channel).not.toHaveBeenCalled();
    expect(h.getToken).not.toHaveBeenCalled();
  });

  it('tenta de novo na volta quando o token não veio', async () => {
    h.getToken.mockRejectedValueOnce(new Error('FIS_AUTH_ERROR'));
    await expect(registerExpoPush([casa])).rejects.toThrow('FIS_AUTH_ERROR');
    expect(await retryExpoPushIfDenied([casa])).toBe('registered');
  });

  it('tenta de novo na volta quando nenhum servidor aceitou', async () => {
    h.subscribe.mockRejectedValueOnce(new Error('500'));
    expect(await registerExpoPush([casa])).toBe('failed');
    expect(await retryExpoPushIfDenied([casa])).toBe('registered');
  });

  it('não registra de novo quando a última tentativa deu certo', async () => {
    await registerExpoPush([casa]);
    h.subscribe.mockClear();
    expect(await retryExpoPushIfDenied([casa])).toBeNull();
    expect(h.subscribe).not.toHaveBeenCalled();
  });
});

describe('installNotificationHandler', () => {
  it('instala uma vez só, com banner, lista e som', async () => {
    installNotificationHandler();
    installNotificationHandler();
    expect(h.handler).toHaveBeenCalledTimes(1);
    const behavior = await h.handler.mock.calls[0][0].handleNotification();
    expect(behavior).toMatchObject({ shouldShowBanner: true, shouldShowList: true, shouldPlaySound: true });
  });
});

describe('handleNotificationResponse', () => {
  const response = (actionIdentifier: string) => ({
    actionIdentifier, notification: { request: { content: { data: { server: 'casa', session: 's1' } } } },
  });

  it('ignora ação que não é o toque padrão', () => {
    handleNotificationResponse(response('reply') as never, [casa]);
    expect(h.push).not.toHaveBeenCalled();
    expect(h.clear).not.toHaveBeenCalled();
  });

  it('navega e limpa a última resposta para não abrir duas vezes', () => {
    handleNotificationResponse(response('default-action') as never, [casa]);
    expect(h.push).toHaveBeenCalledTimes(1);
    expect(h.clear).toHaveBeenCalledTimes(1);
  });
});

describe('pushRegistrationKey', () => {
  it('muda com idioma, nome e conjunto de servidores', () => {
    const base = pushRegistrationKey([casa, work], 'pt');
    expect(pushRegistrationKey([casa, work], 'pt')).toBe(base);
    expect(pushRegistrationKey([casa, work], 'en')).not.toBe(base);
    expect(pushRegistrationKey([{ ...casa, label: 'Casa nova' }, work], 'pt')).not.toBe(base);
    expect(pushRegistrationKey([casa], 'pt')).not.toBe(base);
  });

  it('sem servidor para registrar, chave vazia', () => {
    expect(pushRegistrationKey([], 'pt')).toBe('');
    expect(pushRegistrationKey([{ ...casa, invite: true }], 'pt')).toBe('');
  });
});
