import { useEffect, useMemo, useState } from 'react';
import { View } from 'react-native';
import Animated, { FadeIn, FadeOut, LinearTransition, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { formatElapsed, type ChatEvent } from '@hangar/core';
import { AgentCard } from '../../chat/LiveWork';
import { mkMarkdownStyle } from '../../chat/AssistantBubble';
import { ToolCard } from '../../chat/tools/ToolCard';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';

// Roteiro do `codex-atividade` nativo: o Codex abre dois subagentes; rodando, cada um é o cartão do fim da
// conversa; ao terminar, o cartão sai e fica a linha da chamada, como no chat de verdade.
const CYCLE = 8.5;
// Movimento reduzido: os dois cartões rodando.
const STILL = 3.5;
const TEXT_AT = 0.2;

type Agent = { id: string; name: string; task: () => string; starts: number; ends: number };
const AGENTS: Agent[] = [
  { id: 'a', name: 'explorer', task: () => m.news_codex_agent_explore(), starts: 0.8, ends: 4.8 },
  { id: 'b', name: 'worker', task: () => m.news_codex_agent_task(), starts: 1.6, ends: 6.6 },
];

/** Próximo instante em que o quadro muda: degrau do roteiro ou o segundo seguinte de quem roda. */
function nextStep(t: number): number {
  const ticks = AGENTS.filter((a) => t >= a.starts && t < a.ends).map((a) => a.starts + Math.floor(t - a.starts) + 1);
  return Math.min(CYCLE, ...[TEXT_AT, ...AGENTS.flatMap((a) => [a.starts, a.ends]), ...ticks].filter((s) => s > t));
}

/** Instante do roteiro em degraus: só re-renderiza quando o quadro muda, a transição corre na thread de UI. */
function useStep(reduced: boolean): number {
  const [t, setT] = useState(reduced ? STILL : 0);
  useEffect(() => {
    if (reduced) { setT(STILL); return; }
    const started = Date.now();
    let timer: ReturnType<typeof setTimeout>;
    const tick = () => {
      const now = ((Date.now() - started) / 1000) % CYCLE;
      setT(now);
      timer = setTimeout(tick, (nextStep(now) - now) * 1000 + 5);
    };
    tick();
    return () => clearTimeout(timer);
  }, [reduced]);
  return t;
}

const call = (agent: Agent): ChatEvent => ({ kind: 'tool_use', id: `news-${agent.id}`, tool_name: 'Agent', tool_use_id: `news-${agent.id}`,
  tool_input: { description: agent.task(), subagent_type: agent.name } });
const done = (agent: Agent): ChatEvent => ({ kind: 'tool_result', id: `news-${agent.id}-r`, tool_use_id: `news-${agent.id}`, is_error: false, result: '' });
const noop = () => {};

export function CodexActivityAnimation({ width }: { width: number }) {
  const { theme } = useUnistyles();
  const reduced = useReducedMotion();
  const t = useStep(reduced);
  const md = useMemo(() => mkMarkdownStyle(theme), [theme]);
  const enter = reduced ? undefined : FadeIn.duration(220);
  const exit = reduced ? undefined : FadeOut.duration(220);
  const layout = reduced ? undefined : LinearTransition.duration(220);
  const finished = AGENTS.filter((a) => t >= a.ends);
  const running = AGENTS.filter((a) => t >= a.starts && t < a.ends);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {t >= TEXT_AT ? <Animated.View entering={enter} layout={layout}>
      <EnrichedMarkdownText markdown={m.news_codex_split()} markdownStyle={md} flavor="github" />
    </Animated.View> : null}
    {finished.map((agent) => <Animated.View key={`line-${agent.id}`} entering={enter} layout={layout}>
      <ToolCard use={call(agent)} result={done(agent)} onPress={noop} />
    </Animated.View>)}
    {running.map((agent) => <Animated.View key={`card-${agent.id}`} entering={enter} exiting={exit} layout={layout}>
      <AgentCard description={agent.task()} tag={agent.name} tempo={m.tool_executando_ha({ tempo: formatElapsed(Math.floor(t - agent.starts)) })} />
    </Animated.View>)}
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: {
    gap: theme.base.space[2], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
}));
