#!/bin/bash

# 示例：silent 模式跑完不打扰，结果直接进剪贴板。
#
# Required parameters:
# @raycast.schemaVersion 1
# @raycast.title 生成 UUID
# @raycast.mode silent
#
# Optional parameters:
# @raycast.packageName 示例
# @raycast.icon 🆔
# @raycast.description 生成随机 UUID 并复制到剪贴板
# @raycast.author huan

set -euo pipefail

uuid=$(uuidgen | tr '[:upper:]' '[:lower:]')
printf '%s' "$uuid" | pbcopy
echo "已复制 $uuid"
