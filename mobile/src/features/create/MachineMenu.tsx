import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { AccountBadge, PlacementSuggestion } from '@hangar/core';
import { Icon, type IconName } from '../../ui/Icon';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

export type MachineEntry = {
  id: string;
  label: string;
  // Nome do servidor quando a etiqueta o renomeou: separa duas máquinas de etiqueta parecida.
  server?: string;
  color: string;
  icon: IconName;
  accounts: Record<string, AccountBadge> | undefined;
  demo: boolean;
  offline: boolean;
};

// Mesma ordem dos agentes do app de PC; depois as chaves de API (`chave:`) e, por último, as outras contas.
const PROVIDER_ORDER = ['claude', 'codex', 'pi', 'kimi', 'omp', 'antigravity'];

export type AccountGroup = { provider: string; names: string[] };

/** O grupo da conta pelo prefixo: CLIProxy entra no Codex, como no PC; fora dos agentes e das chaves, o grupo vazio. */
function groupOf(key: string): string {
  const prefix = key.includes(':') ? key.slice(0, key.indexOf(':')) : '';
  if (prefix === 'cliproxy') return 'codex';
  return prefix === 'chave' || PROVIDER_ORDER.includes(prefix) ? prefix : '';
}

/** As contas da máquina por agente: um item por agente, para nenhum agente sumir atrás de um "+N". */
export function accountGroups(accounts: Record<string, AccountBadge> | undefined): AccountGroup[] {
  const groups = new Map<string, string[]>();
  for (const [key, badge] of Object.entries(accounts ?? {})) {
    const provider = groupOf(key);
    groups.set(provider, [...(groups.get(provider) ?? []), badge.name.trim()]);
  }
  const rank = (p: string) => {
    const i = PROVIDER_ORDER.indexOf(p);
    return i >= 0 ? i : PROVIDER_ORDER.length + (p === '' ? 1 : 0);
  };
  return [...groups].map(([provider, names]) => ({ provider, names: names.sort() }))
    .sort((a, b) => rank(a.provider) - rank(b.provider));
}

/** O quadrado da máquina: o ícone da etiqueta na cor dela, igual na pílula e no menu. */
export function MachineTile({ icon, color, size = 38 }: { icon: IconName; color: string; size?: number }) {
  return (
    <View style={[styles.tile, { width: size, height: size, backgroundColor: `${color}26` }]}>
      <Icon name={icon} size={Math.round(size * 0.52)} color={color} />
    </View>
  );
}

function Accounts({ accounts }: { accounts: Record<string, AccountBadge> | undefined }) {
  const { theme } = useUnistyles();
  const groups = accountGroups(accounts);
  if (!groups.length) return null;
  return (
    <View style={styles.accounts}>
      {groups.map(({ provider, names }) => (
        <View key={provider} style={styles.account}>
          {PROVIDER_ORDER.includes(provider) ? <ProviderGlyph provider={provider} size={14} />
            : <Icon name={provider === 'chave' ? 'KeyRound' : 'CreditCard'} size={12} color={theme.tokens.text.muted} />}
          {/* Só o nome de uma conta encolhe com "…"; a contagem fica sempre inteira. */}
          {names.length === 1
            ? <Text style={styles.accountName} numberOfLines={1}>{names[0]}</Text>
            : <Text style={[styles.accountName, styles.accountCount]}>{m.native_new_chat_machine_accounts({ n: names.length })}</Text>}
        </View>
      ))}
    </View>
  );
}

// O menu de máquinas da conversa nova: cada máquina com o ícone dela e as contas que tem; a demonstração por último.
// `suggested`: a máquina que o balanceamento marcou, com o motivo na linha dela.
export function MachineMenu({ machines, active, onPick, suggested }: {
  machines: MachineEntry[]; active: string; onPick: (id: string) => void; suggested?: PlacementSuggestion | null;
}) {
  const { theme } = useUnistyles();
  const real = machines.filter((sv) => !sv.demo);
  const demos = machines.filter((sv) => sv.demo);
  const row = (sv: MachineEntry) => {
    const on = sv.id === active;
    const state = sv.demo ? m.native_new_chat_machine_demo() : sv.offline ? m.native_new_chat_machine_offline() : '';
    const reason = suggested?.serverId === sv.id ? m.native_new_chat_machine_suggested({ motivo: suggested.text }) : '';
    return (
      <Pressable key={sv.id} onPress={() => { haptic.selection(); onPick(sv.id); }}
        accessibilityRole="button" accessibilityLabel={[sv.label, sv.server, state, reason].filter(Boolean).join(', ')} accessibilityState={{ selected: on }}
        style={({ pressed }) => [styles.row, { backgroundColor: on ? theme.tokens.accent.dim : pressed ? theme.tokens.fillSubtle : 'transparent' },
          sv.offline && styles.offline]}>
        <MachineTile icon={sv.icon} color={sv.color} />
        <View style={styles.text}>
          <Text style={styles.nameLine} numberOfLines={1}>
            <Text style={[styles.name, { color: sv.color }]}>{sv.label}</Text>
            {sv.server ? <Text style={styles.state}>{` · ${sv.server}`}</Text> : null}
            {state ? <Text style={styles.state}>{` · ${state}`}</Text> : null}
          </Text>
          {sv.demo ? <Text style={styles.accountName}>{m.native_new_chat_machine_demo_hint()}</Text> : <Accounts accounts={sv.accounts} />}
          {reason ? <Text style={styles.accountName}>{reason}</Text> : null}
        </View>
        <View style={styles.check}>{on ? <Icon name="Check" size={18} color={theme.tokens.accent.base} /> : null}</View>
      </Pressable>
    );
  };
  return (
    <ScrollView style={styles.scroll} keyboardShouldPersistTaps="handled">
      <Text style={styles.header} accessibilityRole="header">{m.native_new_chat_machine_menu_title()}</Text>
      {real.map(row)}
      {demos.length && real.length ? <View style={styles.separator} /> : null}
      {demos.map(row)}
    </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  scroll: { flexGrow: 0, flexShrink: 1 },
  header: {
    fontSize: theme.base.text.xxxs, fontWeight: theme.base.weight.semibold, letterSpacing: 0.6, textTransform: 'uppercase',
    color: theme.tokens.text.muted, paddingHorizontal: theme.base.space[2], paddingTop: theme.base.space[2], paddingBottom: theme.base.space[1.5],
  },
  row: {
    minHeight: 56, flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[3],
    paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[2], borderRadius: theme.base.radius.md,
  },
  offline: { opacity: 0.5 },
  tile: { borderRadius: theme.base.radius.md, alignItems: 'center', justifyContent: 'center' },
  text: { flex: 1, minWidth: 0, gap: theme.base.space[1], paddingTop: theme.base.space[0.5] },
  nameLine: { fontSize: theme.base.text.sm },
  name: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  state: { fontSize: theme.base.text.xxs, color: theme.tokens.text.muted },
  accounts: { flexDirection: 'row', flexWrap: 'wrap', columnGap: theme.base.space[3], rowGap: theme.base.space[1] },
  account: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], maxWidth: '100%' },
  accountName: { flexShrink: 1, fontSize: theme.base.text.xxs, color: theme.tokens.text.secondary },
  accountCount: { flexShrink: 0 },
  check: { width: 22, height: 38, alignItems: 'center', justifyContent: 'center' },
  separator: { height: StyleSheet.hairlineWidth, backgroundColor: theme.tokens.border.default, marginVertical: theme.base.space[1], marginHorizontal: theme.base.space[2] },
}));
