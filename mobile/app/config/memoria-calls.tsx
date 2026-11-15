import { useEffect, useMemo, useState } from 'react';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { useUnistyles } from 'react-native-unistyles';
import { dreamCallLabel, dreamCallParts, dreamCallTitle, dreamErrorText, dreamFailure, dreamRunWhen, dreamSince, dreamSpan, dreamUsd, getDreamCalls,
  type DreamCallMeta } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { PlainRow, Pulse, TapRow } from '../../src/features/dream/rows';
import { useDream } from '../../src/features/dream/useDream';
import { useSettingsColors } from '../../src/features/config/colors';
import { Icon } from '../../src/ui/Icon';
import * as m from '../../src/paraglide/messages';

export default function MemoriaCalls() {
  const router = useRouter();
  const { run, project } = useLocalSearchParams<{ run?: string; project?: string }>();
  const d = useDream();
  const { dreamer, progress } = d;
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [calls, setCalls] = useState<DreamCallMeta[] | null>(null);
  const [error, setError] = useState('');
  const [attempt, setAttempt] = useState(0);
  const live = !!run && progress?.run_id === run;
  const row = live ? progress?.projects.find((p) => p.id === project) ?? null : null;
  // A lista só muda quando uma chamada começa ou acaba (o status do projeto e o `calls_done` contam isso).
  const tick = row ? `${row.status}:${row.calls_done}` : null;
  useEffect(() => {
    if (!dreamer || !run || !project) return;
    let alive = true;
    getDreamCalls(dreamer, run, project)
      .then((r) => { if (alive) { setCalls(r.calls); setError(''); } })
      .catch((e: unknown) => { if (alive) setError(`${m.dream_load_error()} · ${dreamFailure(e)}`); });
    return () => { alive = false; };
  }, [dreamer, run, project, attempt, tick]);

  const parts = useMemo(() => dreamCallParts((calls ?? []).map((x) => x.label)), [calls]);
  const cost = (calls ?? []).reduce((sum, x) => sum + (x.cost_usd ?? 0), 0);
  const waiting = !!row && ['reading', 'analyzing'].includes(row.status) && !(calls ?? []).some((x) => x.kind === 'organizer');
  const failure = run && project ? error || d.error : m.dream_load_error();
  const icon = (call: DreamCallMeta) => call.status === 'running' ? <Pulse />
    : <Icon name={call.status === 'done' ? 'Check' : 'X'} size={16} color={call.status === 'done' ? theme.tokens.status.success : theme.tokens.status.error} />;
  const sub = (call: DreamCallMeta) => call.status === 'running' ? m.dream_call_running({ elapsed: dreamSince(call.started_at) })
    : call.status === 'failed' && call.error ? dreamErrorText(call.error)
    : call.cost_usd === null ? m.dream_call_cost_unknown()
    : m.dream_call_meta({ steps: call.steps, cost: dreamUsd(call.cost_usd),
      duration: dreamSpan(Date.parse(call.ended_at ?? call.started_at) - Date.parse(call.started_at)) });
  // A que está trabalhando primeiro, depois da mais nova para a mais velha: é o que se quer ver ao abrir.
  const ordered = [...(calls ?? [])].sort((a, b) => Number(b.status === 'running') - Number(a.status === 'running') || b.n - a.n);

  return <Pagina>
    <Stack.Screen options={{ title: m.dream_calls_title() }} />
    {failure ? <><Notice text={failure} error />{run && project ? <MachineGroup>
      <MachineRow title={m.lista_tentar_novamente()} action onPress={() => { if (dreamer) setAttempt((n) => n + 1); else void d.reload(); }} />
    </MachineGroup> : null}</> : null}
    {!calls && !failure ? <InfoNotice text={m.comum_carregando()} /> : null}
    {calls ? <>
      {run ? <MachineHelp>{m.dream_calls_header({ when: dreamRunWhen(run), cost: dreamUsd(cost) })}</MachineHelp> : null}
      {calls.length || waiting ? <MachineGroup>
        {ordered.map((call) => {
          const title = dreamCallTitle(dreamCallLabel(call.label, parts), d.labelOf);
          // O título vai junto: só a lista sabe quantas partes a sessão tem.
          return <TapRow key={call.n} leading={icon(call)} title={title} description={sub(call)}
            onPress={() => router.push({ pathname: '/config/memoria-call', params: { run, project, n: String(call.n), title } } as never)} />;
        })}
        {waiting ? <PlainRow leading={<Icon name="Circle" size={14} color={c.faint} />} title={m.dream_call_organizer()} description={m.dream_call_waiting()} /> : null}
      </MachineGroup> : <InfoNotice text={m.dream_calls_empty()} />}
    </> : null}
  </Pagina>;
}
