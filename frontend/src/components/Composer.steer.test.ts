// @vitest-environment happy-dom
// Chip de FILA do Kimi (⏳ N na fila · mandar agora), na fileira de cima do composer. Ele responde
// duas perguntas do dono: "tem coisa esperando?" e "dá pra mandar agora?" — antes disso a única
// pista de fila era a bolha translúcida, e não havia saída nenhuma pelo app.
// Só aparece em Kimi TRABALHANDO e com fila: em Claude a tecla (ctrl-s) não significa isso, e com a
// sessão parada não há turno pra furar.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import Composer from './Composer.svelte';
import * as m from '../paraglide/messages';

vi.mock('../lib/api', () => ({
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  isTimeoutError: vi.fn(() => false),
  isAbortError: vi.fn(() => false),
  getCommands: vi.fn().mockResolvedValue([]),
  setModelEffort: vi.fn(),
  uploadFile: vi.fn(),
  transcribeFile: vi.fn(),
  getCodexModels: vi.fn().mockResolvedValue([]),
  getPiModels: vi.fn().mockResolvedValue([]),
}));

const flush = () => new Promise((r) => setTimeout(r, 0));

function montar(opts: {
  provider?: 'claude' | 'kimi';
  state?: 'working' | 'idle';
  filaCount?: number;
  onSteer?: () => void;
} = {}) {
  const onSteer = opts.onSteer ?? vi.fn();
  const target = document.createElement('div');
  document.body.appendChild(target);
  const comp = mount(Composer, {
    target,
    props: {
      sessionName: 's', sessionState: opts.state ?? 'working', status: null,
      onSend: vi.fn(), onSteer, onCommand: () => {}, onInterrupt: () => {},
      onOpenGit: () => {}, onOpenPreview: () => {},
      inputText: '', provider: opts.provider ?? 'kimi', filaCount: opts.filaCount ?? 2,
    },
  });
  // Seletor pela CLASSE, nao pelo aria-label: o rotulo vem do Paraglide e o teste roda no
  // baseLocale (en), entao casar texto em portugues aqui quebraria sem nada estar errado.
  const chip = () => target.querySelector<HTMLButtonElement>('button.fila-chip');
  return { target, comp, chip, onSteer };
}

describe('chip de fila do Kimi', () => {
  it('Kimi trabalhando com fila: mostra a contagem e manda o ctrl-s no toque', async () => {
    const { comp, chip, onSteer } = montar({ filaCount: 2 });
    await flush();
    expect(chip()?.textContent).toContain(m.composer_fila_contagem({ n: 2 }));
    chip()!.click();
    await flush();
    expect(onSteer).toHaveBeenCalledOnce();
    unmount(comp);
  });

  it('sem fila não há chip (nada esperando = nada a oferecer)', async () => {
    const { comp, chip } = montar({ filaCount: 0 });
    await flush();
    expect(chip()).toBeNull();
    unmount(comp);
  });

  it('sessão parada não mostra: não há turno na frente pra furar', async () => {
    const { comp, chip } = montar({ state: 'idle' });
    await flush();
    expect(chip()).toBeNull();
    unmount(comp);
  });

  it('Claude não mostra: a tecla não existe naquela TUI', async () => {
    const { comp, chip } = montar({ provider: 'claude' });
    await flush();
    expect(chip()).toBeNull();
    unmount(comp);
  });
});
