import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  extract: vi.fn<(file: Blob) => Promise<string[]>>(),
  prepare: vi.fn<(file: Blob) => Promise<Blob>>(),
  save: vi.fn<(image: Blob) => Promise<void>>(),
  clear: vi.fn<() => Promise<void>>(),
  glass: vi.fn(async (): Promise<"macos" | "windows" | null> => null),
}))

vi.mock("@/theme/imagePalette", () => ({ extractPaletteFromImage: mocks.extract }))
vi.mock("@/theme/backgroundImage", () => ({ prepareBackgroundImage: mocks.prepare, saveBackgroundImage: mocks.save, clearBackgroundImage: mocks.clear }))
vi.mock("@/theme/windowGlass", () => ({ windowGlassPlatform: mocks.glass }))

import { BackgroundSettings } from "./BackgroundSettings"
import { DEFAULT_BACKGROUND_APPEARANCE, type BackgroundAppearance } from "./settingsStorage"
import { BACKGROUND_PRESETS, padColor, padPosition } from "@/theme/background"

const gradientValue: BackgroundAppearance = {
  ...DEFAULT_BACKGROUND_APPEARANCE,
  backgroundSource: "gradient",
  backgroundGradient: { colors: ["#3ddc97", "#46a0ff"], intensity: 60 },
}

function renderSettings(value: BackgroundAppearance = DEFAULT_BACKGROUND_APPEARANCE) {
  const onChange = vi.fn()
  render(<BackgroundSettings value={value} onChange={onChange} />)
  return onChange
}

beforeEach(() => {
  mocks.glass.mockResolvedValue(null)
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  vi.restoreAllMocks()
})

describe("background settings", () => {
  it("keeps the accent backdrop until a custom gradient is chosen", () => {
    const onChange = renderSettings()
    expect(screen.getByRole("radio", { name: "Follow accent" })).toHaveAttribute("aria-checked", "true")
    expect(screen.queryByRole("button", { name: "Add color" })).toBeNull()
    fireEvent.click(screen.getByRole("radio", { name: "Custom gradient" }))
    expect(onChange).toHaveBeenCalledWith({ backgroundSource: "gradient" })
  })

  it("switches to a preset gradient while keeping the current intensity", () => {
    const onChange = renderSettings(gradientValue)
    fireEvent.click(screen.getByRole("button", { name: "Sunset" }))
    expect(onChange).toHaveBeenCalledWith({
      backgroundSource: "gradient",
      backgroundGradient: { colors: [...BACKGROUND_PRESETS.sunset], intensity: 60 },
    })
  })

  it("adds up to three colors and never removes the last one", () => {
    const onChange = renderSettings(gradientValue)
    expect(screen.getAllByRole("button", { name: /^Color \d/ })).toHaveLength(2)
    fireEvent.click(screen.getByRole("button", { name: "Add color" }))
    expect(onChange.mock.calls[0][0].backgroundGradient.colors).toHaveLength(3)
    cleanup()

    renderSettings({ ...gradientValue, backgroundGradient: { colors: ["#111111", "#222222", "#333333"], intensity: 50 } })
    expect(screen.getByRole("button", { name: "Add color" })).toBeDisabled()
    cleanup()

    renderSettings({ ...gradientValue, backgroundGradient: { colors: ["#111111"], intensity: 50 } })
    expect(screen.getByRole("button", { name: "Remove color" })).toBeDisabled()
  })

  it("moves the focused color dot with the arrow keys", () => {
    const onChange = renderSettings(gradientValue)
    const { x, y } = padPosition("#46a0ff")
    fireEvent.keyDown(screen.getByRole("button", { name: /^Color 2/ }), { key: "ArrowDown" })
    expect(onChange.mock.calls[0][0].backgroundGradient.colors).toEqual(["#3ddc97", padColor(x, y + 0.05)])
    fireEvent.keyDown(screen.getByRole("button", { name: /^Color 1/ }), { key: "ArrowLeft" })
    const first = padPosition("#3ddc97")
    expect(onChange.mock.calls[1][0].backgroundGradient.colors).toEqual([padColor(first.x - 1 / 72, first.y), "#46a0ff"])
  })

  it("drags the grabbed dot across the pad without recoloring other dots", () => {
    const capture = vi.spyOn(HTMLElement.prototype, "setPointerCapture").mockImplementation(() => {})
    vi.spyOn(HTMLElement.prototype, "hasPointerCapture").mockReturnValue(true)
    vi.spyOn(HTMLElement.prototype, "releasePointerCapture").mockImplementation(() => {})
    const onChange = renderSettings(gradientValue)
    const dot = screen.getByRole("button", { name: /^Color 2/ })
    const pad = dot.parentElement!
    vi.spyOn(pad, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 200, 100))

    fireEvent.pointerDown(dot, { pointerId: 1, clientX: 20, clientY: 20 })
    expect(capture).toHaveBeenCalledWith(1)
    expect(onChange).not.toHaveBeenCalled()
    fireEvent.pointerMove(pad, { pointerId: 1, clientX: 100, clientY: 50 })
    expect(onChange).toHaveBeenLastCalledWith({
      backgroundSource: "gradient",
      backgroundGradient: { colors: ["#3ddc97", padColor(0.5, 0.5)], intensity: 60 },
    })
    fireEvent.pointerUp(pad, { pointerId: 1 })
    fireEvent.pointerMove(pad, { pointerId: 1, clientX: 10, clientY: 10 })
    expect(onChange).toHaveBeenCalledTimes(1)
  })

  it("turns an image into a saved gradient", async () => {
    mocks.extract.mockResolvedValueOnce(["#112233", "#445566"])
    const onChange = renderSettings(gradientValue)
    fireEvent.change(screen.getByTestId("background-image-input"), { target: { files: [new File(["x"], "photo.png", { type: "image/png" })] } })
    await waitFor(() => expect(onChange).toHaveBeenCalled())
    const gradient = { colors: ["#112233", "#445566"], intensity: 60 }
    expect(onChange).toHaveBeenCalledWith({ backgroundSource: "gradient", backgroundGradient: gradient, savedGradients: [gradient] })
  })

  it("explains an unreadable image instead of changing the backdrop", async () => {
    mocks.extract.mockRejectedValueOnce(new Error("decode failed"))
    const onChange = renderSettings(gradientValue)
    fireEvent.change(screen.getByTestId("background-image-input"), { target: { files: [new File(["x"], "broken.heic")] } })
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't read colors")
    expect(onChange).not.toHaveBeenCalled()
  })

  it("removes a saved gradient from the palette", () => {
    const saved = { colors: ["#112233"], intensity: 40 }
    const onChange = renderSettings({ ...gradientValue, savedGradients: [saved] })
    fireEvent.click(screen.getByRole("button", { name: "Remove saved gradient 1" }))
    expect(onChange).toHaveBeenCalledWith({ savedGradients: [] })
  })

  it("offers glass only where the window supports it", async () => {
    renderSettings()
    await waitFor(() => expect(mocks.glass).toHaveBeenCalled())
    expect(screen.queryByRole("switch", { name: "Glass window" })).toBeNull()
    cleanup()

    for (const platform of ["macos", "windows"] as const) {
      mocks.glass.mockResolvedValue(platform)
      const onChange = renderSettings()
      fireEvent.click(await screen.findByRole("switch", { name: "Glass window" }))
      expect(onChange).toHaveBeenCalledWith({ glass: true })
      expect(screen.queryByRole("slider", { name: "Background opacity" })).toBeNull()
      cleanup()
    }

    renderSettings({ ...DEFAULT_BACKGROUND_APPEARANCE, glass: true })
    expect(await screen.findByRole("slider", { name: "Background opacity" })).toBeInTheDocument()
  })

  it("sets a chosen image as the backdrop, stored scaled down", async () => {
    const scaled = new Blob(["jpeg"], { type: "image/jpeg" })
    mocks.prepare.mockResolvedValueOnce(scaled)
    mocks.save.mockResolvedValueOnce()
    vi.spyOn(Date, "now").mockReturnValue(1791460000000)
    const onChange = renderSettings({ ...DEFAULT_BACKGROUND_APPEARANCE, backgroundSource: "image" })
    expect(screen.getByRole("radio", { name: "Image" })).toHaveAttribute("aria-checked", "true")
    expect(screen.queryByRole("slider", { name: "Image strength" })).toBeNull()
    const photo = new File(["x"], "photo.png", { type: "image/png" })
    fireEvent.change(screen.getByTestId("background-backdrop-input"), { target: { files: [photo] } })
    await waitFor(() => expect(onChange).toHaveBeenCalledWith({ backgroundSource: "image", backgroundImageVersion: 1791460000000 }))
    expect(mocks.prepare).toHaveBeenCalledWith(photo)
    expect(mocks.save).toHaveBeenCalledWith(scaled)
  })

  it("keeps the backdrop unchanged when an image can't be stored", async () => {
    mocks.prepare.mockRejectedValueOnce(new Error("decode failed"))
    const onChange = renderSettings({ ...DEFAULT_BACKGROUND_APPEARANCE, backgroundSource: "image" })
    fireEvent.change(screen.getByTestId("background-backdrop-input"), { target: { files: [new File(["x"], "broken.heic")] } })
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't use that image")
    expect(onChange).not.toHaveBeenCalled()
  })

  it("adjusts and removes a stored image", async () => {
    mocks.clear.mockResolvedValueOnce()
    const onChange = renderSettings({ ...DEFAULT_BACKGROUND_APPEARANCE, backgroundSource: "image", backgroundImageVersion: 7 })
    expect(screen.getByRole("button", { name: "Replace image" })).toBeInTheDocument()
    expect(screen.getByRole("slider", { name: "Image strength" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Remove image" }))
    await waitFor(() => expect(onChange).toHaveBeenCalledWith({ backgroundSource: "accent", backgroundImageVersion: 0 }))
    expect(mocks.clear).toHaveBeenCalled()
  })

  it("lets only the latest image pick or removal commit", async () => {
    const pending: Array<(image: Blob) => void> = []
    mocks.prepare.mockImplementation(() => new Promise<Blob>(resolve => { pending.push(resolve) }))
    mocks.save.mockResolvedValue()
    mocks.clear.mockResolvedValue()
    const onChange = renderSettings({ ...DEFAULT_BACKGROUND_APPEARANCE, backgroundSource: "image", backgroundImageVersion: 7 })
    const input = screen.getByTestId("background-backdrop-input")
    const older = new Blob(["older"]), newer = new Blob(["newer"])
    fireEvent.change(input, { target: { files: [new File(["a"], "slow.png")] } })
    fireEvent.change(input, { target: { files: [new File(["b"], "fast.png")] } })
    pending[1](newer)
    await waitFor(() => expect(onChange).toHaveBeenCalledTimes(1))
    pending[0](older)
    await new Promise(resolve => setTimeout(resolve, 0))
    expect(mocks.save.mock.calls).toEqual([[newer]])
    expect(onChange).toHaveBeenCalledTimes(1)

    // A pick still decoding when the image is removed never re-enables it.
    fireEvent.change(input, { target: { files: [new File(["c"], "late.png")] } })
    fireEvent.click(screen.getByRole("button", { name: "Remove image" }))
    await waitFor(() => expect(onChange).toHaveBeenLastCalledWith({ backgroundSource: "accent", backgroundImageVersion: 0 }))
    pending[2](new Blob(["late"]))
    await new Promise(resolve => setTimeout(resolve, 0))
    expect(mocks.save).toHaveBeenCalledTimes(1)
    expect(onChange).toHaveBeenLastCalledWith({ backgroundSource: "accent", backgroundImageVersion: 0 })
  })

  it("does not let a slow removal undo an image picked after it", async () => {
    let cleared!: () => void
    mocks.clear.mockReturnValueOnce(new Promise<void>(resolve => { cleared = resolve }))
    mocks.prepare.mockResolvedValueOnce(new Blob(["new"]))
    mocks.save.mockResolvedValueOnce()
    vi.spyOn(Date, "now").mockReturnValue(1791460000000)
    const onChange = renderSettings({ ...DEFAULT_BACKGROUND_APPEARANCE, backgroundSource: "image", backgroundImageVersion: 7 })
    fireEvent.click(screen.getByRole("button", { name: "Remove image" }))
    fireEvent.change(screen.getByTestId("background-backdrop-input"), { target: { files: [new File(["n"], "new.png")] } })
    await waitFor(() => expect(onChange).toHaveBeenCalledWith({ backgroundSource: "image", backgroundImageVersion: 1791460000000 }))
    cleared()
    await new Promise(resolve => setTimeout(resolve, 0))
    expect(onChange).toHaveBeenCalledTimes(1)
  })

  it("still offers picking gradient colors from an image", () => {
    renderSettings(gradientValue)
    expect(screen.getByRole("button", { name: "Pick colors from an image" })).toBeInTheDocument()
  })
})
