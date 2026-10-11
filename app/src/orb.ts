import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type Mood = "neutral" | "happy" | "concerned";

const DRAG_THRESHOLD = 4;
const DOUBLE_CLICK_MS = 260;

/// Forma 2D: a orb no canto. Clique abre/fecha o chat, duplo clique
/// invoca a forma 3D (fase 2), arrastar move a orb.
export function mountOrb(root: HTMLElement) {
  root.innerHTML = `
    <button class="orb" data-state="idle" data-mood="neutral" aria-label="Lilith — clique para conversar">
      <span class="orb-horn left"></span><span class="orb-horn right"></span>
      <span class="orb-face">
        <span class="orb-eye"></span><span class="orb-eye"></span>
      </span>
      <span class="orb-bubble" hidden></span>
    </button>`;
  const orb = root.querySelector<HTMLButtonElement>(".orb")!;
  const bubble = root.querySelector<HTMLElement>(".orb-bubble")!;
  const win = getCurrentWindow();

  let clickTimer: number | undefined;
  let down: { x: number; y: number } | null = null;

  orb.addEventListener("mousedown", (e) => {
    if (e.button === 0) down = { x: e.screenX, y: e.screenY };
  });
  orb.addEventListener("mousemove", (e) => {
    if (!down) return;
    if (Math.hypot(e.screenX - down.x, e.screenY - down.y) > DRAG_THRESHOLD) {
      down = null;
      win.startDragging();
    }
  });
  orb.addEventListener("mouseup", () => {
    if (!down) return;
    down = null;
    // Distingue clique de duplo clique sem abrir o chat no primeiro toque.
    if (clickTimer) {
      clearTimeout(clickTimer);
      clickTimer = undefined;
      summon3D();
    } else {
      clickTimer = window.setTimeout(() => {
        clickTimer = undefined;
        invoke("toggle_chat");
      }, DOUBLE_CLICK_MS);
    }
  });

  function summon3D() {
    // Fase 2: abre a janela do avatar VRM. Por enquanto, só um aceno.
    setMood("happy");
    flash("forma 3D em breve ✦");
  }

  function setMood(mood: Mood) {
    orb.dataset.mood = mood;
  }

  function flash(text: string) {
    bubble.textContent = text;
    bubble.hidden = false;
    window.setTimeout(() => (bubble.hidden = true), 2200);
  }

  listen<string>("lilith://state", (e) => (orb.dataset.state = e.payload));
  listen<{ emotion: Mood }>("lilith://reply", (e) => {
    setMood(e.payload.emotion);
    window.setTimeout(() => setMood("neutral"), 6000);
  });
  listen("lilith://approval", () => {
    orb.dataset.state = "asking";
  });
}
