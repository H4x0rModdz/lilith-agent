import { getCurrentWindow } from "@tauri-apps/api/window";

/// Arrasta a janela pelo `handle` (ignora cliques em botões dentro dele).
export function makeDraggable(handle: HTMLElement, onDragStart?: () => void) {
  handle.addEventListener("mousedown", (e) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    onDragStart?.();
    getCurrentWindow().startDragging();
  });
}
