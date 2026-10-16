import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Mic, createElement } from "lucide";

/// Voz de entrada (push-to-talk): botão do microfone no chat, cartão de
/// instalação, "ouvindo…" na orb e a seção das Configurações. Quem decide
/// (gravar, cortar o silêncio, reconhecer) é o backend (`voice_in.rs`); aqui só
/// se mostra o estado e o texto ouvido vira mensagem do usuário. Estilos em
/// `voice_input.css` (carregado pelo index.html, depois do styles.css).

export interface VoiceInStatus {
  installed: boolean;
  installing: boolean;
  enabled: boolean;
  hotkey: string;
  hotkey_active: boolean;
  listening: boolean;
  processing: boolean;
}

type ListenState = "start" | "processing" | "stop" | "cancel" | "missing" | "error";
interface Listening {
  state: ListenState;
  message?: string;
}

const MODEL_SIZE = "≈375 MB";

const KEY_NAMES: Record<string, string> = {
  control: "Ctrl",
  ctrl: "Ctrl",
  alt: "Alt",
  shift: "Shift",
  super: "Win",
  space: "Espaço",
  escape: "Esc",
  enter: "Enter",
  backquote: "`",
};

/// "control+alt+Space" → "Ctrl+Alt+Espaço".
export function hotkeyLabel(hotkey: string): string {
  return hotkey
    .split("+")
    .map((part) => {
      const p = part.trim();
      const known = KEY_NAMES[p.toLowerCase()];
      if (known) return known;
      if (/^Key[A-Z]$/.test(p)) return p.slice(3);
      if (/^Digit\d$/.test(p)) return p.slice(5);
      return p;
    })
    .join("+");
}

/// Atalho no formato do backend a partir de uma tecla apertada (`null` = só
/// modificador, ainda esperando a tecla principal).
export function hotkeyFromEvent(e: KeyboardEvent): string | null {
  if (["Control", "Alt", "Shift", "Meta", "AltGraph"].includes(e.key)) return null;
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean);
  return [...mods, e.code].join("+");
}

function micIcon(size = 18): SVGElement {
  const svg = createElement(Mic) as SVGElement;
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("aria-hidden", "true");
  return svg;
}

const status = () => invoke<VoiceInStatus>("voice_in_status");

// ── chat ─────────────────────────────────────────────────────────────────

export interface ChatHooks {
  form: HTMLFormElement;
  input: HTMLTextAreaElement;
  /// Antes de qual botão o microfone entra.
  before: HTMLElement;
  /// Texto "online / pensando…" do cabeçalho.
  statusEl: HTMLElement;
  /// Envia o texto ouvido como mensagem do usuário (mesmo caminho do digitado).
  send: (text: string) => void;
  /// Linha de sistema no chat.
  note: (text: string) => void;
  /// Adiciona um elemento (cartão) na conversa ativa.
  card: (li: HTMLElement) => void;
}

/// Botão do microfone no campo de mensagem. Segurar = falar e soltar para
/// enviar; clique rápido = ouvir até a pausa. Esc cancela.
export async function mountMic(h: ChatHooks) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "mic icon-btn neon-soft";
  btn.append(micIcon(18));
  h.form.insertBefore(btn, h.before);

  const placeholder = h.input.placeholder;
  let st: VoiceInStatus | null = null;
  let active: ListenState = "stop";
  let savedStatus = "";
  let installCard: HTMLElement | null = null;

  function applyStatus(s: VoiceInStatus) {
    st = s;
    btn.hidden = !s.enabled;
    const key = hotkeyLabel(s.hotkey);
    btn.title = s.hotkey_active
      ? `Falar com a Lilith — segure (ou toque) aqui ou ${key}`
      : "Falar com a Lilith — segure ou toque aqui";
    btn.setAttribute("aria-label", "Falar");
  }
  status().then(applyStatus, () => applyStatus({ installed: false, installing: false, enabled: true, hotkey: "", hotkey_active: false, listening: false, processing: false }));
  listen<VoiceInStatus>("lilith://voice-in-config", (e) => applyStatus(e.payload));

  function setState(state: ListenState) {
    const was = active;
    active = state;
    const on = state === "start" || state === "processing";
    btn.classList.toggle("listening", state === "start");
    btn.classList.toggle("processing", state === "processing");
    h.form.classList.toggle("listening", on);
    if (state === "start") {
      h.input.placeholder = "Ouvindo… solte (ou toque de novo) para enviar · Esc cancela";
      if (was !== "processing") savedStatus = h.statusEl.textContent ?? "";
      h.statusEl.textContent = "ouvindo…";
    } else if (state === "processing") {
      h.input.placeholder = "Entendendo o que você disse…";
      h.statusEl.textContent = "entendendo…";
    } else {
      h.input.placeholder = placeholder;
      btn.style.removeProperty("--level");
      if (was === "start" || was === "processing") h.statusEl.textContent = savedStatus || "online";
    }
  }

  listen<Listening>("lilith://listening", (e) => {
    const { state, message } = e.payload;
    if (state === "missing") {
      setState("stop");
      showInstallCard();
      return;
    }
    setState(state);
    if (message) h.note(message);
  });
  listen<number>("lilith://mic-level", (e) => {
    if (active === "start") btn.style.setProperty("--level", String(Math.max(0, Math.min(1, Number(e.payload) || 0))));
  });
  listen<{ text: string }>("lilith://heard", (e) => {
    const text = (e.payload.text ?? "").trim();
    if (text) h.send(text);
  });

  // Segurar e soltar (mouse, caneta, toque). O teclado (Enter/Espaço no botão)
  // vira um toque: começa a ouvir e envia na pausa.
  let pressed = false;
  btn.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    pressed = true;
    btn.setPointerCapture(e.pointerId);
    void invoke("voice_in_press");
  });
  const up = () => {
    if (!pressed) return;
    pressed = false;
    void invoke("voice_in_release");
  };
  btn.addEventListener("pointerup", up);
  btn.addEventListener("pointercancel", up);
  btn.addEventListener("click", (e) => {
    if (e.detail !== 0) return; // clique do mouse já tratado acima
    void invoke("voice_in_press").then(() => invoke("voice_in_release"));
  });

  // Esc no chat enquanto ouve: cancela (antes de o chat se esconder).
  window.addEventListener(
    "keydown",
    (e) => {
      if (e.key !== "Escape" || (active !== "start" && active !== "processing")) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      void invoke("voice_in_cancel");
    },
    true,
  );

  function showInstallCard() {
    if (installCard?.isConnected) {
      installCard.scrollIntoView({ block: "nearest" });
      return;
    }
    const li = document.createElement("li");
    li.className = "msg choice voice-in-card";
    li.innerHTML = `
      <p class="choice-title"><span class="voice-in-icon"></span><span class="choice-text"></span></p>
      <div class="approval-buttons">
        <button type="button" class="primary" data-act="install"></button>
        <button type="button" data-act="later">Agora não</button>
      </div>
      <p class="choice-hint"></p>`;
    li.querySelector(".voice-in-icon")!.append(micIcon(16));
    const title = li.querySelector<HTMLElement>(".choice-text")!;
    const hint = li.querySelector<HTMLElement>(".choice-hint")!;
    const buttons = li.querySelector<HTMLElement>(".approval-buttons")!;
    const installBtn = li.querySelector<HTMLButtonElement>('[data-act="install"]')!;
    title.textContent = "Para eu te ouvir, preciso baixar o reconhecimento de voz.";
    installBtn.textContent = `Baixar agora (${MODEL_SIZE})`;
    hint.textContent = "Baixa uma vez só e roda no seu PC — sua voz não sai daqui.";
    li.querySelector('[data-act="later"]')!.addEventListener("click", () => {
      buttons.remove();
      hint.textContent = "Tudo bem. Dá para instalar depois em Configurações → Ouvir sua voz.";
    });
    installBtn.addEventListener("click", async () => {
      buttons.remove();
      hint.textContent = "Começando o download…";
      const off = await listen<{ stage: string; percent: number }>("lilith://voice-in-install", (e) => {
        const { stage, percent } = e.payload;
        if (stage === "pronto" || stage.startsWith("erro")) return;
        hint.textContent = percent > 0 && percent < 100 ? `${stage}… ${percent}%` : `${stage}…`;
      });
      try {
        await invoke("voice_in_install");
        const key = st?.hotkey_active ? hotkeyLabel(st.hotkey) : "";
        title.textContent = "Pronto! Já consigo te ouvir.";
        hint.textContent = key
          ? `Segure ${key} (ou o microfone aqui embaixo), fale e solte.`
          : "Segure o microfone aqui embaixo, fale e solte.";
      } catch (e) {
        title.textContent = "Não consegui instalar o reconhecimento de voz.";
        hint.textContent = String(e);
      } finally {
        off();
      }
    });
    installCard = li;
    h.card(li);
  }
}

// ── orb ──────────────────────────────────────────────────────────────────

/// A orb mostra "ouvindo" (anel rosa pulsando com o volume) e "entendendo".
export function mountOrbListening(orb: HTMLElement) {
  listen<Listening>("lilith://listening", (e) => {
    orb.classList.toggle("listening", e.payload.state === "start");
    orb.classList.toggle("hearing", e.payload.state === "processing");
    if (e.payload.state !== "start") orb.style.removeProperty("--level");
  });
  listen<number>("lilith://mic-level", (e) => {
    if (orb.classList.contains("listening")) orb.style.setProperty("--level", String(Number(e.payload) || 0));
  });
}

// ── configurações ────────────────────────────────────────────────────────

/// Seção "Ouvir sua voz": liga/desliga, atalho (gravado apertando as teclas) e
/// instalação do modelo. Salva na hora (o atalho só vale se registrar).
export function mountVoiceInputSettings(): { el: HTMLElement; load: () => Promise<void> } {
  const el = document.createElement("section");
  el.className = "settings-card voice-in-settings";
  el.innerHTML = `
    <h3><span class="voice-in-icon"></span>Ouvir sua voz</h3>
    <small class="hint">Segure o atalho (ou o microfone do chat), fale e solte para enviar. Um toque rápido também vale: ela envia quando você faz uma pausa. Esc cancela. O microfone só abre enquanto você usa o atalho, e nada sai do seu PC.</small>
    <label class="switch">
      <input class="vi-enabled" type="checkbox" />
      <span class="switch-track"><span class="switch-thumb"></span></span>
      <span>Ouvir pelo atalho e pelo microfone do chat</span>
    </label>
    <div class="update-row vi-hotkey-row">
      <small class="hint">Atalho</small>
      <span class="voice-actions">
        <button type="button" class="neon-soft vi-hotkey" title="Clique e aperte o atalho novo"></button>
        <button type="button" class="neon-soft vi-default">Padrão</button>
      </span>
    </div>
    <div class="update-row">
      <small class="hint vi-status"></small>
      <span class="voice-actions">
        <button type="button" class="neon-soft vi-install" hidden>Instalar (${MODEL_SIZE})</button>
      </span>
    </div>`;
  el.querySelector(".voice-in-icon")!.append(micIcon(16));
  const enabled = el.querySelector<HTMLInputElement>(".vi-enabled")!;
  const hotkeyBtn = el.querySelector<HTMLButtonElement>(".vi-hotkey")!;
  const defaultBtn = el.querySelector<HTMLButtonElement>(".vi-default")!;
  const statusEl = el.querySelector<HTMLElement>(".vi-status")!;
  const installBtn = el.querySelector<HTMLButtonElement>(".vi-install")!;
  let current: VoiceInStatus | null = null;
  let capturing = false;
  let message = "";

  function render(s: VoiceInStatus) {
    current = s;
    enabled.checked = s.enabled;
    if (!capturing) hotkeyBtn.textContent = hotkeyLabel(s.hotkey);
    hotkeyBtn.disabled = !s.enabled;
    defaultBtn.disabled = !s.enabled;
    installBtn.hidden = s.installed;
    installBtn.disabled = s.installing;
    statusEl.textContent =
      message ||
      (s.installed
        ? s.enabled && !s.hotkey_active
          ? "Instalado. O atalho não registrou (outro programa usa?) — escolha outro."
          : "Reconhecimento de voz instalado."
        : s.installing
          ? "Instalando…"
          : `Ainda não instalado. É baixado uma vez (${MODEL_SIZE}) e roda só no seu PC.`);
    message = "";
  }

  async function configure(on: boolean, hotkey: string) {
    try {
      render(await invoke<VoiceInStatus>("voice_in_configure", { enabled: on, hotkey }));
    } catch (e) {
      message = String(e);
      render(current ?? (await status()));
    }
  }

  enabled.addEventListener("change", () => void configure(enabled.checked, current?.hotkey ?? ""));
  defaultBtn.addEventListener("click", () => void configure(true, "Ctrl+Alt+Space"));

  function stopCapture() {
    capturing = false;
    hotkeyBtn.classList.remove("capturing");
    if (current) hotkeyBtn.textContent = hotkeyLabel(current.hotkey);
  }
  hotkeyBtn.addEventListener("click", () => {
    capturing = true;
    hotkeyBtn.classList.add("capturing");
    hotkeyBtn.textContent = "Aperte o atalho…";
    hotkeyBtn.focus();
  });
  hotkeyBtn.addEventListener("blur", stopCapture);
  hotkeyBtn.addEventListener("keydown", (e) => {
    if (!capturing) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      stopCapture();
      return;
    }
    const combo = hotkeyFromEvent(e);
    if (!combo) {
      hotkeyBtn.textContent = `${hotkeyLabel([e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean).join("+"))}+…`;
      return;
    }
    capturing = false;
    hotkeyBtn.classList.remove("capturing");
    void configure(true, combo);
  });

  installBtn.addEventListener("click", async () => {
    installBtn.disabled = true;
    statusEl.textContent = "Começando o download…";
    try {
      await invoke("voice_in_install");
    } catch (e) {
      message = `Não consegui instalar: ${String(e)}`;
    }
    render(await status());
  });
  listen<{ stage: string; percent: number }>("lilith://voice-in-install", (e) => {
    const { stage, percent } = e.payload;
    if (stage === "pronto" || stage.startsWith("erro")) return;
    statusEl.textContent = percent > 0 && percent < 100 ? `${stage}… ${percent}%` : `${stage}…`;
  });
  listen<VoiceInStatus>("lilith://voice-in-config", (e) => render(e.payload));

  return { el, load: async () => render(await status()) };
}
