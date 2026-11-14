import { useEffect, useState } from 'react';
import { Alert } from 'react-native';
import { Stack, useLocalSearchParams } from 'expo-router';
import { dreamErrorText, dreamFailure, dreamFileChanges, dreamLastText, dreamUndoFailures, getDreamRun, undoDreamRun, type DreamRun } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { SectionCard } from '../../src/features/config/SectionCard';
import { MachineGroup, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { FileChange } from '../../src/features/dream/FileChange';
import { PlainRow } from '../../src/features/dream/rows';
import { useDream } from '../../src/features/dream/useDream';
import * as m from '../../src/paraglide/messages';

export default function MemoriaRun() {
  const { run: runId, project } = useLocalSearchParams<{ run?: string; project?: string }>();
  const d = useDream();
  const { dreamer, status } = d;
  const [run, setRun] = useState<DreamRun | null>(null);
  const [error, setError] = useState('');
  // Cada tentativa relê a rodada; o efeito segue com a limpeza dele.
  const [attempt, setAttempt] = useState(0);
  // O servidor já respondeu ao Desfazer: um segundo toque desfaria o que acabou de voltar.
  const [undone, setUndone] = useState(false);
  useEffect(() => {
    if (!dreamer || !runId) return;
    let live = true;
    setError('');
    getDreamRun(dreamer, runId).then((r) => { if (live) setRun(r); }).catch((e: unknown) => { if (live) setError(`${m.dream_load_error()} · ${dreamFailure(e)}`); });
    return () => { live = false; };
  }, [dreamer, runId, attempt]);
  // Aberta de um projeto, a tela e o Desfazer ficam só nele; a rodada da noite cobre vários.
  const undo = () => {
    if (!dreamer || !runId || undone) return;
    Alert.alert(m.dream_undo(), undefined, [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.dream_undo(), style: 'destructive', onPress: () => void d.act(async () => {
        const { results } = await undoDreamRun(dreamer, runId, project);
        setUndone(true);
        const failed = dreamUndoFailures(results, d.labelOf);
        return failed.length ? failed.join('\n') : undefined;
      }, m.dream_undo_done()).then(() => setAttempt((n) => n + 1)) },
    ]);
  };
  const failure = runId ? error || d.error : m.dream_load_error();
  const projects = run?.projects.filter((p) => !project || p.project === project) ?? [];

  return <Pagina>
    <Stack.Screen options={{ title: m.dream_changes() }} />
    {failure ? <><Notice text={failure} error />{runId ? <MachineGroup>
      <MachineRow title={m.lista_tentar_novamente()} onPress={() => { if (dreamer) setAttempt((n) => n + 1); else void d.reload(); }} action />
    </MachineGroup> : null}</> : null}
    {!run && !failure ? <InfoNotice text={m.comum_carregando()} /> : null}
    {run ? <>
      {projects.length ? null : <InfoNotice text={m.native_side_changes_none()} />}
      {projects.map((p) => <SectionCard key={p.project} icon="Folder" title={status?.projects.find((s) => s.id === p.project)?.name ?? p.project}
        subtitle={dreamLastText(p.at, p.cost_usd)}>
        {p.places.map((place) => {
          const changes = dreamFileChanges(place.before, place.after);
          return [
            <PlainRow key={`${place.machine}:${place.root}`} title={d.labelOf(place.machine)} error={place.error ? dreamErrorText(place.error) : undefined}
              description={changes.length || place.error ? undefined : m.native_side_changes_none()} />,
            ...changes.map((change) => <FileChange key={`${place.machine}:${place.root}:${change.file}`} change={change}
              before={place.before[change.file] ?? ''} after={place.after[change.file] ?? ''} />),
          ];
        })}
      </SectionCard>)}
      {undone ? null : <MachineGroup>
        <MachineRow title={m.dream_undo()} danger disabled={d.busy || !dreamer} onPress={undo} />
      </MachineGroup>}
    </> : null}
  </Pagina>;
}
