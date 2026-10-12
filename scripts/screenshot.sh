#!/usr/bin/env bash
# 开发用：打开指定界面（含弹窗、吐司），截下主窗口存成 PNG。实现见 crates/app/src/shot.rs。
#
#   scripts/screenshot.sh [视图] [dark|light] [输出.png]
#   scripts/screenshot.sh settings:download light
#   SWOOP_DEMO=0 scripts/screenshot.sh home        # 不混入演示任务（看空状态）
#
# 视图：home settings settings:{general,download,notify,integration,about} queues categories network
#       quota completion site-auth updates media grabber add task-window completed
# 默认输出 target/screenshots/<视图>-<主题>.png。用临时数据目录，不碰真实下载记录，也不会被已运行的
# Swoop 实例接管；截图期间会短暂弹出一个窗口。
set -euo pipefail

view=${1:-home}
theme=${2:-dark}
out=${3:-target/screenshots/${view//:/-}-$theme.png}

cd "$(dirname "$0")/.."
data=$(mktemp -d)
trap 'rm -rf "$data"' EXIT

if [[ "${SWOOP_DEMO-1}" == "0" ]]; then
    unset SWOOP_DEMO
else
    export SWOOP_DEMO=1
fi

SWOOP_DATA_DIR="$data" SWOOP_THEME="$theme" SWOOP_SHOT="$out" SWOOP_SHOT_VIEW="$view" \
    cargo run --quiet --locked -p swoop-app --bin swoop --features screenshot
echo "$out"
