// W09 acceptance fixture. Loaded explicitly with `pi -e`; never installed globally.
export default function (pi: {
  registerCommand: (
    name: string,
    command: {
      description: string
      handler: (args: string, ctx: { ui: { confirm: (title: string, message: string) => Promise<boolean> } }) => Promise<void>
    }
  ) => void
}) {
  pi.registerCommand("yuzora-w09-block", {
    description: "Open a blocking confirmation prompt for Yuzora W09 acceptance",
    async handler(_args, ctx) {
      await ctx.ui.confirm(
        "Yuzora W09 blocked",
        "This temporary acceptance prompt verifies blocked then idle lifecycle projection."
      )
    }
  })
}
