import { useTranslation } from "react-i18next"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { useUpdateStore } from "@/state/updateStore"
import type { UpdateChannel } from "@/lib/updateChannel"
import { SettingCard } from "./settingsPrimitives"

export function UpdateChannelSettings() {
    const { t } = useTranslation("updates")
    const channel = useUpdateStore(s => s.channel)
    const setChannel = useUpdateStore(s => s.setChannel)
    const busy = useUpdateStore(s => s.status === "downloading" || s.status === "installing")
    return <SettingCard label={t("channelLabel")} sub={t("channelDescription")}>
        <Select value={channel} disabled={busy} onValueChange={v => setChannel(v as UpdateChannel)}>
            <SelectTrigger aria-label={t("channelLabel")}><SelectValue /></SelectTrigger>
            <SelectContent><SelectGroup>{(["auto", "stable", "preview"] as const).map(id => <SelectItem key={id} value={id}>{t(`channels.${id}`)}</SelectItem>)}</SelectGroup></SelectContent>
        </Select>
        <p className="mt-2 text-xs text-muted-foreground">{t("legacyPreviewNote")}</p>
    </SettingCard>
}
