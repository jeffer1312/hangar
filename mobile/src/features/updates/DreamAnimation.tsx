import { useEffect, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router/react-navigation';
import Animated, { FadeIn, FadeOut, LinearTransition, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { dreamUsd, type DreamFileChange } from '@hangar/core';
import { SectionCard } from '../config/SectionCard';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { FileChange } from '../dream/FileChange';
import { TapRow } from '../dream/rows';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `sonho-da-memoria`: o projeto com conversas novas, Sonhar agora vira Sonhando… e a rodada
// termina com o último sonho e os dois arquivos que ela mexeu, como nas telas da Memória.
const CYCLE = 6;
const RUN = 1.5;
const DONE = 3.5;
const STEPS = [RUN, DONE];
// Movimento reduzido e fora da tela: o quadro final.
const STILL = 5;
const MACHINE = 'notebook';
const CHANGES: DreamFileChange[] = [{ file: 'deploy.md', kind: 'added' }, { file: 'MEMORY.md', kind: 'changed' }];
const noop = () => {};

export function DreamAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const [visible, setVisible] = useState(false);
  const focused = useIsFocused();
  const [active, setActive] = useState(AppState.currentState === 'active');
  useEffect(() => { const sub = AppState.addEventListener('change', (state) => setActive(state === 'active')); return () => sub.remove(); }, []);
  useEffect(() => {
    if (!focused || !active) return;
    const check = () => stage.current?.measureInWindow((_x, y, _w, height) => setVisible(height > 0 && y < window.height && y + height > 0));
    check();
    const timer = setInterval(check, 500);
    return () => clearInterval(timer);
  }, [focused, active, window.height]);
  const still = reduced || !active || !focused || !visible;
  const t = useScriptStep(STEPS, CYCLE, STILL, still);
  const running = t >= RUN && t < DONE;
  const done = t >= DONE;
  const last = done ? m.dream_last({ when: m.tempo_agora(), cost: dreamUsd(0.3) }) : m.dream_never();
  const fade = still ? undefined : FadeIn.duration(240);
  return <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
    <Animated.View layout={still ? undefined : LinearTransition.duration(260)} style={styles.column}>
      <MachineGroup>
        <MachineRow title={running ? m.dream_running() : m.dream_run_now()} action disabled={running} onPress={noop} />
      </MachineGroup>
      <SectionCard icon="Folder" title={m.dream_projects()}>
        <Animated.View key={done ? 'done' : 'waiting'} entering={fade} exiting={still ? undefined : FadeOut.duration(150)}>
          <TapRow title="hangar" description={`${MACHINE} · ${m.dream_new_sessions({ n: done ? 0 : 12 })}\n${last}`} onPress={noop} />
        </Animated.View>
      </SectionCard>
      {done ? <Animated.View entering={fade}>
        <SectionCard icon="FileText" title={m.dream_changes()}>
          {CHANGES.map((change) => <FileChange key={change.file} change={change} before="" after="" />)}
        </SectionCard>
      </Animated.View> : null}
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  // Altura do quadro final: o cartão não pula entre os passos.
  stage: { minHeight: 330, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
  column: { gap: theme.base.space[2] },
}));
