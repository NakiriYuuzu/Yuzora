import { describe, expect, it } from 'vitest'
import { formatTerminalSelection } from './terminalCopyFormat'

describe('HERDR copy formatting', () => {
  it('left-aligns prose and removes padding and excess blank paragraphs without reflow', () => {
    expect(formatTerminalSelection('\n  第一段 內容\t\n\n \n\n   Second  sentence\n  continuation\n\n', 'lf'))
      .toBe('第一段 內容\n\nSecond  sentence\ncontinuation')
  })
  it.each(['lf', 'crlf'] as const)('normalizes mixed input to %s once', eol => {
    expect(formatTerminalSelection('  一\r\n\r  二\n  三\r', eol)).toBe(['一', '', '二', '三'].join(eol === 'lf' ? '\n' : '\r\n'))
  })
  it('preserves nested lists, continuation alignment, quotes and table cells', () => {
    const text = '- first\n  continued\n  - nested\n\n> quote\n>   continuation\n\n| key | value |\n| --- | --- |\n| a   | b     |'
    expect(formatTerminalSelection(text, 'lf')).toBe(text)
  })
  it('preserves fenced and indented code including whitespace inside the code', () => {
    const text = '```py\n  if ok:\n    print("a  b")  \n\n\n```\n\n    indented()  \n    next()'
    expect(formatTerminalSelection(text, 'lf')).toBe(text)
  })
  it('keeps incomplete fences and ambiguous code selections', () => {
    expect(formatTerminalSelection('```ts\n  run()  \n    next()', 'lf')).toBe('```ts\n  run()  \n    next()')
    expect(formatTerminalSelection('\tcode\n\t  nested', 'lf')).toBe('\tcode\n\t  nested')
  })
  it('preserves inline code and intentional hard breaks but drops final padding', () => {
    expect(formatTerminalSelection('  Use `a  b` here  \n  next   ', 'lf')).toBe('Use `a  b` here  \nnext')
    const multiline = 'before `code\n  continuation` after'
    expect(formatTerminalSelection(multiline, 'lf')).toBe(multiline)
  })
  it('preserves tree and ASCII diagram alignment', () => {
    for (const text of ['root\n  ├── one\n  └── two', '  +-----+\n  | box |\n  +-----+', 'NAME    VALUE\n alpha   beta']) {
      expect(formatTerminalSelection(text, 'lf')).toBe(text)
    }
  })
  it('requires two column lines within the same paragraph to preserve indentation', () => {
    expect(formatTerminalSelection('  one  pair\n  ordinary\n\n  another  pair\n  ordinary', 'lf'))
      .toBe('one  pair\nordinary\n\nanother  pair\nordinary')
    const columns = '  first  pair\n  ordinary\n  second  pair\n  trailing'
    expect(formatTerminalSelection(columns, 'lf')).toBe(columns)
  })
  it('preserves the entire paragraph when a diagram appears near either end', () => {
    for (const text of ['  ├── first\n  ordinary\n  trailing', '  ordinary\n  one  pair\n  └── last']) {
      expect(formatTerminalSelection(text, 'lf')).toBe(text)
    }
  })
  it('still detects layout when protected code and plain text share a paragraph', () => {
    const text = '```ts\n  code()  \n```\n  root\n    └── branch'
    expect(formatTerminalSelection(text, 'lf')).toBe(text)
  })
  it('does not invent missing markdown or a trailing newline, and is idempotent', () => {
    for (const text of ['  Rendered heading\n\n  • item', '  a\n\n\n b', '```\n  a\n```']) {
      const result = formatTerminalSelection(text, 'crlf')
      expect(formatTerminalSelection(result, 'crlf')).toBe(result)
      expect(result.endsWith('\n')).toBe(false)
    }
    expect(formatTerminalSelection(' \t\r\n\n', 'crlf')).toBe('')
  })
})
