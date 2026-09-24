import { Text } from "@codemirror/state"
import { runSearchTask, type SearchTask, type SearchTaskResult } from "./searchTask"

let doc = Text.empty
self.onmessage = (event: MessageEvent<SearchTask>) => {
    const task = event.data
    try {
        if (task.lines) doc = Text.of(task.lines)
        self.postMessage(runSearchTask(doc, task))
    } catch {
        self.postMessage({ id: task.id, ranges: [], error: "failed" } satisfies SearchTaskResult)
    }
}
