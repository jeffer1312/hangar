// Ícone por nome de arquivo/pasta, no tema do VS Code (material-icon-theme). Só o subconjunto de
// fileIcons.lista.json existe no bundle; o resto cai no genérico — que é o `document`/`folder`
// do próprio tema, não os dois SVGs de traço de antes.
import { ICONES } from './fileIcons.generated';

const PASTA: Record<string, string> = {
  src: 'src', app: 'src',
  pages: 'views', views: 'views', screens: 'views', routes: 'views',
  components: 'components', widgets: 'components',
  lib: 'lib', libs: 'lib', utils: 'lib', helpers: 'lib',
  test: 'test', tests: 'test', __tests__: 'test', spec: 'test', e2e: 'test',
  docs: 'docs', doc: 'docs',
  node_modules: 'node',
  '.github': 'github', '.vscode': 'vscode',
  scripts: 'scripts', bin: 'scripts', hooks: 'scripts',
  public: 'public', static: 'public', assets: 'public',
  dist: 'dist', build: 'dist', out: 'dist',
  api: 'api', backend: 'api', server: 'api',
  images: 'images', img: 'images', icons: 'images',
  config: 'config', '.config': 'config', settings: 'config',
  '.git': 'git',
};

const NOME: Record<string, string> = {
  'package.json': 'nodejs', 'package-lock.json': 'lock', 'pnpm-lock.yaml': 'lock', 'yarn.lock': 'lock', 'uv.lock': 'lock',
  'claude.md': 'markdown', 'dockerfile': 'docker', '.gitignore': 'git', '.gitattributes': 'git',
  '.editorconfig': 'editorconfig', '.prettierrc': 'prettier', 'biome.json': 'biome',
  'vitest.config.ts': 'vitest', 'jest.config.js': 'jest', 'pyproject.toml': 'toml',
};

const EXT: Record<string, string> = {
  svelte: 'svelte', ts: 'typescript', mts: 'typescript', cts: 'typescript', tsx: 'react', jsx: 'react',
  js: 'javascript', mjs: 'javascript', cjs: 'javascript', py: 'python', cs: 'csharp', dart: 'dart',
  pas: 'pascal', dpr: 'pascal', dfm: 'pascal', md: 'markdown', mdx: 'markdown', json: 'json', jsonc: 'json',
  css: 'css', scss: 'css', less: 'css', html: 'html', htm: 'html', xml: 'xml', svg: 'image',
  sql: 'database', db: 'database', sqlite: 'database', sh: 'console', bash: 'console', fish: 'console',
  zsh: 'console', ps1: 'powershell', yaml: 'yaml', yml: 'yaml', toml: 'toml', lock: 'lock',
  png: 'image', jpg: 'image', jpeg: 'image', gif: 'image', webp: 'image', avif: 'image', bmp: 'image', ico: 'image',
  pdf: 'pdf', txt: 'document', log: 'document', zip: 'zip', tgz: 'zip', gz: 'zip', tar: 'zip', '7z': 'zip',
  ttf: 'font', otf: 'font', woff: 'font', woff2: 'font', mp4: 'video', mov: 'video', webm: 'video', mkv: 'video',
  mp3: 'audio', wav: 'audio', m4a: 'audio', ogg: 'audio', env: 'tune',
};

// ponytail: prefixo testado por startsWith — cobre .env.local, README.pt.md, tsconfig.app.json,
// vite.config.ts, docker-compose.dev.yaml sem regex.
const PREFIXO: [string, string][] = [
  ['.env', 'tune'], ['readme', 'readme'], ['tsconfig', 'tsconfig'], ['vite.config', 'vite'],
  ['docker-compose', 'docker'], ['.eslintrc', 'eslint'], ['eslint.config', 'eslint'],
  ['.prettierrc', 'prettier'], ['vitest.config', 'vitest'], ['settings.', 'settings'],
];

export function nomeIcone(nome: string, isDir: boolean, aberta = false): string {
  const n = nome.toLowerCase();
  if (isDir) {
    const p = PASTA[n];
    return p ? `folder-${p}${aberta ? '-open' : ''}` : (aberta ? 'folder-open' : 'folder');
  }
  const direto = NOME[n] ?? PREFIXO.find(([pre]) => n.startsWith(pre))?.[1];
  if (direto) return direto;
  const i = n.lastIndexOf('.');
  const ext = i >= 0 ? n.slice(i + 1) : '';
  return EXT[ext] ?? 'document';
}

export function svgIcone(nome: string, isDir: boolean, aberta = false): string {
  return ICONES[nomeIcone(nome, isDir, aberta)] ?? ICONES[isDir ? 'folder' : 'document'];
}
