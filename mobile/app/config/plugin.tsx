import { useEffect, useState } from 'react';
import { Alert } from 'react-native';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { getPluginDetailsForServer, intlLocale, type PluginDetails, type PluginInventory } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { ReopenBanner, shortDate, useLiveClaudeCount } from '../../src/features/plugins/parts';
import { pluginsErrorText, runPluginAction, usePlugins } from '../../src/features/plugins/store';
import { useServers } from '../../src/stores/servers';
import * as m from '../../src/paraglide/messages';

const fmt = (n: number) => n.toLocaleString(intlLocale());

function inventoryText(inv: PluginInventory): string {
  const parts: string[] = [];
  const add = (n: number | null, one: () => string, many: (p: { n: string }) => string) => {
    if (n) parts.push(n === 1 ? one() : many({ n: fmt(n) }));
  };
  add(inv.skills, m.plugins_inventory_skills_one, m.plugins_inventory_skills);
  add(inv.agents, m.plugins_inventory_agents_one, m.plugins_inventory_agents);
  add(inv.hooks, m.plugins_inventory_hooks_one, m.plugins_inventory_hooks);
  if (inv.mcp) parts.push(m.plugins_inventory_mcp({ n: fmt(inv.mcp) }));
  if (inv.lsp) parts.push(m.plugins_inventory_lsp({ n: fmt(inv.lsp) }));
  return parts.join(' · ');
}

export default function Plugin() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const router = useRouter();
  const server = useServers((s) => s.active());
  const liveCount = useLiveClaudeCount(server);
  const plugin = usePlugins((s) => s.state?.plugins.find((p) => p.id === id));
  const hasState = usePlugins((s) => !!s.state);
  const busy = usePlugins((s) => s.busy);
  const [details, setDetails] = useState<PluginDetails | null>(null);
  const [detailsError, setDetailsError] = useState('');
  // Remoção que deu certo tira o plugin do estado antes de a tela sair: sem isso, pisca "não encontrado".
  const [removing, setRemoving] = useState(false);

  // Aberto direto (sem passar pela lista) ou com o servidor trocado: lê o estado aqui. `reset` não mexe no mesmo servidor.
  useEffect(() => {
    if (!server) return;
    usePlugins.getState().reset(server);
    if (!usePlugins.getState().state) void usePlugins.getState().load(server);
  }, [server?.id, server?.baseUrl, server?.token]);

  // Versão nova relê o conteúdo e troca no lugar, sem piscar; plugin removido não se busca.
  const installedVersion = plugin ? plugin.version : null;
  useEffect(() => {
    if (!server || !id) return;
    if (installedVersion === null) { setDetails(null); return; }
    let alive = true;
    setDetailsError('');
    getPluginDetailsForServer(server, id)
      .then((d) => { if (alive) setDetails(d); })
      .catch((e: unknown) => { if (alive) setDetailsError(pluginsErrorText(e)); });
    return () => { alive = false; };
  }, [server?.id, server?.baseUrl, server?.token, id, installedVersion]);

  if (!server) return null;
  if (!plugin) {
    return <Pagina>
      <Stack.Screen options={{ title: m.plugins_title() }} />
      {removing ? null : hasState ? <Notice text={m.plugins_not_found()} error /> : <InfoNotice text={m.plugins_loading()} />}
    </Pagina>;
  }

  const p = plugin;
  const updating = busy === `${p.id}:update`;
  const remove = () => Alert.alert(m.plugins_remove_confirm({ name: p.name }), undefined, [
    { text: m.comum_cancelar(), style: 'cancel' },
    { text: m.plugins_remove(), style: 'destructive', onPress: () => {
      setRemoving(true);
      void runPluginAction(server, p, 'uninstall', liveCount).then((ok) => { if (ok) router.back(); else setRemoving(false); });
    } },
  ]);
  const version = p.available_version ? `${p.version} → ${m.plugins_available({ version: p.available_version })}`
    : p.update_known ? `${p.version} (${m.plugins_detail_latest()})` : p.version;
  const updateLabel = updating ? m.plugins_updating()
    : p.available_version ? m.plugins_update_to({ version: p.available_version }) : m.plugins_update();
  return (
    <Pagina>
      <Stack.Screen options={{ title: p.name }} />
      {p.description ? <MachineHelp>{p.description}</MachineHelp> : null}
      <ReopenBanner server={server} />
      <MachineGroup>
        <MachineRow title={m.plugins_detail_version()} value={version} />
        <MachineRow title={m.plugins_detail_updated()} value={shortDate(p.last_updated)} />
        <MachineRow title={m.plugins_detail_marketplace()} value={p.marketplace} />
        {details ? <>
          <MachineRow title={m.plugins_detail_content()} value={inventoryText(details.inventory) || undefined} />
          {details.always_on_tokens !== null ? <MachineRow title={m.plugins_detail_always_on()}
            value={m.plugins_detail_tokens({ n: fmt(details.always_on_tokens) })} /> : null}
        </> : null}
      </MachineGroup>
      <Notice text={detailsError} error />
      {p.notes.map((note) => <InfoNotice key={note} text={note} />)}
      {!p.update_known ? <InfoNotice text={m.plugins_external_note()} /> : null}
      <MachineGroup>
        {p.available_version || !p.update_known ? <MachineRow title={updateLabel} action disabled={!!busy}
          onPress={() => void runPluginAction(server, p, 'update', liveCount)} /> : null}
        <MachineRow title={p.enabled ? m.plugins_disable() : m.plugins_enable()} action disabled={!!busy}
          onPress={() => void runPluginAction(server, p, p.enabled ? 'disable' : 'enable', liveCount)} />
        <MachineRow title={m.plugins_remove()} danger disabled={!!busy} onPress={remove} />
      </MachineGroup>
    </Pagina>
  );
}
