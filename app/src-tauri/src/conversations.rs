//! Conversas salvas: uma por arquivo em `%APPDATA%\Lilith\conversations\`.
//! Guarda as falas do usuário e as respostas da Lilith (não os passos), o
//! título (renomeável) e as datas — o chat agrupa o histórico por dia e o
//! painel agrupa as tarefas pelo título da conversa.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub const DEFAULT_TITLE: &str = "Nova conversa";
const MAX_TITLE: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// "user" ou "lilith".
    pub role: String,
    pub text: String,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    /// O usuário escolheu o título (não sobrescrever com o automático).
    #[serde(default)]
    pub renamed: bool,
    pub created_ms: i64,
    pub updated_ms: i64,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub messages: usize,
}

/// Id vindo do frontend: curto e sem nada que vire caminho.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

pub struct Store {
    dir: PathBuf,
    items: Mutex<HashMap<String, Conversation>>,
}

impl Store {
    pub fn load(dir: PathBuf) -> Self {
        let mut items = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
                match serde_json::from_str::<Conversation>(&text) {
                    Ok(c) if valid_id(&c.id) => {
                        items.insert(c.id.clone(), c);
                    }
                    _ => tracing::warn!(path = %e.path().display(), "conversa ilegível ignorada"),
                }
            }
        }
        Self { dir, items: Mutex::new(items) }
    }

    pub fn list(&self) -> Vec<Summary> {
        let items = self.items.lock().unwrap();
        let mut v: Vec<Summary> = items
            .values()
            .map(|c| Summary {
                id: c.id.clone(),
                title: c.title.clone(),
                created_ms: c.created_ms,
                updated_ms: c.updated_ms,
                messages: c.messages.len(),
            })
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.updated_ms));
        v
    }

    pub fn get(&self, id: &str) -> Option<Conversation> {
        self.items.lock().unwrap().get(id).cloned()
    }

    #[cfg(test)]
    fn title(&self, id: &str) -> Option<String> {
        self.items.lock().unwrap().get(id).map(|c| c.title.clone())
    }

    /// Acrescenta uma fala (cria a conversa se preciso). Na primeira fala do
    /// usuário, o título automático vira o começo dela. Devolve o novo título
    /// se ele mudou.
    pub fn append(&self, id: &str, role: &str, text: &str) -> Option<String> {
        let ts = now_ms();
        let (snapshot, retitled) = {
            let mut items = self.items.lock().unwrap();
            let c = items.entry(id.to_string()).or_insert_with(|| Conversation {
                id: id.to_string(),
                title: DEFAULT_TITLE.into(),
                renamed: false,
                created_ms: ts,
                updated_ms: ts,
                messages: Vec::new(),
            });
            let first_user = role == "user" && !c.messages.iter().any(|m| m.role == "user");
            c.messages.push(Message { role: role.into(), text: text.into(), ts });
            c.updated_ms = ts;
            let retitled = if first_user && !c.renamed {
                c.title = auto_title(text);
                Some(c.title.clone())
            } else {
                None
            };
            (c.clone(), retitled)
        };
        self.save(&snapshot);
        retitled
    }

    /// Renomeia (cria vazia se ainda não existir). Devolve o título final.
    pub fn rename(&self, id: &str, title: &str) -> String {
        let title = clean_title(title);
        let ts = now_ms();
        let snapshot = {
            let mut items = self.items.lock().unwrap();
            let c = items.entry(id.to_string()).or_insert_with(|| Conversation {
                id: id.to_string(),
                title: DEFAULT_TITLE.into(),
                renamed: false,
                created_ms: ts,
                updated_ms: ts,
                messages: Vec::new(),
            });
            c.title = title;
            c.renamed = true;
            c.clone()
        };
        self.save(&snapshot);
        snapshot.title
    }

    pub fn delete(&self, id: &str) {
        self.items.lock().unwrap().remove(id);
        let _ = std::fs::remove_file(self.path(id));
    }

    /// Pares (fala do usuário, resposta) para reabrir a memória da Lilith.
    pub fn turns(&self, id: &str) -> Vec<(String, String)> {
        let Some(c) = self.get(id) else { return Vec::new() };
        let mut out = Vec::new();
        let mut pending: Option<String> = None;
        for m in c.messages {
            match m.role.as_str() {
                "user" => pending = Some(m.text),
                "lilith" => {
                    if let Some(u) = pending.take() {
                        out.push((u, m.text));
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn save(&self, c: &Conversation) {
        let result = (|| {
            std::fs::create_dir_all(&self.dir)?;
            let tmp = self.path(&c.id).with_extension("json.part");
            std::fs::write(&tmp, serde_json::to_vec_pretty(c).unwrap_or_default())?;
            std::fs::rename(&tmp, self.path(&c.id))
        })();
        if let Err(e) = result {
            tracing::warn!(error = %e, id = %c.id, "não salvei a conversa");
        }
    }
}

fn clean_title(raw: &str) -> String {
    let t: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(MAX_TITLE).collect();
    if t.is_empty() {
        DEFAULT_TITLE.into()
    } else {
        t
    }
}

fn auto_title(first_message: &str) -> String {
    let t = clean_title(first_message);
    if t.chars().count() > 32 {
        format!("{}…", t.chars().take(32).collect::<String>())
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_titles_rename_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::load(dir.path().to_path_buf());
        assert_eq!(s.append("c1", "user", "  abre   o spotify pra mim  "), Some("abre o spotify pra mim".into()));
        assert_eq!(s.append("c1", "lilith", "Pronto!"), None);
        assert_eq!(s.append("c1", "user", "valeu"), None, "só a primeira fala dá título");
        // A fala é guardada como veio (o send_message já tira os espaços das pontas).
        assert_eq!(s.turns("c1"), vec![("  abre   o spotify pra mim  ".into(), "Pronto!".into())]);

        assert_eq!(s.rename("c1", "Música"), "Música");
        s.append("c2", "user", "oi");
        let again = Store::load(dir.path().to_path_buf());
        assert_eq!(again.title("c1").as_deref(), Some("Música"));
        assert_eq!(again.list().len(), 2);
        assert_eq!(again.list()[0].id, "c2", "mais recente primeiro");

        again.delete("c1");
        assert!(Store::load(dir.path().to_path_buf()).get("c1").is_none());
    }

    #[test]
    fn renamed_title_survives_and_titles_are_clean() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::load(dir.path().to_path_buf());
        s.rename("c1", "   ");
        assert_eq!(s.title("c1").as_deref(), Some(DEFAULT_TITLE));
        s.rename("c1", "Projeto X");
        assert_eq!(s.append("c1", "user", "primeira fala"), None, "não sobrescreve nome escolhido");
        assert!(auto_title(&"a".repeat(80)).ends_with('…'));
        assert!(!valid_id("../x") && !valid_id("") && valid_id("c1a-2"));
    }
}
