import { useCallback, useEffect, useMemo, useState } from 'react';
import { Pressable, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { parseStatusLine, getPiModels, getKimiModels, getCodexModels, setModelEffort, setPiModel, setKimiModel, setCodexModel } from '@hangar/core';
import { chatStore } from '../../stores/chat';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';
import { PillMenu, type PillMenuItem } from './PillMenu';
import { pillLabels, semEsforco } from './pills';

const CLAUDE_EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max', 'ultracode'];

interface Props {
  serverId: string;
  name: string;
}

export function EffortPill({ serverId, name }: Props) {
  const { theme } = useUnistyles();
  const chat = chatStore(serverId, name);
  const statusLine = chat.use((s) => s.statusLine);
  const statusFields = useMemo(() => parseStatusLine(statusLine), [statusLine]);
  const provider = useSessions((s) => {
    const r = s.rows.find((x) => x.name === name);
    const byServer = s.byServerRecord?.[serverId];
    if (byServer) {
      const hit = byServer.find((x) => x.name === name);
      if (hit?.provider) return hit.provider;
    }
    return (r?.provider ?? null) as string | null;
  });

  const isCodex = provider === 'codex';
  const isPi = provider === 'pi';
  const isKimi = provider === 'kimi';
  const isClaude = !isCodex && !isPi && !isKimi;

  const [chosenEffort, setChosenEffort] = useState<string | null>(null);
  const [tempError, setTempError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [items, setItems] = useState<PillMenuItem[]>([]);

  // haiku não tem esforço — esconde (retorno condicional após todos os hooks, seguro)
  const modelLabel = pillLabels(statusFields, {}).model;

  const display = useMemo(() => {
    if (tempError) return tempError;
    return pillLabels(statusFields, { effort: chosenEffort }).effort ?? m.composer_nivel();
  }, [statusFields, chosenEffort, tempError]);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      if (isPi) {
        const res = await getPiModels(name);
        const levels = res.levels ?? [];
        setItems(levels.map((lv) => ({ label: lv, selected: lv === (chosenEffort ?? statusFields?.effort) })));
      } else if (isKimi) {
        const res = await getKimiModels(name);
        const curName = statusFields?.model ?? '';
        const entry = res.models.find((mo) => mo.name.toLowerCase() === curName.toLowerCase());
        const levels = entry?.efforts ?? [];
        setItems(levels.map((lv) => ({ label: lv, selected: lv === (chosenEffort ?? statusFields?.effort) })));
      } else if (isCodex) {
        const res = await getCodexModels(name);
        // pega esforços do modelo atual
        const cur = res.current.model;
        const hit = res.models.find((mo) => mo.model === cur);
        const levels = hit?.efforts.map((e) => e.value) ?? [];
        const effective = levels.length ? levels : res.models.flatMap((mo) => mo.efforts.map((e) => e.value));
        // dedup
        const uniq = [...new Set(effective)];
        setItems(uniq.map((lv) => ({ label: lv, selected: lv === (chosenEffort ?? statusFields?.effort) })));
      } else {
        setItems(CLAUDE_EFFORTS.map((lv) => ({ label: lv, selected: lv === (chosenEffort ?? statusFields?.effort) })));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [name, isPi, isKimi, isCodex, chosenEffort, statusFields?.effort, statusFields?.model]);

  useEffect(() => {
    if (open && !(isClaude && semEsforco(modelLabel))) void load();
  }, [open, load, isClaude, modelLabel]);

  const handleSelect = useCallback(
    async (it: PillMenuItem) => {
      try {
        if (isPi) {
          const res = await setPiModel(name, { effort: it.label });
          // Pi clampa: pinta o que voltou
          if (res.thinking) setChosenEffort(res.thinking);
          else setChosenEffort(it.label);
          setOpen(false);
        } else if (isKimi) {
          const res = await setKimiModel(name, { effort: it.label });
          if (res.effort) setChosenEffort(res.effort);
          else setChosenEffort(it.label);
          setOpen(false);
        } else if (isCodex) {
          const codex = await getCodexModels(name);
          const curModel = codex.current.model ?? codex.models[0]?.model ?? '';
          await setCodexModel(name, curModel, it.label);
          setChosenEffort(it.label);
          setOpen(false);
        } else {
          const res = await setModelEffort(name, { effort: it.label, scope: 'session' });
          if (res?.pending_confirm) {
            setOpen(false);
            return;
          }
          setChosenEffort(it.label);
          setOpen(false);
        }
      } catch (e) {
        const status = (e as { status?: number }).status;
        const msg = e instanceof Error ? e.message : String(e);
        if (status === 409) {
          setTempError(msg || m.composer_sessao_trabalhando());
          setOpen(false);
          setTimeout(() => setTempError(null), 8000);
        } else {
          setError(msg);
        }
      }
    },
    [name, isPi, isKimi, isCodex],
  );

  if (isClaude && semEsforco(modelLabel)) return null;

  return (
    <>
      <Pressable
        onPress={() => setOpen(true)}
        style={[styles.pill, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]}
        accessibilityRole="button"
        accessibilityLabel={m.composer_nivel()}
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
        title={m.composer_nivel()}
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
