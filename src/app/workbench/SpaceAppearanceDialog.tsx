import {
  useState,
  type CSSProperties,
  type FormEvent,
  type RefObject,
} from "react";
import { Check } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Field,
  FieldGroup,
  FieldLabel,
  FieldDescription,
  FieldError,
} from "@/components/ui/field";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  PROJECT_COLOR_OPTIONS,
  resolveProjectPresentation,
} from "@/app/workbench/projectPresentation";
import {
  useRecentWorkspacesStore,
  type RecentWorkspaceColor,
} from "@/state/recentWorkspaces";
import { SpaceCharacter } from "./SpaceCharacter";
import {
  CHARACTER_SHELLS,
  CHARACTER_FACES,
  CHARACTER_DETAILS,
  type SpaceCharacterConfig,
} from "./space-character";

/** Local display preferences only. Shared by canonical project path, never runtime identity. */
export function SpaceAppearanceDialog({
  path,
  identityKey,
  onClose,
  returnFocusRef,
}: {
  path: string;
  identityKey: string;
  onClose: () => void;
  returnFocusRef: RefObject<HTMLButtonElement | null>;
}) {
  const { t } = useTranslation("spaceTree");
  const { t: tw } = useTranslation("workbench");
  const [saved] = useState(() =>
    useRecentWorkspacesStore.getState().presentationFor(identityKey),
  );
  const [original] = useState(() => resolveProjectPresentation(path, saved));
  const [name, setName] = useState(original.name);
  const [glyph, setGlyph] = useState(saved?.glyph ?? "");
  const [color, setColor] = useState<RecentWorkspaceColor>(original.color.id);
  const [avatarMode, setAvatarMode] = useState<"character" | "glyph">(
    original.avatarMode,
  );
  const [character, setCharacter] = useState<SpaceCharacterConfig>(
    original.character,
  );
  const preview = resolveProjectPresentation(path, { name, glyph, color });
  const invalid = !name.trim();
  function save(event: FormEvent) {
    event.preventDefault();
    if (invalid) return;
    useRecentWorkspacesStore
      .getState()
      .updatePresentation(identityKey, {
        name: name.trim(),
        glyph: glyph.trim(),
        color,
        avatarMode,
        character,
      });
    onClose();
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        className="tree-open-dialog space-appearance-dialog space-character-dialog"
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          returnFocusRef.current?.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle>{t("editSpace")}</DialogTitle>
          <DialogDescription>{t("appearanceScope")}</DialogDescription>
        </DialogHeader>
        <form onSubmit={save} className="space-appearance-form">
          <div className="space-character-workshop">
            <div
              className="space-character-preview"
              style={
                {
                  "--space-color": preview.color.background,
                  "--character-ink": preview.color.foreground,
                } as CSSProperties
              }
            >
              <span className="character-preview-label">
                {t("yourCharacter")}
              </span>
              <div className="character-stage">
                {avatarMode === "character" ? (
                  <SpaceCharacter character={character} />
                ) : (
                  <span
                    className="character-glyph-preview"
                    style={{
                      background: preview.color.background,
                      color: preview.color.foreground,
                    }}
                  >
                    {preview.glyph}
                  </span>
                )}
              </div>
              <strong className="character-preview-name">{preview.name}</strong>
              <span className="character-preview-path" title={path}>
                {path}
              </span>
              <p>{t("characterHint")}</p>
              <p className="character-motion-note">{t("reducedMotionNote")}</p>
              <div
                className="character-sidebar-sample"
                aria-label={t("sidebarPreview")}
              >
                <span
                  className="tree-space-identity"
                  data-avatar={avatarMode}
                  style={{
                    background: preview.color.background,
                    color: preview.color.foreground,
                  }}
                >
                  {avatarMode === "character" ? (
                    <SpaceCharacter character={character} portrait />
                  ) : (
                    preview.glyph
                  )}
                </span>
                <span>
                  <strong>{preview.name}</strong>
                  <small>{t("sidebarPreview")}</small>
                </span>
              </div>
            </div>
            <ScrollArea
              className="space-character-fields"
              viewportClassName="[&>div]:block!"
            >
              <FieldGroup>
                <Field data-invalid={invalid}>
                  <FieldLabel htmlFor="space-name">
                    {t("displayName")}
                  </FieldLabel>
                  <Input
                    id="space-name"
                    value={name}
                    maxLength={80}
                    onChange={(event) => setName(event.target.value)}
                    aria-invalid={invalid}
                    aria-describedby={invalid ? "space-name-error" : undefined}
                  />
                  {invalid && (
                    <FieldError id="space-name-error">
                      {t("nameRequired")}
                    </FieldError>
                  )}
                </Field>
                <Field>
                  <FieldLabel id="space-avatar-label">
                    {t("identityStyle")}
                  </FieldLabel>
                  <ToggleGroup
                    type="single"
                    value={avatarMode}
                    onValueChange={(value) => {
                      if (value) setAvatarMode(value as "character" | "glyph");
                    }}
                    aria-labelledby="space-avatar-label"
                    className="character-choice-group"
                  >
                    <ToggleGroupItem value="character">
                      {t("characterMode")}
                    </ToggleGroupItem>
                    <ToggleGroupItem value="glyph">
                      {t("glyphMode")}
                    </ToggleGroupItem>
                  </ToggleGroup>
                </Field>
                {avatarMode === "character" ? (
                  <>
                    <Field>
                      <FieldLabel id="character-shell-label">
                        {t("characterShape")}
                      </FieldLabel>
                      <ToggleGroup
                        type="single"
                        value={character.shell}
                        onValueChange={(value) => {
                          if (value)
                            setCharacter({
                              ...character,
                              shell: value as SpaceCharacterConfig["shell"],
                            });
                        }}
                        aria-labelledby="character-shell-label"
                        className="character-choice-group character-shape-options"
                      >
                        {CHARACTER_SHELLS.map((shell) => (
                          <ToggleGroupItem
                            key={shell}
                            value={shell}
                            aria-label={t(`character.shell.${shell}`)}
                          >
                            <span
                              style={
                                {
                                  "--space-color": preview.color.background,
                                  "--character-ink": preview.color.foreground,
                                } as CSSProperties
                              }
                            >
                              <SpaceCharacter
                                character={{ ...character, shell }}
                                portrait
                              />
                            </span>
                            {t(`character.shell.${shell}`)}
                          </ToggleGroupItem>
                        ))}
                      </ToggleGroup>
                    </Field>
                    <Field>
                      <FieldLabel id="character-face-label">
                        {t("characterFace")}
                      </FieldLabel>
                      <ToggleGroup
                        type="single"
                        value={character.face}
                        onValueChange={(value) => {
                          if (value)
                            setCharacter({
                              ...character,
                              face: value as SpaceCharacterConfig["face"],
                            });
                        }}
                        aria-labelledby="character-face-label"
                        className="character-choice-group"
                      >
                        {CHARACTER_FACES.map((face) => (
                          <ToggleGroupItem key={face} value={face}>
                            {t(`character.face.${face}`)}
                          </ToggleGroupItem>
                        ))}
                      </ToggleGroup>
                    </Field>
                    <Field>
                      <FieldLabel id="character-detail-label">
                        {t("characterDetail")}
                      </FieldLabel>
                      <ToggleGroup
                        type="single"
                        value={character.detail}
                        onValueChange={(value) => {
                          if (value)
                            setCharacter({
                              ...character,
                              detail: value as SpaceCharacterConfig["detail"],
                            });
                        }}
                        aria-labelledby="character-detail-label"
                        className="character-choice-group"
                      >
                        {CHARACTER_DETAILS.map((detail) => (
                          <ToggleGroupItem key={detail} value={detail}>
                            {t(`character.detail.${detail}`)}
                          </ToggleGroupItem>
                        ))}
                      </ToggleGroup>
                    </Field>
                    <Field>
                      <FieldLabel id="character-motion-label">
                        {t("characterMotion")}
                      </FieldLabel>
                      <ToggleGroup
                        type="single"
                        value={
                          character.motion === false ? "still" : "animated"
                        }
                        onValueChange={(value) => {
                          if (value)
                            setCharacter({
                              ...character,
                              motion: value === "animated",
                            });
                        }}
                        aria-labelledby="character-motion-label"
                        className="character-choice-group"
                      >
                        <ToggleGroupItem value="animated">
                          {t("animatedCharacter")}
                        </ToggleGroupItem>
                        <ToggleGroupItem value="still">
                          {t("stillCharacter")}
                        </ToggleGroupItem>
                      </ToggleGroup>
                    </Field>
                  </>
                ) : (
                  <Field>
                    <FieldLabel htmlFor="space-glyph">
                      {t("identityGlyph")}
                    </FieldLabel>
                    <Input
                      id="space-glyph"
                      value={glyph}
                      maxLength={16}
                      onChange={(event) => setGlyph(event.target.value)}
                      placeholder={t("automaticGlyph")}
                      aria-describedby="space-glyph-help"
                    />
                    <FieldDescription id="space-glyph-help">
                      {t("glyphHint")}
                    </FieldDescription>
                  </Field>
                )}
                <Field>
                  <FieldLabel id="space-palette-label">
                    {t("spacePalette")}
                  </FieldLabel>
                  <ToggleGroup
                    type="single"
                    value={color}
                    onValueChange={(value) => {
                      if (value) setColor(value as RecentWorkspaceColor);
                    }}
                    aria-labelledby="space-palette-label"
                    className="space-palette-options"
                  >
                    {PROJECT_COLOR_OPTIONS.map((option) => (
                      <ToggleGroupItem
                        key={option.id}
                        value={option.id}
                        aria-label={tw(`projectEditor.colors.${option.id}`)}
                        className="space-palette-option"
                      >
                        <span
                          style={{ background: option.background }}
                          aria-hidden="true"
                        />
                        <span>{tw(`projectEditor.colors.${option.id}`)}</span>
                        {option.id === color && <Check aria-hidden="true" />}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                </Field>
              </FieldGroup>
            </ScrollArea>
          </div>
          <DialogFooter>
            <p className="tree-open-demo">{t("appearanceStorage")}</p>
            <Button type="button" variant="ghost" onClick={onClose}>
              {t("cancel")}
            </Button>
            <Button type="submit" disabled={invalid}>
              {t("saveAppearance")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
