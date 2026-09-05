import { useMemo, useRef, type ReactNode } from 'react';
import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { LegendList } from '@legendapp/list/react-native';
import { UserBubble } from './UserBubble';
import { AssistantBubble } from './AssistantBubble';
import { PreviewBubble } from './PreviewBubble';
import { ToolCard } from './tools/ToolCard';
import { ToolGroup } from './tools/ToolGroup';
import { ToolDetailSheet, type ToolDetailHandle } from './tools/ToolDetailSheet';
import { StatusLine } from './StatusLine';
import { ThinkingBlock } from './ThinkingBlock';
import { agruparConversa, entraNoPensamento, type ChatEvent, type ItemConversa } from '@hangar/core';
import { useAparencia } from '../stores/aparencia';
import type { PendingMsg } from './pending';
import * as m from '../paraglide/messages';

// Lista de bolhas do chat. A janela de render da PWA (WINDOW=120 eventos montados) aqui é
// a virtualização nativa do LegendList: ele só monta o visível + buffer, então o store
// pode manter os events completos e a lista continua barata — mesmo objetivo, menos código.
//
// onStartReached dispara loadOlder (busca o histórico anterior sob demanda); os avisos
// olderFailed ficam no rodapé da lista (réguas: falha não some calada).

interface Props {
  events: ChatEvent[];
  preview: string;
  previewMd?: boolean; // prévia do agente = markdown (default true = comportamento antigo)
  previewFull?: boolean;
  statusLine: string | null;
  olderFailed: '' | 'failed' | 'unjoinable';
  onLoadOlder: () => void;
  pending?: PendingMsg[];
  optionsSlot?: ReactNode;
  sessionName?: string;
  serverId?: string;
}

// Bolha sem texto não vira item nenhum. O tool_result é descartado pelo agruparConversa (entra no
// card do tool_use pareado), então aqui sobra só o vazio.
function visivel(ev: ChatEvent): boolean {
  if (ev.kind === 'user_msg' || ev.kind === 'assistant_msg') return !!ev.text;
  return true;
}

export function MessageList({
  events,
  preview,
  previewMd = true,
  previewFull = false,
  statusLine,
  olderFailed,
  onLoadOlder,
  pending = [],
  optionsSlot,
  sessionName,
  serverId,
}: Props) {
  const detail = useRef<ToolDetailHandle>(null);
  const pref = useAparencia((s) => s.pensamentoTools);
  const results = useMemo(() => {
    const mapa = new Map<string, ChatEvent>();
    for (const e of events) if (e.kind === 'tool_result' && e.tool_use_id) mapa.set(e.tool_use_id, e);
    return mapa;
  }, [events]);
  const resultDe = (t: ChatEvent) => results.get(t.tool_use_id ?? '') ?? null;
  const data = useMemo(
    () => agruparConversa(events.filter(visivel), { entraNoPensamento: (n) => entraNoPensamento(pref, n) }),
    [events, pref],
  );

  const renderItem = ({ item }: { item: ItemConversa }) => {
    switch (item.type) {
      case 'group':
        return <ToolGroup tools={item.tools} resultOf={resultDe} onAbrir={(u) => detail.current?.abrir(u)} />;
      case 'tool':
        return <ToolCard use={item.ev} result={resultDe(item.ev)} onPress={() => detail.current?.abrir(item.ev)} />;
      case 'pensamento':
        return <ThinkingBlock eventos={item.eventos} />;
      case 'event': {
        const ev = item.ev;
        if (ev.kind === 'user_msg') {
          // fila durável (queued-*) aparece translúcida igual ao eco local até o real chegar
          if (ev.id.startsWith('queued-')) {
            return (
              <View style={styles.pending}>
                <UserBubble text={ev.text ?? ''} sessionName={sessionName} ts={ev.ts} />
              </View>
            );
          }
          return <UserBubble text={ev.text ?? ''} sessionName={sessionName} ts={ev.ts} />;
        }
        if (ev.kind === 'assistant_msg') {
          return <AssistantBubble text={ev.text ?? ''} sessionName={sessionName} serverId={serverId} ts={ev.ts} />;
        }
        return null;
      }
      default: {
        const nunca: never = item;
        return nunca;
      }
    }
  };

  return (
    <>
    <LegendList
      data={data}
      keyExtractor={(i) => i.id}
      renderItem={renderItem}
      recycleItems={false}
      estimatedItemSize={72}
      alignItemsAtEnd
      initialScrollAtEnd
      maintainScrollAtEnd
      maintainVisibleContentPosition
      onStartReached={onLoadOlder}
      onStartReachedThreshold={1}
      contentContainerStyle={styles.content}
      ListFooterComponent={
        <View style={styles.footer}>
          <StatusLine line={statusLine} />
          {olderFailed === 'failed' ? (
            <Text
              style={styles.gap}
              onPress={onLoadOlder}
              accessibilityRole="button"
            >
              {m.chat_historico_antigo()}
            </Text>
          ) : olderFailed === 'unjoinable' ? (
            <Text style={styles.gap}>{m.chat_sem_historico_anterior()}</Text>
          ) : null}
          {pending.map((p) => (
            <View key={p.id} style={[styles.pending, p.solid && styles.pendingSolid]}>
              <UserBubble text={p.text} sessionName={sessionName} />
            </View>
          ))}
          {optionsSlot ?? null}
          {preview ? <PreviewBubble text={preview} md={previewMd} full={previewFull} /> : null}
        </View>
      }
      accessibilityLabel={m.msg_aria_mensagens()}
    />
    <ToolDetailSheet ref={detail} resultOf={resultDe} />
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  content: {
    padding: theme.base.space[3],
    gap: theme.base.space[2],
  },
  footer: {
    gap: theme.base.space[2],
  },
  gap: {
    fontSize: theme.base.text.xs,
    color: theme.tokens.status.warning,
    textAlign: 'center',
    paddingVertical: theme.base.space[1],
    minHeight: 44,
    lineHeight: 44,
  },
  pending: {
    opacity: 0.5,
  },
  pendingSolid: {
    opacity: 1,
  },
}));
