use gpui_kit::{img, Img, SharedString, Styled};

// Mesma prioridade do core/fileIcons.ts: nome, prefixo, extensão, genérico.
fn icon_name(name: &str, is_dir: bool, open: bool) -> String {
    let name = name.to_lowercase();
    if is_dir {
        let folder = match name.as_str() {
            "src" | "app" => "src",
            "pages" | "views" | "screens" | "routes" => "views",
            "components" | "widgets" => "components",
            "lib" | "libs" | "utils" | "helpers" => "lib",
            "test" | "tests" | "__tests__" | "spec" | "e2e" => "test",
            "docs" | "doc" => "docs",
            "node_modules" => "node",
            ".github" => "github",
            ".vscode" => "vscode",
            "scripts" | "bin" | "hooks" => "scripts",
            "public" | "static" | "assets" => "public",
            "dist" | "build" | "out" => "dist",
            "api" | "backend" | "server" => "api",
            "images" | "img" | "icons" => "images",
            "config" | ".config" | "settings" => "config",
            ".git" => "git",
            _ => "",
        };
        return format!("folder{}{}{}", if folder.is_empty() { "" } else { "-" }, folder, if open { "-open" } else { "" });
    }
    let exact = match name.as_str() {
        "package.json" => "nodejs",
        "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "uv.lock" => "lock",
        "claude.md" => "markdown",
        "dockerfile" => "docker",
        ".gitignore" | ".gitattributes" => "git",
        ".editorconfig" => "editorconfig",
        ".prettierrc" => "prettier",
        "biome.json" => "biome",
        "vitest.config.ts" => "vitest",
        "jest.config.js" => "jest",
        "pyproject.toml" => "toml",
        _ => "",
    };
    if !exact.is_empty() { return exact.to_owned(); }
    for (prefix, icon) in [
        (".env", "tune"),
        ("readme", "readme"),
        ("tsconfig", "tsconfig"),
        ("vite.config", "vite"),
        ("docker-compose", "docker"),
        (".eslintrc", "eslint"),
        ("eslint.config", "eslint"),
        (".prettierrc", "prettier"),
        ("vitest.config", "vitest"),
        ("settings.", "settings"),
    ] {
        if name.starts_with(prefix) { return icon.to_owned(); }
    }
    match name.rsplit_once('.').map_or("", |(_, ext)| ext) {
        "svelte" => "svelte",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" | "jsx" => "react",
        "js" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "cs" => "csharp",
        "dart" => "dart",
        "pas" | "dpr" | "dfm" => "pascal",
        "md" | "mdx" => "markdown",
        "json" | "jsonc" => "json",
        "css" | "scss" | "less" => "css",
        "html" | "htm" => "html",
        "xml" => "xml",
        "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" => "image",
        "sql" | "db" | "sqlite" => "database",
        "sh" | "bash" | "fish" | "zsh" => "console",
        "ps1" => "powershell",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "lock" => "lock",
        "pdf" => "pdf",
        "txt" | "log" => "document",
        "zip" | "tgz" | "gz" | "tar" | "7z" => "zip",
        "ttf" | "otf" | "woff" | "woff2" => "font",
        "mp4" | "mov" | "webm" | "mkv" => "video",
        "mp3" | "wav" | "m4a" | "ogg" => "audio",
        "env" => "tune",
        _ => "document",
    }.to_owned()
}

pub fn citation_icon(name: &str) -> Img {
    // img preserva as cores dos SVGs; svg/Icon os converte em máscara monocromática.
    img(SharedString::from(format!("file-icons/{}.svg", icon_name(name, false, false))))
        .size_3p5().flex_shrink_0()
}

macro_rules! file_assets {
    ($($name:literal),* $(,)?) => {
        const ASSETS: &[(&str, &[u8])] = &[$(
            (concat!("file-icons/", $name, ".svg"), include_bytes!(concat!("../assets/file-icons/", $name, ".svg"))),
        )*];
    };
}

file_assets!(
    "folder", "folder-open", "folder-src", "folder-src-open", "folder-views",
    "folder-views-open", "folder-components", "folder-components-open", "folder-lib", "folder-lib-open",
    "folder-test", "folder-test-open", "folder-docs", "folder-docs-open", "folder-node",
    "folder-node-open", "folder-github", "folder-github-open", "folder-vscode", "folder-vscode-open",
    "folder-scripts", "folder-scripts-open", "folder-public", "folder-public-open", "folder-dist",
    "folder-dist-open", "folder-api", "folder-api-open", "folder-images", "folder-images-open",
    "folder-config", "folder-config-open", "folder-git", "folder-git-open", "svelte",
    "typescript", "javascript", "react", "python", "csharp",
    "dart", "pascal", "markdown", "json", "css",
    "html", "xml", "database", "console", "powershell",
    "yaml", "toml", "lock", "image", "pdf",
    "document", "zip", "font", "video", "audio",
    "tune", "nodejs", "readme", "tsconfig", "vite",
    "biome", "git", "docker", "settings", "eslint",
    "prettier", "editorconfig", "vitest", "jest",
);

pub fn load(path: &str) -> Option<&'static [u8]> {
    ASSETS.iter().find(|(name, _)| *name == path).map(|(_, bytes)| *bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::AssetSource;

    #[test]
    fn web_priority_fallbacks_and_embedded_assets() {
        for (name, expected) in [
            ("main.rs", "document"), ("main.TS", "typescript"), ("note.md", "markdown"),
            ("photo.png", "image"), ("unknown.xyz", "document"), ("Makefile", "document"),
            ("package.json", "nodejs"), ("package-lock.json", "lock"), ("Dockerfile", "docker"),
            ("CLAUDE.md", "markdown"), ("README.pt.md", "readme"), (".env.local", "tune"),
            ("tsconfig.app.json", "tsconfig"), ("docker-compose.dev.yaml", "docker"),
            ("vitest.config.ts", "vitest"), ("settings.json", "settings"), ("src", "document"),
            ("view.tsx", "react"), ("module.mts", "typescript"), ("form.dfm", "pascal"),
        ] {
            assert_eq!(icon_name(name, false, false), expected, "{name}");
        }
        assert_eq!(icon_name("SRC", true, false), "folder-src");
        assert_eq!(icon_name("src", true, true), "folder-src-open");
        assert_eq!(icon_name("other", true, false), "folder");
        assert_eq!(icon_name("other", true, true), "folder-open");
        for (path, bytes) in ASSETS {
            assert!(bytes.starts_with(b"<svg"), "{path}");
            assert_eq!(crate::AppAssets.load(path).unwrap().unwrap().as_ref(), *bytes, "{path}");
        }
        assert!(load("file-icons/missing.svg").is_none());
    }
}
