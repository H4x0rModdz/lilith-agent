//! Memória de interface: o que a Lilith já descobriu sobre cada programa,
//! guardado em `%APPDATA%\Lilith\ui-map.json`.
//!
//! Achar o campo de mensagem do WhatsApp varrendo a árvore de acessibilidade
//! leva ~2 s; saber *onde ele fica na janela* custa milissegundos. Então,
//! depois da primeira descoberta, guardamos o papel do elemento
//! (`message_input`) e a posição dele como margens em relação às bordas da
//! janela — não o endereço do elemento, que muda a cada abertura. Na vez
//! seguinte calculamos o ponto, confirmamos com uma pergunta ao Windows
//! ("o que existe aqui?") e usamos direto.
//!
//! O arquivo é legível e editável. Não guarda nomes de pessoas nem de
//! conversas: só programa, papel e geometria.

use crate::window::Rect;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static PATH: OnceLock<PathBuf> = OnceLock::new();
/// Escritas e leituras do arquivo uma por vez.
static LOCK: Mutex<()> = Mutex::new(());

pub fn init(path: PathBuf) {
    let _ = PATH.set(path);
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Map {
    /// Chave: nome do processo em minúsculas ("whatsapp.root", "discord").
    #[serde(default)]
    apps: BTreeMap<String, BTreeMap<String, Learned>>,
}

/// Posição de um elemento dentro da janela, em pixels a partir das bordas.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Learned {
    /// Tipo do elemento ("campo de texto").
    pub kind: String,
    /// Distância da borda esquerda / direita / inferior da janela.
    pub left: i32,
    pub right: i32,
    pub bottom: i32,
    pub height: i32,
    /// Tamanho da janela quando foi aprendido (só informativo).
    pub window: [i32; 2],
    /// Quantas vezes já foi confirmado funcionando.
    pub confirmed: u32,
    pub learned_ms: i64,
}

fn key(app: &str) -> String {
    app.trim().to_lowercase()
}

fn read() -> Map {
    let Some(path) = PATH.get() else { return Map::default() };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write(map: &Map) {
    let Some(path) = PATH.get() else { return };
    let result = (|| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec_pretty(map).unwrap_or_default())?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        tracing::warn!(error = %e, "não salvei o ui-map.json");
    }
}

/// Onde o elemento deve estar na janela de agora (a janela pode ter
/// mudado de tamanho desde que foi aprendido).
pub fn lookup(app: &str, role: &str, frame: Rect) -> Option<Rect> {
    let _g = LOCK.lock().unwrap();
    let learned = read().apps.get(&key(app))?.get(role)?.clone();
    place(&learned, frame)
}

/// Aplica as margens guardadas à janela atual; None se não couber.
pub(crate) fn place(l: &Learned, frame: Rect) -> Option<Rect> {
    let w = frame.w - l.left - l.right;
    if w <= 0 || l.height <= 0 {
        return None;
    }
    Some(Rect { x: frame.x + l.left, y: frame.y + frame.h - l.bottom - l.height, w, h: l.height })
}

/// Guarda (ou atualiza) o que foi descoberto.
pub fn remember(app: &str, role: &str, kind: &str, rect: Rect, frame: Rect) {
    if !plausible(rect, frame) {
        return;
    }
    let _g = LOCK.lock().unwrap();
    let mut map = read();
    let entry = Learned {
        kind: kind.to_string(),
        left: rect.x - frame.x,
        right: (frame.x + frame.w) - (rect.x + rect.w),
        bottom: (frame.y + frame.h) - (rect.y + rect.h),
        height: rect.h,
        window: [frame.w, frame.h],
        confirmed: 0,
        learned_ms: now_ms(),
    };
    map.apps.entry(key(app)).or_default().insert(role.to_string(), entry);
    write(&map);
}

/// Só aprende de uma janela normal: janela minimizada/na bandeja tem tamanho
/// e posição absurdos (ex.: 146×21 em -32000) e geraria margens lixo.
pub(crate) fn plausible(rect: Rect, frame: Rect) -> bool {
    let inside = rect.x >= frame.x
        && rect.y >= frame.y
        && rect.x + rect.w <= frame.x + frame.w
        && rect.y + rect.h <= frame.y + frame.h;
    frame.w >= 300 && frame.h >= 200 && rect.w > 0 && rect.h > 0 && inside
}

/// O elemento guardado funcionou de novo: conta mais uma confirmação.
pub fn confirm(app: &str, role: &str) {
    let _g = LOCK.lock().unwrap();
    let mut map = read();
    if let Some(l) = map.apps.get_mut(&key(app)).and_then(|a| a.get_mut(role)) {
        l.confirmed = l.confirmed.saturating_add(1);
        write(&map);
    }
}

/// O guardado não bateu com a tela: esquece, para a próxima busca refazer.
pub fn forget(app: &str, role: &str) {
    let _g = LOCK.lock().unwrap();
    let mut map = read();
    if let Some(a) = map.apps.get_mut(&key(app)) {
        if a.remove(role).is_some() {
            write(&map);
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_survives_a_resize() {
        let frame = Rect { x: 100, y: 50, w: 1238, h: 658 };
        let field = Rect { x: 540, y: 660, w: 625, h: 24 };
        let l = Learned {
            kind: "campo de texto".into(),
            left: field.x - frame.x,
            right: (frame.x + frame.w) - (field.x + field.w),
            bottom: (frame.y + frame.h) - (field.y + field.h),
            height: field.h,
            window: [frame.w, frame.h],
            confirmed: 0,
            learned_ms: 0,
        };
        // Mesma janela: mesmo retângulo.
        assert_eq!(place(&l, frame), Some(field));
        // Janela mais larga e movida: a margem esquerda fica, a direita acompanha.
        let bigger = Rect { x: 0, y: 0, w: 1600, h: 900 };
        let r = place(&l, bigger).unwrap();
        assert_eq!((r.x, r.h), (440, 24));
        assert_eq!(r.y + r.h + l.bottom, 900);
        assert_eq!(r.x + r.w + l.right, 1600);
        // Janela pequena demais: não cabe.
        assert_eq!(place(&l, Rect { x: 0, y: 0, w: 300, h: 900 }), None);
    }

    #[test]
    fn minimized_or_odd_windows_are_never_learned_from() {
        let field = Rect { x: 448, y: 718, w: 792, h: 56 };
        // Janela minimizada na bandeja: pequena e fora da tela.
        assert!(!plausible(field, Rect { x: -32000, y: -32000, w: 146, h: 21 }));
        // Campo fora da janela.
        assert!(!plausible(field, Rect { x: 0, y: 0, w: 400, h: 300 }));
        assert!(plausible(field, Rect { x: 400, y: 100, w: 1000, h: 800 }));
    }

    #[test]
    fn round_trip_through_the_json_file() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path().join("ui-map.json"));
        let frame = Rect { x: 0, y: 0, w: 1200, h: 700 };
        let field = Rect { x: 400, y: 640, w: 700, h: 30 };
        assert_eq!(lookup("WhatsApp.Root", "message_input", frame), None);
        remember("WhatsApp.Root", "message_input", "campo de texto", field, frame);
        assert_eq!(lookup("whatsapp.root", "message_input", frame), Some(field), "chave sem diferenciar maiúscula");
        confirm("whatsapp.root", "message_input");
        let text = std::fs::read_to_string(dir.path().join("ui-map.json")).unwrap();
        assert!(text.contains("message_input") && text.contains("\"confirmed\": 1"), "{text}");
        forget("whatsapp.root", "message_input");
        assert_eq!(lookup("whatsapp.root", "message_input", frame), None);
    }
}
