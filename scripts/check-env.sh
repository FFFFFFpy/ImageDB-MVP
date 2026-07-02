#!/usr/bin/env bash
set -u

BUILD_PROBE=0
if [[ "${1:-}" == "--build" ]]; then
  BUILD_PROBE=1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPORTS_DIR="$PROJECT_ROOT/reports"
mkdir -p "$REPORTS_DIR"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
OS_NAME="$(uname -s)"
REPORT_PATH="$REPORTS_DIR/environment-${OS_NAME,,}-$TIMESTAMP.txt"

PASS_COUNT=0
WARN_COUNT=0
FAIL_COUNT=0

line() {
  printf '%s\n' "$*" | tee -a "$REPORT_PATH"
}

check() {
  local level="$1" name="$2" detail="$3"
  case "$level" in
    PASS) PASS_COUNT=$((PASS_COUNT + 1));;
    WARN) WARN_COUNT=$((WARN_COUNT + 1));;
    FAIL) FAIL_COUNT=$((FAIL_COUNT + 1));;
  esac
  line "[$level] $name - $detail"
}

has_cmd() {
  command -v "$1" >/dev/null 2>&1
}

major_version() {
  printf '%s' "$1" | sed -E 's/[^0-9]*([0-9]+).*/\1/'
}

run_probe() {
  local name="$1"
  shift
  line ""
  line "--- $name ---"
  set +e
  "$@" 2>&1 | tee -a "$REPORT_PATH"
  local code=${PIPESTATUS[0]}
  set -e
  if [[ $code -eq 0 ]]; then
    check PASS "$name" "命令执行成功"
  else
    check FAIL "$name" "退出码 $code"
  fi
}

line "ImageDB-MVP 环境检查"
line "项目目录: $PROJECT_ROOT"
line "检查时间: $(date '+%Y-%m-%d %H:%M:%S %z')"
line ""

check INFO "系统" "$(uname -a)"

required_files=(
  package.json
  pnpm-workspace.yaml
  apps/desktop/package.json
  apps/desktop/src-tauri/Cargo.toml
  apps/desktop/src-tauri/tauri.conf.json
  AGENTS.md
  CURRENT_TASK.md
)
missing=()
for file in "${required_files[@]}"; do
  [[ -f "$PROJECT_ROOT/$file" ]] || missing+=("$file")
done
if [[ ${#missing[@]} -eq 0 ]]; then
  check PASS "项目结构" "关键文件齐全"
else
  check FAIL "项目结构" "缺失: ${missing[*]}"
fi

if has_cmd codex; then
  check PASS "Codex CLI" "$(codex --version 2>/dev/null | head -n 1)"
else
  check FAIL "Codex CLI" "未找到 codex 命令"
fi

if has_cmd git; then
  check PASS "Git" "$(git --version)"
  git_name="$(git config --global --get user.name 2>/dev/null || true)"
  git_email="$(git config --global --get user.email 2>/dev/null || true)"
  if [[ -n "$git_name" && -n "$git_email" ]]; then
    check PASS "Git 提交身份" "$git_name <$git_email>"
  else
    check WARN "Git 提交身份" "未完整配置 user.name / user.email"
  fi
else
  check FAIL "Git" "未找到 git"
fi

if has_cmd node; then
  node_version="$(node --version)"
  node_major="$(major_version "$node_version")"
  if [[ "$node_major" -ge 22 && "$node_major" -lt 25 ]]; then
    check PASS "Node.js" "$node_version（支持范围 22/24）"
  elif [[ "$node_major" -ge 22 ]]; then
    check WARN "Node.js" "$node_version（建议使用 Node.js 24 LTS）"
  else
    check FAIL "Node.js" "$node_version（需要 Node.js 22 或 24）"
  fi
else
  check FAIL "Node.js" "未找到 node"
fi

if has_cmd npm; then
  check PASS "npm" "$(npm --version)"
else
  check WARN "npm" "未找到 npm；安装 pnpm 时可能需要"
fi

if has_cmd pnpm; then
  pnpm_version="$(pnpm --version)"
  pnpm_major="$(major_version "$pnpm_version")"
  if [[ "$pnpm_major" -eq 10 ]]; then
    check PASS "pnpm" "$pnpm_version（项目使用 pnpm 10）"
  else
    check FAIL "pnpm" "$pnpm_version（项目要求 pnpm 10）"
  fi
else
  check FAIL "pnpm" "未找到 pnpm 10；可执行: npm install -g pnpm@10"
fi

if has_cmd rustup; then
  check PASS "rustup" "$(rustup --version | head -n 1)"
else
  check FAIL "rustup" "未找到 rustup"
fi

if has_cmd rustc; then
  check PASS "rustc" "$(rustc --version)"
else
  check FAIL "rustc" "未找到 rustc"
fi

if has_cmd cargo; then
  check PASS "cargo" "$(cargo --version)"
else
  check FAIL "cargo" "未找到 cargo"
fi

if has_cmd rustup; then
  toolchain="$(rustup show active-toolchain 2>/dev/null || true)"
  if [[ "$toolchain" == *stable* ]]; then
    check PASS "Rust 工具链" "$toolchain"
  else
    check FAIL "Rust 工具链" "${toolchain:-未检测到 stable 工具链}"
  fi
fi

if has_cmd cargo && cargo fmt --version >/dev/null 2>&1; then
  check PASS "rustfmt" "$(cargo fmt --version)"
else
  check FAIL "rustfmt" "缺失；执行: rustup component add rustfmt"
fi

if has_cmd cargo && cargo clippy --version >/dev/null 2>&1; then
  check PASS "clippy" "$(cargo clippy --version)"
else
  check FAIL "clippy" "缺失；执行: rustup component add clippy"
fi

if [[ "$OS_NAME" == "Darwin" ]]; then
  if xcode-select -p >/dev/null 2>&1; then
    check PASS "Xcode Command Line Tools" "$(xcode-select -p)"
  else
    check FAIL "Xcode Command Line Tools" "缺失；执行: xcode-select --install"
  fi

  if has_cmd clang; then
    check PASS "Clang" "$(clang --version | head -n 1)"
  else
    check FAIL "Clang" "未找到 clang"
  fi
elif [[ "$OS_NAME" == "Linux" ]]; then
  for cmd in pkg-config cc; do
    if has_cmd "$cmd"; then
      check PASS "$cmd" "$(command -v "$cmd")"
    else
      check FAIL "$cmd" "未找到 $cmd"
    fi
  done
  check WARN "Linux 系统库" "请按 Tauri 对应发行版要求安装 WebKitGTK 等系统依赖"
else
  check WARN "操作系统" "此脚本主要用于 macOS/Linux；Windows 请运行 check-env.ps1"
fi

free_gb="$(df -Pk "$PROJECT_ROOT" | awk 'NR==2 {printf "%.1f", $4/1024/1024}')"
free_int="${free_gb%.*}"
if [[ "$free_int" -ge 10 ]]; then
  check PASS "可用磁盘空间" "$free_gb GB"
elif [[ "$free_int" -ge 5 ]]; then
  check WARN "可用磁盘空间" "$free_gb GB；建议至少 10 GB"
else
  check FAIL "可用磁盘空间" "$free_gb GB；不足 5 GB"
fi

if has_cmd postgres; then
  check INFO "系统 PostgreSQL" "$(postgres --version)"
else
  check INFO "系统 PostgreSQL" "未安装或不在 PATH；不阻塞技术探针"
fi
if has_cmd psql; then check INFO "psql" "$(psql --version)"; fi
if has_cmd pg_config; then check INFO "pg_config" "$(pg_config --version)"; fi

if [[ $BUILD_PROBE -eq 1 && $FAIL_COUNT -eq 0 ]]; then
  cd "$PROJECT_ROOT"
  set -e
  run_probe "pnpm install" pnpm install
  run_probe "TypeScript typecheck" pnpm typecheck
  run_probe "Frontend unit tests" pnpm test:unit
  run_probe "Rust tests" pnpm rust:test
  run_probe "Rust clippy" pnpm rust:clippy
  run_probe "Tauri build" pnpm build
elif [[ $BUILD_PROBE -eq 1 ]]; then
  check WARN "深度构建验证" "存在基础环境失败项，已跳过"
fi

line ""
line "汇总: PASS=$PASS_COUNT WARN=$WARN_COUNT FAIL=$FAIL_COUNT"
line "报告: $REPORT_PATH"

if [[ $FAIL_COUNT -gt 0 ]]; then
  exit 1
fi
exit 0
