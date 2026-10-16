import {
  Activity,
  AppWindow,
  Bot,
  Check,
  Clipboard,
  Download,
  FileText,
  FolderOpen,
  Globe,
  History,
  KeyRound,
  type IconNode,
  ListChecks,
  MousePointer2,
  Pencil,
  Plus,
  Power,
  RotateCcw,
  SendHorizontal,
  Settings,
  ShieldCheck,
  Smartphone,
  Sparkles,
  Square,
  Trash2,
  TriangleAlert,
  Volume2,
  Wallpaper,
  X,
  createElement,
} from "lucide";

/// Ícones de traço fino (Lucide), no estilo do tema de vidro.
const ICONS = {
  activity: Activity,
  appWindow: AppWindow,
  bot: Bot,
  check: Check,
  clipboard: Clipboard,
  close: X,
  download: Download,
  file: FileText,
  folder: FolderOpen,
  globe: Globe,
  history: History,
  pencil: Pencil,
  jobs: ListChecks,
  key: KeyRound,
  plus: Plus,
  trash: Trash2,
  pointer: MousePointer2,
  power: Power,
  reset: RotateCcw,
  send: SendHorizontal,
  settings: Settings,
  shield: ShieldCheck,
  phone: Smartphone,
  sparkles: Sparkles,
  stop: Square,
  volume: Volume2,
  wallpaper: Wallpaper,
  warn: TriangleAlert,
} satisfies Record<string, IconNode>;

export type IconName = keyof typeof ICONS;

export function icon(name: IconName, size = 18): SVGElement {
  const el = createElement(ICONS[name], {
    width: size,
    height: size,
    "stroke-width": 1.75,
    "aria-hidden": "true",
  });
  el.classList.add("icon");
  return el;
}

/// Troca `<span data-icon="nome" data-size="16">` pelo SVG (templates estáticos).
export function hydrateIcons(root: ParentNode) {
  root.querySelectorAll<HTMLElement>("[data-icon]").forEach((el) => {
    el.replaceChildren(icon(el.dataset.icon as IconName, Number(el.dataset.size ?? 18)));
  });
}

/// Ícone de cada família de ação, para as linhas de passo do chat e o painel.
export function actionIcon(action: string): IconName {
  const family = action.split(".")[0];
  switch (family) {
    case "window":
    case "app":
      return "appWindow";
    case "file":
      return action === "file.list" || action === "file.find" ? "folder" : "file";
    case "download":
      return "download";
    case "web":
    case "browser":
      return "globe";
    case "audio":
      return "volume";
    case "desktop":
      return "wallpaper";
    case "clipboard":
      return "clipboard";
    case "system":
      return "power";
    case "agent":
      return "bot";
    case "phone":
      return "phone";
    case "settings":
      return "settings";
    case "cursor":
    case "ui":
      return "pointer";
    default:
      return "sparkles";
  }
}
