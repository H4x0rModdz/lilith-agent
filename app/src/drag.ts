import { Effect, getCurrentWindow } from "@tauri-apps/api/window";

/// Desfoque limpo (parado) e acrylic do sistema (enquanto arrasta). O "blur"
/// clássico do Windows pisca/some quando a janela se move no Win11; o acrylic
/// do sistema é estável — então troca só durante o arraste.
const STILL = { effects: [Effect.Blur], color: [255, 255, 255, 12] as [number, number, number, number] };
const MOVING = { effects: [Effect.Acrylic] };
/// Sem evento de "soltou" no arraste nativo: volta ao blur quando a janela
/// para de se mover por esse tempo.
const SETTLE_MS = 220;

/// Arrasta a janela pelo `handle` (ignora cliques em botões e campos).
export function makeDraggable(handle: HTMLElement, onDragStart?: () => void) {
  const win = getCurrentWindow();
  const glassy = win.label === "chat" || win.label === "panel";
  let settle: number | undefined;
  let dragging = false;

  if (glassy) {
    win.onMoved(() => {
      if (!dragging) return;
      window.clearTimeout(settle);
      settle = window.setTimeout(() => {
        dragging = false;
        win.setEffects(STILL);
      }, SETTLE_MS);
    });
  }

  handle.addEventListener("mousedown", async (e) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button, input")) return;
    e.preventDefault();
    onDragStart?.();
    if (glassy && !dragging) {
      dragging = true;
      await win.setEffects(MOVING);
      // Clique sem arrastar: nenhum onMoved chega — volta mesmo assim.
      settle = window.setTimeout(() => {
        dragging = false;
        win.setEffects(STILL);
      }, 1500);
    }
    win.startDragging();
  });
}
