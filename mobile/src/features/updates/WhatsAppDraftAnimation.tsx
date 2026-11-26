import { useState } from 'react';
import { Text, View } from 'react-native';
import Animated, { FadeIn, useAnimatedStyle, useReducedMotion, withTiming } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { WhatsAppDraft } from '@hangar/core';
import { ComposerBox } from '../../chat/ComposerField';
import { RequestNotice } from '../requests/ownerRequests';
import { DraftCardView } from '../whatsapp/OutboxRequests';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `whatsapp-draft`: o aviso do rascunho aparece no chat da sessão, a folha abre com o cartão (foto e número,
// a conversa, o texto exato com o Editar e o anexo), desce até os botões, o Enviar é tocado e o cartão sai com o aviso do envio.
const WIDTH = 390;
const HEIGHT = 520;
const CYCLE = 10;
const NOTICE = 0.6;
const SHEET = 2;
const DOWN = 4.8;
const PRESS = 6.2;
const SENT = 7;
const STEPS = [NOTICE, SHEET, DOWN, PRESS, SENT];
// Movimento reduzido: a folha aberta no destino, na conversa e no texto.
const STILL = 3;
const FADE = { duration: 240 };
// Número de mentira: o DDD 00 não existe.
const NUMBER = '5500912345678';
// Fixo: o cartão mostra a mesma contagem em todo quadro (conta até o answer_by do exemplo).
const NOW = Date.parse('2026-11-24T14:00:18Z');
// Altura mínima da MachineRow: o toque cai no meio da penúltima linha, o Enviar.
const ROW = 56;
const RING = 40;
const noop = () => {};
// Pessoa desenhada (JPEG 96×96), não é foto de ninguém.
const AVATAR = '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';

/** O rascunho de exemplo no formato do evento `whatsapp_outbox`: o cartão lê dele como lê o de verdade. */
export function sampleDraft(): WhatsAppDraft {
  const file = m.whatsapp_draft_news_file();
  const contact = m.whatsapp_draft_news_contact();
  const said = (from_me: boolean, at: string, text: string, kind: 'text' | 'image' = 'text') =>
    ({ from_me, sender: from_me ? '' : contact, at, text, kind, file_name: '' });
  return {
    id: 'news', origin: `${m.native_vault_news_machine()}::site`, chat: `${NUMBER}@s.whatsapp.net`, chat_kind: 'contact',
    chat_number: NUMBER, chat_name: contact, chat_name_source: 'contact', chat_avatar: AVATAR,
    recent: [
      said(true, '2026-11-24T14:02:00Z', m.whatsapp_draft_news_recent_mine()),
      said(false, '2026-11-24T14:03:00Z', '', 'image'),
      said(false, '2026-11-24T14:03:00Z', m.whatsapp_draft_news_recent_theirs()),
    ],
    text: m.whatsapp_draft_news_text(),
    reply_to: '', quoted_text: '', file: { name: file, size: 2_516_582, sha256: '', source: `/home/ana/site/${file}`, outside_cwd: false },
    created_at: '2026-11-24T14:00:00Z', expires_at: '2026-11-24T14:10:00Z', answer_by: '2026-11-24T14:09:30Z', state: 'pending',
    edited: false, sent_text: '',
  };
}

export function WhatsAppDraftAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const [draft] = useState(sampleDraft);
  const [contentHeight, setContentHeight] = useState(0);
  const sheet = t >= SHEET && t < SENT;
  const pressed = t >= PRESS;
  const showNotice = t >= NOTICE && t < SHEET;
  const showResult = t >= SENT;
  // Desce o cartão até os botões, que ficam no fim dele.
  const down = t >= DOWN ? -Math.max(0, contentHeight - HEIGHT) : 0;
  const scroll = useAnimatedStyle(() => ({ transform: [{ translateY: reduced ? down : withTiming(down, { duration: 900 }) }] }), [down, reduced]);
  const notice = useAnimatedStyle(() => ({ opacity: reduced ? (showNotice ? 1 : 0) : withTiming(showNotice ? 1 : 0, FADE) }), [showNotice, reduced]);
  const result = useAnimatedStyle(() => ({ opacity: reduced ? (showResult ? 1 : 0) : withTiming(showResult ? 1 : 0, FADE) }), [showResult, reduced]);
  const scale = width / WIDTH;
  return <View style={[styles.stage, { width, height: HEIGHT * scale }]} pointerEvents="none"
    accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={[styles.surface, { width: WIDTH, height: HEIGHT, transform: [{ scale }] }]}>
      {sheet ? <Animated.View key="sheet" style={[styles.sheet, scroll]} entering={reduced ? undefined : FadeIn.duration(150)}
        onLayout={(e) => setContentHeight(e.nativeEvent.layout.height)}>
        <Text style={styles.title}>{m.whatsapp_draft_title()}</Text>
        <DraftCardView draft={draft} edit now={NOW} busy={pressed} error="" onAnswer={noop} />
        {pressed ? <View style={styles.ring} /> : null}
      </Animated.View> : <Animated.View key="chat" style={styles.chat} entering={reduced ? undefined : FadeIn.duration(150)}>
        <Text style={styles.agent}>{m.whatsapp_draft_news_agent()}</Text>
        {/* O aviso fica no layout mesmo invisível: a altura não muda de um quadro para outro. */}
        <View>
          <Animated.View style={notice}>
            <RequestNotice icon="MessageCircle" hint={m.whatsapp_draft_open()} title={m.whatsapp_draft_session_notice({ name: draft.chat_name })}
              onPress={noop} />
          </Animated.View>
          <Animated.View style={[styles.result, result]}>
            <Text style={styles.agent}>{m.whatsapp_draft_sending()}</Text>
          </Animated.View>
        </View>
        <ComposerBox focused={false}>
          <Text style={styles.placeholder} numberOfLines={1}>{m.composer_mensagem_para({ nome: 'Claude' })}</Text>
        </ComposerBox>
      </Animated.View>}
    </View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: { overflow: 'hidden', borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default },
  surface: { transformOrigin: 'top left', overflow: 'hidden', backgroundColor: superficie(theme, 0.95) },
  sheet: { padding: theme.base.space[4], gap: theme.base.space[3] },
  chat: { flex: 1, justifyContent: 'flex-end', padding: theme.base.space[4], gap: theme.base.space[3] },
  title: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary, textAlign: 'center' },
  agent: { fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
  result: { position: 'absolute', left: 0, right: 0, top: 0, bottom: 0, justifyContent: 'center' },
  placeholder: { padding: theme.base.space[3], fontSize: theme.base.text.sm, color: theme.tokens.text.muted },
  // Contorno do toque no Enviar: a linha fica a uma linha (o Recusar) e uma borda do fim do cartão.
  ring: {
    position: 'absolute', width: RING, height: RING, borderRadius: RING / 2, borderWidth: 2,
    borderColor: theme.tokens.text.primary, backgroundColor: superficie(theme, 0.3),
    left: theme.base.space[4] * 2 + 1, bottom: theme.base.space[4] + 1 + ROW + ROW / 2 - RING / 2,
  },
}));
