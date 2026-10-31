import { ActionSheetIOS, Alert, Platform } from 'react-native';
import { groupShowTargets, showSession } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { toast } from '../../ui/Toast';

const errorText = (e: unknown): string => (e instanceof Error ? e.message : String(e));

/** Mensagem minha numa oculta a traz de volta; com outras ocultas no grupo dela, pergunta se voltam junto.
 *  O grupo não muda: só a marca de oculta sai, cada uma no próprio servidor. */
export function revealOnSend(serverId: string, name: string): void {
  const rows = useSessions.getState().rows;
  const row = rows.find((r) => r.serverId === serverId && r.name === name);
  if (!row?.hidden_from_list) return;
  const ownServer = (id: string) => useServers.getState().servers.find((x) => x.id === id && !x.invite);
  const server = ownServer(serverId);
  if (!server) return;
  showSession(server, name).catch((e: unknown) => toast.erro(m.session_show_failed({ error: errorText(e) })));
  const others = groupShowTargets(row, rows);
  if (others.length === 0) return;
  const showOthers = async () => {
    const results = await Promise.allSettled(others.map((t) => {
      const own = ownServer(t.serverId);
      return own ? showSession(own, t.name) : Promise.reject(new Error(m.servidor_nao_existe()));
    }));
    const failed = results.flatMap((r, i) => (r.status === 'rejected' ? [{ name: others[i].name, error: errorText(r.reason) }] : []));
    if (failed.length === 0) return;
    const reasons = [...new Set(failed.map((f) => f.error))].join('; ');
    toast.erro(`${m.session_show_group_skipped({ names: failed.map((f) => f.name).join(', ') })}\n${reasons}`);
  };
  const body = `${m.session_show_group_body()}\n${others.map((o) => o.name).join('\n')}`;
  const onlyThis = m.session_show_only_this();
  const all = m.session_show_others({ n: others.length });
  if (Platform.OS === 'ios') {
    ActionSheetIOS.showActionSheetWithOptions(
      { title: m.session_show_group_title(), message: body, options: [onlyThis, all], cancelButtonIndex: 0 },
      (i) => { if (i === 1) void showOthers(); },
    );
  } else {
    Alert.alert(m.session_show_group_title(), body, [
      { text: onlyThis, style: 'cancel' },
      { text: all, onPress: () => void showOthers() },
    ]);
  }
}
