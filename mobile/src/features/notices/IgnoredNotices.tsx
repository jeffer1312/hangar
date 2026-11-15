import { Pressable, Text } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { SettingsRow } from '../config/SettingsRow';
import { toast } from '../../ui/Toast';
import { putNoticeIgnore } from './api';
import { noticeKeyLabel } from './notices';
import * as m from '../../paraglide/messages';

/** Avisos ignorados no servidor ativo, com "Voltar a mostrar". Servidor que não manda a lista não mostra nada. */
export function IgnoredNotices() {
  const serverId = useServers((s) => s.activeId);
  const keys = useSessions((s) => (serverId ? s.noticeIgnores[serverId] : undefined));
  if (!serverId || !keys) return null;
  const showAgain = (key: string) => {
    putNoticeIgnore(serverId, key, false).catch(
      (error: unknown) => toast.erro(`${m.notice_ignore_failed()} · ${error instanceof Error ? error.message : String(error)}`),
    );
  };
  return (
    <>
      <SettingsRow title={m.notice_ignored_title()} description={keys.length ? undefined : m.notice_ignored_empty()} />
      {keys.map((key) => (
        <SettingsRow key={key} title={noticeKeyLabel(key)}>
          <Pressable onPress={() => showAgain(key)} hitSlop={12} accessibilityRole="button">
            <Text style={styles.link}>{m.notice_show_again()}</Text>
          </Pressable>
        </SettingsRow>
      ))}
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  link: { ...theme.type.label, color: theme.tokens.accent.base },
}));
