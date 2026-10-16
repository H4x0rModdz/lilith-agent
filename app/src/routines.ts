import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { type IconName, hydrateIcons, icon } from "./icons";

interface ModeStep {
  say: string;
  label: string;
  action: string;
}

interface Mode {
  name: string;
  steps: ModeStep[];
}

interface Layout {
  name: string;
  count: number;
  apps: string[];
}

interface StepPreview {
  line: string;
  ok: boolean;
  label?: string;
}

interface AlertSettings {
  downloads: boolean;
  disk: boolean;
  battery: boolean;
  cpu: boolean;
  routines: boolean;
}

const ALERTS: [keyof AlertSettings, string][] = [
  ["downloads", "Avisar quando um download terminar"],
  ["disk", "Avisar quando um disco estiver quase cheio"],
  ["battery", "Avisar quando a bateria estiver baixa"],
  ["cpu", "Avisar quando a CPU ficar alta por mais de 2 minutos"],
  ["routines", "Aprender minha rotina e sugerir modos (guarda só o nome dos programas que abrem)"],
];

function iconButton(name: IconName, title: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "icon-btn neon-soft";
  b.title = title;
  b.append(icon(name, 14));
  b.addEventListener("click", onClick);
  return b;
}

/// Lixeira com confirmação no próprio botão: o primeiro clique arma, o
/// segundo (em até 3 s) apaga.
function deleteButton(title: string, onConfirm: () => void): HTMLButtonElement {
  let armed = 0;
  const b = iconButton("trash", title, () => {
    if (armed) {
      window.clearTimeout(armed);
      onConfirm();
      return;
    }
    b.classList.add("confirm-delete");
    b.title = "Clique de novo para apagar";
    armed = window.setTimeout(() => {
      armed = 0;
      b.classList.remove("confirm-delete");
      b.title = title;
    }, 3000);
  });
  return b;
}

function empty(text: string): HTMLLIElement {
  const li = document.createElement("li");
  li.className = "facts-empty";
  li.textContent = text;
  return li;
}

/// Seções de rotinas nas Configurações: modos (com editor), layouts de
/// janelas e os avisos proativos. Acrescentadas ao fim do formulário.
export function mountRoutines(container: HTMLElement) {
  const section = (html: string) => {
    const s = document.createElement("section");
    s.className = "settings-card";
    s.innerHTML = html;
    container.append(s);
    return s;
  };

  // ── modos ──────────────────────────────────────────────────────────────
  const modesCard = section(`
    <h3><span data-icon="sparkles" data-size="16"></span>Modos</h3>
    <small class="hint">Rotinas com várias ações. No chat, diga “modo foco” para rodar, ou crie assim: “cria o modo foco: minimiza o monitor 1, muta e abre o vs code”. Cada passo passa pelas suas permissões.</small>
    <ul class="facts modes-list"></ul>
    <div class="routine-editor" hidden>
      <label class="field"><span>Nome</span><input class="mode-name" maxlength="40" placeholder="ex.: foco" /></label>
      <label class="field"><span>Passos (um por linha, do jeito que você falaria)</span>
        <textarea class="mode-steps" rows="4" placeholder="minimiza o monitor 1&#10;muta o pc&#10;abre o vs code"></textarea>
      </label>
      <ul class="routine-preview"></ul>
      <small class="routine-status"></small>
      <div class="settings-actions">
        <button type="button" class="neon-soft mode-cancel">Cancelar</button>
        <button type="button" class="neon mode-save">Salvar modo</button>
      </div>
    </div>
    <div class="settings-actions">
      <button type="button" class="neon-soft mode-new"><span data-icon="plus" data-size="14"></span> Novo modo</button>
    </div>`);
  const modesEl = modesCard.querySelector<HTMLUListElement>(".modes-list")!;
  const editor = modesCard.querySelector<HTMLElement>(".routine-editor")!;
  const nameIn = modesCard.querySelector<HTMLInputElement>(".mode-name")!;
  const stepsIn = modesCard.querySelector<HTMLTextAreaElement>(".mode-steps")!;
  const previewEl = modesCard.querySelector<HTMLUListElement>(".routine-preview")!;
  const statusEl = modesCard.querySelector<HTMLElement>(".routine-status")!;
  const newBtn = modesCard.querySelector<HTMLButtonElement>(".mode-new")!;
  let editing: string | null = null;

  const setStatus = (text: string, error = false) => {
    statusEl.textContent = text;
    if (error) statusEl.dataset.error = "1";
    else delete statusEl.dataset.error;
  };
  const lines = () => stepsIn.value.split("\n").map((l) => l.trim()).filter(Boolean);

  function openEditor(mode?: Mode) {
    editing = mode?.name ?? null;
    nameIn.value = mode?.name ?? "";
    stepsIn.value = mode ? mode.steps.map((s) => s.say || s.label).join("\n") : "";
    editor.hidden = false;
    newBtn.hidden = true;
    setStatus("");
    void refreshPreview();
    nameIn.focus();
  }

  function closeEditor() {
    editor.hidden = true;
    newBtn.hidden = false;
    editing = null;
    previewEl.replaceChildren();
  }

  // O código mostra o que entendeu de cada linha antes de salvar.
  let previewTimer = 0;
  async function refreshPreview() {
    const ls = lines();
    if (ls.length === 0) {
      previewEl.replaceChildren();
      return;
    }
    const preview = await invoke<StepPreview[]>("preview_mode", { lines: ls, name: editing }).catch(() => []);
    previewEl.replaceChildren(
      ...preview.map((p, i) => {
        const li = document.createElement("li");
        li.dataset.ok = String(p.ok);
        li.textContent = p.ok ? `${i + 1}. ${p.label}` : `${i + 1}. não entendi: “${p.line}”`;
        return li;
      }),
    );
  }
  stepsIn.addEventListener("input", () => {
    window.clearTimeout(previewTimer);
    previewTimer = window.setTimeout(() => void refreshPreview(), 300);
  });
  nameIn.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      stepsIn.focus();
    }
  });

  modesCard.querySelector(".mode-cancel")!.addEventListener("click", closeEditor);
  newBtn.addEventListener("click", () => openEditor());
  modesCard.querySelector(".mode-save")!.addEventListener("click", async () => {
    try {
      await invoke<string>("save_mode_lines", { name: nameIn.value, lines: lines(), previous: editing });
      closeEditor();
      setStatus("");
      await loadModes();
    } catch (e) {
      setStatus(String(e), true);
    }
  });

  async function loadModes() {
    const modes = await invoke<Mode[]>("list_modes").catch(() => []);
    if (modes.length === 0) {
      modesEl.replaceChildren(empty("Nenhum modo ainda."));
      return;
    }
    modesEl.replaceChildren(
      ...modes.map((m) => {
        const li = document.createElement("li");
        li.className = "fact";
        li.innerHTML = `<div class="fact-text"><strong></strong><span class="routine-steps"></span></div><span class="routine-actions"></span>`;
        li.querySelector("strong")!.textContent = m.name;
        li.querySelector(".routine-steps")!.textContent = m.steps.map((s) => s.label).join(" → ");
        const actions = li.querySelector<HTMLElement>(".routine-actions")!;
        const run = iconButton("sparkles", `Rodar o modo ${m.name}`, async () => {
          run.disabled = true;
          try {
            await invoke<string>("run_mode", { name: m.name });
          } catch {
            // O resultado (com o erro) já aparece no chat.
          } finally {
            run.disabled = false;
          }
        });
        actions.append(
          run,
          iconButton("pencil", "Editar", () => openEditor(m)),
          deleteButton("Apagar o modo", () => void invoke("remove_mode", { name: m.name })),
        );
        return li;
      }),
    );
  }

  // ── layouts de janelas ─────────────────────────────────────────────────
  const layoutsCard = section(`
    <h3><span data-icon="wallpaper" data-size="16"></span>Layouts de janelas</h3>
    <small class="hint">Arrume as janelas e salve o arranjo (no chat: “salva esse layout como trabalho”). Restaurar abre os apps que faltam e põe cada janela no lugar (“restaura o layout trabalho”).</small>
    <ul class="facts layouts-list"></ul>
    <div class="fact-add">
      <input class="layout-name" maxlength="40" placeholder="nome do arranjo de agora (ex.: trabalho)" />
      <button type="button" class="icon-btn neon-soft layout-save" title="Salvar as janelas como estão agora"><span data-icon="plus" data-size="15"></span></button>
    </div>
    <small class="routine-status layout-status"></small>`);
  const layoutsEl = layoutsCard.querySelector<HTMLUListElement>(".layouts-list")!;
  const layoutIn = layoutsCard.querySelector<HTMLInputElement>(".layout-name")!;
  const layoutStatus = layoutsCard.querySelector<HTMLElement>(".layout-status")!;

  async function saveLayout() {
    const name = layoutIn.value.trim();
    if (!name) return;
    layoutStatus.textContent = "Salvando…";
    try {
      layoutStatus.textContent = await invoke<string>("save_layout_now", { name });
      layoutIn.value = "";
    } catch (e) {
      layoutStatus.textContent = String(e);
    }
  }
  layoutsCard.querySelector(".layout-save")!.addEventListener("click", () => void saveLayout());
  layoutIn.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void saveLayout();
    }
  });

  async function loadLayouts() {
    const layouts = await invoke<Layout[]>("list_layouts").catch(() => []);
    if (layouts.length === 0) {
      layoutsEl.replaceChildren(empty("Nenhum layout salvo."));
      return;
    }
    layoutsEl.replaceChildren(
      ...layouts.map((l) => {
        const li = document.createElement("li");
        li.className = "fact";
        li.innerHTML = `<div class="fact-text"><strong></strong><span></span></div><span class="routine-actions"></span>`;
        li.querySelector("strong")!.textContent = l.name;
        li.querySelector(".fact-text span")!.textContent = `${l.count} janela${l.count === 1 ? "" : "s"}: ${l.apps.join(", ")}`;
        const restore = iconButton("history", `Restaurar o layout ${l.name}`, async () => {
          restore.disabled = true;
          layoutStatus.textContent = "Restaurando…";
          try {
            layoutStatus.textContent = await invoke<string>("restore_layout", { name: l.name });
          } catch (e) {
            layoutStatus.textContent = String(e);
          } finally {
            restore.disabled = false;
          }
        });
        li.querySelector(".routine-actions")!.append(
          restore,
          deleteButton("Apagar o layout", () => void invoke("remove_layout", { name: l.name })),
        );
        return li;
      }),
    );
  }

  // ── avisos proativos ───────────────────────────────────────────────────
  const alertsCard = section(`
    <h3><span data-icon="warn" data-size="16"></span>Avisos</h3>
    <small class="hint">Ela avisa sozinha (notificação do Windows e no chat), sem repetir o mesmo aviso.</small>
    <div class="alert-switches"></div>`);
  const switches = alertsCard.querySelector<HTMLElement>(".alert-switches")!;
  const boxes = new Map<keyof AlertSettings, HTMLInputElement>();
  for (const [key, label] of ALERTS) {
    const l = document.createElement("label");
    l.className = "switch";
    l.innerHTML = `<input type="checkbox" /><span class="switch-track"><span class="switch-thumb"></span></span><span></span>`;
    l.querySelector("span:last-child")!.textContent = label;
    const box = l.querySelector("input")!;
    box.addEventListener("change", () => void saveAlerts());
    boxes.set(key, box);
    switches.append(l);
  }
  switches.style.display = "flex";
  switches.style.flexDirection = "column";
  switches.style.gap = "10px";

  async function saveAlerts() {
    const settings = Object.fromEntries(ALERTS.map(([k]) => [k, boxes.get(k)!.checked])) as unknown as AlertSettings;
    await invoke("save_alert_settings", { settings });
  }

  async function loadAlerts() {
    const s = await invoke<AlertSettings>("get_alert_settings").catch(() => null);
    if (!s) return;
    for (const [k] of ALERTS) boxes.get(k)!.checked = s[k];
  }

  hydrateIcons(modesCard);
  hydrateIcons(layoutsCard);
  hydrateIcons(alertsCard);
  listen("lilith://routines", () => {
    void loadModes();
    void loadLayouts();
  });

  async function load() {
    await Promise.all([loadModes(), loadLayouts(), loadAlerts()]);
  }
  return { load };
}
