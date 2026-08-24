import { defineConfig } from 'vitest/config';
import path from 'node:path';

export default defineConfig({
  resolve: {
    alias: {
      '@/constants/Typography': path.resolve(__dirname, 'src/vendor/happy/constants/Typography.ts'),
      '@': path.resolve(__dirname, 'src'),
      react: path.resolve(__dirname, '../node_modules/react'),
      'react-dom': path.resolve(__dirname, '../node_modules/react-dom'),
      'react-native': path.resolve(__dirname, 'src/__mocks__/react-native.ts'),
      'react-native-unistyles': path.resolve(__dirname, 'src/__mocks__/react-native-unistyles.ts'),
      'react-native-webview': path.resolve(__dirname, 'src/__mocks__/react-native-webview.ts'),
      'expo-image': path.resolve(__dirname, 'src/__mocks__/expo-image.ts'),
    },
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    globals: true,
  },
});
