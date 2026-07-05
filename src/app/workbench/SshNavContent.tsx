import { Server } from "lucide-react"

import { DashedActionButton } from "@/app/workbench/DashedActionButton"
import { EmptyState } from "@/app/workbench/EmptyState"

/**
 * SSH mode nav content — empty hosts list. Reference §7 defines vocabulary
 * for a populated host list ("Hosts") but no empty-state copy, so this
 * follows the Files/Database nav pattern. Reference §8 gap 9: SFTP and SSH
 * are two tabs of one host entry, not separate nav items — hence a single
 * "New host" action here.
 */
export function SshNavContent() {
  return (
    <div className="flex h-full flex-col gap-[10px]">
      <div className="flex flex-1 items-center justify-center">
        <EmptyState
          icon={Server}
          title="No hosts yet"
          description="Add a host to connect over SSH or SFTP"
        />
      </div>
      <DashedActionButton
        label="New host"
        onClick={() => {
          /* no-op placeholder — host setup lands in a later task */
        }}
      />
    </div>
  )
}
