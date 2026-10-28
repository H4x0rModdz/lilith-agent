// API de relatórios de bug da Lilith (Cloudflare Worker, plano grátis).
//
// A Lilith instalada não tem token nenhum: ela manda a ficha técnica + o
// código da falha para POST /v1/report e este Worker, que guarda a chave do
// GitHub App como segredo, abre a issue no repositório configurado
// (GITHUB_OWNER/GITHUB_REPO — trocar para um repo privado é só mudar a var).
//
// Defesas, da mais barata para a mais cara (o que é recusado cedo não gasta
// D1 nem GitHub):
//   1. rota/método/cabeçalhos/tamanho errados → 404/400 sem tocar em nada;
//   2. limitador por IP do próprio Workers (em memória, grátis);
//   3. corpo lido com teto de bytes e validado campo a campo;
//   4. tetos diários no D1: por origem (hash do IP), total de relatórios e
//      total de issues novas;
//   5. falha repetida (mesma ação + código + versão) só soma um contador.
// Mesmo que tudo isso falhe, no plano grátis passar da cota não gera cobrança:
// o Worker só para de responder até o dia seguinte.

import {
  COUNT_MILESTONES,
  MAX_BODY_BYTES,
  fingerprint,
  milestoneComment,
  originKey,
  renderIssue,
  validate,
} from "./report.js";

const json = (status, data) =>
  new Response(JSON.stringify(data), { status, headers: { "content-type": "application/json" } });

const today = () => new Date().toISOString().slice(0, 10);

const limit = (env, name, fallback) => {
  const n = Number.parseInt(env[name] ?? "", 10);
  return Number.isFinite(n) && n >= 0 ? n : fallback;
};

/// Corpo com teto de bytes: não confia no Content-Length.
async function readCapped(request, max) {
  const reader = request.body?.getReader();
  if (!reader) return null;
  const chunks = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > max) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const all = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.byteLength;
  }
  return new TextDecoder().decode(all);
}

/// Soma 1 no contador do dia e devolve o valor novo.
async function bump(env, day, key) {
  const row = await env.DB.prepare(
    "INSERT INTO counters (day, key, n) VALUES (?1, ?2, 1) ON CONFLICT (day, key) DO UPDATE SET n = n + 1 RETURNING n",
  )
    .bind(day, key)
    .first();
  return row?.n ?? 0;
}

// ---------------------------------------------------------------- GitHub App

const b64url = (bytes) =>
  btoa(String.fromCharCode(...new Uint8Array(bytes))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

const b64urlText = (text) => b64url(new TextEncoder().encode(text));

/// JWT RS256 do GitHub App. A chave é PKCS#8 (`BEGIN PRIVATE KEY`) — veja o README.
async function appJwt(env) {
  const pem = env.GITHUB_APP_PRIVATE_KEY.replace(/-----[^-]+-----/g, "").replace(/\s+/g, "");
  const der = Uint8Array.from(atob(pem), (c) => c.charCodeAt(0));
  const key = await crypto.subtle.importKey("pkcs8", der, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["sign"]);
  const now = Math.floor(Date.now() / 1000);
  const head = b64urlText(JSON.stringify({ alg: "RS256", typ: "JWT" }));
  const claims = b64urlText(JSON.stringify({ iat: now - 60, exp: now + 540, iss: env.GITHUB_APP_ID }));
  const sig = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", key, new TextEncoder().encode(`${head}.${claims}`));
  return `${head}.${claims}.${b64url(sig)}`;
}

const GH_HEADERS = {
  accept: "application/vnd.github+json",
  "user-agent": "lilith-report-worker",
  "x-github-api-version": "2022-11-28",
};

// Token de instalação vale 1 h; guardado enquanto o isolate viver.
let cachedToken = null;

/// Token só com Issues: write, só no repositório alvo.
async function installationToken(env) {
  if (cachedToken && cachedToken.expires > Date.now() + 5 * 60_000) return cachedToken.value;
  const res = await fetch(`https://api.github.com/app/installations/${env.GITHUB_INSTALLATION_ID}/access_tokens`, {
    method: "POST",
    headers: { ...GH_HEADERS, authorization: `Bearer ${await appJwt(env)}` },
    body: JSON.stringify({ repositories: [env.GITHUB_REPO], permissions: { issues: "write" } }),
  });
  if (!res.ok) throw new Error(`token do GitHub: HTTP ${res.status}`);
  const data = await res.json();
  cachedToken = { value: data.token, expires: Date.parse(data.expires_at) };
  return cachedToken.value;
}

async function github(env, path, body) {
  const res = await fetch(`https://api.github.com/repos/${env.GITHUB_OWNER}/${env.GITHUB_REPO}${path}`, {
    method: "POST",
    headers: { ...GH_HEADERS, authorization: `Bearer ${await installationToken(env)}` },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`GitHub ${path}: HTTP ${res.status}`);
  return res.json();
}

// ------------------------------------------------------------------- rotas

async function handleReport(request, env) {
  if (request.headers.get("x-lilith-report") !== "1") return json(400, { error: "cliente inválido" });
  if (!(request.headers.get("content-type") ?? "").startsWith("application/json")) {
    return json(415, { error: "envie JSON" });
  }
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > MAX_BODY_BYTES) return json(413, { error: "relatório grande demais" });

  const ip = request.headers.get("cf-connecting-ip") ?? "0.0.0.0";
  if (env.PER_IP) {
    const { success } = await env.PER_IP.limit({ key: ip });
    if (!success) return json(429, { error: "devagar", retry: "minute" });
  }

  const text = await readCapped(request, MAX_BODY_BYTES);
  if (text === null) return json(413, { error: "relatório grande demais" });
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    return json(400, { error: "JSON inválido" });
  }
  const { report, error } = validate(parsed);
  if (error) return json(400, { error });

  const day = today();
  const origin = await originKey(ip, env.IP_SALT ?? "", day);
  if ((await bump(env, day, `origin:${origin}`)) > limit(env, "MAX_PER_ORIGIN_DAY", 5)) {
    return json(429, { error: "limite diário deste PC", retry: "tomorrow" });
  }
  if ((await bump(env, day, "reports")) > limit(env, "MAX_REPORTS_DAY", 300)) {
    return json(429, { error: "limite diário geral", retry: "tomorrow" });
  }

  const fp = await fingerprint(report);
  // Reserva a falha antes de criar a issue: dois envios iguais ao mesmo
  // tempo não abrem duas.
  const claimed = await env.DB.prepare("INSERT OR IGNORE INTO issues (fingerprint, number, count, created_at) VALUES (?1, 0, 1, ?2)")
    .bind(fp, new Date().toISOString())
    .run();

  if (!claimed.meta.changes) {
    const row = await env.DB.prepare("UPDATE issues SET count = count + 1 WHERE fingerprint = ?1 RETURNING number, count")
      .bind(fp)
      .first();
    if (row?.number && COUNT_MILESTONES.includes(row.count) && env.DRY_RUN !== "1") {
      // Melhor esforço: o contador já subiu, o comentário é só um aviso.
      await github(env, `/issues/${row.number}/comments`, { body: milestoneComment(row.count) }).catch(() => {});
    }
    return json(200, { status: "known", issue: row?.number || null });
  }

  if ((await bump(env, day, "new_issues")) > limit(env, "MAX_NEW_ISSUES_DAY", 15)) {
    await env.DB.prepare("DELETE FROM issues WHERE fingerprint = ?1").bind(fp).run();
    return json(429, { error: "limite diário de issues novas", retry: "tomorrow" });
  }

  const issue = renderIssue(report, fp);
  if (env.DRY_RUN === "1") {
    await env.DB.prepare("UPDATE issues SET number = -1 WHERE fingerprint = ?1").bind(fp).run();
    return json(201, { status: "created", issue: null, dry_run: issue });
  }
  try {
    const created = await github(env, "/issues", issue);
    await env.DB.prepare("UPDATE issues SET number = ?2 WHERE fingerprint = ?1").bind(fp, created.number).run();
    return json(201, { status: "created", issue: created.number });
  } catch (e) {
    await env.DB.prepare("DELETE FROM issues WHERE fingerprint = ?1").bind(fp).run();
    console.error(String(e));
    return json(502, { error: "não consegui abrir a issue agora" });
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/v1/report" && request.method === "POST") return handleReport(request, env);
    return new Response("Not found", { status: 404 });
  },

  // Uma vez por dia: contadores de dias passados somem (as chaves de origem
  // já não servem para nada no dia seguinte).
  async scheduled(_event, env) {
    await env.DB.prepare("DELETE FROM counters WHERE day < ?1").bind(today()).run();
  },
};
