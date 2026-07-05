#!/bin/bash
# cap.sh <output-filename-without-ext>
# 對 yuzora 前景視窗做真實色彩、精準裁切的證據截圖。
# osascript 每次即時查視窗 frame（points），screencapture -R 吃 points、依 Retina 比例輸出像素。
set -euo pipefail

NAME="${1:?usage: cap.sh <output-filename-without-ext>}"
DIR="${EVIDENCE_DIR:-./evidence}"
mkdir -p "$DIR"

FRAME=$(osascript -e '
tell application "System Events"
    tell process "yuzora"
        set winPos to position of front window
        set winSize to size of front window
        set x to item 1 of winPos
        set y to item 2 of winPos
        set w to item 1 of winSize
        set h to item 2 of winSize
        return (x as string) & "," & (y as string) & "," & (w as string) & "," & (h as string)
    end tell
end tell')

screencapture -x -R"$FRAME" "$DIR/$NAME.png"
echo "$DIR/$NAME.png"
