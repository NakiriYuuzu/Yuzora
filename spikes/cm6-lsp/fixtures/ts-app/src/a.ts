export interface Greeting {
    text: string
    times: number
}

export function buildGreeting(name: string, times: number): Greeting {
    const text = `Hello, ${name}!`
    return { text, times }
}

export function repeatGreeting(greeting: Greeting): string[] {
    const lines: string[] = []
    for (let i = 0; i < greeting.times; i++) {
        lines.push(greeting.text)
    }
    return lines
}
