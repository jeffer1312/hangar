import { useMemo } from 'react';
import { Pressable, ScrollView, Text, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from '../../../../src/ui/Icon';
import { WorkReviewSection } from '../../../../src/features/workReview/WorkReviewSection';
import * as m from '../../../../src/paraglide/messages';

/** Folha do celular aberta pelo chip da linha: a mesma revisão da aba Revisão do tablet, para decidir sem abrir a conversa. */
export default function WorkReviewSheet() {
  const params = useLocalSearchParams<{ server: string; name: string }>();
  const serverId = Array.isArray(params.server) ? params.server[0] : (params.server ?? '');
  const name = Array.isArray(params.name) ? params.name[0] : (params.name ?? '');
  const router = useRouter();
  const { theme } = useUnistyles();
  const backgroundColor = theme.tokens.bg.base;
  const screenOptions = useMemo(() => ({ headerShown: false, contentStyle: { backgroundColor } }), [backgroundColor]);
  const close = () => {
    if (router.canGoBack()) router.back();
    else router.replace('/' as never);
  };
  return (
    <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.root}>
      <Stack.Screen options={screenOptions} />
      {/* O formSheet precisa medir o cabeçalho junto da rolagem, sem o cabeçalho nativo. */}
      <View collapsable={false} style={styles.header}>
        <Text accessibilityRole="header" style={styles.title} numberOfLines={1}>{name}</Text>
        <Pressable accessibilityRole="button" accessibilityLabel={m.native_close()} onPress={close} style={styles.close}>
          <Icon name="X" size={20} color={theme.tokens.text.secondary} />
        </Pressable>
      </View>
      <ScrollView contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
        <WorkReviewSection key={`${serverId}:${name}`} serverId={serverId} name={name} standalone />
      </ScrollView>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: { flex: 1 },
  header: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3] },
  title: { flex: 1, ...theme.type.heading, color: theme.tokens.text.primary },
  close: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  content: { paddingHorizontal: theme.base.space[4], paddingBottom: theme.base.space[8] },
}));
