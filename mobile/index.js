require('./src/net/appStart');
require('expo');
const { Platform } = require('react-native');
const Constants = require('expo-constants').default;
const Updates = require('expo-updates');
const { installFatalErrorCapture } = require('./src/net/fatalErrors');

installFatalErrorCapture({
  errorUtils: globalThis.ErrorUtils,
  exceptions: require('react-native/Libraries/Core/ExceptionsManager').default,
  registerExceptionListener: globalThis.RN$registerExceptionListener,
  metadata: {
    platform: Platform.OS,
    version: Constants.expoConfig?.version,
    runtime: typeof Constants.expoConfig?.runtimeVersion === 'string' ? Constants.expoConfig.runtimeVersion : undefined,
    updateId: Updates.updateId ?? undefined,
    embedded: Updates.isEmbeddedLaunch,
  },
});

// O Router carrega as telas; a captura precisa estar pronta antes de qualquer uma delas.
require('expo-router/entry');
