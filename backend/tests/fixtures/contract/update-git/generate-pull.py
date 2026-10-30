"""Captura fetch, fast-forward e resgate do arquivo solto antes do reset."""
import argparse, json, os, subprocess, sys, tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument("--reference",type=Path,required=True);p.add_argument("--output",type=Path,required=True);a=p.parse_args()
ref,out=a.reference.resolve(),a.output.resolve();commit=subprocess.check_output(["git","-C",str(ref),"rev-parse","HEAD"],text=True).strip()
with tempfile.TemporaryDirectory(prefix="hangar-pull-contract-") as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,GIT_CONFIG_NOSYSTEM="1",GIT_CONFIG_GLOBAL=os.devnull,GIT_AUTHOR_NAME="Fixture",GIT_AUTHOR_EMAIL="fixture@example.invalid",GIT_COMMITTER_NAME="Fixture",GIT_COMMITTER_EMAIL="fixture@example.invalid")
 os.chdir(root);sys.path.insert(0,str(ref/"backend"));from app import atualizar as app
 original=app._git;app.pinned_target=lambda target:("origin/"+target,None);rows=[]
 for name in ["fast-forward","diverged","untracked-collision","fetch-failed","tag-failed","stash-failed","reset-failed"]:
  base=root/name;base.mkdir();repo=base/"repo";repo.mkdir();remote=base/"remote";remote.mkdir();app.REPO=repo;os.environ["CLAUDE_CONFIG_DIR"]=str(base/"claude")
  def git(cwd,*argv):return subprocess.run(["git","-C",str(cwd),*argv],text=True,capture_output=True)
  def checked(cwd,*argv):
   result=git(cwd,*argv);assert result.returncode==0,result.stderr;return result.stdout.strip()
  checked(repo,"init","-b","main");(repo/"content").write_text("old\n");checked(repo,"add",".");checked(repo,"commit","-m","old");old=checked(repo,"rev-parse","HEAD")
  checked(remote,"init","--bare");checked(repo,"remote","add","origin",str(remote));checked(repo,"push","-u","origin","main")
  (repo/"content").write_text("new\n");(repo/"collision.txt").write_text("remote\n");checked(repo,"add",".");checked(repo,"commit","-m","new");new=checked(repo,"rev-parse","HEAD");checked(repo,"push","origin","main");checked(repo,"reset","--hard",old)
  if name=="diverged":
   (repo/"local.txt").write_text("local\n");checked(repo,"add",".");checked(repo,"commit","-m","local")
  if name in ["untracked-collision","stash-failed","reset-failed"]:(repo/"collision.txt").write_text("untracked\n")
  calls=[];warnings=[];app._log.warning=lambda message,*values:warnings.append(message % values if values else message)
  def recorded(*argv,**kw):
   calls.append({"args":list(argv),"timeout":kw.get("timeout")})
   fail=((name=="fetch-failed" and argv==("fetch","origin")) or
    (name=="tag-failed" and argv[:3]==("fetch","origin","--force")) or
    (name in ["stash-failed","reset-failed"] and argv[0]=="merge") or
    (name=="stash-failed" and argv[0]=="stash") or (name=="reset-failed" and argv[0]=="reset"))
   if fail:return subprocess.CompletedProcess(argv,1,"","fixture-git-failed")
   return original(*argv,**kw)
  app._git=recorded
  try:result={"value":app._puxar({"branch":"main","alvo":"main","sujo":0})}
  except Exception as error:result={"error":str(error),"type":type(error).__name__}
  head=checked(repo,"rev-parse","HEAD");stashed=git(repo,"show","stash@{0}^3:collision.txt")
  rows.append({"name":name,"result":result,"calls":calls,"warnings":warnings,"head":"old" if head==old else "new" if head==new else "local","content":(repo/"content").read_text(),"collision":(repo/"collision.txt").read_text() if (repo/"collision.txt").exists() else None,"stash":git(repo,"stash","list","--format=%s").stdout.splitlines(),"stashed_untracked":stashed.stdout if stashed.returncode==0 else None})
 out.write_text(json.dumps({"source_commit":commit,"command":"python generate-pull.py --reference <checkout> --output <fixture>","normalization":"OIDs do commit local comparados aos commits old/new gerados na própria captura.","cases":rows},ensure_ascii=False,indent=2)+"\n");print(f"{len(rows)} casos capturados")
