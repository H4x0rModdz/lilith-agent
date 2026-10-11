import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { USER_AVATAR_SVG, fillSigil, lilithAvatar } from "./avatar";
import { makeDraggable } from "./drag";
import { mountElectric } from "./electric";
import { type IconName, actionIcon, hydrateIcons, icon } from "./icons";
import { actionLabel, actionVerb } from "./labels";

interface Reply {
  conversation: string;
  reply: string;
  action: string | null;
  emotion: "neutral" | "happy" | "concerned";
}

interface Approval {
  id: number;
  conversation: string;
  channel: string;
  action: string;
  scope: string;
  params: Record<string, unknown>;
  tainted: boolean;
}

interface Summary {
  id: string;
  title: string;
  created_ms: number;
  updated_ms: number;
  messages: number;
}

interface Saved {
  id: string;
  title: string;
  messages: { role: "user" | "lilith"; text: string; ts: number }[];
}

type Role = "user" | "lilith" | "system";

const NAMES: Record<"user" | "lilith", string> = { user: "Eu", lilith: "Lilith" };
const GREETING = "Oi! Tô aqui no cantinho. Pede o que precisar 💗";
const DEFAULT_TITLE = "Nova conversa";

/// Uma aba do chat = uma conversa com memória própria no backend (salva em disco).
interface Conversation {
  id: string;
  log: HTMLOListElement;
  tab: HTMLButtonElement;
  /// Bolha da resposta em streaming; cada passo do agente reinicia o texto.
  live: HTMLElement | null;
  liveStep: number;
  busy: boolean;
}

export async function mountChat(root: HTMLElement) {
  const avatarUrl = await lilithAvatar();
  root.innerHTML = `
    <section class="glass chat">
      <header class="glass-head">
        <span class="sigil" aria-hidden="true"></span>
        <div class="glass-title">
          <strong>Lilith</strong>
          <small class="chat-status">online</small>
        </div>
        <button class="icon-btn neon-soft" data-act="history" title="Conversas anteriores"><span data-icon="history" data-size="17"></span></button>
        <button class="icon-btn neon-soft" data-act="close" title="Fechar"><span data-icon="close" data-size="17"></span></button>
      </header>
      <nav class="chat-tabs">
        <div class="chat-tab-list" role="tablist" aria-label="Conversas abertas"></div>
        <button class="icon-btn neon-soft new-chat" title="Nova conversa"><span data-icon="plus" data-size="16"></span></button>
      </nav>
      <div class="chat-logs">
        <aside class="history" hidden>
          <div class="history-head">
            <strong>Conversas</strong>
            <button class="link" data-act="history-close">voltar</button>
          </div>
          <div class="history-list"></div>
        </aside>
      </div>
      <form class="chat-input">
        <ul class="slash-menu" role="listbox" aria-label="Comandos" hidden></ul>
        <textarea rows="1" placeholder="Fala comigo… (/ para comandos)" aria-label="Mensagem"></textarea>
        <button type="submit" class="send neon" title="Enviar"><span data-icon="send" data-size="18"></span></button>
        <button type="button" class="stop neon danger" title="Parar tudo (Ctrl+Alt+Esc)" hidden><span data-icon="stop" data-size="16"></span></button>
      </form>
    </section>`;
  hydrateIcons(root);
  mountElectric(root.querySelector<HTMLElement>(".glass")!);

  const logs = root.querySelector<HTMLElement>(".chat-logs")!;
  const tabList = root.querySelector<HTMLElement>(".chat-tab-list")!;
  const history = root.querySelector<HTMLElement>(".history")!;
  const historyList = root.querySelector<HTMLElement>(".history-list")!;
  const form = root.querySelector<HTMLFormElement>(".chat-input")!;
  const input = form.querySelector("textarea")!;
  const stopBtn = form.querySelector<HTMLButtonElement>(".stop")!;
  const sendBtn = form.querySelector<HTMLButtonElement>(".send")!;
  const status = root.querySelector<HTMLElement>(".chat-status")!;
  makeDraggable(root.querySelector(".glass-head")!, () => invoke("chat_moved"));
  fillSigil(root.querySelector(".glass-head .sigil")!);

  const slashMenu = form.querySelector<HTMLUListElement>(".slash-menu")!;
  // Atalhos (bolhas em conversa nova + lista do "/"), editáveis no painel.
  let prompts: string[] = await invoke<string[]>("get_quick_prompts");
  listen<string[]>("lilith://quick-prompts", (e) => {
    prompts = e.payload;
    open.forEach((c) => {
      const chips = c.log.querySelector(".quick-prompts");
      if (chips) chips.replaceWith(...quickPrompts(c));
    });
  });

  const open = new Map<string, Conversation>();
  let active!: Conversation;
  let counter = 0;

  // ── abas ──────────────────────────────────────────────────────────────

  /// Abre uma aba: conversa nova, ou uma salva (com as falas antigas).
  function openTab(saved?: Saved): Conversation {
    const existing = saved && open.get(saved.id);
    if (existing) {
      activate(existing);
      return existing;
    }
    counter += 1;
    const id = saved?.id ?? `c${Date.now().toString(36)}-${counter}`;
    const log = document.createElement("ol");
    log.className = "chat-log";
    log.setAttribute("aria-live", "polite");
    logs.append(log);

    const tab = document.createElement("button");
    tab.className = "chat-tab";
    tab.setAttribute("role", "tab");
    tab.innerHTML = `<span class="chat-tab-title"></span><span class="chat-tab-dot" hidden></span><span class="chat-tab-close" title="Fechar aba"></span>`;
    tab.querySelector(".chat-tab-close")!.append(icon("close", 12));
    tabList.append(tab);

    const conv: Conversation = { id, log, tab, live: null, liveStep: -1, busy: false };
    setTabTitle(conv, saved?.title ?? DEFAULT_TITLE);
    tab.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).closest(".chat-tab-close")) closeTab(conv);
      else activate(conv);
    });
    // Duplo clique no nome da aba: renomear.
    tab.addEventListener("dblclick", (e) => {
      e.preventDefault();
      startRename(tab.querySelector<HTMLElement>(".chat-tab-title")!, id);
    });
    open.set(id, conv);

    if (saved && saved.messages.length > 0) {
      for (const m of saved.messages) add(conv, m.role, m.text, undefined, new Date(m.ts));
    } else {
      add(conv, "lilith", GREETING);
      conv.log.append(...quickPrompts(conv));
    }
    activate(conv);
    return conv;
  }

  function setTabTitle(conv: Conversation, title: string) {
    conv.tab.querySelector(".chat-tab-title")!.textContent = title;
    conv.tab.title = `${title} (duplo clique para renomear)`;
  }

  function activate(conv: Conversation) {
    active = conv;
    hideHistory();
    open.forEach((c) => {
      const on = c === conv;
      c.log.hidden = !on;
      c.tab.setAttribute("aria-selected", String(on));
    });
    conv.tab.querySelector<HTMLElement>(".chat-tab-dot")!.hidden = true;
    conv.tab.scrollIntoView({ block: "nearest", inline: "nearest" });
    conv.log.scrollTop = conv.log.scrollHeight;
    refreshBusy();
    input.focus();
  }

  /// Fecha a aba (a conversa continua no histórico).
  function closeTab(conv: Conversation) {
    invoke("close_conversation", { conversation: conv.id });
    open.delete(conv.id);
    conv.log.remove();
    conv.tab.remove();
    if (open.size === 0) openTab();
    else if (active === conv) activate([...open.values()].pop()!);
  }

  /// Marca a aba com um pontinho quando algo chega numa conversa em segundo plano.
  function notify(conv: Conversation) {
    if (conv !== active) conv.tab.querySelector<HTMLElement>(".chat-tab-dot")!.hidden = false;
  }

  // ── atalhos ───────────────────────────────────────────────────────────

  /// Bolhas de escrita rápida: clicou, enviou. Somem na primeira mensagem.
  function quickPrompts(conv: Conversation): HTMLElement[] {
    if (prompts.length === 0) return [];
    const li = document.createElement("li");
    li.className = "quick-prompts";
    for (const p of prompts) {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "quick-chip";
      chip.textContent = p;
      chip.addEventListener("click", () => send(conv, p));
      li.append(chip);
    }
    return [li];
  }

  // ── comandos "/" ──────────────────────────────────────────────────────

  interface Command {
    name: string;
    /// Dica do argumento (vazio = sem argumento).
    arg: string;
    help: string;
    run: (arg: string) => unknown;
  }

  const note = (text: string, ic: IconName = "sparkles") => add(active, "system", text, ic);

  const COMMANDS: Command[] = [
    { name: "new", arg: "", help: "abre uma conversa nova", run: () => void openTab() },
    {
      name: "clear",
      arg: "",
      help: "limpa a conversa atual",
      run: () => {
        invoke("reset_conversation", { conversation: active.id });
        active.log.replaceChildren();
        note("Conversa limpa. O que vamos fazer?");
      },
    },
    {
      name: "rename",
      arg: "<nome>",
      help: "renomeia a conversa atual",
      run: async (arg) => {
        if (!arg) return note("Use: /rename <nome>", "warn");
        await invoke("rename_conversation", { conversation: active.id, title: arg });
      },
    },
    { name: "history", arg: "", help: "mostra as conversas anteriores", run: () => void showHistory() },
    { name: "stop", arg: "", help: "para tudo o que ela está fazendo (Ctrl+Alt+Esc)", run: () => void invoke("cancel") },
    {
      name: "remember",
      arg: "<algo>",
      help: "pede para ela guardar algo na memória",
      run: (arg) => (arg ? send(active, `lembra que ${arg}`) : note("Use: /remember <algo>", "warn")),
    },
    {
      name: "forget",
      arg: "<assunto>",
      help: "apaga um fato da memória dela",
      run: async (arg) => {
        if (!arg) return note("Use: /forget <assunto>", "warn");
        const ok = await invoke<boolean>("forget_fact", { key: arg });
        note(ok ? `Esqueci “${arg}”.` : `Não achei “${arg}” na memória (veja em /settings).`, ok ? "check" : "warn");
      },
    },
    { name: "panel", arg: "", help: "abre o painel da Lilith", run: () => void invoke("show_panel_tab", { tab: "jobs" }) },
    { name: "tasks", arg: "", help: "tarefas delegadas a agentes", run: () => void invoke("show_panel_tab", { tab: "jobs" }) },
    { name: "permissions", arg: "", help: "permissões salvas (“Sempre”)", run: () => void invoke("show_panel_tab", { tab: "rules" }) },
    { name: "settings", arg: "", help: "configurações e o que ela sabe sobre você", run: () => void invoke("show_panel_tab", { tab: "settings" }) },
    {
      name: "help",
      arg: "",
      help: "lista os comandos",
      run: () => note(COMMANDS.map((c) => `/${c.name}${c.arg ? " " + c.arg : ""} — ${c.help}`).join("\n"), "sparkles"),
    },
  ];

  let slashItems: Command[] = [];
  let slashIndex = 0;

  /// "/ren algo" → { name: "ren", arg: "algo" }.
  function parseSlash(v: string) {
    const body = v.slice(1);
    const space = body.search(/\s/);
    return space < 0 ? { name: fold(body), arg: "" } : { name: fold(body.slice(0, space)), arg: body.slice(space).trim() };
  }

  function updateSlash() {
    const v = input.value;
    if (!v.startsWith("/") || v.includes("\n")) return closeSlash();
    const { name, arg } = parseSlash(v);
    // Já digitou o argumento de um comando exato: só ele na lista.
    slashItems = arg
      ? COMMANDS.filter((c) => c.name === name)
      : COMMANDS.filter((c) => c.name.startsWith(name) || fold(c.help).includes(name));
    if (slashItems.length === 0) return closeSlash();
    slashIndex = Math.min(slashIndex, slashItems.length - 1);
    slashMenu.replaceChildren(
      ...slashItems.map((c, i) => {
        const li = document.createElement("li");
        li.setAttribute("role", "option");
        li.className = "slash-item";
        li.setAttribute("aria-selected", String(i === slashIndex));
        const head = document.createElement("span");
        head.className = "slash-name";
        head.textContent = `/${c.name}${c.arg ? " " + c.arg : ""}`;
        const help = document.createElement("span");
        help.className = "slash-help";
        help.textContent = c.help;
        li.append(head, help);
        li.addEventListener("mousedown", (e) => {
          e.preventDefault();
          pick(c);
        });
        return li;
      }),
    );
    slashMenu.hidden = false;
    // A lista mostra ~5 por vez; mantém o escolhido à vista ao usar as setas.
    slashMenu.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }

  /// Escolheu um comando: com argumento pendente, completa e espera o
  /// texto; senão executa.
  function pick(c: Command) {
    const { name, arg } = parseSlash(input.value);
    if (c.arg && !(name === c.name && arg)) {
      input.value = `/${c.name} `;
      closeSlash();
      input.focus();
      return;
    }
    runCommand(c, name === c.name ? arg : "");
  }

  function runCommand(c: Command, arg: string) {
    input.value = "";
    autosize();
    closeSlash();
    void c.run(arg);
  }

  function closeSlash() {
    slashMenu.hidden = true;
    slashItems = [];
    slashIndex = 0;
  }

  // ── renomear ──────────────────────────────────────────────────────────

  /// Troca o texto por um campo; Enter salva (o backend avisa todas as
  /// janelas — abas, histórico e painel atualizam juntos), Esc cancela.
  function startRename(label: HTMLElement, id: string) {
    const current = label.textContent ?? "";
    const field = document.createElement("input");
    field.className = "rename-field";
    field.value = current;
    field.maxLength = 60;
    label.replaceChildren(field);
    field.focus();
    field.select();
    let done = false;
    const finish = async (save: boolean) => {
      if (done) return;
      done = true;
      const title = field.value.trim();
      label.textContent = current;
      if (save && title && title !== current) {
        await invoke("rename_conversation", { conversation: id, title });
      }
    };
    field.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") finish(true);
      else if (e.key === "Escape") finish(false);
    });
    field.addEventListener("click", (e) => e.stopPropagation());
    field.addEventListener("blur", () => finish(true));
  }

  // ── histórico (por dia) ───────────────────────────────────────────────

  async function showHistory() {
    const items = await invoke<Summary[]>("list_conversations");
    historyList.replaceChildren();
    if (items.length === 0) {
      const empty = document.createElement("p");
      empty.className = "empty";
      empty.textContent = "Nenhuma conversa salva ainda.";
      historyList.append(empty);
    }
    let lastDay = "";
    for (const c of items) {
      const day = dayLabel(c.updated_ms);
      if (day !== lastDay) {
        const h = document.createElement("h4");
        h.className = "history-day";
        h.textContent = day;
        historyList.append(h);
        lastDay = day;
      }
      const row = document.createElement("div");
      row.className = "history-item";
      row.dataset.id = c.id;
      if (open.has(c.id)) row.classList.add("is-open");
      row.innerHTML = `
        <button class="history-open"><span class="history-title"></span><small></small></button>
        <button class="icon-btn neon-soft history-rename" title="Renomear"></button>
        <button class="icon-btn neon-soft history-delete" title="Apagar"></button>`;
      row.querySelector(".history-title")!.textContent = c.title;
      row.querySelector("small")!.textContent =
        `${new Date(c.updated_ms).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" })} · ${c.messages} mensagens`;
      row.querySelector(".history-rename")!.append(icon("pencil", 14));
      row.querySelector(".history-delete")!.append(icon("trash", 14));
      row.querySelector(".history-open")!.addEventListener("click", async () => {
        const saved = await invoke<Saved | null>("load_conversation", { conversation: c.id });
        if (saved) openTab(saved);
      });
      row.querySelector(".history-rename")!.addEventListener("click", () =>
        startRename(row.querySelector<HTMLElement>(".history-title")!, c.id),
      );
      row.querySelector(".history-delete")!.addEventListener("click", async () => {
        await invoke("delete_conversation", { conversation: c.id });
        showHistory();
      });
      historyList.append(row);
    }
    history.hidden = false;
  }

  function hideHistory() {
    history.hidden = true;
  }

  // ── mensagens ─────────────────────────────────────────────────────────

  /// Adiciona uma mensagem e devolve a bolha (onde vai o texto). Mensagens
  /// do usuário e da Lilith levam avatar, nome e hora; as de sistema não.
  function add(conv: Conversation, role: Role, text: string, sysIcon?: IconName, when = new Date()): HTMLElement {
    const li = document.createElement("li");
    if (role === "system") {
      li.className = "msg system";
      setSystemText(li, text, sysIcon);
    } else {
      li.className = `msg-row ${role}`;
      li.innerHTML = `
        <span class="avatar"></span>
        <div class="msg-body">
          <div class="msg-meta"><strong></strong><time></time></div>
          <div class="msg"></div>
        </div>`;
      const avatar = li.querySelector<HTMLElement>(".avatar")!;
      if (role === "user") {
        avatar.innerHTML = USER_AVATAR_SVG;
      } else if (avatarUrl) {
        const img = document.createElement("img");
        img.src = avatarUrl;
        img.alt = "";
        avatar.append(img);
      } else {
        avatar.classList.add("sigil");
      }
      const time = li.querySelector("time")!;
      time.dateTime = when.toISOString();
      time.textContent = when.toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
      li.querySelector("strong")!.textContent = NAMES[role];
      li.querySelector(".msg")!.textContent = text;
    }
    conv.log.append(li);
    conv.log.scrollTop = conv.log.scrollHeight;
    return role === "system" ? li : li.querySelector<HTMLElement>(".msg")!;
  }

  /// Linha de sistema = ícone (opcional) + texto.
  function setSystemText(li: HTMLElement, text: string, sysIcon?: IconName) {
    const span = document.createElement("span");
    span.textContent = text;
    li.replaceChildren(...(sysIcon ? [icon(sysIcon, 14), span] : [span]));
  }

  function refreshBusy() {
    stopBtn.hidden = !active.busy;
    sendBtn.hidden = active.busy;
    const others = [...open.values()].filter((c) => c !== active && c.busy).length;
    status.textContent = active.busy ? "pensando…" : others > 0 ? `trabalhando em ${others} outra(s) aba(s)` : "online";
  }

  // ── entrada ───────────────────────────────────────────────────────────

  function send(conv: Conversation, text: string) {
    text = text.trim();
    if (!text) return;
    if (conv !== active) activate(conv);
    conv.log.querySelector(".quick-prompts")?.remove();
    add(conv, "user", text);
    input.value = "";
    autosize();
    closeSlash();
    conv.live = null;
    conv.liveStep = -1;
    invoke("send_message", { text, conversation: conv.id });
  }

  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const v = input.value.trim();
    if (v.startsWith("/")) {
      const { name, arg } = parseSlash(v);
      const c = COMMANDS.find((x) => x.name === name);
      if (c) runCommand(c, arg);
      else note(`Comando desconhecido: /${name}. Digite /help.`, "warn");
      return;
    }
    send(active, v);
  });

  input.addEventListener("keydown", (e) => {
    // Lista do "/" aberta: setas escolhem, Enter envia, Tab só preenche.
    if (!slashMenu.hidden && slashItems.length > 0) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const n = slashItems.length;
        slashIndex = (slashIndex + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
        updateSlash();
        return;
      }
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        pick(slashItems[slashIndex]);
        return;
      }
      if (e.key === "Tab") {
        e.preventDefault();
        const c = slashItems[slashIndex];
        input.value = `/${c.name}${c.arg ? " " : ""}`;
        updateSlash();
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        closeSlash();
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      form.requestSubmit();
    } else if (e.key === "Escape") {
      if (!history.hidden) hideHistory();
      else invoke("hide_window", { label: "chat" });
    }
  });
  input.addEventListener("input", () => {
    autosize();
    updateSlash();
  });
  input.addEventListener("blur", () => window.setTimeout(closeSlash, 120));
  function autosize() {
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, 120)}px`;
  }

  stopBtn.addEventListener("click", () => invoke("cancel"));
  root.querySelector(".new-chat")!.addEventListener("click", () => openTab());
  root.querySelector('[data-act="close"]')!.addEventListener("click", () => invoke("hide_window", { label: "chat" }));
  root.querySelector('[data-act="history"]')!.addEventListener("click", () =>
    history.hidden ? showHistory() : hideHistory(),
  );
  root.querySelector('[data-act="history-close"]')!.addEventListener("click", hideHistory);

  // ── eventos do backend (sempre com o id da conversa) ──────────────────

  const byId = (id: string) => open.get(id);

  // Título novo (automático na 1ª fala ou renomeado em qualquer janela).
  listen<{ id: string; title: string }>("lilith://conversation", (e) => {
    const conv = byId(e.payload.id);
    if (conv) setTabTitle(conv, e.payload.title);
    const row = historyList.querySelector<HTMLElement>(`.history-item[data-id="${CSS.escape(e.payload.id)}"] .history-title`);
    if (row && !row.querySelector("input")) row.textContent = e.payload.title;
  });

  listen<{ id: string }>("lilith://conversation-deleted", (e) => {
    const conv = byId(e.payload.id);
    if (conv) closeTab(conv);
  });

  listen<{ conversation: string; state: string }>("lilith://state", (e) => {
    const conv = byId(e.payload.conversation);
    if (!conv) return;
    conv.busy = e.payload.state === "thinking";
    refreshBusy();
  });

  listen<{ conversation: string; step: number; chunk: string }>("lilith://partial", (e) => {
    const conv = byId(e.payload.conversation);
    if (!conv) return;
    if (!conv.live || conv.liveStep !== e.payload.step) {
      conv.live = add(conv, "lilith", "");
      conv.liveStep = e.payload.step;
    }
    conv.live.textContent += e.payload.chunk;
    conv.log.scrollTop = conv.log.scrollHeight;
  });

  listen<{ conversation: string; step: number; action: string }>("lilith://step", (e) => {
    const conv = byId(e.payload.conversation);
    if (!conv) return;
    // Texto de um passo que termina chamando uma tool é o modelo "pensando
    // alto" — nunca a resposta. Some da tela; a resposta final vem depois.
    if (conv.live) conv.live.closest("li")?.remove();
    conv.live = null;
    add(conv, "system", `${actionLabel(e.payload.action)}…`, actionIcon(e.payload.action)).classList.add("step");
  });

  listen<Reply>("lilith://reply", (e) => {
    const conv = byId(e.payload.conversation);
    if (!conv) return;
    const { reply, emotion } = e.payload;
    if (conv.live) conv.live.textContent = reply;
    else conv.live = add(conv, "lilith", reply);
    conv.live.dataset.emotion = emotion;
    conv.live = null;
    conv.liveStep = -1;
    notify(conv);
  });

  listen<{ conversation: string }>("lilith://cancelled", (e) => {
    const conv = byId(e.payload.conversation);
    if (!conv) return;
    conv.live = null;
    add(conv, "system", "Parei.", "stop");
  });

  // A linha do passo bloqueado vira o aviso (ele nunca aconteceu).
  listen<{ conversation: string; action: string }>("lilith://blocked", (e) => {
    const conv = byId(e.payload.conversation);
    if (!conv) return;
    const text = `O texto lido pedia para ${actionVerb(e.payload.action)}. Ignorei — só obedeço o chat.`;
    const steps = conv.log.querySelectorAll<HTMLElement>(".msg.system.step");
    const last = steps[steps.length - 1];
    if (last && last === conv.log.lastElementChild) {
      setSystemText(last, text, "shield");
      last.classList.replace("step", "blocked");
    } else {
      add(conv, "system", text, "shield").classList.add("blocked");
    }
  });

  listen<Approval>("lilith://approval", (e) => {
    const conv = byId(e.payload.conversation) ?? active;
    activate(conv);
    renderApproval(conv, e.payload);
  });
  listen<number>("lilith://approval-expired", (e) => {
    const card = logs.querySelector<HTMLElement>(`[data-approval="${e.payload}"]`);
    if (card) settle(card, "Tempo esgotado — negado.");
  });

  function renderApproval(conv: Conversation, req: Approval) {
    const li = document.createElement("li");
    li.className = "msg approval";
    li.dataset.approval = String(req.id);
    li.innerHTML = `
      <p class="approval-title"><span data-icon="shield" data-size="16"></span>Posso fazer isso?</p>
      <p class="approval-action"></p>
      <pre class="approval-params"></pre>
      <p class="approval-scope"></p>
      <div class="approval-buttons">
        <button data-answer="deny">Negar</button>
        <button data-answer="once" class="primary neon">Permitir uma vez</button>
        <button data-answer="always">Sempre</button>
      </div>`;
    hydrateIcons(li);
    if (req.tainted) {
      const warn = document.createElement("p");
      warn.className = "approval-taint";
      const msg = document.createElement("span");
      msg.textContent = "Isso apareceu depois de eu ler conteúdo de fora (janela, arquivo ou web). Confira se foi você que pediu.";
      warn.append(icon("warn", 16), msg);
      li.querySelector(".approval-title")!.after(warn);
      li.querySelector('[data-answer="always"]')!.remove();
    }
    li.querySelector(".approval-action")!.textContent = actionLabel(req.action);
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
    conv.log.append(li);
    conv.log.scrollTop = conv.log.scrollHeight;
    li.querySelector<HTMLButtonElement>(".primary")!.focus();
  }

  function settle(card: HTMLElement, text: string) {
    card.querySelector(".approval-buttons")?.remove();
    const p = document.createElement("p");
    p.className = "approval-result";
    p.textContent = text;
    card.append(p);
  }

  // Tarefas delegadas avisam na conversa que pediu (ou na aba em uso).
  listen<{
    kind: string;
    job?: { id: number; agent: string; status: string; result: string | null; conversation: string | null };
  }>("lilith://job", (e) => {
    const job = e.payload.job;
    if (e.payload.kind !== "updated" || !job) return;
    const conv = (job.conversation && byId(job.conversation)) || active;
    if (job.status === "running") {
      add(conv, "system", `${job.agent} começou a tarefa #${job.id} — acompanhe no painel`, "bot");
    } else if (job.status === "done" || job.status === "failed") {
      const head = job.status === "done" ? `${job.agent} terminou #${job.id}` : `${job.agent} falhou em #${job.id}`;
      const body = (job.result ?? "").slice(0, 400);
      add(conv, "lilith", body ? `${head}:\n${body}` : head);
      notify(conv);
    }
  });

  // Começa pela conversa de hoje mais recente, se houver; senão, uma nova.
  const recent = (await invoke<Summary[]>("list_conversations"))[0];
  const saved = recent && isToday(recent.updated_ms)
    ? await invoke<Saved | null>("load_conversation", { conversation: recent.id })
    : null;
  openTab(saved ?? undefined);
}

/// Minúsculas e sem acento, para filtrar atalhos.
function fold(s: string): string {
  return s.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

function isToday(ms: number): boolean {
  return new Date(ms).toDateString() === new Date().toDateString();
}

/// "Hoje", "Ontem", dia da semana (até 6 dias) ou a data.
export function dayLabel(ms: number): string {
  const d = new Date(ms);
  const today = new Date();
  const startOf = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((startOf(today) - startOf(d)) / 86_400_000);
  if (days <= 0) return "Hoje";
  if (days === 1) return "Ontem";
  if (days < 7) {
    const w = d.toLocaleDateString("pt-BR", { weekday: "long" });
    return w.charAt(0).toUpperCase() + w.slice(1);
  }
  return d.toLocaleDateString("pt-BR", { day: "2-digit", month: "2-digit", year: "numeric" });
}

function formatParams(params: Record<string, unknown>): string {
  return Object.entries(params)
    .map(([k, v]) => `${k}: ${typeof v === "string" ? v : JSON.stringify(v)}`)
    .join("\n");
}
