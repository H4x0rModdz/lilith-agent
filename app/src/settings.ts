import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { hydrateIcons, icon } from "./icons";

interface Settings {
  user_name: string;
  persona: string;
  auto_learn: boolean;
  chat_minutes: number;
  auto_update: boolean;
  voice: boolean;
}

interface VoiceStatus {
  installed: boolean;
  installing: boolean;
}

interface Reminder {
  id: number;
  due: string;
  at: string;
  text: string;
}

interface AppInfo {
  version: string;
  autostart: boolean;
}

interface Term {
  term: string;
  meaning: string;
  source: "user" | "learned";
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
          <input name="user_name" maxlength="40" placeholder="ex.: Alex" />
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
        <label class="switch">
          <input name="auto_update" type="checkbox" />
          <span class="switch-track"><span class="switch-thumb"></span></span>
          <span>Atualizar sozinha ao abrir</span>
        </label>
        <label class="switch">
          <input name="voice" type="checkbox" />
          <span class="switch-track"><span class="switch-thumb"></span></span>
          <span>Falar as respostas em voz alta</span>
        </label>
        <div class="update-row voice-row">
          <small class="hint voice-status"></small>
          <span class="voice-actions">
            <button type="button" class="neon-soft voice-test" hidden>Testar</button>
            <button type="button" class="neon-soft voice-install" hidden>Instalar a voz (≈150 MB)</button>
          </span>
        </div>
        <label class="switch">
          <input name="autostart" type="checkbox" />
          <span class="switch-track"><span class="switch-thumb"></span></span>
          <span>Iniciar com o Windows</span>
        </label>
        <div class="update-row">
          <small class="hint update-status"></small>
          <button type="button" class="neon-soft update-btn">Procurar atualização</button>
        </div>
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
        <h3><span data-icon="sparkles" data-size="16"></span>Lembretes</h3>
        <small class="hint">Peça no chat: “me lembra dia 12/10 de abrir o discord”. Ela avisa quando o chat abrir naquele dia.</small>
        <ul class="facts reminders-list"></ul>
      </section>

      <section class="settings-card">
        <h3><span data-icon="sparkles" data-size="16"></span>Suas gírias</h3>
        <small class="hint">Palavras ou apelidos que ela deve entender do seu jeito. Só entram na conversa quando você usa a palavra. Também vale dizer no chat: “fulano quer dizer amigo”.</small>
        <ul class="facts terms-list"></ul>
        <div class="fact-add">
          <input class="term-key" maxlength="40" placeholder="gíria (ex.: zap)" />
          <input class="term-value" maxlength="120" placeholder="significa (ex.: WhatsApp)" />
          <button type="button" class="icon-btn neon-soft term-add-btn" title="Adicionar"><span data-icon="plus" data-size="15"></span></button>
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

  const termsEl = view.querySelector<HTMLUListElement>(".terms-list")!;
  const termKeyIn = view.querySelector<HTMLInputElement>(".term-key")!;
  const termValueIn = view.querySelector<HTMLInputElement>(".term-value")!;

  async function loadTerms() {
    const terms = await invoke<Term[]>("list_glossary");
    if (terms.length === 0) {
      const li = document.createElement("li");
      li.className = "facts-empty";
      li.textContent = "Nenhuma gíria sua ainda. Ela já entende as comuns (vc, tb, blz, cmg…).";
      termsEl.replaceChildren(li);
      return;
    }
    termsEl.replaceChildren(
      ...terms.map((t) => {
        const li = document.createElement("li");
        li.className = "fact";
        li.innerHTML = `<div class="fact-text"><strong></strong><span></span></div><span class="badge fact-source"></span>`;
        li.querySelector("strong")!.textContent = t.term;
        li.querySelector(".fact-text span")!.textContent = t.meaning;
        const badge = li.querySelector<HTMLElement>(".fact-source")!;
        badge.textContent = t.source === "user" ? "você" : "aprendido";
        badge.dataset.source = t.source;
        const del = document.createElement("button");
        del.type = "button";
        del.className = "icon-btn neon-soft";
        del.title = "Esquecer";
        del.append(icon("trash", 14));
        del.addEventListener("click", () => invoke("remove_term", { term: t.term }));
        li.append(del);
        return li;
      }),
    );
  }

  async function addTerm() {
    const term = termKeyIn.value.trim();
    const meaning = termValueIn.value.trim();
    if (!term || !meaning) return;
    await invoke("add_term", { term, meaning });
    termKeyIn.value = "";
    termValueIn.value = "";
    termKeyIn.focus();
  }
  view.querySelector(".term-add-btn")!.addEventListener("click", addTerm);
  termValueIn.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      addTerm();
    }
  });
  listen("lilith://glossary", () => loadTerms());

  const remindersEl = view.querySelector<HTMLUListElement>(".reminders-list")!;

  async function loadReminders() {
    const list = await invoke<Reminder[]>("list_reminders");
    if (list.length === 0) {
      const li = document.createElement("li");
      li.className = "facts-empty";
      li.textContent = "Nenhum lembrete pendente.";
      remindersEl.replaceChildren(li);
      return;
    }
    remindersEl.replaceChildren(
      ...list.map((r) => {
        const li = document.createElement("li");
        li.className = "fact";
        li.innerHTML = `<div class="fact-text"><strong></strong><span></span></div>`;
        const [y, m, d] = r.due.split("-");
        li.querySelector("strong")!.textContent = r.at === "00:00" ? `${d}/${m}/${y}` : `${d}/${m}/${y} ${r.at}`;
        li.querySelector(".fact-text span")!.textContent = r.text;
        const del = document.createElement("button");
        del.type = "button";
        del.className = "icon-btn neon-soft";
        del.title = "Apagar lembrete";
        del.append(icon("trash", 14));
        del.addEventListener("click", () => invoke("remove_reminder", { id: r.id }));
        li.append(del);
        return li;
      }),
    );
  }
  listen("lilith://reminders", () => loadReminders());

  // Voz: instalação (Piper + voz em português) e teste.
  const voiceStatus = view.querySelector<HTMLElement>(".voice-status")!;
  const voiceInstall = view.querySelector<HTMLButtonElement>(".voice-install")!;
  const voiceTest = view.querySelector<HTMLButtonElement>(".voice-test")!;

  async function loadVoice() {
    const st = await invoke<VoiceStatus>("voice_status");
    voiceInstall.hidden = st.installed;
    voiceInstall.disabled = st.installing;
    voiceTest.hidden = !st.installed;
    voiceStatus.textContent = st.installed
      ? "Voz instalada."
      : st.installing
        ? "Instalando a voz…"
        : "A voz ainda não está instalada. Ela é baixada uma vez (≈150 MB) e fica só no seu PC.";
  }
  voiceInstall.addEventListener("click", async () => {
    voiceInstall.disabled = true;
    voiceStatus.textContent = "Iniciando o download…";
    try {
      await invoke("voice_install");
    } catch (e) {
      voiceStatus.textContent = `Não consegui instalar a voz: ${String(e)}`;
    }
    await loadVoice();
  });
  voiceTest.addEventListener("click", () => void invoke("voice_test"));
  listen<{ stage: string; percent: number }>("lilith://voice-install", (e) => {
    const { stage, percent } = e.payload;
    voiceStatus.textContent = stage.startsWith("erro") || stage === "pronto" ? stage : `${stage}… ${percent}%`;
  });

  // Versão, início com o Windows e atualização.
  const updateStatus = view.querySelector<HTMLElement>(".update-status")!;
  const updateBtn = view.querySelector<HTMLButtonElement>(".update-btn")!;
  let version = "";
  const setStatus = (text: string) => (updateStatus.textContent = text);

  let pending = "";
  updateBtn.addEventListener("click", async () => {
    updateBtn.disabled = true;
    try {
      if (pending) {
        setStatus(`Baixando a ${pending}… a Lilith vai reiniciar.`);
        await invoke("check_update", { install: true });
        return;
      }
      setStatus("Procurando…");
      const found = await invoke<{ version: string } | null>("check_update", { install: false });
      if (found) {
        pending = found.version;
        updateBtn.textContent = `Atualizar para ${pending}`;
        setStatus(`Versão ${version}: a ${pending} está disponível.`);
      } else {
        setStatus(`Versão ${version}: você já está na última.`);
      }
    } catch {
      setStatus("Não consegui agora. Tente de novo mais tarde.");
    } finally {
      updateBtn.disabled = false;
    }
  });
  listen<{ version: string }>("lilith://update", (e) => setStatus(`Versão ${version}: a ${e.payload.version} está disponível.`));

  async function load() {
    const s = await invoke<Settings>("get_settings");
    field("user_name").value = s.user_name;
    (form.elements.namedItem("persona") as HTMLTextAreaElement).value = s.persona;
    field("chat_minutes").value = String(s.chat_minutes);
    field("auto_learn").checked = s.auto_learn;
    field("auto_update").checked = s.auto_update;
    field("voice").checked = s.voice;
    await loadVoice();
    const info = await invoke<AppInfo>("app_info");
    version = info.version;
    field("autostart").checked = info.autostart;
    setStatus(`Versão ${version}`);
    await loadReminders();
    prompts = await invoke<string[]>("get_quick_prompts");
    renderPrompts();
    await loadFacts();
    await loadTerms();
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
      auto_update: field("auto_update").checked,
      voice: field("voice").checked,
    };
    field("autostart").checked = await invoke<boolean>("set_autostart", { on: field("autostart").checked });
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
