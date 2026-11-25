const { existsSync, readFileSync, writeFileSync } = require('node:fs');
const path = require('node:path');

// O @expo/ui chama NativeStatePropsGetter.clearAllContentOrigins() ao desmontar a instância. Quando a
// instância cai antes de a biblioteca nativa do expo-modules-core registrar os métodos (o lançador de
// desenvolvimento troca de instância assim), a chamada lança UnsatisfiedLinkError e o app trava cinza.
// Sem métodos registrados não houve origem publicada, então não há o que limpar: a exceção é engolida.
const MARK = '// hangar: patch-expo-ui';
const CALL = '      NativeStatePropsGetter().clearAllContentOrigins()\n';
const GUARDED = `      ${MARK}\n      try {\n        NativeStatePropsGetter().clearAllContentOrigins()\n      } catch (e: UnsatisfiedLinkError) {\n      }\n`;

function patchExpoUi(mobileDir) {
  let pkg;
  try { pkg = require.resolve('@expo/ui/package.json', { paths: [mobileDir] }); } catch { return 'absent'; }
  const file = path.join(path.dirname(pkg), 'android/src/main/java/expo/modules/ui/ExpoUIModule.kt');
  // Pacote instalado sem o arquivo: o caminho mudou, e seguir calado geraria o binário sem a proteção.
  if (!existsSync(file)) throw new Error(`patch-expo-ui: ${file} não existe mais; revise o ajuste.`);
  const source = readFileSync(file, 'utf8');
  if (source.includes(MARK)) return 'already';
  if (!source.includes(CALL)) {
    throw new Error(`patch-expo-ui: ${file} mudou e a chamada a proteger não está mais lá; revise o ajuste.`);
  }
  writeFileSync(file, source.replace(CALL, GUARDED));
  return 'patched';
}

if (require.main === module) console.log(`patch-expo-ui: ${patchExpoUi(path.resolve(__dirname, '..'))}`);

module.exports = { patchExpoUi };
