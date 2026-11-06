import { fileURLToPath } from 'node:url';
import { getPrebuildConfigAsync } from '@expo/prebuild-config';
import { getConfig } from 'expo/config';
import { AndroidConfig, IOSConfig, compileModsAsync } from 'expo/config-plugins';
import { beforeAll, describe, expect, it } from 'vitest';

const projectRoot = fileURLToPath(new URL('..', import.meta.url));
const { exp: config } = getConfig(projectRoot);
const updateUrl = 'https://u.expo.dev/9fd06001-323e-4b82-835f-6dfc3674236d';

describe('OTA na configuração nativa', () => {
  it('gera o IPA local no canal preview com runtime nativo explícito e sem esperar a rede', async () => {
    const plist = await IOSConfig.Updates.setUpdatesConfigAsync(projectRoot, config, {}, null);

    expect(plist).toMatchObject({
      EXUpdatesEnabled: true,
      EXUpdatesURL: updateUrl,
      EXUpdatesRequestHeaders: { 'expo-channel-name': 'preview' },
      EXUpdatesRuntimeVersion: 'native-5',
      EXUpdatesCheckOnLaunch: 'ALWAYS',
      EXUpdatesLaunchWaitMs: 0,
    });
    expect(plist.EXUpdatesHasEmbeddedUpdate).not.toBe(false);
  });

  it('preserva o runtime quando só a versão visível do app muda', async () => {
    const nextConfig = { ...config, version: '0.2.0' };
    const plist = await IOSConfig.Updates.setUpdatesConfigAsync(projectRoot, nextConfig, {}, null);
    expect(plist.EXUpdatesRuntimeVersion).toBe('native-5');
  });

  it('muda o runtime quando a versão nativa explícita sobe', async () => {
    const nextConfig = { ...config, runtimeVersion: 'native-6' };
    const plist = await IOSConfig.Updates.setUpdatesConfigAsync(projectRoot, nextConfig, {}, null);
    expect(plist.EXUpdatesRuntimeVersion).toBe('native-6');
  });

  it('gera o Android com a mesma busca automática e o runtime nativo explícito', async () => {
    const manifest = await AndroidConfig.Updates.setUpdatesConfigAsync(projectRoot, config, {
      manifest: {
        $: { 'xmlns:android': 'http://schemas.android.com/apk/res/android' },
        queries: [],
        application: [{ $: { 'android:name': '.MainApplication' } }],
      },
    }, null);
    const metadata = Object.fromEntries(
      manifest.manifest.application![0]['meta-data']!.map(({ $: entry }) => [entry['android:name'], entry['android:value']]),
    );
    const strings = await AndroidConfig.Updates.applyRuntimeVersionFromConfigForProjectRootAsync(projectRoot, config, { resources: {} });

    expect(metadata).toMatchObject({
      'expo.modules.updates.ENABLED': 'true',
      'expo.modules.updates.EXPO_UPDATE_URL': updateUrl,
      'expo.modules.updates.UPDATES_CONFIGURATION_REQUEST_HEADERS_KEY': '{"expo-channel-name":"preview"}',
      'expo.modules.updates.EXPO_UPDATES_CHECK_ON_LAUNCH': 'ALWAYS',
      'expo.modules.updates.EXPO_UPDATES_LAUNCH_WAIT_MS': '0',
    });
    expect(strings.resources.string).toContainEqual({ $: { name: 'expo_runtime_version' }, _: 'native-5' });
  });
});

// O introspect roda os config plugins sem prebuild, como o `expo config --type introspect`.
async function generatedNative() {
  const { exp } = await getPrebuildConfigAsync(projectRoot, { platforms: ['ios', 'android'] });
  const out = await compileModsAsync(exp, {
    projectRoot,
    introspect: true,
    platforms: ['ios', 'android'],
    assertMissingModProviders: false,
  });
  // Campo interno do Expo, o mesmo que o `expo config --type introspect` lê.
  const results = (out as any)._internal.modResults;
  return { plist: results.ios.infoPlist, entitlements: results.ios.entitlements, manifest: results.android.manifest };
}

const pluginOptions = (name: string) => {
  const entry = config.plugins?.find((p) => (Array.isArray(p) ? p[0] : p) === name);
  return Array.isArray(entry) ? entry[1] : entry;
};

describe('native-4: tablet, ditado em segundo plano e push', () => {
  let native: Awaited<ReturnType<typeof generatedNative>>;
  beforeAll(async () => { native = await generatedNative(); }, 60_000);

  it('iPhone só em retrato e iPad nas quatro orientações', () => {
    const { plist } = native;
    expect(config.orientation).toBe('default');
    expect(config.ios?.supportsTablet).toBe(true);
    expect(plist.UISupportedInterfaceOrientations).toEqual(['UIInterfaceOrientationPortrait']);
    expect(plist['UISupportedInterfaceOrientations~ipad']).toEqual([
      'UIInterfaceOrientationPortrait',
      'UIInterfaceOrientationPortraitUpsideDown',
      'UIInterfaceOrientationLandscapeLeft',
      'UIInterfaceOrientationLandscapeRight',
    ]);
  });

  it('Android nasce sem trava nativa para permitir que o tablet gire', () => {
    const activity = AndroidConfig.Manifest.getMainActivityOrThrow(native.manifest);
    expect(activity.$['android:screenOrientation']).toBe('unspecified');
  });

  it('Android grava em segundo plano pelo serviço do expo-audio', () => {
    expect(pluginOptions('expo-audio')).toMatchObject({ enableBackgroundRecording: true });
    const { manifest } = native;
    const services = manifest.manifest.application[0].service ?? [];
    expect(services).toContainEqual(expect.objectContaining({
      $: expect.objectContaining({
        'android:name': 'expo.modules.audio.service.AudioRecordingService',
        'android:foregroundServiceType': 'microphone',
      }),
    }));
    const permissions = manifest.manifest['uses-permission'].map((p: any) => p.$['android:name']);
    expect(permissions).toContain('android.permission.FOREGROUND_SERVICE_MICROPHONE');
  });

  it('traz o plugin do expo-notifications', () => {
    expect(pluginOptions('expo-notifications')).toBe('expo-notifications');
  });

  it('iOS sai sem o entitlement de push, que o SideStore com Apple ID gratuito não assina', () => {
    const { entitlements } = native;
    expect(entitlements).toBeDefined();
    expect(entitlements).not.toHaveProperty('aps-environment');
  });

  it('lê o google-services.json do ambiente do EAS', () => {
    process.env.GOOGLE_SERVICES_JSON = '/tmp/x.json';
    try {
      expect(getConfig(projectRoot).exp.android?.googleServicesFile).toBe('/tmp/x.json');
    } finally {
      delete process.env.GOOGLE_SERVICES_JSON;
    }
  });
});
