#!/usr/bin/env bash
set -euo pipefail
sandbox() { bash scripts/sandbox "$@"; }
case ${1:?setup, play or crop} in
  crop) echo '940:370:330:60' ;;
  setup)
    sandbox do overlay.close '{"id":"frame-welcome"}'
    sandbox do space.new '{"name":"website","tab":"dev server","argv":["bash","--noprofile","--norc","-c","printf \"\\033[2J\\033[H\\033[1;36mwebsite / frontend\\033[0m\\n\\n$ bun run dev\\n\\n  VITE v6.1.0  ready in 180 ms\\n\\n  Local: http://localhost:5173\\n\\n  14:32:08  page reload src/App.tsx\\n  14:32:11  hmr update src/styles.css\\n\"; exec sleep infinity"]}'
    sandbox do pane.split '{"direction":"right","argv":["bash","--noprofile","--norc","-c","printf \"\\033[2J\\033[H\\033[1;32mwebsite / tests\\033[0m\\n\\n$ bun test --watch\\n\\n  PASS  src/App.test.tsx\\n  PASS  src/components/Nav.test.tsx\\n  PASS  src/routes/pricing.test.tsx\\n\\n  12 tests passed\\n  Watching for changes...\\n\"; exec sleep infinity"]}'
    sandbox do tab.new '{"name":"pricing page","argv":["sh","-c","printf '\''\\033]7501;state=working:app=claude-code\\033\\\\'\''; exec sleep 100000"]}'
    sandbox do space.new '{"name":"service-api","tab":"request tracing","argv":["sh","-c","printf '\''\\033]7501;state=blocked:app=codex\\033\\\\'\''; exec sleep 100000"]}'
    sandbox do space.select '{"index":1}'
    sandbox do tab.select '{"index":1}'
    sandbox click 1006 82
    sandbox move 850 600 850 600
    sleep 8
    ;;
  play)
    sandbox sequence '[["sleep",0.15],["move",850,600,560,76,60],["sleep",0.3],["press"],["sleep",0.1],["move",560,76,1240,420,120],["sleep",0.7],["lift"],["sleep",0.2]]'
    ;;
esac
