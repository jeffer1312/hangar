import { memo, useState } from 'react';
import { Text, View } from 'react-native';
import Animated, { FadeIn, FadeInDown, FadeOut, LinearTransition, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { accountMatchRange, type CodexAccount, type Credencial, type Server } from '@hangar/core';
import { Pill } from '../config/PageHeader';
import { useSettingsColors, withAlpha } from '../config/colors';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { AccountUsage } from './AccountUsage';
import { AccountReset } from './CodexReset';
import { ClaudeAccountActions } from './ClaudeAccountActions';
import { MachineSheet } from '../machines/parts';
import { TagEditor } from '../tags/TagEditor';
import * as m from '../../paraglide/messages';

export type AccountRowData = {
  key: string;
  provider: 'claude' | 'codex' | 'kimi' | '';
  title: string;
  plan: string;
  email: string;
  description: string;
  active: boolean;
  account: CodexAccount | null;
  credential: Credencial | null;
  engineName?: string;
  canSignIn: boolean;
};

export type AccountRowHandlers = {
  signIn: (accountId: string) => void;
  remove: (account: CodexAccount) => void;
  editEngine: (name: string) => void;
  removeEngine: (name: string) => void;
  refreshClaude: () => Promise<void>;
};

const MOVE = LinearTransition.duration(420);

function Highlight({ text, term, style, markColor }: { text: string; term: string; style: object; markColor: string }) {
  const range = accountMatchRange(text, term);
  if (!range) return <Text style={style} numberOfLines={1}>{text}</Text>;
  return (
    <Text style={style} numberOfLines={1}>
      {text.slice(0, range[0])}
      <Text style={[styles.mark, { backgroundColor: markColor }]}>{text.slice(range[0], range[1])}</Text>
      {text.slice(range[1])}
    </Text>
  );
}

function Avatar({ row }: { row: AccountRowData }) {
  const c = useSettingsColors();
  if (row.provider) return <ProviderGlyph provider={row.provider} size={36} />;
  const initial = [...row.title].find((ch) => ch.toLowerCase() !== ch.toUpperCase() || /\d/.test(ch))?.toUpperCase() ?? '?';
  return (
    <View style={[styles.initial, { backgroundColor: c.hover }]} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Text style={[styles.initialText, { color: c.muted }]} maxFontSizeMultiplier={1}>{initial}</Text>
    </View>
  );
}

type Props = {
  row: AccountRowData;
  term: string;
  /** Posição na primeira pintura (entrada escalonada); `null` = linha que entrou depois (filtro, busca). */
  enterIndex: number | null;
  server: Server | null;
  busy: boolean;
  engineReady: boolean;
  engineBusy: boolean;
  handlers: AccountRowHandlers;
};

export const AccountRow = memo(function AccountRow({ row, term, enterIndex, server, busy, engineReady, engineBusy, handlers }: Props) {
  const c = useSettingsColors();
  const reduced = useReducedMotion();
  const { credential, account } = row;
  // A folha só monta na primeira abertura. Convidado não tem `/api/tags`.
  const [tagOpen, setTagOpen] = useState(false);
  const tagServer = credential && server && !server.invite ? server : null;
  const entering = reduced ? undefined : enterIndex == null ? FadeIn.duration(320)
    : FadeInDown.duration(500).delay(35 * Math.min(enterIndex, 12)).withInitialValues({ opacity: 0, transform: [{ translateY: 4 }] });
  return (
    <Animated.View entering={entering} exiting={reduced ? undefined : FadeOut.duration(160)} layout={reduced ? undefined : MOVE}
      style={[styles.row, { borderTopColor: c.border }, row.active && { backgroundColor: withAlpha(c.accent, 0.06) }]}>
      <View style={styles.head}>
        <Avatar row={row} />
        <View style={styles.texts}>
          <View style={styles.nameLine}>
            <Highlight text={row.title} term={term} style={[styles.title, { color: c.text }]} markColor={c.accentDim} />
            {row.plan ? <Text style={[styles.plan, { color: c.muted, borderColor: c.borderStrong }]}>{row.plan}</Text> : null}
            {row.active ? <Text style={[styles.inUse, { color: c.accentText, backgroundColor: c.accentDim }]}>{m.contas_em_uso()}</Text> : null}
          </View>
          {row.email ? <Highlight text={row.email} term={term} style={[styles.email, { color: c.muted }]} markColor={c.accentDim} /> : null}
          {row.description ? <Text style={[styles.desc, { color: c.muted }]}>{row.description}</Text> : null}
        </View>
      </View>
      <View style={styles.body}>
        {credential && (credential.cota || credential.tipo === 'claude' || credential.tipo === 'codex')
          ? <AccountUsage credential={credential} animate={!reduced} grow={enterIndex != null} /> : null}
        {credential && server ? <AccountReset server={server} credential={credential} /> : null}
        {credential?.tipo === 'claude' && server ? (
          <ClaudeAccountActions server={server} credential={credential} onChanged={handlers.refreshClaude} />
        ) : null}
        {tagServer ? (
          <View style={styles.actions}>
            <Pill icon="Tag" label={m.tags_account_title()} onPress={() => setTagOpen(true)} />
          </View>
        ) : null}
        {tagOpen && tagServer && credential ? (
          <MachineSheet title={m.tags_account_title()} onClose={() => setTagOpen(false)}>
            <TagEditor target={{ kind: 'account', server: tagServer, accountId: credential.id, provider: row.provider || credential.tipo }} />
          </MachineSheet>
        ) : null}
        {row.engineName && engineReady ? (
          <View style={styles.actions}>
            <Pill label={m.mobile_accounts_engine_edit()} disabled={engineBusy} onPress={() => handlers.editEngine(row.engineName!)} />
            <Pill label={m.mobile_accounts_engine_delete()} disabled={engineBusy} onPress={() => handlers.removeEngine(row.engineName!)} />
          </View>
        ) : null}
        {account && (row.canSignIn || !account.is_default) ? (
          <View style={styles.actions}>
            {row.canSignIn ? <Pill icon="LogIn" label={m.contas_entrar()} onPress={() => handlers.signIn(account.id)} disabled={busy} /> : null}
            {!account.is_default ? <Pill icon="Trash2" label={m.lista_remover()} onPress={() => handlers.remove(account)} disabled={busy} /> : null}
          </View>
        ) : null}
      </View>
    </Animated.View>
  );
});

const AVATAR = 36;
const styles = StyleSheet.create((theme) => ({
  row: { borderTopWidth: 1, paddingVertical: theme.base.space[3], paddingHorizontal: theme.base.space[4], gap: theme.base.space[2] },
  head: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[3] },
  initial: { width: AVATAR, height: AVATAR, borderRadius: theme.base.radius.sm, alignItems: 'center', justifyContent: 'center' },
  initialText: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.bold },
  texts: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  nameLine: { flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: theme.base.space[1.5] },
  title: { flexShrink: 1, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  plan: { fontSize: theme.base.text.xxxs, borderWidth: 1, borderRadius: theme.base.radius.xs, paddingHorizontal: theme.base.space[1.5], overflow: 'hidden' },
  inUse: { fontSize: theme.base.text.xxxs, borderRadius: theme.base.radius.full, paddingHorizontal: theme.base.space[2], overflow: 'hidden' },
  email: { fontSize: theme.base.text.xs },
  desc: { fontSize: theme.base.text.xs },
  mark: { borderRadius: 3 },
  body: { gap: theme.base.space[2], paddingLeft: AVATAR + theme.base.space[3] },
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2] },
}));
