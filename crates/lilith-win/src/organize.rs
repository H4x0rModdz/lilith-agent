//! Arrumar uma pasta (Downloads, Área de Trabalho…): cada arquivo solto vai
//! para uma subpasta pelo tipo (Imagens, Documentos, Instaladores…). Tudo é
//! decidido aqui, pelo código: o plano é calculado, mostrado como prévia, e é
//! exatamente ele que roda depois da aprovação. Cada execução fica registrada
//! para "desfaz a organização" devolver os arquivos ao lugar.

use crate::paths::unique;
use crate::WinError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Subpastas criadas pela organização, na ordem em que aparecem na prévia.
pub const CATEGORIES: &[&str] = &["Imagens", "Documentos", "Instaladores", "Compactados", "Vídeos", "Áudio", "Outros"];

const IMAGES: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "gif", "bmp", "webp", "svg", "ico", "tif", "tiff", "heic", "heif", "avif", "psd", "raw",
    "cr2", "nef", "dng",
];
const DOCUMENTS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "xlsm", "ppt", "pptx", "odt", "ods", "odp", "txt", "rtf", "csv", "md", "epub",
    "mobi", "pages", "numbers", "key",
];
const INSTALLERS: &[&str] = &["exe", "msi", "msix", "msixbundle", "appx", "appxbundle", "apk", "iso", "img"];
const ARCHIVES: &[&str] = &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "cab"];
const VIDEOS: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "3gp"];
const AUDIO: &[&str] = &["mp3", "wav", "flac", "aac", "ogg", "oga", "m4a", "wma", "opus", "mid", "midi", "aiff"];

/// Downloads ainda em andamento (Chrome, Firefox, Edge, Opera, torrents…).
const PARTIAL: &[&str] = &["crdownload", "part", "partial", "download", "opdownload", "tmp", "!ut", "!qb", "filepart"];
/// Atalhos ficam onde estão (na Área de Trabalho, movê-los quebraria o uso).
const SHORTCUTS: &[&str] = &["lnk", "url"];
/// Mexido há menos que isso: pode estar sendo baixado ou salvo agora.
const RECENT: Duration = Duration::from_secs(60);
/// Quantas organizações ficam guardadas para desfazer.
const KEEP_BATCHES: usize = 20;

fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

/// Subpasta de um arquivo pelo tipo (extensão).
pub fn category_for(path: &Path) -> &'static str {
    let ext = ext_of(path);
    let ext = ext.as_str();
    if IMAGES.contains(&ext) {
        "Imagens"
    } else if DOCUMENTS.contains(&ext) {
        "Documentos"
    } else if INSTALLERS.contains(&ext) {
        "Instaladores"
    } else if ARCHIVES.contains(&ext) {
        "Compactados"
    } else if VIDEOS.contains(&ext) {
        "Vídeos"
    } else if AUDIO.contains(&ext) {
        "Áudio"
    } else {
        "Outros"
    }
}

/// Download incompleto ou arquivo temporário de programa aberto.
pub fn is_partial(name: &str) -> bool {
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    PARTIAL.contains(&ext) || lower.starts_with("~$") || lower.starts_with(".~lock")
}

/// Atributos do Windows (oculto, sistema, só na nuvem…).
pub(crate) fn attributes(meta: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x4_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

/// Oculto ou do sistema (desktop.ini, thumbs.db…): nunca é tocado.
pub(crate) fn is_hidden_or_system(meta: &std::fs::Metadata) -> bool {
    attributes(meta) & (ATTR_HIDDEN | ATTR_SYSTEM) != 0
}

/// Arquivo que só existe na nuvem (OneDrive "sob demanda"): ler o conteúdo
/// faria o Windows baixá-lo. Varreduras que leem conteúdo pulam esses.
pub(crate) fn is_cloud_only(meta: &std::fs::Metadata) -> bool {
    attributes(meta) & (ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS) != 0
}

/// Algum programa está com o arquivo aberto de um jeito que impede movê-lo
/// (download terminando, documento aberto no Word…)? Abre pedindo só o
/// direito de mover/apagar (DELETE), sem ler o conteúdo — rápido, não acorda
/// o antivírus nem baixa arquivo da nuvem — e compartilhando tudo, para não
/// atrapalhar quem está usando. `Err` = não deu para saber (sem permissão).
fn in_use(path: &Path) -> Result<bool, std::io::Error> {
    use std::os::windows::fs::OpenOptionsExt;
    const DELETE: u32 = 0x0001_0000;
    const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
    match std::fs::OpenOptions::new().access_mode(DELETE).share_mode(SHARE_ALL).open(path) {
        Ok(_) => Ok(false),
        // ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
        Err(e) if matches!(e.raw_os_error(), Some(32) | Some(33)) => Ok(true),
        Err(e) => Err(e),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
    pub category: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skip {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub folder: PathBuf,
    pub moves: Vec<Move>,
    pub skipped: Vec<Skip>,
    /// Pastas que já existiam ali (ficam como estão).
    pub folders_kept: usize,
}

/// Calcula o que seria movido, sem mexer em nada. Só os arquivos soltos na
/// pasta (subpastas ficam como estão).
pub fn plan(folder: &Path) -> Result<Plan, WinError> {
    if !folder.is_dir() {
        return Err(WinError::NotFound(format!("pasta {}", folder.display())));
    }
    let now = SystemTime::now();
    let mut moves = Vec::new();
    let mut skipped = Vec::new();
    let mut folders_kept = 0;
    for entry in std::fs::read_dir(folder)?.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            folders_kept += 1;
            continue;
        }
        if !kind.is_file() {
            continue; // links simbólicos e afins
        }
        let Ok(meta) = entry.metadata() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if is_hidden_or_system(&meta) || SHORTCUTS.contains(&ext_of(&path).as_str()) {
            continue;
        }
        if is_partial(&name) {
            skipped.push(Skip { name, reason: "download incompleto".into() });
            continue;
        }
        let recent = meta.modified().ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age < RECENT);
        if recent {
            skipped.push(Skip { name, reason: "mexido agora há pouco".into() });
            continue;
        }
        if !is_cloud_only(&meta) {
            match in_use(&path) {
                Ok(true) => {
                    skipped.push(Skip { name, reason: "aberto em outro programa".into() });
                    continue;
                }
                Ok(false) => {}
                Err(_) => {
                    skipped.push(Skip { name, reason: "sem permissão".into() });
                    continue;
                }
            }
        }
        let category = category_for(&path);
        moves.push(Move { to: folder.join(category).join(&name), from: path, category: category.into(), size: meta.len() });
    }
    let order = |c: &str| CATEGORIES.iter().position(|x| *x == c).unwrap_or(usize::MAX);
    moves.sort_by(|a, b| (order(&a.category), a.from.file_name()).cmp(&(order(&b.category), b.from.file_name())));
    skipped.sort_by_key(|s| s.name.to_lowercase());
    Ok(Plan { folder: folder.to_path_buf(), moves, skipped, folders_kept })
}

/// O movimento tem o formato de uma organização: de um arquivo solto da
/// pasta para `pasta\<Categoria>\<mesmo nome>`. Um plano adulterado (que
/// tentasse mover qualquer outra coisa) é recusado inteiro.
fn valid_move(folder: &Path, m: &Move) -> bool {
    let category_dir = m.to.parent();
    m.from.parent() == Some(folder)
        && category_dir.and_then(Path::parent) == Some(folder)
        && category_dir
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .is_some_and(|n| CATEGORIES.contains(&n) && n == m.category)
        && m.from.file_name().is_some()
        && m.from.file_name() == m.to.file_name()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Batch {
    ts_ms: i64,
    folder: PathBuf,
    /// (de onde veio, para onde foi de verdade).
    moves: Vec<(PathBuf, PathBuf)>,
}

/// Registro padrão para desfazer: `%APPDATA%\Lilith\organize-undo.json`.
pub fn default_log() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("Lilith").join("organize-undo.json")
}

fn read_log(log: &Path) -> Vec<Batch> {
    std::fs::read_to_string(log).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write_log(log: &Path, batches: &[Batch]) -> Result<(), WinError> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(batches).map_err(|e| WinError::Failed(e.to_string()))?;
    std::fs::write(log, text)?;
    Ok(())
}

/// Executa um plano já aprovado. Confere cada arquivo de novo (sumiu, mudou
/// de tamanho ou foi aberto nesse meio-tempo → fica onde está) e nunca
/// sobrescreve: nome repetido vira "nome (1).ext".
pub fn execute(plan: &Plan, log: &Path) -> Result<Value, WinError> {
    if let Some(bad) = plan.moves.iter().find(|m| !valid_move(&plan.folder, m)) {
        return Err(WinError::InvalidParams(format!("plano inválido: {}", bad.from.display())));
    }
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut by_category: BTreeMap<String, usize> = BTreeMap::new();
    let mut skipped: Vec<Skip> = plan.skipped.clone();
    let mut failed: Vec<Skip> = Vec::new();
    for m in &plan.moves {
        let name = m.from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Ok(meta) = std::fs::metadata(&m.from) else {
            skipped.push(Skip { name, reason: "não está mais lá".into() });
            continue;
        };
        if meta.len() != m.size {
            skipped.push(Skip { name, reason: "mudou desde a prévia".into() });
            continue;
        }
        if !is_cloud_only(&meta) && in_use(&m.from).unwrap_or(true) {
            skipped.push(Skip { name, reason: "aberto em outro programa".into() });
            continue;
        }
        let result = (|| -> Result<PathBuf, std::io::Error> {
            if let Some(dir) = m.to.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let dest = unique(&m.to);
            std::fs::rename(&m.from, &dest)?;
            Ok(dest)
        })();
        match result {
            Ok(dest) => {
                *by_category.entry(m.category.clone()).or_default() += 1;
                done.push((m.from.clone(), dest));
            }
            Err(e) => failed.push(Skip { name, reason: e.to_string() }),
        }
    }
    if !done.is_empty() {
        let mut batches = read_log(log);
        batches.push(Batch { ts_ms: now_ms(), folder: plan.folder.clone(), moves: done.clone() });
        let excess = batches.len().saturating_sub(KEEP_BATCHES);
        batches.drain(..excess);
        write_log(log, &batches)?;
    }
    let categories: Vec<Value> = CATEGORIES
        .iter()
        .filter_map(|c| by_category.get(*c).map(|n| json!({ "category": c, "count": n })))
        .collect();
    Ok(json!({
        "folder": plan.folder,
        "moved": done.len(),
        "categories": categories,
        "skipped": skipped,
        "failed": failed,
        "undo": !done.is_empty(),
    }))
}

fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// A última organização (para a prévia do desfazer).
pub fn last_batch_summary(log: &Path) -> Option<Value> {
    let b = read_log(log).pop()?;
    Some(json!({ "folder": b.folder, "count": b.moves.len(), "ts_ms": b.ts_ms, "moves": b.moves }))
}

/// Desfaz a última organização: cada arquivo volta para onde estava (se o
/// nome já estiver ocupado lá, ganha um "(1)"), e as subpastas que ficaram
/// vazias somem.
pub fn undo(log: &Path) -> Result<Value, WinError> {
    let mut batches = read_log(log);
    let Some(batch) = batches.pop() else {
        return Err(WinError::NotFound("nenhuma organização para desfazer".into()));
    };
    let (mut restored, mut missing) = (0usize, 0usize);
    let mut touched_dirs: Vec<PathBuf> = Vec::new();
    for (from, to) in batch.moves.iter().rev() {
        if !to.exists() {
            missing += 1;
            continue;
        }
        let dest = unique(from);
        match std::fs::rename(to, &dest) {
            Ok(()) => restored += 1,
            Err(_) => missing += 1,
        }
        if let Some(dir) = to.parent() {
            if !touched_dirs.iter().any(|d| d == dir) {
                touched_dirs.push(dir.to_path_buf());
            }
        }
    }
    for dir in touched_dirs {
        let is_category = dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| CATEGORIES.contains(&n));
        let empty = std::fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(false);
        if is_category && empty {
            let _ = std::fs::remove_dir(&dir);
        }
    }
    write_log(log, &batches)?;
    Ok(json!({ "folder": batch.folder, "restored": restored, "missing": missing }))
}

/// "2,1 MB", "340 MB", "1,2 GB" (base 1024, vírgula decimal).
pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{:.1} {}", value, UNITS[unit]).replace('.', ",")
    } else {
        format!("{:.0} {}", value, UNITS[unit])
    }
}

/// Prévia para o cartão do chat: quantos arquivos vão para cada pasta (com
/// alguns nomes de exemplo) e o que fica de fora, e por quê.
pub fn preview(plan: &Plan) -> Value {
    const SAMPLE: usize = 4;
    let folder_name = folder_label(&plan.folder);
    let mut groups = Vec::new();
    for c in CATEGORIES {
        let items: Vec<&Move> = plan.moves.iter().filter(|m| m.category == *c).collect();
        if items.is_empty() {
            continue;
        }
        let size: u64 = items.iter().map(|m| m.size).sum();
        groups.push(json!({
            "label": format!("{c} ({}, {})", items.len(), human_size(size)),
            "items": items.iter().take(SAMPLE).map(|m| m.from.file_name().map(|n| n.to_string_lossy().into_owned())).collect::<Vec<_>>(),
            "more": items.len().saturating_sub(SAMPLE),
        }));
    }
    let mut notes = Vec::new();
    if !plan.skipped.is_empty() {
        let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
        for s in &plan.skipped {
            *reasons.entry(s.reason.as_str()).or_default() += 1;
        }
        let why: Vec<String> = reasons.iter().map(|(r, n)| format!("{n} {r}")).collect();
        notes.push(format!("{} ficam onde estão: {}.", count_files(plan.skipped.len()), why.join(", ")));
    }
    if plan.folders_kept > 0 {
        notes.push(format!("As {} subpastas que já existem ficam como estão.", plan.folders_kept));
    }
    notes.push("Dá para desfazer depois: é só dizer “desfaz a organização”.".into());
    json!({
        "kind": "organize",
        "title": format!("Organizar {folder_name}"),
        "summary": format!("Vou mover {} para {} pastas pelo tipo.", count_files(plan.moves.len()), groups.len()),
        "groups": groups,
        "notes": notes,
        "confirm": "Organizar",
    })
}

fn count_files(n: usize) -> String {
    if n == 1 { "1 arquivo".into() } else { format!("{n} arquivos") }
}

/// Nome curto da pasta para mostrar ("Downloads").
pub fn folder_label(folder: &Path) -> String {
    folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old(path: &Path) {
        // Arquivos de teste "antigos": a regra de "mexido agora há pouco" não vale.
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
    }

    fn file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        old(&p);
        p
    }

    #[test]
    fn categories_by_extension() {
        assert_eq!(category_for(Path::new("a.JPG")), "Imagens");
        assert_eq!(category_for(Path::new("nota.pdf")), "Documentos");
        assert_eq!(category_for(Path::new("setup.exe")), "Instaladores");
        assert_eq!(category_for(Path::new("x.tar.gz")), "Compactados");
        assert_eq!(category_for(Path::new("filme.mkv")), "Vídeos");
        assert_eq!(category_for(Path::new("som.flac")), "Áudio");
        assert_eq!(category_for(Path::new("sem-extensao")), "Outros");
        assert!(is_partial("video.mp4.crdownload"));
        assert!(is_partial("arquivo.part"));
        assert!(is_partial("~$relatorio.docx"));
        assert!(!is_partial("relatorio.docx"));
    }

    #[test]
    fn plan_preview_execute_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        file(root, "foto.png", "png");
        file(root, "contrato.pdf", "pdf");
        file(root, "setup.exe", "exe");
        file(root, "pacote.zip", "zip");
        file(root, "baixando.mp4.crdownload", "x");
        file(root, "atalho.lnk", "x");
        std::fs::write(root.join("novo.txt"), "acabou de chegar").unwrap(); // recente: fica
        std::fs::create_dir(root.join("Projetos")).unwrap();
        // Já existe uma foto com o mesmo nome em Imagens: não pode sobrescrever.
        std::fs::create_dir(root.join("Imagens")).unwrap();
        std::fs::write(root.join("Imagens").join("foto.png"), "antiga").unwrap();

        let p = plan(root).unwrap();
        let names: Vec<_> = p.moves.iter().map(|m| (m.category.as_str(), m.from.file_name().unwrap().to_str().unwrap())).collect();
        assert_eq!(
            names,
            vec![("Imagens", "foto.png"), ("Documentos", "contrato.pdf"), ("Instaladores", "setup.exe"), ("Compactados", "pacote.zip")]
        );
        let reasons: Vec<_> = p.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert_eq!(reasons, vec!["download incompleto", "mexido agora há pouco"]);
        assert_eq!(p.folders_kept, 2);

        let pv = preview(&p);
        assert_eq!(pv["summary"], "Vou mover 4 arquivos para 4 pastas pelo tipo.");
        assert!(pv["notes"][0].as_str().unwrap().starts_with("2 arquivos ficam onde estão"));

        let log = root.join("undo.json");
        let r = execute(&p, &log).unwrap();
        assert_eq!(r["moved"], 4);
        assert!(root.join("Documentos").join("contrato.pdf").exists());
        assert!(root.join("Imagens").join("foto (1).png").exists(), "colisão vira (1)");
        assert_eq!(std::fs::read_to_string(root.join("Imagens").join("foto.png")).unwrap(), "antiga");
        assert!(root.join("baixando.mp4.crdownload").exists());
        assert!(root.join("atalho.lnk").exists());

        let summary = last_batch_summary(&log).unwrap();
        assert_eq!(summary["count"], 4);
        let u = undo(&log).unwrap();
        assert_eq!(u["restored"], 4);
        assert!(root.join("foto.png").exists() && root.join("contrato.pdf").exists() && root.join("setup.exe").exists());
        assert!(!root.join("Documentos").exists(), "pasta criada e vazia some");
        assert!(root.join("Imagens").join("foto.png").exists(), "a pasta que já tinha coisa fica");
        assert!(undo(&log).is_err(), "nada mais para desfazer");
    }

    #[test]
    fn tampered_plans_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let src = file(root, "a.txt", "x");
        let outside = tempfile::tempdir().unwrap();
        let log = root.join("undo.json");
        for to in [outside.path().join("a.txt"), root.join("Outros").join("b.txt"), root.join("Qualquer").join("a.txt")] {
            let p = Plan {
                folder: root.to_path_buf(),
                moves: vec![Move { from: src.clone(), to, category: "Outros".into(), size: 1 }],
                skipped: vec![],
                folders_kept: 0,
            };
            assert!(execute(&p, &log).is_err());
        }
        assert!(src.exists());
    }

    #[test]
    fn files_that_changed_after_the_preview_stay() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let a = file(root, "a.pdf", "x");
        let p = plan(root).unwrap();
        std::fs::write(&a, "agora maior").unwrap();
        let r = execute(&p, &root.join("undo.json")).unwrap();
        assert_eq!(r["moved"], 0);
        assert_eq!(r["skipped"][0]["reason"], "mudou desde a prévia");
        assert!(a.exists());
    }

    #[test]
    fn open_files_are_skipped() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let a = file(dir.path(), "aberto.docx", "x");
        let _held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&a).unwrap();
        let p = plan(dir.path()).unwrap();
        assert!(p.moves.is_empty());
        assert_eq!(p.skipped[0].reason, "aberto em outro programa");
    }

    #[test]
    fn sizes_in_portuguese() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1,5 KB");
        assert_eq!(human_size(340 * 1024 * 1024), "340 MB");
        assert_eq!(human_size(1288490188), "1,2 GB");
    }
}
