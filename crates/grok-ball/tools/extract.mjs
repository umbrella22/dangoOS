#!/usr/bin/env node
//! Deterministic generator for `crates/grok-ball/src/data.rs`.
//!
//! Reads the pinned upstream `grok-ball.js`, evaluates only its data block
//! (`window.EB_RINGS`, `window.EMOTION_GROUPS`, `window.EMOTION_SEED`) with a
//! minimal `window` stub, normalizes the expression seeds to the frozen
//! `ExpressionSeed` subset and emits a rustfmt-clean Rust module.
//!
//! The upstream repository is read-only here. The source path may be
//! overridden with `GROK_BALL_JS`, but the recorded commit and SHA-256 always
//! describe the file that was actually read.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const HERE = dirname(fileURLToPath(import.meta.url));
const CRATE_DIR = resolve(HERE, "..");
const OUT_PATH = resolve(CRATE_DIR, "src", "data.rs");

const DEFAULT_UPSTREAM = "/Volumes/project_home/github/grok-ball/src/grok-ball.js";
const SOURCE_PATH = resolve(process.env.GROK_BALL_JS || DEFAULT_UPSTREAM);

const SOURCE_URL = "https://github.com/tycoding/grok-ball";
const LICENSE = "MIT";
const EXTRACTED = "2026-10-20";

const RING_SETS = 25;
const RING_POINTS = 48;
const BODY_POINTS = 96;
const SEED_COUNT = 32;

// Upstream driver default (`DEFAULT_BODY.color` and the compatibility facade's
// `emotion.body.color || '#F3F0EA'`).
const DEFAULT_BODY_COLOR = "#F3F0EA";

// Fields intentionally not carried into the first-release Rust types. The four
// pose fields and `color` are mapped; anything else is upstream decoration.
const IGNORED_BODY_FIELDS = new Set([
  "y",
  "scale",
  "rotate",
  "breathe",
  "color",
  "ribbons",
  "confetti",
  "sketch",
  "zzz",
  "orbit",
]);

function fail(message) {
  throw new Error(`extract.mjs: ${message}`);
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function integer(value, where, min = 0, max = 0xffff) {
  if (!Number.isInteger(value) || value < min || value > max) {
    fail(`${where}: expected an integer in ${min}..=${max}, got ${JSON.stringify(value)}`);
  }
  return value;
}

function number(value, where) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    fail(`${where}: expected a finite number, got ${JSON.stringify(value)}`);
  }
  return value;
}

function f32(value, where) {
  const n = number(value, where);
  let text = String(n);
  if (!text.includes(".") && !text.includes("e") && !text.includes("E")) {
    text += ".0";
  }
  return text;
}

function u16(value, where) {
  return String(integer(value, where, 0, 0xffff));
}

function msPair(value, where, absentValue) {
  if (value === null || value === undefined) {
    if (absentValue === undefined) {
      fail(`${where}: expected a two-element millisecond pair`);
    }
    return absentValue;
  }
  if (!Array.isArray(value) || value.length !== 2) {
    fail(`${where}: expected [min, max], got ${JSON.stringify(value)}`);
  }
  return [integer(value[0], `${where}[0]`), integer(value[1], `${where}[1]`)];
}

function rgb(value, where) {
  if (typeof value !== "string" || !/^#[0-9a-fA-F]{6}$/.test(value)) {
    fail(`${where}: expected #RRGGBB, got ${JSON.stringify(value)}`);
  }
  const hex = value.slice(1).toLowerCase();
  return `Rgb::from_hex(0x${hex})`;
}

function pointLiteral(point, where) {
  if (!Array.isArray(point) || point.length !== 2) {
    fail(`${where}: expected [x, y], got ${JSON.stringify(point)}`);
  }
  return `Point { x: ${f32(point[0], `${where}.x`)}, y: ${f32(point[1], `${where}.y`)} },`;
}

// ---------------------------------------------------------------------------
// Read and evaluate the data block.
// ---------------------------------------------------------------------------

let sourceBytes;
try {
  sourceBytes = readFileSync(SOURCE_PATH);
} catch (error) {
  fail(`cannot read upstream source ${SOURCE_PATH}: ${error.message}`);
}
const sourceText = sourceBytes.toString("utf8");
const sourceSha256 = createHash("sha256").update(sourceBytes).digest("hex");
const sourceLines = sourceText.split(/\r?\n/);

const startIndex = sourceLines.findIndex((line) => /^window\.EB_RINGS\s*=\s*\{/.test(line));
if (startIndex < 0) {
  fail(`did not find the window.EB_RINGS data block in ${SOURCE_PATH}`);
}
const endIndex = sourceLines.findIndex(
  (line, index) => index > startIndex && /^\(function\s*\(\)\s*\{/.test(line),
);
if (endIndex < 0) {
  fail("did not find the renderer IIFE that terminates the data block");
}
const dataBlock = sourceLines.slice(startIndex, endIndex).join("\n");

const window = {};
vm.runInNewContext(dataBlock, { window }, { filename: SOURCE_PATH });

const rings = window.EB_RINGS;
if (!isObject(rings)) {
  fail("window.EB_RINGS is not an object");
}
const expressions = rings.EXPRESSIONS;
const bodyRing = rings.SHAPES && rings.SHAPES.blob && rings.SHAPES.blob.ring;
const rawSeeds = window.EMOTION_SEED;

if (!Array.isArray(expressions) || expressions.length !== RING_SETS) {
  fail(`EB_RINGS.EXPRESSIONS: expected ${RING_SETS} sets`);
}
if (!Array.isArray(bodyRing) || bodyRing.length !== BODY_POINTS) {
  fail(`EB_RINGS.SHAPES.blob.ring: expected ${BODY_POINTS} points`);
}
if (!Array.isArray(rawSeeds) || rawSeeds.length !== SEED_COUNT) {
  fail(`EMOTION_SEED: expected ${SEED_COUNT} seeds`);
}

// ---------------------------------------------------------------------------
// Normalize the upstream seed descriptors.
// ---------------------------------------------------------------------------

const seenIds = new Set();
const ignoredBody = new Set();
const ignoredEyes = new Set();

function mergeEyePose(eyes, side) {
  const pose = { open: 1, scaleX: 1, scaleY: 1, x: 0, y: 0, lookX: 0, lookY: 0 };
  const apply = (patch, where) => {
    if (patch === undefined) {
      return;
    }
    if (!isObject(patch)) {
      fail(`${where}: expected an object`);
    }
    for (const key of Object.keys(patch)) {
      if (!(key in pose)) {
        ignoredEyes.add(key);
        continue;
      }
      pose[key] = number(patch[key], `${where}.${key}`);
    }
  };
  if (eyes !== undefined && !isObject(eyes)) {
    fail("eyes: expected an object");
  }
  apply(eyes && eyes.both, "eyes.both");
  apply(eyes && eyes[side], `eyes.${side}`);
  return pose;
}

const seeds = rawSeeds.map((raw, index) => {
  const where = `EMOTION_SEED[${index}]`;
  if (!isObject(raw)) {
    fail(`${where}: expected an object`);
  }
  if (typeof raw.id !== "string" || !/^\d{1,3}$/.test(raw.id)) {
    fail(`${where}.id: expected a numeric string, got ${JSON.stringify(raw.id)}`);
  }
  const id = Number.parseInt(raw.id, 10);
  if (!Number.isInteger(id) || id < 0 || id > 0xff) {
    fail(`${where}.id: ${raw.id} does not fit u8`);
  }
  if (seenIds.has(id)) {
    fail(`${where}.id: duplicate id ${id}`);
  }
  seenIds.add(id);

  if (!Array.isArray(raw.pool) || raw.pool.length === 0) {
    fail(`${where}.pool: expected a non-empty array`);
  }
  const pool = raw.pool.map((entry, i) =>
    integer(entry, `${where}.pool[${i}]`, 0, RING_SETS - 1),
  );

  // poolMs absent -> 0/0 per the G0.1 contract.
  const poolMs = msPair(raw.poolMs, `${where}.poolMs`, [0, 0]);
  // blinkMs null or absent -> 0/0 (no blinking) per the G0.1 contract.
  const blinkMs = msPair(raw.blinkMs, `${where}.blinkMs`, [0, 0]);
  // Every pinned seed has `transition`; fall back to the upstream 500 ms default.
  const transition = raw.transition === undefined ? 500 : integer(raw.transition, `${where}.transition`);
  const openness = raw.openness === undefined ? 1 : number(raw.openness, `${where}.openness`);

  const bodySpec = raw.body === undefined ? {} : raw.body;
  if (!isObject(bodySpec)) {
    fail(`${where}.body: expected an object`);
  }
  for (const key of Object.keys(bodySpec)) {
    if (!IGNORED_BODY_FIELDS.has(key)) {
      ignoredBody.add(key);
    }
  }
  const body = {
    y: bodySpec.y === undefined ? 0 : number(bodySpec.y, `${where}.body.y`),
    scale: bodySpec.scale === undefined ? 1 : number(bodySpec.scale, `${where}.body.scale`),
    rotate: bodySpec.rotate === undefined ? 0 : number(bodySpec.rotate, `${where}.body.rotate`),
    breathe:
      bodySpec.breathe === undefined ? 0 : number(bodySpec.breathe, `${where}.body.breathe`),
    color: rgb(bodySpec.color === undefined ? DEFAULT_BODY_COLOR : bodySpec.color, `${where}.body.color`),
  };

  return {
    id,
    pool,
    poolMs,
    transition,
    blinkMs,
    openness,
    body,
    left: mergeEyePose(raw.eyes, "left"),
    right: mergeEyePose(raw.eyes, "right"),
  };
});

// ---------------------------------------------------------------------------
// Record the pinned upstream revision.
// ---------------------------------------------------------------------------

const repoDir = resolve(dirname(SOURCE_PATH), "..");
let commit;
try {
  commit = execFileSync("git", ["-C", repoDir, "rev-parse", "HEAD"], {
    encoding: "utf8",
  })
    .trim()
    .toLowerCase();
} catch (error) {
  fail(`cannot read the upstream commit from ${repoDir}: ${error.message}`);
}
if (!/^[0-9a-f]{40,64}$/.test(commit)) {
  fail(`git rev-parse returned an unexpected commit id: ${JSON.stringify(commit)}`);
}

// ---------------------------------------------------------------------------
// Emit Rust.
// ---------------------------------------------------------------------------

const out = [];

const moduleDoc = [
  "//! Generated upstream grok-ball tables.",
  "//!",
  "//! Generated by `crates/grok-ball/tools/extract.mjs` from the pinned data",
  "//! block of `grok-ball.js`; do not edit by hand.",
  "//!",
  `//! - Upstream: <${SOURCE_URL}>`,
  `//! - Source commit: \`${commit}\``,
  `//! - Source SHA-256: \`${sourceSha256}\``,
  `//! - License: ${LICENSE}`,
  `//! - Extracted: ${EXTRACTED}`,
  "//!",
  "//! Normalization defaults applied by the generator:",
  "//!",
  "//! - `poolMs` absent: `0/0`.",
  "//! - `blinkMs: null` or absent: `0/0` (no blinking).",
  "//! - `openness` absent: `1.0`.",
  "//! - `transition` absent: `500` ms (upstream register default).",
  "//! - `body` fields absent: `y = 0`, `scale = 1`, `rotate = 0`, `breathe = 0`.",
  `//! - \`body.color\` absent: \`${DEFAULT_BODY_COLOR}\`, the upstream driver`,
  "//!   default (`DEFAULT_BODY` in `grok-ball.js`).",
  "//! - `eyes` absent: `EyePose::DEFAULT` for both sides; `eyes.both` is applied",
  "//!   to both sides before `eyes.left` / `eyes.right` overrides.",
];

out.push(...moduleDoc);
out.push("");
out.push("use crate::types::{");
out.push(
  "    BodyParams, ExpressionSeed, EyePair, EyePose, EyeRings, Point, Rgb, BODY_POINTS, RING_SETS,",
);
out.push("    SEED_COUNT,");
out.push("};");
out.push("");
out.push("/// Upstream commit recorded by the G0.1 generator.");
out.push(`pub const SOURCE_COMMIT: &str = "${commit}";`);
out.push("");
out.push("/// SHA-256 of the pinned `grok-ball.js`.");
out.push(`pub const SOURCE_SHA256: &str = "${sourceSha256}";`);
out.push("");
out.push("/// Upstream license.");
out.push(`pub const LICENSE: &str = "${LICENSE}";`);
out.push("");

out.push("/// The blob body outline in source coordinates.");
out.push("#[rustfmt::skip]");
out.push("pub static BODY_RING: [Point; BODY_POINTS] = [");
for (let i = 0; i < bodyRing.length; i += 1) {
  out.push(`    ${pointLiteral(bodyRing[i], `SHAPES.blob.ring[${i}]`)}`);
}
out.push("];");
out.push("");

out.push("/// The 25 upstream eye-ring sets.");
out.push("#[rustfmt::skip]");
out.push("pub static RINGS: [EyeRings; RING_SETS] = [");
for (let setIndex = 0; setIndex < expressions.length; setIndex += 1) {
  const set = expressions[setIndex];
  if (!Array.isArray(set) || set.length !== 2) {
    fail(`EB_RINGS.EXPRESSIONS[${setIndex}]: expected [left, right]`);
  }
  out.push("    EyeRings {");
  for (const [sideIndex, sideName] of [
    [0, "left"],
    [1, "right"],
  ]) {
    const ring = set[sideIndex];
    if (!Array.isArray(ring) || ring.length !== RING_POINTS) {
      fail(`EB_RINGS.EXPRESSIONS[${setIndex}].${sideName}: expected ${RING_POINTS} points`);
    }
    out.push(`        ${sideName}: [`);
    for (let i = 0; i < ring.length; i += 1) {
      out.push(
        `            ${pointLiteral(ring[i], `EXPRESSIONS[${setIndex}].${sideName}[${i}]`)}`,
      );
    }
    out.push("        ],");
  }
  out.push("    },");
}
out.push("];");
out.push("");

out.push("/// The 32 upstream expression seeds, normalized to the first-release fields.");
out.push("#[rustfmt::skip]");
out.push("pub static SEEDS: [ExpressionSeed; SEED_COUNT] = [");
for (const seed of seeds) {
  out.push("    ExpressionSeed {");
  out.push(`        id: ${seed.id},`);
  out.push(`        pool: &[${seed.pool.join(", ")}],`);
  out.push(`        pool_min_ms: ${seed.poolMs[0]},`);
  out.push(`        pool_max_ms: ${seed.poolMs[1]},`);
  out.push(`        transition_ms: ${seed.transition},`);
  out.push(`        blink_min_ms: ${seed.blinkMs[0]},`);
  out.push(`        blink_max_ms: ${seed.blinkMs[1]},`);
  out.push(`        openness: ${f32(seed.openness, `seed ${seed.id} openness`)},`);
  out.push("        body: BodyParams {");
  out.push(`            y: ${f32(seed.body.y, `seed ${seed.id} body.y`)},`);
  out.push(`            scale: ${f32(seed.body.scale, `seed ${seed.id} body.scale`)},`);
  out.push(`            rotate: ${f32(seed.body.rotate, `seed ${seed.id} body.rotate`)},`);
  out.push(`            breathe: ${f32(seed.body.breathe, `seed ${seed.id} body.breathe`)},`);
  out.push(`            color: ${seed.body.color},`);
  out.push("        },");
  out.push("        eyes: EyePair {");
  for (const [sideName, pose] of [
    ["left", seed.left],
    ["right", seed.right],
  ]) {
    out.push(`            ${sideName}: EyePose {`);
    out.push(`                open: ${f32(pose.open, `seed ${seed.id} ${sideName}.open`)},`);
    out.push(
      `                scale_x: ${f32(pose.scaleX, `seed ${seed.id} ${sideName}.scaleX`)},`,
    );
    out.push(
      `                scale_y: ${f32(pose.scaleY, `seed ${seed.id} ${sideName}.scaleY`)},`,
    );
    out.push(`                x: ${f32(pose.x, `seed ${seed.id} ${sideName}.x`)},`);
    out.push(`                y: ${f32(pose.y, `seed ${seed.id} ${sideName}.y`)},`);
    out.push(`                look_x: ${f32(pose.lookX, `seed ${seed.id} ${sideName}.lookX`)},`);
    out.push(`                look_y: ${f32(pose.lookY, `seed ${seed.id} ${sideName}.lookY`)},`);
    out.push("            },");
  }
  out.push("        },");
  out.push("    },");
}
out.push("];");
out.push("");
out.push("/// Look up a seed by its upstream id.");
out.push("pub fn seed(id: u8) -> Option<&'static ExpressionSeed> {");
out.push("    SEEDS.iter().find(|seed| seed.id == id)");
out.push("}");
out.push("");
out.push("/// Look up an eye-ring set by its upstream index.");
out.push("pub fn rings(index: u8) -> Option<&'static EyeRings> {");
out.push("    RINGS.get(index as usize)");
out.push("}");

const rust = `${out.join("\n")}\n`;

mkdirSync(dirname(OUT_PATH), { recursive: true });
writeFileSync(OUT_PATH, rust, "utf8");

console.log(`source:        ${SOURCE_PATH}`);
console.log(`data block:    lines ${startIndex + 1}-${endIndex}`);
console.log(`ring sets:     ${expressions.length} (${RING_POINTS} points per side)`);
console.log(`body ring:     ${bodyRing.length} points`);
console.log(`seeds:         ${seeds.length}`);
console.log(`seed ids:      ${seeds.map((seed) => seed.id).join(", ")}`);
console.log(`source commit: ${commit}`);
console.log(`source sha256: ${sourceSha256}`);
console.log(`license:       ${LICENSE}`);
console.log(`default body:  ${DEFAULT_BODY_COLOR}`);
if (ignoredBody.size > 0) {
  console.log(`ignored body fields: ${[...ignoredBody].sort().join(", ")}`);
}
if (ignoredEyes.size > 0) {
  console.log(`ignored eye fields:  ${[...ignoredEyes].sort().join(", ")}`);
}
console.log(`wrote:         ${OUT_PATH} (${rust.split("\n").length - 1} lines)`);
