# Milestone 0 - Technical Probe Report

## Date

2026-07-03

## Environment

- OS: Windows 11 (10.0.26200, x64)
- Node.js: v24.16.0
- Rust: 1.96.1 (stable)
- pnpm: 11.7.0
- PostgreSQL: NOT INSTALLED on this machine

## Summary

This milestone is a technical probe prototype. It validates that the chosen tech stack (React + Tauri + Rust + PostgreSQL + pgvector) can work together in a desktop application. The probe covers three infrastructure modules: PostgreSQL lifecycle management, image fingerprinting, and file transaction safety.

IMPORTANT: PostgreSQL and pgvector were NOT runtime-verified on this machine because PostgreSQL binaries are not installed. The database module passed code review and unit-level checks only. Full database acceptance criteria (auto-init on first launch, data reuse on second launch, pgvector health check) remain unverified until a PostgreSQL binary is available.

## Verification Commands

| Command                  | Result                                                              |
| ------------------------ | ------------------------------------------------------------------- |
| pnpm install             | PASS                                                                |
| pnpm typecheck           | PASS                                                                |
| pnpm test:unit           | PASS (4 tests)                                                      |
| pnpm rust:test           | PASS (17 tests)                                                     |
| pnpm rust:clippy         | PASS (zero warnings)                                                |
| pnpm build               | PASS                                                                |
| release exe smoke launch | PASS (imagedb-desktop.exe started and stayed running for 5 seconds) |

## 1. React + Tauri Minimal Application

Status: PASS

- Tauri 2 + React 19 + Vite 6 application builds and runs
- IPC verified via invoke: get_app_status returns connected status
- 4 frontend unit tests pass
- Production build produces dist/index.html + imagedb-desktop.exe
- Release executable smoke launch passed: apps/desktop/src-tauri/target/release/imagedb-desktop.exe started and stayed running for 5 seconds before manual termination

## 2. PostgreSQL Lifecycle Module Prototype

Status: CODE IMPLEMENTED, NOT RUNTIME-VERIFIED (no PostgreSQL binary on this machine)

Implemented features:

- Binary search (app directory, system PATH, common install paths)
- Isolated data directory (<app_data>/ImageDB/postgres_data)
- Port persistence via port file (postgres_port in data directory)
- Random local port allocation via TcpListener bind to 127.0.0.1:0
- initdb initialization, pg_ctl start/stop lifecycle management
- TCP-only connection (-h 127.0.0.1), no Unix socket dependency
- CREATE DATABASE imagedb
- CREATE EXTENSION IF NOT EXISTS vector (pgvector)
- tokio-postgres connection test
- Missing binary diagnostic: returns available: false with search path details

Unit tests (no PostgreSQL required):

- Missing binary diagnostic message
- Port persistence round-trip (save and read)
- Invalid port file content returns None
- Initialize without binaries returns unavailable result

Diagnostic output:

- PostgreSQL binaries (pg_ctl, initdb, psql) not found on this machine
- Probe correctly returns available: false with diagnostic log listing searched paths

Known limitations:

- PostgreSQL binaries must be installed or bundled for runtime verification
- pgvector requires separate installation into the PostgreSQL extension directory
- First-launch auto-init and second-launch data reuse NOT runtime-tested

## 3. Image Fingerprint Module Prototype

Status: PASS

Implemented features:

- Decode JPEG, PNG, WebP samples (via image 0.25 crate)
- BLAKE3: computed on raw file bytes, returns 64-char hex
- Pixel Hash: RGBA normalization (orientation, alpha normalization, fixed pixel layout) + versioned prefix + BLAKE3 first 16 chars
- Gradient Hash: 8x8 Lanczos3 grayscale resize, horizontal gradient comparison, 16-char hex
- Block Hash: original-size 8x8 grid block mean comparison, 16-char hex
- Median Hash: 8x8 Lanczos3 grayscale resize, median comparison, 16-char hex
- fingerprint_version field exposed in DTO (value: 1)

Unit tests (all pass):

- BLAKE3 determinism
- Pixel Hash determinism
- Gradient Hash determinism
- Block Hash determinism
- Median Hash determinism
- Different spatial patterns produce different hashes
- Generate samples and fingerprint (PNG + JPEG + WebP)
- Full probe flow (directory scan, fingerprint, report)

Generated samples:

- fixtures/test-sample.png (PNG, 64x64)
- fixtures/test-sample.jpg (JPEG, 64x64)
- fixtures/test-sample.webp (WebP, 64x64)

## 4. File Transaction Module Prototype

Status: PASS

Implemented features:

- State machine: Ready -> Staging -> Verifying -> Verified -> Publishing -> Published
- Copy to staging (<library>/.imagedb/staging/<tx_id>/)
- Per-file BLAKE3 verification (staging vs source)
- Publish to final directory (<library>/Albums/<tx_id>/)
- Post-publish BLAKE3 verification
- JSON manifest written only after all verifications pass (<library>/.imagedb/manifests/<tx_id>.json)
- Staging cleanup after success
- Interrupt safety: any step failure returns Failed state and cleans up, no false success

Unit tests (all pass):

- Normal flow: 2 files published, BLAKE3 verified, state = PUBLISHED
- Empty source directory: correctly returns Failed state
- Staging cleanup: empty after successful transaction
- Fault injection - staging failure: state is NOT Published, no manifest, no published files
- Fault injection - pre-publish failure: state is NOT Published, no manifest, no published directories

## 5. GUI Probe Display

Status: PASS

- Three tabs: Database, Image Fingerprint, File Transaction
- Run All Probes button executes all probes
- Each probe has independent run button
- Results displayed in table format
- Fingerprint version field shown in fingerprint card
- Expandable diagnostic logs
- Card border-radius within 8px limit
- Text overflow protection for narrow screens

## Modified Files

### New files

- apps/desktop/src-tauri/src/domain/mod.rs - domain types (TransactionState)
- apps/desktop/src-tauri/src/infrastructure/mod.rs - infrastructure module entry
- apps/desktop/src-tauri/src/infrastructure/postgres.rs - PostgreSQL lifecycle management
- apps/desktop/src-tauri/src/infrastructure/image_fingerprint.rs - image decoding and fingerprint
- apps/desktop/src-tauri/src/infrastructure/file_transaction.rs - file transaction probe
- apps/desktop/src-tauri/src/commands/probe.rs - probe Tauri commands
- fixtures/test-sample.png - PNG test sample
- fixtures/test-sample.jpg - JPEG test sample
- fixtures/test-sample.webp - WebP test sample

### Modified files

- pnpm-workspace.yaml - allow esbuild build script and pin minimumReleaseAge exclusions for current lockfile entries
- package.json - removed invalid pnpm section
- apps/desktop/src-tauri/Cargo.toml - added image, blake3, tokio-postgres, dirs, uuid
- apps/desktop/src-tauri/src/lib.rs - registered modules and commands
- apps/desktop/src-tauri/src/error.rs - extended error types
- apps/desktop/src-tauri/src/state.rs - AppState holds PostgresManager
- apps/desktop/src-tauri/src/commands/mod.rs - export probe commands
- apps/desktop/src/app/App.tsx - probe GUI (three tabs + results)
- apps/desktop/src/app/App.test.tsx - updated tests (4 items)
- apps/desktop/src/styles/global.css - extended styles (tabs, tables, diagnostics)

## Remaining Risks

1. PostgreSQL NOT runtime-verified: No PostgreSQL binary on this machine. Lifecycle code verified only through static analysis and unit tests. Real database acceptance (auto-init, data reuse, pgvector) requires a PostgreSQL-equipped environment.
2. pgvector NOT verified: Same as above.
3. WebP sample is small: Generated WebP test sample is minimal; richer samples may be needed for fingerprint stability validation.
4. macOS build: Only verified on Windows; macOS build needs separate testing.
