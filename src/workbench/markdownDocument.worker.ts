import { renderMarkdownDocument } from "./markdownDocumentRender"
self.onmessage = (event: MessageEvent<string>) => { self.postMessage(renderMarkdownDocument(event.data)) }
