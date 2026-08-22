import { View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { ToolView } from '@/components/tools/ToolView';
import type { ToolCall } from '@/sync/typesMessage';

// Card de ferramenta: o ToolView do vendor Happy (Bash, Edit com diff, etc) já estilizado
// pelo tema via mapHappy. metadata null = sem flavor especial (Claude).
export function ToolBubble({ tool }: { tool: ToolCall }) {
  return (
    <View style={styles.wrap}>
      <ToolView tool={tool} metadata={null} />
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    alignSelf: 'stretch',
    borderRadius: theme.base.radius.md,
    overflow: 'hidden',
  },
}));
