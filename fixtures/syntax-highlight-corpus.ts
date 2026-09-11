export type SyntaxRole = "keyword" | "string" | "comment" | "number" | "function" | "type" | "property" | "tag" | "heading" | "link" | "variable" | "meta"
export type RenderedRole = SyntaxRole | "definition" | "plain"
export interface SyntaxProbe { text: string; role: SyntaxRole; renderedAs?: RenderedRole }
export interface SyntaxSample {
    id: string
    name: string
    files: string[]
    grammar: "structured" | "legacy"
    code: string
    probes: SyntaxProbe[]
    note?: string
    limitations?: { scope: string; status: "partial"; detail: string }[]
}

const probes = (values: Partial<Record<SyntaxRole, string>>, classifications: Partial<Record<SyntaxRole, RenderedRole>> = {}): SyntaxProbe[] => Object.entries(values).map(([role, text]) => ({ role: role as SyntaxRole, text, renderedAs: classifications[role as SyntaxRole] }))

/** Shared by the real EditorPane acceptance page and parser/highlight regression tests. */
export const syntaxSamples: SyntaxSample[] = [
    { id: "javascript", name: "JavaScript", files: ["sample.js", "sample.mjs", "sample.cjs"], grammar: "structured", code: '// Comment sample\nclass Greeter {}\nfunction greet(name) {\n  const message = "hello";\n  return message + name + 42;\n}\ngreet("world");\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }) },
    { id: "typescript", name: "TypeScript", files: ["sample.ts", "sample.mts", "sample.cts"], grammar: "structured", code: '// Comment sample\ninterface Greeter { name: string }\nfunction greet(person: Greeter): string {\n  const count: number = 42;\n  return "hello" + person.name + count;\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }) },
    { id: "jsx", name: "JSX", files: ["sample.jsx"], grammar: "structured", code: '// Comment sample\nfunction Greeting() {\n  const count = 42;\n  return <section title="hello">{count}</section>;\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "Greeting", tag: "section" }) },
    { id: "tsx", name: "TSX", files: ["sample.tsx"], grammar: "structured", code: '// Comment sample\ninterface Props { count: number }\nfunction Greeting(props: Props) {\n  const count: number = 42;\n  return <section title="hello">{props.count + count}</section>;\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "Greeting", type: "Props", tag: "section" }) },
    { id: "html", name: "HTML", files: ["sample.html", "sample.htm"], grammar: "structured", code: '<!-- Comment sample -->\n<section title="hello">Content</section>\n<script>function greet() { return 42; }</script>\n<style>.card { color: red; }</style>\n', probes: probes({ comment: "<!-- Comment sample -->", tag: "section", property: "title", string: '"hello"', keyword: "return", number: "42", function: "greet" }), note: "標籤與屬性；script/style 內分別使用 JavaScript/CSS。" },
    { id: "css", name: "CSS", files: ["sample.css"], grammar: "structured", code: '/* Comment sample */\n@media screen {\n  .card { color: red; width: 42px; content: "hello"; transform: rotate(30deg); }\n}\n', probes: probes({ comment: "/* Comment sample */", keyword: "@media", property: "color", string: '"hello"', number: "42", function: "rotate" }), note: "依 CSS 規則驗證屬性、函式與單位數值，不套用程式語言型別。" },
    { id: "scss", name: "SCSS", files: ["sample.scss"], grammar: "structured", code: '// Comment sample\n$gap: 42px;\n@mixin greeting($name) { content: "hello"; padding: $gap; }\n.card { @include greeting("world"); color: red; }\n', probes: probes({ comment: "// Comment sample", keyword: "@mixin", string: '"hello"', number: "42", property: "padding", function: "greeting" }) },
    { id: "json", name: "JSON", files: ["sample.json"], grammar: "structured", code: '{\n  "message": "hello",\n  "count": 42,\n  "enabled": true,\n  "items": [1, 2, null]\n}\n', probes: probes({ property: '"message"', string: '"hello"', number: "42" }), note: "JSON 不含註解、函式、型別或程式關鍵字；驗證鍵、字串、數值與常值。" },
    { id: "markdown", name: "Markdown", files: ["sample.md", "sample.markdown"], grammar: "structured", code: '# Heading sample\n\nA **strong** word and *emphasis*.\n\n[Reference](https://example.com)\n\n<!-- Comment sample -->\n', probes: probes({ heading: "Heading sample", link: "https://example.com", comment: "<!-- Comment sample -->" }), note: "Markdown 驗證標題、連結、HTML 註解、粗體與斜體。" },
    { id: "python", name: "Python", files: ["sample.py", "sample.pyi", "sample.pyw"], grammar: "structured", code: '# Comment sample\nclass Greeter:\n    def greet(self, name: str):\n        count = 42\n        return "hello" + name\n', probes: probes({ comment: "# Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }) },
    { id: "java", name: "Java", files: ["Sample.java"], grammar: "structured", code: '// Comment sample\nclass Greeter {\n  String greet() {\n    int count = 42;\n    return "hello";\n  }\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }, { function: "definition", type: "definition" }), note: "方法與類別宣告仍為一般定義；方法呼叫與型別參照有獨立分類。" },
    { id: "c", name: "C", files: ["sample.c", "sample.h"], grammar: "structured", code: '// Comment sample\nstruct Greeter { int count; };\nconst char *greet(void) {\n  int count = 42;\n  return "hello";\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }) },
    { id: "cpp", name: "C++", files: ["sample.cpp", "sample.hpp", "sample.cc", "sample.cxx", "sample.hh", "sample.hxx"], grammar: "structured", code: '// Comment sample\nclass Greeter {\npublic:\n  const char *greet() {\n    int count = 42;\n    return "hello";\n  }\n};\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }) },
    { id: "csharp", name: "C#", files: ["Sample.cs", "sample.csx"], grammar: "legacy", code: '// Comment sample\nclass Greeter {\n  string SayHello() {\n    int count = 42;\n    return "hello";\n  }\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "SayHello", type: "string" }, { function: "variable" }), note: "基本關鍵字、字串、註解、數值與內建型別有分類；函式名稱仍為一般識別字，自訂類別為一般定義。" },
    { id: "go", name: "Go", files: ["sample.go"], grammar: "structured", code: '// Comment sample\npackage main\ntype Greeter struct { Count int }\nfunc greet() string {\n  count := 42\n  return "hello"\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }) },
    { id: "rust", name: "Rust", files: ["sample.rs"], grammar: "structured", code: '// Comment sample\nstruct Greeter { count: i32 }\nfn greet() -> String {\n  let count = 42;\n  return String::from("hello");\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }) },
    { id: "php", name: "PHP", files: ["sample.php"], grammar: "structured", code: '<?php\n// Comment sample\nclass Greeter {}\nfunction greet(): string {\n  $count = 42;\n  return "hello";\n}\n?>\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }) },
    { id: "ruby", name: "Ruby", files: ["sample.rb"], grammar: "legacy", code: '# Comment sample\nclass Greeter\n  def greet(name)\n    count = 42\n    return "hello" + name\n  end\nend\n', probes: probes({ comment: "# Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter" }, { function: "definition", type: "tag" }), note: "基本 token 可用；函式為一般定義，自訂類別沿用 legacy 標籤分類，未提供完整函式／型別語義。" },
    { id: "swift", name: "Swift", files: ["sample.swift"], grammar: "legacy", code: '// Comment sample\nclass Greeter {}\nfunc greet(name: String) -> String {\n  let count = 42\n  return "hello" + name\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "String" }, { function: "definition" }), note: "基本 token 與內建型別可用；函式和自訂類別仍為一般定義。" },
    { id: "kotlin", name: "Kotlin", files: ["sample.kt", "sample.kts"], grammar: "legacy", code: '// Comment sample\nclass Greeter\nfun greet(name: String): String {\n  val count = 42\n  return "hello" + name\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "String" }, { function: "definition" }), note: "基本 token 與內建型別可用；函式和自訂類別仍為一般定義。" },
    { id: "dart", name: "Dart", files: ["sample.dart"], grammar: "legacy", code: '// Comment sample\nclass Greeter {}\nString greet(String name) {\n  var count = 42;\n  return "hello" + name;\n}\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "String" }, { function: "variable", type: "variable" }), note: "基本 token 與自訂類別可用；函式和 String 等型別參照仍為一般識別字。" },
    { id: "shell", name: "Shell", files: ["sample.sh", "sample.bash", "sample.zsh", ".bashrc", ".zshrc"], grammar: "legacy", code: '# Comment sample\ngreet() {\n  local count=42\n  if [ "$count" -gt 0 ]; then\n    echo "hello"\n  fi\n}\ngreet\n', probes: probes({ comment: "# Comment sample", keyword: "if", string: '"hello"', number: "42", function: "greet" }, { function: "plain" }), note: "關鍵字、字串、註解、數值可用；使用者函式名稱未獨立分類。" },
    { id: "powershell", name: "PowerShell", files: ["sample.ps1", "sample.psm1", "sample.psd1"], grammar: "legacy", code: '# Comment sample\nfunction Get-Greeting {\n  [int]$count = 42\n  if ($count -gt 0) { return "hello" }\n}\nGet-Greeting\n', probes: probes({ comment: "# Comment sample", keyword: "return", string: '"hello"', number: "42", function: "Get-Greeting", type: "int" }, { function: "variable", type: "variable" }), note: "關鍵字、字串、註解、數值可用；函式與方括號型別仍為一般識別字。" },
    { id: "sql", name: "SQL", files: ["sample.sql"], grammar: "structured", code: '-- Comment sample\nSELECT COUNT(id), \'hello\' AS message, 42 AS count\nFROM greetings\nWHERE enabled = TRUE;\n', probes: probes({ comment: "-- Comment sample", keyword: "SELECT", string: "'hello'", number: "42", function: "COUNT" }, { function: "keyword" }), note: "通用 SQL 方言；COUNT 等內建函式沿用關鍵字分類。" },
    { id: "yaml", name: "YAML", files: ["sample.yaml", "sample.yml"], grammar: "structured", code: '# Comment sample\nmessage: "hello"\ncount: 42\nenabled: true\nitems:\n  - sample\n', probes: probes({ comment: "# Comment sample", property: "message", string: '"hello"', number: "42" }, { number: "plain" }), note: "鍵、引號字串和註解可用；未引號數字與布林值在上游 grammar 為一般純量，尚無獨立配色。" },
    { id: "toml", name: "TOML", files: ["sample.toml"], grammar: "legacy", code: '# Comment sample\n[server]\nmessage = "hello"\ncount = 42\nenabled = true\n', probes: probes({ comment: "# Comment sample", property: "message", string: '"hello"', number: "42" }), note: "資料語言，驗證鍵、字串、註解、數值；無函式要求。" },
    { id: "xml", name: "XML", files: ["sample.xml", "sample.svg", "sample.xsd", "sample.xsl", "sample.csproj", "sample.fsproj"], grammar: "structured", code: '<?xml version="1.0"?>\n<!-- Comment sample -->\n<greeting message="hello" count="42">Content</greeting>\n', probes: probes({ comment: "<!-- Comment sample -->", tag: "greeting", property: "message", string: '"hello"' }), note: "XML 属性中的 42 是字串，csproj/fsproj 是 XML 格式而非 C#/F# 程式。" },
    { id: "vue", name: "Vue", files: ["Sample.vue"], grammar: "structured", code: '<script setup lang="tsx">\n// Comment sample\ninterface Greeter { name: string }\nfunction greet(): string { return "hello"; }\nconst count: number = 42;\nconst node = <strong title="tsx">Embedded JSX</strong>;\n</script>\n<template><section title="welcome">{{ greet() }} {{ count }}</section></template>\n<style lang="scss">\n$gap: 12px;\n.card { color: red; padding: $gap; }\n</style>\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter", tag: "section", property: "padding" }) },
    { id: "svelte", name: "Svelte", files: ["Sample.svelte"], grammar: "structured", code: '<script lang="ts">\n// Comment sample\ninterface Greeter { name: string }\nfunction greet(): string { return "hello"; }\nlet count: number = 42;\n</script>\n<section title="welcome">{greet()} {count}</section>\n{#if count > 0}<strong>Visible</strong>{/if}\n<style>.card { color: red; }</style>\n', probes: probes({ comment: "// Comment sample", keyword: "return", string: '"hello"', number: "42", function: "greet", type: "Greeter", tag: "section", property: "color" }) },
]

for (const id of ["csharp", "swift", "kotlin"]) {
    syntaxSamples.find(sample => sample.id === id)!.probes.push({ role: "type", text: "Greeter", renderedAs: "definition" })
}
syntaxSamples.find(sample => sample.id === "dart")!.probes.push({ role: "type", text: "Greeter" })
const svelteSample = syntaxSamples.find(sample => sample.id === "svelte")!
svelteSample.note = "已驗證 TypeScript、原生 CSS 與 Svelte 標記；style lang=\"scss\"/\"sass\"/\"less\" 尚未提供對應預處理器語法，不能視為完整支援。"
svelteSample.limitations = [{ scope: "style-preprocessors", status: "partial", detail: "套件沒有公開 nestedLanguages 設定；SCSS 被當成 CSS，Sass/Less 沒有對應嵌入 parser。" }]

export const syntaxCoreRoles = ["keyword", "string", "comment", "number", "function", "type"] as const

/** Machine-readable role coverage. A partial role is asserted against its actual
 * grammar classification; it is never counted as a successful semantic role. */
export const syntaxCoverage = syntaxSamples.map(sample => ({
    id: sample.id,
    name: sample.name,
    files: sample.files,
    grammar: sample.grammar,
    status: sample.limitations?.length || sample.probes.some(probe => probe.renderedAs && probe.renderedAs !== probe.role) ? "partial" : "supported",
    note: sample.note ?? "",
    limitations: sample.limitations ?? [],
    roles: Object.fromEntries([...syntaxCoreRoles, "property", "tag", "heading", "link"].map(role => {
        const matches = sample.probes.filter(probe => probe.role === role)
        return [role, {
            status: matches.length === 0 ? "not-applicable" : matches.some(probe => probe.renderedAs && probe.renderedAs !== role) ? "partial" : "supported",
            probes: matches.map(probe => ({ text: probe.text, expected: role, hit: !probe.renderedAs || probe.renderedAs === role, renderedAs: probe.renderedAs ?? role })),
        }]
    })),
}))
