import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  answerWhatsAppDraft, draftAvatarUri, draftCharCount, draftField, draftInitial, draftRefusalEndsCard, draftSecondsLeft, draftTextError,
  invisibleMark, markInvisibles, normalizeDraftText, parseWhatsAppOutboxEvent, recentBubble, recentSpeaker, sendEditedWhatsAppDraft, sentTextMatches,
  whatsappDraftRefusalText,
} from './whatsappOutbox';
import { formatCountdown } from './format';
import invisibleCases from './whatsappInvisible.fixtures.json';
import * as m from './paraglide/messages';

const server = { id: 'a', label: 'Servidor A', baseUrl: 'https://a.test', token: 'token-a' };

const draft = (extra: Record<string, unknown> = {}) => ({
  id: 'd1', origin: 'casa::web', chat: '5511987654321@s.whatsapp.net', chat_kind: 'contact', chat_number: '5511987654321',
  chat_name: 'Carlos', chat_name_source: 'push', recent: [{ from_me: false, sender: 'Carlos', at: '2026-11-24T14:02:00Z', text: 'oi' }],
  text: 'segue', reply_to: '', quoted_text: '', file: null, created_at: '2026-11-24T14:00:00Z', expires_at: '2026-11-24T14:10:00Z',
  answer_by: '2026-11-24T14:09:30Z', state: 'pending', via: '', answered_at: '', message_id: '', error: '', wa_code: 0, ...extra,
});

afterEach(() => vi.restoreAllMocks());

describe('evento whatsapp_outbox', () => {
  it('lê os pendentes e deixa de fora o que não é pendente ou não tem o necessário', () => {
    const event = parseWhatsAppOutboxEvent(JSON.stringify({ drafts: [
      draft({ file: { name: 'a.zip', size: 10, sha256: 'x', source: '/home/a.zip', outside_cwd: true } }),
      draft({ id: 'd2', state: 'sending' }),
      draft({ id: 'd3', chat_number: '' }),
      { id: 'd4' },
    ] }));
    expect(event?.drafts.map((d) => d.id)).toEqual(['d1']);
    expect(event?.drafts[0]).toMatchObject({ chat_name_source: 'push', chat_kind: 'contact', file: { name: 'a.zip', outside_cwd: true } });
    expect(event?.drafts[0].recent).toEqual([{ from_me: false, sender: 'Carlos', at: '2026-11-24T14:02:00Z', text: 'oi', kind: 'text', file_name: '' }]);
    expect(event?.drafts[0].answer_by).toBe('2026-11-24T14:09:30Z');
  });

  it('lista vazia é sem cartões; corpo torto é null (o anterior fica)', () => {
    expect(parseWhatsAppOutboxEvent('{"drafts":[]}')).toEqual({ drafts: [], edit: false });
    expect(parseWhatsAppOutboxEvent('{"drafts":{}}')).toBeNull();
    expect(parseWhatsAppOutboxEvent('não é json')).toBeNull();
  });

  it('lê a edição e a foto; sem elas (serviço e hangar-server antigos), sem foto e sem Editar', () => {
    const avatar = '/9j/4AAQSkZJRgABAQAAAQABAAD/2w==';
    const now = parseWhatsAppOutboxEvent(JSON.stringify({ edit: true, drafts: [draft({ chat_avatar: avatar })] }));
    expect(now?.edit).toBe(true);
    expect(now?.drafts[0]).toMatchObject({ chat_avatar: avatar });
    const old = parseWhatsAppOutboxEvent(JSON.stringify({ drafts: [draft()] }));
    expect(old?.edit).toBe(false);
    expect(old?.drafts[0]).toMatchObject({ chat_avatar: null });
    expect(parseWhatsAppOutboxEvent(JSON.stringify({ edit: 'true', drafts: [] }))?.edit).toBe(false);
  });

  it('foto que não é base64 de JPEG vira sem foto', () => {
    const avatarOf = (chat_avatar: unknown) => parseWhatsAppOutboxEvent(JSON.stringify({ drafts: [draft({ chat_avatar })] }))?.drafts[0].chat_avatar;
    expect(avatarOf(null)).toBeNull();
    expect(avatarOf('')).toBeNull();
    expect(avatarOf('não é base64')).toBeNull();
    // PNG em base64 começa por iVBOR: o endereço da imagem diz JPEG.
    expect(avatarOf('iVBORw0KGgo=')).toBeNull();
    expect(avatarOf(42)).toBeNull();
  });

  it('recentes: sem kind é texto, reação sai, tipo desconhecido é other e o nome do documento vem junto', () => {
    const recent = [
      { from_me: true, sender: '', at: '2026-11-24T14:01:00Z', text: 'oi' },
      { from_me: false, sender: 'Carlos', at: '2026-11-24T14:02:00Z', text: '👍', kind: 'reaction' },
      { from_me: false, sender: 'Carlos', at: '2026-11-24T14:03:00Z', text: '', kind: 'document', file_name: 'nota.pdf' },
      { from_me: false, sender: 'Carlos', at: '2026-11-24T14:04:00Z', text: 'enquete', kind: 'poll' },
    ];
    const event = parseWhatsAppOutboxEvent(JSON.stringify({ drafts: [draft({ recent })] }));
    expect(event?.drafts[0].recent).toEqual([
      { from_me: true, sender: '', at: '2026-11-24T14:01:00Z', text: 'oi', kind: 'text', file_name: '' },
      { from_me: false, sender: 'Carlos', at: '2026-11-24T14:03:00Z', text: '', kind: 'document', file_name: 'nota.pdf' },
      { from_me: false, sender: 'Carlos', at: '2026-11-24T14:04:00Z', text: 'enquete', kind: 'other', file_name: '' },
    ]);
  });

  it('origem do nome desconhecida vira vazia e anexo sem outside_cwd é destacado', () => {
    const event = parseWhatsAppOutboxEvent(JSON.stringify({ drafts: [draft({ chat_name_source: 'xyz', file: { name: 'b', size: 1, source: '/b' } })] }));
    expect(event?.drafts[0].chat_name_source).toBe('');
    expect(event?.drafts[0].file?.outside_cwd).toBe(true);
  });
});

describe('caracteres invisíveis', () => {
  const shown = (text: string) => markInvisibles(text).map((p) => (p.kind === 'mark' ? invisibleMark(p.code) : p.text)).join('');

  // Os mesmos casos do nativo (`whatsapp_outbox.rs`).
  it.each(invisibleCases.cases)('$name', ({ input, shown: expected }) => {
    expect(shown(input)).toBe(expected);
  });

  it('quebra de linha e tabulação ficam num trecho só', () => {
    expect(markInvisibles('a\n\tb')).toEqual([{ kind: 'text', text: 'a\n\tb' }]);
  });

  it('separa os trechos na ordem', () => {
    expect(markInvisibles('a\u2060b')).toEqual([{ kind: 'text', text: 'a' }, { kind: 'mark', code: 'U+2060' }, { kind: 'text', text: 'b' }]);
  });
});

describe('cartão', () => {
  it('conta pontos de código, não unidades UTF-16', () => {
    expect(draftCharCount('🚀a')).toBe(2);
  });

  it('conta até answer_by; sem ele legível, até expires_at', () => {
    const now = Date.parse('2026-11-24T14:00:48Z');
    const left = (answer_by: string, expires_at = '2026-11-24T14:10:00Z') => draftSecondsLeft({ answer_by, expires_at }, now);
    // O Hangar manda o prazo da tool (14:09:30), antes das 14:10:00 do serviço.
    expect(left('2026-11-24T14:09:30Z')).toBe(522);
    expect(formatCountdown(522)).toBe('8:42');
    expect(left('')).toBe(552);
    expect(formatCountdown(552)).toBe('9:12');
    expect(left('ruim')).toBe(552);
    expect(left('2026-11-24T13:00:00Z')).toBe(0);
    expect(left('', 'ruim')).toBe(0);
    expect(draftSecondsLeft({ answer_by: '2026-11-24T14:09:30Z', expires_at: '2026-11-24T14:10:00Z' }, Date.parse('2026-11-24T14:09:45Z'))).toBe(0);
  });

  it('expirado, respondido e inexistente tiram o cartão; os outros ficam', () => {
    expect(['whatsapp_expired', 'whatsapp_answered', 'whatsapp_not_found'].every(draftRefusalEndsCard)).toBe(true);
    expect(draftRefusalEndsCard('whatsapp_rate_limited')).toBe(false);
    expect(draftRefusalEndsCard(undefined)).toBe(false);
  });

  it('cada recusa tem frase própria; a trava diz até quando quando o status diz', () => {
    expect(whatsappDraftRefusalText('whatsapp_outdated')).toBe(m.whatsapp_draft_err_outdated());
    expect(whatsappDraftRefusalText('whatsapp_send_unsupported')).toBe(m.whatsapp_draft_err_outdated());
    expect(whatsappDraftRefusalText('whatsapp_rate_limited')).toBe(m.whatsapp_draft_err_rate_limited());
    expect(whatsappDraftRefusalText('whatsapp_not_paired')).toBe(m.whatsapp_draft_err_not_paired());
    expect(whatsappDraftRefusalText('whatsapp_expired')).toBe(m.whatsapp_draft_err_expired());
    expect(whatsappDraftRefusalText('whatsapp_reachout_locked')).toBe(m.whatsapp_draft_err_reachout_locked_unknown());
    expect(whatsappDraftRefusalText('whatsapp_new_contact')).toBe(m.whatsapp_draft_err_new_contact());
    const locked = whatsappDraftRefusalText('whatsapp_reachout_locked', '2026-11-25T12:30:00Z');
    expect(locked).not.toBe(m.whatsapp_draft_err_reachout_locked_unknown());
    expect(locked).toMatch(/\d{2}:\d{2}/);
    expect(whatsappDraftRefusalText('whatsapp_qualquer')).toBeUndefined();
  });

  it('envio recusado durante a atualização ou com a vez ocupada tem frase do app e o cartão fica', () => {
    expect(whatsappDraftRefusalText('whatsapp_updating')).toBe(m.native_whatsapp_send_updating());
    expect(whatsappDraftRefusalText('whatsapp_busy')).toBe(m.native_whatsapp_update_busy());
    expect(draftRefusalEndsCard('whatsapp_updating')).toBe(false);
    expect(draftRefusalEndsCard('whatsapp_busy')).toBe(false);
  });
});

describe('answerWhatsAppDraft', () => {
  it('Enviar manda o via; Recusar vai sem corpo', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify({ id: 'a b', state: 'sending' }), { status: 202 }));
    await expect(answerWhatsAppDraft(server, 'a b', 'send', 'mobile')).resolves.toEqual({ id: 'a b', state: 'sending' });
    await answerWhatsAppDraft(server, 'a b', 'decline', 'mobile');
    const [[sendUrl, sendInit], [declineUrl, declineInit]] = fetchMock.mock.calls;
    expect(sendUrl).toBe('https://a.test/api/whatsapp/outbox/a%20b/send');
    expect(sendInit?.method).toBe('POST');
    expect(JSON.parse(String(sendInit?.body))).toEqual({ via: 'mobile' });
    expect(declineUrl).toBe('https://a.test/api/whatsapp/outbox/a%20b/decline');
    expect(declineInit?.method).toBe('POST');
    expect(declineInit?.body).toBeUndefined();
  });

  it('a recusa chega com o código whatsapp_*', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(
      JSON.stringify({ detail: { code: 'whatsapp_answered', params: {}, msg: 'answered' } }), { status: 409 }));
    await expect(answerWhatsAppDraft(server, 'd1', 'send', 'desktop')).rejects.toMatchObject({ status: 409, code: 'whatsapp_answered' });
  });
});

describe('balões e foto', () => {
  const upper = (word: string) => word.toLocaleUpperCase('pt-BR');

  it('mídia mostra a etiqueta do tipo; sem legenda, o nome do documento ou a palavra do tipo', () => {
    expect(recentBubble({ kind: 'image', text: '', file_name: '' })).toEqual({ tag: upper(m.whatsapp_draft_kind_image()), text: m.whatsapp_draft_kind_image() });
    expect(recentBubble({ kind: 'image', text: 'olha isso', file_name: '' })).toEqual({ tag: upper(m.whatsapp_draft_kind_image()), text: 'olha isso' });
    expect(recentBubble({ kind: 'document', text: '', file_name: 'nota.pdf' })).toEqual({ tag: upper(m.whatsapp_draft_kind_document()), text: 'nota.pdf' });
    expect(recentBubble({ kind: 'document', text: ' ', file_name: '' })).toEqual({ tag: upper(m.whatsapp_draft_kind_document()), text: m.whatsapp_draft_kind_document() });
    // O nome do arquivo só vale para documento.
    expect(recentBubble({ kind: 'audio', text: '', file_name: 'x.ogg' }).text).toBe(m.whatsapp_draft_kind_audio());
    expect(recentBubble({ kind: 'video', text: '', file_name: '' }).tag).toBe(upper(m.whatsapp_draft_kind_video()));
    expect(recentBubble({ kind: 'sticker', text: '', file_name: '' }).tag).toBe(upper(m.whatsapp_draft_kind_sticker()));
  });

  it('quem falou: no grupo só o remetente; na conversa, o nome dela, senão o remetente, senão o número', () => {
    const contact = { chat_kind: 'contact' as const, chat_name: 'Carlos', chat_number: '5511987654321' };
    expect(recentSpeaker(contact, { sender: 'Ana' })).toBe('Carlos');
    expect(recentSpeaker({ ...contact, chat_name: '' }, { sender: 'Ana' })).toBe('Ana');
    expect(recentSpeaker({ ...contact, chat_name: '' }, { sender: '' })).toBe('5511987654321');
    const group = { chat_kind: 'group' as const, chat_name: 'Família', chat_number: '120363@g.us' };
    expect(recentSpeaker(group, { sender: 'Ana' })).toBe('Ana');
    expect(recentSpeaker(group, { sender: '' })).toBe('');
  });

  it('texto e other mostram só o texto', () => {
    expect(recentBubble({ kind: 'text', text: 'oi', file_name: '' })).toEqual({ tag: null, text: 'oi' });
    expect(recentBubble({ kind: 'other', text: 'enquete', file_name: '' })).toEqual({ tag: null, text: 'enquete' });
  });

  it('inicial: a primeira visível do nome, maiúscula; sem nome, o primeiro dígito do número', () => {
    expect(draftInitial({ chat_name: 'piterson', chat_number: '556496120521' })).toBe('P');
    expect(draftInitial({ chat_name: ' \u200Bana', chat_number: '55' })).toBe('A');
    expect(draftInitial({ chat_name: '', chat_number: '556496120521' })).toBe('5');
    expect(draftInitial({ chat_name: '', chat_number: '120363@g.us' })).toBe('1');
    expect(draftInitial({ chat_name: '', chat_number: '' })).toBe('');
  });

  it('a foto vira endereço de imagem JPEG', () => {
    expect(draftAvatarUri('/9j/AA==')).toBe('data:image/jpeg;base64,/9j/AA==');
  });
});

describe('texto editado', () => {
  it('\\r\\n e \\r viram \\n', () => {
    expect(normalizeDraftText('a\r\nb\rc\nd')).toBe('a\nb\nc\nd');
  });

  it('até 4096 pontos de código; vazio só com anexo', () => {
    expect(draftTextError(draftField('🚀'.repeat(4096)), false)).toBeUndefined();
    expect(draftTextError(draftField('🚀'.repeat(4097)), false)).toBe(m.whatsapp_draft_err_text_long());
    // A quebra \r\n conta como uma depois de normalizar.
    expect(draftField('a'.repeat(4095) + '\r\n')).toEqual({ text: 'a'.repeat(4095) + '\n', count: 4096 });
    expect(draftTextError(draftField('a'.repeat(4095) + '\r\n'), false)).toBeUndefined();
    expect(draftTextError(draftField(' \n '), false)).toBe(m.whatsapp_draft_err_text_empty());
    expect(draftTextError(draftField(''), true)).toBeUndefined();
    // Espaço é o do serviço (Go `strings.TrimSpace`): U+FEFF não é espaço, U+3000 e U+0085 são.
    expect(draftTextError(draftField('\uFEFF'), false)).toBeUndefined();
    expect(draftTextError(draftField('\u3000\u0085\u00A0\u202F'), false)).toBe(m.whatsapp_draft_err_text_empty());
  });

  it('envio editado vai na rota própria com o via e o texto normalizado, e devolve o 202', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify({ id: 'a b', sent_text: 'x\ny' }), { status: 202 }));
    await expect(sendEditedWhatsAppDraft(server, 'a b', 'x\r\ny', 'mobile')).resolves.toEqual({ id: 'a b', sent_text: 'x\ny' });
    const [[url, init]] = fetchMock.mock.calls;
    expect(url).toBe('https://a.test/api/whatsapp/outbox/a%20b/send-edited');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(String(init?.body))).toEqual({ via: 'mobile', text: 'x\ny' });
  });

  it('o 202 só confirma o editado com sent_text igual ao enviado', () => {
    expect(sentTextMatches({ sent_text: 'x\ny' }, 'x\r\ny')).toBe(true);
    // Texto igual ao do rascunho: o serviço devolve o cru, com \r\n.
    expect(sentTextMatches({ sent_text: 'x\r\ny' }, 'x\ny')).toBe(true);
    expect(sentTextMatches({ sent_text: 'original' }, 'editado')).toBe(false);
    expect(sentTextMatches({ id: 'd1' }, 'editado')).toBe(false);
    expect(sentTextMatches(null, 'editado')).toBe(false);
  });

  it('serviço sem a edição tem frase própria e não tira o cartão', async () => {
    expect(whatsappDraftRefusalText('whatsapp_edit_unsupported')).toBe(m.whatsapp_draft_err_edit_unsupported());
    expect(draftRefusalEndsCard('whatsapp_edit_unsupported')).toBe(false);
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(
      JSON.stringify({ detail: { code: 'whatsapp_edit_unsupported', params: {}, msg: 'api' } }), { status: 409 }));
    await expect(sendEditedWhatsAppDraft(server, 'd1', 'x', 'desktop')).rejects.toMatchObject({ status: 409, code: 'whatsapp_edit_unsupported' });
  });
});
