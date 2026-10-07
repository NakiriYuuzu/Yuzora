import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { initialGitState, useGitStore } from "@/state/gitStore"
import { GitRepositoryList, GitRepositorySelect } from "./GitRepositorySelect"

const repo = (relativePath: string, linked = false) => ({
    relativePath,
    name: relativePath.split("/").pop() || "workspace",
    linked
})

describe("GitRepositorySelect", () => {
    afterEach(() => useGitStore.setState(initialGitState))

    it("stays hidden for a single repository workspace", () => {
        useGitStore.setState({ repositories: [repo("")], repositoryPath: null })
        render(<GitRepositorySelect />)
        expect(screen.queryByRole("combobox", { name: "Git repository" })).toBeNull()
    })

    it("shows the active nested repository", () => {
        useGitStore.setState({ repositories: [repo(""), repo("apps/api")], repositoryPath: "apps/api" })
        render(<GitRepositorySelect />)
        expect(screen.getByRole("combobox", { name: "Git repository" })).toHaveTextContent("apps/api")
    })

    it("stays visible for a lone nested repository so the user sees which one is open", () => {
        useGitStore.setState({ repositories: [repo("api")], repositoryPath: "api" })
        render(<GitRepositorySelect />)
        expect(screen.getByRole("combobox", { name: "Git repository" })).toHaveTextContent("api")
    })
})

describe("GitRepositoryList", () => {
    afterEach(() => useGitStore.setState(initialGitState))

    it("opens a nested repository from the empty state", () => {
        const selectRepository = vi.fn(async () => undefined)
        useGitStore.setState({ repositories: [repo("api"), repo("libs/core", true)], selectRepository })
        render(<GitRepositoryList />)
        expect(screen.getByText("This folder contains 2 Git repositories:")).toBeInTheDocument()
        fireEvent.click(screen.getByRole("button", { name: "libs/core" }))
        expect(selectRepository).toHaveBeenCalledWith("libs/core")
    })

    it("renders nothing without nested repositories", () => {
        useGitStore.setState({ repositories: [repo("")] })
        const { container } = render(<GitRepositoryList />)
        expect(container).toBeEmptyDOMElement()
    })
})
