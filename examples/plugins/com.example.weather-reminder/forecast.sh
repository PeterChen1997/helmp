#!/bin/sh
# Fetch one line of text from the forecast URL (a plugin preference) and ask Helmp to post it as a notification.
# stdout is only the notification request; anything for debugging goes to stderr.
set -eu

# Escape a value for a JSON string: backslashes, quotes, and line breaks become spaces.
json() { printf '%s' "$1" | tr '\n\r\t' '   ' | sed 's/\\/\\\\/g; s/"/\\"/g'; }

if [ -z "${FORECAST_URL:-}" ]; then
  echo "no forecast URL set" >&2
  exit 1
fi
text=$(curl -fsS --max-time 15 "$FORECAST_URL" | head -c 400)
echo "fetched ${#text} bytes" >&2
printf '{"notify": {"title": "%s", "body": "%s", "url": "%s"}}\n' \
  "Today's weather" "$(json "$text")" "$(json "$FORECAST_URL")"
