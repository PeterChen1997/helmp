#!/bin/bash

# 示例：compact 模式只展示 stdout 的最后一行。
#
# Required parameters:
# @raycast.schemaVersion 1
# @raycast.title 当前时间
# @raycast.mode compact
#
# Optional parameters:
# @raycast.packageName 示例
# @raycast.icon 🕐
# @raycast.description 显示当前日期与时间
# @raycast.author helmp

date "+%Y-%m-%d %H:%M:%S %Z"
