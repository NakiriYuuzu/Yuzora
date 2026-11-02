# Release-note clips

On Linux, run `just notes-clip pane-drag` or `just notes-clip peek`.
An optional second argument selects the version (otherwise Cargo.toml).
Requires Bun, ffmpeg, ImageMagick, Weston/Xwayland and the sandbox's uv/python-xlib input.
Each run builds the app, creates its own sandbox, records app-rendered PNGs
at 12 fps for five seconds, then removes the sandbox and temporary frames.
The looped lossless WebP starts at 1200px wide and must fit within 1,500,000
bytes; oversized scenes retry at 1100px then 960px without losing motion.
Scenes use the shipped Main layout, then crop to the action: the pane headers
and terminal content, or the agent message and peek overlay.
A normal arrow, pointing hand or closed hand is composited onto each app
frame at the scripted pointer's recorded position after capture, preserving
the recording cadence. Hover, drag preview and drop highlights are real.

Scenes are shell scripts with `setup`, `play` and `crop` stages. The last
prints an ffmpeg crop (`width:height:x:y`) in the original 1280×820 frame. Setup may use app
commands. Play uses `sandbox sequence` with JSON actions (sleep, move,
click, hold/release keys, press/lift mouse); one XTest connection preserves
modifiers and smooth pointer movement. Coordinates match the sandbox's
1280×820 app window at its default 120% zoom. Update these when UI changes,
and inspect the generated clip before uploading.

Output: `/var/tmp/herdr-notes-media/<version>/<scene>.webp`.
Override the base with `HERDR_NOTES_MEDIA_DIR`. Launch the app with that
same variable to preview CHANGELOG images pointing at the final URLs:
`https://media.herdr.dev/<version>/<scene>.webp`. No CHANGELOG rewrite needed.

`just notes-upload <version>` uploads the version's WebP files to
`herdr-media`, using Bun's S3Client and credentials in
`~/.config/herdr-media/env`:
`R2_MEDIA_ACCESS_KEY_ID=...` and `R2_MEDIA_SECRET_ACCESS_KEY=...`.
Never commit that file. Uploads set image/webp and immutable one-year caching.
Existing keys are refused, including concurrent writes. Upload only final
clips: immutable URLs must never be reused for changed content.

`just notes-check <version>` checks every image in that CHANGELOG entry
for HTTP 200 and an image content type. The release publish job runs it
before any release files or manifest are uploaded. An entry without media
is valid; a missing entry isn't.
