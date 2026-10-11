import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { makeDraggable } from "./drag";
import { actionLabel } from "./labels";

interface Reply {
  reply: string;
  action: string | null;
  emotion: "neutral" | "happy" | "concerned";
}

interface Approval {
  id: number;
  channel: string;
  action: string;
  scope: string;
  params: Record<string, unknown>;
}

export function mountChat(root: HTMLElement) {
  root.innerHTML = `
    <section class="glass chat">
      <header class="glass-head">
        <span class="sigil" aria-hidden="true"></span>
        <div class="glass-title">
          <strong>Lilith</strong>
          <small class="chat-status">online</small>
        </div>
        <button class="icon-btn" data-act="reset" title="Nova conversa">↺</button>
        <button class="icon-btn" data-act="close" title="Fechar">✕</button>
      </header>
      <ol class="chat-log" aria-live="polite"></ol>
      <form class="chat-input">
        <textarea rows="1" placeholder="Fala comigo…" aria-label="Mensagem"></textarea>
        <button type="submit" class="send" title="Enviar">➤</button>
        <button type="button" class="stop" title="Parar (Ctrl+Alt+Esc)" hidden>■</button>
      </form>
    </section>`;

  const log = root.querySelector<HTMLOListElement>(".chat-log")!;
  const form = root.querySelector<HTMLFormElement>(".chat-input")!;
  const input = form.querySelector("textarea")!;
  const stopBtn = form.querySelector<HTMLButtonElement>(".stop")!;
  const sendBtn = form.querySelector<HTMLButtonElement>(".send")!;
  const status = root.querySelector<HTMLElement>(".chat-status")!;
  makeDraggable(root.querySelector(".glass-head")!, () => invoke("chat_moved"));

  // Bolha da resposta em streaming; cada passo do agente reinicia o texto.
  let live: HTMLLIElement | null = null;
  let liveStep = -1;

  function add(role: "user" | "lilith" | "system", text: string): HTMLLIElement {
    const li = document.createElement("li");
    li.className = `msg ${role}`;
    li.textContent = text;
    log.append(li);
    log.scrollTop = log.scrollHeight;
    return li;
  }

  function setBusy(busy: boolean) {
    stopBtn.hidden = !busy;
    sendBtn.hidden = busy;
    status.textContent = busy ? "pensando…" : "online";
  }

  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    add("user", text);
    input.value = "";
    autosize();
    live = null;
    liveStep = -1;
    invoke("send_message", { text });
  });

  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      form.requestSubmit();
    } else if (e.key === "Escape") {
      invoke("hide_window", { label: "chat" });
    }
  });
  input.addEventListener("input", autosize);
  function autosize() {
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, 120)}px`;
  }

  stopBtn.addEventListener("click", () => invoke("cancel"));
  root.querySelector('[data-act="close"]')!.addEventListener("click", () => invoke("hide_window", { label: "chat" }));
  root.querySelector('[data-act="reset"]')!.addEventListener("click", () => {
    invoke("reset_conversation");
    log.replaceChildren();
    add("system", "Conversa nova. O que vamos fazer?");
  });

  listen<string>("lilith://state", (e) => setBusy(e.payload === "thinking"));

  listen<{ step: number; chunk: string }>("lilith://partial", (e) => {
    if (!live || liveStep !== e.payload.step) {
      live = add("lilith", "");
      liveStep = e.payload.step;
    }
    live.textContent += e.payload.chunk;
    log.scrollTop = log.scrollHeight;
  });

  listen<{ step: number; action: string }>("lilith://step", (e) => {
    // Texto que o modelo soltou antes de chamar a tool não é a resposta final.
    if (live && !live.textContent?.trim()) live.remove();
    live = null;
    const label = actionLabel(e.payload.action);
    add("system", `⚙ ${label}…`).classList.add("step");
  });

  listen<Reply>("lilith://reply", (e) => {
    const { reply, emotion } = e.payload;
    if (live) {
      live.textContent = reply;
    } else {
      live = add("lilith", reply);
    }
    live.dataset.emotion = emotion;
    live = null;
    liveStep = -1;
  });

  listen("lilith://cancelled", () => {
    live = null;
    add("system", "⏹ Parei tudo.");
  });

  listen<Approval>("lilith://approval", (e) => renderApproval(e.payload));
  listen<number>("lilith://approval-expired", (e) => {
    const card = log.querySelector<HTMLElement>(`[data-approval="${e.payload}"]`);
    if (card) settle(card, "Tempo esgotado — negado.");
  });

  function renderApproval(req: Approval) {
    const li = document.createElement("li");
    li.className = "msg approval";
    li.dataset.approval = String(req.id);
    const label = actionLabel(req.action);
    li.innerHTML = `
      <p class="approval-title">Posso fazer isso?</p>
      <p class="approval-action"></p>
      <pre class="approval-params"></pre>
      <p class="approval-scope"></p>
      <div class="approval-buttons">
        <button data-answer="deny">Negar</button>
        <button data-answer="once" class="primary">Permitir uma vez</button>
        <button data-answer="always">Sempre</button>
      </div>`;
    li.querySelector(".approval-action")!.textContent = label;
    li.querySelector(".approval-params")!.textContent = formatParams(req.params);
    li.querySelector(".approval-scope")!.textContent =
      `permissão: ${req.scope}${req.channel !== "local" ? ` · via ${req.channel}` : ""}`;
    li.querySelectorAll<HTMLButtonElement>("[data-answer]").forEach((btn) =>
      btn.addEventListener("click", async () => {
        const answer = btn.dataset.answer!;
        await invoke("answer_approval", { id: req.id, answer });
        settle(li, answer === "deny" ? "Negado." : answer === "once" ? "Permitido uma vez." : "Permitido sempre.");
      }),
    );
    log.append(li);
    log.scrollTop = log.scrollHeight;
    li.querySelector<HTMLButtonElement>(".primary")!.focus();
  }

  function settle(card: HTMLElement, text: string) {
    card.querySelector(".approval-buttons")?.remove();
    const p = document.createElement("p");
    p.className = "approval-result";
    p.textContent = text;
    card.append(p);
  }

  // Avisa no chat quando uma tarefa delegada termina.
  listen<{ kind: string; job?: { id: number; agent: string; status: string; result: string | null } }>(
    "lilith://job",
    (e) => {
      const job = e.payload.job;
      if (e.payload.kind !== "updated" || !job) return;
      if (job.status === "running") {
        add("system", `🛠 ${job.agent} começou a tarefa #${job.id} — acompanhe no painel`);
      } else if (job.status === "done" || job.status === "failed") {
        const head = job.status === "done" ? `✅ ${job.agent} terminou #${job.id}` : `⚠ ${job.agent} falhou em #${job.id}`;
        const body = (job.result ?? "").slice(0, 400);
        add("lilith", body ? `${head}:
${body}` : head);
      }
    },
  );

  add("lilith", "Oi! Tô aqui no cantinho. Pede o que precisar 💗");
  input.focus();
}

function formatParams(params: Record<string, unknown>): string {
  return Object.entries(params)
    .map(([k, v]) => `${k}: ${typeof v === "string" ? v : JSON.stringify(v)}`)
    .join("\n");
}
