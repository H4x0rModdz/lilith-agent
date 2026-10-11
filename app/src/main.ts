import "@fontsource-variable/inter";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { mountOrb } from "./orb";
import { mountChat } from "./chat";
import { mountPanel } from "./panel";
import { mountBorder, mountCursor } from "./presence";

// Uma página só para todas as janelas; cada uma monta a sua UI pelo label.
const root = document.getElementById("root")!;
const label = getCurrentWindow().label;
document.body.dataset.window = label;

if (label === "orb") mountOrb(root);
else if (label === "chat") mountChat(root);
else if (label === "panel") mountPanel(root);
else if (label === "cursor") mountCursor(root);
// Carregado só nesta janela: three.js pesa e as outras não precisam dele.
else if (label === "avatar") void import("./avatar3d").then((m) => m.mountAvatar(root));
else if (label.startsWith("border-")) mountBorder(root);
