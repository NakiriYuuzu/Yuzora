import { FolderSync, TerminalSquare } from "lucide-react"

import { EmptyState } from "@/app/workbench/EmptyState"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"

/**
 * SSH mode main region — design reference 5.7. Reference §8 gap 9: SFTP and
 * SSH are two tabs of a single host entry, not separate modes, hence the
 * segmented switch up top. No host is connected yet, so both views are an
 * empty state (no fake file listing or terminal output).
 */
export function SshPanel() {
  return (
    <div className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)">
      <Tabs defaultValue="sftp" className="min-h-0 flex-1 gap-0">
        <div className="flex shrink-0 items-center justify-center border-b border-(--line-1) px-[10px] py-[10px]">
          <TabsList aria-label="SFTP or SSH" className="group-data-horizontal/tabs:h-[26px] rounded-(--r-pill) bg-(--paper-2) p-[3px]">
            <TabsTrigger
              value="sftp"
              className="rounded-(--r-pill) px-[12px] text-[11.5px] font-medium data-active:bg-(--yz-solid) data-active:shadow-(--shadow-xs)"
            >
              SFTP
            </TabsTrigger>
            <TabsTrigger
              value="ssh"
              className="rounded-(--r-pill) px-[12px] text-[11.5px] font-medium data-active:bg-(--yz-solid) data-active:shadow-(--shadow-xs)"
            >
              SSH
            </TabsTrigger>
          </TabsList>
        </div>

        <TabsContent value="sftp" className="flex min-h-0 flex-1 items-center justify-center">
          <EmptyState
            icon={FolderSync}
            title="Remote sessions are not configured"
            description="Connect a host to transfer files here."
          />
        </TabsContent>

        <TabsContent value="ssh" className="flex min-h-0 flex-1 items-center justify-center">
          <EmptyState
            icon={TerminalSquare}
            title="Remote sessions are not configured"
            description="Connect a host to open a terminal session here."
          />
        </TabsContent>
      </Tabs>
    </div>
  )
}
