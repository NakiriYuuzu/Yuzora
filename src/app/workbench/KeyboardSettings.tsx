import { useState } from "react"
import { useTranslation } from "react-i18next"
import { APP_COMMANDS, bindingError, bindingLabel, useKeyboardSettingsStore, type AppCommandId } from "@/state/keyboardSettingsStore"
import { Input } from "@/components/ui/input"
import { Button } from "@/components/ui/button"
import { Field, FieldLabel, FieldDescription, FieldGroup } from "@/components/ui/field"

function BindingField({ id, binding }: { id: AppCommandId; binding: string }) {
    const { t } = useTranslation("editorPreferences")
    const [draft, setDraft] = useState(binding)
    const overrides = useKeyboardSettingsStore(s => s.overrides)
    const setBinding = useKeyboardSettingsStore(s => s.setBinding)
    const error = bindingError(id, draft, overrides)
    return <Field data-invalid={!!error}>
        <FieldLabel htmlFor={`binding-${id}`}>{t(`commands.${id}`)}</FieldLabel>
        <div className="flex gap-2">
            <Input id={`binding-${id}`} value={draft} data-shortcut-capture aria-invalid={!!error} onChange={e => setDraft(e.target.value)} />
            <Button variant="outline" disabled={!!error || draft === binding} onClick={() => setBinding(id, draft)}>{t("apply")}</Button>
        </div>
        <FieldDescription>{error ? t(`errors.${error}`) : bindingLabel(binding)}</FieldDescription>
    </Field>
}

export function KeyboardSettings() {
    const { t } = useTranslation("editorPreferences")
    const [search, setSearch] = useState("")
    const overrides = useKeyboardSettingsStore(s => s.overrides)
    const reset = useKeyboardSettingsStore(s => s.reset)
    return <FieldGroup>
        <p className="text-sm text-muted-foreground">{t("keyboardScope")}</p>
        <Field><FieldLabel htmlFor="shortcut-search">{t("search")}</FieldLabel><Input id="shortcut-search" value={search} onChange={e => setSearch(e.target.value)} /></Field>
        {APP_COMMANDS.filter(c => `${t(`commands.${c.id}`)} ${c.id}`.toLocaleLowerCase().includes(search.toLocaleLowerCase())).map(c => <BindingField key={`${c.id}-${overrides[c.id] ?? c.defaultBinding}`} id={c.id} binding={overrides[c.id] ?? c.defaultBinding} />)}
        <Button variant="outline" onClick={reset}>{t("reset")}</Button>
    </FieldGroup>
}
