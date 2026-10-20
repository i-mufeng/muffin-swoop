#!/usr/bin/env bash
# 一键发布 swoop.muffin.xin：构建 → 上传到服务器
#
#   website/deploy.sh          # 构建并上传
#   website/deploy.sh --build  # 只构建，不上传
#
# 服务器约定：ssh root@ms.imufeng.cn -p 15990
# 站点目录：/data/web-service/sites/swoop.muffin.xin/index/
set -euo pipefail

cd "$(dirname "$0")"

SSH_HOST=root@ms.imufeng.cn
SSH_PORT=15990
REMOTE_DIR=/data/web-service/sites/swoop.muffin.xin/index

echo "▸ 构建中…"
npm run build

if [[ "${1:-}" == "--build" ]]; then
  echo "✓ 构建完成（--build，跳过上传）：dist/"
  exit 0
fi

echo "▸ 上传到 ${SSH_HOST}:${REMOTE_DIR} …"
scp -P "$SSH_PORT" -o BatchMode=yes -r dist/* "${SSH_HOST}:${REMOTE_DIR}/"

echo "▸ 验证线上状态…"
code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 15 https://swoop.muffin.xin/)
echo "✓ 发布完成：https://swoop.muffin.xin/ （HTTP ${code}）"
