#!/bin/sh
# A fixed reminder for the date set in plugin.json. Change the text to suit.
printf '{"notify": {"title": "%s", "body": "%s"}}\n' "A reminder for today" "Take a moment for the people you care about."
