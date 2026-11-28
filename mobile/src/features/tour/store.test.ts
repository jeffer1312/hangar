import { beforeEach, describe, expect, it, vi } from 'vitest';

const { memory, servers, tourProgressFor } = vi.hoisted(() => ({
  memory: new Map<string, string>(),
  servers: { list: [] as { id: string; label: string; baseUrl: string; token: string; invite?: boolean; disabled?: boolean }[] },
  tourProgressFor: vi.fn(),
}));
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (key: string) => memory.get(key),
    set: (key: string, value: string) => { memory.set(key, value); },
  }),
}));
vi.mock('../../stores/servers', () => ({
  useServers: { getState: () => ({ servers: servers.list }) },
  ownServers: (list: { invite?: boolean }[]) => list.filter((s) => !s.invite),
}));
vi.mock('../../net/demo', () => ({ DEMO_BASE_URL: 'http://127.0.0.1:1' }));
vi.mock('../../ui/haptics', () => ({ haptic: { selection: vi.fn(), success: vi.fn() } }));
vi.mock('@hangar/core', async (original) => ({ ...(await original<typeof import('@hangar/core')>()), tourProgressFor, registrarDiag: vi.fn() }));

import { MOBILE_TOUR_DONE, MOBILE_TOUR_SKIPPED } from '@hangar/core';
import { planTour, replayTour, syncTour, tourAction, tourChoice, useTour } from './store';

const casa = { id: 'casa', label: 'Casa', baseUrl: 'https://casa.test', token: 't' };

beforeEach(() => {
  memory.clear();
  tourProgressFor.mockReset();
  servers.list = [casa];
  useTour.setState({ progress: {}, synced: false, planned: false, replay: false, run: null, tips: [] });
});

describe('sincronização do visto', () => {
  it('junta o que o servidor tem e devolve a ele o que faltava', async () => {
    useTour.setState({ progress: { tips: ['mobile-tip:media'] } });
    tourProgressFor.mockImplementation(async (_s: unknown, patch?: unknown) => patch ? {} : { tour: 'done', tips: [MOBILE_TOUR_SKIPPED] });
    await syncTour();
    const { progress, synced } = useTour.getState();
    expect(synced).toBe(true);
    expect(progress.tour).toBe('done');
    expect(progress.tips).toEqual(['mobile-tip:media', MOBILE_TOUR_SKIPPED]);
    const puts = tourProgressFor.mock.calls.filter((call) => call[1]);
    expect(puts).toHaveLength(1);
    expect(puts[0][1].tips).toContain('mobile-tip:media');
    expect(JSON.parse(memory.get('tour.progress.v1')!).tips).toContain(MOBILE_TOUR_SKIPPED);
  });

  it('servidor que já tem tudo não recebe PUT', async () => {
    tourProgressFor.mockResolvedValue({ tips: ['mobile-tip:media'] });
    useTour.setState({ progress: { tips: ['mobile-tip:media'] } });
    await syncTour();
    expect(tourProgressFor.mock.calls.filter((call) => call[1])).toHaveLength(0);
  });

  it('não fala com a demonstração, com convite nem com servidor desligado', async () => {
    servers.list = [{ ...casa, id: 'demo', baseUrl: 'http://127.0.0.1:1' }, { ...casa, id: 'par', invite: true }, { ...casa, id: 'off', disabled: true }, casa];
    tourProgressFor.mockResolvedValue({});
    await syncTour();
    expect(tourProgressFor.mock.calls.map((call) => call[0].id)).toEqual(['casa']);
  });

  it('sem nenhuma resposta o visto segue desconhecido e a próxima chamada tenta de novo', async () => {
    tourProgressFor.mockRejectedValueOnce(new Error('fora do ar'));
    await syncTour();
    expect(useTour.getState().synced).toBe(false);
    servers.list = [];
    await syncTour();
    expect(useTour.getState().synced).toBe(false);
    servers.list = [casa];
    tourProgressFor.mockResolvedValue({ tips: [MOBILE_TOUR_DONE] });
    await syncTour();
    expect(useTour.getState().synced).toBe(true);
    expect(useTour.getState().progress.tips).toContain(MOBILE_TOUR_DONE);
  });

  it('servidor que só não tem a rota conta como resposta', async () => {
    tourProgressFor.mockResolvedValue(null);
    await syncTour();
    expect(useTour.getState().synced).toBe(true);
  });

  it('pedido com outra rodada em voo roda mais uma, e ela lê o servidor que chegou', async () => {
    let release!: () => void;
    tourProgressFor.mockImplementationOnce(() => new Promise((resolve) => { release = () => resolve({}); }));
    tourProgressFor.mockResolvedValue({});
    const first = syncTour();
    const nova = { ...casa, id: 'nova', baseUrl: 'https://nova.test' };
    servers.list = [casa, nova];
    const second = syncTour();
    expect(syncTour()).toBe(second);
    release();
    await Promise.all([first, second]);
    const read = tourProgressFor.mock.calls.filter((call) => !call[1]).map((call) => call[0].id);
    expect(read).toEqual(['casa', 'casa', 'nova']);
  });
});

describe('volta do tour', () => {
  beforeEach(() => { useTour.setState({ synced: true }); });

  it('sem o visto dos servidores só as dicas existem, e a volta é planejada quando ele chega', () => {
    useTour.setState({ synced: false });
    planTour(2);
    expect(useTour.getState().planned).toBe(false);
    expect(useTour.getState().run).toBeNull();
    expect(useTour.getState().tips.map((t) => t.id)).toEqual(['media']);
    useTour.setState({ synced: true });
    planTour(2);
    expect(useTour.getState().run?.steps[0].id).toBe('menu');
  });

  it('no modo nova conversa o tour começa pelas peças da conversa', () => {
    planTour(2, 'newConversation');
    expect(useTour.getState().run?.steps.map((s) => s.id)).toEqual(['attach', 'dictation', 'model']);
    expect(useTour.getState().tips.map((t) => t.id)).toEqual(['tour-menu', 'tour-search', 'media']);
  });

  it('tour já visto no celular deixa só as dicas', () => {
    useTour.setState({ progress: { tips: [MOBILE_TOUR_DONE] } });
    planTour(2);
    expect(useTour.getState().run).toBeNull();
    expect(useTour.getState().tips.map((t) => t.id)).toEqual(['media']);
  });

  it('tour visto só no PC não esconde o do celular', () => {
    useTour.setState({ progress: { tour: 'done' } });
    planTour(2);
    expect(useTour.getState().run?.steps[0].id).toBe('menu');
  });

  it('a última ação grava done no aparelho e manda só isso aos servidores', () => {
    tourProgressFor.mockResolvedValue({});
    planTour(2);
    for (const id of ['menu', 'search', 'attach', 'dictation']) tourAction(id);
    expect(useTour.getState().progress.tips ?? []).not.toContain(MOBILE_TOUR_DONE);
    tourAction('model');
    expect(useTour.getState().progress.tips).toContain(MOBILE_TOUR_DONE);
    expect(tourProgressFor).toHaveBeenCalledWith(casa, { tips: [MOBILE_TOUR_DONE] });
  });

  it('ver depois não grava; ver o tour de novo planeja outra volta mesmo já visto', () => {
    planTour(2);
    tourChoice('later');
    expect(useTour.getState().run?.index).toBeNull();
    expect(useTour.getState().progress.tips ?? []).toEqual([]);
    useTour.setState({ progress: { tips: [MOBILE_TOUR_SKIPPED] } });
    replayTour();
    expect(useTour.getState().planned).toBe(false);
    planTour(2);
    expect(useTour.getState().run?.index).toBe(0);
    expect(useTour.getState().replay).toBe(false);
  });
});
