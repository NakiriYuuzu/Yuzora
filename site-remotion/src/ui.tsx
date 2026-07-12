import React from "react";
import { Easing, interpolate, useCurrentFrame } from "remotion";
import { EASE } from "./theme";

// 進場：opacity + translateY（app 的 yzslideup / yzpop 節奏）
export const useRise = (from: number, duration = 12, dy = 10) => {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [from, from + duration], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.bezier(...EASE),
  });
  const translateY = interpolate(frame, [from, from + duration], [dy, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.bezier(...EASE),
  });
  return { opacity, translate: `0px ${translateY}px` };
};

// app 的 yzrowin：translateX(-5px) + fade（表格 row 進場）
export const useRowIn = (from: number, duration = 8) => {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [from, from + duration], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.bezier(...EASE),
  });
  const translateX = interpolate(frame, [from, from + duration], [-5, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.bezier(...EASE),
  });
  return { opacity, translate: `${translateX}px 0px` };
};

// typewriter：回傳到本 frame 為止該顯示的字串
export const typed = (
  frame: number,
  text: string,
  from: number,
  charsPerFrame = 0.9,
) => {
  const n = Math.max(0, Math.floor((frame - from) * charsPerFrame));
  return text.slice(0, n);
};

export const typedDone = (
  frame: number,
  text: string,
  from: number,
  charsPerFrame = 0.9,
) => frame - from >= text.length / charsPerFrame;

// 游標：app 的 yzblink（step-end）。streaming cursor 7×13 #5b3fd1 r2
export const Cursor: React.FC<{
  color: string;
  width?: number;
  height?: number;
}> = ({ color, width = 7, height = 13 }) => {
  const frame = useCurrentFrame();
  const on = Math.floor(frame / 16) % 2 === 0;
  return (
    <span
      style={{
        display: "inline-block",
        width,
        height,
        background: color,
        borderRadius: 2,
        opacity: on ? 1 : 0,
        verticalAlign: -2,
      }}
    />
  );
};

// app 的 yzpulse：狀態點呼吸
export const PulseDot: React.FC<{ color: string; size?: number }> = ({
  color,
  size = 8,
}) => {
  const frame = useCurrentFrame();
  const p = (frame % 42) / 42; // 1.4s @30fps
  const opacity = interpolate(p, [0, 0.5, 1], [0.5, 1, 0.5]);
  const scale = interpolate(p, [0, 0.5, 1], [1, 1.3, 1]);
  return (
    <span
      style={{
        display: "inline-block",
        width: size,
        height: size,
        borderRadius: "50%",
        background: color,
        opacity,
        scale: String(scale),
      }}
    />
  );
};
