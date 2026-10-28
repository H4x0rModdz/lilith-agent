// node --test services/report-worker/test
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import worker from "../src/index.js";
import { fingerprint, renderIssue, validate } from "../src/report.js";

const good = () => ({
  v: 1,
  app: "0.4.1",
  action: "window.read",
  code: "EXECUTION_FAILED",
  os: "Windows 11 25H2 (build 26200)",
  cpu: "Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz",
  cores: 8,
  ram_gb: 32,
  gpu: "NVIDIA GeForce RTX 5070",
  browser: "waterfox",
  browser_version: "153.5.0",
  ollama: "0.12.0",
  model: "qwen3.5:4b",
  voice: "kokoro",
  source: "auto",
  missing: null,
});

/// D1 de mentira sobre o SQLite do Node (mesma API que o Worker usa).
function fakeD1() {
  const db = new DatabaseSync(":memory:");
  db.exec(readFileSync(new URL("../schema.sql", import.meta.url), "utf8"));
  return {
    raw: db,
    prepare(sql) {
      let args = [];
      const stmt = {
        bind: (...a) => ((args = a), stmt),
        first: async () => db.prepare(sql).get(...args) ?? null,
        run: async () => ({ meta: { changes: Number(db.prepare(sql).run(...args).changes) } }),
      };
      return stmt;
    },
  };
}

const env = (over = {}) => ({ DB: fakeD1(), DRY_RUN: "1", IP_SALT: "s", ...over });

const post = (body, { ip = "1.2.3.4", headers = {} } = {}) => {
  const text = typeof body === "string" ? body : JSON.stringify(body);
  return new Request("https://w.example/v1/report", {
    method: "POST",
    headers: { "content-type": "application/json", "x-lilith-report": "1", "cf-connecting-ip": ip, ...headers },
    body: text,
  });
};

test("aceita o relatório certo e recusa qualquer desvio", () => {
  assert.ok(validate(good()).report);
  const cases = {
    extra: { ...good(), nome: "user X" },
    missing: (({ cpu, ...r }) => r)(good()),
    url: { ...good(), gpu: "compre já https://spam.example" },
    newline: { ...good(), cpu: "a\nb" },
    backtick: { ...good(), os: "Windows `x`" },
    pipe: { ...good(), os: "Windows | x" },
    action: { ...good(), action: "Window.Read; rm" },
    long: { ...good(), cpu: "x".repeat(81) },
    voice: { ...good(), voice: "outra" },
    source: { ...good(), source: "admin" },
    gapPhone: { ...good(), missing: "ligar para 11 99999-9999" },
    gapMail: { ...good(), missing: "mandar e-mail para ana@x.com" },
    gapPath: { ...good(), missing: "ler C:/Users/x" },
  };
  for (const [name, r] of Object.entries(cases)) assert.ok(validate(r).error, name);
});

test("a issue só tem a ficha técnica, cada valor em código", async () => {
  const fp = await fingerprint(good());
  const { title, body } = renderIssue(good(), fp);
  assert.equal(title, "[auto] window.read falhou (EXECUTION_FAILED) — v0.4.1");
  assert.match(body, /\| CPU \| `Intel\(R\) Core\(TM\) i7-9700K CPU @ 3\.60GHz \(8 threads\)` \|/);
  assert.match(body, /lilith-fingerprint: [0-9a-f]{16}/);
});

test("mesma falha na mesma versão = mesma digital; versão nova muda", async () => {
  assert.equal(await fingerprint(good()), await fingerprint({ ...good(), cpu: "AMD Ryzen 9 9900X" }));
  assert.notEqual(await fingerprint(good()), await fingerprint({ ...good(), app: "0.4.2" }));
});

test("fluxo: cria, depois só conta a repetição", async () => {
  const e = env();
  const first = await worker.fetch(post(good()), e);
  assert.equal(first.status, 201);
  assert.match((await first.json()).dry_run.title, /window\.read/);
  const again = await worker.fetch(post(good(), { ip: "5.6.7.8" }), e);
  assert.equal((await again.json()).status, "known");
  assert.equal(e.DB.raw.prepare("SELECT count FROM issues").get().count, 2);
});

test("recusa barato o que não é da Lilith", async () => {
  const e = env();
  assert.equal((await worker.fetch(new Request("https://w.example/"), e)).status, 404);
  assert.equal((await worker.fetch(post(good(), { headers: { "x-lilith-report": "0" } }), e)).status, 400);
  assert.equal((await worker.fetch(post("{" + " ".repeat(3000) + "}"), e)).status, 413);
  assert.equal((await worker.fetch(post("não é json"), e)).status, 400);
  assert.equal((await worker.fetch(post({ ...good(), extra: 1 }), e)).status, 400);
});

test("limites diários: por origem e de issues novas", async () => {
  const e = env({ MAX_PER_ORIGIN_DAY: "2", MAX_NEW_ISSUES_DAY: "1" });
  const r = (action, ip) => worker.fetch(post({ ...good(), action }, { ip }), e);
  assert.equal((await r("a.one", "9.9.9.9")).status, 201);
  assert.equal((await r("a.two", "9.9.9.9")).status, 429); // 2ª issue nova do dia
  assert.equal((await r("a.three", "9.9.9.9")).status, 429); // passou do teto da origem
  // A falha recusada pelo teto de issues não ficou reservada.
  assert.equal(e.DB.raw.prepare("SELECT COUNT(*) AS n FROM issues").get().n, 1);
});

test("limitador por IP do Workers barra antes de tudo", async () => {
  const e = env({ PER_IP: { limit: async () => ({ success: false }) } });
  assert.equal((await worker.fetch(post(good()), e)).status, 429);
  assert.equal(e.DB.raw.prepare("SELECT COUNT(*) AS n FROM counters").get().n, 0);
});

test('pedido de melhoria vira issue de enhancement com o texto em código', async () => {
  const r = { ...good(), action: 'capability.missing', code: 'MISSING_FEATURE', missing: 'editar planilhas do Excel' };
  assert.ok(validate(r).report);
  const { title, body, labels } = renderIssue(r, await fingerprint(r));
  assert.equal(title, '[melhoria] ainda não sei: editar planilhas do Excel — v0.4.1');
  assert.match(body, /\| O que falta \| `editar planilhas do Excel` \|/);
  assert.deepEqual(labels, ['enhancement', 'auto-report']);
  assert.notEqual(await fingerprint(r), await fingerprint({ ...r, missing: 'abrir arquivos zip' }));
});
