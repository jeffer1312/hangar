import { useEffect, useState } from 'react';
import { ActivityIndicator, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { getSubagent } from '@hangar/core';
import type { SubagentRun } from '@hangar/core';
import { MessageList } from '../../chat/MessageList';
import * as m from '../../paraglide/messages';

interface Props {
  sessionName: string;
  agentId: string;
}

// Lida com os três desfechos: sucesso (pinta), falha (texto), pendente (spinner).
export function SubagentLive({ sessionName, agentId }: Props) {
  const { theme } = useUnistyles();
  const [detail, setDetail] = useState<SubagentRun | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [fails, setFails] = useState(0);

  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setInterval> | null = null;
    let gen = 0;
    let failCount = 0;

    async function tick() {
      const myGen = ++gen;
      try {
        const d = await getSubagent(sessionName, agentId, 60);
        if (!alive || myGen !== gen) return;
        setDetail(d);
        setError('');
        failCount = 0;
        setFails(0);
        setLoading(false);
      } catch (e) {
        if (!alive || myGen !== gen) return;
        failCount += 1;
        setFails(failCount);
        if (failCount >= 3) {
          if (timer) {
            clearInterval(timer);
            timer = null;
          }
          setError(m.atividade_erro_vivo());
          setLoading(false);
        }
      }
    }

    void tick();
    timer = setInterval(tick, 3000);
    return () => {
      alive = false;
      if (timer) clearInterval(timer);
    };
  }, [sessionName, agentId]);

  if (loading && !detail) {
    return (
      <View style={styles.center}>
        <ActivityIndicator color={theme.tokens.text.muted} />
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
      </View>
    );
  }

  if (error && !detail) {
    return (
      <View style={styles.center}>
        <Text style={[styles.err, { color: theme.tokens.status.error }]} accessibilityRole="alert">
          {error}
        </Text>
      </View>
    );
  }

  if (!detail) {
    return (
      <View style={styles.center}>
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.atividade_sem_transcript()}</Text>
      </View>
    );
  }

  // quando há eventos, mostra a conversa com o mesmo MessageList do chat
  if (detail.events?.length) {
    return (
      <View style={styles.wrap}>
        <View style={styles.meta}>
          <Text style={[styles.metaTxt, { color: theme.tokens.text.muted }]}>
            ◐ {m.atividade_rodando()} · {m.atividade_chamadas({ n: detail.toolCalls })}
            {detail.agentType ? ` · ${detail.agentType}` : ''}
          </Text>
          {fails > 0 ? <Text style={[styles.metaErr, { color: theme.tokens.status.warning }]}>{error}</Text> : null}
        </View>
        <View style={[styles.chatBox, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.subtle }]}>
          <MessageList
            events={detail.events}
            preview=""
            statusLine={null}
            olderFailed=""
            onLoadOlder={() => {}}
            pending={[]}
            sessionName={sessionName}
          />
        </View>
        {detail.tools.length > 0 ? (
          <View style={styles.toolsRow}>
            <Text style={[styles.rotulo, { color: theme.tokens.text.muted }]}>{m.atividade_ferramentas_chamadas({ n: detail.toolCalls })}</Text>
            <View style={styles.chips}>
              {detail.tools.map((t) => (
                <View key={t.name} style={[styles.chip, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]}>
                  <Text style={[styles.chipTxt, { color: theme.tokens.text.secondary }]}>
                    {t.name} ×{t.count}
                  </Text>
                </View>
              ))}
            </View>
          </View>
        ) : null}
        <Text style={[styles.rodape, { color: theme.tokens.text.muted, borderTopColor: theme.tokens.border.subtle }]}>{m.atividade_conversa_so_leitura({ nome: sessionName })}</Text>
      </View>
    );
  }

  if (detail.toolCalls > 0) {
    return (
      <View style={styles.center}>
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.atividade_erro_transcript()}</Text>
      </View>
    );
  }

  return (
    <View style={styles.center}>
      <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.atividade_pensando()}</Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  center: { flex: 1, alignItems: 'center', justifyContent: 'center', gap: theme.base.space[2], padding: theme.base.space[4] },
  muted: { fontSize: theme.base.text.sm, textAlign: 'center' },
  err: { fontSize: theme.base.text.sm, textAlign: 'center' },
  wrap: { flex: 1, gap: theme.base.space[3] },
  meta: { gap: 4 },
  metaTxt: { fontSize: theme.base.text.xs },
  metaErr: { fontSize: theme.base.text.xs },
  chatBox: { flex: 1, minHeight: 320, maxHeight: 520, borderWidth: 1, borderRadius: theme.base.radius.md, overflow: 'hidden' },
  toolsRow: { gap: theme.base.space[1] },
  rotulo: { fontSize: 10.5, letterSpacing: 0.8, textTransform: 'uppercase', fontWeight: '600' },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[1] },
  chip: { paddingHorizontal: theme.base.space[2], paddingVertical: 3, borderRadius: theme.base.radius.sm, borderWidth: 1 },
  chipTxt: { fontFamily: theme.base.fontMono, fontSize: 11 },
  rodape: { textAlign: 'center', fontSize: 11.5, fontStyle: 'italic', borderTopWidth: 1, paddingTop: theme.base.space[2] },
}));
