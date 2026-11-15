import { useEffect, useState } from 'react';
import { Text, View } from 'react-native';
import { Stack, useLocalSearchParams } from 'expo-router';
import { StyleSheet } from 'react-native-unistyles';
import { dreamCallLabel, dreamCallTitle, dreamErrorText, dreamFailure, dreamModelName, dreamSince, dreamSpan, dreamUsd, getDreamCall,
  type DreamCall, type DreamStep } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { SectionCard } from '../../src/features/config/SectionCard';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { Pulse } from '../../src/features/dream/rows';
import { useDream } from '../../src/features/dream/useDream';
import { useSettingsColors } from '../../src/features/config/colors';
import { Icon, type IconName } from '../../src/ui/Icon';
import { MONO } from '../../src/theme/scale';
import * as m from '../../src/paraglide/messages';

const TOOLS: Record<string, { icon: IconName; verb: () => string }> = {
  Read: { icon: 'FileText', verb: m.dream_call_read },
  Write: { icon: 'FilePlus', verb: m.dream_call_wrote },
  Edit: { icon: 'Pencil', verb: m.dream_call_edited },
  Glob: { icon: 'Search', verb: m.dream_call_searched },
  Grep: { icon: 'Search', verb: m.dream_call_searched },
};

function Step({ step }: { step: DreamStep }) {
  const c = useSettingsColors();
  if (step.kind === 'text') return <Text selectable style={[styles.say, { color: c.text }]}>{step.text}</Text>;
  const tool = TOOLS[step.tool];
  return <View style={styles.step}>
    <View style={styles.stepLine}>
      <Icon name={tool?.icon ?? 'Wrench'} size={14} color={c.muted} />
      <Text style={[styles.small, { color: c.muted }]}>{tool ? tool.verb() : step.tool}</Text>
      {step.target ? <Text selectable numberOfLines={1} style={[styles.chip, { color: c.text, backgroundColor: c.inset }]}>{step.target}</Text> : null}
    </View>
    {step.detail ? <Text selectable style={[styles.box, { color: c.text, backgroundColor: c.inset, borderColor: c.border }]}>{step.detail}</Text> : null}
  </View>;
}

export default function MemoriaCall() {
  // `title` vem da lista, que conhece as partes da sessão ("Conversa 3 · parte 2").
  const { run, project, n, title } = useLocalSearchParams<{ run?: string; project?: string; n?: string; title?: string }>();
  const d = useDream();
  const { dreamer, progress } = d;
  const c = useSettingsColors();
  const [call, setCall] = useState<DreamCall | null>(null);
  const [error, setError] = useState('');
  const [attempt, setAttempt] = useState(0);
  const [showPrompt, setShowPrompt] = useState(false);
  const running = call?.meta.status === 'running';
  // Chamada em andamento relê no máximo a cada 2 s de atividade da rodada; acabada, não muda mais.
  const tick = running && progress && progress.run_id === run ? Math.floor(Date.parse(progress.last_activity_at) / 2000) : null;
  useEffect(() => {
    if (!dreamer || !run || !project || n === undefined) return;
    let alive = true;
    getDreamCall(dreamer, run, project, Number(n))
      .then((r) => { if (alive) { setCall(r); setError(''); } })
      .catch((e: unknown) => { if (alive) setError(`${m.dream_load_error()} · ${dreamFailure(e)}`); });
    return () => { alive = false; };
  }, [dreamer, run, project, n, attempt, tick]);

  const meta = call?.meta;
  const failure = run && project && n !== undefined ? error || d.error : m.dream_load_error();
  const header = meta ? [
    dreamModelName(meta.model),
    meta.ended_at ? dreamSpan(Date.parse(meta.ended_at) - Date.parse(meta.started_at)) : m.dream_call_running({ elapsed: dreamSince(meta.started_at) }),
    meta.cost_usd === null ? (meta.ended_at ? m.dream_call_cost_unknown() : '') : m.dream_call_cost({ cost: dreamUsd(meta.cost_usd) }),
  ].filter(Boolean).join(' · ') : '';

  return <Pagina>
    <Stack.Screen options={{ title: title ?? (meta ? dreamCallTitle(dreamCallLabel(meta.label), d.labelOf) : m.dream_calls_title()) }} />
    {failure ? <><Notice text={failure} error /><MachineGroup>
      <MachineRow title={m.lista_tentar_novamente()} action onPress={() => { if (dreamer) setAttempt((x) => x + 1); else void d.reload(); }} />
    </MachineGroup></> : null}
    {!call && !failure ? <InfoNotice text={m.comum_carregando()} /> : null}
    {call && meta ? <>
      <MachineHelp>{header}</MachineHelp>
      {meta.status === 'failed' && meta.error ? <Notice text={dreamErrorText(meta.error)} error /> : null}
      <SectionCard>
        <View style={styles.body}>
          {call.steps.map((step, i) => <Step key={i} step={step} />)}
          {running ? <View style={styles.stepLine}><Pulse size={8} /><Text style={[styles.small, { color: c.muted }]}>
            {m.dream_call_running({ elapsed: dreamSince(meta.started_at) })}</Text></View> : null}
          {!call.steps.length && !running ? <Text style={[styles.small, { color: c.muted }]}>{m.dream_calls_empty()}</Text> : null}
        </View>
      </SectionCard>
      <MachineGroup>
        <MachineRow title={m.dream_call_prompt()} expanded={showPrompt} onPress={() => setShowPrompt((v) => !v)} />
        {showPrompt ? <Text selectable style={[styles.prompt, { color: c.text, borderTopColor: c.border }]}>{call.prompt}</Text> : null}
      </MachineGroup>
    </> : null}
  </Pagina>;
}

const styles = StyleSheet.create((theme) => ({
  body: { paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], gap: theme.base.space[2] },
  step: { gap: theme.base.space[1] },
  stepLine: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 24 },
  small: { fontSize: theme.base.text.xs },
  say: { fontSize: theme.base.text.sm, lineHeight: 21, marginVertical: theme.base.space[1] },
  chip: { fontFamily: MONO, fontSize: theme.base.text.xs, paddingHorizontal: theme.base.space[1], borderRadius: 4, overflow: 'hidden', flexShrink: 1 },
  box: { fontFamily: MONO, fontSize: theme.base.text.xs, lineHeight: 18, padding: theme.base.space[3], borderRadius: 8, borderWidth: 1 },
  prompt: { fontFamily: MONO, fontSize: theme.base.text.xs, lineHeight: 18, padding: theme.base.space[4], borderTopWidth: 1 },
}));
