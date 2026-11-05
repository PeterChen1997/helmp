#!/bin/bash

# 示例：fullOutput 模式展示完整输出。只读本机网卡，不访问网络。
#
# Required parameters:
# @raycast.schemaVersion 1
# @raycast.title 本机 IP
# @raycast.mode fullOutput
#
# Optional parameters:
# @raycast.packageName 示例
# @raycast.icon 🌐
# @raycast.description 列出本机各网卡的 IPv4 地址
# @raycast.author huan

set -euo pipefail

addresses=$(/sbin/ifconfig | awk '
  /^[^ \t]/ { iface = $1; sub(":$", "", iface) }
  $1 == "inet" && $2 != "127.0.0.1" { print iface "\t" $2 }
')

if [[ -z "$addresses" ]]; then
  echo "没有可用的 IPv4 地址"
else
  echo "$addresses"
fi
