import os
import time
from pathlib import Path

import pytest

from app import planprog
from app.planprog import plan_progress


PLAN_A = """# Plano de teste — Implementation Plan

## Global Constraints

- [x] isto NAO e um step (checkbox fora de Step)

### Task 0: Medir o que o resto assume

- [x] **Step 1: Medir**
- [x] **Step 2: Anotar**

### Task 1: Fazer a coisa

- [x] **Step 1: Teste que falha**
- [ ] **Step 2: Implementar**
- [ ] **Step 3: Gate de tipos + verificação manual (mobile E desktop)**

Exemplo de codigo que o executor deve colar (NAO sao steps de verdade):

```python
### Task 9: fantasma
- [x] **Step 1: nao conta**
- [x] **Step 2: nao conta**
```
"""


def _write(tmp_path, name, body):
    d = tmp_path / "docs" / "superpowers" / "plans"
    d.mkdir(parents=True, exist_ok=True)
    (tmp_path / ".git").mkdir(exist_ok=True)      # _plans_dir para no 1o dir com .git
    p = d / name
    p.write_text(body, encoding="utf-8")
    return p


@pytest.fixture(autouse=True)
def _clean():
    planprog._reset_caches()
    yield
    planprog._reset_caches()


def test_parcial_e_ignora_fence(tmp_path):
    _write(tmp_path, "2026-07-29-plano-de-teste.md", PLAN_A)
    r = plan_progress(str(tmp_path))
    assert r is not None
    # 5 steps reais; os 2 do bloco de codigo e o "[x]" fora de Step nao contam
    assert (r.done, r.total) == (3, 5)
    assert r.task_total == 2          # "Task 9" mora dentro do fence
    assert r.task_idx == 2            # ordinal: "Task 0" e a 1a
    assert r.name == "plano-de-teste"
    assert r.complete is False
    assert r.tasks[0].title == "Task 0: Medir o que o resto assume"
    assert (r.tasks[0].done, r.tasks[0].total) == (2, 2)
    assert r.tasks[1].steps[2].manual is True
    assert r.tasks[1].steps[0].manual is False


def test_step_orfao_antes_da_1a_task_vira_task_implicita(tmp_path):
    """Step marcado antes do primeiro '### Task' contava no done/total geral mas sumia da lista de
    Tasks — a soma dos pares (plan_tasks) nao batia com r.done/r.total (a barra segmentada)."""
    body = (
        "- [x] **Step 1: orfao**\n\n"
        "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n"
    )
    _write(tmp_path, "2026-07-29-com-orfao.md", body)
    r = plan_progress(str(tmp_path))
    assert r is not None
    assert (r.done, r.total) == (2, 3)
    assert sum(t.done for t in r.tasks) == r.done
    assert sum(t.total for t in r.tasks) == r.total
    assert r.tasks[0].title == "(sem Task)"
    assert (r.tasks[0].done, r.tasks[0].total) == (1, 1)


def test_so_fence_marcado_nao_elege(tmp_path):
    """O caso que envenenava o proprio plano: exemplo de step marcado dentro de bloco de codigo."""
    body = (
        "### Task 1: X\n\n- [ ] **Step 1: A**\n\n"
        "```python\n- [x] **Step 1: exemplo**\n```\n"
    )
    _write(tmp_path, "2026-07-29-so-exemplo.md", body)
    assert plan_progress(str(tmp_path)) is None


def test_sem_step_marcado_nao_elege(tmp_path):
    _write(tmp_path, "2026-07-29-so-escrito.md",
           "### Task 1: X\n\n- [ ] **Step 1: A**\n- [ ] **Step 2: B**\n")
    assert plan_progress(str(tmp_path)) is None


def test_checkbox_fora_de_step_nao_elege(tmp_path):
    _write(tmp_path, "2026-07-29-falso.md",
           "## Criterios\n\n- [x] criterio atendido\n\n### Task 1: X\n\n- [ ] **Step 1: A**\n")
    assert plan_progress(str(tmp_path)) is None


def test_sem_step_nenhum_devolve_none(tmp_path):
    _write(tmp_path, "2026-07-29-vazio.md", "### Task 1: X\n\ntexto solto\n")
    assert plan_progress(str(tmp_path)) is None


def test_completo(tmp_path):
    _write(tmp_path, "2026-07-29-fechado.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [x] **Step 2: B**\n")
    r = plan_progress(str(tmp_path))
    assert r is not None and r.complete is True
    assert (r.done, r.total) == (2, 2)
    assert r.task_idx == 1        # sem pendencia: aponta pra ultima Task


def test_prefere_plano_com_step_pendente(tmp_path):
    _write(tmp_path, "2026-07-29-andando.md",
           "### Task 1: Y\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    time.sleep(0.02)
    _write(tmp_path, "2026-07-29-fechado.md",       # mais NOVO, porem concluido
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [x] **Step 2: Z**\n")
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "andando"


def test_entre_dois_pendentes_vence_o_mais_novo(tmp_path):
    """Fixa a DIREcao do sort. Sem este teste, inverter o reverse=True passava despercebido."""
    _write(tmp_path, "2026-07-29-velho.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    time.sleep(0.02)
    _write(tmp_path, "2026-07-29-novo.md",
           "### Task 1: Y\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "novo"


def test_sticky_mantem_plano_quando_outro_e_reescrito(tmp_path):
    _write(tmp_path, "2026-07-29-executando.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    first = plan_progress(str(tmp_path))
    assert first is not None and first.name == "executando"
    time.sleep(0.02)
    _write(tmp_path, "2026-07-29-outro.md",
           "### Task 1: Y\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n- [ ] **Step 3: C**\n")
    planprog._invalidate_discovery()     # simula o TTL vencendo, sem sleep de 3s
    again = plan_progress(str(tmp_path))
    assert again is not None and again.name == "executando"


def test_plano_velho_nao_elege(tmp_path):
    p = _write(tmp_path, "2026-06-01-antigo.md",
               "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    old = time.time() - 15 * 86400
    os.utime(p, (old, old))
    assert plan_progress(str(tmp_path)) is None


def test_sobe_ate_a_raiz_do_repo(tmp_path):
    _write(tmp_path, "2026-07-29-plano.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    sub = tmp_path / "frontend" / "src" / "lib"
    sub.mkdir(parents=True)
    r = plan_progress(str(sub))
    assert r is not None and r.name == "plano"


def test_para_no_repo_de_dentro_e_nao_vaza_pro_de_fora(tmp_path):
    """Worktree sem planos NAO pode mostrar o plano do checkout principal."""
    _write(tmp_path, "2026-07-29-de-fora.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    dentro = tmp_path / "sub" / "wt"
    dentro.mkdir(parents=True)
    (dentro / ".git").write_text("gitdir: /qualquer\n", encoding="utf-8")   # worktree: .git e ARQUIVO
    assert plan_progress(str(dentro)) is None


def test_cwd_none_e_sem_pasta(tmp_path):
    assert plan_progress(None) is None
    assert plan_progress("") is None
    assert plan_progress(str(tmp_path)) is None


def test_cache_por_mtime_ns(tmp_path):
    """_load e cacheado por mtime_ns: mesmo path + mesmo ns = nao rele; ns diferente = rele."""
    p = _write(tmp_path, "2026-07-29-plano.md",
               "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    first = planprog._load(str(p), 111)
    assert first is not None and first.done == 1
    # marcar troca "[ ]" por "[x]" — MESMO tamanho de arquivo
    p.write_text(p.read_text(encoding="utf-8").replace("- [ ] **Step 2", "- [x] **Step 2"),
                 encoding="utf-8")
    assert planprog._load(str(p), 111).done == 1       # mesmo ns: serve o cache
    assert planprog._load(str(p), 222).done == 2       # ns novo: rele


def test_um_plano_ilegivel_nao_derruba_os_outros(tmp_path, monkeypatch):
    bom = _write(tmp_path, "2026-07-29-bom.md",
                 "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")
    _write(tmp_path, "2026-07-29-ruim.md", "### Task 1: Y\n\n- [x] **Step 1: A**\n")
    real = planprog.Path.read_bytes

    def _seletivo(self, *a, **k):
        if self.name.endswith("ruim.md"):
            raise PermissionError("negado")
        return real(self, *a, **k)

    monkeypatch.setattr(planprog.Path, "read_bytes", _seletivo)
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "bom"       # o ilegivel nao mata a feature do repo


def test_todos_ilegiveis_devolve_none(tmp_path, monkeypatch):
    _write(tmp_path, "2026-07-29-plano.md",
           "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: B**\n")

    def _boom(self, *a, **k):
        raise FileNotFoundError("sumiu")

    monkeypatch.setattr(planprog.Path, "read_bytes", _boom)
    assert plan_progress(str(tmp_path)) is None


def test_formato_real_dos_planos_do_repo():
    """Fixture viva contra deriva de formato: um plano TRACKED deste worktree ainda casa o regex.
    Nao afirma progresso (os git-* tem 0 marcados) — afirma que o formato nao mudou."""
    root = Path(__file__).resolve().parents[2]
    alvo = root / "docs" / "superpowers" / "plans" / "2026-07-29-git-log-hub.md"
    if not alvo.exists():
        pytest.skip("plano ausente neste checkout")
    raw = alvo.read_text(encoding="utf-8")
    assert len(planprog._STEP_RE.findall(raw)) >= 15
    assert len(planprog._TASK_RE.findall(raw)) >= 2


# ── Pin manual: o usuario escolhe QUAL plano o painel mostra ──────────────────────────────────────

NAO_COMECADO = """# Plano novo — Implementation Plan

### Task 1: Fazer

- [ ] **Step 1: Um**
- [ ] **Step 2: Dois**
"""

COMPLETO = """# Plano pronto — Implementation Plan

### Task 1: Fazer

- [x] **Step 1: Um**
"""

PENDENTE = """# Plano andando — Implementation Plan

### Task 1: Fazer

- [x] **Step 1: Um**
- [ ] **Step 2: Dois**
"""


def _tres_planos(tmp_path):
    """Um plano de cada tipo. O 'pendente' e o unico que a eleicao automatica escolheria."""
    _write(tmp_path, "2026-07-01-pendente.md", PENDENTE)
    _write(tmp_path, "2026-07-02-nao-comecado.md", NAO_COMECADO)
    _write(tmp_path, "2026-07-03-completo.md", COMPLETO)
    planprog._reset_caches()
    return planprog._plans_dir(str(tmp_path))


def test_sem_pin_a_eleicao_automatica_manda(tmp_path):
    _tres_planos(tmp_path)
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "pendente"


def test_pin_manda_num_plano_NAO_COMECADO(tmp_path):
    # O caso que motivou o pin: um plano 0/N nao acende a barra sozinho (regra do _load), mas e
    # exatamente o que se fixa — fixa-se porque se vai comecar.
    root = _tres_planos(tmp_path)
    planprog.write_pin(root, "2026-07-02-nao-comecado")
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "nao-comecado"
    assert (r.done, r.total) == (0, 2)


def test_pin_em_plano_COMPLETO_devolve_pro_automatico(tmp_path):
    # Regra escolhida pelo usuario: o pin vale ate o plano fechar. Ficar preso num plano terminado
    # seria pior que o problema que o pin resolve.
    root = _tres_planos(tmp_path)
    planprog.write_pin(root, "2026-07-03-completo")
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "pendente"


def test_pin_apontando_pra_arquivo_que_sumiu_cai_no_automatico(tmp_path):
    root = _tres_planos(tmp_path)
    planprog.write_pin(root, "2026-07-09-nunca-existiu")
    r = plan_progress(str(tmp_path))
    assert r is not None and r.name == "pendente"


def test_soltar_o_pin_volta_pro_automatico(tmp_path):
    root = _tres_planos(tmp_path)
    planprog.write_pin(root, "2026-07-02-nao-comecado")
    assert plan_progress(str(tmp_path)).name == "nao-comecado"
    planprog.write_pin(root, None)
    assert planprog.read_pin(root) is None
    assert plan_progress(str(tmp_path)).name == "pendente"


def test_pin_mora_no_git_e_nao_suja_a_pasta_de_planos(tmp_path):
    # Gravar na pasta de planos apareceria no `git status` de quem versiona os planos. O `.git/`
    # nunca e rastreado e some junto com o clone — comportamento certo pra preferencia local.
    root = _tres_planos(tmp_path)
    planprog.write_pin(root, "2026-07-01-pendente")
    assert (tmp_path / ".git" / "cp-plan-pin").is_file()
    assert not any(f.name.endswith("plan-pin") for f in Path(root).iterdir())


@pytest.mark.parametrize("veneno", ["../fora", "a/b", "..", ".", ""])
def test_pin_com_travessia_e_ignorado(tmp_path, veneno):
    # read_pin e a ultima linha de defesa: o valor vira nome de arquivo. A rota tambem valida, mas
    # um pin escrito a mao no .git nao passa por ela.
    root = _tres_planos(tmp_path)
    (tmp_path / ".git" / "cp-plan-pin").write_text(veneno + "\n", encoding="utf-8")
    planprog._reset_caches()
    assert planprog.read_pin(root) is None
    assert plan_progress(str(tmp_path)).name == "pendente"


def test_list_plans_traz_os_tres_inclusive_os_que_a_eleicao_descarta(tmp_path):
    _tres_planos(tmp_path)
    r = planprog.list_plans(str(tmp_path))
    assert r is not None
    por_nome = {p["name"]: p for p in r["plans"]}
    assert set(por_nome) == {"pendente", "nao-comecado", "completo"}
    # O nao-comecado precisa mostrar 0/2, nao 0/0: sem o total, o seletor diria que o plano nao tem
    # passo nenhum.
    assert (por_nome["nao-comecado"]["done"], por_nome["nao-comecado"]["total"]) == (0, 2)
    assert por_nome["completo"]["complete"] is True
    assert por_nome["pendente"]["complete"] is False
    assert r["pinned"] is None


def test_list_plans_sem_pasta_de_planos(tmp_path):
    (tmp_path / ".git").mkdir()
    assert planprog.list_plans(str(tmp_path)) is None
