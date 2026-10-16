import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Menu } from "@tauri-apps/api/menu";
import { cursorPosition, getCurrentWindow } from "@tauri-apps/api/window";
import { mountOrbListening } from "./voice_input";

type Mood = "neutral" | "happy" | "concerned" | "sleepy";

const DRAG_THRESHOLD = 4;
const DOUBLE_CLICK_MS = 260;
/// Olhar: quantas vezes por segundo lê a posição do mouse, e quanto os
/// olhos podem andar (px) dentro do rosto.
const GAZE_MS = 90;
const GAZE_RANGE = 4;
/// Sem interação por esse tempo, ela "cochila" (olhos meio fechados).
const SLEEPY_AFTER_MS = 4 * 60_000;

/// Forma 2D: fica pequena no canto e cresce quando o mouse chega perto.
/// Clique abre/fecha o chat, duplo clique invoca a forma 3D (fase 2),
/// arrastar move, botão direito abre o menu rápido. Os olhos seguem o mouse,
/// ela pisca sozinha, e a bolinha mostra o que está esperando você.
export function mountOrb(root: HTMLElement) {
  root.innerHTML = `
    <div class="orb-stage">
      <button class="orb" data-state="idle" data-mood="neutral" aria-label="Lilith — clique para conversar">
        <span class="orb-ring"></span>
        <span class="orb-horn left"></span><span class="orb-horn right"></span>
        <span class="orb-face">
          <span class="orb-eye"></span><span class="orb-eye"></span>
        </span>
        <span class="orb-badge" hidden></span>
      </button>
    </div>`;
  const stage = root.querySelector<HTMLElement>(".orb-stage")!;
  const orb = root.querySelector<HTMLButtonElement>(".orb")!;
  mountOrbListening(orb);
  const face = root.querySelector<HTMLElement>(".orb-face")!;
  const badge = root.querySelector<HTMLElement>(".orb-badge")!;
  const win = getCurrentWindow();

  let baseMood: Mood = "neutral";
  let lastActivity = Date.now();
  const touch = () => {
    lastActivity = Date.now();
    if (baseMood === "sleepy") setMood("neutral");
  };

  // ── clique, duplo clique e arrastar ─────────────────────────────────

  let clickTimer: number | undefined;
  let down: { x: number; y: number } | null = null;

  orb.addEventListener("mousedown", (e) => {
    touch();
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
    // Duplo clique: abre a forma 3D (o orb continua no canto).
    setMood("happy");
    // A orb se desfaz num brilho e, então, a Lilith 3D aparece.
    orb.classList.add("morph-out");
    window.setTimeout(() => {
      void invoke("toggle_avatar");
      window.setTimeout(() => orb.classList.remove("morph-out"), 400);
    }, 280);
  }

  // Voltou da forma 3D: a orb pula de volta.
  listen("lilith://orb-pop", () => {
    orb.classList.add("pop");
    window.setTimeout(() => orb.classList.remove("pop"), 700);
  });

  // ── menu rápido (botão direito) ─────────────────────────────────────

  orb.addEventListener("contextmenu", async (e) => {
    e.preventDefault();
    touch();
    const menu = await Menu.new({
      items: [
        { id: "new", text: "Nova conversa", action: () => void invoke("new_chat") },
        { id: "chat", text: "Abrir / fechar o chat", action: () => void invoke("toggle_chat") },
        { id: "panel", text: "Painel da Lilith", action: () => void invoke("show_panel_tab", { tab: "jobs" }) },
        { id: "3d", text: "Forma 3D", action: () => void invoke("toggle_avatar") },
        { item: "Separator" },
        { id: "stop", text: "Parar tudo (Ctrl+Alt+Esc)", action: () => void invoke("cancel") },
        { item: "Separator" },
        { id: "quit", text: "Sair da Lilith", action: () => void invoke("quit_app") },
      ],
    });
    await menu.popup();
  });

  // ── humor, piscar e cochilar ────────────────────────────────────────

  function setMood(mood: Mood) {
    baseMood = mood;
    orb.dataset.mood = mood;
  }

  /// Pisca em intervalos irregulares (às vezes duas vezes seguidas).
  function scheduleBlink() {
    window.setTimeout(() => {
      if (orb.dataset.mood !== "sleepy") {
        orb.classList.add("blinking");
        window.setTimeout(() => orb.classList.remove("blinking"), 140);
        if (Math.random() < 0.2) {
          window.setTimeout(() => {
            orb.classList.add("blinking");
            window.setTimeout(() => orb.classList.remove("blinking"), 140);
          }, 260);
        }
      }
      scheduleBlink();
    }, 2200 + Math.random() * 4200);
  }
  scheduleBlink();

  window.setInterval(() => {
    if (baseMood === "neutral" && Date.now() - lastActivity > SLEEPY_AFTER_MS && busy.size === 0) {
      setMood("sleepy");
    }
  }, 15_000);

  // ── olhar: os olhos seguem o mouse ──────────────────────────────────

  let center = { x: 0, y: 0 };
  const recenter = async () => {
    const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
    center = { x: pos.x + size.width / 2, y: pos.y + size.height / 2 };
  };
  void recenter();
  void win.onMoved(() => void recenter());

  window.setInterval(async () => {
    if (document.hidden || baseMood === "sleepy" || orb.dataset.state === "thinking") return;
    try {
      const p = await cursorPosition();
      const dx = p.x - center.x;
      const dy = p.y - center.y;
      const dist = Math.hypot(dx, dy);
      // Longe demais (mais de ~900 px) ela "desvia o olhar" para frente.
      const k = dist > 900 ? 0 : Math.min(1, dist / 220);
      const norm = dist || 1;
      face.style.setProperty("--gx", `${((dx / norm) * GAZE_RANGE * k).toFixed(2)}px`);
      face.style.setProperty("--gy", `${((dy / norm) * GAZE_RANGE * k * 0.8).toFixed(2)}px`);
    } catch {
      /* janela fechando */
    }
  }, GAZE_MS);

  // Cresce quando o mouse chega (a janela inteira conta, não só o círculo).
  stage.addEventListener("mouseenter", () => {
    stage.classList.add("hover");
    touch();
  });
  stage.addEventListener("mouseleave", () => stage.classList.remove("hover"));

  // ── estado: anel, bolinha de pendências ─────────────────────────────

  const busy = new Set<string>();
  const approvals = new Set<number>();
  const jobs = new Set<number>();

  function refresh() {
    const asking = approvals.size > 0;
    orb.dataset.state = asking ? "asking" : busy.size > 0 ? "thinking" : jobs.size > 0 ? "working" : "idle";
    const pending = approvals.size + jobs.size;
    badge.hidden = pending === 0;
    badge.textContent = pending > 9 ? "9+" : String(pending);
    badge.dataset.kind = asking ? "approval" : "job";
    orb.title = asking
      ? `${approvals.size} pedido(s) esperando sua aprovação`
      : jobs.size > 0
        ? `${jobs.size} tarefa(s) rodando`
        : "";
  }

  listen<{ conversation: string; state: string }>("lilith://state", (e) => {
    if (e.payload.state === "thinking") busy.add(e.payload.conversation);
    else busy.delete(e.payload.conversation);
    touch();
    refresh();
  });
  listen<{ emotion: Mood }>("lilith://reply", (e) => {
    setMood(e.payload.emotion);
    window.setTimeout(() => setMood("neutral"), 6000);
  });
  listen<{ id: number }>("lilith://approval", (e) => {
    approvals.add(e.payload.id);
    touch();
    refresh();
  });
  // Resposta, expiração ou kill switch: o pedido deixou de esperar.
  listen<{ id: number }>("lilith://approval-resolved", (e) => {
    approvals.delete(e.payload.id);
    refresh();
  });
  listen<number>("lilith://approval-expired", (e) => {
    approvals.delete(e.payload);
    refresh();
  });
  listen("lilith://cancelled", () => {
    approvals.clear();
    refresh();
  });
  listen<{ kind: string; job?: { id: number; status: string } }>("lilith://job", (e) => {
    const job = e.payload.job;
    if (e.payload.kind !== "updated" || !job) return;
    if (job.status === "running") jobs.add(job.id);
    else jobs.delete(job.id);
    touch();
    refresh();
  });
}
