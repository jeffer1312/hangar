import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { relativeTime, type FolderGit } from '@hangar/core';
import { Icon, type IconName } from '../../ui/Icon';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';

export type GitAction = 'fetch' | 'pull';
type Props = { git: FolderGit; busy: GitAction | null; note: { ok: boolean; text: string } | null; onAction: (action: GitAction) => void };

function gitDetail(git: FolderGit): string {
  const fetched = git.last_fetch ? m.native_folder_git_fetched({ quando: relativeTime(git.last_fetch) }) : m.native_folder_git_never_fetched();
  return `${git.upstream ?? m.native_folder_git_no_upstream()} · ${fetched}`;
}

// Mesmas condições da recusa do servidor: o motivo aparece antes do toque.
function pullBlock(git: FolderGit): string {
  if (git.dirty) return m.native_folder_git_dirty({ n: git.dirty });
  if (!git.upstream) return m.native_folder_git_no_upstream_pull();
  if ((git.ahead ?? 0) > 0 && (git.behind ?? 0) > 0) return m.native_folder_git_diverged({ upstream: git.upstream, ahead: git.ahead ?? 0, behind: git.behind ?? 0 });
  return '';
}

export function FolderGitCard({ git, busy, note, onAction }: Props) {
  const { theme } = useUnistyles();
  const blocked = pullBlock(git);
  const behind = git.behind ?? 0;
  const ahead = git.ahead ?? 0;
  const button = (action: GitAction, label: string, icon: IconName, disabled: boolean, primary = false) => {
    const ink = primary ? theme.tokens.accent.base : theme.tokens.text.primary;
    return (
      <Pressable onPress={() => onAction(action)} disabled={disabled} accessibilityRole="button" accessibilityLabel={label}
        accessibilityState={{ disabled, busy: busy === action }} style={[styles.button, primary && styles.buttonPrimary, disabled && styles.dim]}>
        {busy === action ? <ActivityIndicator size="small" color={ink} /> : <Icon name={icon} size={15} color={ink} />}
        <Text style={[styles.buttonText, { color: ink }]} numberOfLines={1}>{label}</Text>
      </Pressable>
    );
  };
  const count = (icon: IconName, n: number, label: string) => (
    <View style={styles.count} accessible accessibilityLabel={label}>
      <Icon name={icon} size={12} color={theme.tokens.accent.base} />
      <Text style={styles.countText}>{n}</Text>
    </View>
  );
  return (
    <View style={styles.card}>
      <View style={styles.top}>
        <Icon name="GitBranch" size={15} color={theme.tokens.text.secondary} />
        <Text selectable style={styles.branch} numberOfLines={1} ellipsizeMode="middle">{git.current ?? 'HEAD'}</Text>
        {git.upstream && !behind && !ahead ? (
          <View style={styles.count}>
            <Icon name="Check" size={12} color={theme.tokens.status.success} />
            <Text style={styles.done}>{m.native_folder_git_up_to_date()}</Text>
          </View>
        ) : null}
        {git.upstream && behind ? count('ArrowDown', behind, m.native_folder_git_behind({ n: behind })) : null}
        {git.upstream && ahead ? count('ArrowUp', ahead, m.native_folder_git_ahead({ n: ahead })) : null}
      </View>
      <Text style={styles.detail} numberOfLines={2} accessibilityLiveRegion="polite">{gitDetail(git)}</Text>
      <View style={styles.line}>
        {button('fetch', m.native_folder_git_fetch(), 'RefreshCw', !!busy)}
        {button('pull', m.native_folder_git_pull(), 'ArrowDown', !!busy || !!blocked, behind > 0 && !blocked)}
      </View>
      {note && !note.ok ? <Text selectable style={styles.error} accessibilityRole="alert">{note.text}</Text>
        : blocked ? <Text style={styles.warn}>{blocked}</Text>
        : note ? <Text style={styles.done} accessibilityLiveRegion="polite">{note.text}</Text> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  card: {
    gap: theme.base.space[2], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    backgroundColor: superficie(theme),
  },
  top: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  branch: { flex: 1, minWidth: 0, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  count: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[0.5] },
  countText: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, color: theme.tokens.accent.base, fontVariant: ['tabular-nums'] },
  detail: { color: theme.tokens.text.secondary, fontSize: theme.base.text.xs },
  line: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  button: {
    flex: 1, minHeight: 44, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: theme.base.space[1.5],
    borderRadius: theme.base.radius.md, borderCurve: 'continuous', borderWidth: 1, borderColor: theme.tokens.border.default,
  },
  buttonPrimary: { backgroundColor: theme.tokens.accent.dim, borderColor: theme.tokens.accent.base },
  buttonText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  dim: { opacity: 0.5 },
  done: { color: theme.tokens.status.success, fontSize: theme.base.text.xs },
  warn: { color: theme.tokens.status.warning, fontSize: theme.base.text.xs },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.sm },
}));
