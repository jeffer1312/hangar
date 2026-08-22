import { useMemo } from 'react';
import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { LegendList } from '@legendapp/list/react-native';
import { pairTools, toToolCall } from './toolAdapter';
import { UserBubble } from './UserBubble';
import { AssistantBubble } from './AssistantBubble';
import { PreviewBubble } from './PreviewBubble';
import { ToolBubble } from './ToolBubble';
import { StatusLine } from './StatusLine';
import type { ChatEvent } from '@hangar/core';
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
  statusLine: string | null;
  olderFailed: '' | 'failed' | 'unjoinable';
  onLoadOlder: () => void;
}

// O que vira bolha (espelho do MessageList.svelte): tool_result nunca direto — entra no
// card do tool_use pareado; assistant_msg sem texto não renderiza nada.
function visivel(ev: ChatEvent): boolean {
  if (ev.kind === 'tool_result') return false;
  if (ev.kind === 'user_msg') return !!ev.text;
  if (ev.kind === 'assistant_msg') return !!ev.text;
  return true; // tool_use
}

export function MessageList({
  events,
  preview,
  statusLine,
  olderFailed,
  onLoadOlder,
}: Props) {
  const data = useMemo(() => events.filter(visivel), [events]);
  const tools = useMemo(() => pairTools(events), [events]);

  const renderItem = ({ item }: { item: ChatEvent }) => {
    if (item.kind === 'user_msg') return <UserBubble text={item.text ?? ''} />;
    if (item.kind === 'assistant_msg') return <AssistantBubble text={item.text ?? ''} />;
    if (item.kind === 'tool_use') {
      const par = tools.get(item.tool_use_id ?? '');
      return <ToolBubble tool={toToolCall(item, par?.result)} />;
    }
    return null;
  };

  return (
    <LegendList
      data={data}
      keyExtractor={(e) => e.id}
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
          {olderFailed !== '' ? (
            <Text style={styles.gap} onPress={onLoadOlder}>
              {olderFailed === 'unjoinable'
                ? m.chat_sem_historico_anterior()
                : m.chat_historico_antigo()}
            </Text>
          ) : null}
          {preview ? <PreviewBubble text={preview} /> : null}
        </View>
      }
      accessibilityLabel={m.msg_aria_mensagens()}
    />
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
  },
}));
