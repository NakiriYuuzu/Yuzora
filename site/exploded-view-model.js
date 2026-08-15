export const PHASE_STOPS = [0, 0.14, 0.3, 0.46, 0.62, 0.78, 0.94];

export const MEDIA_BY_PHASE = Object.freeze({
  2: "ade-herdr",
  3: "ade-herdr",
  4: "remote-db",
  5: "terminal-git",
});

export function clamp(value, min = 0, max = 1) {
  return Math.min(max, Math.max(min, value));
}

function smooth(value) {
  const progress = clamp(value);
  return progress * progress * (3 - 2 * progress);
}

function lerp(from, to, amount) {
  return from + (to - from) * amount;
}

export function sampleTrack(track, progress, stops = PHASE_STOPS) {
  let segment = stops.length - 2;
  for (let index = 0; index < stops.length - 1; index += 1) {
    if (progress <= stops[index + 1]) {
      segment = index;
      break;
    }
  }
  const start = stops[segment];
  const end = stops[segment + 1];
  const amount = smooth((progress - start) / Math.max(end - start, 0.001));
  return track[segment].map((value, index) =>
    lerp(value, track[segment + 1][index], amount),
  );
}

export function resolvePhase(progress, stops = PHASE_STOPS) {
  let closest = 0;
  let distance = Infinity;
  stops.forEach((stop, index) => {
    const nextDistance = Math.abs(progress - stop);
    if (nextDistance < distance) {
      closest = index;
      distance = nextDistance;
    }
  });
  return closest;
}

export function mediaForPhase(phase) {
  return MEDIA_BY_PHASE[phase] ?? null;
}

export async function activateExclusiveMedia(
  videos,
  mediaName,
  { shouldPlay = true, onPlayError = () => {} } = {},
) {
  let activeVideo = null;
  for (const video of videos) {
    const isActive = video.dataset.media === mediaName;
    video.hidden = !isActive;
    if (!isActive || !shouldPlay) {
      video.pause();
      continue;
    }

    activeVideo = video;
    try {
      await video.play();
    } catch (error) {
      onPlayError(error, video);
    }
  }
  return activeVideo;
}
