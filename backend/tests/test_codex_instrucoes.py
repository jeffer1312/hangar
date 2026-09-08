from pathlib import Path
import shutil
import subprocess

import pytest

from app.codex_instrucoes import preparar_instrucoes


def ambiente(tmp_path):
    home = tmp_path / 'home'
    (home / '.claude').mkdir(parents=True)
    cx = home / '.codex'
    cx.mkdir()
    projeto = home / 'projeto'
    (projeto / '.git').mkdir(parents=True)
    return home, cx, projeto


@pytest.mark.skipif(shutil.which('git') is None, reason='sem git')
def test_alias_de_projeto_sai_do_git_status_pelo_info_exclude(tmp_path):
    home, cx, projeto = ambiente(tmp_path)
    shutil.rmtree(projeto / '.git')
    subprocess.run(['git', 'init', '-q', str(projeto)], check=True)
    (projeto / 'CLAUDE.md').write_text('PROJETO')
    sub = projeto / 'sub'
    sub.mkdir()
    (sub / 'CLAUDE.md').write_text('SUB')
    preparar_instrucoes(home, cx, sub)
    preparar_instrucoes(home, cx, sub)
    exclude = (projeto / '.git/info/exclude').read_text()
    assert exclude.count('AGENTS.override.md') == 1
    status = subprocess.run(['git', '-C', str(projeto), 'status', '--porcelain'], capture_output=True, text=True).stdout
    assert 'AGENTS.override.md' not in status
    assert 'CLAUDE.md' in status


def test_conteudo_nativo_prioriza_claude_global_e_projeto(tmp_path):
    home, cx, projeto = ambiente(tmp_path)
    (home / '.claude/CLAUDE.md').write_text('GLOBAL obrigatório')
    (cx / 'AGENTS.md').write_text('GLOBAL antigo')
    (projeto / 'CLAUDE.md').write_text('PROJETO obrigatório')
    (projeto / 'AGENTS.md').write_text('PROJETO antigo')
    preparar_instrucoes(home, cx, projeto)
    assert (cx / 'AGENTS.override.md').read_text() == 'GLOBAL obrigatório'
    assert (projeto / 'AGENTS.override.md').read_text() == 'PROJETO obrigatório'
    assert (cx / 'AGENTS.md').read_text() == 'GLOBAL antigo'
    assert (projeto / 'AGENTS.md').read_text() == 'PROJETO antigo'


def test_link_acompanha_edicao_atomica_sem_reconciliar(tmp_path):
    home, cx, projeto = ambiente(tmp_path)
    source = projeto / 'CLAUDE.md'
    source.write_text('antes')
    preparar_instrucoes(home, cx, projeto)
    target = projeto / 'AGENTS.override.md'
    if not target.is_symlink():
        pytest.skip('Sistema sem links simbólicos')
    novo = projeto / 'novo'
    novo.write_text('depois')
    novo.replace(source)
    assert target.read_text() == 'depois'


def test_escopo_raiz_ate_cwd_e_nome_maiusculo(tmp_path):
    home, cx, projeto = ambiente(tmp_path)
    (home / 'CLAUDE.md').write_text('fora do projeto')
    (projeto / 'CLAUDE.MD').write_text('raiz')
    sub = projeto / 'src'
    sub.mkdir()
    (sub / 'CLAUDE.md').write_text('escopo local')
    preparar_instrucoes(home, cx, sub)
    assert (projeto / 'AGENTS.override.md').read_text() == 'raiz'
    assert (sub / 'AGENTS.override.md').read_text() == 'escopo local'
    assert not (home / 'AGENTS.override.md').exists()


def test_override_pessoal_nao_e_sobrescrito(tmp_path):
    home, cx, projeto = ambiente(tmp_path)
    (projeto / 'CLAUDE.md').write_text('Claude')
    target = projeto / 'AGENTS.override.md'
    target.write_text('personalizado')
    with pytest.raises(ValueError, match='AGENTS.override.md'):
        preparar_instrucoes(home, cx, projeto)
    assert target.read_text() == 'personalizado'


def test_copia_windows_atualiza_e_remove_sem_fonte(tmp_path, monkeypatch):
    home, cx, projeto = ambiente(tmp_path)
    source = projeto / 'CLAUDE.md'
    source.write_text('antes')
    def sem_link(*args, **kwargs):
        raise OSError('Links indisponíveis')
    monkeypatch.setattr(Path, 'symlink_to', sem_link)
    preparar_instrucoes(home, cx, projeto)
    target = projeto / 'AGENTS.override.md'
    assert not target.is_symlink()
    source.write_text('depois')
    preparar_instrucoes(home, cx, projeto)
    assert target.read_text() == 'depois'
    source.unlink()
    preparar_instrucoes(home, cx, projeto)
    assert not target.exists()


def test_copia_editada_pelo_usuario_e_preservada(tmp_path, monkeypatch):
    home, cx, projeto = ambiente(tmp_path)
    (projeto / 'CLAUDE.md').write_text('Claude')
    def sem_link(*args, **kwargs):
        raise OSError('Links indisponíveis')
    monkeypatch.setattr(Path, 'symlink_to', sem_link)
    preparar_instrucoes(home, cx, projeto)
    target = projeto / 'AGENTS.override.md'
    target.write_text('minha edição')
    with pytest.raises(ValueError):
        preparar_instrucoes(home, cx, projeto)
    assert target.read_text() == 'minha edição'


def test_sem_claude_preserva_override_pessoal(tmp_path):
    home, cx, projeto = ambiente(tmp_path)
    target = cx / 'AGENTS.override.md'
    target.write_text('Só Codex')
    preparar_instrucoes(home, cx, projeto)
    assert target.read_text() == 'Só Codex'


def test_projeto_registrado_e_reconciliado_sem_cwd(tmp_path):
    import json
    home, cx, projeto = ambiente(tmp_path)
    (cx / 'config.toml').write_text(f'[projects.{json.dumps(str(projeto))}]\ntrust_level="trusted"\n')
    (projeto / 'CLAUDE.md').write_text('registrado')
    preparar_instrucoes(home, cx)
    assert (projeto / 'AGENTS.override.md').read_text() == 'registrado'


def test_limite_cresce_com_fontes_e_preserva_config_maior(tmp_path):
    from app.codex_instrucoes import limite_instrucoes
    home, cx, projeto = ambiente(tmp_path)
    (projeto / 'CLAUDE.md').write_text('x' * 2000000)
    preparar_instrucoes(home, cx, projeto)
    assert limite_instrucoes(cx) > 2000000
    (cx / 'config.toml').write_text('project_doc_max_bytes=8000000\n')
    assert limite_instrucoes(cx) == 8000000


def test_abertura_nao_mexe_em_projeto_alheio(tmp_path):
    import json
    home, cx, projeto = ambiente(tmp_path)
    outro = home / 'outro'
    outro.mkdir()
    (outro / 'CLAUDE.md').write_text('Claude alheio')
    (outro / 'AGENTS.override.md').write_text('Override pessoal')
    (cx / 'config.toml').write_text(f'[projects.{json.dumps(str(outro))}]\ntrust_level="trusted"\n')
    (projeto / 'CLAUDE.md').write_text('meu projeto')
    preparar_instrucoes(home, cx, projeto)
    assert (projeto / 'AGENTS.override.md').read_text() == 'meu projeto'
    assert (outro / 'AGENTS.override.md').read_text() == 'Override pessoal'


def test_recupera_copia_apos_queda_antes_da_troca(tmp_path, monkeypatch):
    import os
    home, cx, projeto = ambiente(tmp_path)
    source = projeto / 'CLAUDE.md'
    source.write_text('antes')
    def sem_link(*args, **kwargs):
        raise OSError('Links indisponíveis')
    monkeypatch.setattr(Path, 'symlink_to', sem_link)
    preparar_instrucoes(home, cx, projeto)
    source.write_text('depois')
    original = os.replace
    def queda(src, dst, *args, **kwargs):
        if Path(dst) == projeto / 'AGENTS.override.md':
            raise OSError('Queda simulada')
        return original(src, dst, *args, **kwargs)
    with monkeypatch.context() as patch:
        patch.setattr(os, 'replace', queda)
        with pytest.raises(OSError, match='Queda simulada'):
            preparar_instrucoes(home, cx, projeto)
    preparar_instrucoes(home, cx, projeto)
    assert (projeto / 'AGENTS.override.md').read_text() == 'depois'


@pytest.mark.integration
def test_codex_real_recebe_conteudo_antes_de_qualquer_ferramenta(tmp_path):
    import json
    import os
    import shutil
    import subprocess
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    if os.environ.get('RUN_CODEX_INTEGRATION') != '1' or not shutil.which('codex'):
        pytest.skip('Exige Codex CLI; nenhuma chamada externa ou inferência')
    from app.codex_instrucoes import LIMITE_INSTRUCOES
    home, cx, projeto = ambiente(tmp_path)
    (home / '.claude/CLAUDE.md').write_text('MARCADOR_GLOBAL_NATIVO')
    (cx / 'AGENTS.md').write_text('MARCADOR_GLOBAL_PRETERIDO')
    (projeto / 'AGENTS.md').write_text('MARCADOR_PROJETO_PRETERIDO')
    (projeto / 'CLAUDE.md').write_text('MARCADOR_INICIO_NATIVO\n' + 'regra de teste\n' * 13000 + 'MARCADOR_FIM_NATIVO')
    recebidos = []

    class Captura(BaseHTTPRequestHandler):
        def do_POST(self):
            recebidos.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
            self.send_response(400)
            self.end_headers()
            self.wfile.write(b'{"error":{"message":"Fim da captura local"}}')

        def log_message(self, *args):
            pass

    with ThreadingHTTPServer(('127.0.0.1', 0), Captura) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        (cx / 'config.toml').write_text(
            f'model="gpt-5.4"\nmodel_provider="captura"\nproject_doc_max_bytes={LIMITE_INSTRUCOES}\n'
            '[model_providers.captura]\nname="Captura local"\nwire_api="responses"\n'
            f'base_url="http://127.0.0.1:{server.server_port}/v1"\n'
            'requires_openai_auth=false\nrequest_max_retries=0\nstream_max_retries=0\n')
        preparar_instrucoes(home, cx, projeto)
        try:
            from app.codex_importador import CodexNativo
            proc = subprocess.run([*CodexNativo(home, cx)._comando(), 'exec', '--skip-git-repo-check', '-C', str(projeto), 'OK'],
                env={**os.environ, 'HOME': str(home), 'USERPROFILE': str(home), 'CODEX_HOME': str(cx)},
                capture_output=True, timeout=45)
        finally:
            server.shutdown()
            thread.join()
    assert recebidos, proc.stderr.decode(errors='replace')[-1500:]
    entrada = json.dumps(recebidos[0]['input'])
    assert 'MARCADOR_GLOBAL_NATIVO' in entrada
    assert 'MARCADOR_INICIO_NATIVO' in entrada
    assert 'MARCADOR_FIM_NATIVO' in entrada
    assert 'MARCADOR_GLOBAL_PRETERIDO' not in entrada
    assert 'MARCADOR_PROJETO_PRETERIDO' not in entrada
