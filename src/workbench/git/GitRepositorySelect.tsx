import { FolderGit2 } from "lucide-react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import type { DiscoveredRepository } from "@/lib/types"
import { useGitStore } from "@/state/gitStore"

// Select values must be non-empty; this stands for the workspace's own repository.
const WORKSPACE_VALUE = "\u0000workspace"

function optionValue(repository: DiscoveredRepository): string {
    return repository.relativePath || WORKSPACE_VALUE
}

function optionLabel(repository: DiscoveredRepository): string {
    return repository.relativePath || repository.name
}

/**
 * Switches the active repository of a multi-repository workspace. Hidden when
 * the workspace has a single repository and no nested one is selected.
 */
export function GitRepositorySelect({ className = "" }: { className?: string }) {
    const { t } = useTranslation("menus")
    const repositories = useGitStore((s) => s.repositories)
    const repositoryPath = useGitStore((s) => s.repositoryPath)
    const busy = useGitStore((s) => s.busy)
    const selectRepository = useGitStore((s) => s.selectRepository)
    if (!repositories?.length || (repositories.length < 2 && !repositoryPath)) return null

    const selected = repositoryPath ?? WORKSPACE_VALUE
    const known = repositories.some((repository) => optionValue(repository) === selected)
    return (
        <Select
            value={known ? selected : undefined}
            disabled={busy != null}
            onValueChange={(value) => void selectRepository(value === WORKSPACE_VALUE ? null : value)}
        >
            <SelectTrigger
                size="sm"
                aria-label={t("gitRepository.selectAriaLabel")}
                title={t("gitRepository.selectAriaLabel")}
                className={`h-[28px] min-w-0 max-w-[220px] gap-[6px] text-[11.5px] ${className}`}
            >
                <FolderGit2 aria-hidden="true" className="size-[13px] shrink-0 text-(--ink-3)" />
                <SelectValue placeholder={t("gitRepository.placeholder")} />
            </SelectTrigger>
            <SelectContent>
                <SelectGroup>
                    {repositories.map((repository) => (
                        <SelectItem key={optionValue(repository)} value={optionValue(repository)}>
                            <span className="truncate font-mono">{optionLabel(repository)}</span>
                            {repository.linked && (
                                <span className="text-[10px] text-(--ink-3)">{t("gitRepository.linked")}</span>
                            )}
                        </SelectItem>
                    ))}
                </SelectGroup>
            </SelectContent>
        </Select>
    )
}

/** Empty-state list for a folder that contains repositories but is not one. */
export function GitRepositoryList() {
    const { t } = useTranslation("menus")
    const repositories = useGitStore((s) => s.repositories)
    const truncated = useGitStore((s) => s.repositoriesTruncated)
    const busy = useGitStore((s) => s.busy)
    const selectRepository = useGitStore((s) => s.selectRepository)
    const nested = repositories?.filter((repository) => repository.relativePath !== "") ?? []
    if (!nested.length) return null
    return (
        <div className="flex w-full max-w-[320px] flex-col gap-[4px]">
            <p className="text-[11px] text-(--ink-3)">{t("gitRepository.found", { count: nested.length })}</p>
            <ul aria-label={t("gitRepository.listAriaLabel")} className="flex flex-col gap-[2px]">
                {nested.map((repository) => (
                    <li key={repository.relativePath}>
                        <Button
                            type="button"
                            variant="ghost"
                            size="xs"
                            disabled={busy != null}
                            className="w-full justify-start gap-[6px] font-mono text-[11.5px]"
                            onClick={() => void selectRepository(repository.relativePath)}
                        >
                            <FolderGit2 aria-hidden="true" className="size-[12px] shrink-0 text-(--ink-3)" />
                            <span className="truncate">{repository.relativePath}</span>
                        </Button>
                    </li>
                ))}
            </ul>
            {truncated && <p className="text-[10px] text-(--ink-3)">{t("gitRepository.truncated")}</p>}
        </div>
    )
}
