import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { fillSigil } from "./avatar";
import { makeDraggable } from "./drag";
import { mountElectric } from "./electric";
import { call } from "./ipc";
import { mountSettings } from "./settings";
import { actionIcon, hydrateIcons, icon } from "./icons";
import { actionLabel } from "./labels";

type JobStatus = "running" | "done" | "failed" | "cancelled";

interface Job {
  id: number;
  agent: string;
  task: string;
  folder: string;
  status: JobStatus;
  started_ms: number;
  finished_ms: number | null;
  log: string[];
  result: string | null;
  conversation: string | null;
}

type JobEvent = { kind: "updated"; job: Job } | { kind: "line"; id: number; line: string };

interface Activity {
  ts: string;
  channel: string;
  action: string;
  outcome: string;
}

const STATUS_LABEL: Record<JobStatus, string> = {
  running: "rodando",
  done: "concluído",
  failed: "falhou",
  cancelled: "cancelado",
};

const OUTCOME_LABEL: Record<string, string> = {
  success: "ok",
  error: "erro",
  denied: "bloqueado",
  denied_by_user: "negado",
  blocked_injection: "bloqueado",
  aborted: "parado",
};

/// Painel da Lilith (aberto pelo ícone na área de notificação): tarefas
/// delegadas a agentes de código e o histórico de ações.
export function mountPanel(root: HTMLElement) {
  root.innerHTML = `
    <section class="glass panel">
      <header class="glass-head">
        <span class="sigil" aria-hidden="true"></span>
        <div class="glass-title">
          <strong>Painel da Lilith</strong>
          <small class="panel-summary">nenhuma tarefa</small>
        </div>
        <button class="icon-btn neon-soft" data-act="close" title="Fechar"><span data-icon="close" data-size="17"></span></button>
      </header>
      <nav class="tabs" role="tablist">
        <button role="tab" class="tab" data-tab="jobs" aria-selected="true"><span data-icon="jobs" data-size="16"></span><span class="tab-label">Tarefas</span></button>
        <button role="tab" class="tab" data-tab="activity" aria-selected="false"><span data-icon="activity" data-size="16"></span><span class="tab-label">Atividade</span></button>
        <button role="tab" class="tab" data-tab="rules" aria-selected="false" title="Permissões"><span data-icon="key" data-size="16"></span><span class="tab-label">Permissões</span></button>
        <button role="tab" class="tab" data-tab="settings" aria-selected="false" title="Configurações"><span data-icon="settings" data-size="16"></span><span class="tab-label">Configurações</span></button>
      </nav>
      <ol class="panel-list jobs" data-view="jobs"></ol>
      <ol class="panel-list activity" data-view="activity" hidden></ol>
      <ol class="panel-list rules" data-view="rules" hidden></ol>
      <div class="panel-list settings-view" data-view="settings" hidden></div>
    </section>`;
  hydrateIcons(root);
  mountElectric(root.querySelector<HTMLElement>(".glass")!);

  const jobsEl = root.querySelector<HTMLOListElement>('[data-view="jobs"]')!;
  const activityEl = root.querySelector<HTMLOListElement>('[data-view="activity"]')!;
  const rulesEl = root.querySelector<HTMLOListElement>('[data-view="rules"]')!;
  const settingsEl = root.querySelector<HTMLElement>('[data-view="settings"]')!;
  const settings = mountSettings(settingsEl);
  const summary = root.querySelector<HTMLElement>(".panel-summary")!;
  const jobs = new Map<number, Job>();
  const titles = new Map<string, string>();
  makeDraggable(root.querySelector(".glass-head")!);
  fillSigil(root.querySelector(".glass-head .sigil")!);
  const expanded = new Set<number>();

  root.querySelector('[data-act="close"]')!.addEventListener("click", () =>
    invoke("hide_window", { label: "panel" }),
  );
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") invoke("hide_window", { label: "panel" });
  });

  root.querySelectorAll<HTMLButtonElement>(".tab").forEach((tab) =>
    tab.addEventListener("click", () => showTab(tab.dataset.tab!)),
  );

  listen<string>("lilith://panel-tab", (e) => showTab(e.payload));

  function showTab(name: string) {
    root.querySelectorAll<HTMLButtonElement>(".tab").forEach((t) =>
      t.setAttribute("aria-selected", String(t.dataset.tab === name)),
    );
    jobsEl.hidden = name !== "jobs";
    activityEl.hidden = name !== "activity";
    rulesEl.hidden = name !== "rules";
    settingsEl.hidden = name !== "settings";
    if (name === "settings") settings.load();
    if (name === "activity") loadActivity();
    if (name === "rules") loadRules();
  }

  // ── tarefas ──────────────────────────────────────────────────────────

  function renderJobs() {
    const list = [...jobs.values()].sort((a, b) => b.id - a.id);
    const running = list.filter((j) => j.status === "running").length;
    summary.textContent =
      list.length === 0 ? "nenhuma tarefa" : running > 0 ? `${running} rodando agora` : `${list.length} tarefa(s)`;

    if (list.length === 0) {
      jobsEl.innerHTML = `<li class="empty"><span data-icon="bot" data-size="28"></span>Peça algo à Lilith como “pede pro Claude arrumar o build do projeto X” e acompanhe aqui.</li>`;
      hydrateIcons(jobsEl);
      return;
    }
    // Agrupadas pela conversa que pediu (título atual, renomeável no chat).
    const groups = new Map<string, Job[]>();
    for (const job of list) {
      const key = job.conversation ?? "";
      groups.set(key, [...(groups.get(key) ?? []), job]);
    }
    const nodes: HTMLElement[] = [];
    for (const [conv, items] of groups) {
      const h = document.createElement("li");
      h.className = "group-head";
      h.dataset.conversation = conv;
      h.textContent = conv ? (titles.get(conv) ?? "Conversa") : "Sem conversa";
      nodes.push(h, ...items.map(renderJob));
    }
    jobsEl.replaceChildren(...nodes);
  }

  function renderJob(job: Job): HTMLLIElement {
    const li = document.createElement("li");
    li.className = "job";
    li.dataset.id = String(job.id);
    li.dataset.status = job.status;
    li.innerHTML = `
      <div class="job-head">
        <span class="badge agent"></span>
        <span class="badge status"></span>
        <span class="job-time"></span>
        <button class="icon-btn job-cancel" title="Cancelar"><span data-icon="stop" data-size="13"></span></button>
      </div>
      <p class="job-task"></p>
      <p class="job-folder"></p>
      <p class="job-result"></p>
      <button class="link job-toggle"></button>
      <pre class="job-log"></pre>`;
    hydrateIcons(li);
    const agentBadge = li.querySelector<HTMLElement>(".agent")!;
    const agentName = document.createElement("span");
    agentName.textContent = job.agent;
    agentBadge.replaceChildren(icon("bot", 13), agentName);
    li.querySelector(".status")!.textContent = STATUS_LABEL[job.status];
    li.querySelector(".job-time")!.textContent = elapsed(job);
    li.querySelector(".job-task")!.textContent = job.task;
    li.querySelector(".job-folder")!.textContent = job.folder;

    const result = li.querySelector<HTMLElement>(".job-result")!;
    result.textContent = job.result ?? "";
    result.hidden = !job.result;

    const cancelBtn = li.querySelector<HTMLButtonElement>(".job-cancel")!;
    cancelBtn.hidden = job.status !== "running";
    cancelBtn.addEventListener("click", () => invoke("cancel_job", { id: job.id }));

    const log = li.querySelector<HTMLPreElement>(".job-log")!;
    const toggle = li.querySelector<HTMLButtonElement>(".job-toggle")!;
    const open = expanded.has(job.id);
    log.textContent = job.log.join("\n");
    log.hidden = !open;
    toggle.textContent = open ? "esconder log" : `ver log (${job.log.length})`;
    toggle.addEventListener("click", () => {
      if (expanded.has(job.id)) expanded.delete(job.id);
      else expanded.add(job.id);
      renderJobs();
    });
    if (open) requestAnimationFrame(() => (log.scrollTop = log.scrollHeight));
    return li;
  }

  listen<JobEvent>("lilith://job", (e) => {
    const ev = e.payload;
    if (ev.kind === "updated") {
      jobs.set(ev.job.id, ev.job);
    } else {
      const job = jobs.get(ev.id);
      if (job) job.log.push(ev.line);
    }
    renderJobs();
  });

  // Atualiza os tempos de quem ainda está rodando.
  window.setInterval(() => {
    jobsEl.querySelectorAll<HTMLElement>('.job[data-status="running"]').forEach((li) => {
      const job = jobs.get(Number(li.dataset.id));
      if (job) li.querySelector(".job-time")!.textContent = elapsed(job);
    });
  }, 1000);

  // ── atividade ────────────────────────────────────────────────────────

  async function loadActivity() {
    const items = await invoke<Activity[]>("recent_activity", { limit: 80 });
    if (items.length === 0) {
      activityEl.innerHTML = `<li class="empty"><span data-icon="activity" data-size="28"></span>Nada ainda. Toda ação da Lilith aparece aqui.</li>`;
      hydrateIcons(activityEl);
      return;
    }
    activityEl.replaceChildren(
      ...items.map((a) => {
        const li = document.createElement("li");
        li.className = "activity-item";
        li.dataset.outcome = a.outcome;
        li.innerHTML = `<span class="act-time"></span><span class="act-name"></span><span class="badge outcome"></span>`;
        li.querySelector(".act-time")!.after(icon(actionIcon(a.action), 16));
        li.querySelector(".act-time")!.textContent = new Date(a.ts).toLocaleTimeString("pt-BR", {
          hour: "2-digit",
          minute: "2-digit",
        });
        li.querySelector(".act-name")!.textContent =
          actionLabel(a.action) + (a.channel !== "local" ? ` · ${a.channel}` : "");
        li.querySelector(".outcome")!.textContent = OUTCOME_LABEL[a.outcome] ?? a.outcome;
        return li;
      }),
    );
  }

  // ── permissões ("Sempre" salvos) ───────────────────────────────────

  async function loadRules() {
    const rules = await invoke<{ channel: string; scope: string; rule: string }[]>("list_rules");
    if (rules.length === 0) {
      rulesEl.innerHTML = `<li class="empty"><span data-icon="key" data-size="28"></span>Nenhuma permissão salva. Quando você escolhe “Sempre” num pedido, ela aparece aqui para você poder revogar.</li>`;
      hydrateIcons(rulesEl);
      return;
    }
    rulesEl.replaceChildren(
      ...rules.map((r) => {
        const li = document.createElement("li");
        li.className = "rule-item";
        li.innerHTML = `
          <div class="rule-text"><strong></strong><small></small></div>
          <span class="badge rule-kind"></span>
          <button class="icon-btn neon-soft rule-revoke" title="Revogar"></button>`;
        li.querySelector("strong")!.textContent = r.scope;
        li.querySelector("small")!.textContent = r.channel === "local" ? "neste PC" : `via ${r.channel}`;
        const kind = li.querySelector<HTMLElement>(".rule-kind")!;
        kind.textContent = r.rule === "allow" ? "sempre permitido" : r.rule === "deny" ? "bloqueado" : "perguntar";
        kind.dataset.rule = r.rule;
        const revoke = li.querySelector<HTMLButtonElement>(".rule-revoke")!;
        revoke.append(icon("trash", 15));
        revoke.addEventListener("click", async () => {
          await invoke("revoke_rule", { channel: r.channel, scope: r.scope });
          loadRules();
        });
        return li;
      }),
    );
  }

  getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (!focused) return;
    if (!activityEl.hidden) loadActivity();
    if (!rulesEl.hidden) loadRules();
  });

  // Títulos das conversas: carrega e acompanha renomeações em tempo real.
  listen<{ id: string; title: string }>("lilith://conversation", (e) => {
    titles.set(e.payload.id, e.payload.title);
    jobsEl.querySelectorAll<HTMLElement>(".group-head").forEach((h) => {
      if (h.dataset.conversation === e.payload.id) h.textContent = e.payload.title;
    });
  });

  Promise.all([
    call<{ id: string; title: string }[]>("list_conversations", undefined, []),
    call<Job[]>("list_jobs", undefined, []),
  ]).then(([convs, list]) => {
    convs.forEach((c) => titles.set(c.id, c.title));
    list.forEach((j) => jobs.set(j.id, j));
    renderJobs();
  });
}

function elapsed(job: Job): string {
  const end = job.finished_ms ?? Date.now();
  const s = Math.max(0, Math.round((end - job.started_ms) / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}min ${s % 60}s`;
}
