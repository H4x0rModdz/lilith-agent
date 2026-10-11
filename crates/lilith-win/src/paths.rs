use std::path::{Path, PathBuf};

/// Resolve o caminho que o LLM mandou: `~`, `%VAR%` e nomes de pastas
/// conhecidas em pt-BR/inglês no primeiro segmento ("Downloads\\foo.zip",
/// "Área de Trabalho"). Usa `dirs`, que respeita redirecionamento do OneDrive.
pub fn resolve(input: &str) -> PathBuf {
    let expanded = expand_env(input.trim().trim_matches('"'));
    let normalized = expanded.replace('/', "\\");

    if let Some(rest) = normalized.strip_prefix('~') {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest.trim_start_matches('\\'));
        }
    }

    let (first, rest) = match normalized.split_once('\\') {
        Some((f, r)) => (f, Some(r)),
        None => (normalized.as_str(), None),
    };
    if let Some(base) = known_folder(first) {
        return match rest {
            Some(r) if !r.is_empty() => base.join(r),
            _ => base,
        };
    }
    PathBuf::from(normalized)
}

pub fn known_folder(name: &str) -> Option<PathBuf> {
    match fold(name).as_str() {
        "downloads" | "download" | "transferencias" => dirs::download_dir(),
        "documentos" | "documents" | "meus documentos" => dirs::document_dir(),
        "area de trabalho" | "desktop" => dirs::desktop_dir(),
        "imagens" | "pictures" | "fotos" => dirs::picture_dir(),
        "musicas" | "music" | "musica" => dirs::audio_dir(),
        "videos" | "video" => dirs::video_dir(),
        "home" | "usuario" | "pasta do usuario" => dirs::home_dir(),
        _ => None,
    }
}

/// Minúsculas e sem acentos — para comparar nomes digitados pelo usuário.
pub fn fold(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let var = &after[..end];
                match std::env::var(var) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(var);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// `dest` livre: se já existe, tenta "nome (1).ext", "nome (2).ext", …
pub fn unique(dest: &Path) -> PathBuf {
    if !dest.exists() {
        return dest.to_path_buf();
    }
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("arquivo");
    let ext = dest.extension().and_then(|s| s.to_str());
    let parent = dest.parent().unwrap_or(Path::new("."));
    (1..)
        .map(|n| match ext {
            Some(e) => parent.join(format!("{stem} ({n}).{e}")),
            None => parent.join(format!("{stem} ({n})")),
        })
        .find(|p| !p.exists())
        .expect("sequência infinita")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_folders_in_portuguese() {
        assert_eq!(resolve("Downloads"), dirs::download_dir().unwrap());
        assert_eq!(
            resolve("Área de Trabalho/nota.txt"),
            dirs::desktop_dir().unwrap().join("nota.txt")
        );
    }

    #[test]
    fn env_and_absolute_paths() {
        std::env::set_var("LILITH_TEST_DIR", "C:\\tmp");
        assert_eq!(resolve("%LILITH_TEST_DIR%\\a.txt"), PathBuf::from("C:\\tmp\\a.txt"));
        assert_eq!(resolve("C:/x/y"), PathBuf::from("C:\\x\\y"));
        assert_eq!(resolve("%NAO_EXISTE%"), PathBuf::from("%NAO_EXISTE%"));
    }

    #[test]
    fn unique_appends_counter() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        assert_eq!(unique(&p), p);
        std::fs::write(&p, "x").unwrap();
        assert_eq!(unique(&p), dir.path().join("a (1).txt"));
    }
}
