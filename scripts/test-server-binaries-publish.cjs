// Publicação real em disco; os resultados vêm da execução isolada do Python original.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const assert = require('node:assert/strict');
const {plan,publishPair,digest,installFile,validatePair} = require('./server-binaries-publish.cjs');
const fixture = require('../backend/tests/fixtures/contract/update-git/' + ((process.platform==='win32'||process.argv.includes('--windows-contract')) ? 'auth-release-windows.json' : 'auth-release.json'));
const platform=fixture.platform, ext=platform.startsWith('windows')?'.exe':'';
const root = fs.mkdtempSync(path.join(os.tmpdir(),'hangar-bootstrap-pair-'));
const snapshot = dir => Object.fromEntries(fs.readdirSync(dir,{recursive:true}).filter(name=>fs.statSync(path.join(dir,name)).isFile()).map(name=>[name.split(path.sep).join('/'),fs.readFileSync(path.join(dir,name),'utf8')]));
let count=0;
try {
  for (const name of ['new','upgrade','same','metadata-repair','server-publish-failure','metadata-publish-failure','auth-existing-corrupt']) {
    const row=fixture.cases.find(row=>row.name===name), dest=path.join(root,name); fs.mkdirSync(dest);
    for(const [file,data] of Object.entries(row.initial)){fs.mkdirSync(path.dirname(path.join(dest,file)),{recursive:true});fs.writeFileSync(path.join(dest,file),data);}
    const files=Object.fromEntries(['hangar-auth','hangar-cano','hangar-server'].map(n=>[`${platform}/${n}`,{name:n,sha256:digest(Buffer.from(`new-${n}`))}]));
    const prepared=plan({files},platform,dest),items=[];
    for(const [n,target] of [['hangar-auth',prepared.library],['hangar-server',prepared.server]]) {
      const data=Buffer.from(`new-${n}`);if(!fs.existsSync(target)||!fs.readFileSync(target).equals(data))items.push({name:n,target,data});
    }
    if(!fs.existsSync(prepared.metadataPath)||!fs.readFileSync(prepared.metadataPath).equals(prepared.metadata))items.push({name:'hangar-auth-pair',target:prepared.metadataPath,data:prepared.metadata});
    const cano=path.join(dest,'hangar-cano'+ext);
    if(!fs.existsSync(cano)||fs.readFileSync(cano,'utf8')!=='new-hangar-cano')installFile(Buffer.from('new-hangar-cano'),cano,platform);
    let failed=false;
    const io=Object.create(fs);io.renameSync=(src,target)=>{
      if(!failed&&((name==='server-publish-failure'&&target===prepared.server)||(name==='metadata-publish-failure'&&target===prepared.metadataPath))){failed=true;throw new Error('fixture-publication-failed');}return fs.renameSync(src,target);
    };
    const errors=items.length?publishPair(items,{platform,dest,io}):[];
    assert.deepEqual(errors.map(text=>text.replaceAll(dest,'<DEST>')),row.result,name+' avisos');
    assert.deepEqual(snapshot(dest),row.after_publish,name+' arquivos');
    if(!errors.length)validatePair(prepared.server);
    count++;
  }
  for(const symlink of process.platform==='win32'?[]:[false,true]){
    const dir=path.join(root,`literal-${symlink}`);fs.mkdirSync(dir);const target=path.join(root,`target-${symlink}`);
    if(symlink)fs.symlinkSync(dir,target,'dir');else fs.mkdirSync(target);
    if(symlink){installFile(Buffer.from('replacement'),target,'linux-x86_64');assert.equal(fs.readFileSync(target,'utf8'),'replacement');}
    else assert.throws(()=>installFile(Buffer.from('replacement'),target,'linux-x86_64'));
    assert.deepEqual(fs.readdirSync(dir),[]);count++;
  }
  for (const competing of ['equal','corrupt','denied']) {
    const dest=path.join(root,`hardlink-${competing}`);fs.mkdirSync(dest);
    const files=Object.fromEntries(['hangar-auth','hangar-cano','hangar-server'].map(n=>[`${platform}/${n}`,{name:n,sha256:digest(Buffer.from(`new-${n}`))}]));
    const prepared=plan({files},platform,dest),io=Object.create(fs);let inode;
    io.linkSync=(temp,target)=>{
      if(competing==='denied'){const error=new Error('fixture-hardlink-denied');error.code='EPERM';throw error;}
      fs.writeFileSync(target,competing==='equal'?'new-hangar-auth':'corrupt');inode=fs.statSync(target).ino;
      const error=new Error('fixture-existing');error.code='EEXIST';throw error;
    };
    const errors=publishPair([{name:'hangar-auth',target:prepared.library,data:Buffer.from('new-hangar-auth')},{name:'hangar-server',target:prepared.server,data:Buffer.from('new-hangar-server')},{name:'hangar-auth-pair',target:prepared.metadataPath,data:prepared.metadata}],{platform,dest,io});
    assert.equal(errors.length,competing==='equal'?0:1);
    if(competing==='equal')validatePair(prepared.server);
    else {assert.equal(fs.existsSync(prepared.server),false);assert.equal(fs.existsSync(prepared.metadataPath),false);}
    if(competing!=='denied')assert.equal(fs.statSync(prepared.library).ino,inode);
    assert.equal(fs.readdirSync(path.dirname(prepared.library)).some(n=>n.endsWith('.tmp')),false);count++;
  }
  if(process.platform!=='win32') {
    const mode=require('../backend/tests/fixtures/contract/update-git/backup-mode.json');
    const dest=path.join(root,'previous-mode');fs.mkdirSync(dest);
    const files=Object.fromEntries(['hangar-auth','hangar-cano','hangar-server'].map(n=>[`${platform}/${n}`,{name:n,sha256:digest(Buffer.from(`new-${n}`))}]));
    const prepared=plan({files},platform,dest);
    fs.writeFileSync(prepared.server,'old');fs.chmodSync(prepared.server,mode.source_mode);
    fs.writeFileSync(prepared.metadataPath,'{}');
    const oldMask=process.umask(mode.umask);
    try {
      assert.deepEqual(publishPair([{name:'hangar-auth',target:prepared.library,data:Buffer.from('new-hangar-auth')},{name:'hangar-server',target:prepared.server,data:Buffer.from('new-hangar-server')},{name:'hangar-auth-pair',target:prepared.metadataPath,data:prepared.metadata}],{platform,dest,preservePrevious:true}),[]);
      assert.equal(fs.statSync(prepared.server+'.prev').mode&0o777,mode.previous_mode);count++;
    }finally{process.umask(oldMask);}
  }
  console.log(`${count} casos de publicação conferidos`);
}finally{fs.rmSync(root,{recursive:true,force:true});}
