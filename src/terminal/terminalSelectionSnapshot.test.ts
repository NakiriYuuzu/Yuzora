import { describe, expect, it } from 'vitest'
import type { Terminal } from '@xterm/xterm'
import { terminalSelectionSnapshot } from './terminalSelectionSnapshot'
import { formatTerminalSelection } from './terminalCopyFormat'

function terminal(lines: string[], end = lines.at(-1)!.length, wrapped: number[] = []) {
  const selected: string[] = []
  lines.forEach((line, index) => { if (wrapped.includes(index)) selected[selected.length - 1] += line; else selected.push(line) })
  return {
    cols: 40,
    getSelection: () => selected.join('\n'),
    getSelectionPosition: () => ({ start: { x: 0, y: 0 }, end: { x: end, y: lines.length - 1 } }),
    buffer: { active: { getLine: (row: number) => lines[row] === undefined ? undefined : { isWrapped: wrapped.includes(row), translateToString: () => lines[row] } } },
  } as unknown as Terminal
}
describe('native terminal selection snapshot', () => {
  it('removes HERDR right-margin cells before protecting Markdown code', () => {
    const lines = ['  paragraph', '', '```py', '  if ok:', '    print("a  b")', '```'].map(line => line.padEnd(40))
    const text = terminalSelectionSnapshot(terminal(lines, 40))
    expect(formatTerminalSelection(text, 'lf')).toBe('paragraph\n\n```py\n  if ok:\n    print("a  b")\n```')
  })
  it('retains short intentional trailing whitespace and partial selection spaces', () => {
    expect(terminalSelectionSnapshot(terminal(['```', '  code  ', '```']))).toBe('```\n  code  \n```')
    expect(terminalSelectionSnapshot(terminal(['partial    ']))).toBe('partial    ')
  })
  it('preserves spaces joining soft-wrapped lines', () => {
    const first = 'word'.padEnd(40)
    expect(terminalSelectionSnapshot(terminal([first, 'next'], 4, [1]))).toBe(first + 'next')
  })
  it('falls back to xterm selection for column modes rather than copying outside the selection', () => {
    const term = terminal(['full line'.padEnd(40), 'another line'.padEnd(40)], 40)
    term.getSelection = () => 'line\nline'
    expect(terminalSelectionSnapshot(term)).toBe('line\nline')
  })
})
