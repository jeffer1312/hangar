// URL escolhida e geometria da janela, num JSON só dentro do userData do Electron.
// Sem dependência nova (electron-window-state faz isto e mais nada que a gente use).
const fs = require('fs');
const path = require('path');

function arquivo(userDataDir) {
  return path.join(userDataDir, 'settings.json');
}

function ler(userDataDir) {
  try {
    return JSON.parse(fs.readFileSync(arquivo(userDataDir), 'utf8'));
  } catch {
    // Ausente ou ilegível: começa do zero. Não é erro — é a primeira execução.
    return {};
  }
}

function gravar(userDataDir, dados) {
  try {
    fs.mkdirSync(userDataDir, { recursive: true });
    fs.writeFileSync(arquivo(userDataDir), JSON.stringify(dados, null, 2));
  } catch {
    // Disco cheio / permissão: perder a geometria não pode derrubar o app.
  }
}

module.exports = { ler, gravar };
