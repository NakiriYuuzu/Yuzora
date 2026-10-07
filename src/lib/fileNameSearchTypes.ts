import type { FileNode } from "./types"

export interface FileNameSearchResponse {
  files: FileNode[]
  incomplete: boolean
}
