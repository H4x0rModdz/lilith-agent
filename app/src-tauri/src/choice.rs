//! Pedidos de escolha: quando falta um detalhe que só o usuário sabe (qual
//! dispositivo de entrada, qual conta…), o app mostra no chat um cartão com as
//! opções numa lista suspensa — como as aprovações — e espera. O usuário escolhe
//! ali ou responde escrevendo (ou, mais tarde, falando) o nome da opção.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

/// Quanto tempo o cartão espera uma resposta.
const CHOICE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Serialize, Clone)]
struct ChoiceRequest {
    id: u64,
    conversation: String,
    title: String,
    options: Vec<String>,
}

struct Pending {
    conversation: String,
    options: Vec<String>,
    tx: oneshot::Sender<Option<String>>,
}

/// Escolhas aguardando resposta.
#[derive(Default)]
pub struct Choices {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, Pending>>,
}

/// Minúsculas, sem acento: para casar o que o usuário escreveu com a opção.
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' => 'a',
            'é' | 'ê' => 'e',
            'í' => 'i',
            'ó' | 'ô' | 'õ' => 'o',
            'ú' => 'u',
            'ç' => 'c',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Qual opção o texto escolhe: número ("2", "a segunda") ou parte do nome que
/// só uma opção tem. `None` se não der para saber.
pub(crate) fn match_option(options: &[String], text: &str) -> Option<String> {
    let t = fold(text.trim().trim_end_matches(['.', '!', '?']));
    if t.is_empty() {
        return None;
    }
    const ORDINALS: &[&str] = &["primeira", "segunda", "terceira", "quarta", "quinta", "sexta"];
    let index = t
        .split_whitespace()
        .find_map(|w| w.parse::<usize>().ok().filter(|n| *n >= 1).or_else(|| ORDINALS.iter().position(|o| w.starts_with(o)).map(|i| i + 1)));
    if let Some(n) = index {
        if t.split_whitespace().count() <= 3 {
            return options.get(n - 1).cloned();
        }
    }
    let exact: Vec<&String> = options.iter().filter(|o| fold(o) == t).collect();
    if exact.len() == 1 {
        return Some(exact[0].clone());
    }
    let partial: Vec<&String> = options.iter().filter(|o| t.chars().count() >= 3 && fold(o).contains(&t)).collect();
    (partial.len() == 1).then(|| partial[0].clone())
}

impl Choices {
    fn open(&self, conversation: &str, options: Vec<String>) -> (u64, oneshot::Receiver<Option<String>>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, Pending { conversation: conversation.into(), options, tx });
        (id, rx)
    }

    /// Resposta pelo cartão. `None` = cancelou. Valor fora das opções é ignorado.
    pub fn answer(&self, id: u64, value: Option<String>) -> bool {
        let mut pending = self.pending.lock().unwrap();
        let ok = match (&value, pending.get(&id)) {
            (Some(v), Some(p)) => p.options.contains(v),
            (None, Some(_)) => true,
            _ => false,
        };
        if !ok {
            return false;
        }
        pending.remove(&id).is_some_and(|p| p.tx.send(value).is_ok())
    }

    /// Resposta escrita no chat: se há uma escolha esperando nesta conversa e o
    /// texto aponta uma opção, resolve e devolve (id, opção).
    pub fn answer_by_text(&self, conversation: &str, text: &str) -> Option<(u64, String)> {
        let mut pending = self.pending.lock().unwrap();
        let (id, chosen) = pending
            .iter()
            .filter(|(_, p)| p.conversation == conversation)
            .find_map(|(id, p)| match_option(&p.options, text).map(|c| (*id, c)))?;
        let p = pending.remove(&id)?;
        p.tx.send(Some(chosen.clone())).ok()?;
        Some((id, chosen))
    }

    /// Mostra o cartão e espera a escolha (ou o tempo acabar → `None`).
    pub async fn ask(&self, app: &AppHandle, conversation: &str, title: &str, options: Vec<String>) -> Option<String> {
        if options.is_empty() {
            return None;
        }
        let (id, rx) = self.open(conversation, options.clone());
        crate::windows::show_chat(app);
        let _ = app.emit(
            "lilith://choice",
            ChoiceRequest { id, conversation: conversation.into(), title: title.into(), options },
        );
        let answer = match tokio::time::timeout(CHOICE_TIMEOUT, rx).await {
            Ok(Ok(v)) => v,
            _ => {
                self.pending.lock().unwrap().remove(&id);
                None
            }
        };
        let _ = app.emit("lilith://choice-resolved", serde_json::json!({ "id": id, "value": answer }));
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_answers_pick_one_option() {
        let opts = vec![
            "Padrão do Windows JBL Tune 770NC".to_string(),
            "JBL Tune 770NC Auscultadores".to_string(),
            "NVIDIA High Definition Audio U34G4C".to_string(),
        ];
        assert_eq!(match_option(&opts, "nvidia").as_deref(), Some("NVIDIA High Definition Audio U34G4C"));
        assert_eq!(match_option(&opts, "a segunda").as_deref(), Some("JBL Tune 770NC Auscultadores"));
        assert_eq!(match_option(&opts, "1").as_deref(), Some("Padrão do Windows JBL Tune 770NC"));
        assert_eq!(match_option(&opts, "padrao do windows").as_deref(), Some("Padrão do Windows JBL Tune 770NC"));
        assert_eq!(match_option(&opts, "jbl"), None, "duas opções têm 'jbl': ambíguo");
        assert_eq!(match_option(&opts, "abre o spotify"), None);
    }

    #[test]
    fn answers_must_be_one_of_the_options() {
        let c = Choices::default();
        let (id, _rx) = c.open("conv", vec!["A".into(), "B".into()]);
        assert!(!c.answer(id, Some("Z".into())), "fora da lista não vale");
        assert!(c.answer(id, Some("B".into())));
        let (_id2, _rx2) = c.open("conv", vec!["Microfone X".into(), "Microfone Y".into()]);
        assert_eq!(c.answer_by_text("outra", "y"), None, "só na mesma conversa");
        assert_eq!(c.answer_by_text("conv", "microfone y").map(|(_, v)| v).as_deref(), Some("Microfone Y"));
    }
}
