// Publicação real em disco, numa pasta temporária.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const assert = require('node:assert/strict');
const {plan,publishPair,digest,installFile,validatePair} = require('./server-binaries-publish.cjs');
const platform = (process.platform==='win32'||process.argv.includes('--windows-contract')) ? 'windows-x86_64' : 'linux-x86_64';
const root = fs.mkdtempSync(path.join(os.tmpdir(),'hangar-bootstrap-pair-'));
let count=0;
try {
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
  console.log(`${count} casos de publicação conferidos`);
}finally{fs.rmSync(root,{recursive:true,force:true});}
