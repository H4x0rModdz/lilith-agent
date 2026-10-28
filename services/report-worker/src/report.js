// Regras do relatório, sem I/O (testáveis com `node --test`).
//
// O corpo da issue é montado AQUI, a partir de campos com formato fixo: o
// cliente não manda texto livre, então ninguém consegue usar o endpoint para
// publicar spam, links ou menções no repositório. Cada valor vai dentro de
// `code` no Markdown (sem autolink, sem @menção) e os caracteres que fecham o
// código ou a tabela (` | quebra de linha) são recusados.

export const MAX_BODY_BYTES = 2048;

// Texto curto descritivo: letras, números e a pontuação de nomes de hardware
// ("Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz", "qwen3.5:4b").
const LABEL = /^[\p{L}\p{N} ._()+\-\/,:@®™]{1,80}$/u;
// Nem endereço de site: nome de hardware nunca tem "://" nem "www.".
const isLabel = (x) => typeof x === "string" && LABEL.test(x) && !/:\/\/|www\./i.test(x);
// O que ela ainda não sabe fazer (pedido de melhoria): o mesmo filtro do app
// (safe_gap) — só letras, espaço e pontuação simples. Sem números, @, barras
// ou dois-pontos: não passa telefone, CPF, e-mail, caminho nem link.
const isGap = (x) => typeof x === "string" && /^[\p{L} ,'()\-]{3,100}$/u.test(x);

const FIELDS = {
  v: (x) => x === 1,
  app: (x) => typeof x === "string" && /^\d{1,3}\.\d{1,3}\.\d{1,4}(-[a-z0-9.]{1,20})?$/.test(x),
  action: (x) => typeof x === "string" && /^[a-z][a-z0-9_]{0,24}(\.[a-z0-9_]{1,24}){0,2}$/.test(x),
  code: (x) => typeof x === "string" && /^[A-Z][A-Z_]{2,29}$/.test(x),
  os: (x) => isLabel(x),
  cpu: (x) => isLabel(x),
  cores: (x) => Number.isInteger(x) && x >= 1 && x <= 1024,
  ram_gb: (x) => Number.isInteger(x) && x >= 1 && x <= 4096,
  gpu: (x) => x === null || isLabel(x),
  browser: (x) => x === null || (typeof x === "string" && /^[a-z0-9_\-]{1,30}$/.test(x)),
  browser_version: (x) => x === null || (typeof x === "string" && /^\d{1,5}(\.\d{1,6}){0,3}$/.test(x)),
  ollama: (x) => x === null || (typeof x === "string" && /^\d{1,3}\.\d{1,3}(\.\d{1,4})?$/.test(x)),
  model: (x) => x === null || (typeof x === "string" && /^[a-z0-9][a-z0-9._:\-\/]{0,59}$/i.test(x)),
  voice: (x) => x === "off" || x === "kokoro" || x === "serena",
  source: (x) => x === "auto" || x === "user",
  missing: (x) => x === null || isGap(x),
};

/// Confere o relatório: todos os campos, só os campos conhecidos, cada um no
/// formato esperado. Devolve `{ report }` ou `{ error }`.
export function validate(input) {
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    return { error: "corpo inválido" };
  }
  const keys = Object.keys(input);
  const unknown = keys.filter((k) => !(k in FIELDS));
  if (unknown.length) return { error: `campo desconhecido: ${unknown[0].slice(0, 30)}` };
  for (const [name, ok] of Object.entries(FIELDS)) {
    if (!(name in input) || !ok(input[name])) return { error: `campo inválido: ${name}` };
  }
  return { report: input };
}

async function sha256Hex(text) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/// Mesma falha na mesma versão = mesma issue. Versão nova abre outra (o bug
/// voltou ou não foi corrigido).
export async function fingerprint(report) {
  const gap = report.missing ? `|${report.missing.toLowerCase()}` : "";
  return (await sha256Hex(`${report.action}|${report.code}|${report.app}${gap}`)).slice(0, 16);
}

/// Chave diária de limite por origem: hash do IP com um segredo e o dia. O IP
/// em si não é guardado e a chave muda todo dia (não dá para seguir ninguém).
export async function originKey(ip, salt, day) {
  return (await sha256Hex(`${salt}|${day}|${ip}`)).slice(0, 24);
}

const cell = (x) => (x === null || x === undefined ? "—" : `\`${x}\``);

export function renderIssue(report, fp) {
  if (report.missing) return renderGap(report, fp);
  const title = `[auto] ${report.action} falhou (${report.code}) — v${report.app}`;
  const rows = [
    ["Versão da Lilith", report.app],
    ["Ação", report.action],
    ["Erro", report.code],
    ["Windows", report.os],
    ["CPU", `${report.cpu} (${report.cores} threads)`],
    ["RAM", `${report.ram_gb} GB`],
    ["GPU", report.gpu],
    ["Navegador padrão", report.browser ? `${report.browser} ${report.browser_version ?? ""}`.trim() : null],
    ["Ollama", report.ollama ? `${report.ollama} · ${report.model ?? "?"}` : null],
    ["Voz", report.voice],
    ["Origem", report.source === "user" ? "pedido de quem usa" : "oferta automática"],
  ];
  const body = [
    "Relatório automático enviado pela Lilith com a permissão de quem usa.",
    "Contém só a ficha técnica do PC e o código da falha — nenhum dado pessoal.",
    "",
    "| | |",
    "|---|---|",
    ...rows.map(([k, v]) => `| ${k} | ${cell(v)} |`),
    "",
    `<!-- lilith-fingerprint: ${fp} -->`,
  ].join("\n");
  return { title, body, labels: ["bug", "auto-report"] };
}

/// Pedido de melhoria: algo que ela disse que ainda não sabe fazer.
function renderGap(report, fp) {
  const title = `[melhoria] ainda não sei: ${report.missing} — v${report.app}`;
  const rows = [
    ["O que falta", report.missing],
    ["Versão da Lilith", report.app],
    ["Windows", report.os],
    ["Navegador padrão", report.browser ? `${report.browser} ${report.browser_version ?? ""}`.trim() : null],
  ];
  const body = [
    "A Lilith disse a quem usa que ainda não sabe fazer isto, e a pessoa pediu para registrar.",
    "O texto é genérico e foi conferido por quem usa antes de enviar.",
    "",
    "| | |",
    "|---|---|",
    ...rows.map(([k, v]) => `| ${k} | ${cell(v)} |`),
    "",
    `<!-- lilith-fingerprint: ${fp} -->`,
  ].join("\n");
  return { title, body, labels: ["enhancement", "auto-report"] };
}

/// Quando comentar numa issue que já existe: só em marcos, para não gastar a
/// API (nem encher a issue) a cada repetição.
export const COUNT_MILESTONES = [5, 25, 100, 500, 2500];

export function milestoneComment(count) {
  return `A mesma falha já foi relatada **${count}** vezes nesta versão.`;
}
