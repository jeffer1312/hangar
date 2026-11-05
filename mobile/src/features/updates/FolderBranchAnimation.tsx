import { useMemo } from 'react';
import { View } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { FolderBranches, FolderGit, Server } from '@hangar/core';
import { FolderBranchMenu } from '../create/FolderBranchMenu';
import { FolderGitCard } from '../create/FolderGitCard';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `branch-no-celular`: a branch está 2 commits atrás, o Pull a atualiza, e a lista troca para outra branch.
const CYCLE = 7.5;
const PULLING = 1.4;
const PULLED = 2.4;
const LIST = 4;
const SWITCHED = 5.4;
const STEPS = [PULLING, PULLED, LIST, SWITCHED];
// Movimento reduzido: o cartão com os commits a baixar.
const STILL = 0;

const noop = () => {};
const NO_SERVER = {} as Server;

export function FolderBranchAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const main = m.news_branch_sample_main();
  const work = m.news_branch_sample_work();
  const info = useMemo<FolderBranches>(() => ({ current: main, branches: [main, work], remotes: [], dirty: false }), [main, work]);
  const git: FolderGit = { repo: true, current: t >= SWITCHED ? work : main, upstream: m.news_branch_sample_upstream(), behind: t >= PULLED ? 0 : 2, ahead: 0, dirty: 0, last_fetch: Date.now() / 1000 };
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {t >= LIST
      ? <FolderBranchMenu server={NO_SERVER} cwd="" info={info} git={git} onBack={noop} onDone={noop} onRefused={noop} onOpenFolder={noop} />
      : <FolderGitCard git={git} busy={t >= PULLING && t < PULLED ? 'pull' : null}
          note={t >= PULLED ? { ok: true, text: m.native_folder_git_pull_done({ upstream: m.news_branch_sample_upstream() }) } : null} onAction={noop} />}
  </View>;
}

const styles = StyleSheet.create(theme => ({
  // Altura do quadro mais alto (a lista): o cartão não pula entre os passos.
  stage: {
    minHeight: 300, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
}));
