// Commit do checkout que ESTE shell carregou. A janela nativa roda o main.cjs que estava no disco
// quando abriu; a tela de atualização compara este valor com o commit da última atualização pra
// saber se o "feche e abra o Hangar" ainda vale — sem isso o aviso ficava até a próxima atualização
// sobrescrever o estado, mesmo com o app já reaberto. Lê o .git direto (sem spawnar git): o shell
// sobe antes de tudo e não pode depender do PATH do .desktop.
const fs = require('fs');
const path = require('path');

function commitDoCheckout(raizRepo) {
  try {
    const git = path.join(raizRepo, '.git');
    const head = fs.readFileSync(path.join(git, 'HEAD'), 'utf8').trim();
    const m = /^ref:\s*(\S+)/.exec(head);
    if (!m) return /^[0-9a-f]{40}$/.test(head) ? head : null;   // HEAD solto (checkout de commit)
    const ref = m[1];
    const solto = path.join(git, ...ref.split('/'));
    if (fs.existsSync(solto)) return fs.readFileSync(solto, 'utf8').trim() || null;
    // Ref empacotada (`git gc`): uma linha "<hash> <ref>" em packed-refs.
    const packed = fs.readFileSync(path.join(git, 'packed-refs'), 'utf8');
    for (const linha of packed.split('\n')) {
      const [hash, nome] = linha.trim().split(/\s+/);
      if (nome === ref && /^[0-9a-f]{40}$/.test(hash)) return hash;
    }
    return null;
  } catch {
    return null;
  }
}

module.exports = { commitDoCheckout };
