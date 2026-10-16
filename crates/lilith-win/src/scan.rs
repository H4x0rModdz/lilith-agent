//! Varreduras de disco decididas pelo código: arquivos duplicados (tamanho
//! primeiro, depois hash do começo/fim e, só se precisar, do arquivo todo) e
//! os maiores arquivos de uma pasta. Pastas de sistema, links e arquivos que
//! só existem na nuvem ficam de fora (ler esses faria o OneDrive baixá-los).

use crate::organize::{human_size, is_cloud_only, is_hidden_or_system};
use crate::paths::fold;
use crate::WinError;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Pastas grandes e sem interesse para o usuário.
const SKIP_DIRS: &[&str] = &[
    "appdata", "node_modules", "$recycle.bin", "windows", "target", ".git", "system volume information",
];
const MAX_DEPTH: usize = 16;
/// Teto de arquivos olhados numa varredura (nunca trava o disco).
pub const MAX_FILES: usize = 300_000;
/// Pedaço lido do começo e do fim no hash rápido.
const EDGE: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

pub struct Walk {
    pub files: Vec<Entry>,
    /// Parou no teto de arquivos.
    pub truncated: bool,
}

/// Todos os arquivos de verdade (locais, não ocultos) sob `root`.
pub fn walk(root: &Path, max_files: usize) -> Result<Walk, WinError> {
    if !root.is_dir() {
        return Err(WinError::NotFound(format!("pasta {}", root.display())));
    }
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            // Links e junções não são seguidos (evita laços e outros discos).
            if kind.is_symlink() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if kind.is_dir() {
                let name = fold(&entry.file_name().to_string_lossy());
                if depth < MAX_DEPTH && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) && !is_hidden_or_system(&meta) {
                    stack.push((entry.path(), depth + 1));
                }
                continue;
            }
            if !kind.is_file() || is_hidden_or_system(&meta) || is_cloud_only(&meta) {
                continue;
            }
            files.push(Entry { path: entry.path(), size: meta.len(), modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH) });
            if files.len() >= max_files {
                return Ok(Walk { files, truncated: true });
            }
        }
    }
    Ok(Walk { files, truncated: false })
}

/// Hash do começo e do fim (mais o tamanho): barato e separa quase tudo.
fn edge_hash(path: &Path, size: u64) -> std::io::Result<[u8; 32]> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    h.update(size.to_le_bytes());
    let mut buf = vec![0u8; EDGE as usize];
    let n = read_up_to(&mut f, &mut buf)?;
    h.update(&buf[..n]);
    if size > EDGE * 2 {
        f.seek(SeekFrom::End(-(EDGE as i64)))?;
        let n = read_up_to(&mut f, &mut buf)?;
        h.update(&buf[..n]);
    } else if size > EDGE {
        let n = read_up_to(&mut f, &mut buf)?;
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

/// Hash do arquivo inteiro.
pub fn full_hash(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

fn read_up_to(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = f.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

/// Um grupo de cópias idênticas.
#[derive(Debug, Clone)]
pub struct Group {
    pub size: u64,
    pub hash: [u8; 32],
    pub files: Vec<Entry>,
}

impl Group {
    /// Espaço liberado apagando todas as cópias menos uma.
    pub fn reclaimable(&self) -> u64 {
        self.size * (self.files.len() as u64).saturating_sub(1)
    }
}

/// Agrupa os arquivos idênticos (mesmo conteúdo), do que libera mais espaço
/// para o que libera menos. Arquivos menores que `min_size` ficam de fora.
pub fn duplicates(files: Vec<Entry>, min_size: u64) -> Vec<Group> {
    let mut by_size: HashMap<u64, Vec<Entry>> = HashMap::new();
    for f in files.into_iter().filter(|f| f.size >= min_size.max(1)) {
        by_size.entry(f.size).or_default().push(f);
    }
    let mut groups = Vec::new();
    for (size, same_size) in by_size.into_iter().filter(|(_, v)| v.len() > 1) {
        let mut by_edge: HashMap<[u8; 32], Vec<Entry>> = HashMap::new();
        for f in same_size {
            if let Ok(h) = edge_hash(&f.path, size) {
                by_edge.entry(h).or_default().push(f);
            }
        }
        for (edge, candidates) in by_edge.into_iter().filter(|(_, v)| v.len() > 1) {
            // O hash rápido já leu o arquivo inteiro: não precisa ler de novo.
            if size <= EDGE * 2 {
                groups.push(Group { size, hash: edge, files: candidates });
                continue;
            }
            let mut by_full: HashMap<[u8; 32], Vec<Entry>> = HashMap::new();
            for f in candidates {
                if let Ok(h) = full_hash(&f.path) {
                    by_full.entry(h).or_default().push(f);
                }
            }
            groups.extend(by_full.into_iter().filter(|(_, v)| v.len() > 1).map(|(hash, files)| Group { size, hash, files }));
        }
    }
    for g in &mut groups {
        g.files.sort_by(|a, b| a.path.cmp(&b.path));
    }
    groups.sort_by(|a, b| b.reclaimable().cmp(&a.reclaimable()).then_with(|| a.files[0].path.cmp(&b.files[0].path)));
    groups
}

/// Nome com cara de cópia: "foto (1).jpg", "doc - Cópia.pdf", "a copy.txt".
fn looks_like_copy(path: &Path) -> bool {
    let stem = fold(&path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let numbered = stem.ends_with(')')
        && stem.rfind(" (").is_some_and(|i| stem[i + 2..stem.len() - 1].chars().all(|c| c.is_ascii_digit()) && stem.len() > i + 3);
    numbered || stem.contains("copia") || stem.contains("copy")
}

/// Qual cópia fica (o código decide): a que não tem nome de cópia, a mais
/// rasa na árvore, a mais antiga (a original) e, por fim, o nome mais curto.
pub fn keeper(files: &[Entry]) -> usize {
    (0..files.len())
        .min_by_key(|&i| {
            let f = &files[i];
            (
                looks_like_copy(&f.path),
                f.path.components().count(),
                f.modified,
                f.path.as_os_str().len(),
                f.path.clone(),
            )
        })
        .unwrap_or(0)
}

/// Os `n` maiores arquivos, do maior para o menor.
pub fn biggest(mut files: Vec<Entry>, n: usize) -> Vec<Entry> {
    files.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    files.truncate(n);
    files
}

/// Resultado de duplicados para o modelo/a resposta.
pub fn duplicates_report(root: &Path, groups: &[Group], truncated: bool, max_groups: usize) -> Value {
    let total: u64 = groups.iter().map(Group::reclaimable).sum();
    let shown: Vec<Value> = groups
        .iter()
        .take(max_groups)
        .map(|g| {
            let keep = keeper(&g.files);
            json!({
                "size": g.size,
                "size_text": human_size(g.size),
                "copies": g.files.len(),
                "reclaimable_text": human_size(g.reclaimable()),
                "keep": g.files[keep].path,
                "files": g.files.iter().map(|f| &f.path).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "folder": root,
        "groups": shown,
        "group_count": groups.len(),
        "reclaimable": total,
        "reclaimable_text": human_size(total),
        "truncated": truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn finds_identical_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let mut big_changed = big.clone();
        big_changed[150_000] ^= 1; // mesmo começo e fim, meio diferente
        write(root, "video.mp4", &big);
        write(root, "sub/video (1).mp4", &big);
        write(root, "sub/quase.mp4", &big_changed);
        write(root, "a.txt", b"conteudo igual");
        write(root, "b.txt", b"conteudo igual");
        write(root, "c.txt", b"conteudo outro");
        write(root, "vazio1.txt", b"");
        write(root, "vazio2.txt", b"");
        write(root, "node_modules/x/video.mp4", &big);

        let w = walk(root, MAX_FILES).unwrap();
        assert!(!w.truncated);
        let groups = duplicates(w.files, 1);
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].files.len(), 2);
        assert_eq!(groups[0].size, 300_000);
        assert!(groups[0].files.iter().all(|f| !f.path.ends_with("quase.mp4")));
        let keep = keeper(&groups[0].files);
        assert!(groups[0].files[keep].path.ends_with("video.mp4"));
        assert!(!groups[0].files[keep].path.to_string_lossy().contains("(1)"));

        let report = duplicates_report(root, &groups, false, 10);
        assert_eq!(report["group_count"], 2);
        assert_eq!(report["reclaimable"], 300_000 + 14);
    }

    #[test]
    fn copy_names_lose_to_the_original() {
        let e = |p: &str| Entry { path: PathBuf::from(p), size: 1, modified: SystemTime::UNIX_EPOCH };
        assert_eq!(keeper(&[e(r"C:\d\foto (2).jpg"), e(r"C:\d\foto.jpg")]), 1);
        assert_eq!(keeper(&[e(r"C:\d\doc - Cópia.pdf"), e(r"C:\d\outra\doc.pdf")]), 1);
        assert_eq!(keeper(&[e(r"C:\d\a\b\x.txt"), e(r"C:\d\x.txt")]), 1, "o mais raso");
        assert!(!looks_like_copy(Path::new("relatorio (final).pdf")));
        assert!(looks_like_copy(Path::new("relatorio (12).pdf")));
    }

    #[test]
    fn biggest_files_first() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "p.bin", &[0; 10]);
        write(dir.path(), "g/grande.bin", &[0; 5000]);
        write(dir.path(), "m.bin", &[0; 500]);
        let top = biggest(walk(dir.path(), MAX_FILES).unwrap().files, 2);
        let names: Vec<_> = top.iter().map(|e| e.path.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, vec!["grande.bin", "m.bin"]);
    }

    #[test]
    fn walk_stops_at_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write(dir.path(), &format!("{i}.txt"), b"x");
        }
        let w = walk(dir.path(), 3).unwrap();
        assert!(w.truncated);
        assert_eq!(w.files.len(), 3);
    }
}
