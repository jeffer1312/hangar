import { useCallback, useEffect, useState } from 'react';
import { Alert } from 'react-native';
import { Stack, useLocalSearchParams } from 'expo-router';
import { dreamErrorText, dreamFailure, dreamFileChanges, dreamLastText, getDreamRun, undoDreamRun, type DreamRun } from '@hangar/core';
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
  const { run: runId } = useLocalSearchParams<{ run: string }>();
  const d = useDream();
  const { dreamer, status } = d;
  const [run, setRun] = useState<DreamRun | null>(null);
  const [error, setError] = useState('');
  const load = useCallback(() => {
    if (!dreamer || !runId) return () => {};
    let live = true;
    setError('');
    getDreamRun(dreamer, runId).then((r) => { if (live) setRun(r); }).catch((e: unknown) => { if (live) setError(`${m.dream_load_error()} · ${dreamFailure(e)}`); });
    return () => { live = false; };
  }, [dreamer, runId]);
  useEffect(load, [load]);
  // Desfazer volta a rodada inteira, por isso a tela mostra todos os projetos dela.
  const undo = () => {
    if (!dreamer || !runId) return;
    Alert.alert(m.dream_undo(), undefined, [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.dream_undo(), style: 'destructive', onPress: () => void d.act(async () => {
        const { results } = await undoDreamRun(dreamer, runId);
        const failed = results.filter((r) => !r.ok).map((r) => m.dream_undo_failed({ machine: d.labelOf(r.machine) }));
        return failed.length ? failed.join('\n') : undefined;
      }, m.dream_undo_done()) },
    ]);
  };
  const failure = error || d.error;

  return <Pagina>
    <Stack.Screen options={{ title: m.dream_changes() }} />
    {failure ? <><Notice text={failure} error /><MachineGroup>
      <MachineRow title={m.lista_tentar_novamente()} onPress={() => { if (dreamer) load(); else void d.reload(); }} action />
    </MachineGroup></> : null}
    {!run && !failure ? <InfoNotice text={m.comum_carregando()} /> : null}
    {run ? <>
      {run.projects.map((p) => <SectionCard key={p.project} icon="Folder" title={status?.projects.find((s) => s.id === p.project)?.name ?? p.project}
        subtitle={dreamLastText(p.at, p.cost_usd)}>
        {p.places.map((place) => {
          const changes = dreamFileChanges(place.before, place.after);
          return [
            <PlainRow key={`${place.machine}:${place.root}`} title={d.labelOf(place.machine)} error={place.error ? dreamErrorText(place.error) : undefined} />,
            ...changes.map((change) => <FileChange key={`${place.machine}:${place.root}:${change.file}`} change={change}
              before={place.before[change.file] ?? ''} after={place.after[change.file] ?? ''} />),
          ];
        })}
      </SectionCard>)}
      <MachineGroup>
        <MachineRow title={m.dream_undo()} danger disabled={d.busy || !dreamer} onPress={undo} />
      </MachineGroup>
    </> : null}
  </Pagina>;
}
