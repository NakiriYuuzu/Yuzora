import { useState } from "react"
import { useTranslation } from "react-i18next"
import { RotateCcw, Search } from "lucide-react"
import { APP_COMMANDS, bindingError, bindingLabel, defaultBindingFor, useKeyboardSettingsStore, type AppCommandId } from "@/state/keyboardSettingsStore"
import { Input } from "@/components/ui/input"
import { Button } from "@/components/ui/button"
import { Field, FieldContent, FieldLabel, FieldDescription, FieldGroup } from "@/components/ui/field"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group"
import { Kbd } from "@/components/ui/kbd"
import { SettingsRowGroup } from "./settingsPrimitives"

function BindingField({ id, binding }: { id: AppCommandId; binding: string }) {
    const { t } = useTranslation("editorPreferences")
    const [draft, setDraft] = useState(binding)
    const overrides = useKeyboardSettingsStore(s => s.overrides)
    const setBinding = useKeyboardSettingsStore(s => s.setBinding)
    const error = bindingError(id, draft, overrides)
    return <Field orientation="horizontal" data-invalid={!!error} className="settings-shortcut-row">
        <FieldContent>
            <FieldLabel htmlFor={`binding-${id}`}>{t(`commands.${id}`)}</FieldLabel>
            <FieldDescription>{error ? t(`errors.${error}`) : <Kbd className="settings-shortcut-kbd">{bindingLabel(binding)}</Kbd>}</FieldDescription>
        </FieldContent>
        <div className="settings-shortcut-edit">
            <Input id={`binding-${id}`} value={draft} data-shortcut-capture aria-invalid={!!error} onChange={e => setDraft(e.target.value)} className="font-mono" />
            <Button variant="outline" size="sm" disabled={!!error || draft === binding} onClick={() => setBinding(id, draft)}>{t("apply")}</Button>
        </div>
    </Field>
}

export function KeyboardSettings() {
    const { t } = useTranslation("editorPreferences")
    const [search, setSearch] = useState("")
    const overrides = useKeyboardSettingsStore(s => s.overrides)
    const reset = useKeyboardSettingsStore(s => s.reset)
    const commands = APP_COMMANDS.filter(c => `${t(`commands.${c.id}`)} ${c.id}`.toLocaleLowerCase().includes(search.toLocaleLowerCase()))
    return <FieldGroup className="settings-fields">
        <p className="settings-inline-hint settings-keyboard-scope">{t("keyboardScope")}</p>
        <div className="settings-keyboard-toolbar">
            <InputGroup className="settings-keyboard-search">
                <InputGroupAddon><Search aria-hidden="true" /></InputGroupAddon>
                <InputGroupInput id="shortcut-search" aria-label={t("search")} placeholder={t("search")} value={search} onChange={e => setSearch(e.target.value)} />
            </InputGroup>
            <Button variant="outline" size="sm" onClick={reset}><RotateCcw aria-hidden="true" />{t("reset")}</Button>
        </div>
        {commands.length > 0 && <SettingsRowGroup>
            {commands.map(c => <BindingField key={`${c.id}-${overrides[c.id] ?? defaultBindingFor(c.id)}`} id={c.id} binding={overrides[c.id] ?? defaultBindingFor(c.id)} />)}
        </SettingsRowGroup>}
    </FieldGroup>
}
