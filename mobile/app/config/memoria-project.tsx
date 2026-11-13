import { RefreshControl } from 'react-native';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { dreamErrorText, dreamLastText, putDreamProject, startDream, type DreamProjectPatch } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { PickerRow, SwitchRow, TapRow } from '../../src/features/dream/rows';
import { useDream } from '../../src/features/dream/useDream';
import * as m from '../../src/paraglide/messages';

export default function MemoriaProject() {
  const router = useRouter();
  const { id } = useLocalSearchParams<{ id: string }>();
  const d = useDream();
  const { status, dreamer, busy } = d;
  const project = status?.projects.find((p) => p.id === id);
  const patch = (body: DreamProjectPatch) => { if (dreamer && project) void d.act(() => putDreamProject(dreamer, project.id, body)); };
  const modes = [{ id: 'key', title: m.dream_mode_key() }, { id: 'account', title: m.dream_mode_account() }] as const;

  return <Pagina refreshControl={<RefreshControl refreshing={d.loading && !!status} onRefresh={() => void d.reload()} />}>
    <Stack.Screen options={{ title: project?.name ?? m.dream_projects() }} />
    {d.error ? <><Notice text={d.error} error /><MachineGroup>
      <MachineRow title={m.lista_tentar_novamente()} action disabled={d.loading} onPress={() => void d.reload()} />
    </MachineGroup></> : null}
    {!status && d.loading ? <InfoNotice text={m.comum_carregando()} /> : null}
    {status && !project ? <InfoNotice text={m.dream_load_error()} /> : null}
    {project ? <>
      <MachineHelp>{`${project.machines.map(d.labelOf).join(', ')} · ${m.dream_new_sessions({ n: project.new_sessions })}\n${dreamLastText(project.last_dream_at, project.last_cost_usd)}`}</MachineHelp>
      {project.last_error ? <Notice text={dreamErrorText(project.last_error)} error /> : null}
      <MachineGroup>
        <SwitchRow title={m.dream_project_enabled()} value={project.enabled} disabled={busy} onChange={(enabled) => patch({ enabled })} />
        <PickerRow title={m.dream_mode()} value={project.mode === 'key' ? m.dream_mode_key() : m.dream_mode_account()} disabled={busy}
          options={modes.map((o) => ({ ...o, selected: o.id === project.mode }))} onPick={(mode) => patch({ mode: mode as 'key' | 'account' })} />
        {project.needs_confirm ? <PickerRow title={m.dream_confirm_pair()} description={m.dream_confirm_pair_hint()} value="—" disabled={busy}
          options={(status?.projects ?? []).filter((p) => p.id !== project.id).map((p) => ({ id: p.id, title: `${p.name} · ${p.machines.map(d.labelOf).join(', ')}` }))}
          onPick={(alias_of) => patch({ alias_of })} /> : null}
      </MachineGroup>
      <MachineGroup>
        <MachineRow title={status?.running ? m.dream_running() : m.dream_run_now()} action disabled={busy || !!status?.running || !dreamer}
          onPress={() => { if (dreamer) void d.act(() => startDream(dreamer, project.id)); }} />
        {project.last_run_id ? <TapRow title={m.dream_changes()}
          onPress={() => router.push({ pathname: '/config/memoria-run', params: { run: project.last_run_id } } as never)} /> : null}
      </MachineGroup>
    </> : null}
  </Pagina>;
}
