import { forwardRef } from "react"
import { PanelLeftClose, PanelLeftOpen } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
    Tooltip,
    TooltipContent,
    TooltipProvider,
    TooltipTrigger
} from "@/components/ui/tooltip"

export const DiffFilesToggle = forwardRef<HTMLButtonElement, {
    collapsed: boolean
    label: string
    controlsId: string
    onToggle: () => void
}>(function DiffFilesToggle({ collapsed, label, controlsId, onToggle }, ref) {
    const Icon = collapsed ? PanelLeftOpen : PanelLeftClose
    return (
        <TooltipProvider>
            <Tooltip>
                <TooltipTrigger asChild>
                    <Button
                        ref={ref}
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        data-testid="diff-files-toggle"
                        aria-expanded={!collapsed}
                        aria-controls={controlsId}
                        aria-label={label}
                        onClick={onToggle}
                        className="shrink-0 text-(--ink-3) hover:text-(--ink-1)"
                    >
                        <Icon aria-hidden="true" />
                    </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom" data-testid="diff-files-toggle-tooltip" className="z-[70]">{label}</TooltipContent>
            </Tooltip>
        </TooltipProvider>
    )
})
