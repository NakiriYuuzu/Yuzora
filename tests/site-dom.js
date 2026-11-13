// The site's tests read its pages as a browser parses them. bun test has no
// DOM of its own, so each test file imports this before anything else.
import { JSDOM } from "jsdom"

const { window } = new JSDOM("<!doctype html><html><body></body></html>")

globalThis.DOMParser = window.DOMParser
globalThis.document = window.document
