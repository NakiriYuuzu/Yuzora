"""Turns the probe's results.jsonl files into the table for the job summary.

usage: python summarize.py <out dir> [<out dir> ...]
Exits 1 when the probe itself broke (a probe-error result), 0 otherwise: a product
failure is a finding, not a broken run.
"""
import json
import os
import sys

LABEL = {"pass": "通過", "fail": "失敗", "info": "資訊", "not-measured": "沒量到", "probe-error": "探針壞了"}
LEGEND = "通過＝量到更新需要的行為；失敗＝產品做錯了（證據在 artifact）；資訊＝量到的事實，不下判斷；沒量到＝沒能看（原因在說明）；探針壞了＝探針自己的問題，與產品無關。"


def load(directory):
    rows = []
    path = os.path.join(directory, "results.jsonl")
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            for line in f:
                try:
                    rows.append(json.loads(line))
                except ValueError:
                    pass
    return rows


def cell(text, limit=700):
    text = " ".join(str(text).split())
    text = text[: limit - 1] + "…" if len(text) > limit else text
    return text.replace("|", "\\|")


def main():
    lines = []
    broken = 0
    for directory in sys.argv[1:]:
        rows = load(directory)
        counts = {}
        for r in rows:
            counts[r["status"]] = counts.get(r["status"], 0) + 1
        lines.append("### %s" % os.path.basename(os.path.normpath(directory)))
        if not rows:
            lines.append("沒有任何結果（上一個步驟沒跑到探針）。\n")
            continue
        lines.append(" ・ ".join("%s %d" % (LABEL[k], counts[k]) for k in LABEL if k in counts) + "\n")
        lines.append("| 項目 | 結果 | 說明 |\n| --- | --- | --- |")
        for r in rows:
            lines.append("| `%s` | %s | %s |" % (r["id"], LABEL.get(r["status"], r["status"]), cell(r["detail"])))
            if r["status"] == "probe-error":
                broken += 1
                print("::error title=probe broke::%s" % cell(r["id"] + ": " + r["detail"], 200))
            elif r["status"] == "fail":
                print("::warning title=product fails::%s" % cell(r["id"] + ": " + r["detail"], 300))
        lines.append("")
    lines.append(LEGEND)
    text = "\n".join(lines) + "\n"
    print(text)
    target = os.environ.get("GITHUB_STEP_SUMMARY")
    if target:
        with open(target, "a", encoding="utf-8") as f:
            f.write(text)
    sys.exit(1 if broken else 0)


if __name__ == "__main__":
    main()
