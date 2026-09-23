import type { HerdrAttentionItem } from "./herdrTypes"

/** A refresh of an existing completion is not another completed turn. */
export function newHerdrAttention(previous: Map<string, HerdrAttentionItem>, current: Map<string, HerdrAttentionItem>): HerdrAttentionItem[] {
  return [...current.values()].filter(item => (item.kind === "done" || item.kind === "blocked")
    && !item.seen && previous.get(item.key)?.kind !== item.kind)
}

let audio: AudioContext | undefined
export async function playHerdrSound(kind: "done" | "blocked"): Promise<void> {
  audio ??= new AudioContext()
  if (audio.state === "suspended") await audio.resume()
  const oscillator = audio.createOscillator()
  const gain = audio.createGain()
  oscillator.frequency.value = kind === "done" ? 660 : 880
  gain.gain.setValueAtTime(0.06, audio.currentTime)
  gain.gain.exponentialRampToValueAtTime(0.001, audio.currentTime + 0.16)
  oscillator.connect(gain); gain.connect(audio.destination)
  oscillator.start(); oscillator.stop(audio.currentTime + 0.18)
  oscillator.onended = () => { oscillator.disconnect(); gain.disconnect() }
}
