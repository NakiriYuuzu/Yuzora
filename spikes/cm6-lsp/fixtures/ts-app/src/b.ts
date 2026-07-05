import { buildGreeting, repeatGreeting, type Greeting } from './a'

const greeting = buildGreeting('Yuzora', 3)
const lines = repeatGreeting(greeting)

console.log(lines.join('\n'))
