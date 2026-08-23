import { useCallback, useEffect, useState } from 'react';
import { Pressable, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { getPermissionModes, setPermissionMode } from '@hangar/core';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';
import { PillMenu, type PillMenuItem } from './PillMenu';

interface Props {
  serverId: string;
  name: string;
}

export function PermissionPill({ serverId, name }: Props) {
  const { theme } = useUnistyles();
  const provider = useSessions((s) => {
    const r = s.rows.find((x) => x.name === name);
    const byServer = s.byServerRecord?.[serverId];
    if (byServer) {
      const hit = byServer.find((x) => x.name === name);
      if (hit?.provider) return hit.provider;
    }
    return (r?.provider ?? null) as string | null;
  });
  const isClaude = provider === null || provider === 'claude' || provider === undefined;

  const [current, setCurrent] = useState<string | null>(null);
  const [tempError, setTempError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [items, setItems] = useState<PillMenuItem[]>([]);

  if (!isClaude) return null;

  const display = tempError ?? current ?? m.composer_permissao();

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await getPermissionModes(name);
      setCurrent(res.current);
      setItems(res.modes.map((mo) => ({ label: mo, selected: mo === res.current })));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [name]);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  const handleSelect = useCallback(
    async (it: PillMenuItem) => {
      try {
        const res = await setPermissionMode(name, it.label);
        const ficou = (res as { mode?: string; current?: string }).mode ?? (res as { current?: string }).current ?? it.label;
        setCurrent(ficou);
        setOpen(false);
      } catch (e) {
        const status = (e as { status?: number }).status;
        const msg = e instanceof Error ? e.message : String(e);
        if (status === 409) {
          setTempError(msg || m.composer_sessao_trabalhando());
          setOpen(false);
          setTimeout(() => setTempError(null), 8000);
          // tenta re-ler o atual
          try {
            const cur = await getPermissionModes(name);
            setCurrent(cur.current);
            setItems(cur.modes.map((mo) => ({ label: mo, selected: mo === cur.current })));
          } catch {}
        } else {
          setError(msg);
        }
      }
    },
    [name],
  );

  return (
    <>
      <Pressable
        onPress={() => setOpen(true)}
        style={[styles.pill, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]}
        accessibilityRole="button"
        accessibilityLabel={m.composer_permissao()}
      >
        <Text style={[styles.pillText, { color: theme.tokens.text.primary }]} numberOfLines={1}>
          {display}
        </Text>
      </Pressable>
      <PillMenu
        open={open}
        onClose={() => setOpen(false)}
        items={items}
        loading={loading}
        error={error}
        onRetry={() => void load()}
        onSelect={handleSelect}
        title={m.composer_permissao()}
      />
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  pill: {
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 6,
    minHeight: 32,
    justifyContent: 'center',
  },
  pillText: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
}));
