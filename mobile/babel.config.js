module.exports = (api) => {
  api.cache(true);
  return {
    presets: ['babel-preset-expo'],
    plugins: [
      [
        'module-resolver',
        {
          alias: {
            '@/sync/typesMessage': './src/vendor/happy/shims/typesMessage',
            '@/sync/storage': './src/vendor/happy/shims/storage',
            '@/sync/storageTypes': './src/vendor/happy/shims/storageTypes',
            '@/sync/ops': './src/vendor/happy/shims/ops',
            '@/text': './src/vendor/happy/shims/text',
            '@/sync/suggestionCommands': './src/vendor/happy/sync/suggestionCommands',
            '@/sync/suggestionFile': './src/vendor/happy/sync/suggestionFile',
            '@/hooks/useElapsedTime': './src/vendor/happy/hooks/useElapsedTime',
            '@/hooks/useAttachmentImage': './src/vendor/happy/hooks/useAttachmentImage',
            '@/components/CodeView': './src/vendor/happy/components/CodeView',
            '@/components/CommandView': './src/vendor/happy/components/CommandView',
            '@/components/diff/calculateDiff': './src/vendor/happy/components/diff/calculateDiff',
            '@/components/diff/DiffView': './src/vendor/happy/components/diff/DiffView',
            '@/components/diff/PierreDiffView': './src/vendor/happy/components/diff/PierreDiffView',
            '@/components/tools/knownTools': './src/vendor/happy/components/tools/knownTools',
            '@/components/tools/ToolDiffView': './src/vendor/happy/components/tools/ToolDiffView',
            '@/components/markdown/MarkdownView': './src/vendor/happy/components/markdown/MarkdownView',
            '@/components/AgentInputSuggestionView': './src/vendor/happy/components/AgentInputSuggestionView',
            '@/constants/Typography': './src/vendor/happy/constants/Typography',
            '@/utils/trimIdent': './src/vendor/happy/utils/trimIdent',
            '@/utils/pathUtils': './src/vendor/happy/utils/pathUtils',
            '@/utils/toolErrorParser': './src/vendor/happy/utils/toolErrorParser',
            '@/utils/toolCommand': './src/vendor/happy/utils/toolCommand',
            '@/utils/codexUnifiedDiff': './src/vendor/happy/utils/codexUnifiedDiff',
            '@/utils/responsive': './src/vendor/happy/utils/responsive',
            '@/utils/thumbhash': './src/vendor/happy/utils/thumbhash',
            '@/utils/toolDisplay': './src/vendor/happy/utils/toolDisplay',
            '@/utils/sync': './src/vendor/happy/utils/sync',
            '@/utils/time': './src/vendor/happy/utils/time',
            '@/utils/truncateForLogs': './src/vendor/happy/utils/truncateForLogs',
            '@/utils/platform': './src/vendor/happy/utils/platform',
            '@': './src/vendor/happy',
          },
        },
      ],
      ['react-native-unistyles/plugin', { root: 'src' }],
      'react-native-worklets/plugin',
    ],
  };
};
