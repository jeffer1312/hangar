// O primeiro binário precisa publicar o mesmo par que o atualizador Rust valida depois.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const NAMES = ['hangar-auth', 'hangar-cano', 'hangar-server'];
const digest = data => crypto.createHash('sha256').update(data).digest('hex');
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const exists = (io, name) => { try { io.lstatSync(name); return true; } catch (e) { if (e.code === 'ENOENT') return false; throw e; } };
const regular = (io, name) => { try { return io.statSync(name).isFile(); } catch (e) { if (e.code === 'ENOENT') return false; throw e; } };
const previous = target => `${target}.prev`;
function plan(manifest, platform, dest) {
  if (!object(manifest.files) || !NAMES.every(name => object(manifest.files[`${platform}/${name}`]))) {
    throw new Error(NAMES.filter(name => !object(manifest.files?.[`${platform}/${name}`])).map(name => `${name}: a release não traz build para ${platform}`).join('\n'));
  }
  const auth = manifest.files[`${platform}/hangar-auth`].sha256;
  const server = manifest.files[`${platform}/hangar-server`].sha256;
  if (typeof auth !== 'string' || !/^[a-f0-9]{64}$/i.test(auth) || typeof server !== 'string') throw new Error('biblioteca de autenticação ausente ou inválida no manifesto');
  const libraryName = platform.startsWith('windows') ? 'hangar_auth.dll' : platform.startsWith('macos') ? 'libhangar_auth.dylib' : 'libhangar_auth.so';
  const relative = `auth/${auth.toLowerCase()}/${libraryName}`;
  const serverPath = path.join(dest, `hangar-server${platform.startsWith('windows') ? '.exe' : ''}`);
  // Separadores e ordem iguais ao json.dumps(sort_keys=True) do contrato capturado.
  const metadata = Buffer.from(`{"abi": 1, "library": ${JSON.stringify(relative)}, "library_sha256": ${JSON.stringify(auth.toLowerCase())}, "server_sha256": ${JSON.stringify(server.toLowerCase())}, "store_version": 1}`);
  return { library: path.join(dest, relative), server: serverPath, metadataPath: path.join(dest, 'hangar-server.auth.json'), metadata };
}
function installFile(data, target, platform, io = fs) {
  const temp = path.join(path.dirname(target), `.${path.basename(target)}.${crypto.randomBytes(8).toString('hex')}.tmp`);
  try {
    io.writeFileSync(temp, data); io.chmodSync(temp, 0o755);
    if (platform.startsWith('windows') && path.extname(target) !== '.json' && exists(io, target)) {
      let old;
      for (let n = 0; n < 100; n++) { const candidate = `${target}.old${n ? `-${n}` : ''}`; if (!exists(io, candidate)) { old = candidate; break; } }
      if (!old) throw new Error(`sem nome livre para ${path.basename(target)}.old`);
      io.renameSync(target, old);
      try { io.renameSync(temp, target); } catch (e) { io.renameSync(old, target); throw e; }
    } else io.renameSync(temp, target);
  } finally { try { io.unlinkSync(temp); } catch (e) { if (e.code !== 'ENOENT') throw e; } }
}
function snapshot(io, targets) { return new Map(targets.map(target => [target, io.existsSync(target) ? io.readFileSync(target) : null])); }
function restoreSnapshot(io, snapshot, platform) {
  const errors = [];
  for (const [target, data] of snapshot) {
    try {
      if (data === null) { try { io.unlinkSync(target); } catch (e) { if (e.code !== 'ENOENT') throw e; } }
      else if (!regular(io, target) || !io.readFileSync(target).equals(data)) installFile(data, target, platform, io);
    } catch (e) { errors.push(`${path.basename(target)}: desfazer falhou (${e.message})`); }
  }
  return errors;
}
function keepPrevious(io, target, name) {
  const prev = previous(target);
  if (!regular(io, target)) {
    try { io.writeFileSync(prev, Buffer.alloc(0)); return null; }
    catch (e) { return `${name}: não consegui guardar a ausência do par anterior (${e.message})`; }
  }
  const temp = `${prev}.${crypto.randomBytes(8).toString('hex')}.tmp`;
  try { io.writeFileSync(temp, io.readFileSync(target)); io.renameSync(temp, prev); return null; }
  catch (e) { try { io.unlinkSync(prev); } catch {} return `${name}: sem cópia do binário anterior; se a atualização voltar, o Rust não volta junto (${e.message})`; }
  finally { try { io.unlinkSync(temp); } catch (e) { if (e.code !== 'ENOENT') throw e; } }
}
function validatePair(server, io = fs) {
  const actualServer = io.realpathSync(server);
  const root = path.dirname(actualServer);
  const manifest = JSON.parse(io.readFileSync(path.join(root, `${path.parse(actualServer).name}.auth.json`)));
  if (!object(manifest) || Object.keys(manifest).sort().join(',') !== 'abi,library,library_sha256,server_sha256,store_version' || manifest.abi !== 1 || manifest.store_version !== 1) throw new Error('auth_pair_invalid');
  const relative = manifest.library;
  const sha = manifest.library_sha256;
  if (typeof relative !== 'string' || typeof sha !== 'string' || !/^[a-f0-9]{64}$/.test(sha) || ![`auth/${sha}/libhangar_auth.so`, `auth/${sha}/libhangar_auth.dylib`, `auth/${sha}/hangar_auth.dll`].includes(relative)) throw new Error('auth_pair_invalid');
  const library = io.realpathSync(path.join(root, relative));
  const local = path.relative(root, library);
  if (local === '..' || local.startsWith(`..${path.sep}`) || path.isAbsolute(local) || !regular(io, library)) throw new Error('auth_pair_invalid');
  if (digest(io.readFileSync(actualServer)) !== manifest.server_sha256 || digest(io.readFileSync(library)) !== sha) throw new Error('auth_pair_hash_mismatch');
}
function publishPair(items, { platform, dest, preservePrevious = false, io = fs }) {
  const server = path.join(dest, `hangar-server${platform.startsWith('windows') ? '.exe' : ''}`);
  const metadata = path.join(dest, 'hangar-server.auth.json');
  let before, prev;
  try { before = snapshot(io, [server, metadata]); prev = snapshot(io, [previous(server), previous(metadata)]); }
  catch (e) { return [`hangar-auth-pair: não consegui preparar a troca (${e.message})`]; }
  if (items.some(item => item.name !== 'hangar-auth')) {
    for (const [target, name] of [[server, 'hangar-server'], [metadata, 'hangar-auth-pair']]) {
      const warning = keepPrevious(io, target, name);
      if (warning) return [warning, ...restoreSnapshot(io, new Map([...prev.keys()].map(p => [p, null])), platform)];
    }
  }
  const undo = () => { const errors = restoreSnapshot(io, before, platform); if (!errors.length) errors.push(...restoreSnapshot(io, new Map([...prev.keys()].map(p => [p, null])), platform)); return errors; };
  const order = {'hangar-auth':0,'hangar-server':1,'hangar-auth-pair':2};
  for (const item of [...items].sort((a,b) => order[a.name] - order[b.name])) {
    try {
      io.mkdirSync(path.dirname(item.target), { recursive:true });
      if (item.name === 'hangar-auth') {
        if (io.existsSync(item.target)) { if (digest(io.readFileSync(item.target)) !== digest(item.data)) throw new Error('auth_library_hash_mismatch'); continue; }
        const temp = `${item.target}.${crypto.randomBytes(8).toString('hex')}.tmp`;
        try {
          io.writeFileSync(temp, item.data);
          try { io.linkSync(temp, item.target); }
          catch (e) { if (e.code !== 'EEXIST') throw e; if (digest(io.readFileSync(item.target)) !== digest(item.data)) throw new Error('auth_library_hash_mismatch'); }
        } finally { try { io.unlinkSync(temp); } catch (e) { if (e.code !== 'ENOENT') throw e; } }
      } else installFile(item.data, item.target, platform, io);
    } catch (e) { return [`${item.name}: não consegui gravar em ${path.dirname(item.target)} (${e.message})`, ...undo()]; }
  }
  try { validatePair(server, io); }
  catch (e) { return [`hangar-auth-pair: o par publicado não confere (${e.message})`, ...undo()]; }
  return preservePrevious ? [] : restoreSnapshot(io, prev, platform);
}
module.exports = { plan, installFile, publishPair, validatePair, digest };
if (require.main === module) {
  const [mode, platform, dest, manifestPath, stage, name] = process.argv.slice(2);
  try {
    const manifest = JSON.parse(fs.readFileSync(manifestPath)); const prepared = plan(manifest, platform, dest);
    if (mode === 'check') process.exit(0);
    if (mode === 'installed') {
      const names = (name || '').split(', ').filter(Boolean).filter(name => {
        const target = name === 'hangar-auth' ? prepared.library : path.join(dest, `${name}${platform.startsWith('windows') ? '.exe' : ''}`);
        try { return digest(fs.readFileSync(target)) === manifest.files[`${platform}/${name}`].sha256.toLowerCase(); } catch { return false; }
      });
      process.stdout.write(names.join(', ')); process.exit(0);
    }
    if (mode === 'target') { process.stdout.write(name === 'hangar-auth' ? prepared.library : path.join(dest, `${name}${platform.startsWith('windows') ? '.exe' : ''}`)); process.exit(0); }
    if (mode !== 'publish') throw new Error('operação inválida');
    const items = [];
    for (const [name, target] of [['hangar-auth', prepared.library], ['hangar-server', prepared.server]]) {
      const file = path.join(stage, name); if (fs.existsSync(file)) items.push({name,target,data:fs.readFileSync(file)});
    }
    if (!fs.existsSync(prepared.metadataPath) || !fs.readFileSync(prepared.metadataPath).equals(prepared.metadata)) items.push({name:'hangar-auth-pair',target:prepared.metadataPath,data:prepared.metadata});
    const errors = publishPair(items,{platform,dest}); if (errors.length) { console.log(errors.join('\n')); process.exit(1); }
  } catch (e) { console.log(e.message); process.exit(1); }
}
