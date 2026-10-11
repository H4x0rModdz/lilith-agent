import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { VRMLoaderPlugin, VRMUtils, type VRM } from "@pixiv/three-vrm";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Menu } from "@tauri-apps/api/menu";
import { cursorPosition, getCurrentWindow } from "@tauri-apps/api/window";

/// Forma 3D da Lilith: o modelo VRM (`%APPDATA%\Lilith\lilith.vrm`) da cintura
/// para cima. O modelo não vai no repositório nem no instalador (a licença
/// dele proíbe redistribuir): cada pessoa coloca o seu arquivo lá.

const FPS = 30;
const DRAG_THRESHOLD = 4;
const DOUBLE_CLICK_MS = 280;

/// Expressões VRM (nomes normalizados do three-vrm) e o quanto cada emoção
/// da Lilith liga delas.
type Face = { happy: number; sad: number; relaxed: number; angry: number };
const FACES: Record<string, Face> = {
  neutral: { happy: 0, sad: 0, relaxed: 0, angry: 0 },
  happy: { happy: 0.8, sad: 0, relaxed: 0, angry: 0 },
  concerned: { happy: 0, sad: 0.7, relaxed: 0, angry: 0 },
  thinking: { happy: 0, sad: 0, relaxed: 0.35, angry: 0 },
};

export async function mountAvatar(root: HTMLElement) {
  root.innerHTML = `
    <div class="avatar3d">
      <canvas></canvas>
      <p class="avatar-hint" hidden></p>
      <span class="avatar-badge" hidden></span>
    </div>`;
  const canvas = root.querySelector<HTMLCanvasElement>("canvas")!;
  const hint = root.querySelector<HTMLElement>(".avatar-hint")!;
  const badge = root.querySelector<HTMLElement>(".avatar-badge")!;
  const win = getCurrentWindow();

  const renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true, powerPreference: "low-power" });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.5));
  renderer.setClearColor(0x000000, 0);
  renderer.outputColorSpace = THREE.SRGBColorSpace;

  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(22, 1, 0.1, 20);
  const key = new THREE.DirectionalLight(0xffffff, 1.9);
  key.position.set(0.5, 1.1, 1.6);
  scene.add(key, new THREE.AmbientLight(0xffffff, 0.9));

  function resize() {
    const w = canvas.clientWidth || 360;
    const h = canvas.clientHeight || 440;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  }
  window.addEventListener("resize", resize);
  resize();

  // ── carrega o modelo ──────────────────────────────────────────────────

  let vrm: VRM | null = null;
  let waistY = 0;
  let topY = 1;
  const lookTarget = new THREE.Object3D();
  scene.add(lookTarget);

  try {
    const bytes = await invoke<ArrayBuffer>("avatar_model");
    const loader = new GLTFLoader();
    loader.register((parser) => new VRMLoaderPlugin(parser));
    const gltf = await loader.parseAsync(bytes, "");
    vrm = gltf.userData.vrm as VRM;
    VRMUtils.removeUnnecessaryVertices(gltf.scene);
    VRMUtils.rotateVRM0(vrm);
    gltf.scene.traverse((o) => (o.frustumCulled = false));
    scene.add(vrm.scene);
    if (vrm.lookAt) vrm.lookAt.target = lookTarget;
    relaxArms(vrm);
    frame(vrm);
  } catch (e) {
    console.warn("sem modelo 3D", e);
    hint.hidden = false;
    hint.textContent = "Coloque o arquivo lilith.vrm na pasta %APPDATA%\\Lilith e abra a forma 3D de novo.";
  }

  /// O modelo vem em T-pose: baixa os braços para uma pose natural.
  function relaxArms(v: VRM) {
    const h = v.humanoid;
    const set = (name: Parameters<typeof h.getNormalizedBoneNode>[0], x: number, y: number, z: number) => {
      const n = h.getNormalizedBoneNode(name);
      if (n) n.rotation.set(x, y, z);
    };
    set("leftUpperArm", 0, 0, 1.15);
    set("rightUpperArm", 0, 0, -1.15);
    set("leftLowerArm", 0, 0.12, 0.08);
    set("rightLowerArm", 0, -0.12, -0.08);
    set("leftHand", 0, 0, 0.05);
    set("rightHand", 0, 0, -0.05);
  }

  /// Enquadra da cintura (quadril) ao topo da cabeça.
  function frame(v: VRM) {
    v.scene.updateMatrixWorld(true);
    const y = (b: Parameters<typeof v.humanoid.getNormalizedBoneNode>[0]) => {
      const n = v.humanoid.getNormalizedBoneNode(b);
      return n ? n.getWorldPosition(new THREE.Vector3()).y : 0;
    };
    const hips = y("hips");
    const head = y("head");
    const neck = y("neck");
    waistY = hips - (y("spine") - hips) * 1.2;
    topY = head + (head - neck) * 3.6;
    const height = topY - waistY;
    const dist = (height / 2 / Math.tan((camera.fov * Math.PI) / 360)) * 1.1;
    camera.position.set(0, (topY + waistY) / 2, dist);
    camera.lookAt(0, (topY + waistY) / 2, 0);
    lookTarget.position.set(0, topY - 0.08, 1.5);
  }

  // ── emoções, estado e olhar ───────────────────────────────────────────

  let face: Face = { ...FACES.neutral };
  let target: Face = FACES.neutral;
  let thinking = false;
  let emotionTimer: number | undefined;
  /// Boca aberta (0..1), alimentada pela voz (lip-sync).
  let mouth = 0;

  function applyTarget(name: string) {
    target = FACES[name] ?? FACES.neutral;
  }

  void listen<{ state: string }>("lilith://state", (e) => {
    thinking = e.payload.state === "thinking";
    if (thinking) applyTarget("thinking");
    else if (!emotionTimer) applyTarget("neutral");
  });
  void listen<{ emotion?: string }>("lilith://reply", (e) => {
    thinking = false;
    applyTarget(e.payload.emotion ?? "neutral");
    window.clearTimeout(emotionTimer);
    emotionTimer = window.setTimeout(() => {
      emotionTimer = undefined;
      applyTarget("neutral");
    }, 6000);
  });
  // Bolinha de pendências (como na orb): aprovações e tarefas em andamento.
  const approvals = new Set<number>();
  const jobs = new Set<number>();
  function refreshBadge() {
    const pending = approvals.size + jobs.size;
    badge.hidden = pending === 0;
    badge.textContent = pending > 9 ? "9+" : String(pending);
    badge.dataset.kind = approvals.size > 0 ? "approval" : "job";
    badge.title = approvals.size > 0 ? `${approvals.size} pedido(s) esperando sua aprovação` : `${jobs.size} tarefa(s) rodando`;
  }
  void listen<{ id: number }>("lilith://approval", (e) => (approvals.add(e.payload.id), refreshBadge()));
  void listen<{ id: number }>("lilith://approval-resolved", (e) => (approvals.delete(e.payload.id), refreshBadge()));
  void listen<number>("lilith://approval-expired", (e) => (approvals.delete(e.payload), refreshBadge()));
  void listen("lilith://cancelled", () => (approvals.clear(), refreshBadge()));
  void listen<{ kind: string; job?: { id: number; status: string } }>("lilith://job", (e) => {
    const job = e.payload.job;
    if (e.payload.kind !== "updated" || !job) return;
    if (job.status === "running") jobs.add(job.id);
    else jobs.delete(job.id);
    refreshBadge();
  });
  void listen<number>("lilith://mouth", (e) => (mouth = Math.max(0, Math.min(1, Number(e.payload) || 0))));

  // Olha para o cursor do usuário (consulta global ~12x por segundo).
  let lookX = 0;
  let lookY = 0;
  let overBody = false;
  async function pollCursor() {
    try {
      const [pos, win_pos, size] = await Promise.all([cursorPosition(), win.outerPosition(), win.outerSize()]);
      const cx = win_pos.x + size.width / 2;
      const cy = win_pos.y + size.height * 0.28;
      lookX = Math.max(-1, Math.min(1, (pos.x - cx) / 900));
      lookY = Math.max(-1, Math.min(1, (cy - pos.y) / 700));
      // Fora do corpo, o clique atravessa a janela transparente.
      const inside = hitBody(pos.x - win_pos.x, pos.y - win_pos.y, size.width, size.height);
      if (inside !== overBody) {
        overBody = inside;
        await win.setIgnoreCursorEvents(!inside);
      }
    } catch {
      /* janela fechando */
    }
  }

  /// Área aproximada do corpo na tela (cabeça, ombros e tronco).
  function hitBody(px: number, py: number, w: number, h: number): boolean {
    const nx = px / w;
    const ny = py / h;
    if (nx < 0 || nx > 1 || ny < 0 || ny > 1) return false;
    const headBox = ny < 0.4 && nx > 0.3 && nx < 0.7;
    const torsoBox = ny >= 0.3 && ny < 0.9 && nx > 0.16 && nx < 0.84;
    return headBox || torsoBox;
  }
  const cursorTimer = window.setInterval(() => void pollCursor(), 80);

  // ── laço de animação (30 fps) ─────────────────────────────────────────

  const clock = new THREE.Clock();
  let acc = 0;
  let nextBlink = 1.5;
  let blinkT = -1;
  let t = 0;

  function frameTick() {
    requestAnimationFrame(frameTick);
    acc += clock.getDelta();
    if (acc < 1 / FPS) return;
    const dt = acc;
    acc = 0;
    t += dt;
    if (!vrm) return;
    const h = vrm.humanoid;
    const em = vrm.expressionManager;

    // Respiração e balanço leve.
    const chest = h.getNormalizedBoneNode("chest");
    if (chest) chest.rotation.x = Math.sin(t * 1.7) * 0.012;
    const spine = h.getNormalizedBoneNode("spine");
    if (spine) spine.rotation.z = Math.sin(t * 0.6) * 0.008;

    // Cabeça: segue o cursor e inclina um pouco ao pensar.
    const head = h.getNormalizedBoneNode("head");
    const neck = h.getNormalizedBoneNode("neck");
    const tiltZ = thinking ? 0.07 : 0;
    const yaw = lookX * 0.32 + Math.sin(t * 0.45) * 0.03;
    const pitch = -lookY * 0.16 + (thinking ? -0.05 : 0);
    if (head) {
      head.rotation.y += (yaw * 0.6 - head.rotation.y) * 0.12;
      head.rotation.x += (pitch * 0.6 - head.rotation.x) * 0.12;
      head.rotation.z += (tiltZ - head.rotation.z) * 0.08;
    }
    if (neck) {
      neck.rotation.y += (yaw * 0.4 - neck.rotation.y) * 0.12;
      neck.rotation.x += (pitch * 0.4 - neck.rotation.x) * 0.12;
    }
    lookTarget.position.set(lookX * 0.9, camera.position.y + lookY * 0.5, 1.5);

    // Expressões: aproxima suavemente do alvo.
    for (const k of Object.keys(face) as (keyof Face)[]) {
      face[k] += (target[k] - face[k]) * Math.min(1, dt * 6);
    }
    if (em) {
      em.setValue("happy", face.happy);
      em.setValue("sad", face.sad);
      em.setValue("relaxed", face.relaxed);
      em.setValue("angry", face.angry);
      // Piscar em intervalos irregulares (às vezes duas seguidas).
      nextBlink -= dt;
      if (nextBlink <= 0 && blinkT < 0) blinkT = 0;
      if (blinkT >= 0) {
        blinkT += dt;
        const v = blinkT < 0.07 ? blinkT / 0.07 : Math.max(0, 1 - (blinkT - 0.07) / 0.1);
        em.setValue("blink", v);
        if (blinkT > 0.18) {
          blinkT = -1;
          nextBlink = Math.random() < 0.2 ? 0.25 : 2 + Math.random() * 3.5;
        }
      }
      // Boca (voz): "aa" proporcional ao volume.
      em.setValue("aa", mouth);
    }
    vrm.update(dt);
    renderer.render(scene, camera);
  }
  requestAnimationFrame(frameTick);

  // ── clique, duplo clique e arrastar ───────────────────────────────────

  let clickTimer: number | undefined;
  let down: { x: number; y: number } | null = null;
  canvas.addEventListener("mousedown", (e) => {
    if (e.button === 0) down = { x: e.screenX, y: e.screenY };
  });
  canvas.addEventListener("mousemove", (e) => {
    if (!down) return;
    if (Math.hypot(e.screenX - down.x, e.screenY - down.y) > DRAG_THRESHOLD) {
      down = null;
      void win.startDragging();
    }
  });
  canvas.addEventListener("mouseup", () => {
    if (!down) return;
    down = null;
    if (clickTimer) {
      // Duplo clique: volta para a forma 2D.
      clearTimeout(clickTimer);
      clickTimer = undefined;
      void invoke("toggle_avatar");
    } else {
      clickTimer = window.setTimeout(() => {
        clickTimer = undefined;
        void invoke("toggle_chat");
      }, DOUBLE_CLICK_MS);
    }
  });
  canvas.addEventListener("contextmenu", async (e) => {
    e.preventDefault();
    const menu = await Menu.new({
      items: [
        { id: "new", text: "Nova conversa", action: () => void invoke("new_chat") },
        { id: "chat", text: "Abrir / fechar o chat", action: () => void invoke("toggle_chat") },
        { id: "panel", text: "Painel da Lilith", action: () => void invoke("show_panel_tab", { tab: "jobs" }) },
        { item: "Separator" },
        { id: "stop", text: "Parar tudo (Ctrl+Alt+Esc)", action: () => void invoke("cancel") },
        { id: "2d", text: "Voltar para a forma 2D", action: () => void invoke("toggle_avatar") },
        { item: "Separator" },
        { id: "quit", text: "Sair da Lilith", action: () => void invoke("quit_app") },
      ],
    });
    await menu.popup();
  });

  window.addEventListener("beforeunload", () => {
    window.clearInterval(cursorTimer);
    renderer.dispose();
  });
}
