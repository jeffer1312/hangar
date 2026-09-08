// @ts-check
const path = require('node:path');
const { getDefaultConfig } = require('expo/metro-config');

const raiz = path.resolve(__dirname, '..');

/** @type {import('expo/metro-config').MetroConfig} */
const config = getDefaultConfig(__dirname);

// O app NÃO é workspace da raiz (quem instala o Hangar não pode baixar o toolchain do RN), então a
// detecção automática de monorepo do Expo não vale aqui: sem estas duas linhas o Metro não vigia
// nem resolve o `@hangar/core`, que mora fora de mobile/ e entra por `file:`.
config.watchFolders = [path.resolve(raiz, 'packages/core')];
config.resolver.nodeModulesPaths = [
  path.resolve(__dirname, 'node_modules'),
  path.resolve(raiz, 'node_modules'),
];

module.exports = config;
