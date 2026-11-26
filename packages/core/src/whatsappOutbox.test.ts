import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  answerWhatsAppDraft, draftCharCount, draftRefusalEndsCard, draftSecondsLeft, invisibleMark, markInvisibles, parseWhatsAppOutboxEvent,
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
    expect(event?.drafts[0].recent).toEqual([{ from_me: false, sender: 'Carlos', at: '2026-11-24T14:02:00Z', text: 'oi' }]);
    expect(event?.drafts[0].answer_by).toBe('2026-11-24T14:09:30Z');
  });

  it('lista vazia é sem cartões; corpo torto é null (o anterior fica)', () => {
    expect(parseWhatsAppOutboxEvent('{"drafts":[]}')).toEqual({ drafts: [] });
    expect(parseWhatsAppOutboxEvent('{"drafts":{}}')).toBeNull();
    expect(parseWhatsAppOutboxEvent('não é json')).toBeNull();
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
