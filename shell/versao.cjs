// Commit do checkout que ESTE shell carregou. A janela nativa roda o main.cjs que estava no disco
// quando abriu; a tela de atualização compara este valor com o commit da última atualização pra
// saber se o "feche e abra o Hangar" ainda vale — sem isso o aviso ficava até a próxima atualização
// sobrescrever o estado, mesmo com o app já reaberto. Lê o .git direto (sem spawnar git): o shell
// sobe antes de tudo e não pode depender do PATH do .desktop.
const fs = require('fs');
const path = require('path');

function commitDoCheckout(raizRepo) {
  try {
    let git = path.join(raizRepo, '.git');
    // Worktree (`git worktree add`): `.git` é ARQUIVO com `gitdir: <pasta real>`. Sem seguir o
    // ponteiro, o shell aberto de uma worktree ficava sem commit e o aviso nunca sumia.
    if (fs.statSync(git).isFile()) {
      const m = /^gitdir:\s*(.+)$/m.exec(fs.readFileSync(git, 'utf8'));
      if (!m) return null;
      git = path.resolve(raizRepo, m[1].trim());
    }
    let head = fs.readFileSync(path.join(git, 'HEAD'), 'utf8').trim();
    // Na worktree o HEAD mora na pasta dela, mas as refs ficam no repo principal (`commondir`).
    let comum = git;
    try {
      const c = fs.readFileSync(path.join(git, 'commondir'), 'utf8').trim();
      if (c) comum = path.resolve(git, c);
    } catch { /* checkout normal: sem commondir */ }
    const m = /^ref:\s*(\S+)/.exec(head);
    if (!m) return /^[0-9a-f]{40}$/.test(head) ? head : null;   // HEAD solto (checkout de commit)
    const ref = m[1];
    const solto = path.join(comum, ...ref.split('/'));
    if (fs.existsSync(solto)) return fs.readFileSync(solto, 'utf8').trim() || null;
    // Ref empacotada (`git gc`): uma linha "<hash> <ref>" em packed-refs.
    const packed = fs.readFileSync(path.join(comum, 'packed-refs'), 'utf8');
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
