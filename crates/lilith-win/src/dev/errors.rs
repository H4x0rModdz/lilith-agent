//! Erros de build estruturados por código (sem modelo) a partir do texto que
//! aparece no Visual Studio (Lista de Erros, Saída), no VS Code e em
//! terminais. Formatos:
//! - MSBuild/C#/VB/tsc: `arquivo(linha,col): error CS1234: msg [projeto]`
//!   (e sem local: `CSC : error CS5001: …`, `error NU1101: …`);
//! - tsc --pretty: `arquivo:linha:col - error TS1234: msg`;
//! - rustc/cargo: `error[E0425]: msg` e, linhas depois, `--> arquivo:linha:col`;
//! - gcc/clang e afins: `arquivo:linha:col: error: msg`;
//! - npm: bloco `npm ERR!` / `npm error` (vira um diagnóstico só);
//! - Node: `TypeError: msg` seguido de `at … (arquivo:linha:col)`.
//!
//! Linhas da Lista de Erros (células lidas por acessibilidade) passam por
//! [`parse_rows`]. O texto lido é de terceiros: mensagens são cortadas.

use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::OnceLock;

/// Teto de diagnósticos guardados (a resposta mostra só os primeiros).
pub const MAX_DIAGNOSTICS: usize = 40;
/// Mensagens curtas: texto de terceiros não vira veículo de prompt injection.
const MAX_MESSAGE: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// Um erro/aviso de build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// csharp, typescript, rust, msbuild, npm, node, cpp…
    pub tool: &'static str,
    pub severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl Diagnostic {
    fn new(tool: &'static str, severity: Severity, code: Option<&str>, message: &str) -> Self {
        Diagnostic {
            tool,
            severity,
            code: code.map(str::to_string).filter(|c| !c.is_empty()),
            message: clip(message),
            file: None,
            line: None,
            col: None,
            project: None,
        }
    }

    fn at(mut self, file: &str, line: Option<&str>, col: Option<&str>) -> Self {
        let file = file.trim().trim_matches('"');
        if !file.is_empty() {
            self.file = Some(file.to_string());
        }
        self.line = line.and_then(|l| l.parse().ok());
        self.col = col.and_then(|c| c.parse().ok());
        self
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex de erro de build válida"))
}

fn clip(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= MAX_MESSAGE {
        s
    } else {
        s.chars().take(MAX_MESSAGE).collect::<String>() + "…"
    }
}

/// Ferramenta pelo prefixo do código (CS0103 → csharp).
fn tool_for_code(code: &str) -> &'static str {
    let prefix: String = code.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase();
    match prefix.as_str() {
        "CS" => "csharp",
        "TS" => "typescript",
        "BC" => "vb",
        "FS" => "fsharp",
        "C" | "LNK" => "cpp",
        "E" => "rust",
        "CA" | "IDE" => "analyzer",
        _ => "msbuild", // MSB, NU, NETSDK…
    }
}

fn severity_of(word: &str) -> Option<Severity> {
    let w = word.trim().to_lowercase();
    if w.contains("error") || w == "erro" || w == "erros" {
        Some(Severity::Error)
    } else if w.contains("warning") || w == "aviso" || w == "avisos" {
        Some(Severity::Warning)
    } else {
        None
    }
}

/// Tira cores ANSI, os rótulos da leitura da janela ("[item] ") e o prefixo de
/// nó do MSBuild ("12>").
fn clean_line(raw: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    static LABEL: OnceLock<Regex> = OnceLock::new();
    static NODE: OnceLock<Regex> = OnceLock::new();
    let s = re(&ANSI, r"\x1b\[[0-9;?]*[A-Za-z]").replace_all(raw, "");
    let s = re(&LABEL, r"^\s*\[(?:texto|link|botão|item|árvore|status|aviso)\]\s*").replace(&s, "");
    let s = re(&NODE, r"^\s*\d{1,3}>\s*").replace(&s, "");
    s.trim_end().to_string()
}

/// Nome do projeto a partir de "[C:\src\Demo\Demo.csproj]".
fn project_name(raw: &str) -> Option<String> {
    let last = raw.trim().rsplit(['\\', '/']).next()?.trim();
    let stem = last.rsplit_once('.').map(|(s, _)| s).unwrap_or(last);
    (!stem.is_empty()).then(|| stem.to_string())
}

/// Bloco de erro do npm vira um diagnóstico: o código (`code X`) e a primeira
/// linha que explica algo.
fn npm_diagnostic(lines: &[String]) -> Option<Diagnostic> {
    const NOISE: &[&str] = &[
        "code ", "errno ", "path ", "syscall ", "command ", "signal ", "cwd ", "a complete log", "this is probably",
        "there is likely", "exit status", "log files", "to see a list", "did you mean", "for a full report", "npm err!",
        "lifecycle",
    ];
    static CODE: OnceLock<Regex> = OnceLock::new();
    let code = lines.iter().find_map(|l| re(&CODE, r"^code\s+(\S+)").captures(l).map(|c| c[1].to_string()));
    let message = lines
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !NOISE.iter().any(|n| l.to_lowercase().starts_with(n)))
        .map(str::to_string)
        .or_else(|| code.clone())?;
    Some(Diagnostic::new("npm", Severity::Error, code.as_deref(), &message))
}

/// Quadro da pilha de dependência ou do próprio Node: não é o código do usuário.
fn internal_frame(loc: &str) -> bool {
    let l = loc.to_lowercase();
    l.starts_with("node:") || l.contains("node_modules") || l.starts_with("internal/")
}

#[derive(Default)]
struct Parser {
    out: Vec<Diagnostic>,
    /// rustc: cabeçalho esperando o `--> arquivo:linha:col`.
    rust: Option<Diagnostic>,
    /// Node: erro esperando os quadros `at …` (sem quadro, não era do Node).
    node: Option<(Diagnostic, bool)>,
    /// Linhas do bloco `npm ERR!` em andamento.
    npm: Vec<String>,
}

impl Parser {
    fn flush_rust(&mut self) {
        if let Some(d) = self.rust.take() {
            self.out.push(d);
        }
    }

    fn flush_node(&mut self) {
        if let Some((d, seen_frame)) = self.node.take() {
            if seen_frame {
                self.out.push(d);
            }
        }
    }

    fn flush_npm(&mut self) {
        if !self.npm.is_empty() {
            let lines = std::mem::take(&mut self.npm);
            if let Some(d) = npm_diagnostic(&lines) {
                self.out.push(d);
            }
        }
    }

    fn line(&mut self, raw: &str) {
        static MSBUILD: OnceLock<Regex> = OnceLock::new();
        static MSBUILD_NOLOC: OnceLock<Regex> = OnceLock::new();
        static TSC_PRETTY: OnceLock<Regex> = OnceLock::new();
        static RUST_HEAD: OnceLock<Regex> = OnceLock::new();
        static RUST_LOC: OnceLock<Regex> = OnceLock::new();
        static GCC: OnceLock<Regex> = OnceLock::new();
        static NPM: OnceLock<Regex> = OnceLock::new();
        static NODE_ERR: OnceLock<Regex> = OnceLock::new();
        static NODE_AT: OnceLock<Regex> = OnceLock::new();

        let l = clean_line(raw);
        let t = l.trim();

        if let Some(c) = re(&NPM, r"^npm\s+(?:ERR!|error)\s?(?P<msg>.*)$").captures(t) {
            self.flush_rust();
            self.flush_node();
            self.npm.push(c["msg"].trim().to_string());
            return;
        }
        self.flush_npm();

        if let Some(c) = re(&RUST_LOC, r"^-->\s*(?P<file>.+?):(?P<line>\d+):(?P<col>\d+)$").captures(t) {
            if let Some(d) = self.rust.as_mut() {
                if d.file.is_none() {
                    *d = d.clone().at(&c["file"], Some(&c["line"]), Some(&c["col"]));
                    d.tool = "rust";
                }
            }
            return;
        }
        if let Some(c) = re(&NODE_AT, r"^at\s+(?:.*?\()?(?P<loc>[^()]+?):(?P<line>\d+):(?P<col>\d+)\)?$").captures(t) {
            if let Some((d, seen)) = self.node.as_mut() {
                *seen = true;
                if d.file.is_none() && !internal_frame(&c["loc"]) {
                    *d = d.clone().at(c["loc"].trim_start_matches("file:///"), Some(&c["line"]), Some(&c["col"]));
                }
            }
            return;
        }
        if t.is_empty() {
            return;
        }
        // Qualquer outra linha encerra a pilha do Node.
        self.flush_node();

        if let Some(c) = re(
            &MSBUILD,
            r"^(?P<file>.+?)\((?P<line>\d+)(?:,(?P<col>\d+))?(?:,\d+){0,2}\)\s*:\s*(?P<sev>fatal error|error|warning)\s+(?P<code>[A-Za-z]{1,8}\d{1,6})\s*:\s*(?P<msg>.*?)\s*(?:\[(?P<proj>[^\[\]]+)\])?$",
        )
        .captures(t)
        {
            self.flush_rust();
            let code = &c["code"];
            let mut d = Diagnostic::new(tool_for_code(code), severity_of(&c["sev"]).unwrap_or(Severity::Error), Some(code), &c["msg"])
                .at(&c["file"], Some(&c["line"]), c.name("col").map(|m| m.as_str()));
            d.project = c.name("proj").and_then(|p| project_name(p.as_str()));
            self.out.push(d);
            return;
        }
        if let Some(c) = re(
            &TSC_PRETTY,
            r"^(?P<file>.+?):(?P<line>\d+):(?P<col>\d+)\s+-\s+(?P<sev>error|warning)\s+(?P<code>TS\d+)\s*:\s*(?P<msg>.+)$",
        )
        .captures(t)
        {
            self.flush_rust();
            let d = Diagnostic::new("typescript", severity_of(&c["sev"]).unwrap_or(Severity::Error), Some(&c["code"]), &c["msg"])
                .at(&c["file"], Some(&c["line"]), Some(&c["col"]));
            self.out.push(d);
            return;
        }
        if let Some(c) = re(
            &MSBUILD_NOLOC,
            r"^(?:(?P<origin>.{1,260}?)\s+:\s+)?(?P<sev>fatal error|error|warning)\s+(?P<code>[A-Za-z]{1,8}\d{1,6})\s*:\s*(?P<msg>.+?)\s*(?:\[(?P<proj>[^\[\]]+)\])?$",
        )
        .captures(t)
        {
            self.flush_rust();
            let code = &c["code"];
            let mut d = Diagnostic::new(tool_for_code(code), severity_of(&c["sev"]).unwrap_or(Severity::Error), Some(code), &c["msg"]);
            // Origem com cara de arquivo (".csproj", caminho) vira o arquivo.
            if let Some(origin) = c.name("origin").map(|m| m.as_str().trim()) {
                if origin.contains(['\\', '/', '.']) {
                    d = d.at(origin, None, None);
                }
            }
            d.project = c.name("proj").and_then(|p| project_name(p.as_str()));
            self.out.push(d);
            return;
        }
        if let Some(c) = re(&RUST_HEAD, r"^(?P<sev>error|warning)(?:\[(?P<code>E\d{4})\])?:\s*(?P<msg>.+)$").captures(t) {
            self.flush_rust();
            let msg = c["msg"].to_lowercase();
            // Resumos do cargo ("could not compile", "generated 3 warnings") não são erros novos.
            let summary = msg.starts_with("could not compile")
                || msg.starts_with("aborting due to")
                || msg.starts_with("build failed")
                || (msg.contains("generated") && msg.contains("warning"));
            if !summary {
                let code = c.name("code").map(|m| m.as_str());
                let tool = if code.is_some() { "rust" } else { "build" };
                self.rust = Some(Diagnostic::new(tool, severity_of(&c["sev"]).unwrap_or(Severity::Error), code, &c["msg"]));
            }
            return;
        }
        if let Some(c) = re(
            &GCC,
            r"^(?P<file>(?:[A-Za-z]:)?[^:\s][^:]*?):(?P<line>\d+):(?:(?P<col>\d+):)?\s*(?P<sev>fatal error|error|warning):\s*(?P<msg>.+)$",
        )
        .captures(t)
        {
            self.flush_rust();
            let d = Diagnostic::new("cpp", severity_of(&c["sev"]).unwrap_or(Severity::Error), None, &c["msg"])
                .at(&c["file"], Some(&c["line"]), c.name("col").map(|m| m.as_str()));
            self.out.push(d);
            return;
        }
        if let Some(c) = re(
            &NODE_ERR,
            r"^(?:Uncaught\s+)?(?P<name>[A-Z][A-Za-z]*(?:Error|Exception))(?:\s*\[(?P<code>[A-Z0-9_]+)\])?:\s*(?P<msg>.+)$",
        )
        .captures(t)
        {
            self.flush_rust();
            let code = c.name("code").map(|m| m.as_str()).unwrap_or(&c["name"]).to_string();
            self.node = Some((Diagnostic::new("node", Severity::Error, Some(&code), &c["msg"]), false));
        }
    }

    fn finish(mut self) -> Vec<Diagnostic> {
        self.flush_npm();
        self.flush_node();
        self.flush_rust();
        self.out
    }
}

/// Diagnósticos de um texto (terminal, janela Saída, log).
pub fn parse_text(text: &str) -> Vec<Diagnostic> {
    let mut p = Parser::default();
    for raw in text.lines() {
        p.line(raw);
    }
    dedup(p.finish())
}

/// Código de diagnóstico sozinho numa célula ("CS0103", "TS2322", "NU1903").
fn code_cell(s: &str) -> Option<&str> {
    static CODE: OnceLock<Regex> = OnceLock::new();
    let s = s.trim();
    re(&CODE, r"^(?:CS|BC|FS|TS|NU|MSB|NETSDK|CA|IDE|LNK|C|E)\d{3,5}$").is_match(s).then_some(s)
}

/// Célula com cara de arquivo de código/projeto.
fn file_cell(s: &str) -> bool {
    static FILE: OnceLock<Regex> = OnceLock::new();
    re(
        &FILE,
        r"(?i)^\S.*\.(?:cs|vb|fs|ts|tsx|js|jsx|mjs|cjs|rs|cpp|cc|c|h|hpp|xaml|razor|cshtml|csproj|vbproj|fsproj|props|targets|json|sln|slnx|py|go|java|kt)$",
    )
    .is_match(s.trim())
}

/// Linhas de tabela (Lista de Erros do Visual Studio, painel Problemas):
/// cada linha é a lista de células lidas por acessibilidade. Só vira
/// diagnóstico a linha que tem um código (CS0103…) — o resto da janela
/// (Gerenciador de Soluções, abas) também é lista e é ignorado.
pub fn parse_rows(rows: &[Vec<String>]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for row in rows {
        let cells: Vec<String> = row.iter().map(|c| clean_line(c).trim().to_string()).filter(|c| !c.is_empty()).collect();
        // A célula pode trazer a linha inteira em formato canônico (Saída, terminal).
        let canonical: Vec<Diagnostic> = cells.iter().flat_map(|c| parse_text(c)).collect();
        if !canonical.is_empty() {
            out.extend(canonical);
            continue;
        }
        if cells.iter().any(|c| matches!(c.to_lowercase().as_str(), "mensagem" | "message" | "informação" | "info")) {
            continue; // mensagens informativas não são erro nem aviso
        }
        if let Some(code) = cells.iter().find_map(|c| code_cell(c)).map(str::to_string) {
            out.push(row_from_cells(&cells, &code));
        } else if let Some(d) = cells.iter().find_map(|c| row_from_one_cell(c)) {
            out.push(d);
        }
    }
    dedup(out)
}

/// Célula que é só a gravidade ("Erro", "Warning").
fn severity_cell(c: &str) -> Option<Severity> {
    match c.trim().to_lowercase().as_str() {
        "error" | "erro" | "fatal error" => Some(Severity::Error),
        "warning" | "aviso" => Some(Severity::Warning),
        _ => None,
    }
}

/// Gravidade padrão quando a linha não diz: analisadores (CA/IDE) são avisos.
fn default_severity(code: &str) -> Severity {
    if tool_for_code(code) == "analyzer" {
        Severity::Warning
    } else {
        Severity::Error
    }
}

/// Uma célula por coluna: gravidade, código, descrição, projeto, arquivo, linha.
fn row_from_cells(cells: &[String], code: &str) -> Diagnostic {
    let severity = cells.iter().find_map(|c| severity_cell(c)).unwrap_or_else(|| default_severity(code));
    let file = cells.iter().find(|c| file_cell(c)).cloned();
    let code_at = cells.iter().position(|c| c.trim() == code).unwrap_or(0);
    let line = cells.iter().skip(code_at + 1).find_map(|c| c.parse::<u32>().ok());
    // Mensagem: a célula mais longa que não é código/arquivo/número/gravidade
    // nem a linha inteira (o nome da linha às vezes repete tudo).
    let message = cells
        .iter()
        .filter(|c| {
            code_cell(c).is_none() && !file_cell(c) && c.parse::<u32>().is_err() && severity_cell(c).is_none() && !c.contains(code)
        })
        .max_by_key(|c| c.chars().count())
        .cloned()
        .unwrap_or_default();
    let mut d = Diagnostic::new(tool_for_code(code), severity, Some(code), message.trim());
    d.file = file;
    d.line = line;
    d
}

/// A linha inteira num nome só: "Error CS1002 ; expected Demo Program.cs 20".
fn row_from_one_cell(cell: &str) -> Option<Diagnostic> {
    static CODE_IN: OnceLock<Regex> = OnceLock::new();
    let m = re(&CODE_IN, r"\b(?:CS|BC|FS|TS|NU|MSB|NETSDK|CA|IDE|LNK)\d{3,5}\b").find(cell)?;
    let code = m.as_str();
    let before = cell[..m.start()].trim();
    let severity = severity_cell(before).unwrap_or_else(|| default_severity(code));
    let after: Vec<&str> = cell[m.end()..].split_whitespace().collect();
    // Arquivo e linha no fim; a mensagem é o que vem antes do arquivo.
    let file_at = after.iter().position(|w| file_cell(w));
    let message = after[..file_at.unwrap_or(after.len())].join(" ");
    let mut d = Diagnostic::new(tool_for_code(code), severity, Some(code), &message);
    if let Some(i) = file_at {
        d.file = Some(after[i].to_string());
        d.line = after.get(i + 1).and_then(|w| w.parse().ok());
    }
    Some(d)
}

/// Remove repetidos (o MSBuild repete os erros no resumo final) e limita.
pub fn dedup(list: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen = HashSet::new();
    let base = |f: &Option<String>| f.as_deref().map(|f| f.rsplit(['\\', '/']).next().unwrap_or(f).to_lowercase());
    list.into_iter()
        .filter(|d| seen.insert((d.code.clone(), base(&d.file), d.line, d.message.to_lowercase())))
        .take(MAX_DIAGNOSTICS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(text: &str) -> Vec<Diagnostic> {
        parse_text(text).into_iter().filter(|d| d.severity == Severity::Error).collect()
    }

    #[test]
    fn msbuild_and_csharp() {
        let out = "\
Build started...
1>------ Build started: Project: Demo, Configuration: Debug Any CPU ------
1>C:\\src\\Demo\\Program.cs(12,17,12,20): error CS0103: The name 'foo' does not exist in the current context [C:\\src\\Demo\\Demo.csproj]
1>C:\\src\\Demo\\Services\\Repo.cs(40,5): warning CS8618: Non-nullable property 'Name' must contain a non-null value.
2>C:\\Program Files (x86)\\MSBuild\\Microsoft.Common.targets(5,5): error MSB3027: Could not copy \"obj\\Debug\\Demo.dll\".
CSC : error CS5001: Program does not contain a static 'Main' method suitable for an entry point
C:\\src\\Demo\\Demo.csproj : error NU1101: Unable to find package Fake.Package. No packages exist with this id
========== Build: 0 succeeded, 1 failed ==========
C:\\src\\Demo\\Program.cs(12,17,12,20): error CS0103: The name 'foo' does not exist in the current context [C:\\src\\Demo\\Demo.csproj]";
        let all = parse_text(out);
        let errs: Vec<_> = all.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errs.len(), 4, "{all:#?}");
        let cs = errs[0];
        assert_eq!(cs.tool, "csharp");
        assert_eq!(cs.code.as_deref(), Some("CS0103"));
        assert_eq!(cs.file.as_deref(), Some("C:\\src\\Demo\\Program.cs"));
        assert_eq!((cs.line, cs.col), (Some(12), Some(17)));
        assert_eq!(cs.project.as_deref(), Some("Demo"));
        assert_eq!(cs.message, "The name 'foo' does not exist in the current context");
        assert_eq!(errs[1].code.as_deref(), Some("MSB3027"));
        assert_eq!(errs[1].file.as_deref(), Some("C:\\Program Files (x86)\\MSBuild\\Microsoft.Common.targets"));
        assert_eq!(errs[2].code.as_deref(), Some("CS5001"));
        assert_eq!(errs[2].file, None, "CSC não é arquivo");
        assert_eq!(errs[3].code.as_deref(), Some("NU1101"));
        assert_eq!(errs[3].file.as_deref(), Some("C:\\src\\Demo\\Demo.csproj"));
        assert_eq!(all.iter().filter(|d| d.severity == Severity::Warning).count(), 1);
    }

    #[test]
    fn typescript_both_formats() {
        let out = "\
src/app.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.
src/util.ts:10:5 - error TS2304: Cannot find name 'foo'.

10     foo();
       ~~~
Found 2 errors in 2 files.";
        let e = errors(out);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].tool, "typescript");
        assert_eq!((e[0].file.as_deref(), e[0].line, e[0].col), (Some("src/app.ts"), Some(3), Some(7)));
        assert_eq!(e[1].code.as_deref(), Some("TS2304"));
        assert_eq!((e[1].file.as_deref(), e[1].line), (Some("src/util.ts"), Some(10)));
    }

    #[test]
    fn rustc_and_cargo() {
        let out = "\
   Compiling demo v0.1.0 (C:\\src\\demo)
error[E0425]: cannot find value `x` in this scope
  --> src\\main.rs:4:13
   |
4  |     let y = x + 1;
   |             ^ not found in this scope

warning: unused variable: `z`
 --> src/lib.rs:2:9
  |
error: expected one of `,` or `}`, found `;`
 --> src/lib.rs:9:1

error: could not compile `demo` (bin \"demo\") due to 2 previous errors";
        let all = parse_text(out);
        let e: Vec<_> = all.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(e.len(), 2, "{all:#?}");
        assert_eq!(e[0].code.as_deref(), Some("E0425"));
        assert_eq!(e[0].tool, "rust");
        assert_eq!((e[0].file.as_deref(), e[0].line, e[0].col), (Some("src\\main.rs"), Some(4), Some(13)));
        assert_eq!(e[1].code, None);
        assert_eq!(e[1].tool, "rust", "com local vira rust");
        assert_eq!(e[1].file.as_deref(), Some("src/lib.rs"));
        assert_eq!(all.iter().filter(|d| d.severity == Severity::Warning).count(), 1);
    }

    #[test]
    fn npm_block_becomes_one_error() {
        let out = "\
> demo@1.0.0 build
> tsc -p .

npm ERR! code ELIFECYCLE
npm ERR! errno 2
npm ERR! demo@1.0.0 build: `tsc -p .`
npm ERR! Exit status 2
npm ERR! A complete log of this run can be found in: C:\\fake\\logs\\x.log
PS C:\\src\\demo> npm run test
npm error Missing script: \"test\"
npm error
npm error To see a list of scripts, run:
npm error   npm run";
        let e = errors(out);
        assert_eq!(e.len(), 2, "{e:#?}");
        assert_eq!(e[0].tool, "npm");
        assert_eq!(e[0].code.as_deref(), Some("ELIFECYCLE"));
        assert_eq!(e[0].message, "demo@1.0.0 build: `tsc -p .`");
        assert_eq!(e[1].message, "Missing script: \"test\"");
    }

    #[test]
    fn node_stack_trace_points_to_user_code() {
        let out = "\
C:\\src\\demo\\src\\index.js:10
    return user.name;
                ^

TypeError: Cannot read properties of undefined (reading 'name')
    at getName (C:\\src\\demo\\node_modules\\lib\\x.js:3:9)
    at main (C:\\src\\demo\\src\\index.js:10:17)
    at node:internal/main/run_main_module:28:49

Node.js v20.11.0";
        let e = errors(out);
        assert_eq!(e.len(), 1, "{e:#?}");
        assert_eq!(e[0].tool, "node");
        assert_eq!(e[0].code.as_deref(), Some("TypeError"));
        assert_eq!((e[0].file.as_deref(), e[0].line), (Some("C:\\src\\demo\\src\\index.js"), Some(10)));
        // Sem quadro "at", não é pilha do Node (ex.: um texto qualquer).
        assert!(errors("ValueError: algo deu errado\nfim").is_empty());
    }

    #[test]
    fn gcc_style_and_noise() {
        let e = errors("main.c:5:3: error: 'x' undeclared (first use in this function)\nC:\\src\\a.cpp:7: error: falta ;");
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].file.as_deref(), e[0].line, e[0].col), (Some("main.c"), Some(5), Some(3)));
        assert_eq!(e[1].file.as_deref(), Some("C:\\src\\a.cpp"));
        // Texto comum não vira erro.
        for t in ["Build succeeded.", "0 Error(s)", "no errors found", "Erro ao abrir o arquivo", "PS C:\\src> cargo build"] {
            assert!(parse_text(t).is_empty(), "{t}");
        }
    }

    #[test]
    fn ansi_and_window_labels_are_stripped() {
        let e = errors("\x1b[31merror\x1b[0m[E0308]: mismatched types\n  --> src/a.rs:1:2\n[texto] src/b.ts(1,1): error TS1005: ';' expected.");
        assert_eq!(e.len(), 2, "{e:#?}");
        assert_eq!(e[0].code.as_deref(), Some("E0308"));
        assert_eq!(e[1].code.as_deref(), Some("TS1005"));
    }

    #[test]
    fn error_list_rows_from_visual_studio() {
        let row = |cells: &[&str]| cells.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let rows = vec![
            // Solution Explorer: lista, mas sem código → ignorada.
            row(&["Demo", "Program.cs"]),
            row(&["Erro", "CS0103", "O nome 'foo' não existe no contexto atual", "Demo", "Program.cs", "12", "Ativo"]),
            row(&["Warning", "CS8618", "Non-nullable property 'Name'", "Demo", "Repo.cs", "40"]),
            row(&["IDE0005", "Using directive is unnecessary.", "Demo", "Program.cs", "1"]),
            row(&["Mensagem", "CA1822", "Member can be static", "Demo", "Program.cs", "3"]),
            // A linha inteira num nome só.
            row(&["Error CS1002 ; expected Demo Program.cs 20"]),
        ];
        let d = parse_rows(&rows);
        assert_eq!(d.len(), 4, "{d:#?}");
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].message, "O nome 'foo' não existe no contexto atual");
        assert_eq!((d[0].file.as_deref(), d[0].line), (Some("Program.cs"), Some(12)));
        assert_eq!(d[1].severity, Severity::Warning);
        assert_eq!(d[2].severity, Severity::Warning, "analisador sem gravidade é aviso");
        assert_eq!(d[3].code.as_deref(), Some("CS1002"));
        assert_eq!(d[3].severity, Severity::Error);
        assert_eq!(d[3].message, "; expected Demo", "o projeto fica junto: não há como separar");
        assert_eq!((d[3].file.as_deref(), d[3].line), (Some("Program.cs"), Some(20)));
    }

    #[test]
    fn duplicates_are_removed_and_capped() {
        let line = "a.cs(1,1): error CS0001: x\n";
        assert_eq!(parse_text(&line.repeat(3)).len(), 1);
        let many: String = (1..=100).map(|n| format!("a.cs({n},1): error CS0001: x\n")).collect();
        assert_eq!(parse_text(&many).len(), MAX_DIAGNOSTICS);
        let long = format!("a.cs(1,1): error CS0001: {}", "y".repeat(1000));
        assert!(parse_text(&long)[0].message.chars().count() <= MAX_MESSAGE + 1);
    }
}
