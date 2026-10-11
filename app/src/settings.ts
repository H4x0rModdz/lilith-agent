import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { hydrateIcons, icon } from "./icons";

interface Settings {
  user_name: string;
  persona: string;
  auto_learn: boolean;
  chat_minutes: number;
}

interface Fact {
  key: string;
  value: string;
  source: "user" | "learned";
  updated_ms: number;
}

/// Aba "Configurações" do painel: comportamento inicial da Lilith e o que
/// ela sabe sobre o usuário (configurado aqui ou aprendido nas conversas).
export function mountSettings(view: HTMLElement) {
  view.innerHTML = `
    <form class="settings">
      <section class="settings-card">
        <h3><span data-icon="sparkles" data-size="16"></span>Comportamento</h3>
        <label class="field">
          <span>Como ela te chama</span>
          <input name="user_name" maxlength="40" placeholder="ex.: Lobo" />
        </label>
        <label class="field">
          <span>Jeito dela</span>
          <textarea name="persona" maxlength="800" rows="3"
            placeholder="ex.: carinhosa e meio provocadora, usa gírias, respostas curtas"></textarea>
        </label>
        <label class="field inline">
          <span>Ler mensagens dos últimos</span>
          <input name="chat_minutes" type="number" min="1" max="1440" />
          <span>min</span>
        </label>
        <label class="switch">
          <input name="auto_learn" type="checkbox" />
          <span class="switch-track"><span class="switch-thumb"></span></span>
          <span>Aprender sozinha com as conversas</span>
        </label>
        <div class="settings-actions">
          <small class="settings-saved" hidden>Salvo ✓</small>
          <button type="submit" class="neon save-btn">Salvar</button>
        </div>
      </section>

      <section class="settings-card">
        <h3><span data-icon="send" data-size="16"></span>Atalhos do chat</h3>
        <small class="hint">Aparecem como bolhas em toda conversa nova: um clique e já vai para ela.</small>
        <ul class="facts prompts-list"></ul>
        <div class="fact-add">
          <input class="prompt-new" maxlength="120" placeholder="ex.: Lê as mensagens novas do WhatsApp" />
          <button type="button" class="icon-btn neon-soft prompt-add-btn" title="Adicionar atalho"><span data-icon="plus" data-size="15"></span></button>
        </div>
      </section>

      <section class="settings-card">
        <h3><span data-icon="key" data-size="16"></span>O que ela sabe sobre você</h3>
        <ul class="facts"></ul>
        <div class="fact-add">
          <input class="fact-key" maxlength="40" placeholder="assunto (ex.: time)" />
          <input class="fact-value" maxlength="200" placeholder="o quê (ex.: Corinthians)" />
          <button type="button" class="icon-btn neon-soft fact-add-btn" title="Adicionar"><span data-icon="plus" data-size="15"></span></button>
        </div>
      </section>
    </form>`;
  hydrateIcons(view);

  const form = view.querySelector<HTMLFormElement>(".settings")!;
  const saved = view.querySelector<HTMLElement>(".settings-saved")!;
  const factsEl = view.querySelector<HTMLUListElement>(".facts")!;
  const keyIn = view.querySelector<HTMLInputElement>(".fact-key")!;
  const valueIn = view.querySelector<HTMLInputElement>(".fact-value")!;
  const field = (n: string) => form.elements.namedItem(n) as HTMLInputElement;
  const promptsEl = view.querySelector<HTMLUListElement>(".prompts-list")!;
  const promptIn = view.querySelector<HTMLInputElement>(".prompt-new")!;
  let prompts: string[] = [];

  function renderPrompts() {
    if (prompts.length === 0) {
      const li = document.createElement("li");
      li.className = "facts-empty";
      li.textContent = "Sem atalhos — o chat começa limpo.";
      promptsEl.replaceChildren(li);
      return;
    }
    promptsEl.replaceChildren(
      ...prompts.map((p, i) => {
        const li = document.createElement("li");
        li.className = "fact";
        const text = document.createElement("div");
        text.className = "fact-text";
        const span = document.createElement("span");
        span.textContent = p;
        span.style.color = "var(--text)";
        text.append(span);
        const del = document.createElement("button");
        del.type = "button";
        del.className = "icon-btn neon-soft";
        del.title = "Remover";
        del.append(icon("trash", 14));
        del.addEventListener("click", () => savePrompts(prompts.filter((_, j) => j !== i)));
        li.append(text, del);
        return li;
      }),
    );
  }

  async function savePrompts(next: string[]) {
    prompts = await invoke<string[]>("save_quick_prompts", { prompts: next });
    renderPrompts();
  }

  function addPrompt() {
    const p = promptIn.value.trim();
    if (!p) return;
    promptIn.value = "";
    savePrompts([...prompts, p]);
  }
  view.querySelector(".prompt-add-btn")!.addEventListener("click", addPrompt);
  promptIn.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      addPrompt();
    }
  });

  async function load() {
    const s = await invoke<Settings>("get_settings");
    field("user_name").value = s.user_name;
    (form.elements.namedItem("persona") as HTMLTextAreaElement).value = s.persona;
    field("chat_minutes").value = String(s.chat_minutes);
    field("auto_learn").checked = s.auto_learn;
    prompts = await invoke<string[]>("get_quick_prompts");
    renderPrompts();
    await loadFacts();
  }

  async function loadFacts() {
    const facts = await invoke<Fact[]>("list_facts");
    if (facts.length === 0) {
      const li = document.createElement("li");
      li.className = "facts-empty";
      li.textContent = "Nada ainda. Ela aprende conversando — ou adicione aqui embaixo.";
      factsEl.replaceChildren(li);
      return;
    }
    factsEl.replaceChildren(
      ...facts.map((f) => {
        const li = document.createElement("li");
        li.className = "fact";
        li.innerHTML = `<div class="fact-text"><strong></strong><span></span></div><span class="badge fact-source"></span>`;
        li.querySelector("strong")!.textContent = f.key;
        li.querySelector(".fact-text span")!.textContent = f.value;
        const badge = li.querySelector<HTMLElement>(".fact-source")!;
        badge.textContent = f.source === "user" ? "você" : "aprendido";
        badge.dataset.source = f.source;
        const del = document.createElement("button");
        del.type = "button";
        del.className = "icon-btn neon-soft";
        del.title = "Esquecer";
        del.append(icon("trash", 14));
        del.addEventListener("click", () => invoke("forget_fact", { key: f.key }));
        li.append(del);
        return li;
      }),
    );
  }

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const settings: Settings = {
      user_name: field("user_name").value,
      persona: (form.elements.namedItem("persona") as HTMLTextAreaElement).value,
      chat_minutes: Math.max(1, Math.min(1440, Number(field("chat_minutes").value) || 5)),
      auto_learn: field("auto_learn").checked,
    };
    await invoke("save_settings", { settings });
    saved.hidden = false;
    window.setTimeout(() => (saved.hidden = true), 1800);
  });

  async function addFact() {
    const key = keyIn.value.trim();
    const value = valueIn.value.trim();
    if (!key || !value) return;
    await invoke("add_fact", { key, value });
    keyIn.value = "";
    valueIn.value = "";
    keyIn.focus();
  }
  view.querySelector(".fact-add-btn")!.addEventListener("click", addFact);
  valueIn.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      addFact();
    }
  });

  // Fatos novos (aprendidos numa conversa, ou mudados aqui) em tempo real.
  listen("lilith://facts", () => loadFacts());

  return { load };
}
