"""Cobertura do git_ops: list/switch/action contra um repo temporario + rejeicoes e erro de binario."""
import pytest

from app import git_ops
from app.git_ops import GitError


def _repo(tmp_path):
    d = str(tmp_path)
    for args in (
        ["init", "-q", "-b", "main"],
        ["config", "user.email", "t@t"],
        ["config", "user.name", "t"],
        ["commit", "-q", "--allow-empty", "-m", "init"],
        ["branch", "feature"],
    ):
        git_ops._run(d, *args)
    return d


def test_list_branches(tmp_path):
    info = git_ops.list_branches(_repo(tmp_path))
    assert info["current"] == "main"
    assert set(info["branches"]) == {"main", "feature"}
    assert info["dirty"] is False


def test_list_branches_dirty(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "novo.txt").write_text("x")  # arquivo untracked -> tree suja
    assert git_ops.list_branches(d)["dirty"] is True


def test_switch_branch_ok(tmp_path):
    d = _repo(tmp_path)
    assert git_ops.switch_branch(d, "feature")["current"] == "feature"
    assert git_ops.list_branches(d)["current"] == "feature"


@pytest.mark.parametrize("bad", ["nope", "main; rm -rf /", "--orphan x"])
def test_switch_branch_rejects_invalid(tmp_path, bad):
    with pytest.raises(GitError) as e:
        git_ops.switch_branch(_repo(tmp_path), bad)
    assert e.value.status == 400  # nao esta na lista -> sem injecao/flag


def test_git_action_status_ok(tmp_path):
    assert git_ops.git_action(_repo(tmp_path), "status")["ok"] is True


def test_git_action_invalid(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.git_action(_repo(tmp_path), "rm -rf")  # fora da allowlist
    assert e.value.status == 400


def _with_remote(tmp_path):
    """Repo local + um segundo repo como 'origin' (com uma branch so-remota), ja com fetch feito."""
    d = _repo(tmp_path)
    (tmp_path / "remote").mkdir()   # git -C exige o path existente
    rd = str(tmp_path / "remote")
    for args in (
        ["init", "-q", "-b", "main"],
        ["config", "user.email", "t@t"],
        ["config", "user.name", "t"],
        ["commit", "-q", "--allow-empty", "-m", "r"],
        ["branch", "only-remote"],
    ):
        git_ops._run(rd, *args)
    git_ops._run(d, "remote", "add", "origin", rd)
    assert git_ops.git_action(d, "fetch")["ok"] is True
    return d


def test_list_branches_remotes_dedup(tmp_path):
    info = git_ops.list_branches(_with_remote(tmp_path))
    assert "only-remote" in info["remotes"]        # remota sem local -> aparece
    assert "main" not in info["remotes"]            # ja tem local -> dedup pelo nome curto
    assert "only-remote" not in info["branches"]    # ainda nao e local


def test_switch_remote_dwim_creates_local(tmp_path):
    d = _with_remote(tmp_path)
    assert git_ops.switch_branch(d, "only-remote")["current"] == "only-remote"
    assert "only-remote" in git_ops.list_branches(d)["branches"]  # DWIM criou a local


def _repo_with_file(tmp_path):
    """Repo com um arquivo tracked commitado (pra exercitar changed_files/diff/discard/stash)."""
    d = _repo(tmp_path)
    f = tmp_path / "tracked.txt"
    f.write_text("linha original\n")
    git_ops._run(d, "add", "tracked.txt")
    git_ops._run(d, "commit", "-q", "-m", "add tracked")
    return d, f


def test_changed_files_lists_tracked_and_untracked(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("linha modificada\n")            # tracked modificado
    (tmp_path / "novo.txt").write_text("x")        # untracked
    files = {c["path"]: c for c in git_ops.changed_files(d)}
    assert files["tracked.txt"]["code"] == " M" and files["tracked.txt"]["staged"] is False
    assert files["novo.txt"]["code"] == "??"


def test_file_diff_rejects_unlisted_path(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    for bad in ("nao-existe.txt", "../../etc/passwd", "--output=x"):
        with pytest.raises(GitError) as e:
            git_ops.file_diff(d, bad)              # so paths na lista de alterados passam
        assert e.value.status == 400


def test_file_diff_shows_edits(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("linha modificada\n")
    out = git_ops.file_diff(d, "tracked.txt")["diff"]
    assert "linha modificada" in out and "linha original" in out


def test_file_diff_untracked_shows_content(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    (tmp_path / "novo.txt").write_text("conteudo novo\n")
    assert "conteudo novo" in git_ops.file_diff(d, "novo.txt")["diff"]


def test_discard_tracked_restores_head(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("estragado\n")
    git_ops.discard_file(d, "tracked.txt")
    assert f.read_text() == "linha original\n"     # voltou ao HEAD
    assert "tracked.txt" not in git_ops._changed_map(d)


def test_discard_untracked_removes_file(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    novo = tmp_path / "novo.txt"
    novo.write_text("lixo")
    git_ops.discard_file(d, "novo.txt")
    assert not novo.exists()


def test_discard_rejects_unlisted_path(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    with pytest.raises(GitError) as e:
        git_ops.discard_file(d, "../../etc/passwd")
    assert e.value.status == 400


def test_stash_and_pop_roundtrip(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("mudanca pendente\n")
    assert git_ops.git_action(d, "stash")["ok"] is True
    assert git_ops.changed_files(d) == []          # tree limpa apos stash
    assert git_ops.git_action(d, "stash-pop")["ok"] is True
    assert f.read_text() == "mudanca pendente\n"    # reaplicado


def test_run_git_not_found(tmp_path, monkeypatch):
    # FileNotFoundError (git ausente) -> GitError 500, nao traceback cru.
    def boom(*a, **k):
        raise FileNotFoundError("no git")
    monkeypatch.setattr(git_ops.subprocess, "run", boom)
    with pytest.raises(GitError) as e:
        git_ops._run(str(tmp_path), "status")
    assert e.value.status == 500


def test_commit_amend_reescreve_e_dobra(tmp_path):
    d, f = _repo_with_file(tmp_path)          # commits: "init" (vazio) + "add tracked"
    (tmp_path / "novo.txt").write_text("N\n")
    r = git_ops.commit(d, "mensagem corrigida", ["novo.txt"], amend=True)
    assert r["ok"]
    out = git_ops._run(d, "log", "--pretty=%s").stdout.splitlines()
    assert out == ["mensagem corrigida", "init"]        # 2 commits, nao 3
    names = git_ops._run(d, "show", "--name-only", "--format=", "HEAD").stdout.split()
    assert "tracked.txt" in names and "novo.txt" in names   # dobrou sem perder o original


def test_commit_amend_sem_paths_e_so_reword(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    # Mudanca staged por FORA do app NAO pode vazar pra dentro de um reword:
    (tmp_path / "staged.txt").write_text("S\n")
    git_ops._run(d, "add", "staged.txt")
    r = git_ops.commit(d, "so renomeia a mensagem", [], amend=True)   # paths vazio SO vale com amend
    assert r["ok"]
    assert git_ops._run(d, "log", "-1", "--pretty=%B").stdout.strip() == "so renomeia a mensagem"
    names = git_ops._run(d, "show", "--name-only", "--format=", "HEAD").stdout.split()
    assert "staged.txt" not in names                      # --amend --only: staged nao vaza
    assert "A  staged.txt" in git_ops._run(d, "status", "--porcelain").stdout   # continua staged


def test_commit_amend_sem_head_falha(tmp_path):
    d = str(tmp_path)
    for args in (["init", "-q", "-b", "main"], ["config", "user.email", "t@t"], ["config", "user.name", "t"]):
        git_ops._run(d, *args)                          # repo SEM nenhum commit
    with pytest.raises(GitError) as e:
        git_ops.commit(d, "m", [], amend=True)
    assert e.value.status == 409


def test_commit_new_branch(tmp_path):
    d = _repo(tmp_path)
    (tmp_path / "x.txt").write_text("X\n")
    r = git_ops.commit(d, "na branch nova", ["x.txt"], new_branch="feat-x")
    assert r["ok"]
    assert git_ops._run(d, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip() == "feat-x"


@pytest.mark.parametrize("bad", ["feature", "nome com espaço", "-D", "../x"])
def test_commit_new_branch_invalida_ou_existente(tmp_path, bad):
    d = _repo(tmp_path)                                 # _repo ja cria a branch "feature"
    (tmp_path / "x.txt").write_text("X\n")
    with pytest.raises(GitError) as e:
        git_ops.commit(d, "m", ["x.txt"], new_branch=bad)
    assert e.value.status == 400


def test_commit_paths_vazio_sem_amend_segue_falhando(tmp_path):
    d = _repo(tmp_path)
    with pytest.raises(GitError) as e:
        git_ops.commit(d, "m", [])                      # regra antiga intacta
    assert e.value.status == 400


def test_last_commit_message(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    git_ops._run(d, "commit", "-q", "--amend", "-m", "assunto\n\ncorpo da mensagem")
    assert git_ops.last_commit_message(d)["message"] == "assunto\n\ncorpo da mensagem"


def test_commit_diff_inteiro(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("linha original\nlinha 2\n")
    (tmp_path / "g.txt").write_text("G\n")
    git_ops.commit(d, "mexe tracked e add g", ["tracked.txt", "g.txt"])
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    diff = git_ops.commit_diff(d, sha)["diff"]
    assert "+linha 2" in diff and "g.txt" in diff          # os DOIS arquivos no mesmo diff


def test_commit_diff_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.commit_diff(_repo(tmp_path), "nope; rm -rf /")
    assert e.value.status == 400


def test_commit_diff_truncado_alem_do_teto(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("x" * (git_ops._DIFF_MAX + 1000))
    git_ops.commit(d, "arquivo grande", ["tracked.txt"])
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    result = git_ops.commit_diff(d, sha)
    assert result["truncated"] is True
    assert len(result["diff"]) == git_ops._DIFF_MAX


def test_commit_diff_nao_trunca_abaixo_do_teto(tmp_path):
    d, f = _repo_with_file(tmp_path)
    f.write_text("linha 2\n")
    git_ops.commit(d, "pequeno", ["tracked.txt"])
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    assert git_ops.commit_diff(d, sha)["truncated"] is False


def test_revert_commit(tmp_path):
    d, f = _repo_with_file(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    r = git_ops.revert_commit(d, sha)
    assert r["ok"]
    assert not f.exists()                                  # revert desfez o add, commitado
    assert "Revert" in git_ops._run(d, "log", "-1", "--pretty=%s").stdout


def test_revert_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.revert_commit(_repo(tmp_path), "--no-edit")
    assert e.value.status == 400


def test_cherry_pick(tmp_path):
    d = _repo(tmp_path)
    git_ops._run(d, "switch", "-q", "-c", "feat")
    (tmp_path / "feat.txt").write_text("F\n")
    git_ops.commit(d, "na feat", ["feat.txt"])
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    git_ops._run(d, "switch", "-q", "main")
    r = git_ops.cherry_pick(d, sha)
    assert r["ok"] and (tmp_path / "feat.txt").exists()
    assert "na feat" in git_ops._run(d, "log", "-1", "--pretty=%s").stdout


def test_cherry_pick_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.cherry_pick(_repo(tmp_path), "HEAD; echo pwned")
    assert e.value.status == 400


def test_abort_actions_na_allowlist(tmp_path):
    d, f = _repo_with_file(tmp_path)
    # Sem sequencer em andamento, o abort FALHA (git diz "no revert in progress") —
    # o que prova que a ação chegou ao git (e não foi rejeitada pela allowlist).
    r = git_ops.git_action(d, "revert-abort")
    assert r["ok"] is False and "revert" in r["output"]
    r = git_ops.git_action(d, "cherry-pick-abort")
    assert r["ok"] is False and "cherry-pick" in r["output"]


def test_sequencer_state_repo_limpo(tmp_path):
    assert git_ops.sequencer_state(_repo(tmp_path)) is None


def test_sequencer_state_cherry_pick_em_conflito(tmp_path):
    d, f = _repo_with_file(tmp_path)          # tracked.txt = "linha original\n"
    git_ops._run(d, "switch", "-q", "-c", "feat")
    f.write_text("linha da feat\n")
    git_ops._run(d, "commit", "-q", "-am", "muda na feat")
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    git_ops._run(d, "switch", "-q", "main")
    f.write_text("linha da main\n")
    git_ops._run(d, "commit", "-q", "-am", "muda na main")
    p = git_ops._run(d, "cherry-pick", sha)
    assert p.returncode != 0                  # conflito real -> sequencer fica em andamento
    assert git_ops.sequencer_state(d) == "cherry-pick"
    git_ops._run(d, "cherry-pick", "--abort")
    assert git_ops.sequencer_state(d) is None


def test_sequencer_state_revert_em_conflito(tmp_path):
    d, f = _repo_with_file(tmp_path)
    add_sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    f.write_text("linha original\nlinha extra\n")
    git_ops._run(d, "commit", "-q", "-am", "extra depois do add")
    p = git_ops._run(d, "revert", "--no-edit", add_sha)
    assert p.returncode != 0                  # revert do "add" conflita com a edicao seguinte
    assert git_ops.sequencer_state(d) == "revert"
    git_ops._run(d, "revert", "--abort")
    assert git_ops.sequencer_state(d) is None


def test_reset_to_soft_mantem_staged(tmp_path):
    d, f = _repo_with_file(tmp_path)
    base = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    (tmp_path / "b.txt").write_text("B\n")
    git_ops.commit(d, "c2", ["b.txt"])
    r = git_ops.reset_to(d, base, "soft")
    assert r["ok"]
    assert git_ops._run(d, "rev-parse", "HEAD").stdout.strip() == base
    st = git_ops._run(d, "status", "--porcelain").stdout
    assert "A  b.txt" in st                              # soft: b.txt ficou STAGED


def test_reset_to_hard_descarta(tmp_path):
    d, f = _repo_with_file(tmp_path)
    base = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    f.write_text("estragado\n")
    git_ops.commit(d, "c2", ["tracked.txt"])
    git_ops.reset_to(d, base, "hard")
    assert f.read_text() == "linha original\n"


@pytest.mark.parametrize("bad_mode", ["--hard; rm", "HARD", ""])
def test_reset_modo_invalido(tmp_path, bad_mode):
    d = _repo(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    with pytest.raises(GitError) as e:
        git_ops.reset_to(d, sha, bad_mode)               # fora do enum -> 400 antes do git
    assert e.value.status == 400


def test_reset_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.reset_to(_repo(tmp_path), "HEAD~1", "soft")
    assert e.value.status == 400


def test_create_branch_at_sem_switch(tmp_path):
    d = _repo(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    r = git_ops.create_branch_at(d, "feat-nova", sha)
    assert r["ok"]
    info = git_ops.list_branches(d)
    assert "feat-nova" in info["branches"] and info["current"] == "main"   # não trocou


def test_create_branch_at_com_switch(tmp_path):
    d = _repo(tmp_path)
    git_ops.create_branch_at(d, "feat-vai", None, switch_after=True)
    assert git_ops.list_branches(d)["current"] == "feat-vai"


def test_create_branch_nome_invalido_ou_existente(tmp_path):
    d = _repo(tmp_path)                                  # _repo já cria "feature"
    for bad in ["feature", "nome com espaço", "-D"]:
        with pytest.raises(GitError) as e:
            git_ops.create_branch_at(d, bad, None)
        assert e.value.status == 400


def test_create_tag_anotada_e_leve(tmp_path):
    d = _repo(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    git_ops.create_tag(d, "v1.0", sha, message="release 1")
    git_ops.create_tag(d, "marcador", sha)
    out = git_ops._run(d, "tag", "--format=%(refname:short)%09%(objecttype)").stdout
    assert "v1.0\ttag" in out                            # anotada = objeto tag
    assert "marcador\tcommit" in out                     # leve = aponta pro commit


def test_create_tag_invalida_ou_duplicada(tmp_path):
    d = _repo(tmp_path)
    git_ops.create_tag(d, "v1", None)
    for bad in ["v1", "nome com espaço"]:
        with pytest.raises(GitError) as e:
            git_ops.create_tag(d, bad, None)
        assert e.value.status == 400


def test_diff_vs_worktree(tmp_path):
    d, f = _repo_with_file(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    f.write_text("linha original\nlinha nova nao commitada\n")
    diff = git_ops.diff_vs_worktree(d, sha)["diff"]
    assert "+linha nova nao commitada" in diff       # a mudanca do DISCO vs o commit
    assert git_ops.commit_diff(d, sha)["diff"] != diff   # difere do diff do commit em si


def test_diff_vs_worktree_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.diff_vs_worktree(_repo(tmp_path), "HEAD")   # nao casa _SHA_RE
    assert e.value.status == 400


def test_diff_vs_worktree_truncado_alem_do_teto(tmp_path):
    d, f = _repo_with_file(tmp_path)
    sha = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    f.write_text("y" * (git_ops._DIFF_MAX + 1000))          # mudanca NAO commitada, so no disco
    result = git_ops.diff_vs_worktree(d, sha)
    assert result["truncated"] is True
    assert len(result["diff"]) == git_ops._DIFF_MAX


def test_branches_containing(tmp_path):
    d = _repo(tmp_path)                                  # _repo cria "main" + "feature" no mesmo commit
    base = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    git_ops._run(d, "switch", "-q", "-c", "so-nesta")
    (tmp_path / "z.txt").write_text("Z\n")
    git_ops.commit(d, "so na nova", ["z.txt"])
    novo = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    assert set(git_ops.branches_containing(d, base)["local"]) == {"main", "feature", "so-nesta"}
    assert git_ops.branches_containing(d, novo)["local"] == ["so-nesta"]


def test_branches_containing_sha_invalido(tmp_path):
    with pytest.raises(GitError) as e:
        git_ops.branches_containing(_repo(tmp_path), "--all")
    assert e.value.status == 400


def test_branches_containing_ignora_origin_head_simbolico(tmp_path):
    # git branch -a --format='%(refname:short)' renderiza o origin/HEAD simbolico como o nome NU
    # do remote ('origin', sem '/HEAD') -- reproduzido contra este proprio repo antes do fix.
    d = _repo(tmp_path)                                   # main + feature no mesmo commit
    base = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    (tmp_path / "remote").mkdir()
    rd = str(tmp_path / "remote")
    git_ops._run(rd, "init", "-q", "-b", "main")
    git_ops._run(rd, "config", "receive.denyCurrentBranch", "ignore")   # aceita push na branch atual
    git_ops._run(d, "remote", "add", "origin", rd)
    assert git_ops._run(d, "push", "origin", "main").returncode == 0
    assert git_ops._run(d, "fetch", "origin").returncode == 0
    git_ops._run(d, "remote", "set-head", "origin", "main")             # cria o origin/HEAD simbolico
    result = git_ops.branches_containing(d, base)
    assert "origin" not in result["local"]              # nao pode virar branch local falsa
    assert "origin/main" in result["remote"]
    assert set(result["local"]) == {"main", "feature"}


def test_branches_containing_branch_local_com_nome_de_remote(tmp_path):
    # Um branch LOCAL chamado igual ao remote ('origin') nao pode sumir: no :short os dois saem com
    # o MESMO texto 'origin' -- e por isso o fix classifica pelo refname completo, nao pelo :short.
    d = _repo(tmp_path)
    base = git_ops._run(d, "rev-parse", "HEAD").stdout.strip()
    git_ops._run(d, "branch", "origin")                  # branch LOCAL chamada "origin"
    (tmp_path / "remote").mkdir()
    rd = str(tmp_path / "remote")
    git_ops._run(rd, "init", "-q", "-b", "main")
    git_ops._run(d, "remote", "add", "origin", rd)       # remote de mesmo nome, sem fetch ainda
    result = git_ops.branches_containing(d, base)
    assert set(result["local"]) == {"main", "feature", "origin"}   # a branch local NAO some
    assert result["remote"] == []


def test_git_log_grep(tmp_path):
    d, _ = _repo_with_file(tmp_path)                     # commits: "init", "add tracked"
    (tmp_path / "y.txt").write_text("Y\n")
    git_ops.commit(d, "agulha no palheiro", ["y.txt"])
    assuntos = [c["subject"] for c in git_ops.git_log(d, grep="agulha")]
    assert assuntos == ["agulha no palheiro"]
    assert git_ops.git_log(d, grep="nao existe nada assim") == []
    assert len(git_ops.git_log(d)) == 3                  # sem grep, tudo


def test_git_log_grep_nao_vira_flag(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    # Texto flag-like vai como VALOR de --grep= (nunca argv separado) -> zero resultado, sem erro.
    assert git_ops.git_log(d, grep="--all") == []


def test_git_log_grep_e_literal_nao_regex(tmp_path):
    d, _ = _repo_with_file(tmp_path)
    (tmp_path / "w.txt").write_text("W\n")
    git_ops.commit(d, "corrige c++ (de novo)", ["w.txt"])
    # Sem -F o git responderia "Invalid regular expression" (409) nestes dois:
    assert [c["subject"] for c in git_ops.git_log(d, grep="c++")] == ["corrige c++ (de novo)"]
    assert [c["subject"] for c in git_ops.git_log(d, grep="(de novo)")] == ["corrige c++ (de novo)"]
    # E o ponto e ponto, nao curinga:
    assert git_ops.git_log(d, grep="c.+") == []
