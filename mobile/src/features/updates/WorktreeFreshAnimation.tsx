import { useMemo } from 'react';
import { View } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { FolderBranches } from '@hangar/core';
import { BaseField } from '../create/BranchPicker';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `worktree-atualizada`: a branch está atrás, "Parte de" passa para a remota com o aviso, e a lista abre com as duas.
const CYCLE = 7;
const FETCHED = 1.6;
const LIST = 3.8;
const STEPS = [FETCHED, LIST];
// Movimento reduzido: a remota já marcada, com o aviso.
const STILL = FETCHED;

const noop = () => {};

export function WorktreeFreshAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const main = m.news_branch_sample_main();
  const upstream = m.news_branch_sample_upstream();
  const info = useMemo<FolderBranches>(() => ({ current: main, branches: [main], remotes: [], dirty: false,
    upstreams: { [main]: { upstream, ahead: 1, behind: 112 } } }), [main, upstream]);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <BaseField info={info} base={t >= FETCHED ? upstream : main} open={t >= LIST} disabled={false} onToggle={noop} onPick={noop} />
  </View>;
}

const styles = StyleSheet.create(theme => ({
  // Altura do quadro mais alto (a lista): o cartão não pula entre os passos.
  stage: {
    minHeight: 190, gap: theme.base.space[2], padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
}));
