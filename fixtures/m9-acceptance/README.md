# M9 Acceptance Dataset

This fixture set is the fixed release-closure dataset for M9 GUI/IPC and release gate runs.

Use `source/` as the import source root. Each first-level child directory is one source album.

`history-library/` is a seed library root layout for tests that need a pre-existing historical image before importing `source/`.

Expected outcomes are recorded in `expected-results.json`.

The dataset intentionally includes:

- exact duplicate files in one album;
- the same sample image encoded as PNG, JPEG, and WebP;
- Unicode path components and sidecar files;
- a corrupt image neighbor;
- an empty album;
- cross-album duplicate files;
- a small many-image smoke album;
- one historical-library seed image.
