import { useContext, useEffect, useRef, useState } from 'react';
import { AppState, Text, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import Animated, { FadeIn, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { CREATE_SESSION_PROVIDERS, providerName, type Provider } from '@hangar/core';
import { ComposerActions, ComposerBox } from '../../chat/ComposerField';
import { SessionSettingsControl } from '../../chat/SessionSettings';
import { SessionSettingsLink } from '../../chat/SessionSettingsFrame';
import { ProviderPicker } from '../create/ProviderPicker';
import { permissionLabel } from '../pills/permissionLabel';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import type { IconName } from '../../ui/Icon';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `antigravity`: o Antigravity é escolhido entre os agentes, a sessão abre com o modelo do agy
// na pílula, em Plano, e o modo passa a Automático.
const PICK = 1.4;
const SESSION = 2.8;
const AUTO = 4.4;
const CYCLE = 6;
const STEPS = [PICK, SESSION, AUTO];
// Movimento reduzido: o Antigravity já na sessão, com o modelo e o modo Automático.
const STILL = 5;
// Largura de telefone, reduzida ao cartão: o chip do composer tem o espaço que tem no chat e mostra o modelo inteiro.
const WIDTH = 420;
const MODEL = 'Gemini 4 Neon (Low)';
// Os mesmos ícones que o chip do composer usa para cada modo.
const MODE_ICON: Record<'plan' | 'auto', IconName> = { plan: 'Diamond', auto: 'Sparkles' };
const noop = () => {};
const none = async () => {};

export type AntigravityFrame = { provider: Provider; session: boolean; mode: 'plan' | 'auto' };

export function frameAt(t: number): AntigravityFrame {
  return {
    provider: t < PICK ? 'claude' : 'antigravity',
    session: t >= SESSION,
    mode: t >= AUTO ? 'auto' : 'plan',
  };
}

export function AntigravityAnimation({ width }: { width: number }) {
  const paused = useContext(WorkingMarkPaused);
  const reduced = useReducedMotion();
  const focused = useIsFocused();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const [inView, setInView] = useState(false);
  const [innerHeight, setInnerHeight] = useState(0);
  const [active, setActive] = useState(AppState.currentState === 'active');
  useEffect(() => { const sub = AppState.addEventListener('change', (state) => setActive(state === 'active')); return () => sub.remove(); }, []);
  useEffect(() => {
    if (!focused || !active || paused) return;
    const check = () => stage.current?.measureInWindow((_x, y, _w, height) => setInView(height > 0 && y < window.height && y + height > 0));
    check();
    const timer = setInterval(check, 500);
    return () => clearInterval(timer);
  }, [focused, active, paused, window.height]);
  const still = reduced || paused || !active || !focused || !inView;
  const t = useScriptStep(STEPS, CYCLE, STILL, still);
  const { provider, session, mode } = frameAt(t);
  const enter = still ? undefined : FadeIn.duration(200);
  const plan = session && mode === 'plan';
  const scale = width / WIDTH;
  return (
    <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants"
      style={[styles.stage, { width, height: innerHeight ? innerHeight * scale : undefined }]}>
      {/* A escala não muda a medida do layout: o cartão recebe a altura medida já reduzida. */}
      <View onLayout={(e) => setInnerHeight(e.nativeEvent.layout.height)} style={[styles.surface, { width: WIDTH, transform: [{ scale }] }]}>
        {/* O seletor continua montado e dá a altura; na sessão o modo ocupa o lugar dele e o cartão não pula. */}
        <View>
          <View style={session ? styles.hidden : undefined}>
            <ProviderPicker value={provider} onChange={noop} providers={CREATE_SESSION_PROVIDERS} />
          </View>
          {session ? (
            <Animated.View key={`mode-${mode}`} entering={enter} style={styles.over}>
              <SessionSettingsLink label={m.native_ctl_mode_title()} value={permissionLabel(mode)} onPress={noop} />
            </Animated.View>
          ) : null}
        </View>
        <ComposerBox focused={false} footer={(
          <ComposerActions attach={{ onPress: noop, disabled: false }}
            mic={{ onPress: noop, disabled: false, recording: false, styleLabel: '', onStyle: noop }}
            send={{ label: m.nova_conversa_enviar(), disabled: true, onPress: noop }}
            settings={(
              <Animated.View key={session ? `session-${mode}` : `create-${provider}`} entering={enter} style={styles.chip}>
                {/* Na criação o chip é só o logo do agente; na sessão, o modelo e o modo. Miniatura só leitura: sem painel. */}
                <SessionSettingsControl open={false} onOpen={noop} onClose={noop} panel={false}
                  glyph={session ? undefined : <ProviderGlyph provider={provider} size={16} inline />}
                  plan={plan} icon={session ? MODE_ICON[mode] : undefined} modeText={session ? permissionLabel(mode) : null}
                  model={{ display: MODEL, items: [], select: none }}
                  effort={{ hidden: true, items: [], select: none }}
                  fast={{ hidden: true, tier: 'default', available: false, choose: none }} />
              </Animated.View>
            )} />
        )}>
          <Text style={styles.placeholder} numberOfLines={1}>{m.composer_mensagem_para({ nome: providerName(provider) })}</Text>
        </ComposerBox>
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  stage: {
    borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  surface: { transformOrigin: 'top left', gap: theme.base.space[2], padding: theme.base.space[3] },
  hidden: { opacity: 0 },
  over: { position: 'absolute', top: 0, bottom: 0, left: 0, right: 0, justifyContent: 'center' },
  chip: { flexShrink: 1, minWidth: 0 },
  placeholder: { padding: theme.base.space[3], fontSize: theme.base.text.sm, color: theme.tokens.text.muted },
}));
