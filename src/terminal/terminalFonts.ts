const CJK_FALLBACK = '"PingFang TC", "Microsoft JhengHei", "Noto Sans CJK TC", "Hiragino Sans", "Yu Gothic", monospace'
const JETBRAINS_MONO = '"JetBrains Mono Variable", "JetBrains Mono"'

export const TERMINAL_FONTS = [
  { id: 'jetbrains', name: 'JetBrains Mono', stack: `${JETBRAINS_MONO}, ${CJK_FALLBACK}` },
  { id: 'system', name: 'system', stack: `ui-monospace, "SFMono-Regular", Consolas, ${JETBRAINS_MONO}, ${CJK_FALLBACK}` },
  { id: 'menlo', name: 'Menlo', stack: `Menlo, ${JETBRAINS_MONO}, ${CJK_FALLBACK}` },
  { id: 'cascadia', name: 'Cascadia Code', stack: `"Cascadia Code", ${JETBRAINS_MONO}, ${CJK_FALLBACK}` },
  { id: 'consolas', name: 'Consolas', stack: `Consolas, ${JETBRAINS_MONO}, ${CJK_FALLBACK}` },
] as const

export type TerminalFontFamily = typeof TERMINAL_FONTS[number]['id']
export function normalizeTerminalFontFamily(value: unknown): TerminalFontFamily {
  return TERMINAL_FONTS.find(font => font.id === value)?.id ?? 'jetbrains'
}
export function terminalFontStack(value: TerminalFontFamily): string {
  return (TERMINAL_FONTS.find(font => font.id === value) ?? TERMINAL_FONTS[0]).stack
}
