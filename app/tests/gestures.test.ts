// Testes da lógica pura dos gestos (sem three.js). Rodar: `npm test` em app/.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  GesturePlayer,
  TUNING,
  armForSide,
  ease,
  envelope,
  gestureForReply,
  isGreeting,
  isPointingStep,
  mirror,
  sideOf,
} from "../src/gestures.ts";

const FRAME = 1 / 30;

function run(p: GesturePlayer, seconds: number) {
  let last = p.update(0);
  for (let t = 0; t < seconds; t += FRAME) last = p.update(FRAME);
  return last;
}

test("ease is smooth and clamped", () => {
  assert.equal(ease(-1), 0);
  assert.equal(ease(0), 0);
  assert.equal(ease(1), 1);
  assert.equal(ease(2), 1);
  assert.ok(Math.abs(ease(0.5) - 0.5) < 1e-9);
  assert.ok(ease(0.1) < 0.1, "starts slow");
});

test("envelope rises, holds and falls back to zero", () => {
  assert.equal(envelope(0, 2, 0.4, 0.5), 0);
  assert.ok(envelope(0.2, 2, 0.4, 0.5) > 0 && envelope(0.2, 2, 0.4, 0.5) < 1);
  assert.equal(envelope(1, 2, 0.4, 0.5), 1);
  assert.ok(envelope(1.8, 2, 0.4, 0.5) < 1);
  assert.equal(envelope(2, 2, 0.4, 0.5), 0);
});

test("mirror flips Y and Z for the other arm", () => {
  assert.deepEqual(mirror([0.1, 0.2, 0.3]), [0.1, -0.2, -0.3]);
  assert.equal(armForSide("right"), "left", "her left arm is on the right of the screen");
  assert.equal(armForSide("left"), "right");
});

test("a one-shot gesture blends in and returns to idle", () => {
  const p = new GesturePlayer();
  assert.ok(p.play("nod"));
  assert.equal(p.current, "nod");
  const mid = run(p, 0.25);
  assert.ok((mid.rot.head?.[0] ?? 0) > 0, "head goes down");
  const end = run(p, TUNING.nod.duration);
  assert.equal(p.current, null);
  assert.deepEqual(end.rot, {}, "nothing left applied");
  assert.equal(end.lift, 0);
});

test("no jumps: per-frame change stays small", () => {
  const p = new GesturePlayer();
  p.play("wave");
  let prev = p.update(0);
  for (let i = 0; i < 90; i++) {
    if (i === 20) p.play("point", "right"); // cancels the wave mid-way
    const cur = p.update(FRAME);
    for (const bone of new Set([...Object.keys(prev.rot), ...Object.keys(cur.rot)]) as Set<keyof typeof cur.rot>) {
      const a = prev.rot[bone] ?? [0, 0, 0];
      const b = cur.rot[bone] ?? [0, 0, 0];
      for (let k = 0; k < 3; k++) assert.ok(Math.abs(a[k] - b[k]) < 0.25, `${bone}[${k}] jumped at frame ${i}`);
    }
    prev = cur;
  }
});

test("higher or equal priority cancels; lower priority waits in the queue", () => {
  const p = new GesturePlayer();
  p.play("nod");
  assert.ok(p.play("wave"));
  assert.equal(p.current, "wave", "wave (3) replaces nod (2)");
  assert.ok(p.play("shake"));
  assert.equal(p.current, "wave", "shake (2) does not cut the wave");
  assert.equal(p.pending, 1);
  run(p, TUNING.wave.duration);
  assert.equal(p.current, "shake", "queued gesture plays after");
});

test("same gesture is not restarted while playing", () => {
  const p = new GesturePlayer();
  p.play("bounce");
  run(p, 0.3);
  assert.equal(p.play("bounce"), false);
});

test("queued gestures expire", () => {
  // A wave longer than the queue TTL drops the waiting nod.
  TUNING.queueTtl = 0.5;
  try {
    const q = new GesturePlayer();
    q.play("wave");
    q.play("nod");
    run(q, 1);
    assert.equal(q.pending, 0);
    run(q, TUNING.wave.duration);
    assert.equal(q.current, null);
  } finally {
    TUNING.queueTtl = 2.5;
  }
});

test("held pose ducks under a one-shot and comes back", () => {
  const p = new GesturePlayer();
  p.hold("think", true);
  const held = run(p, 1);
  const elbow = held.rot.rightLowerArm?.[1] ?? 0;
  assert.ok(elbow > 1.5, "thinking pose fully in");
  p.play("nod");
  const during = run(p, 0.45);
  assert.ok((during.rot.rightLowerArm?.[1] ?? 0) < elbow * 0.2, "ducked during the nod");
  const after = run(p, 1.5);
  assert.ok(Math.abs((after.rot.rightLowerArm?.[1] ?? 0) - elbow) < 1e-6, "back after");
  p.hold("think", false);
  const released = run(p, 1);
  assert.equal(released.rot.rightLowerArm, undefined, "released smoothly to idle");
});

test("disabled player ignores gestures; stopAll fades out", () => {
  const p = new GesturePlayer();
  p.play("wave");
  run(p, 0.5);
  p.enabled = false;
  p.stopAll();
  assert.equal(p.play("nod"), false);
  const end = run(p, 0.5);
  assert.deepEqual(end.rot, {});
});

test("point uses the arm on that side of the screen and turns the head", () => {
  const p = new GesturePlayer();
  p.play("point", "right");
  const pose = run(p, 0.6);
  assert.ok(pose.rot.leftUpperArm && !pose.rot.rightUpperArm);
  assert.ok((pose.rot.head?.[1] ?? 0) > 0, "looks to the right of the screen");
});

test("bounce lifts and lands", () => {
  const p = new GesturePlayer();
  p.play("bounce");
  let max = 0;
  let last = 0;
  for (let t = 0; t < 1.2; t += FRAME) {
    last = p.update(FRAME).lift;
    max = Math.max(max, last);
  }
  assert.ok(max > 0 && max <= TUNING.bounce.lift + 1e-9);
  assert.equal(last, 0);
});

test("reply → gesture is decided by code", () => {
  assert.equal(gestureForReply({ reply: "Oi! Tudo bem?", emotion: "happy" }), "wave");
  assert.equal(gestureForReply({ reply: "Boa noite, até amanhã", emotion: "neutral" }), "wave");
  assert.equal(gestureForReply({ reply: "Não consegui abrir", emotion: "concerned" }), "shake");
  assert.equal(gestureForReply({ reply: "Pronto, abri o Spotify!", emotion: "happy" }), "bounce");
  assert.equal(gestureForReply({ reply: "Volume em 30.", emotion: "neutral", action: "volume.set" }), "nod");
  assert.equal(gestureForReply({ reply: "São 15h.", emotion: "neutral", action: null }), null);
  assert.ok(!isGreeting("Oito janelas abertas"), "word boundary");
  assert.ok(!isGreeting("oi ".repeat(100)), "long texts are not greetings");
});

test("pointing steps and screen side", () => {
  assert.ok(isPointingStep("cursor.point"));
  assert.ok(isPointingStep("window.minimize"));
  assert.ok(!isPointingStep("window.list"));
  assert.ok(!isPointingStep("window.read"));
  assert.ok(!isPointingStep("app.open"));
  assert.equal(sideOf(2000, 1000), "right");
  assert.equal(sideOf(100, 1000), "left");
  assert.equal(sideOf(1030, 1000), null, "dead zone");
});
