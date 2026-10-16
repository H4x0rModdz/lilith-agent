//! Celular (Vincular ao Celular) e atalhos nativos do Windows (páginas das
//! Configurações, recorte de tela): as tools, os atalhos sem IA, a resposta
//! montada pelo código a partir do resultado e a correção do plano do
//! intérprete. Num módulo só para mexer pouco nos arquivos compartilhados.

use crate::shortcut::Shortcut;
use crate::tools::{Tool, ToolCall};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

/// Tools deste módulo (entram no fim do catálogo).
pub fn tools() -> Vec<Tool> {
    vec![
        // ── celular (Vincular ao Celular) ───────────────────────────────
        Tool {
            action: "phone.status",
            description: "Estado do CELULAR do usuário pelo app Vincular ao Celular: se está conectado, \
                          bateria (%), se está carregando e o sinal. Use para 'como tá a bateria do meu \
                          celular', 'meu celular tá conectado?'.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "phone.notifications",
            description: "Notificações recentes do CELULAR (app + resumo), pelo Vincular ao Celular. Use para \
                          'tem notificação no celular?'. Não é a central de notificações do PC.",
            schema: obj(json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 15 } }), &[]),
        },
        Tool {
            action: "phone.messages",
            description: "Conversas de SMS mais recentes do CELULAR (quem + prévia da última mensagem), pelo \
                          Vincular ao Celular. Use para 'lê minhas mensagens do celular', 'tenho SMS novo?'. \
                          WhatsApp/Discord/Teams do PC NÃO são isto.",
            schema: obj(json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 15 } }), &[]),
        },
        Tool {
            action: "phone.calls",
            description: "Chamadas recentes do CELULAR (quem ligou, perdidas), pelo Vincular ao Celular. Use \
                          para 'quem me ligou?'.",
            schema: obj(json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 15 } }), &[]),
        },
        Tool {
            action: "phone.send_sms",
            description: "Manda um SMS pelo CELULAR (Vincular ao Celular). `contact`: nome do contato ou número; \
                          `text`: a mensagem exata. Se o nome bater com mais de um contato, o app pergunta ao \
                          usuário. Só para SMS/torpedo/'pelo celular'; WhatsApp, Discord e Teams usam \
                          ui.open_chat + ui.type.",
            schema: obj(
                json!({ "contact": { "type": "string" }, "text": { "type": "string" } }),
                &["contact", "text"],
            ),
        },
        // ── apps nativos do Windows ─────────────────────────────────────
        Tool {
            action: "settings.open",
            description: "Abre uma página das CONFIGURAÇÕES do Windows. `page` em palavras: bluetooth, wi-fi, \
                          rede, som, tela, brilho, luz noturna, notificações, não perturbe, apps, apps padrão, \
                          energia, bateria, armazenamento, modo escuro, papel de parede, barra de tarefas, \
                          windows update, privacidade, microfone, câmera, teclado, mouse, impressoras, data e \
                          hora, idioma… Vazio = início das Configurações.",
            schema: obj(json!({ "page": { "type": "string" } }), &[]),
        },
        Tool {
            action: "screen.snip",
            description: "Abre o recorte de tela do Windows (o mesmo do Win+Shift+S): o usuário escolhe a área e \
                          o recorte vai para a área de transferência. Use para 'print de uma parte da tela', 'recorta a tela' \n                          ('tira um print' sem parte = `screen.capture`).",
            schema: obj(json!({}), &[]),
        },
    ]
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex de atalho válida"))
}

fn one(action: &str, params: Value, reply: &str) -> Shortcut {
    Shortcut { calls: vec![ToolCall { action: action.into(), params }], reply: reply.into() }
}

/// Atalhos sem IA. `t` já vem normalizado por `shortcut::parse` (minúsculas,
/// sem pontuação final nem "por favor").
pub(crate) fn parse(t: &str) -> Option<Shortcut> {
    const PHONE: &str = r"(?:o\s+|meu\s+)?(?:celular|telefone|smartphone|cel)";
    static STATUS: OnceLock<Regex> = OnceLock::new();
    static NOTIF: OnceLock<Regex> = OnceLock::new();
    static MSGS: OnceLock<Regex> = OnceLock::new();
    static CALLS: OnceLock<Regex> = OnceLock::new();
    static SETTINGS: OnceLock<Regex> = OnceLock::new();
    static SETTINGS_DIRECT: OnceLock<Regex> = OnceLock::new();
    static SNIP: OnceLock<Regex> = OnceLock::new();

    // ── celular ──────────────────────────────────────────────────────────
    let status = re(
        &STATUS,
        &format!(
            r"^(?:como\s+(?:t[aá]|est[aá])\s+a\s+bateria\s+d[oe]\s+{PHONE}|quanto\s+(?:de\s+bateria\s+)?(?:tem|t[aá]|est[aá])\s+(?:a\s+bateria\s+d[oe]\s+)?{PHONE}|(?:qual\s+[eé]\s+a\s+)?bateria\s+d[oe]\s+{PHONE}|{PHONE}\s+(?:t[aá]|est[aá])\s+conectado|status\s+d[oe]\s+{PHONE}|como\s+(?:t[aá]|est[aá])\s+{PHONE})$"
        ),
    );
    if status.is_match(t) {
        return Some(one("phone.status", json!({}), "Pronto!"));
    }
    let notif = re(
        &NOTIF,
        &format!(
            r"^(?:tem\s+(?:alguma\s+)?notifica[cç](?:[aã]o|[oõ]es)(?:\s+novas?)?|(?:l[eê]|mostra|quais\s+(?:s[aã]o\s+)?)\s+(?:as\s+)?(?:minhas\s+)?notifica[cç][oõ]es)\s+(?:no|do)\s+{PHONE}$"
        ),
    );
    if notif.is_match(t) {
        return Some(one("phone.notifications", json!({}), "Pronto!"));
    }
    let msgs = re(
        &MSGS,
        &format!(
            r"^(?:(?:l[eê]|mostra|quais\s+(?:s[aã]o\s+)?)\s+(?:as\s+)?(?:minhas\s+)?(?:[uú]ltimas\s+)?(?:mensagens|sms)\s+(?:no|do)\s+{PHONE}|tem\s+(?:alguma\s+)?(?:mensage(?:m|ns)|sms)(?:\s+nov[oa]s?)?\s+(?:no|do)\s+{PHONE}|(?:l[eê]|mostra)\s+(?:os\s+)?(?:meus\s+)?sms)$"
        ),
    );
    if msgs.is_match(t) {
        return Some(one("phone.messages", json!({}), "Pronto!"));
    }
    let calls = re(
        &CALLS,
        &format!(
            r"^(?:quem\s+(?:me\s+)?ligou|(?:mostra|quais|l[eê])\s+(?:as\s+)?(?:[uú]ltimas\s+)?(?:chamadas|liga[cç][oõ]es)(?:\s+recentes)?|tem\s+(?:alguma\s+)?(?:chamada|liga[cç][aã]o)\s+perdida)(?:\s+(?:no|do|pro)\s+{PHONE})?$"
        ),
    );
    if calls.is_match(t) {
        return Some(one("phone.calls", json!({}), "Pronto!"));
    }

    // ── Configurações do Windows ─────────────────────────────────────────
    let settings = re(
        &SETTINGS,
        r"^(?:abre|abra|abrir|mostra|vai\s+(?:nas|pras|para\s+as))\s+(?:as\s+|a\s+)?(?:configura[cç](?:[oõ]es|[aã]o)|config|ajustes|defini[cç][oõ]es)(?:\s+(?:de|do|da|dos|das)\s+(.{1,40}?))?$",
    );
    if let Some(c) = settings.captures(t) {
        let page = c.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        return Some(one("settings.open", json!({ "page": page }), "Abri!"));
    }
    let direct = re(&SETTINGS_DIRECT, r"^(?:abre|abra|mostra)\s+(?:o\s+)?(bluetooth|wi-?fi|windows\s+update)$");
    if let Some(c) = direct.captures(t) {
        return Some(one("settings.open", json!({ "page": &c[1] }), "Abri!"));
    }

    // ── recorte de tela ──────────────────────────────────────────────────
    let snip = re(
        &SNIP,
        r"^(?:(?:tira|tirar|faz|fazer|fa[cç]a)\s+(?:um\s+|uma\s+)?(?:recorte(?:\s+de\s+tela)?|(?:print|printscreen|screenshot|captura(?:\s+de\s+tela)?)\s+de\s+(?:uma\s+)?(?:parte|[aá]rea|peda[cç]o))(?:\s+da\s+(?:minha\s+)?tela)?|(?:abre\s+(?:o\s+|a\s+)?)?(?:recorte\s+de\s+tela|ferramenta\s+de\s+(?:captura|recorte))|recorta\s+(?:a\s+)?tela)$",
    );
    if snip.is_match(t) {
        return Some(one("screen.snip", json!({}), "Pronto, é só selecionar a área."));
    }
    None
}

fn list_lines(items: Option<&Vec<Value>>, head: &str, rest: &str) -> Vec<String> {
    items
        .map(|a| {
            a.iter()
                .map(|v| {
                    let (h, r) = (v[head].as_str().unwrap_or("").trim(), v[rest].as_str().unwrap_or("").trim());
                    if r.is_empty() { format!("- {h}") } else { format!("- {h}: {r}") }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Resposta montada pelo código a partir do resultado (sem modelo).
pub(crate) fn render(action: &str, result: &Value) -> Option<String> {
    let text = |k: &str| result[k].as_str().unwrap_or("").trim().to_string();
    match action {
        "phone.status" => Some(if result["paired"] == false {
            "O Vincular ao Celular ainda não está ligado a um celular. Abre o app e faz o pareamento que eu passo a ver tudo.".into()
        } else if result["connected"] != true {
            "Seu celular está desconectado do Vincular ao Celular agora — confere o Bluetooth/Wi-Fi e o app no celular.".into()
        } else {
            let mut parts: Vec<String> = Vec::new();
            match result["battery"].as_u64() {
                Some(b) if result["charging"] == true => parts.push(format!("bateria em {b}% (carregando)")),
                Some(b) => parts.push(format!("bateria em {b}%")),
                None => {}
            }
            if let Some(s) = result["signal"].as_str().filter(|s| !s.trim().is_empty()) {
                parts.push(format!("sinal {s}"));
            }
            if parts.is_empty() {
                "Seu celular está conectado, mas o app não mostrou a bateria agora.".into()
            } else {
                format!("Seu celular está conectado: {}.", parts.join(", "))
            }
        }),
        "phone.notifications" => {
            let lines = list_lines(result["notifications"].as_array(), "app", "text");
            Some(if lines.is_empty() {
                "Nenhuma notificação no celular agora.".into()
            } else {
                format!("Notificações do celular:\n{}", lines.join("\n"))
            })
        }
        "phone.messages" => {
            let lines = list_lines(result["conversations"].as_array(), "who", "preview");
            Some(if lines.is_empty() {
                "Não vi conversas de SMS no celular.".into()
            } else {
                format!("Conversas recentes no celular:\n{}", lines.join("\n"))
            })
        }
        "phone.calls" => {
            let lines = list_lines(result["calls"].as_array(), "who", "detail");
            Some(if !lines.is_empty() {
                format!("Chamadas recentes:\n{}", lines.join("\n"))
            } else if result["needs_permission"] == true {
                "O Vincular ao Celular ainda não tem permissão para mostrar as chamadas — libera no app do celular.".into()
            } else {
                "Nenhuma chamada recente no celular.".into()
            })
        }
        "phone.send_sms" => Some(if result["cancelled"] == true {
            "Ok, não mandei nada.".into()
        } else if result["sent"] != true {
            "Não mandei o SMS.".into()
        } else if result["verified"] == true {
            format!("SMS enviado para {}.", text("to"))
        } else {
            format!("Mandei o SMS para {}, mas não consegui confirmar na conversa — dá uma olhada no Vincular ao Celular.", text("to"))
        }),
        "settings.open" => Some(if result["fallback"] == true {
            format!("Não achei uma página exata para \"{}\", então abri o início das Configurações.", text("asked"))
        } else if result["verified"] == false {
            format!("Pedi para abrir as Configurações ({}), mas não vi a janela aparecer.", text("opened"))
        } else if text("opened") == "Início" {
            "Abri as Configurações.".into()
        } else {
            format!("Abri as Configurações em {}.", text("opened"))
        }),
        "screen.snip" => Some(if result["verified"] == false {
            "Pedi o recorte de tela, mas não vi ele abrir — tenta Win+Shift+S.".into()
        } else {
            "Pronto, é só selecionar a área — o recorte vai para a área de transferência.".into()
        }),
        _ => None,
    }
}

fn has(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

/// Correção do plano do intérprete para pedidos do celular e das
/// Configurações. `None` = não é com este módulo (o plano segue como veio).
pub(crate) fn refine(t: &str, plan: &[String]) -> Option<Vec<String>> {
    let phone = has(t, &["celular", "sms", "torpedo", "telefone", "vincular ao celular"]);
    let other_app = has(t, &["whatsapp", "zap", "discord", "teams", "telegram", "slack"]);
    let already = plan.iter().any(|p| p.starts_with("phone."));
    if phone && !other_app && !already {
        let send = has(t, &["manda", "mande", "envia", "enviar", "diz ", "diga", "fala pr", "avisa", "responde", "pergunta"]);
        let about_messages = has(t, &["mensage", "sms", "torpedo"]);
        if send && (about_messages || plan.iter().any(|p| p == "ui.open_chat" || p == "ui.type")) {
            return Some(vec!["phone.send_sms".into()]);
        }
        let read = has(t, &["lê ", "le ", "ler ", "leia", "mostra", "quais", "tem ", "tenho", "o que", "chegou", "novas", "novo"]);
        if read && has(t, &["notifica"]) {
            return Some(vec!["phone.notifications".into()]);
        }
        if read && about_messages {
            return Some(vec!["phone.messages".into()]);
        }
        if has(t, &["ligou", "ligação", "ligacao", "ligações", "ligacoes", "chamada"]) {
            return Some(vec!["phone.calls".into()]);
        }
        if has(t, &["bateria", "conectado", "sinal"]) {
            return Some(vec!["phone.status".into()]);
        }
    }
    // "Abre as configurações de X" não é um programa chamado X.
    if has(t, &["configuraç", "configurac"]) && plan.iter().any(|p| p == "app.open") {
        // Só o primeiro: "abre as configurações de energia e o spotify" ainda abre o Spotify.
        let mut out = plan.to_vec();
        if let Some(i) = out.iter().position(|p| p == "app.open") {
            out[i] = "settings.open".into();
        }
        return Some(out);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Option<(String, Value)> {
        crate::shortcut::parse(text).map(|s| (s.calls[0].action.clone(), s.calls[0].params.clone()))
    }

    #[test]
    fn phone_questions_are_decided_by_code() {
        assert_eq!(one("como tá a bateria do meu celular?").unwrap().0, "phone.status");
        assert_eq!(one("quanto de bateria tem o celular").unwrap().0, "phone.status");
        assert_eq!(one("meu celular tá conectado?").unwrap().0, "phone.status");
        assert_eq!(one("tem notificação no celular?").unwrap().0, "phone.notifications");
        assert_eq!(one("lê as notificações do meu celular").unwrap().0, "phone.notifications");
        assert_eq!(one("lê minhas mensagens do celular").unwrap().0, "phone.messages");
        assert_eq!(one("tem sms novo no celular?").unwrap().0, "phone.messages");
        assert_eq!(one("quem me ligou?").unwrap().0, "phone.calls");
        assert_eq!(one("mostra as últimas chamadas do celular").unwrap().0, "phone.calls");
        // O PC continua com o atalho dele.
        assert_eq!(one("como tá meu pc").unwrap().0, "system.status");
        // Mandar SMS precisa do texto exato: vai pelo modelo.
        assert_eq!(one("manda um sms pro user x dizendo oi"), None);
    }

    #[test]
    fn settings_pages_and_snip_are_decided_by_code() {
        assert_eq!(one("abre as configurações de bluetooth"), Some(("settings.open".into(), json!({ "page": "bluetooth" }))));
        assert_eq!(one("abre as configurações de som por favor").unwrap().1, json!({ "page": "som" }));
        assert_eq!(one("abre as configurações").unwrap().1, json!({ "page": "" }));
        assert_eq!(one("abre a configuração do modo escuro").unwrap().1, json!({ "page": "modo escuro" }));
        assert_eq!(one("abre o wi-fi").unwrap().1, json!({ "page": "wi-fi" }));
        assert_eq!(one("abre o bloco de notas").unwrap().0, "app.open", "programa continua igual");
        assert_eq!(one("tira um print").unwrap().0, "screen.capture", "print inteiro não é recorte");
        assert_eq!(one("tira um print de uma parte da tela").unwrap().0, "screen.snip");
        assert_eq!(one("faz uma captura de uma área").unwrap().0, "screen.snip");
        assert_eq!(one("abre a ferramenta de captura").unwrap().0, "screen.snip");
        assert_eq!(one("recorta a tela").unwrap().0, "screen.snip");
    }

    #[test]
    fn phone_replies_are_rendered() {
        let ok = json!({ "paired": true, "connected": true, "battery": 76, "charging": false, "signal": "3 de 4" });
        assert_eq!(render("phone.status", &ok).unwrap(), "Seu celular está conectado: bateria em 76%, sinal 3 de 4.");
        let charging = json!({ "paired": true, "connected": true, "battery": 15, "charging": true });
        assert_eq!(render("phone.status", &charging).unwrap(), "Seu celular está conectado: bateria em 15% (carregando).");
        assert!(render("phone.status", &json!({ "paired": false })).unwrap().contains("pareamento"));
        assert!(render("phone.status", &json!({ "paired": true, "connected": false })).unwrap().contains("desconectado"));
        let n = json!({ "notifications": [{ "app": "App A", "text": "algo novo" }, { "app": "App B", "text": "" }] });
        assert_eq!(render("phone.notifications", &n).unwrap(), "Notificações do celular:\n- App A: algo novo\n- App B");
        assert_eq!(render("phone.notifications", &json!({ "notifications": [] })).unwrap(), "Nenhuma notificação no celular agora.");
        let m = json!({ "conversations": [{ "who": "user x", "preview": "oi" }] });
        assert_eq!(render("phone.messages", &m).unwrap(), "Conversas recentes no celular:\n- user x: oi");
        assert!(render("phone.calls", &json!({ "calls": [], "needs_permission": true })).unwrap().contains("permissão"));
        assert_eq!(render("phone.send_sms", &json!({ "sent": true, "to": "user x", "verified": true })).unwrap(), "SMS enviado para user x.");
        assert!(render("phone.send_sms", &json!({ "sent": true, "to": "user x", "verified": false })).unwrap().contains("não consegui confirmar"));
        assert_eq!(render("phone.send_sms", &json!({ "cancelled": true })).unwrap(), "Ok, não mandei nada.");
    }

    #[test]
    fn settings_replies_are_rendered() {
        let r = json!({ "opened": "Bluetooth e dispositivos", "verified": true, "fallback": false });
        assert_eq!(render("settings.open", &r).unwrap(), "Abri as Configurações em Bluetooth e dispositivos.");
        let f = json!({ "opened": "Início", "asked": "xyz", "verified": true, "fallback": true });
        assert!(render("settings.open", &f).unwrap().starts_with("Não achei uma página exata para \"xyz\""));
        assert!(render("settings.open", &json!({ "opened": "Som", "verified": false })).unwrap().contains("não vi"));
    }

    #[test]
    fn plans_about_the_phone_are_fixed() {
        let p = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(refine("manda um sms pro user x dizendo que já chego", &p(&["ui.open_chat", "ui.type"])), Some(p(&["phone.send_sms"])));
        assert_eq!(refine("manda mensagem pelo celular pro user y", &p(&["ui.open_chat", "ui.type"])), Some(p(&["phone.send_sms"])));
        assert_eq!(refine("manda oi pro user x no whatsapp do celular", &p(&["ui.open_chat", "ui.type"])), None, "whatsapp é chat do PC");
        assert_eq!(refine("chegou alguma notificação no celular?", &p(&["window.read"])), Some(p(&["phone.notifications"])));
        assert_eq!(refine("quem ligou no meu celular hoje", &p(&[])), Some(p(&["phone.calls"])));
        assert_eq!(refine("a bateria do celular tá boa?", &p(&["system.status"])), Some(p(&["phone.status"])));
        assert_eq!(refine("lê meus sms", &p(&["phone.messages"])), None, "já está certo");
        assert_eq!(refine("abre as configurações de energia e o spotify", &p(&["app.open", "app.open"])), Some(p(&["settings.open", "app.open"])));
        assert_eq!(refine("abre o spotify", &p(&["app.open"])), None);
    }
}
