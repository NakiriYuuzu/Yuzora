#!/usr/bin/env bash
set -euo pipefail
sandbox() { bash scripts/sandbox "$@"; }
case ${1:?setup, play or crop} in
  crop) echo '1040:420:120:60' ;;
  setup)
    sandbox do overlay.close '{"id":"frame-welcome"}'
    sandbox do space.new '{"name":"website","tab":"pricing page","argv":["bash","--noprofile","--norc","-c","printf \"\\033[2J\\033[H\\033[1;36mClaude Code\\033[0m\\n\\nUpdated the pricing cards and mobile navigation.\\n\\nThe test run is ready in t2\\n\\nAll 12 checks passed. Take a look before we ship.\\n\"; printf '\''\\033]7501;state=done:app=claude-code\\033\\\\'\''; exec sleep infinity"]}'
    sandbox do tab.new '{"name":"test run","argv":["bash","--noprofile","--norc","-c","printf \"\\033[2J\\033[H\\033[1;32mwebsite / tests\\033[0m\\n\\n$ bun test\\n\\n  PASS  src/App.test.tsx\\n  PASS  src/components/Nav.test.tsx\\n  PASS  src/routes/pricing.test.tsx\\n\\n  12 tests passed\\n  0 failures\\n\\nDone in 340 ms.\\n\"; exec sleep infinity"]}'
    sandbox do space.new '{"name":"service-api","tab":"request tracing","argv":["sh","-c","printf '\''\\033]7501;state=working:app=codex\\033\\\\'\''; exec sleep 100000"]}'
    sandbox do space.select '{"index":1}'
    sandbox do tab.select '{"index":1}'
    sandbox click 1006 82
    sandbox move 850 600 850 600
    sleep 8
    ;;
  play)
    sandbox sequence '[["sleep",0.05],["move",850,600,614,240,90],["hold","Control_L"],["sleep",0.35],["click",614,240],["release","Control_L"],["sleep",0.15],["move",614,240,1060,102,90],["click",1060,102],["sleep",0.15]]'
    ;;
esac
