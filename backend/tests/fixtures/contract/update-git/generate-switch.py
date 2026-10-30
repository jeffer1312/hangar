"""Captura troca de branch, resgate local, upstream e marcador em Git isolado."""
import argparse,json,os,re,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument("--reference",type=Path,required=True);p.add_argument("--output",type=Path,required=True);a=p.parse_args();ref,out=a.reference.resolve(),a.output.resolve();commit=subprocess.check_output(["git","-C",str(ref),"rev-parse","HEAD"],text=True).strip()
with tempfile.TemporaryDirectory(prefix="hangar-switch-contract-") as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,GIT_CONFIG_NOSYSTEM="1",GIT_CONFIG_GLOBAL=os.devnull,GIT_AUTHOR_NAME="Fixture",GIT_AUTHOR_EMAIL="fixture@example.invalid",GIT_COMMITTER_NAME="Fixture",GIT_COMMITTER_EMAIL="fixture@example.invalid")
 os.chdir(root);sys.path.insert(0,str(ref/"backend"));from app import atualizar as app
 original=app._git;rows=[]
 for name in ["new","new-selected","existing","local-ahead","collision","stash-failed","retry-failed","rescue-failed","upstream-failed","marker-failed","return-main","return-master"]:
  repo=root/name;repo.mkdir();state=root/(name+"-state");state.mkdir();app.REPO=repo;app._base=lambda:state
  def git(*argv):return subprocess.run(["git","-C",str(repo),*argv],text=True,capture_output=True)
  def checked(*argv):
   result=git(*argv);assert result.returncode==0,result.stderr;return result.stdout.strip()
  checked("init","-b","main");(repo/"content").write_text("old\n");checked("add",".");checked("commit","-m","old");old=checked("rev-parse","HEAD")
  checked("config","remote.origin.url",str(repo));checked("config","remote.origin.fetch","+refs/heads/*:refs/remotes/origin/*");checked("update-ref","refs/remotes/origin/main",old);checked("update-ref","refs/remotes/origin/master",old)
  checked("checkout","-b","remote-build");(repo/"beta.txt").write_text("remote\n");checked("add",".");checked("commit","-m","remote");remote=checked("rev-parse","HEAD");checked("update-ref","refs/remotes/origin/beta",remote);checked("checkout","main")
  local=None
  if name in ["existing","local-ahead","rescue-failed","return-main","return-master"]:
   checked("checkout","-b","beta",old)
   if name in ["local-ahead","rescue-failed"]:
    (repo/"local.txt").write_text("local\n");checked("add",".");checked("commit","-m","local");local=checked("rev-parse","HEAD")
   if not name.startswith("return-"):checked("checkout","main")
  if name=="return-master":checked("branch","master",old)
  if name.startswith("return-"):(state/"branch").write_text("beta")
  if name in ["collision","stash-failed","retry-failed"]:(repo/"beta.txt").write_text("untracked\n")
  if name=="marker-failed":(state/"branch").mkdir()
  calls=[];warnings=[];app._log.warning=lambda message,*values:warnings.append(message%values if values else message)
  def recorded(*argv,**kw):
   calls.append({"args":list(argv),"timeout":kw.get("timeout")})
   fail=((name=="stash-failed" and argv[0] in ["checkout","stash"]) or
    (name=="retry-failed" and argv[0]=="checkout") or
    (name=="rescue-failed" and argv[:2]==("branch","--force")) or
    (name=="upstream-failed" and argv[:2]==("branch","--set-upstream-to")))
   if fail:return subprocess.CompletedProcess(argv,1,"","fixture-git-failed")
   return original(*argv,**kw)
  app._git=recorded;target=name.removeprefix("return-") if name.startswith("return-") else "beta";reference=old if name=="new-selected" else "origin/"+target
  try:app._trocar_para(target,reference);result={"ok":True}
  except Exception as error:result={"error":str(error),"type":type(error).__name__}
  refs=checked("for-each-ref","--format=%(refname:short)","refs/heads/resgate/").splitlines();head=checked("rev-parse","HEAD")
  marker=state/"branch";stash=git("show","stash@{0}^3:beta.txt")
  row={"name":name,"target":target,"reference":"<OLD>" if reference==old else reference,"result":result,"calls":calls,"warnings":warnings,"state_warnings":app.estado().get("avisos",[]),"branch":checked("rev-parse","--abbrev-ref","HEAD"),"head":"old" if head==old else "remote" if head==remote else "local","upstream":git("rev-parse","--abbrev-ref","@{upstream}").stdout.strip(),"refs":[{"name":x,"at_local":checked("rev-parse",x)==local}for x in refs],"marker":marker.read_text() if marker.is_file() else None,"marker_directory":marker.is_dir(),"beta_file":(repo/"beta.txt").read_text() if (repo/"beta.txt").exists() else None,"stashed_untracked":stash.stdout if stash.returncode==0 else None}
  def normalize(v):
   if isinstance(v,str):return re.sub(r"resgate/\d{4}-\d{2}-\d{2}-\d{4}","resgate/<MINUTE>",v.replace(str(state),"<STATE>").replace(str(repo),"<REPO>").replace(old,"<OLD>"))
   if isinstance(v,list):return [normalize(x)for x in v]
   if isinstance(v,dict):return {k:normalize(x)for k,x in v.items()}
   return v
  rows.append(normalize(row))
 out.write_text(json.dumps({"source_commit":commit,"command":"python generate-switch.py --reference <checkout> --output <fixture>","normalization":"Minuto no nome do resgate, OID old e caminhos temporários; demais bytes integrais.","cases":rows},ensure_ascii=False,indent=2)+"\n");print(f"{len(rows)} casos capturados")
