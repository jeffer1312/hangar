// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { ServerBucket, WhatsAppDraft, WhatsAppDraftRecent, WhatsAppOutboxEvent } from '@hangar/core';
import { OutboxRequestSheet, OutboxRequests, SessionOutboxRequests } from './OutboxRequests';
import { openWhatsAppDraft, useWhatsAppTarget } from './target';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';

const mocks = vi.hoisted(() => ({ answer: vi.fn(), sendEdited: vi.fn(), status: vi.fn(), ok: vi.fn(), erro: vi.fn() }));

vi.mock('@hangar/core', async (original) => ({
  ...await original<object>(), answerWhatsAppDraft: mocks.answer, sendEditedWhatsAppDraft: mocks.sendEdited, getWhatsApp: mocks.status,
}));
vi.mock('expo-image', () => ({ Image: ({ source, alt }: { source: { uri: string }; alt: string }) => createElement('img', { src: source.uri, alt }) }));
vi.mock('react-native', async (original) => {
  const actual = await original<typeof import('react-native')>();
  return { ...actual,
    TextInput: ({ onChangeText, accessibilityLabel, value, editable }: any) => createElement('textarea', {
      'aria-label': accessibilityLabel, value: value ?? '', disabled: editable === false,
      onInput: (event: { currentTarget: HTMLTextAreaElement }) => onChangeText?.(event.currentTarget.value),
    }),
  };
});
vi.mock('../machines/parts', () => ({
  MachineSheet: ({ title, children }: { title: string; children: ReactNode }) => createElement('section', { 'aria-label': title }, children),
  Notice: ({ text }: { text: string }) => text ? createElement('p', { role: 'alert' }, text) : null,
}));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: mocks.ok, erro: mocks.erro } }));
vi.mock('../pair/identities', () => ({ useIdentities: () => ({ identities: new Map([['a', 'a']]), loading: new Set() }) }));

// Relógio fixo: o cartão conta até answer_by (o prazo da tool, antes do expires_at).
const NOW = new Date('2026-11-24T14:00:48Z');

const draft = (id: string, extra: Partial<WhatsAppDraft> = {}): WhatsAppDraft => ({
  id, origin: 'a::sessao', chat: '5511987654321@s.whatsapp.net', chat_kind: 'contact', chat_number: '5511987654321',
  chat_name: 'Carlos', chat_name_source: 'contact', chat_avatar: null, recent: [], text: 'nada perdido\u200B', reply_to: '', quoted_text: '',
  file: null, created_at: '2026-11-24T14:00:00Z', expires_at: '2026-11-24T14:10:00Z', answer_by: '2026-11-24T14:09:30Z', state: 'pending',
  ...extra,
});
const event = (...drafts: WhatsAppDraft[]): WhatsAppOutboxEvent => ({ drafts, edit: false });
const editable = (...drafts: WhatsAppDraft[]): WhatsAppOutboxEvent => ({ drafts, edit: true });
const bucket = (id: string, extra: Partial<ServerBucket> = {}) => ({
  server: { id, label: id, baseUrl: `https://${id}.test`, token: 't' }, sessions: [], error: null, loaded: true, ...extra,
}) as unknown as ServerBucket;

let container: HTMLElement;
let root: Root;
const render = (byServer: ServerBucket[], outbox: Record<string, WhatsAppOutboxEvent>) => act(async () => {
  useSessions.setState({ byServer, whatsappOutbox: outbox });
  root.render(<><OutboxRequests byServer={byServer} /><OutboxRequestSheet /></>);
});
const notice = (label: string, count = 1) => container.querySelector<HTMLElement>(`button[aria-label="${m.whatsapp_draft_requests_title({ label, count })}"]`);
const button = (label: string) => container.querySelector<HTMLElement>(`button[aria-label="${label}"]`);
const sheet = () => container.querySelector(`section[aria-label="${m.whatsapp_draft_title()}"]`);

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(NOW);
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement('div');
  root = createRoot(container);
  openWhatsAppDraft(null);
  mocks.answer.mockReset().mockResolvedValue({});
  mocks.sendEdited.mockReset();
  mocks.status.mockReset();
  mocks.ok.mockReset();
  mocks.erro.mockReset();
});

afterEach(async () => {
  await act(async () => { root.unmount(); });
  useSessions.setState({ whatsappOutbox: {}, byServer: [] });
  openWhatsAppDraft(null);
  vi.useRealTimers();
});

it('tocar no aviso da máquina abre a folha com o número, o texto exato e o invisível marcado', async () => {
  await render([bucket('a'), bucket('b')], { a: event(draft('d1')) });
  expect(notice('b')).toBeNull();
  await act(async () => { notice('a')!.click(); });
  expect(useWhatsAppTarget.getState().target).toEqual({ serverId: 'a', draftId: null });
  expect(sheet()).not.toBeNull();
  expect(container.textContent).toContain('5511987654321');
  expect(container.textContent).toContain('nada perdido⟨U+200B⟩');
  expect(container.textContent).toContain(m.whatsapp_draft_text({ count: 13 }));
  expect(container.textContent).toContain('8:42');
});

it('máquina com erro ou ainda carregando não mostra o aviso', async () => {
  await render([bucket('a', { error: 'offline' })], { a: event(draft('d2')) });
  expect(notice('a')).toBeNull();
  await render([bucket('a', { loaded: false })], { a: event(draft('d2')) });
  expect(notice('a')).toBeNull();
});

it('no chat da sessão que pediu, o aviso abre aquele rascunho; em outra sessão, nada', async () => {
  useSessions.setState({ byServer: [bucket('a')], whatsappOutbox: { a: event(draft('d3')) } });
  const banner = m.whatsapp_draft_session_notice({ name: '5511987654321 · Carlos' });
  await act(async () => { root.render(<SessionOutboxRequests serverId="a" name="outra" />); });
  expect(button(banner)).toBeNull();
  await act(async () => { root.render(<><SessionOutboxRequests serverId="a" name="sessao" /><OutboxRequestSheet /></>); });
  await act(async () => { button(banner)!.click(); });
  expect(useWhatsAppTarget.getState().target).toEqual({ serverId: 'a', draftId: 'd3' });
  expect(container.textContent).toContain('5511987654321');
});

it('sem o identificador da máquina, o chat só mostra rascunho guardado nela, mesmo com sessão homônima em outra', async () => {
  // 'c' e 'd' ficam fora do mapa de identificadores do mock: o aviso cai no ramo pelo nome da sessão.
  useSessions.setState({
    byServer: [bucket('c'), bucket('d')],
    whatsappOutbox: { c: event(draft('d11', { origin: 'x::sessao' })), d: event(draft('d12', { origin: 'y::sessao' })) },
  });
  const banner = m.whatsapp_draft_session_notice({ name: '5511987654321 · Carlos' });
  await act(async () => { root.render(<SessionOutboxRequests serverId="c" name="sessao" />); });
  expect(container.querySelectorAll(`button[aria-label="${banner}"]`)).toHaveLength(1);
  await act(async () => { button(banner)!.click(); });
  expect(useWhatsAppTarget.getState().target).toEqual({ serverId: 'c', draftId: 'd11' });
});

it('no chat, rascunho de máquina com a lista caída não aparece', async () => {
  useSessions.setState({ byServer: [bucket('a', { error: 'offline' })], whatsappOutbox: { a: event(draft('d9')) } });
  await act(async () => { root.render(<SessionOutboxRequests serverId="a" name="sessao" />); });
  expect(button(m.whatsapp_draft_session_notice({ name: '5511987654321 · Carlos' }))).toBeNull();
});

it('sem answer_by legível, conta até expires_at', async () => {
  await render([bucket('a')], { a: event(draft('d10', { answer_by: '' })) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: 'd10' }); });
  expect(container.textContent).toContain('9:12');
});

it('Enviar responde com via mobile e o cartão some', async () => {
  await render([bucket('a')], { a: event(draft('d4')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: null }); });
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  expect(mocks.answer).toHaveBeenCalledWith(expect.objectContaining({ id: 'a' }), 'd4', 'send', 'mobile');
  expect(mocks.ok).toHaveBeenCalledWith(m.whatsapp_draft_sending());
  expect(useWhatsAppTarget.getState().target).toBeNull();
  expect(notice('a')).toBeNull();
});

it('dois toques seguidos em Enviar respondem uma vez só', async () => {
  let release!: () => void;
  mocks.answer.mockReturnValueOnce(new Promise<void>((resolve) => { release = resolve; }));
  await render([bucket('a')], { a: event(draft('d20')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: 'd20' }); });
  await act(async () => { const send = button(m.whatsapp_draft_send())!; send.click(); send.click(); });
  expect(mocks.answer).toHaveBeenCalledTimes(1);
  await act(async () => { release(); });
});

it('resposta que chega depois de a pessoa abrir outro pedido em outra máquina não tira a folha dele', async () => {
  let release!: () => void;
  mocks.answer.mockReturnValueOnce(new Promise<void>((resolve) => { release = resolve; }));
  await render([bucket('a'), bucket('b')], { a: event(draft('d21')), b: event(draft('e1')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: 'd21' }); });
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  await act(async () => { openWhatsAppDraft({ serverId: 'b', draftId: 'e1' }); });
  await act(async () => { release(); });
  expect(mocks.ok).toHaveBeenCalledWith(m.whatsapp_draft_sending());
  expect(notice('a')).toBeNull();
  expect(useWhatsAppTarget.getState().target).toEqual({ serverId: 'b', draftId: 'e1' });
});

it('Recusar responde decline', async () => {
  await render([bucket('a')], { a: event(draft('d5')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: 'd5' }); });
  await act(async () => { button(m.whatsapp_draft_decline())!.click(); });
  expect(mocks.answer).toHaveBeenCalledWith(expect.objectContaining({ id: 'a' }), 'd5', 'decline', 'mobile');
  expect(mocks.ok).toHaveBeenCalledWith(m.whatsapp_draft_declined());
});

it('recusa com frase própria deixa o cartão; a trava diz até quando pelo status', async () => {
  await render([bucket('a')], { a: event(draft('d6')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: 'd6' }); });
  mocks.answer.mockRejectedValueOnce(Object.assign(new Error('429: limite'), { status: 429, code: 'whatsapp_rate_limited' }));
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.whatsapp_draft_err_rate_limited());
  expect(sheet()).not.toBeNull();

  mocks.answer.mockRejectedValueOnce(Object.assign(new Error('409: trava'), { status: 409, code: 'whatsapp_reachout_locked' }));
  mocks.status.mockResolvedValueOnce({ status: { reachout_locked_until: '2026-11-25T12:30:00Z' } });
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  const text = container.querySelector('[role="alert"]')?.textContent ?? '';
  expect(text).not.toBe(m.whatsapp_draft_err_reachout_locked_unknown());
  expect(text).toMatch(/\d{2}:\d{2}/);

  mocks.answer.mockRejectedValueOnce(Object.assign(new Error('409: velho'), { status: 409, code: 'whatsapp_outdated' }));
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.whatsapp_draft_err_outdated());
});

it('expirado tira o cartão e avisa o motivo', async () => {
  await render([bucket('a')], { a: event(draft('d7')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: null }); });
  mocks.answer.mockRejectedValueOnce(Object.assign(new Error('409: expired'), { status: 409, code: 'whatsapp_expired' }));
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  expect(mocks.erro).toHaveBeenCalledWith(m.whatsapp_draft_err_expired());
  expect(useWhatsAppTarget.getState().target).toBeNull();
  expect(notice('a')).toBeNull();
});

it('anexo fora da pasta da sessão ganha a linha de aviso e o nome escolhido pela pessoa, o dela', async () => {
  const file = { name: 'custos.xlsx', size: 88 * 1024, sha256: 'x', source: '/home/j/custos.xlsx', outside_cwd: true };
  await render([bucket('a')], { a: event(draft('d8', { file, chat_name_source: 'push' })) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: 'd8' }); });
  expect(container.textContent).toContain('/home/j/custos.xlsx');
  expect(container.textContent).toContain('custos.xlsx · 88 KB');
  expect(container.textContent).toContain(m.whatsapp_draft_outside_cwd());
  expect(container.textContent).toContain(m.whatsapp_draft_push_warning());
});

// Base64 de um JPEG (começa pelos bytes FF D8 FF).
const AVATAR = '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBD';
const recent = (extra: Partial<WhatsAppDraftRecent>): WhatsAppDraftRecent => ({
  from_me: false, sender: 'Carlos', at: '2026-11-24T14:02:00Z', text: '', kind: 'text', file_name: '', ...extra,
});
const field = () => container.querySelector<HTMLTextAreaElement>('textarea');
// O React só vê a mudança pelo setter nativo do campo, não pela atribuição direta.
const type = (value: string) => act(async () => {
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field()!, value);
  field()!.dispatchEvent(new Event('input', { bubbles: true }));
});
const openDraft = async (draftId: string, outboxEvent: WhatsAppOutboxEvent) => {
  await render([bucket('a')], { a: outboxEvent });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId }); });
};

it('com foto, o destino mostra a imagem; sem foto, a inicial do nome e, sem nome, o primeiro dígito', async () => {
  await openDraft('f1', event(draft('f1', { chat_avatar: AVATAR })));
  const img = container.querySelector('img');
  expect(img?.getAttribute('src')).toBe(`data:image/jpeg;base64,${AVATAR}`);
  expect(img?.getAttribute('alt')).toBe(m.whatsapp_draft_avatar({ name: 'Carlos' }));

  await openDraft('f2', event(draft('f2', { chat_name: 'piterson' })));
  expect(container.querySelector('img')).toBeNull();
  expect(container.textContent).toContain('P5511987654321');

  await openDraft('f3', event(draft('f3', { chat_name: '', chat_number: '5511987654321' })));
  expect(container.textContent).toContain('55511987654321');
});

it('últimas mensagens em balões: as minhas à direita, as dele à esquerda com o nome, e a mídia com o tipo', async () => {
  await openDraft('b1', event(draft('b1', { recent: [
    recent({ from_me: true, sender: '', text: 'nao tem 2 fatores?' }),
    recent({ kind: 'image' }),
    recent({ kind: 'document', file_name: 'nota.pdf' }),
    recent({ kind: 'audio', text: 'escuta\u200B' }),
  ] })));
  const bubbles = [...container.querySelectorAll<HTMLElement>('[aria-label]')].filter((e) => e.tagName === 'DIV');
  const mine = bubbles.find((b) => b.textContent?.includes('nao tem 2 fatores?'))!;
  expect(mine.style.alignSelf).toBe('flex-end');
  expect(mine.getAttribute('aria-label')).toContain(m.whatsapp_draft_me());
  const photo = bubbles.find((b) => b.textContent?.includes(m.whatsapp_draft_kind_image().toLocaleUpperCase('pt-BR')))!;
  expect(photo.style.alignSelf).toBe('flex-start');
  expect(photo.textContent).toContain(m.whatsapp_draft_kind_image());
  expect(photo.textContent).toContain('Carlos · ');
  expect(container.textContent).toContain(`${m.whatsapp_draft_kind_document().toLocaleUpperCase('pt-BR')}nota.pdf`);
  // Texto de terceiro tem os invisíveis marcados, como o do rascunho.
  expect(container.textContent).toContain('escuta⟨U+200B⟩');
});

it('Editar só aparece quando a máquina diz que o serviço aceita a edição', async () => {
  await openDraft('e1', event(draft('e1')));
  expect(button(m.whatsapp_draft_edit())).toBeNull();
  await openDraft('e1', editable(draft('e1')));
  expect(button(m.whatsapp_draft_edit())).not.toBeNull();
});

it('o botão vira "Enviar texto editado" só com o texto diferente; Desfazer volta ao texto da sessão', async () => {
  await openDraft('e2', editable(draft('e2', { text: 'linha 1\nlinha 2' })));
  await act(async () => { button(m.whatsapp_draft_edit())!.click(); });
  expect(field()?.value).toBe('linha 1\nlinha 2');
  expect(container.textContent).toContain(m.whatsapp_draft_text_editing({ count: 15 }));
  // \r\n é a mesma quebra depois de normalizar: continua o Enviar de sempre.
  await type('linha 1\r\nlinha 2');
  expect(button(m.whatsapp_draft_send())).not.toBeNull();
  expect(button(m.whatsapp_draft_send_edited())).toBeNull();
  await type('linha 1');
  expect(button(m.whatsapp_draft_send_edited())).not.toBeNull();
  expect(container.textContent).toContain(m.whatsapp_draft_text_editing({ count: 7 }));
  await act(async () => { button(m.whatsapp_draft_edit_undo())!.click(); });
  expect(field()).toBeNull();
  expect(button(m.whatsapp_draft_send())).not.toBeNull();
  expect(container.textContent).toContain('linha 1\nlinha 2');
});

it('Enviar texto editado vai pela rota da edição com o texto normalizado e confere o sent_text', async () => {
  mocks.sendEdited.mockResolvedValueOnce({ id: 'e3', sent_text: 'novo\ntexto' });
  await openDraft('e3', editable(draft('e3')));
  await act(async () => { button(m.whatsapp_draft_edit())!.click(); });
  await type('novo\r\ntexto');
  await act(async () => { button(m.whatsapp_draft_send_edited())!.click(); });
  expect(mocks.sendEdited).toHaveBeenCalledWith(expect.objectContaining({ id: 'a' }), 'e3', 'novo\ntexto', 'mobile');
  expect(mocks.answer).not.toHaveBeenCalled();
  expect(mocks.ok).toHaveBeenCalledWith(m.whatsapp_draft_sending());
  expect(useWhatsAppTarget.getState().target).toBeNull();
});

it('sent_text diferente do editado nunca vira "enviando": avisa para conferir a conversa', async () => {
  mocks.sendEdited.mockResolvedValueOnce({ id: 'e4', sent_text: 'nada perdido\u200B' });
  await openDraft('e4', editable(draft('e4')));
  await act(async () => { button(m.whatsapp_draft_edit())!.click(); });
  await type('outro');
  await act(async () => { button(m.whatsapp_draft_send_edited())!.click(); });
  expect(mocks.ok).not.toHaveBeenCalled();
  expect(mocks.erro).toHaveBeenCalledWith(m.whatsapp_draft_err_sent_text_mismatch());
});

it('serviço sem a edição deixa o cartão com a frase de atualizar', async () => {
  mocks.sendEdited.mockRejectedValueOnce(Object.assign(new Error('409: api'), { status: 409, code: 'whatsapp_edit_unsupported' }));
  await openDraft('e5', editable(draft('e5')));
  await act(async () => { button(m.whatsapp_draft_edit())!.click(); });
  await type('outro');
  await act(async () => { button(m.whatsapp_draft_send_edited())!.click(); });
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.whatsapp_draft_err_edit_unsupported());
  expect(sheet()).not.toBeNull();
  expect(field()?.value).toBe('outro');
});

it('campo aberto continua editado quando o evento passa a dizer edit: false: nunca sai o original', async () => {
  mocks.sendEdited.mockRejectedValueOnce(Object.assign(new Error('409: api'), { status: 409, code: 'whatsapp_edit_unsupported' }));
  await openDraft('e7', editable(draft('e7')));
  await act(async () => { button(m.whatsapp_draft_edit())!.click(); });
  await type('outro');
  await act(async () => { useSessions.setState({ whatsappOutbox: { a: event(draft('e7')) } }); });
  expect(field()?.value).toBe('outro');
  await act(async () => { button(m.whatsapp_draft_send_edited())!.click(); });
  expect(mocks.sendEdited).toHaveBeenCalledWith(expect.objectContaining({ id: 'a' }), 'e7', 'outro', 'mobile');
  expect(mocks.answer).not.toHaveBeenCalled();
});

it('texto vazio sem anexo ou acima de 4096 desabilita o envio e diz o motivo no cartão', async () => {
  await openDraft('e6', editable(draft('e6')));
  await act(async () => { button(m.whatsapp_draft_edit())!.click(); });
  await type('  ');
  expect(button(m.whatsapp_draft_send_edited())?.hasAttribute('disabled')).toBe(true);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.whatsapp_draft_err_text_empty());
  await type('a'.repeat(4097));
  expect(button(m.whatsapp_draft_send_edited())?.hasAttribute('disabled')).toBe(true);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.whatsapp_draft_err_text_long());
  await type('a'.repeat(4096));
  expect(button(m.whatsapp_draft_send_edited())?.hasAttribute('disabled')).toBe(false);
  expect(container.querySelector('[role="alert"]')).toBeNull();
});

it('com anexo, o texto editado pode ficar vazio', async () => {
  const file = { name: 'a.pdf', size: 10, sha256: 'x', source: '/home/j/a.pdf', outside_cwd: false };
  await openDraft('e7', editable(draft('e7', { file })));
  await act(async () => { button(m.whatsapp_draft_edit())!.click(); });
  await type('');
  expect(button(m.whatsapp_draft_send_edited())?.hasAttribute('disabled')).toBe(false);
});
