// Painel "Na fila", faixa de cima da caixa do composer: o que espera a vez no Hangar. O modo de envio mora nas
// Configurações > Geral.
// Toque longo na linha abre o menu (Subir, Descer, Orientar, Editar, Apagar), como o T3 mobile; tocar no
// texto mostra a mensagem inteira. Fila vazia não ocupa espaço nenhum.
import { useEffect, useState } from 'react';
import { AccessibilityInfo, ActionSheetIOS, ActivityIndicator, Platform, Pressable, ScrollView, Text, View } from 'react-native';
import { MenuView, type NativeActionEvent } from '@react-native-menu/menu';
import * as Haptics from 'expo-haptics';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { QueuePanel as Panel } from '@hangar/core';
import { contextRefsPlainText, formataErro, recarregarSessao } from '@hangar/core';
import * as m from '../paraglide/messages';
import { useServers } from '../stores/servers';
import { Icon } from '../ui/Icon';
import { haptic } from '../ui/haptics';
import { toast } from '../ui/Toast';
import { QUEUE_APPEAR_MS, type PendingMsg } from './pending';
import { FlightDestination } from './SendFlight';

type MenuId = 'up' | 'down' | 'steer' | 'edit' | 'remove';
interface MenuAction { id: MenuId; title: string; disabled: boolean; destructive?: boolean }

// A linha em edição já está no composer: reabrir, orientar ou apagar por ela jogaria fora o que foi digitado.
function menuActions({ index, count, busy, editing = false }: {
  index: number; count: number; busy: boolean; editing?: boolean;
}): MenuAction[] {
  return [
    { id: 'up', title: m.queue_panel_move_up(), disabled: busy || index === 0 },
    { id: 'down', title: m.queue_panel_move_down(), disabled: busy || index === count - 1 },
    { id: 'steer', title: m.codex_orientar(), disabled: busy || editing },
    { id: 'edit', title: m.atalhos_editar(), disabled: busy || editing },
    { id: 'remove', title: m.comum_apagar(), disabled: busy || editing, destructive: true },
  ];
}

// O iOS já vibra ao abrir o menu de contexto; o PopupMenu do Android não.
const vibrateOnOpen =
  Platform.OS === 'android' ? () => void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {}) : undefined;

interface Props {
  panel: Panel;
  editingId: string | null;
  busy: boolean;
  onEdit: (id: string) => void;
  onCancelEdit: () => void;
  onSteer: (id: string) => void;
  onRemove: (id: string) => void;
  onMove: (id: string, delta: -1 | 1) => void;
  /** Só com o aviso `headless_delivery_stalled` da sessão: a fila não anda e só a pessoa reabre. */
  reopen?: { serverId: string; name: string };
  /** Envios daqui que o servidor ainda não listou: a linha nasce no envio, sem ações, e a do servidor toma o lugar. */
  locals?: readonly PendingMsg[];
  /** A conversa, para a cópia que sai do campo pousar na linha local. */
  route?: string | null;
}

const NO_LOCALS: readonly PendingMsg[] = [];

// O servidor nunca reabre sozinho a sessão surda: este botão é a única saída, e só existe com o aviso ativo.
function ReopenStalled({ serverId, name }: { serverId: string; name: string }) {
  const { theme } = useUnistyles();
  const [busy, setBusy] = useState(false);
  // O VoiceOver não anuncia `accessibilityRole="alert"` num Text: o aviso é falado ao aparecer.
  useEffect(() => { AccessibilityInfo.announceForAccessibility(m.problema_headless_delivery_stalled()); }, []);
  const reopen = async () => {
    const server = useServers.getState().servers.find((s) => s.id === serverId);
    if (!server) { toast.erro(m.chat_servidor_removido()); return; }
    setBusy(true);
    try {
      await recarregarSessao(name, server);
      haptic.success();
    } catch (e) {
      toast.erro(m.erro_recarregar({ erro: (e instanceof Error ? e.message : formataErro(e)) || m.erro_desconhecido() }));
    } finally {
      setBusy(false);
    }
  };
  return (
    <View style={styles.stalled}>
      <Text style={[styles.stalledText, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{m.problema_headless_delivery_stalled()}</Text>
      <Pressable accessibilityRole="button" accessibilityLabel={m.reabrir_sessao_presa()}
        accessibilityState={{ disabled: busy, busy }} disabled={busy}
        onPress={() => void reopen()} style={[styles.reopen, busy && styles.disabled]}>
        {busy ? <ActivityIndicator size="small" color={theme.tokens.accent.base} /> : null}
        <Text style={styles.cancel}>{m.reabrir_sessao_presa()}</Text>
      </Pressable>
    </View>
  );
}

// ponytail: a linha nasce e é despachada em escritas separadas, e entre as duas ela aparece como "esperando"
// por um instante; o painel só surge se ela continuar esperando, para a mensagem que já está indo não piscar.
export const QUEUE_PANEL_APPEAR_MS = QUEUE_APPEAR_MS;

export function QueuePanel({ panel, editingId, busy, onEdit, onCancelEdit, onSteer, onRemove, onMove, reopen, locals = NO_LOCALS, route = null }: Props) {
  const { theme } = useUnistyles();
  const [expanded, setExpanded] = useState(true);
  const [full, setFull] = useState<string | null>(null);
  const waiting = panel.entries.length + locals.length > 0;
  // Quem enviou com a sessão trabalhando sabe que a mensagem espera: a linha local abre o painel na hora.
  const sentHere = locals.length > 0;
  const [shown, setShown] = useState(waiting);
  useEffect(() => {
    if (!waiting) { setShown(false); return; }
    if (sentHere) { setShown(true); return; }
    const timer = setTimeout(() => setShown(true), QUEUE_PANEL_APPEAR_MS);
    return () => clearTimeout(timer);
  }, [waiting, sentHere]);
  if (!waiting || !shown) return null;
  const pick = (id: string, action: MenuId) => {
    void Haptics.selectionAsync().catch(() => {});
    if (action === 'up') onMove(id, -1);
    else if (action === 'down') onMove(id, 1);
    else if (action === 'steer') onSteer(id);
    else if (action === 'edit') onEdit(id);
    else onRemove(id);
  };
  const muted = theme.tokens.text.muted;
  return (
    <View style={styles.band}>
      <View style={styles.header}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={expanded ? m.queue_panel_collapse() : m.queue_panel_expand()}
          accessibilityState={{ expanded }}
          onPress={() => setExpanded(!expanded)}
          hitSlop={4}
          style={styles.toggle}
        >
          <Icon name="ListOrdered" size={14} color={muted} />
          <Text style={styles.title}>{m.queue_panel_title()}</Text>
          <Text style={[styles.count, { backgroundColor: theme.tokens.fillSubtle }]}>{panel.entries.length + locals.length}</Text>
          <Icon name={expanded ? 'ChevronDown' : 'ChevronUp'} size={14} color={muted} />
        </Pressable>
      </View>
      {reopen ? <ReopenStalled serverId={reopen.serverId} name={reopen.name} /> : null}
      {expanded ? (
        <ScrollView style={styles.list} nestedScrollEnabled keyboardShouldPersistTaps="handled">
          {panel.entries.map((entry, index) => {
            const editing = editingId === entry.id;
            const actions = menuActions({ index, count: panel.entries.length, busy, editing });
            const enabled = actions.filter((a) => !a.disabled);
            const openIOS = () => {
              void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {});
              const destructive = enabled.findIndex((a) => a.destructive);
              ActionSheetIOS.showActionSheetWithOptions(
                {
                  title: contextRefsPlainText(entry.text).slice(0, 80),
                  options: [...enabled.map((a) => a.title), m.comum_cancelar()],
                  cancelButtonIndex: enabled.length,
                  destructiveButtonIndex: destructive >= 0 ? destructive : undefined,
                },
                (i) => { if (i < enabled.length) pick(entry.id, enabled[i].id); },
              );
            };
            const row = (
              <Pressable
                onPress={() => setFull(full === entry.id ? null : entry.id)}
                // No Android o toque longo é do MenuView; o no-op impede que soltar o dedo vire toque.
                onLongPress={!panel.actions ? undefined : Platform.OS === 'ios' ? openIOS : () => {}}
                delayLongPress={350}
                accessibilityLabel={contextRefsPlainText(entry.text)}
                accessibilityHint={panel.actions ? m.queue_panel_actions() : undefined}
                // O menu mora no toque longo, que o leitor de tela não alcança: as ações viram ações da linha.
                accessibilityActions={panel.actions ? enabled.map((a) => ({ name: a.id, label: a.title })) : undefined}
                onAccessibilityAction={({ nativeEvent }) => {
                  if (enabled.some((a) => a.id === nativeEvent.actionName)) pick(entry.id, nativeEvent.actionName as MenuId);
                }}
                style={[styles.row, editing && { backgroundColor: theme.tokens.accent.dim }]}
              >
                <Text style={styles.text} numberOfLines={full === entry.id ? undefined : 1}>{contextRefsPlainText(entry.text)}</Text>
                {editing ? (
                  <Pressable accessibilityRole="button" onPress={onCancelEdit} disabled={busy} hitSlop={8} style={styles.cancelBtn}>
                    <Text style={[styles.cancel, busy && styles.disabled]}>{m.comum_cancelar()}</Text>
                  </Pressable>
                ) : null}
                {panel.actions && !editing ? (
                  <>
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={m.queue_panel_steer_message({ text: entry.text.replace(/\s+/g, ' ').trim().slice(0, 80) })}
                      accessibilityHint={m.codex_orientar_ajuda()}
                      accessibilityState={{ disabled: busy }}
                      disabled={busy}
                      onPress={() => pick(entry.id, 'steer')}
                      hitSlop={6}
                      style={[styles.iconBtn, busy && styles.disabled]}
                    >
                      <Icon name="CornerUpRight" size={16} color={theme.tokens.accent.base} />
                    </Pressable>
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={m.queue_panel_remove()}
                      accessibilityState={{ disabled: busy }}
                      disabled={busy}
                      onPress={() => pick(entry.id, 'remove')}
                      hitSlop={6}
                      style={[styles.iconBtn, busy && styles.disabled]}
                    >
                      <Icon name="X" size={16} color={muted} />
                    </Pressable>
                  </>
                ) : null}
              </Pressable>
            );
            if (!panel.actions || Platform.OS === 'ios') return <View key={entry.id}>{row}</View>;
            return (
              <MenuView
                key={entry.id}
                shouldOpenOnLongPress
                title={m.queue_panel_actions()}
                actions={actions.map((a) => ({ id: a.id, title: a.title, attributes: { disabled: a.disabled, destructive: !!a.destructive } }))}
                onPressAction={({ nativeEvent }: NativeActionEvent) => {
                  const action = actions.find((a) => a.id === nativeEvent.event);
                  if (action && !action.disabled) pick(entry.id, action.id);
                }}
                onOpenMenu={vibrateOnOpen}
              >
                {row}
              </MenuView>
            );
          })}
          {locals.map((local) => (
            <FlightDestination key={local.id} route={route} text={local.text} target="queue" style={styles.row}>
              <Text style={styles.text} numberOfLines={1} accessibilityLabel={contextRefsPlainText(local.text)}>{contextRefsPlainText(local.text)}</Text>
            </FlightDestination>
          ))}
        </ScrollView>
      ) : null}
    </View>
  );
}
QueuePanel.menuActions = menuActions;

const ROW_HEIGHT = 40;

const styles = StyleSheet.create((theme) => ({
  // Faixa de cima da caixa do composer: anula o recuo dela para a linha de baixo ir de borda a borda.
  band: {
    marginHorizontal: -theme.base.space[3],
    marginTop: -theme.base.space[3],
    marginBottom: theme.base.space[1],
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: theme.tokens.border.default,
  },
  header: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingLeft: theme.base.space[3], paddingRight: theme.base.space[2], minHeight: 44 },
  toggle: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], minHeight: 44 },
  title: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.muted },
  count: { fontSize: theme.base.text.xxs, color: theme.tokens.text.muted, fontVariant: ['tabular-nums'], paddingHorizontal: theme.base.space[1.5], borderRadius: theme.base.radius.full, overflow: 'hidden' },
  // Cerca de quatro linhas; passou disso, a lista rola dentro do painel.
  list: { maxHeight: 4 * ROW_HEIGHT + 6, paddingHorizontal: theme.base.space[1], marginBottom: theme.base.space[1] },
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minHeight: ROW_HEIGHT, paddingLeft: theme.base.space[2], borderRadius: theme.base.radius.sm },
  text: { flex: 1, fontSize: theme.base.text.xs, color: theme.tokens.text.primary, paddingVertical: theme.base.space[1.5] },
  iconBtn: { width: 32, height: 32, borderRadius: theme.base.radius.full, alignItems: 'center', justifyContent: 'center' },
  cancelBtn: { minHeight: 32, justifyContent: 'center', paddingHorizontal: theme.base.space[2] },
  cancel: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, color: theme.tokens.accent.base },
  disabled: { opacity: 0.4 },
  stalled: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingLeft: theme.base.space[3], paddingRight: theme.base.space[2] },
  stalledText: { flex: 1, fontSize: theme.base.text.xs },
  reopen: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], paddingHorizontal: theme.base.space[2] },
}));
