//! Coarse emotion tagging for Lilith's replies (ADR 0028 — embodied avatar).
//!
//! The avatar's facial expression is driven by an `emotion` string carried on
//! every `Command()` reply. This is the **v1, heuristic** classifier: it reads
//! the final reply text and picks one of a small fixed set. It is deliberately
//! not the LLM emitting a structured mood — parsing that out of free-form model
//! output is fragile, and a keyword pass over our own reply text is honest,
//! deterministic, and testable. Phase 2 can upgrade the source behind the same
//! `emotion` field without touching the bridge or the avatar.
//!
//! The set is intentionally tiny — the avatar (and the VRM expression presets)
//! only need a handful of distinct faces. `thinking` is owned by the *state*
//! channel (the avatar shows it while Lilith is busy), so it is not produced
//! here; this classifier describes how the finished reply *landed*.

/// Emotions the avatar can render. Kept as `&'static str` so it drops straight
/// into the reply JSON and maps 1:1 to VRM expression presets / fallback poses.
pub const NEUTRAL: &str = "neutral";
pub const HAPPY: &str = "happy";
pub const CONCERNED: &str = "concerned";

/// Classify the mood of a finished reply from its text.
///
/// Order matters: "concerned" markers win over "happy" ones, because a reply
/// that both apologises and mentions success ("desculpe, mas consegui…") should
/// read as the problem, not the win. Everything unmatched is `neutral`.
pub fn classify(reply: &str) -> &'static str {
    let r = reply.to_lowercase();

    // Trouble: errors, refusals, denials, can't-do-it. These are the replies
    // where the face should look apologetic / concerned.
    const CONCERNED_MARKERS: &[&str] = &[
        "não consigo",
        "nao consigo",
        "não entendi",
        "nao entendi",
        "não foi possível",
        "nao foi possivel",
        "não foi possivel",
        "desculpe",
        "erro",
        "falhou",
        "falha",
        "offline",
        "indisponível",
        "indisponivel",
        "negad",     // negado / negada (permission denied)
        "permissão", // "permissão negada"
        "permissao",
        "não pude",
        "nao pude",
        "parei após", // step-cap bailout
        "parei apos",
    ];
    if CONCERNED_MARKERS.iter().any(|m| r.contains(m)) {
        return CONCERNED;
    }

    // Wins: things got done. Action verbs in the past/gerund Lilith uses when
    // she actually carried something out, plus an exclamation mark (she only
    // really exclaims on good news).
    const HAPPY_MARKERS: &[&str] = &[
        "pronto",
        "feito",
        "abrindo",
        "abri ",
        "concluí",
        "conclui",
        "consegui",
        "aqui está",
        "aqui esta",
        "instalado",
        "instalando",
        "salvo",
        "capturei",
        "tirei",
        "!",
    ];
    if HAPPY_MARKERS.iter().any(|m| r.contains(m)) {
        return HAPPY;
    }

    NEUTRAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_and_refusals_are_concerned() {
        assert_eq!(
            classify("Não consigo falar com o modelo de IA agora."),
            CONCERNED
        );
        assert_eq!(classify("Não entendi o comando."), CONCERNED);
        assert_eq!(classify("Erro ao abrir o aplicativo."), CONCERNED);
        assert_eq!(classify("Permissão negada para essa ação."), CONCERNED);
        assert_eq!(classify("(parei após 8 passos) ..."), CONCERNED);
    }

    #[test]
    fn completions_are_happy() {
        assert_eq!(classify("Pronto! Firefox aberto."), HAPPY);
        assert_eq!(classify("Abrindo o navegador para você."), HAPPY);
        assert_eq!(classify("Screenshot salvo em ~/Imagens."), HAPPY);
        assert_eq!(classify("Consegui instalar o GIMP."), HAPPY);
    }

    #[test]
    fn plain_statements_are_neutral() {
        assert_eq!(classify("O céu é azul porque a luz se espalha."), NEUTRAL);
        assert_eq!(classify("São 14h30."), NEUTRAL);
        assert_eq!(classify(""), NEUTRAL);
    }

    #[test]
    fn concerned_beats_happy_when_both_present() {
        // Apology + a success word: the problem should win the face.
        assert_eq!(classify("Desculpe, mas consegui só parte."), CONCERNED);
    }
}
