import { defineConfig } from 'vitest/config';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
require('./scripts/build-terminal-document.cjs').generateTerminalDocument();

export default defineConfig({
  resolve: {
    alias: {
      // A instalação seletiva da OTA e a instalação local precisam da mesma instância do React.
      react: path.dirname(require.resolve('react/package.json')),
      'react-dom': path.dirname(require.resolve('react-dom/package.json')),
      'react-native': path.resolve(__dirname, 'src/__mocks__/react-native.ts'),
      'react-native-unistyles': path.resolve(__dirname, 'src/__mocks__/react-native-unistyles.ts'),
      'react-native-webview': path.resolve(__dirname, 'src/__mocks__/react-native-webview.ts'),
      'expo-image': path.resolve(__dirname, 'src/__mocks__/expo-image.ts'),
      'expo-screen-orientation': path.resolve(__dirname, 'src/__mocks__/expo-screen-orientation.ts'),
      'react-native-reanimated': path.resolve(__dirname, 'src/__mocks__/react-native-reanimated.ts'),
      'react-native-worklets': path.resolve(__dirname, 'src/__mocks__/react-native-worklets.ts'),
      'react-native-svg': path.resolve(__dirname, 'src/__mocks__/react-native-svg.ts'),
      'expo-haptics': path.resolve(__dirname, 'src/__mocks__/expo-haptics.ts'),
      'expo-symbols': path.resolve(__dirname, 'src/__mocks__/expo-symbols.ts'),
      'react-native-safe-area-context': path.resolve(__dirname, 'src/__mocks__/react-native-safe-area-context.ts'),
      // Antes do pacote inteiro: o alias casa por prefixo.
      'react-native-gesture-handler/ReanimatedDrawerLayout': path.resolve(__dirname, 'src/__mocks__/react-native-gesture-handler-drawer.ts'),
      'react-native-gesture-handler': path.resolve(__dirname, 'src/__mocks__/react-native-gesture-handler.ts'),
    },
  },
  test: {
    // Externo, o lucide carregaria o react-native de verdade (Flow) e não passaria pelos aliases acima.
    server: { deps: { inline: ['lucide-react-native'] } },
    // Um fork por núcleo (o padrão do vitest) com mais de uma suíte rodando ao mesmo tempo
    // enche a RAM e joga a máquina em swap. Dois bastam pro tamanho desta suíte.
    maxWorkers: 2,
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    globals: true,
  },
});
