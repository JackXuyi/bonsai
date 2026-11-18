// Generate app/tray icons from Lucide (https://lucide.dev/icons/).
//
// App icon:  tree-deciduous (trunk + foliage cloud — echoes the face SVG).
// Tray icon: sprout (the "slowly learns you" growth motif).
//
// Lucide ships SVGs; Tauri needs PNG, so we render with resvg-js:
//   pnpm icons
//
// Re-render after tuning colors/sizes only.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const outDir = resolve(root, "src-tauri/icons");

// bonsai green, matches ui/styles.css --leaf
const STROKE = "#4c8c2b";

function loadLucide(name) {
  const path = resolve(root, `node_modules/lucide-static/icons/${name}.svg`);
  let svg = readFileSync(path, "utf8");
  // Lucide uses stroke="currentColor" — bake in the brand color.
  svg = svg.replaceAll('stroke="currentColor"', `stroke="${STROKE}"`);
  return svg;
}

// Add breathing room around the 24x24 viewBox so the glyph doesn't sit
// edge-to-edge (matters most at tray size).
function withPadding(svg, margin) {
  return svg.replace(
    /viewBox="0 0 24 24"/,
    `viewBox="${-margin} ${-margin} ${24 + 2 * margin} ${24 + 2 * margin}"`
  );
}

function renderPng(svg, size) {
  const resvg = new Resvg(svg, { fitTo: { mode: "width", value: size } });
  return resvg.render().asPng();
}

function writeIcon(file, svg, size) {
  const out = resolve(outDir, file);
  writeFileSync(out, renderPng(svg, size));
  console.log(`wrote ${out} (${size}x${size})`);
}

mkdirSync(outDir, { recursive: true });

// App icon: more margin so it reads as an icon, not a poster.
writeIcon("icon.png", withPadding(loadLucide("tree-deciduous"), 3.5), 128);

// Tray icon: macOS renders it at 16-18pt; keep the glyph dominant.
writeIcon("tray.png", withPadding(loadLucide("sprout"), 2), 32);
