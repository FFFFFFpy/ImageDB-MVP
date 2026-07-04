import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const repoRoot = process.cwd();
const datasetRoot = join(repoRoot, 'fixtures', 'm9-acceptance');
const expectedPath = join(datasetRoot, 'expected-results.json');
const imageExtensions = new Set(['.jpg', '.jpeg', '.png', '.webp']);

function fail(message) {
  console.error(`[m9-dataset] ${message}`);
  process.exit(1);
}

function walkFiles(root) {
  if (!existsSync(root)) {
    fail(`missing path: ${root}`);
  }
  const out = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) {
      out.push(...walkFiles(path));
    } else if (entry.isFile()) {
      out.push(path);
    }
  }
  return out;
}

function extensionOf(path) {
  const idx = path.lastIndexOf('.');
  return idx >= 0 ? path.slice(idx).toLowerCase() : '';
}

if (!existsSync(expectedPath)) {
  fail(`missing expected results: ${expectedPath}`);
}

const expected = JSON.parse(readFileSync(expectedPath, 'utf8'));
const sourceRoot = join(repoRoot, expected.source_root);
const historyRoot = join(repoRoot, expected.history_library_root);

let checkedFiles = 0;
for (const album of expected.albums) {
  const albumRoot = join(sourceRoot, album.relative_path);
  const files = walkFiles(albumRoot);
  const imageCount = files.filter((file) => imageExtensions.has(extensionOf(file))).length;
  if (imageCount !== album.image_files) {
    fail(`${album.relative_path}: expected ${album.image_files} image files, found ${imageCount}`);
  }
  checkedFiles += files.length;

  for (const sidecar of album.sidecar_files ?? []) {
    const sidecarPath = join(albumRoot, sidecar);
    if (!existsSync(sidecarPath) || !statSync(sidecarPath).isFile()) {
      fail(`${album.relative_path}: missing sidecar ${sidecar}`);
    }
  }
}

const historyFiles = walkFiles(join(historyRoot, expected.history_seed.relative_path));
const historyImageCount = historyFiles.filter((file) =>
  imageExtensions.has(extensionOf(file)),
).length;
if (historyImageCount !== expected.history_seed.image_files) {
  fail(
    `history seed expected ${expected.history_seed.image_files} image files, found ${historyImageCount}`,
  );
}

console.log(
  `[m9-dataset] verified ${expected.albums.length} source albums, ${checkedFiles} source files, ${historyImageCount} history image(s)`,
);
