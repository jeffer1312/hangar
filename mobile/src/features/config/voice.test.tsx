// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CampoConfig } from '@hangar/core';
import Voice from '../../../app/config/voice';
import * as m from '../../paraglide/messages';
import { getLocale, overwriteGetLocale } from '../../paraglide/runtime';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const calls = vi.hoisted(() => ({ get: vi.fn(), patch: vi.fn() }));
const saved = vi.hoisted(() => new Map<string, unknown>());
vi.mock('../../stores/prefs', () => ({
  prefs: { getBoolean: (key: string) => saved.get(key) as boolean | undefined, set: (key: string, value: unknown) => { saved.set(key, value); } },
}));
const server = { id: 'voice-server', label: 'voice-fixture', baseUrl: 'https://voice.test', token: 'fixture' };

vi.mock('../../stores/servers', () => ({
  useServers: (selector: (state: { active: () => typeof server }) => unknown) => selector({ active: () => server }),
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  getConfigForServer: calls.get,
  patchConfigForServer: calls.patch,
}));
vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(),
  Pressable: ({ children, onPress, accessibilityRole, accessibilityLabel, accessibilityState, disabled }: {
    children?: ReactNode; onPress?: () => void; accessibilityRole?: string; accessibilityLabel?: string;
    accessibilityState?: { selected?: boolean; expanded?: boolean }; disabled?: boolean;
  }) => createElement('button', { onClick: onPress, role: accessibilityRole, 'aria-label': accessibilityLabel,
    'aria-selected': accessibilityState?.selected, 'aria-expanded': accessibilityState?.expanded, disabled }, children),
  TextInput: ({ value, onChangeText, accessibilityLabel, editable }: {
    value?: string; onChangeText?: (value: string) => void; accessibilityLabel?: string; editable?: boolean;
  }) => createElement('input', { value, 'aria-label': accessibilityLabel, disabled: editable === false,
    onInput: (event: { currentTarget: { value: string } }) => onChangeText?.(event.currentTarget.value) }),
  Switch: ({ value, onValueChange, accessibilityLabel, disabled }: {
    value: boolean; onValueChange: (value: boolean) => void; accessibilityLabel: string; disabled: boolean;
  }) => createElement('button', { role: 'switch', 'aria-label': accessibilityLabel, 'aria-checked': value, disabled,
    onClick: () => onValueChange(!value) }),
}));
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: ReactNode }) => children }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('./Pagina', () => ({ Pagina: ({ children }: { children: ReactNode }) => createElement('main', null, children) }));
vi.mock('./Slider', () => ({ Slider: () => null }));
vi.mock('./TranscriptionProviders', () => ({ TranscriptionProviders: () => null }));

let root: Root;
let container: HTMLDivElement;
let fields: Record<string, CampoConfig>;

beforeEach(() => {
  calls.get.mockReset();
  calls.patch.mockReset();
  fields = Object.fromEntries(Object.entries({
    dictation_organization_mode: 'none', dictation_claude_model: 'claude-old', dictation_codex_model: 'codex-old',
    dictation_include_recent_messages: false, ditado_estilo: 'limpar', ditado_vocabulario: '', dictation_max_minutes: 5,
    groq_api_key: '', transcription_base_url: '', transcription_model: '',
    llm_base_url: 'https://organization.test', llm_api_key: 'masked-key', llm_model: 'api-old', llm_reasoning_effort: '',
    llm_briefing_base_url: '', llm_briefing_api_key: '', llm_briefing_model: '',
    elevenlabs_api_key: '', tts_local_cmd: '', tts_max_chars: 1000,
  }).map(([key, valor]) => [key, { valor, origem: 'app', definido: key === 'llm_api_key' }]));
  calls.get.mockImplementation(async () => ({ campos: fields }));
  calls.patch.mockImplementation(async (_server, payload: Record<string, CampoConfig['valor']>) => {
    fields = { ...fields, ...Object.fromEntries(Object.entries(payload).map(([key, valor]) => [key, { ...fields[key], valor }])) };
    return { campos: fields };
  });
  container = document.createElement('div');
  root = createRoot(container);
});

afterEach(async () => { await act(async () => root.unmount()); });

const input = (label: string) => [...container.querySelectorAll('input')].find((el) => el.getAttribute('aria-label') === label);
const button = (label: string) => [...container.querySelectorAll('button')].find((el) => el.textContent === label
  || el.getAttribute('aria-label') === label || el.getAttribute('aria-label')?.startsWith(`${label}.`))!;
const click = (label: string) => act(async () => button(label).click());
const edit = (label: string, value: string) => act(async () => {
  const el = input(label)!;
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
});
const render = () => act(async () => root.render(createElement(Voice)));

describe('envio automático do ditado na tela de Voz', () => {
  it('nasce desligado e o toque liga no aparelho, sem passar pelo rascunho da máquina', async () => {
    await render();
    const toggle = () => container.querySelector(`[role="switch"][aria-label="${m.native_voice_hands_free()}"]`)!;
    expect(toggle().getAttribute('aria-checked')).toBe('false');
    await act(async () => (toggle() as HTMLElement).click());
    expect(toggle().getAttribute('aria-checked')).toBe('true');
    expect(saved.get('ditado.maosLivres')).toBe(true);
    expect(calls.patch).not.toHaveBeenCalled();
  });
});

describe('organização do ditado na tela de Voz', () => {
  it('organização ocupa a largura sem recuo e preserva rótulos, seleção e rascunho', async () => {
    const resolveLocale = getLocale;
    overwriteGetLocale(() => 'pt');
    try {
      await render();
      expect(container.textContent!.split(m.native_voice_cleanup()).length - 1).toBe(1);
      const group = container.querySelector<HTMLElement>(`[role="radiogroup"][aria-label="${m.native_voice_cleanup()}"]`)!;
      expect(group.parentElement?.style.paddingLeft).toBe('0px');
      expect([...group.querySelectorAll('button')].map((el) => el.textContent)).toEqual([
        'Sem organização', 'Harness da conversa', 'API externa',
      ]);
      expect([...group.querySelectorAll('button')].map((el) => el.getAttribute('aria-selected'))).toEqual(['true', 'false', 'false']);

      await click(m.native_voice_organization_harness());
      expect(button(m.native_voice_organization_harness()).getAttribute('aria-selected')).toBe('true');
      expect(button(m.native_voice_organization_none()).getAttribute('aria-selected')).toBe('false');
      expect(input(m.native_voice_harness_model_claude())?.parentElement?.style.paddingLeft).toBe('48px');
      await edit(m.native_voice_harness_model_claude(), 'claude-draft');
      expect(calls.patch).not.toHaveBeenCalled();
      expect(fields.dictation_organization_mode.valor).toBe('none');
      expect(fields.dictation_claude_model.valor).toBe('claude-old');
      await click(m.native_server_save());
      expect(calls.patch).toHaveBeenCalledExactlyOnceWith(server, {
        dictation_organization_mode: 'harness', dictation_claude_model: 'claude-draft',
      });
    } finally {
      overwriteGetLocale(resolveLocale);
    }
  });

  it('limite de gravação continua visível e salvável com organização desligada', async () => {
    await render();
    expect(container.textContent).not.toContain(m.native_voice_style());
    expect(input(m.native_voice_dictation_max())?.value).toBe('5');
    await edit(m.native_voice_dictation_max(), '12');
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenCalledWith(server, { dictation_max_minutes: '12' });
  });

  it('parte desligada, muda pelo rascunho e salva modelos, estilo e referência do harness', async () => {
    await render();
    expect(container.textContent).toContain(m.native_voice_transcribe());
    expect(container.textContent).toContain(m.native_voice_read());
    expect(container.textContent).not.toContain(m.native_voice_style());
    expect(container.textContent).not.toContain(m.native_voice_recent_messages());
    expect(input(m.native_voice_harness_model_claude())).toBeUndefined();
    expect(input(m.native_voice_llm_endpoint())).toBeUndefined();
    expect(container.textContent).not.toContain(m.native_voice_briefing_own());

    await click(m.native_voice_organization_harness());
    expect(calls.patch).not.toHaveBeenCalled();
    expect(fields.dictation_organization_mode.valor).toBe('none');
    expect(input(m.native_voice_harness_model_claude())?.value).toBe('claude-old');
    expect(input(m.native_voice_harness_model_codex())?.value).toBe('codex-old');
    expect(container.textContent).toContain(m.native_voice_harness_model_help());
    expect(input(m.native_voice_llm_endpoint())).toBeUndefined();
    expect(container.textContent).not.toContain(m.native_voice_briefing_own());
    await edit(m.native_voice_harness_model_claude(), 'claude-new');
    await edit(m.native_voice_harness_model_codex(), 'codex-new');
    await click(m.native_voice_recent_messages());
    await click(m.native_voice_style_briefing());
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenCalledWith(server, {
      dictation_organization_mode: 'harness', dictation_claude_model: 'claude-new', dictation_codex_model: 'codex-new',
      dictation_include_recent_messages: true, ditado_estilo: 'briefing',
    });
  });

  it('API externa mostra só sua configuração e briefing; desligar esconde a organização antes de salvar', async () => {
    fields.dictation_organization_mode.valor = 'harness';
    await render();
    await click(m.native_voice_organization_external_api());
    expect(input(m.native_voice_harness_model_claude())).toBeUndefined();
    expect(input(m.native_voice_harness_model_codex())).toBeUndefined();
    expect(input(m.native_voice_llm_endpoint())?.value).toBe('https://organization.test');
    await edit(m.native_voice_llm_model(), 'api-new');
    await edit(m.native_voice_llm_key(), 'new-key');
    await click(m.native_voice_briefing_own());
    await edit(m.native_voice_briefing_model(), 'briefing-new');
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenLastCalledWith(server, {
      dictation_organization_mode: 'external_api', llm_model: 'api-new', llm_api_key: 'new-key', llm_briefing_model: 'briefing-new',
    });

    await click(m.native_voice_organization_none());
    expect(input(m.native_voice_llm_endpoint())).toBeUndefined();
    expect(input(m.native_voice_briefing_model())).toBeUndefined();
    expect(container.textContent).not.toContain(m.native_voice_style());
    expect(container.textContent).not.toContain(m.native_voice_recent_messages());
    expect(container.textContent).not.toContain(m.native_voice_briefing_own());
    expect(container.textContent).toContain(m.native_voice_transcribe());
    expect(container.textContent).toContain(m.native_voice_elevenlabs_section());
    expect(container.textContent).toContain(m.native_voice_local_section());
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenLastCalledWith(server, { dictation_organization_mode: 'none' });
  });

  it.each([undefined, null, ''])('modo ausente ou vazio (%s) aparece desligado sem gravar o padrão', async (value) => {
    if (value === undefined) delete fields.dictation_organization_mode;
    else fields.dictation_organization_mode.valor = value;
    await render();
    expect(button(m.native_voice_organization_none()).getAttribute('aria-selected')).toBe('true');
    expect(container.textContent).not.toContain(m.native_voice_organization_unknown());
    expect(container.textContent).not.toContain(m.native_voice_style());
    expect(input(m.native_voice_harness_model_claude())).toBeUndefined();
    expect(input(m.native_voice_llm_endpoint())).toBeUndefined();
    await edit(m.native_voice_vocabulary(), 'vocabulário');
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenCalledWith(server, { ditado_vocabulario: 'vocabulário' });
  });

  it('modo desconhecido mantém todos os campos disponíveis sem selecionar nem sobrescrever o modo', async () => {
    fields.dictation_organization_mode.valor = 'future-mode';
    fields.llm_briefing_model.valor = 'briefing-saved';
    await render();
    expect(container.textContent).toContain(m.native_voice_organization_unknown());
    expect(container.textContent).toContain(m.native_voice_organization_unknown_hint());
    for (const label of [m.native_voice_organization_none(), m.native_voice_organization_harness(), m.native_voice_organization_external_api()]) {
      expect(button(label).getAttribute('aria-selected')).toBe('false');
    }
    expect(container.textContent).toContain(m.native_voice_style());
    expect(button(m.native_voice_recent_messages()).getAttribute('aria-checked')).toBe('false');
    expect(input(m.native_voice_harness_model_claude())?.value).toBe('claude-old');
    expect(input(m.native_voice_harness_model_codex())?.value).toBe('codex-old');
    expect(input(m.native_voice_llm_endpoint())?.value).toBe('https://organization.test');
    expect(input(m.native_voice_llm_key())).toBeDefined();
    expect(input(m.native_voice_llm_model())?.value).toBe('api-old');
    expect(button(m.native_voice_llm_effort_default())).toBeDefined();
    expect(input(m.native_voice_briefing_model())?.value).toBe('briefing-saved');
    await edit(m.native_voice_vocabulary(), 'preservar modo');
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenCalledWith(server, { ditado_vocabulario: 'preservar modo' });
    expect(fields.dictation_organization_mode.valor).toBe('future-mode');
  });

  it.each([undefined, null, '', 'future-style'])('estilo ausente ou desconhecido (%s) seleciona prosa sem gravar o padrão', async (value) => {
    fields.dictation_organization_mode.valor = 'harness';
    if (value === undefined) delete fields.ditado_estilo;
    else fields.ditado_estilo.valor = value;
    await render();
    expect(button(m.native_voice_style_prosa())?.getAttribute('aria-selected')).toBe('true');
    expect(button(m.native_voice_style_prosa())?.disabled).toBe(value === undefined);
    expect(container.textContent).toContain(m.native_voice_style_prosa_hint());
    await edit(m.native_voice_vocabulary(), 'não gravar estilo');
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenCalledWith(server, { ditado_vocabulario: 'não gravar estilo' });
  });

  it.each([
    { mode: 'none', label: m.native_voice_organization_none, expected: { dictation_organization_mode: 'none' } },
    { mode: 'harness', label: m.native_voice_organization_harness, expected: {
      dictation_organization_mode: 'harness', dictation_claude_model: 'claude-draft', dictation_codex_model: 'codex-draft',
      ditado_estilo: 'briefing', dictation_include_recent_messages: true,
    } },
    { mode: 'external_api', label: m.native_voice_organization_external_api, expected: {
      dictation_organization_mode: 'external_api', llm_base_url: 'https://draft.test', llm_api_key: 'key-draft', llm_model: 'api-draft',
      llm_reasoning_effort: 'high', llm_briefing_base_url: 'https://briefing-draft.test', llm_briefing_api_key: 'briefing-key-draft',
      llm_briefing_model: 'briefing-draft', ditado_estilo: 'briefing', dictation_include_recent_messages: true,
    } },
  ])('escolha explícita de $mode após modo desconhecido descarta só os rascunhos que ficam ocultos', async ({ mode, label, expected }) => {
    fields.dictation_organization_mode.valor = 'future-mode';
    await render();
    expect(input(m.native_voice_harness_model_claude())).toBeDefined();
    expect(input(m.native_voice_llm_endpoint())).toBeDefined();
    await edit(m.native_voice_harness_model_claude(), 'claude-draft');
    await edit(m.native_voice_harness_model_codex(), 'codex-draft');
    await edit(m.native_voice_llm_endpoint(), 'https://draft.test');
    await edit(m.native_voice_llm_key(), 'key-draft');
    await edit(m.native_voice_llm_model(), 'api-draft');
    await click('high');
    await click(m.native_voice_briefing_own());
    await edit(m.native_voice_briefing_endpoint(), 'https://briefing-draft.test');
    await edit(m.native_voice_briefing_key(), 'briefing-key-draft');
    await edit(m.native_voice_briefing_model(), 'briefing-draft');
    await click(m.native_voice_style_briefing());
    await click(m.native_voice_recent_messages());
    await click(label());
    expect(button(label()).getAttribute('aria-selected')).toBe('true');
    expect(input(m.native_voice_harness_model_claude()) !== undefined).toBe(mode === 'harness');
    expect(input(m.native_voice_llm_endpoint()) !== undefined).toBe(mode === 'external_api');
    expect(container.textContent?.includes(m.native_voice_style())).toBe(mode !== 'none');
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenCalledWith(server, expected);
    if (mode !== 'harness') expect(fields.dictation_claude_model.valor).toBe('claude-old');
    if (mode !== 'external_api') {
      expect(fields.llm_model.valor).toBe('api-old');
      expect(fields.llm_api_key.valor).toBe('masked-key');
      expect(fields.llm_briefing_model.valor).toBe('');
    }
    if (mode === 'none') {
      expect(fields.ditado_estilo.valor).toBe('limpar');
      expect(fields.dictation_include_recent_messages.valor).toBe(false);
    }
  });

  it('sair de harness descarta modelos e sair de API externa descarta seus campos antes do Salvar', async () => {
    fields.dictation_organization_mode.valor = 'harness';
    await render();
    await edit(m.native_voice_harness_model_claude(), 'hidden-claude');
    await edit(m.native_voice_harness_model_codex(), 'hidden-codex');
    await click(m.native_voice_organization_external_api());
    await edit(m.native_voice_llm_model(), 'hidden-api');
    await edit(m.native_voice_llm_key(), 'hidden-key');
    await click(m.native_voice_briefing_own());
    await edit(m.native_voice_briefing_model(), 'hidden-briefing');
    await click(m.native_voice_organization_harness());
    expect(input(m.native_voice_harness_model_claude())?.value).toBe('claude-old');
    expect(input(m.native_voice_harness_model_codex())?.value).toBe('codex-old');
    await edit(m.native_voice_vocabulary(), 'visível');
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenCalledWith(server, { dictation_organization_mode: 'harness', ditado_vocabulario: 'visível' });
    expect(fields.llm_api_key.valor).toBe('masked-key');
  });

  it('fechar briefing descarta seus rascunhos sem apagar os valores salvos nem reabrir ao Salvar', async () => {
    fields.dictation_organization_mode.valor = 'external_api';
    fields.llm_briefing_base_url.valor = 'https://briefing-saved.test';
    fields.llm_briefing_api_key = { valor: 'masked-briefing', origem: 'app', definido: true };
    fields.llm_briefing_model.valor = 'briefing-saved';
    await render();
    expect(button(m.native_voice_briefing_own()).getAttribute('aria-expanded')).toBe('true');
    await edit(m.native_voice_briefing_endpoint(), 'https://hidden.test');
    await edit(m.native_voice_briefing_key(), 'hidden-key');
    await edit(m.native_voice_briefing_model(), 'hidden-model');
    await edit(m.native_voice_llm_model(), 'visible-api');
    await click(m.native_voice_briefing_own());
    expect(input(m.native_voice_briefing_model())).toBeUndefined();
    await click(m.native_server_save());
    expect(calls.patch).toHaveBeenCalledWith(server, { llm_model: 'visible-api' });
    expect(button(m.native_voice_briefing_own()).getAttribute('aria-expanded')).toBe('false');
    await click(m.native_voice_briefing_own());
    expect(input(m.native_voice_briefing_endpoint())?.value).toBe('https://briefing-saved.test');
    expect(input(m.native_voice_briefing_model())?.value).toBe('briefing-saved');
    expect(input(m.native_voice_briefing_key())?.value).toBe('');
    expect(fields.llm_briefing_api_key.valor).toBe('masked-briefing');
  });

  it('abre cada seção com seus valores salvos e mantém os controles de abertura independentes', async () => {
    fields.dictation_organization_mode.valor = 'external_api';
    fields.transcription_model.valor = 'transcription-saved';
    fields.llm_briefing_model.valor = 'briefing-saved';
    fields.elevenlabs_api_key = { valor: 'masked-eleven', origem: 'app', definido: true };
    fields.tts_local_cmd.valor = 'reader-saved';
    await render();
    expect(input(m.native_voice_transcription_model())?.value).toBe('transcription-saved');
    expect(input(m.native_voice_briefing_model())?.value).toBe('briefing-saved');
    expect(input(m.native_voice_elevenlabs_key())).toBeDefined();
    expect(input(m.native_voice_local_cmd())?.value).toBe('reader-saved');
    expect(button(m.native_voice_tune()).getAttribute('aria-expanded')).toBe('false');
    await click(m.native_voice_tune());
    expect(button(m.native_voice_tune()).getAttribute('aria-expanded')).toBe('true');
    await click(m.native_voice_transcribe_other());
    expect(input(m.native_voice_transcription_model())).toBeUndefined();
    expect(input(m.native_voice_briefing_model())).toBeDefined();
    await click(m.native_voice_elevenlabs_section());
    expect(input(m.native_voice_elevenlabs_key())).toBeUndefined();
    expect(input(m.native_voice_local_cmd())?.value).toBe('reader-saved');
    await click(m.native_voice_local_section());
    expect(input(m.native_voice_local_cmd())).toBeUndefined();
    await click(m.native_voice_briefing_own());
    expect(input(m.native_voice_briefing_model())).toBeUndefined();
    await click(m.native_voice_local_section());
    expect(input(m.native_voice_local_cmd())?.value).toBe('reader-saved');
    expect(calls.patch).not.toHaveBeenCalled();
  });

  it('não grava campos que um servidor antigo não trouxe', async () => {
    delete fields.dictation_organization_mode;
    await render();
    expect(button(m.native_voice_organization_harness()).disabled).toBe(true);
    await click(m.native_voice_organization_harness());
    expect(input(m.native_voice_harness_model_claude())).toBeUndefined();
    expect(calls.patch).not.toHaveBeenCalled();
  });
});
