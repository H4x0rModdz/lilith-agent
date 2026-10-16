import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { VRMLoaderPlugin, VRMUtils, type VRM } from "@pixiv/three-vrm";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Menu } from "@tauri-apps/api/menu";
import { cursorPosition, getCurrentWindow, Window } from "@tauri-apps/api/window";
import { GesturePlayer, gestureForReply, isPointingStep, sideOf, type Bone, type Pose, type Vec3 } from "./gestures";

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
    <div class="avatar3d intro">
      <canvas></canvas>
      <span class="dock-hint" hidden>solte para guardar</span>
      <p class="avatar-hint" hidden></p>
      <span class="avatar-badge" hidden></span>
    </div>`;
  const canvas = root.querySelector<HTMLCanvasElement>("canvas")!;
  const hint = root.querySelector<HTMLElement>(".avatar-hint")!;
  const badge = root.querySelector<HTMLElement>(".avatar-badge")!;
  const win = getCurrentWindow();
  const stage = root.querySelector<HTMLElement>(".avatar3d")!;
  const dockHint = root.querySelector<HTMLElement>(".dock-hint")!;

  /// Faíscas rosa saindo do canto da orb (entrada e saída).
  function sparkles() {
    for (let i = 0; i < 16; i++) {
      const s = document.createElement("span");
      s.className = "spark";
      const angle = Math.random() * Math.PI * 2;
      const dist = 70 + Math.random() * 120;
      s.style.setProperty("--dx", `${Math.cos(angle) * dist}px`);
      s.style.setProperty("--dy", `${Math.sin(angle) * dist}px`);
      s.style.animationDelay = `${Math.random() * 120}ms`;
      stage.append(s);
      window.setTimeout(() => s.remove(), 900);
    }
  }
  sparkles();
  window.setTimeout(() => stage.classList.remove("intro"), 900);

  /// Volta para a forma 2D com a animação de "guardar" (e só então troca a janela).
  let leaving = false;
  function dismiss() {
    if (leaving) return;
    leaving = true;
    dockHint.hidden = true;
    sparkles();
    stage.classList.add("outro");
    window.setTimeout(() => void invoke("toggle_avatar"), 420);
  }
  void listen<boolean>("lilith://avatar-dock-hint", (e) => {
    dockHint.hidden = !e.payload || leaving;
    stage.classList.toggle("near-corner", e.payload);
  });
  void listen("lilith://avatar-dock", () => dismiss());

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
  /// VRM 0.x é girado 180° para ficar de frente: os eixos X e Z locais ficam
  /// espelhados (o Y, não). -1 corrige a inclinação vertical e os braços.
  let flip = 1;
  /// Escondida (forma 2D): não renderiza nem consulta o cursor.
  let paused = false;
  let waistY = 0;
  let topY = 1;
  const lookTarget = new THREE.Object3D();
  scene.add(lookTarget);
  /// Gestos (aceno, sim/não, apontar…), somados por cima da pose de cada quadro.
  const gestures = new GesturePlayer();

  try {
    const bytes = await invoke<ArrayBuffer>("avatar_model");
    const loader = new GLTFLoader();
    loader.register((parser) => new VRMLoaderPlugin(parser));
    const gltf = await loader.parseAsync(bytes, "");
    vrm = gltf.userData.vrm as VRM;
    flip = vrm.meta?.metaVersion === "0" ? -1 : 1;
    VRMUtils.removeUnnecessaryVertices(gltf.scene);
    VRMUtils.rotateVRM0(vrm);
    gltf.scene.traverse((o) => (o.frustumCulled = false));
    scene.add(vrm.scene);
    if (vrm.lookAt) vrm.lookAt.target = lookTarget;
    relaxArms(vrm);
    frame(vrm);
    // Chegou: acena (depois das faíscas de entrada).
    window.setTimeout(() => gestures.play("wave"), 500);
  } catch (e) {
    const msg = String(e);
    console.warn("sem modelo 3D", e);
    // Só pede o arquivo quando ele realmente não existe; outras falhas mostram o motivo.
    const missing = msg.includes("não encontrado");
    hint.hidden = false;
    hint.textContent = missing
      ? "Coloque o arquivo lilith.vrm na pasta %APPDATA%\Lilith e abra a forma 3D de novo."
      : `Não consegui carregar o modelo 3D: ${msg.slice(0, 160)}`;
    void invoke("js_log", { msg: `modelo 3D: ${msg}` });
  }

  /// O modelo vem em T-pose: baixa os braços para uma pose natural.
  function relaxArms(v: VRM) {
    const h = v.humanoid;
    const set = (name: Parameters<typeof h.getNormalizedBoneNode>[0], x: number, y: number, z: number) => {
      const n = h.getNormalizedBoneNode(name);
      if (n) n.rotation.set(x, y, z);
    };
    const k = -flip; // VRM0 (flip = -1): braço esquerdo em +z
    set("leftUpperArm", 0, 0, 1.15 * k);
    set("rightUpperArm", 0, 0, -1.15 * k);
    set("leftLowerArm", 0, 0.12 * k, 0.08 * k);
    set("rightLowerArm", 0, -0.12 * k, -0.08 * k);
    set("leftHand", 0, 0, 0.05 * k);
    set("rightHand", 0, 0, -0.05 * k);
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
  /// Ouvindo o usuário (push-to-talk): inclina a cabeça para o outro lado.
  let listening = false;
  let emotionTimer: number | undefined;
  /// Boca aberta (0..1), alimentada pela voz (lip-sync).
  let mouth = 0;

  function applyTarget(name: string) {
    target = FACES[name] ?? FACES.neutral;
  }

  void listen<{ state: string }>("lilith://state", (e) => {
    thinking = e.payload.state === "thinking";
    gestures.hold("think", thinking);
    if (thinking) applyTarget("thinking");
    else if (!emotionTimer) applyTarget("neutral");
  });
  void listen<{ state: string }>("lilith://listening", (e) => {
    listening = e.payload.state === "start";
  });
  void listen<{ emotion?: string; reply?: string; action?: string | null }>("lilith://reply", (e) => {
    thinking = false;
    gestures.hold("think", false);
    const g = gestureForReply(e.payload);
    if (g) gestures.play(g);
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
  void listen<{ id: number }>("lilith://approval-resolved", (e) => {
    approvals.delete(e.payload.id);
    refreshBadge();
    gestures.play("nod"); // "entendido"
  });
  void listen<number>("lilith://approval-expired", (e) => (approvals.delete(e.payload), refreshBadge()));
  void listen("lilith://cancelled", () => {
    approvals.clear();
    refreshBadge();
    gestures.hold("think", false);
  });
  void listen<{ kind: string; job?: { id: number; status: string } }>("lilith://job", (e) => {
    const job = e.payload.job;
    if (e.payload.kind !== "updated" || !job) return;
    if (job.status === "running") jobs.add(job.id);
    else jobs.delete(job.id);
    refreshBadge();
  });
  void listen<number>("lilith://mouth", (e) => (mouth = Math.max(0, Math.min(1, Number(e.payload) || 0))));

  // ── gestos (decididos pelo código, por eventos) ───────────────────────

  void invoke<{ gestures?: boolean }>("get_settings")
    .then((s) => setGestures(s.gestures !== false))
    .catch(() => {});
  void listen<boolean>("lilith://gestures", (e) => setGestures(e.payload !== false));
  function setGestures(on: boolean) {
    gestures.enabled = on;
    if (!on) gestures.stopAll();
    else if (thinking) gestures.hold("think", true);
  }
  // Ação barrada pelos guardrails: "não" com a cabeça.
  void listen("lilith://blocked", () => gestures.play("shake"));
  // O cursor dela vai até uma janela: aponta para o lado da tela onde isso acontece.
  void listen<{ action: string }>("lilith://step", (e) => {
    if (isPointingStep(e.payload.action)) void pointAtAction();
  });
  // Voz de entrada (se existir): pose de quem está ouvindo.
  void listen<unknown>("lilith://listening", (e) => {
    const p = e.payload as { listening?: boolean; active?: boolean; state?: string } | boolean | null;
    const on = typeof p === "boolean" ? p : !!p && (p.listening ?? p.active ?? p.state === "listening");
    gestures.hold("listen", on);
  });

  /// Acompanha a seta rosa (janela "cursor") até ela parar no alvo e aponta
  /// para aquele lado em relação ao centro da janela do avatar.
  let pointSeq = 0;
  async function pointAtAction() {
    const seq = ++pointSeq;
    const cursor = await Window.getByLabel("cursor").catch(() => null);
    if (!cursor) return;
    try {
      const start = await cursor.outerPosition();
      let prev = start;
      let target: { x: number } | null = null;
      for (let i = 0; i < 20 && seq === pointSeq && !paused; i++) {
        await new Promise((r) => window.setTimeout(r, 120));
        const p = await cursor.outerPosition();
        const moved = Math.hypot(p.x - start.x, p.y - start.y) > 30;
        if (moved && Math.hypot(p.x - prev.x, p.y - prev.y) < 4) {
          target = p;
          break;
        }
        prev = p;
      }
      if (!target || seq !== pointSeq || paused) return;
      const [wp, ws] = await Promise.all([win.outerPosition(), win.outerSize()]);
      const side = sideOf(target.x, wp.x + ws.width / 2);
      if (side) gestures.play("point", side);
    } catch {
      /* janela fechando */
    }
  }

  /// O que foi somado no último quadro: tirado antes da pose base do quadro
  /// seguinte, para os gestos virarem camada (sem brigar com o olhar e a respiração).
  let applied: Pose = { rot: {}, lift: 0 };
  function addPose(v: VRM, p: Pose, sign: number) {
    const h = v.humanoid;
    for (const [bone, r] of Object.entries(p.rot) as [Bone, Vec3][]) {
      const n = h.getNormalizedBoneNode(bone);
      if (!n) continue;
      // VRM 0.x: X e Z espelhados (ver `flip`).
      n.rotation.x += sign * r[0] * flip;
      n.rotation.y += sign * r[1];
      n.rotation.z += sign * r[2] * flip;
    }
    const hips = h.getNormalizedBoneNode("hips");
    if (hips && p.lift) hips.position.y += sign * p.lift;
  }

  // Olha para o cursor do usuário (consulta global ~12x por segundo).
  let lookX = 0;
  let lookY = 0;
  let overBody = false;
  async function pollCursor() {
    if (paused) return;
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
    if (paused) return;
    acc += clock.getDelta();
    if (acc < 1 / FPS) return;
    const dt = acc;
    acc = 0;
    t += dt;
    if (!vrm) return;
    const h = vrm.humanoid;
    const em = vrm.expressionManager;
    // Tira os gestos do quadro anterior: a pose base segue sozinha.
    addPose(vrm, applied, -1);

    // Respiração e balanço leve.
    const chest = h.getNormalizedBoneNode("chest");
    if (chest) chest.rotation.x = Math.sin(t * 1.7) * 0.012;
    const spine = h.getNormalizedBoneNode("spine");
    if (spine) spine.rotation.z = Math.sin(t * 0.6) * 0.008;

    // Cabeça: segue o cursor e inclina um pouco ao pensar.
    const head = h.getNormalizedBoneNode("head");
    const neck = h.getNormalizedBoneNode("neck");
    const tiltZ = thinking ? 0.07 : listening ? -0.06 : 0;
    const yaw = lookX * 0.32 + Math.sin(t * 0.45) * 0.03;
    const pitch = flip * (-lookY * 0.16 + (thinking ? -0.05 : 0));
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
    // Gestos por cima de tudo (camada somada).
    applied = gestures.update(dt);
    addPose(vrm, applied, 1);
    vrm.update(dt);
    renderer.render(scene, camera);
  }
  // A janela fica carregada quando volta para a forma 2D: pausa, e na volta
  // toca a animação de entrada de novo.
  void listen<boolean>("lilith://avatar-visible", (e) => {
    paused = !e.payload;
    if (!e.payload) {
      // Escondida: some com os gestos (o próximo quadro tira o que estava somado).
      gestures.reset();
      return;
    }
    leaving = false;
    window.setTimeout(() => gestures.play("wave"), 500);
    if (thinking) gestures.hold("think", true);
    dockHint.hidden = true;
    stage.classList.remove("outro", "near-corner");
    stage.classList.add("intro");
    sparkles();
    window.setTimeout(() => stage.classList.remove("intro"), 900);
    clock.getDelta();
    acc = 0;
  });

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
      dismiss();
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
        { id: "2d", text: "Voltar para a forma 2D", action: () => dismiss() },
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
