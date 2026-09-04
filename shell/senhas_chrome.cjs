// Senhas salvas do Chrome do usuário -> preenchimento no navegador embutido. Lê o SQLite
// `Login Data` (cópia, porque o Chrome mantém o arquivo travado) e decifra o campo `password_value`.
//
// Cifra no Linux (v10/v11): AES-128-CBC, IV de 16 espaços, chave = PBKDF2(senha, "saltysalt", 1
// iteração, 16 bytes, sha1). A "senha" é ou o literal "peanuts" (armazenamento básico) ou a chave
// que o Chrome guarda no chaveiro ("Chrome Safe Storage") — perfis diferem, então tenta as duas e
// fica com a que decifra pra texto limpo. Sem `sqlite3` de dependência: usa o binário do sistema.
const crypto = require('crypto');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

function chavesCandidatas() {
  const chaves = [Buffer.from('peanuts')];
  for (const rotulo of ['Chrome Safe Storage', 'Chromium Safe Storage']) {
    try {
      const k = execFileSync('secret-tool', ['lookup', 'application', 'chrome'], { encoding: 'utf8', timeout: 3000 });
      if (k) { chaves.push(Buffer.from(k)); break; }
    } catch { /* sem secret-tool ou sem entrada */ }
    void rotulo;
  }
  // dedup por conteúdo
  const vistos = new Set();
  return chaves.filter((c) => { const h = c.toString('base64'); if (vistos.has(h)) return false; vistos.add(h); return true; });
}

function decifrar(buf, chaveSenha) {
  if (!buf || buf.length < 3) return null;
  const pref = buf.subarray(0, 3).toString();
  if (pref !== 'v10' && pref !== 'v11') return null;   // texto puro antigo não é o caso no Linux atual
  const chave = crypto.pbkdf2Sync(chaveSenha, 'saltysalt', 1, 16, 'sha1');
  try {
    const d = crypto.createDecipheriv('aes-128-cbc', chave, Buffer.alloc(16, ' '));
    let out = Buffer.concat([d.update(buf.subarray(3)), d.final()]);
    // Chrome ≥ v10 no Linux não usa o prefixo de 32 bytes do mac/win; se vier lixo de controle
    // no começo, não é esta chave.
    const s = out.toString('utf8');
    return /^[\x20-\x7e -￿]*$/.test(s.replace(/\s/g, '')) ? s : null;
  } catch { return null; }
}

// Perfis de Login Data a ler (o padrão e o "For Account", que às vezes tem as senhas de trabalho).
function bancos() {
  const base = process.platform === 'darwin'
    ? path.join(os.homedir(), 'Library', 'Application Support', 'Google', 'Chrome')
    : path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config'), 'google-chrome');
  return ['Login Data', 'Login Data For Account'].map((n) => path.join(base, 'Default', n)).filter(fs.existsSync);
}

// (host sem www) -> [{usuario, senha, origem}]. Uma cópia por banco porque o Chrome trava o arquivo.
function credenciaisPara(host) {
  const alvo = String(host || '').replace(/^www\./, '').toLowerCase();
  if (!alvo) return [];
  const chaves = chavesCandidatas();
  const out = [];
  for (const db of bancos()) {
    const tmp = path.join(os.tmpdir(), `hangar-ld-${process.pid}-${crypto.randomBytes(4).toString('hex')}.db`);
    try {
      fs.copyFileSync(db, tmp);
      const linhas = execFileSync('sqlite3', ['-newline', '\x1e', '-separator', '\x1f', tmp,
        "select origin_url, username_value, hex(password_value) from logins where blacklisted_by_user=0 and length(password_value)>0"],
        { encoding: 'utf8', timeout: 5000, maxBuffer: 16 * 1024 * 1024 });
      for (const linha of linhas.split('\x1e')) {
        const [url, usuario, hex] = linha.split('\x1f');
        if (!url) continue;
        let hostDaSenha = '';
        try { hostDaSenha = new URL(url).hostname.replace(/^www\./, '').toLowerCase(); } catch { continue; }
        // Casa o domínio: mesma host ou subdomínio (auth.exemplo.com pra exemplo.com).
        if (hostDaSenha !== alvo && !alvo.endsWith('.' + hostDaSenha) && !hostDaSenha.endsWith('.' + alvo)) continue;
        const buf = Buffer.from(hex, 'hex');
        let senha = null;
        for (const ch of chaves) { senha = decifrar(buf, ch); if (senha !== null) break; }
        if (senha) out.push({ usuario: usuario || '', senha, origem: url });
      }
    } catch { /* banco travado/ausente: pula */ }
    finally { try { fs.rmSync(tmp, { force: true }); } catch { /* já foi */ } }
  }
  return out;
}

module.exports = { credenciaisPara, decifrar, chavesCandidatas };
