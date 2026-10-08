import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { BookmarkPlus, ImagePlus, ImageUp, Minus, Plus, Trash2, X } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Field, FieldLabel } from "@/components/ui/field"
import { Slider } from "@/components/ui/slider"
import type { BackgroundAppearance } from "@/app/workbench/settingsStorage"
import { SettingCard, SettingsRowGroup, Segmented, ToggleRow } from "./settingsPrimitives"
import {
  BACKGROUND_PRESETS,
  MAX_GRADIENT_COLORS,
  addSavedGradient,
  gradientSwatchBackground,
  padColor,
  padPosition,
  sameGradientColors,
  type BackgroundGradient,
  type BackgroundPresetId,
  type BackgroundSource,
} from "@/theme/background"
import { clearBackgroundImage, prepareBackgroundImage, saveBackgroundImage } from "@/theme/backgroundImage"
import { extractPaletteFromImage } from "@/theme/imagePalette"
import { windowGlassPlatform, type GlassPlatform } from "@/theme/windowGlass"
import "./background-settings.css"

const PRESETS = Object.entries(BACKGROUND_PRESETS) as [BackgroundPresetId, readonly string[]][]
const HUE_STEP = 1 / 72
const TONE_STEP = 0.05

/** Arc-style backdrop editor: color dots on a hue × vividness pad, plus palettes. */
export function BackgroundSettings({
  value,
  onChange,
}: {
  value: BackgroundAppearance
  onChange: (patch: Partial<BackgroundAppearance>) => void
}) {
  const { t: tw } = useTranslation("workbench")
  const { t: td } = useTranslation("settingsDemo")
  const { backgroundSource, backgroundGradient, savedGradients } = value
  const [selected, setSelected] = useState(0)
  const [imageError, setImageError] = useState(false)
  /** i18n key of the backdrop image failure being shown. */
  const [backdropImageError, setBackdropImageError] = useState<string | null>(null)
  const fileRef = useRef<HTMLInputElement>(null)
  const backdropFileRef = useRef<HTMLInputElement>(null)
  /** Bumped by every pick and removal: a slower, older pick must not land after a newer action. */
  const imageRequest = useRef(0)
  const hasImage = value.backgroundImageVersion > 0
  // Windows reports its build asynchronously (UA-CH), so the glass row appears once known.
  const [glassPlatform, setGlassPlatform] = useState<GlassPlatform | null>(null)
  useEffect(() => {
    let live = true
    void windowGlassPlatform().then(platform => { if (live) setGlassPlatform(platform) })
    return () => { live = false }
  }, [])
  const selectedIndex = Math.min(selected, backgroundGradient.colors.length - 1)
  // Same colors at another intensity can still be saved: Save then replaces that entry.
  const isSaved = savedGradients.some(entry =>
    sameGradientColors(entry.colors, backgroundGradient.colors) && entry.intensity === backgroundGradient.intensity)
  /**
   * Bumped by every source-affecting action (gradient edits, source switch,
   * palette import, image pick or removal): a slower, older one never
   * overrides the newer intent.
   */
  const intent = useRef(0)
  const changePalette = (patch: Partial<BackgroundAppearance>) => {
    intent.current += 1
    onChange(patch)
  }

  const applyGradient = (gradient: BackgroundGradient, patch: Partial<BackgroundAppearance> = {}) => {
    changePalette({ backgroundSource: "gradient", backgroundGradient: gradient, ...patch })
  }
  const setColors = (colors: string[]) => applyGradient({ ...backgroundGradient, colors })
  const setColor = (index: number, color: string) => setColors(backgroundGradient.colors.map((entry, i) => i === index ? color : entry))

  const addColor = () => {
    const last = padPosition(backgroundGradient.colors[backgroundGradient.colors.length - 1])
    setColors([...backgroundGradient.colors, padColor((last.x + 0.33) % 1, last.y)])
    setSelected(backgroundGradient.colors.length)
  }
  const removeColor = () => {
    setColors(backgroundGradient.colors.filter((_, index) => index !== selectedIndex))
    setSelected(Math.max(0, selectedIndex - 1))
  }

  const setBackdropImage = async (file: File) => {
    const request = ++imageRequest.current
    const mine = ++intent.current
    setBackdropImageError(null)
    try {
      const image = await prepareBackgroundImage(file)
      // IndexedDB runs the writes in the order they start, so a pick that
      // still saves here is the latest one or is followed by the newer write.
      if (request !== imageRequest.current || mine !== intent.current) return
      await saveBackgroundImage(image)
      if (request !== imageRequest.current) return
      // Stored before a newer source choice landed: keep that choice and only
      // record the new image.
      onChange(mine === intent.current
        ? { backgroundSource: "image", backgroundImageVersion: Date.now() }
        : { backgroundImageVersion: Date.now() })
    } catch {
      if (request === imageRequest.current && mine === intent.current) setBackdropImageError("settings.backgroundImageUnusable")
    }
  }
  const removeBackdropImage = async () => {
    const request = ++imageRequest.current
    const mine = ++intent.current
    setBackdropImageError(null)
    try {
      await clearBackgroundImage()
    } catch {
      // Still stored: keep the image (and its Remove button) and say so.
      if (request === imageRequest.current) setBackdropImageError("settings.backgroundImageRemoveFailed")
      return
    }
    if (request !== imageRequest.current) return
    onChange(mine === intent.current
      ? { backgroundSource: "accent", backgroundImageVersion: 0 }
      : { backgroundImageVersion: 0 })
  }

  const importImage = async (file: File) => {
    const request = ++intent.current
    setImageError(false)
    try {
      const colors = await extractPaletteFromImage(file)
      // A newer import or palette change since then wins; this result is stale.
      if (request !== intent.current) return
      if (colors.length === 0) throw new Error("no colors")
      const gradient = { colors, intensity: backgroundGradient.intensity }
      setSelected(0)
      applyGradient(gradient, { savedGradients: addSavedGradient(savedGradients, gradient) })
    } catch {
      if (request === intent.current) setImageError(true)
    }
  }

  return (
    <>
      <SettingCard label={tw("settings.background")} sub={tw("settings.backgroundSub")}>
        <Segmented
          label={tw("settings.background")}
          options={[
            { id: "accent", label: tw("settings.backgroundAccent") },
            { id: "gradient", label: tw("settings.backgroundGradient") },
            { id: "image", label: tw("settings.backgroundImage") },
          ]}
          value={backgroundSource}
          onChange={id => changePalette({ backgroundSource: id as BackgroundSource })}
        />
        {backgroundSource === "gradient" && <div className="background-editor">
          <GradientPad
            colors={backgroundGradient.colors}
            selected={selectedIndex}
            onSelect={setSelected}
            onColorChange={setColor}
          />
          <div className="background-editor-actions">
            <Button variant="outline" size="sm" disabled={backgroundGradient.colors.length >= MAX_GRADIENT_COLORS} onClick={addColor}><Plus data-icon="inline-start" />{tw("settings.backgroundAddColor")}</Button>
            <Button variant="outline" size="sm" disabled={backgroundGradient.colors.length <= 1} onClick={removeColor}><Minus data-icon="inline-start" />{tw("settings.backgroundRemoveColor")}</Button>
            <Button variant="outline" size="sm" disabled={isSaved} onClick={() => changePalette({ savedGradients: addSavedGradient(savedGradients, backgroundGradient) })}><BookmarkPlus data-icon="inline-start" />{tw(isSaved ? "settings.backgroundSaved" : "settings.backgroundSave")}</Button>
          </div>
          <Field data-settings-label={tw("settings.backgroundIntensity")}>
            <FieldLabel>{tw("settings.backgroundIntensity")}</FieldLabel>
            <div className="settings-slider-row">
              <Slider min={0} max={100} step={1} value={[backgroundGradient.intensity]} aria-label={tw("settings.backgroundIntensity")} onValueChange={next => applyGradient({ ...backgroundGradient, intensity: next[0] })} />
              <output className="settings-slider-value">{backgroundGradient.intensity}%</output>
            </div>
          </Field>
        </div>}
        {backgroundSource === "image" && <div className="background-editor">
          <div className="background-editor-actions">
            <Button variant="outline" size="sm" onClick={() => backdropFileRef.current?.click()}><ImageUp data-icon="inline-start" />{tw(hasImage ? "settings.backgroundReplaceImage" : "settings.backgroundChooseImage")}</Button>
            {hasImage && <Button variant="outline" size="sm" onClick={() => void removeBackdropImage()}><Trash2 data-icon="inline-start" />{tw("settings.backgroundRemoveImage")}</Button>}
            <input
              ref={backdropFileRef}
              type="file"
              accept="image/*"
              hidden
              data-testid="background-backdrop-input"
              onChange={event => {
                const file = event.target.files?.[0]
                event.target.value = ""
                if (file) void setBackdropImage(file)
              }}
            />
          </div>
          {hasImage
            ? <Field data-settings-label={tw("settings.backgroundImageIntensity")}>
              <FieldLabel>{tw("settings.backgroundImageIntensity")}</FieldLabel>
              <div className="settings-slider-row">
                <Slider min={0} max={100} step={1} value={[value.backgroundImageIntensity]} aria-label={tw("settings.backgroundImageIntensity")} onValueChange={next => onChange({ backgroundImageIntensity: next[0] })} />
                <output className="settings-slider-value">{value.backgroundImageIntensity}%</output>
              </div>
            </Field>
            : <p className="settings-inline-hint">{tw("settings.backgroundImageEmpty")}</p>}
          {backdropImageError && <p role="alert" className="settings-inline-hint background-image-error">{tw(backdropImageError)}</p>}
        </div>}
        <div className="background-swatches" role="group" aria-label={tw("settings.backgroundPalette")}>
          {PRESETS.map(([id, colors]) => <button
            key={id}
            type="button"
            className="background-swatch"
            aria-label={td(`backgrounds.${id}`)}
            title={td(`backgrounds.${id}`)}
            aria-pressed={backgroundSource === "gradient" && sameGradientColors(backgroundGradient.colors, colors)}
            style={{ background: gradientSwatchBackground(colors) }}
            onClick={() => { setSelected(0); applyGradient({ colors: [...colors], intensity: backgroundGradient.intensity }) }}
          />)}
          {savedGradients.map((gradient, index) => <span key={gradient.colors.join()} className="background-swatch-saved">
            <button
              type="button"
              className="background-swatch"
              aria-label={tw("settings.backgroundSavedSwatch", { index: index + 1 })}
              title={gradient.colors.join(" · ")}
              aria-pressed={backgroundSource === "gradient" && sameGradientColors(backgroundGradient.colors, gradient.colors)}
              style={{ background: gradientSwatchBackground(gradient.colors) }}
              onClick={() => { setSelected(0); applyGradient(gradient) }}
            />
            <button
              type="button"
              className="background-swatch-remove"
              aria-label={tw("settings.backgroundRemoveSaved", { index: index + 1 })}
              onClick={() => changePalette({ savedGradients: savedGradients.filter((_, i) => i !== index) })}
            ><X aria-hidden="true" /></button>
          </span>)}
          <button type="button" className="background-swatch background-swatch-upload" aria-label={tw("settings.backgroundFromImage")} title={tw("settings.backgroundFromImage")} onClick={() => fileRef.current?.click()}><ImagePlus aria-hidden="true" /></button>
          <input
            ref={fileRef}
            type="file"
            accept="image/*"
            hidden
            data-testid="background-image-input"
            onChange={event => {
              const file = event.target.files?.[0]
              event.target.value = ""
              if (file) void importImage(file)
            }}
          />
        </div>
        {imageError
          ? <p role="alert" className="settings-inline-hint background-image-error">{tw("settings.backgroundImageFailed")}</p>
          : <p className="settings-inline-hint">{tw("settings.backgroundHint")}</p>}
      </SettingCard>

      {glassPlatform && <SettingsRowGroup>
        <ToggleRow
          label={tw("settings.windowGlass")}
          sub={tw("settings.windowGlassSub")}
          checked={value.glass}
          onCheckedChange={glass => onChange({ glass })}
        />
        {value.glass && <Field className="background-glass-tint" data-settings-label={tw("settings.windowGlassTint")}>
          <FieldLabel>{tw("settings.windowGlassTint")}</FieldLabel>
          <div className="settings-slider-row">
            <Slider min={0} max={100} step={1} value={[value.glassTint]} aria-label={tw("settings.windowGlassTint")} onValueChange={next => onChange({ glassTint: next[0] })} />
            <output className="settings-slider-value">{value.glassTint}%</output>
          </div>
        </Field>}
      </SettingsRowGroup>}
    </>
  )
}

function GradientPad({
  colors,
  selected,
  onSelect,
  onColorChange,
}: {
  colors: string[]
  selected: number
  onSelect: (index: number) => void
  onColorChange: (index: number, color: string) => void
}) {
  const { t: tw } = useTranslation("workbench")
  const padRef = useRef<HTMLDivElement>(null)
  const dragRef = useRef<number | null>(null)

  const moveTo = (index: number, event: React.PointerEvent) => {
    const rect = padRef.current?.getBoundingClientRect()
    if (!rect || rect.width === 0 || rect.height === 0) return
    onColorChange(index, padColor((event.clientX - rect.left) / rect.width, (event.clientY - rect.top) / rect.height))
  }

  return (
    <div
      ref={padRef}
      className="background-pad"
      onPointerDown={event => {
        // A right-click (context menu) or other button must not recolor anything.
        if (event.button !== 0) return
        const dot = (event.target as HTMLElement).closest<HTMLElement>("[data-dot-index]")
        const index = dot ? Number(dot.dataset.dotIndex) : selected
        onSelect(index)
        dragRef.current = index
        event.currentTarget.setPointerCapture(event.pointerId)
        // Grabbing a dot keeps its color until it actually moves.
        if (!dot) moveTo(index, event)
      }}
      onPointerMove={event => { if (dragRef.current !== null) moveTo(dragRef.current, event) }}
      onPointerUp={event => {
        dragRef.current = null
        if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
      }}
      onLostPointerCapture={() => { dragRef.current = null }}
    >
      {colors.map((color, index) => {
        const { x, y } = padPosition(color)
        return <button
          key={index}
          type="button"
          className="background-pad-dot"
          data-dot-index={index}
          data-selected={index === selected}
          aria-pressed={index === selected}
          aria-label={tw("settings.backgroundColorDot", { index: index + 1, color })}
          style={{ left: `${x * 100}%`, top: `${y * 100}%`, background: color }}
          onFocus={() => onSelect(index)}
          onKeyDown={event => {
            const dx = event.key === "ArrowLeft" ? -HUE_STEP : event.key === "ArrowRight" ? HUE_STEP : 0
            const dy = event.key === "ArrowUp" ? -TONE_STEP : event.key === "ArrowDown" ? TONE_STEP : 0
            if (!dx && !dy) return
            event.preventDefault()
            onColorChange(index, padColor((x + dx + 1) % 1, y + dy))
          }}
        />
      })}
    </div>
  )
}
