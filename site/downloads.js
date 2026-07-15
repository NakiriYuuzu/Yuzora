const RELEASE_DOWNLOAD_BASE = "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/"

const DOWNLOADS = {
  macos: {
    platform: "macos",
    url: `${RELEASE_DOWNLOAD_BASE}Yuzora-macos-universal.dmg`,
  },
  windows: {
    platform: "windows",
    url: `${RELEASE_DOWNLOAD_BASE}Yuzora-windows-x64-setup.exe`,
  },
  linux: {
    platform: "linux",
    url: `${RELEASE_DOWNLOAD_BASE}Yuzora-linux-x86_64.AppImage`,
  },
}

export function resolveDownloadTarget({
  userAgentData,
  platform = "",
  userAgent = "",
  maxTouchPoints = 0,
} = {}) {
  const reportedPlatform = `${userAgentData?.platform ?? ""} ${platform} ${userAgent}`
  const architecture = userAgentData?.architecture?.toLowerCase() ?? ""
  const bitness = userAgentData?.bitness ?? ""
  const architectureDescription = `${architecture} ${reportedPlatform}`
  const hasIncompatibleX64Architecture =
    /\b(?:arm(?:64|v\d+l?)?|aarch64)\b/i.test(architectureDescription) ||
    /\bi[3-6]86\b/i.test(architectureDescription) ||
    bitness === "32"
  const isIPad = /mac/i.test(platform) && maxTouchPoints > 1
  const isUnsupportedDevice =
    userAgentData?.mobile === true ||
    /android|iphone|ipad|ipod|cros|chrome os/i.test(reportedPlatform) ||
    isIPad

  if (isUnsupportedDevice) {
    return { status: "unsupported", platform: null, url: null }
  }

  if (/mac/i.test(reportedPlatform)) {
    return { status: "supported", ...DOWNLOADS.macos }
  }

  if (/win/i.test(reportedPlatform)) {
    if (hasIncompatibleX64Architecture) {
      return { status: "unsupported-architecture", platform: "windows", url: null }
    }
    return { status: "supported", ...DOWNLOADS.windows }
  }

  if (/linux/i.test(reportedPlatform)) {
    if (hasIncompatibleX64Architecture) {
      return { status: "unsupported-architecture", platform: "linux", url: null }
    }
    return { status: "supported", ...DOWNLOADS.linux }
  }

  return { status: "unknown", platform: null, url: null }
}

export async function detectDownloadTarget(navigatorLike = {}) {
  const userAgentData = navigatorLike.userAgentData
  let highEntropyValues = {}

  if (typeof userAgentData?.getHighEntropyValues === "function") {
    try {
      highEntropyValues = await userAgentData.getHighEntropyValues(["architecture", "bitness"])
    } catch {
      highEntropyValues = {}
    }
  }

  return resolveDownloadTarget({
    userAgent: navigatorLike.userAgent,
    platform: navigatorLike.platform,
    maxTouchPoints: navigatorLike.maxTouchPoints,
    userAgentData: {
      platform: userAgentData?.platform,
      mobile: userAgentData?.mobile,
      ...highEntropyValues,
    },
  })
}

export function applyDownloadTarget(target, documentLike) {
  const primaryDownload = documentLike.querySelector("#primary-download")
  const messageKey = target.status === "supported" ? target.platform : target.status

  primaryDownload?.setAttribute(
    "href",
    target.status === "supported" && target.url ? target.url : "#download",
  )

  documentLike.querySelectorAll("[data-platform-download]").forEach((row) => {
    const isRecommended =
      target.status === "supported" && row.getAttribute("data-platform-download") === target.platform

    row.classList.toggle("is-recommended", isRecommended)
    if (isRecommended) row.setAttribute("aria-current", "true")
    else row.removeAttribute("aria-current")

    const badge = row.querySelector("[data-recommended-badge]")
    if (badge) badge.toggleAttribute("hidden", !isRecommended)
  })

  const deviceNote = documentLike.querySelector("#download-device-note")
  if (deviceNote) {
    deviceNote.setAttribute("data-status", target.status)
    if (target.platform) deviceNote.setAttribute("data-platform", target.platform)
    else deviceNote.removeAttribute("data-platform")
  }

  documentLike.querySelectorAll("[data-device-message]").forEach((message) => {
    message.toggleAttribute("hidden", message.getAttribute("data-device-message") !== messageKey)
  })
}

export async function initDownloadExperience(navigatorLike, documentLike) {
  const target = await detectDownloadTarget(navigatorLike)
  applyDownloadTarget(target, documentLike)
  return target
}
