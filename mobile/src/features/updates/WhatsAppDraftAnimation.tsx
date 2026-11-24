import { useState } from 'react';
import { Text, View } from 'react-native';
import Animated, { FadeIn, useAnimatedStyle, useReducedMotion, withTiming } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { WhatsAppDraft } from '@hangar/core';
import { ComposerBox } from '../../chat/ComposerField';
import { RequestNotice } from '../vault/VaultRequests';
import { DraftCardView } from '../whatsapp/OutboxRequests';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `whatsapp-rascunho`: o aviso do rascunho aparece no chat da sessão, a folha abre com o cartão (número,
// texto exato e anexo), desce até os botões, o Enviar é tocado e o cartão sai com o aviso do envio.
const WIDTH = 390;
const HEIGHT = 520;
const CYCLE = 10;
const NOTICE = 0.6;
const SHEET = 2;
const DOWN = 4.8;
const PRESS = 6.2;
const SENT = 7;
const STEPS = [NOTICE, SHEET, DOWN, PRESS, SENT];
// Movimento reduzido: a folha aberta no destino, no texto e no anexo.
const STILL = 3;
const FADE = { duration: 240 };
// Número de mentira: o DDD 00 não existe.
const NUMBER = '5500912345678';
// Fixo: o cartão mostra "9:12" em todo quadro (conta até 570 s depois do created_at do exemplo).
const NOW = Date.parse('2026-11-24T14:00:18Z');
// Altura mínima da MachineRow: o toque cai no meio da penúltima linha, o Enviar.
const ROW = 56;
const RING = 40;
const noop = () => {};

/** O rascunho de exemplo no formato do evento `whatsapp_outbox`: o cartão lê dele como lê o de verdade. */
export function sampleDraft(): WhatsAppDraft {
  const file = m.whatsapp_draft_news_file();
  return {
    id: 'news', origin: `${m.native_vault_news_machine()}::site`, chat: `${NUMBER}@s.whatsapp.net`, chat_kind: 'contact',
    chat_number: NUMBER, chat_name: 'Marina', chat_name_source: 'contact', recent: [], text: m.whatsapp_draft_news_text(),
    reply_to: '', quoted_text: '', file: { name: file, size: 2_516_582, sha256: '', source: `/home/ana/site/${file}`, outside_cwd: false },
    created_at: '2026-11-24T14:00:00Z', expires_at: '2026-11-24T14:10:00Z', state: 'pending',
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
        <DraftCardView draft={draft} now={NOW} busy={pressed} error="" onAnswer={noop} />
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
