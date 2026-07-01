/* @ds-bundle: {"format":3,"namespace":"TheBrowserCompanyDesignSystem_6aff16","components":[{"name":"Avatar","sourcePath":"components/core/Avatar.jsx"},{"name":"Badge","sourcePath":"components/core/Badge.jsx"},{"name":"Button","sourcePath":"components/core/Button.jsx"},{"name":"Card","sourcePath":"components/core/Card.jsx"},{"name":"IconButton","sourcePath":"components/core/IconButton.jsx"},{"name":"Input","sourcePath":"components/core/Input.jsx"},{"name":"Switch","sourcePath":"components/core/Switch.jsx"},{"name":"Tabs","sourcePath":"components/core/Tabs.jsx"},{"name":"Tag","sourcePath":"components/core/Tag.jsx"},{"name":"Tooltip","sourcePath":"components/core/Tooltip.jsx"}],"sourceHashes":{"components/core/Avatar.jsx":"e23b630e9dac","components/core/Badge.jsx":"b18ee3bca914","components/core/Button.jsx":"c715c41fb51f","components/core/Card.jsx":"7e6790d730f8","components/core/IconButton.jsx":"0278fc742249","components/core/Input.jsx":"626b53f0389c","components/core/Switch.jsx":"515502b8940a","components/core/Tabs.jsx":"29a9706666ff","components/core/Tag.jsx":"c9f224648f4c","components/core/Tooltip.jsx":"c3e9a2bda4a9","ui_kits/arc/ArcBrowser.jsx":"3de92ac569f8","ui_kits/arc/CommandBar.jsx":"6e0e980e9db6","ui_kits/arc/Icons.jsx":"5b7de31d7f81","ui_kits/arc/Sidebar.jsx":"393a2fd9cf26","ui_kits/arc/WebView.jsx":"d6ac34020ae2","ui_kits/dia/Assistant.jsx":"aa1bd3c464b6","ui_kits/dia/Chrome.jsx":"71271ef87951","ui_kits/dia/DiaBrowser.jsx":"f88660233379"},"inlinedExternals":[],"unexposedExports":[]} */

(() => {

const __ds_ns = (window.TheBrowserCompanyDesignSystem_6aff16 = window.TheBrowserCompanyDesignSystem_6aff16 || {});

const __ds_scope = {};

(__ds_ns.__errors = __ds_ns.__errors || []);

// components/core/Avatar.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/** Avatar — gradient-filled initial or image, round. */
function Avatar({
  src = null,
  name = "",
  size = 36,
  gradient = "var(--grad-arc)",
  style = {},
  ...rest
}) {
  const initials = name.split(" ").map(w => w[0]).filter(Boolean).slice(0, 2).join("").toUpperCase();
  return /*#__PURE__*/React.createElement("span", _extends({
    style: {
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      width: size,
      height: size,
      borderRadius: "50%",
      background: src ? "transparent" : gradient,
      color: "#fff",
      fontFamily: "var(--font-sans)",
      fontWeight: "var(--fw-semibold)",
      fontSize: size * 0.38,
      letterSpacing: "0.01em",
      overflow: "hidden",
      flexShrink: 0,
      boxShadow: "inset 0 0 0 1px rgba(255,255,255,0.25)",
      ...style
    }
  }, rest), src ? /*#__PURE__*/React.createElement("img", {
    src: src,
    alt: name,
    style: {
      width: "100%",
      height: "100%",
      objectFit: "cover"
    }
  }) : initials || "?");
}
Object.assign(__ds_scope, { Avatar });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Avatar.jsx", error: String((e && e.message) || e) }); }

// components/core/Badge.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/** Badge — small status/count pill in soft tinted colors. */
function Badge({
  tone = "neutral",
  size = "md",
  dot = false,
  style = {},
  children,
  ...rest
}) {
  const tones = {
    neutral: {
      background: "var(--paper-3)",
      color: "var(--ink-2)"
    },
    accent: {
      background: "var(--blue-soft)",
      color: "var(--accent-press)"
    },
    success: {
      background: "var(--mint-soft)",
      color: "#0f7a55"
    },
    warning: {
      background: "var(--amber-soft)",
      color: "#9a6512"
    },
    danger: {
      background: "var(--danger-soft)",
      color: "#b51f38"
    },
    coral: {
      background: "var(--coral-soft)",
      color: "#c63b22"
    },
    violet: {
      background: "var(--violet-soft)",
      color: "#5b3fd1"
    }
  };
  const sizes = {
    sm: {
      fontSize: "var(--fs-micro)",
      padding: "2px 8px",
      height: 18
    },
    md: {
      fontSize: "var(--fs-xs)",
      padding: "3px 10px",
      height: 22
    }
  };
  const t = tones[tone];
  return /*#__PURE__*/React.createElement("span", _extends({
    style: {
      display: "inline-flex",
      alignItems: "center",
      gap: 6,
      height: sizes[size].height,
      padding: sizes[size].padding,
      fontSize: sizes[size].fontSize,
      fontFamily: "var(--font-sans)",
      fontWeight: "var(--fw-semibold)",
      letterSpacing: "-0.004em",
      borderRadius: "var(--r-pill)",
      whiteSpace: "nowrap",
      ...t,
      ...style
    }
  }, rest), dot && /*#__PURE__*/React.createElement("span", {
    style: {
      width: 6,
      height: 6,
      borderRadius: "50%",
      background: "currentColor"
    }
  }), children);
}
Object.assign(__ds_scope, { Badge });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Badge.jsx", error: String((e && e.message) || e) }); }

// components/core/Button.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Button — The Browser Company's primary action control.
 * Sentence-case labels, rounded/pill geometry, springy press.
 */
function Button({
  variant = "primary",
  size = "md",
  pill = false,
  fullWidth = false,
  disabled = false,
  leadingIcon = null,
  trailingIcon = null,
  style = {},
  children,
  ...rest
}) {
  const heights = {
    sm: "var(--control-h-sm)",
    md: "var(--control-h-md)",
    lg: "var(--control-h-lg)"
  };
  const pads = {
    sm: "0 14px",
    md: "0 18px",
    lg: "0 24px"
  };
  const fontSizes = {
    sm: "var(--fs-sm)",
    md: "var(--fs-body)",
    lg: "var(--fs-title)"
  };
  const variants = {
    primary: {
      background: "var(--ink-1)",
      color: "var(--paper-0)",
      border: "1px solid transparent",
      boxShadow: "var(--shadow-sm)"
    },
    gradient: {
      background: "var(--grad-arc)",
      color: "#fff",
      border: "1px solid transparent",
      boxShadow: "var(--glow-accent)"
    },
    secondary: {
      background: "var(--paper-0)",
      color: "var(--ink-1)",
      border: "1px solid var(--line-2)",
      boxShadow: "var(--shadow-xs)"
    },
    ghost: {
      background: "transparent",
      color: "var(--ink-1)",
      border: "1px solid transparent",
      boxShadow: "none"
    },
    danger: {
      background: "var(--danger)",
      color: "#fff",
      border: "1px solid transparent",
      boxShadow: "var(--shadow-sm)"
    }
  };
  return /*#__PURE__*/React.createElement("button", _extends({
    disabled: disabled,
    style: {
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      gap: "8px",
      height: heights[size],
      padding: pads[size],
      width: fullWidth ? "100%" : "auto",
      font: "var(--text-body)",
      fontSize: fontSizes[size],
      fontWeight: "var(--fw-semibold)",
      letterSpacing: "-0.006em",
      whiteSpace: "nowrap",
      flexShrink: 0,
      borderRadius: pill ? "var(--r-pill)" : "var(--r-md)",
      cursor: disabled ? "not-allowed" : "pointer",
      opacity: disabled ? 0.45 : 1,
      transition: "transform var(--dur-fast) var(--ease-spring), box-shadow var(--dur-mid) var(--ease-out), background var(--dur-mid) var(--ease-out)",
      WebkitTapHighlightColor: "transparent",
      ...variants[variant],
      ...style
    },
    onMouseDown: e => {
      if (!disabled) e.currentTarget.style.transform = "scale(0.97)";
    },
    onMouseUp: e => {
      e.currentTarget.style.transform = "scale(1)";
    },
    onMouseLeave: e => {
      e.currentTarget.style.transform = "scale(1)";
    }
  }, rest), leadingIcon, children, trailingIcon);
}
Object.assign(__ds_scope, { Button });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Button.jsx", error: String((e && e.message) || e) }); }

// components/core/Card.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Card — warm surface container. Shadow defines the edge; no border
 * by default. Optional gradient header strip for "Space"-style cards.
 */
function Card({
  elevation = "sm",
  padding = "lg",
  gradientStrip = null,
  interactive = false,
  style = {},
  children,
  ...rest
}) {
  const shadows = {
    flat: "none",
    xs: "var(--shadow-xs)",
    sm: "var(--shadow-sm)",
    md: "var(--shadow-md)",
    lg: "var(--shadow-lg)"
  };
  const pads = {
    none: "0",
    sm: "var(--sp-3)",
    md: "var(--sp-4)",
    lg: "var(--sp-5)",
    xl: "var(--sp-8)"
  };
  return /*#__PURE__*/React.createElement("div", _extends({
    style: {
      position: "relative",
      background: "var(--paper-0)",
      borderRadius: "var(--r-lg)",
      boxShadow: shadows[elevation],
      border: elevation === "flat" ? "1px solid var(--line-1)" : "none",
      overflow: "hidden",
      transition: "transform var(--dur-mid) var(--ease-out), box-shadow var(--dur-mid) var(--ease-out)",
      cursor: interactive ? "pointer" : "default",
      ...style
    },
    onMouseEnter: e => {
      if (interactive) {
        e.currentTarget.style.transform = "translateY(-2px)";
        e.currentTarget.style.boxShadow = "var(--shadow-md)";
      }
    },
    onMouseLeave: e => {
      if (interactive) {
        e.currentTarget.style.transform = "translateY(0)";
        e.currentTarget.style.boxShadow = shadows[elevation];
      }
    }
  }, rest), gradientStrip && /*#__PURE__*/React.createElement("div", {
    style: {
      height: 6,
      background: gradientStrip === true ? "var(--grad-arc)" : gradientStrip
    }
  }), /*#__PURE__*/React.createElement("div", {
    style: {
      padding: pads[padding]
    }
  }, children));
}
Object.assign(__ds_scope, { Card });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Card.jsx", error: String((e && e.message) || e) }); }

// components/core/IconButton.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * IconButton — square/round button for a single icon. Used heavily
 * in browser chrome (sidebar controls, toolbar actions).
 */
function IconButton({
  size = "md",
  variant = "ghost",
  round = false,
  active = false,
  disabled = false,
  label,
  style = {},
  children,
  ...rest
}) {
  const dims = {
    sm: 30,
    md: 36,
    lg: 44
  };
  const d = dims[size];
  const variants = {
    ghost: {
      background: active ? "var(--paper-3)" : "transparent",
      color: "var(--ink-1)"
    },
    solid: {
      background: "var(--paper-0)",
      color: "var(--ink-1)",
      boxShadow: "var(--shadow-xs)",
      border: "1px solid var(--line-1)"
    },
    night: {
      background: active ? "var(--night-3)" : "transparent",
      color: "var(--night-fg)"
    }
  };
  return /*#__PURE__*/React.createElement("button", _extends({
    "aria-label": label,
    title: label,
    disabled: disabled,
    style: {
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      width: d,
      height: d,
      borderRadius: round ? "var(--r-pill)" : "var(--r-sm)",
      border: "1px solid transparent",
      cursor: disabled ? "not-allowed" : "pointer",
      opacity: disabled ? 0.4 : 1,
      transition: "transform var(--dur-fast) var(--ease-spring), background var(--dur-mid) var(--ease-out)",
      ...variants[variant],
      ...style
    },
    onMouseEnter: e => {
      if (variant === "ghost" && !active) e.currentTarget.style.background = "var(--paper-2)";
    },
    onMouseLeave: e => {
      if (variant === "ghost" && !active) e.currentTarget.style.background = "transparent";
    },
    onMouseDown: e => {
      if (!disabled) e.currentTarget.style.transform = "scale(0.9)";
    },
    onMouseUp: e => {
      e.currentTarget.style.transform = "scale(1)";
    }
  }, rest), children);
}
Object.assign(__ds_scope, { IconButton });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/IconButton.jsx", error: String((e && e.message) || e) }); }

// components/core/Input.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Input — text field with optional leading icon. Warm hairline border
 * that thickens to accent on focus, with a soft focus ring.
 */
function Input({
  size = "md",
  leadingIcon = null,
  trailingIcon = null,
  invalid = false,
  disabled = false,
  fullWidth = true,
  style = {},
  ...rest
}) {
  const [focused, setFocused] = React.useState(false);
  const heights = {
    sm: "var(--control-h-sm)",
    md: "var(--control-h-md)",
    lg: "var(--control-h-lg)"
  };
  const borderColor = invalid ? "var(--danger)" : focused ? "var(--accent)" : "var(--line-2)";
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: "inline-flex",
      alignItems: "center",
      gap: "8px",
      height: heights[size],
      width: fullWidth ? "100%" : "auto",
      padding: "0 14px",
      background: disabled ? "var(--paper-2)" : "var(--paper-0)",
      border: `1.5px solid ${borderColor}`,
      borderRadius: "var(--r-md)",
      boxShadow: focused ? "var(--ring)" : "var(--shadow-xs)",
      transition: "border-color var(--dur-mid) var(--ease-out), box-shadow var(--dur-mid) var(--ease-out)",
      opacity: disabled ? 0.6 : 1,
      ...style
    }
  }, leadingIcon && /*#__PURE__*/React.createElement("span", {
    style: {
      display: "flex",
      color: "var(--ink-3)"
    }
  }, leadingIcon), /*#__PURE__*/React.createElement("input", _extends({
    disabled: disabled,
    onFocus: () => setFocused(true),
    onBlur: () => setFocused(false),
    style: {
      flex: 1,
      minWidth: 0,
      border: "none",
      outline: "none",
      background: "transparent",
      font: "var(--text-body)",
      fontSize: "var(--fs-body)",
      color: "var(--ink-1)"
    }
  }, rest)), trailingIcon && /*#__PURE__*/React.createElement("span", {
    style: {
      display: "flex",
      color: "var(--ink-3)"
    }
  }, trailingIcon));
}
Object.assign(__ds_scope, { Input });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Input.jsx", error: String((e && e.message) || e) }); }

// components/core/Switch.jsx
try { (() => {
/** Switch — springy pill toggle. */
function Switch({
  checked,
  defaultChecked = false,
  onChange,
  disabled = false,
  size = "md",
  style = {}
}) {
  const isControlled = checked !== undefined;
  const [internal, setInternal] = React.useState(defaultChecked);
  const on = isControlled ? checked : internal;
  const dims = {
    sm: {
      w: 36,
      h: 20,
      k: 14
    },
    md: {
      w: 46,
      h: 26,
      k: 20
    }
  };
  const d = dims[size];
  return /*#__PURE__*/React.createElement("button", {
    role: "switch",
    "aria-checked": on,
    disabled: disabled,
    onClick: () => {
      if (disabled) return;
      if (!isControlled) setInternal(!on);
      onChange && onChange(!on);
    },
    style: {
      position: "relative",
      width: d.w,
      height: d.h,
      flexShrink: 0,
      border: "none",
      borderRadius: "var(--r-pill)",
      background: on ? "var(--accent)" : "var(--paper-3)",
      cursor: disabled ? "not-allowed" : "pointer",
      opacity: disabled ? 0.5 : 1,
      transition: "background var(--dur-mid) var(--ease-out)",
      padding: 0,
      ...style
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      position: "absolute",
      top: (d.h - d.k) / 2,
      left: on ? d.w - d.k - (d.h - d.k) / 2 : (d.h - d.k) / 2,
      width: d.k,
      height: d.k,
      borderRadius: "50%",
      background: "#fff",
      boxShadow: "var(--shadow-sm)",
      transition: "left var(--dur-mid) var(--ease-spring)"
    }
  }));
}
Object.assign(__ds_scope, { Switch });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Switch.jsx", error: String((e && e.message) || e) }); }

// components/core/Tabs.jsx
try { (() => {
/** Tabs — underline-style segmented navigation, sentence case. */
function Tabs({
  tabs = [],
  value,
  defaultValue,
  onChange,
  style = {}
}) {
  const isControlled = value !== undefined;
  const [internal, setInternal] = React.useState(defaultValue ?? (tabs[0] && tabs[0].id));
  const active = isControlled ? value : internal;
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      gap: 4,
      borderBottom: "1px solid var(--line-1)",
      ...style
    }
  }, tabs.map(t => {
    const on = t.id === active;
    return /*#__PURE__*/React.createElement("button", {
      key: t.id,
      onClick: () => {
        if (!isControlled) setInternal(t.id);
        onChange && onChange(t.id);
      },
      style: {
        position: "relative",
        display: "inline-flex",
        alignItems: "center",
        gap: 7,
        padding: "10px 14px",
        marginBottom: -1,
        border: "none",
        background: "transparent",
        cursor: "pointer",
        font: "var(--text-body)",
        fontSize: "var(--fs-sm)",
        fontWeight: on ? "var(--fw-semibold)" : "var(--fw-medium)",
        color: on ? "var(--ink-1)" : "var(--ink-3)",
        borderBottom: `2px solid ${on ? "var(--ink-1)" : "transparent"}`,
        transition: "color var(--dur-mid) var(--ease-out)"
      }
    }, t.icon, t.label, t.count != null && /*#__PURE__*/React.createElement("span", {
      style: {
        fontSize: "var(--fs-micro)",
        fontWeight: "var(--fw-semibold)",
        color: "var(--ink-3)",
        background: "var(--paper-2)",
        borderRadius: "var(--r-pill)",
        padding: "1px 7px"
      }
    }, t.count));
  }));
}
Object.assign(__ds_scope, { Tabs });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Tabs.jsx", error: String((e && e.message) || e) }); }

// components/core/Tag.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/** Tag — removable pill, often colored to match a Space. */
function Tag({
  color = "var(--ink-3)",
  removable = false,
  onRemove,
  style = {},
  children,
  ...rest
}) {
  return /*#__PURE__*/React.createElement("span", _extends({
    style: {
      display: "inline-flex",
      alignItems: "center",
      gap: 7,
      height: 26,
      padding: removable ? "0 6px 0 12px" : "0 12px",
      background: "var(--paper-2)",
      color: "var(--ink-1)",
      fontFamily: "var(--font-sans)",
      fontSize: "var(--fs-sm)",
      fontWeight: "var(--fw-medium)",
      borderRadius: "var(--r-pill)",
      border: "1px solid var(--line-1)",
      ...style
    }
  }, rest), /*#__PURE__*/React.createElement("span", {
    style: {
      width: 8,
      height: 8,
      borderRadius: "50%",
      background: color,
      flexShrink: 0
    }
  }), children, removable && /*#__PURE__*/React.createElement("button", {
    onClick: onRemove,
    "aria-label": "Remove",
    style: {
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      width: 18,
      height: 18,
      border: "none",
      borderRadius: "var(--r-pill)",
      background: "transparent",
      color: "var(--ink-3)",
      cursor: "pointer",
      fontSize: 14,
      lineHeight: 1
    },
    onMouseEnter: e => e.currentTarget.style.background = "var(--paper-3)",
    onMouseLeave: e => e.currentTarget.style.background = "transparent"
  }, "\xD7"));
}
Object.assign(__ds_scope, { Tag });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Tag.jsx", error: String((e && e.message) || e) }); }

// components/core/Tooltip.jsx
try { (() => {
/** Tooltip — dark frosted bubble on hover. */
function Tooltip({
  label,
  side = "top",
  children,
  style = {}
}) {
  const [show, setShow] = React.useState(false);
  const pos = {
    top: {
      bottom: "calc(100% + 8px)",
      left: "50%",
      transform: "translateX(-50%)"
    },
    bottom: {
      top: "calc(100% + 8px)",
      left: "50%",
      transform: "translateX(-50%)"
    },
    left: {
      right: "calc(100% + 8px)",
      top: "50%",
      transform: "translateY(-50%)"
    },
    right: {
      left: "calc(100% + 8px)",
      top: "50%",
      transform: "translateY(-50%)"
    }
  };
  return /*#__PURE__*/React.createElement("span", {
    style: {
      position: "relative",
      display: "inline-flex"
    },
    onMouseEnter: () => setShow(true),
    onMouseLeave: () => setShow(false)
  }, children, /*#__PURE__*/React.createElement("span", {
    role: "tooltip",
    style: {
      position: "absolute",
      ...pos[side],
      padding: "6px 10px",
      background: "var(--ink-0)",
      color: "var(--paper-1)",
      fontFamily: "var(--font-sans)",
      fontSize: "var(--fs-xs)",
      fontWeight: "var(--fw-medium)",
      whiteSpace: "nowrap",
      borderRadius: "var(--r-sm)",
      boxShadow: "var(--shadow-md)",
      pointerEvents: "none",
      opacity: show ? 1 : 0,
      transform: `${pos[side].transform} translateY(${show ? "0" : side === "top" ? "4px" : "-4px"})`,
      transition: "opacity var(--dur-fast) var(--ease-out), transform var(--dur-fast) var(--ease-out)",
      zIndex: 50,
      ...style
    }
  }, label));
}
Object.assign(__ds_scope, { Tooltip });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Tooltip.jsx", error: String((e && e.message) || e) }); }

// ui_kits/arc/ArcBrowser.jsx
try { (() => {
// Arc browser shell — composes the sidebar, command bar and web card over
// the active Space's gradient. Click-through interactive.
function ArcBrowser() {
  const {
    ArcSidebar,
    ArcCommandBar,
    ArcWebView
  } = window;

  // Letter-tile "favicons" (reliable offline, on-brand).
  const FAV = (ch, bg) => ({
    ch,
    bg
  });
  const SPACES = [{
    id: "work",
    name: "Work",
    emoji: "💼",
    grad: "linear-gradient(150deg,#bcd0ff,#cdc1ff 60%,#f4c1da)",
    tabs: [{
      id: "t1",
      title: "The Browser Company",
      favicon: FAV("B", "var(--grad-arc)"),
      pinned: true,
      page: "home"
    }, {
      id: "t2",
      title: "Linear",
      favicon: FAV("L", "#5e6ad2"),
      pinned: true,
      kicker: "Issues"
    }, {
      id: "t3",
      title: "Figma",
      favicon: FAV("F", "#f24e1e"),
      pinned: true,
      kicker: "Design"
    }, {
      id: "t4",
      title: "GitHub",
      favicon: FAV("G", "#1b1a17"),
      pinned: true,
      kicker: "Pull requests"
    }, {
      id: "t5",
      title: "Notion — Q3 planning doc",
      favicon: FAV("N", "#2e2c28"),
      pinned: false,
      kicker: "Doc"
    }, {
      id: "t6",
      title: "Why we built Arc",
      favicon: FAV("B", "var(--grad-sunrise)"),
      pinned: false,
      kicker: "Essay",
      banner: "var(--grad-sunrise)"
    }]
  }, {
    id: "life",
    name: "Life",
    emoji: "🌿",
    grad: "linear-gradient(150deg,#ffd9c2,#f4c1da 70%,#cdc1ff)",
    tabs: [{
      id: "l1",
      title: "NYT Cooking",
      favicon: FAV("C", "#c63b22"),
      pinned: true,
      kicker: "Recipe"
    }, {
      id: "l2",
      title: "Spotify",
      favicon: FAV("S", "#1db954"),
      pinned: true,
      kicker: "Now playing"
    }, {
      id: "l3",
      title: "A weekend in the mountains",
      favicon: FAV("A", "#e0539b"),
      pinned: false,
      kicker: "Travel",
      banner: "var(--grad-dusk)"
    }]
  }, {
    id: "read",
    name: "Reading",
    emoji: "📚",
    grad: "linear-gradient(150deg,#cde8ff,#cdc1ff 60%,#ffd9c2)",
    tabs: [{
      id: "r1",
      title: "The internet is a place",
      favicon: FAV("B", "var(--grad-arc)"),
      pinned: true,
      kicker: "Essay",
      page: "home"
    }, {
      id: "r2",
      title: "Long reads",
      favicon: FAV("L", "#7b5bff"),
      pinned: false,
      kicker: "Saved"
    }]
  }];
  const [spaceId, setSpaceId] = React.useState("work");
  const [tabsBySpace, setTabsBySpace] = React.useState(() => Object.fromEntries(SPACES.map(s => [s.id, s.tabs])));
  const [activeBySpace, setActiveBySpace] = React.useState(() => Object.fromEntries(SPACES.map(s => [s.id, s.tabs[0].id])));
  const [cmdOpen, setCmdOpen] = React.useState(false);
  const space = SPACES.find(s => s.id === spaceId);
  const tabs = tabsBySpace[spaceId];
  const activeTab = activeBySpace[spaceId];
  const tab = tabs.find(t => t.id === activeTab);
  React.useEffect(() => {
    const h = e => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "t" || e.key === "l")) {
        e.preventDefault();
        setCmdOpen(true);
      }
      if (e.key === "Escape") setCmdOpen(false);
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);
  const selectTab = id => setActiveBySpace(m => ({
    ...m,
    [spaceId]: id
  }));
  const closeTab = id => {
    setTabsBySpace(m => {
      const next = m[spaceId].filter(t => t.id !== id);
      if (activeTab === id && next.length) setActiveBySpace(a => ({
        ...a,
        [spaceId]: next[0].id
      }));
      return {
        ...m,
        [spaceId]: next
      };
    });
  };
  const navigate = queryOrLabel => {
    const id = "n" + Date.now();
    const newTab = {
      id,
      title: queryOrLabel.replace(/^Search the web.*$/, "Search results"),
      favicon: FAV("🔍", "var(--grad-dusk)"),
      pinned: false,
      kicker: "Web"
    };
    setTabsBySpace(m => ({
      ...m,
      [spaceId]: [...m[spaceId], newTab]
    }));
    setActiveBySpace(a => ({
      ...a,
      [spaceId]: id
    }));
    setCmdOpen(false);
  };
  return /*#__PURE__*/React.createElement("div", {
    style: {
      position: "absolute",
      inset: 0,
      background: space.grad,
      transition: "background var(--dur-slow) var(--ease-out)",
      display: "flex",
      padding: 0,
      fontFamily: "var(--font-sans)",
      overflow: "hidden"
    }
  }, /*#__PURE__*/React.createElement(ArcSidebar, {
    space: space,
    spaces: SPACES,
    tabs: tabs,
    activeTab: activeTab,
    onSelectTab: selectTab,
    onCloseTab: closeTab,
    onSwitchSpace: setSpaceId,
    onOpenCommand: () => setCmdOpen(true),
    onNewTab: () => setCmdOpen(true)
  }), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1,
      margin: "10px 10px 10px 0",
      borderRadius: 16,
      overflow: "hidden",
      background: "var(--paper-0)",
      boxShadow: "var(--shadow-lg)",
      minWidth: 0
    }
  }, /*#__PURE__*/React.createElement(ArcWebView, {
    tab: tab
  })), /*#__PURE__*/React.createElement(ArcCommandBar, {
    open: cmdOpen,
    onClose: () => setCmdOpen(false),
    onNavigate: navigate
  }));
}
window.ArcBrowser = ArcBrowser;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/arc/ArcBrowser.jsx", error: String((e && e.message) || e) }); }

// ui_kits/arc/CommandBar.jsx
try { (() => {
// Arc command bar — the ⌘T / ⌘L overlay. Frosted floating panel.
function ArcCommandBar({
  open,
  onClose,
  onNavigate
}) {
  const {
    Icons
  } = window;
  const [q, setQ] = React.useState("");
  const inputRef = React.useRef(null);
  React.useEffect(() => {
    if (open) {
      setQ("");
      setTimeout(() => inputRef.current && inputRef.current.focus(), 30);
    }
  }, [open]);
  if (!open) return null;
  const suggestions = [{
    icon: /*#__PURE__*/React.createElement(Icons.Search, {
      size: 16
    }),
    label: q ? `Search the web for "${q}"` : "Search the web",
    kind: "Search"
  }, {
    icon: /*#__PURE__*/React.createElement(Icons.Globe, {
      size: 16
    }),
    label: "thebrowser.company",
    kind: "Open"
  }, {
    icon: /*#__PURE__*/React.createElement(Icons.Sparkle, {
      size: 16
    }),
    label: "Ask on this page",
    kind: "AI"
  }, {
    icon: /*#__PURE__*/React.createElement(Icons.Plus, {
      size: 16
    }),
    label: "New Space…",
    kind: "Command"
  }].filter(s => !q || s.kind === "Search" || s.label.toLowerCase().includes(q.toLowerCase()));
  return /*#__PURE__*/React.createElement("div", {
    onClick: onClose,
    style: {
      position: "absolute",
      inset: 0,
      background: "rgba(27,26,23,0.28)",
      display: "flex",
      alignItems: "flex-start",
      justifyContent: "center",
      paddingTop: "16vh",
      zIndex: 60,
      backdropFilter: "blur(2px)"
    }
  }, /*#__PURE__*/React.createElement("div", {
    onClick: e => e.stopPropagation(),
    style: {
      width: 620,
      maxWidth: "84%",
      background: "var(--frost-light)",
      backdropFilter: "var(--blur-frost)",
      WebkitBackdropFilter: "var(--blur-frost)",
      borderRadius: 20,
      boxShadow: "var(--shadow-xl)",
      border: "1px solid rgba(255,255,255,0.6)",
      overflow: "hidden",
      animation: "cmdpop var(--dur-mid) var(--ease-spring)"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 12,
      padding: "18px 22px"
    }
  }, /*#__PURE__*/React.createElement(Icons.Search, {
    size: 22,
    style: {
      color: "var(--ink-3)"
    }
  }), /*#__PURE__*/React.createElement("input", {
    ref: inputRef,
    value: q,
    onChange: e => setQ(e.target.value),
    onKeyDown: e => {
      if (e.key === "Enter") {
        onNavigate(q || "thebrowser.company");
      }
      if (e.key === "Escape") onClose();
    },
    placeholder: "Search or enter URL\u2026",
    style: {
      flex: 1,
      border: "none",
      outline: "none",
      background: "transparent",
      fontFamily: "var(--font-sans)",
      fontSize: 20,
      fontWeight: 500,
      color: "var(--ink-1)"
    }
  }), /*#__PURE__*/React.createElement("kbd", {
    style: {
      fontFamily: "var(--font-mono)",
      fontSize: 12,
      color: "var(--ink-3)",
      background: "rgba(255,255,255,0.6)",
      borderRadius: 6,
      padding: "3px 8px"
    }
  }, "esc")), /*#__PURE__*/React.createElement("div", {
    style: {
      height: 1,
      background: "rgba(27,26,23,0.1)"
    }
  }), /*#__PURE__*/React.createElement("div", {
    style: {
      padding: 8
    }
  }, suggestions.map((s, i) => /*#__PURE__*/React.createElement("div", {
    key: i,
    onClick: () => onNavigate(s.label),
    style: {
      display: "flex",
      alignItems: "center",
      gap: 12,
      padding: "11px 14px",
      borderRadius: 12,
      cursor: "pointer",
      color: "var(--ink-1)",
      background: i === 0 ? "rgba(255,255,255,0.7)" : "transparent"
    },
    onMouseEnter: e => e.currentTarget.style.background = "rgba(255,255,255,0.7)",
    onMouseLeave: e => e.currentTarget.style.background = i === 0 ? "rgba(255,255,255,0.7)" : "transparent"
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      color: "var(--ink-2)",
      display: "flex"
    }
  }, s.icon), /*#__PURE__*/React.createElement("span", {
    style: {
      flex: 1,
      fontSize: 15,
      fontWeight: 500
    }
  }, s.label), /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: 11,
      fontWeight: 700,
      textTransform: "uppercase",
      letterSpacing: "0.05em",
      color: "var(--ink-3)"
    }
  }, s.kind))))));
}
window.ArcCommandBar = ArcCommandBar;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/arc/CommandBar.jsx", error: String((e && e.message) || e) }); }

// ui_kits/arc/Icons.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
// Lucide-style line icons (rounded, 2px stroke) — closest open match to
// The Browser Company's iconography. Each is a tiny stateless component.
const Icon = ({
  d,
  size = 18,
  fill = "none",
  sw = 2,
  children,
  style
}) => /*#__PURE__*/React.createElement("svg", {
  width: size,
  height: size,
  viewBox: "0 0 24 24",
  fill: fill,
  stroke: "currentColor",
  strokeWidth: sw,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  style: style
}, d ? /*#__PURE__*/React.createElement("path", {
  d: d
}) : children);
const Icons = {
  Search: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("circle", {
    cx: "11",
    cy: "11",
    r: "7"
  }), /*#__PURE__*/React.createElement("path", {
    d: "m21 21-4.3-4.3"
  })),
  Plus: p => /*#__PURE__*/React.createElement(Icon, _extends({}, p, {
    d: "M12 5v14M5 12h14"
  })),
  ChevronLeft: p => /*#__PURE__*/React.createElement(Icon, _extends({}, p, {
    d: "m15 18-6-6 6-6"
  })),
  ChevronRight: p => /*#__PURE__*/React.createElement(Icon, _extends({}, p, {
    d: "m9 18 6-6-6-6"
  })),
  Rotate: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("path", {
    d: "M3 12a9 9 0 1 0 3-6.7L3 8"
  }), /*#__PURE__*/React.createElement("path", {
    d: "M3 3v5h5"
  })),
  Sidebar: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("rect", {
    x: "3",
    y: "3",
    width: "18",
    height: "18",
    rx: "3"
  }), /*#__PURE__*/React.createElement("path", {
    d: "M9 3v18"
  })),
  Share: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("circle", {
    cx: "18",
    cy: "5",
    r: "3"
  }), /*#__PURE__*/React.createElement("circle", {
    cx: "6",
    cy: "12",
    r: "3"
  }), /*#__PURE__*/React.createElement("circle", {
    cx: "18",
    cy: "19",
    r: "3"
  }), /*#__PURE__*/React.createElement("path", {
    d: "m8.6 13.5 6.8 4M15.4 6.5 8.6 10.5"
  })),
  Copy: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("rect", {
    x: "9",
    y: "9",
    width: "12",
    height: "12",
    rx: "2"
  }), /*#__PURE__*/React.createElement("path", {
    d: "M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"
  })),
  X: p => /*#__PURE__*/React.createElement(Icon, _extends({}, p, {
    d: "M18 6 6 18M6 6l12 12"
  })),
  Pin: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("path", {
    d: "M12 17v5"
  }), /*#__PURE__*/React.createElement("path", {
    d: "M9 10.8V4h6v6.8a2 2 0 0 0 .6 1.4l1.9 2A1 1 0 0 1 16.8 16H7.2a1 1 0 0 1-.7-1.7l1.9-2A2 2 0 0 0 9 10.8Z"
  })),
  Archive: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("rect", {
    x: "3",
    y: "4",
    width: "18",
    height: "4",
    rx: "1"
  }), /*#__PURE__*/React.createElement("path", {
    d: "M5 8v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8M10 12h4"
  })),
  Folder: p => /*#__PURE__*/React.createElement(Icon, _extends({}, p, {
    d: "M4 20a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2Z"
  })),
  Settings: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("circle", {
    cx: "12",
    cy: "12",
    r: "3"
  }), /*#__PURE__*/React.createElement("path", {
    d: "M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"
  })),
  Sparkle: p => /*#__PURE__*/React.createElement(Icon, _extends({}, p, {
    d: "M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z"
  })),
  Star: p => /*#__PURE__*/React.createElement(Icon, _extends({}, p, {
    d: "M12 3l2.6 5.3 5.9.9-4.3 4.2 1 5.9-5.2-2.8-5.2 2.8 1-5.9L3.5 9.2l5.9-.9z"
  })),
  Lock: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("rect", {
    x: "4",
    y: "11",
    width: "16",
    height: "9",
    rx: "2"
  }), /*#__PURE__*/React.createElement("path", {
    d: "M8 11V7a4 4 0 0 1 8 0v4"
  })),
  MoreH: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("circle", {
    cx: "5",
    cy: "12",
    r: "1.4"
  }), /*#__PURE__*/React.createElement("circle", {
    cx: "12",
    cy: "12",
    r: "1.4"
  }), /*#__PURE__*/React.createElement("circle", {
    cx: "19",
    cy: "12",
    r: "1.4"
  })),
  ArrowRight: p => /*#__PURE__*/React.createElement(Icon, _extends({}, p, {
    d: "M5 12h14M13 6l6 6-6 6"
  })),
  Globe: p => /*#__PURE__*/React.createElement(Icon, p, /*#__PURE__*/React.createElement("circle", {
    cx: "12",
    cy: "12",
    r: "9"
  }), /*#__PURE__*/React.createElement("path", {
    d: "M3 12h18M12 3a14 14 0 0 1 0 18 14 14 0 0 1 0-18Z"
  })),
  Download: p => /*#__PURE__*/React.createElement(Icon, _extends({}, p, {
    d: "M12 3v12m-5-5 5 5 5-5M5 21h14"
  }))
};
window.Icons = Icons;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/arc/Icons.jsx", error: String((e && e.message) || e) }); }

// ui_kits/arc/Sidebar.jsx
try { (() => {
// Arc sidebar — Spaces, command field, pinned + today tabs.
function ArcSidebar({
  space,
  spaces,
  tabs,
  activeTab,
  onSelectTab,
  onCloseTab,
  onSwitchSpace,
  onOpenCommand,
  onNewTab
}) {
  const {
    Icons
  } = window;
  const pinned = tabs.filter(t => t.pinned);
  const today = tabs.filter(t => !t.pinned);
  const Favicon = ({
    f,
    size = 16
  }) => /*#__PURE__*/React.createElement("span", {
    style: {
      width: size,
      height: size,
      borderRadius: Math.round(size / 4),
      flexShrink: 0,
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      background: f.bg,
      color: "#fff",
      fontSize: size * 0.6,
      fontWeight: 700,
      fontFamily: "var(--font-sans)",
      lineHeight: 1
    }
  }, f.ch);
  const TabRow = ({
    t
  }) => {
    const on = t.id === activeTab;
    return /*#__PURE__*/React.createElement("div", {
      onClick: () => onSelectTab(t.id),
      style: {
        display: "flex",
        alignItems: "center",
        gap: 10,
        height: 34,
        padding: "0 8px",
        borderRadius: 10,
        cursor: "pointer",
        background: on ? "rgba(255,255,255,0.66)" : "transparent",
        boxShadow: on ? "var(--shadow-xs)" : "none",
        color: "var(--ink-1)",
        transition: "background var(--dur-fast) var(--ease-out)"
      },
      onMouseEnter: e => {
        if (!on) e.currentTarget.style.background = "rgba(255,255,255,0.38)";
      },
      onMouseLeave: e => {
        if (!on) e.currentTarget.style.background = "transparent";
      },
      className: "arc-tab"
    }, /*#__PURE__*/React.createElement(Favicon, {
      f: t.favicon,
      size: 16
    }), /*#__PURE__*/React.createElement("span", {
      style: {
        flex: 1,
        fontSize: 13.5,
        fontWeight: on ? 600 : 500,
        whiteSpace: "nowrap",
        overflow: "hidden",
        textOverflow: "ellipsis"
      }
    }, t.title), /*#__PURE__*/React.createElement("button", {
      className: "arc-tab-x",
      onClick: e => {
        e.stopPropagation();
        onCloseTab(t.id);
      },
      style: {
        border: "none",
        background: "transparent",
        color: "var(--ink-3)",
        cursor: "pointer",
        display: "flex",
        padding: 2,
        borderRadius: 6,
        opacity: 0
      }
    }, /*#__PURE__*/React.createElement(Icons.X, {
      size: 14
    })));
  };
  return /*#__PURE__*/React.createElement("div", {
    style: {
      width: 248,
      flexShrink: 0,
      display: "flex",
      flexDirection: "column",
      padding: "14px 10px 10px",
      gap: 12,
      height: "100%",
      boxSizing: "border-box"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 8,
      paddingLeft: 4
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      gap: 7
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      width: 12,
      height: 12,
      borderRadius: "50%",
      background: "#ff5f57"
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      width: 12,
      height: 12,
      borderRadius: "50%",
      background: "#febc2e"
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      width: 12,
      height: 12,
      borderRadius: "50%",
      background: "#28c840"
    }
  })), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1
    }
  }), /*#__PURE__*/React.createElement("button", {
    className: "arc-icn"
  }, /*#__PURE__*/React.createElement(Icons.Sidebar, {
    size: 16
  }))), /*#__PURE__*/React.createElement("button", {
    onClick: onOpenCommand,
    style: {
      display: "flex",
      alignItems: "center",
      gap: 9,
      height: 38,
      padding: "0 12px",
      background: "rgba(255,255,255,0.55)",
      border: "1px solid rgba(255,255,255,0.6)",
      borderRadius: 12,
      cursor: "text",
      color: "var(--ink-3)",
      width: "100%",
      boxShadow: "var(--shadow-xs)"
    }
  }, /*#__PURE__*/React.createElement(Icons.Search, {
    size: 16
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: 13.5,
      fontWeight: 500
    }
  }, "Search or enter URL\u2026")), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      gap: 4
    }
  }, /*#__PURE__*/React.createElement("button", {
    className: "arc-icn"
  }, /*#__PURE__*/React.createElement(Icons.ChevronLeft, {
    size: 18
  })), /*#__PURE__*/React.createElement("button", {
    className: "arc-icn"
  }, /*#__PURE__*/React.createElement(Icons.ChevronRight, {
    size: 18
  })), /*#__PURE__*/React.createElement("button", {
    className: "arc-icn"
  }, /*#__PURE__*/React.createElement(Icons.Rotate, {
    size: 16
  })), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1
    }
  }), /*#__PURE__*/React.createElement("button", {
    className: "arc-icn",
    onClick: onNewTab
  }, /*#__PURE__*/React.createElement(Icons.Plus, {
    size: 18
  }))), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "grid",
      gridTemplateColumns: "repeat(4, 1fr)",
      gap: 6
    }
  }, pinned.slice(0, 4).map(t => /*#__PURE__*/React.createElement("button", {
    key: t.id,
    onClick: () => onSelectTab(t.id),
    style: {
      height: 44,
      borderRadius: 12,
      border: "none",
      cursor: "pointer",
      background: t.id === activeTab ? "rgba(255,255,255,0.7)" : "rgba(255,255,255,0.32)",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      boxShadow: t.id === activeTab ? "var(--shadow-xs)" : "none"
    }
  }, /*#__PURE__*/React.createElement(Favicon, {
    f: t.favicon,
    size: 20
  })))), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1,
      overflowY: "auto",
      display: "flex",
      flexDirection: "column",
      gap: 2
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 7,
      padding: "6px 8px 2px",
      color: "var(--ink-3)"
    }
  }, /*#__PURE__*/React.createElement(Icons.Folder, {
    size: 14
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: 11,
      fontWeight: 700,
      textTransform: "uppercase",
      letterSpacing: "0.06em"
    }
  }, "Pinned")), pinned.map(t => /*#__PURE__*/React.createElement(TabRow, {
    key: t.id,
    t: t
  })), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 7,
      padding: "12px 8px 2px",
      color: "var(--ink-3)"
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: 11,
      fontWeight: 700,
      textTransform: "uppercase",
      letterSpacing: "0.06em"
    }
  }, "Today"), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1,
      height: 1,
      background: "rgba(27,26,23,0.12)"
    }
  })), today.map(t => /*#__PURE__*/React.createElement(TabRow, {
    key: t.id,
    t: t
  }))), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 6,
      paddingTop: 8,
      borderTop: "1px solid rgba(27,26,23,0.1)"
    }
  }, spaces.map(s => /*#__PURE__*/React.createElement("button", {
    key: s.id,
    onClick: () => onSwitchSpace(s.id),
    title: s.name,
    style: {
      width: 30,
      height: 30,
      borderRadius: 9,
      border: "none",
      cursor: "pointer",
      fontSize: 15,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      background: s.id === space.id ? "rgba(255,255,255,0.7)" : "transparent",
      boxShadow: s.id === space.id ? "var(--shadow-xs)" : "none",
      transform: s.id === space.id ? "scale(1)" : "scale(0.92)",
      opacity: s.id === space.id ? 1 : 0.7
    }
  }, s.emoji)), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1
    }
  }), /*#__PURE__*/React.createElement("button", {
    className: "arc-icn"
  }, /*#__PURE__*/React.createElement(Icons.Settings, {
    size: 16
  }))));
}
window.ArcSidebar = ArcSidebar;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/arc/Sidebar.jsx", error: String((e && e.message) || e) }); }

// ui_kits/arc/WebView.jsx
try { (() => {
// Faux web content rendered inside Arc's floating card.
function ArcWebView({
  tab
}) {
  const {
    Icons
  } = window;
  if (!tab) {
    return /*#__PURE__*/React.createElement("div", {
      style: {
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        height: "100%",
        gap: 14,
        color: "var(--ink-3)"
      }
    }, /*#__PURE__*/React.createElement(Icons.Globe, {
      size: 40
    }), /*#__PURE__*/React.createElement("div", {
      style: {
        fontFamily: "var(--font-serif)",
        fontSize: 22,
        color: "var(--ink-2)"
      }
    }, "Nothing open. Press \u2318T."));
  }

  // The Browser Company homepage-style splash for the brand tab.
  if (tab.page === "home") {
    return /*#__PURE__*/React.createElement("div", {
      style: {
        height: "100%",
        overflowY: "auto",
        background: "var(--paper-1)"
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        position: "relative",
        padding: "72px 64px 64px",
        background: "var(--grad-mesh), var(--paper-1)"
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        display: "inline-flex",
        alignItems: "center",
        gap: 8,
        padding: "6px 14px",
        background: "rgba(255,255,255,0.7)",
        borderRadius: 999,
        boxShadow: "var(--shadow-xs)",
        fontSize: 13,
        fontWeight: 600,
        color: "var(--ink-2)",
        marginBottom: 28
      }
    }, /*#__PURE__*/React.createElement("span", {
      style: {
        width: 16,
        height: 16,
        borderRadius: 5,
        background: "var(--grad-arc)"
      }
    }), "The Browser Company"), /*#__PURE__*/React.createElement("h1", {
      style: {
        fontFamily: "var(--font-serif)",
        fontSize: 64,
        lineHeight: 1.02,
        letterSpacing: "-0.03em",
        fontWeight: 400,
        color: "var(--ink-1)",
        margin: "0 0 20px",
        maxWidth: 760
      }
    }, "We're building a", /*#__PURE__*/React.createElement("br", null), "better way to", /*#__PURE__*/React.createElement("br", null), /*#__PURE__*/React.createElement("span", {
      style: {
        background: "var(--grad-arc)",
        WebkitBackgroundClip: "text",
        WebkitTextFillColor: "transparent",
        backgroundClip: "text"
      }
    }, "use the internet.")), /*#__PURE__*/React.createElement("p", {
      style: {
        fontSize: 19,
        lineHeight: 1.55,
        color: "var(--ink-2)",
        maxWidth: 540,
        margin: "0 0 32px"
      }
    }, "The internet is our home. And it deserves a browser that feels like one \u2014 warm, personal, and a little bit magic."), /*#__PURE__*/React.createElement("div", {
      style: {
        display: "flex",
        gap: 12
      }
    }, /*#__PURE__*/React.createElement("button", {
      style: {
        height: 48,
        padding: "0 26px",
        borderRadius: 999,
        border: "none",
        cursor: "pointer",
        background: "var(--grad-arc)",
        color: "#fff",
        fontFamily: "var(--font-sans)",
        fontSize: 16,
        fontWeight: 600,
        boxShadow: "var(--glow-accent)"
      }
    }, "Download Arc"), /*#__PURE__*/React.createElement("button", {
      style: {
        height: 48,
        padding: "0 24px",
        borderRadius: 999,
        cursor: "pointer",
        background: "var(--paper-0)",
        border: "1px solid var(--line-2)",
        color: "var(--ink-1)",
        fontFamily: "var(--font-sans)",
        fontSize: 16,
        fontWeight: 600
      }
    }, "Meet Dia"))), /*#__PURE__*/React.createElement("div", {
      style: {
        padding: "8px 64px 64px",
        display: "grid",
        gridTemplateColumns: "repeat(3,1fr)",
        gap: 18
      }
    }, [{
      t: "Spaces",
      d: "Group your tabs by what you're doing — work, life, a weekend project.",
      g: "var(--grad-sunrise)"
    }, {
      t: "Command bar",
      d: "Search, switch, and navigate without ever lifting your hands.",
      g: "var(--grad-dusk)"
    }, {
      t: "Tabs that tidy up",
      d: "Open tabs auto-archive after a day, so you always start fresh.",
      g: "var(--grad-arc)"
    }].map(c => /*#__PURE__*/React.createElement("div", {
      key: c.t,
      style: {
        background: "var(--paper-0)",
        borderRadius: 20,
        padding: 22,
        boxShadow: "var(--shadow-sm)"
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        width: 40,
        height: 40,
        borderRadius: 12,
        background: c.g,
        marginBottom: 14
      }
    }), /*#__PURE__*/React.createElement("div", {
      style: {
        fontFamily: "var(--font-serif)",
        fontSize: 22,
        color: "var(--ink-1)",
        marginBottom: 6
      }
    }, c.t), /*#__PURE__*/React.createElement("div", {
      style: {
        fontSize: 14.5,
        lineHeight: 1.5,
        color: "var(--ink-2)"
      }
    }, c.d)))));
  }

  // Generic article-style page for other tabs.
  return /*#__PURE__*/React.createElement("div", {
    style: {
      height: "100%",
      overflowY: "auto",
      background: "var(--paper-0)"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      height: 200,
      background: tab.banner || "var(--grad-dusk)"
    }
  }), /*#__PURE__*/React.createElement("div", {
    style: {
      maxWidth: 680,
      margin: "0 auto",
      padding: "40px 32px 64px"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      fontSize: 12,
      fontWeight: 700,
      textTransform: "uppercase",
      letterSpacing: "0.07em",
      color: "var(--ink-3)",
      marginBottom: 14
    }
  }, tab.kicker || "Article"), /*#__PURE__*/React.createElement("h1", {
    style: {
      fontFamily: "var(--font-serif)",
      fontSize: 40,
      lineHeight: 1.08,
      letterSpacing: "-0.02em",
      fontWeight: 400,
      color: "var(--ink-1)",
      margin: "0 0 18px"
    }
  }, tab.title), /*#__PURE__*/React.createElement("p", {
    style: {
      fontSize: 18,
      lineHeight: 1.6,
      color: "var(--ink-2)",
      margin: "0 0 16px"
    }
  }, "The browser has barely changed in twenty-five years. We think that's strange \u2014 it's the place we spend most of our day."), /*#__PURE__*/React.createElement("p", {
    style: {
      fontSize: 18,
      lineHeight: 1.6,
      color: "var(--ink-2)",
      margin: 0
    }
  }, "So we started over. Not with features, but with feeling: what if your browser felt less like software and more like a place that's yours?")));
}
window.ArcWebView = ArcWebView;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/arc/WebView.jsx", error: String((e && e.message) || e) }); }

// ui_kits/dia/Assistant.jsx
try { (() => {
// Dia AI assistant side panel. Chat-forward, context-aware about the page.
function DiaAssistant({
  open,
  onClose,
  pageTitle
}) {
  const {
    Icons
  } = window;
  const [msgs, setMsgs] = React.useState([{
    from: "ai",
    text: `I'm reading "${pageTitle}" for you. Ask me anything about this page — or the web.`
  }]);
  const [draft, setDraft] = React.useState("");
  const scrollRef = React.useRef(null);
  React.useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
  }, [msgs, open]);
  const send = text => {
    const q = (text ?? draft).trim();
    if (!q) return;
    setDraft("");
    setMsgs(m => [...m, {
      from: "me",
      text: q
    }]);
    setTimeout(() => {
      setMsgs(m => [...m, {
        from: "ai",
        text: "Here's the gist: this page is The Browser Company's note on building a browser that feels personal. The core idea — software should feel like a place, not a tool. Want me to pull out the key quotes or summarize the rest?"
      }]);
    }, 600);
  };
  if (!open) return null;
  const chips = ["Summarize this page", "Key takeaways", "Explain simply"];
  return /*#__PURE__*/React.createElement("div", {
    style: {
      width: 360,
      flexShrink: 0,
      height: "100%",
      display: "flex",
      flexDirection: "column",
      background: "var(--paper-1)",
      borderLeft: "1px solid var(--line-1)"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 10,
      padding: "14px 16px",
      borderBottom: "1px solid var(--line-1)"
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      width: 26,
      height: 26,
      borderRadius: 8,
      background: "var(--grad-arc)",
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      color: "#fff"
    }
  }, /*#__PURE__*/React.createElement(Icons.Sparkle, {
    size: 15
  })), /*#__PURE__*/React.createElement("span", {
    style: {
      flex: 1,
      fontFamily: "var(--font-serif)",
      fontSize: 18,
      color: "var(--ink-1)"
    }
  }, "Dia"), /*#__PURE__*/React.createElement("button", {
    className: "dia-icn",
    onClick: onClose
  }, /*#__PURE__*/React.createElement(Icons.X, {
    size: 16
  }))), /*#__PURE__*/React.createElement("div", {
    ref: scrollRef,
    style: {
      flex: 1,
      overflowY: "auto",
      padding: 16,
      display: "flex",
      flexDirection: "column",
      gap: 12
    }
  }, msgs.map((m, i) => /*#__PURE__*/React.createElement("div", {
    key: i,
    style: {
      alignSelf: m.from === "me" ? "flex-end" : "flex-start",
      maxWidth: "88%"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      padding: "10px 14px",
      borderRadius: m.from === "me" ? "16px 16px 4px 16px" : "16px 16px 16px 4px",
      background: m.from === "me" ? "var(--ink-1)" : "var(--paper-0)",
      color: m.from === "me" ? "var(--paper-0)" : "var(--ink-1)",
      boxShadow: m.from === "me" ? "none" : "var(--shadow-xs)",
      fontSize: 14.5,
      lineHeight: 1.55
    }
  }, m.text)))), /*#__PURE__*/React.createElement("div", {
    style: {
      padding: 14,
      display: "flex",
      flexDirection: "column",
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      gap: 7,
      flexWrap: "wrap"
    }
  }, chips.map(c => /*#__PURE__*/React.createElement("button", {
    key: c,
    onClick: () => send(c),
    style: {
      height: 30,
      padding: "0 12px",
      borderRadius: 999,
      cursor: "pointer",
      background: "var(--paper-0)",
      border: "1px solid var(--line-1)",
      fontFamily: "var(--font-sans)",
      fontSize: 12.5,
      fontWeight: 500,
      color: "var(--ink-2)"
    }
  }, c))), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 8,
      padding: "6px 6px 6px 16px",
      background: "var(--paper-0)",
      border: "1px solid var(--line-2)",
      borderRadius: 16,
      boxShadow: "var(--shadow-xs)"
    }
  }, /*#__PURE__*/React.createElement("input", {
    value: draft,
    onChange: e => setDraft(e.target.value),
    onKeyDown: e => e.key === "Enter" && send(),
    placeholder: "Ask about this page\u2026",
    style: {
      flex: 1,
      border: "none",
      outline: "none",
      background: "transparent",
      fontFamily: "var(--font-sans)",
      fontSize: 14.5,
      color: "var(--ink-1)"
    }
  }), /*#__PURE__*/React.createElement("button", {
    onClick: () => send(),
    style: {
      width: 34,
      height: 34,
      borderRadius: 11,
      border: "none",
      cursor: "pointer",
      background: "var(--grad-arc)",
      color: "#fff",
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      flexShrink: 0
    }
  }, /*#__PURE__*/React.createElement(Icons.ArrowRight, {
    size: 17
  })))));
}
window.DiaAssistant = DiaAssistant;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/dia/Assistant.jsx", error: String((e && e.message) || e) }); }

// ui_kits/dia/Chrome.jsx
try { (() => {
// Dia top chrome — horizontal tab strip + URL pill. Calmer, lighter than Arc.
function DiaChrome({
  tabs,
  activeTab,
  onSelectTab,
  onCloseTab,
  onNewTab,
  url,
  onToggleAI,
  aiOpen
}) {
  const {
    Icons
  } = window;
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      flexDirection: "column",
      background: "var(--paper-1)",
      borderBottom: "1px solid var(--line-1)"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 6,
      padding: "8px 12px 0",
      height: 40
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      gap: 7,
      marginRight: 8
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      width: 12,
      height: 12,
      borderRadius: "50%",
      background: "#ff5f57"
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      width: 12,
      height: 12,
      borderRadius: "50%",
      background: "#febc2e"
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      width: 12,
      height: 12,
      borderRadius: "50%",
      background: "#28c840"
    }
  })), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      gap: 4,
      flex: 1,
      overflow: "hidden"
    }
  }, tabs.map(t => {
    const on = t.id === activeTab;
    return /*#__PURE__*/React.createElement("div", {
      key: t.id,
      onClick: () => onSelectTab(t.id),
      className: "dia-tab",
      style: {
        display: "flex",
        alignItems: "center",
        gap: 8,
        height: 34,
        padding: "0 10px",
        minWidth: 0,
        maxWidth: 200,
        borderRadius: "10px 10px 0 0",
        cursor: "pointer",
        background: on ? "var(--paper-0)" : "transparent",
        boxShadow: on ? "0 -1px 4px rgba(27,26,23,0.05)" : "none",
        color: on ? "var(--ink-1)" : "var(--ink-3)"
      }
    }, /*#__PURE__*/React.createElement("span", {
      style: {
        width: 15,
        height: 15,
        borderRadius: 4,
        flexShrink: 0,
        background: t.bg,
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        color: "#fff",
        fontSize: 9,
        fontWeight: 700
      }
    }, t.ch), /*#__PURE__*/React.createElement("span", {
      style: {
        flex: 1,
        fontSize: 13,
        fontWeight: on ? 600 : 500,
        whiteSpace: "nowrap",
        overflow: "hidden",
        textOverflow: "ellipsis"
      }
    }, t.title), /*#__PURE__*/React.createElement("button", {
      className: "dia-tab-x",
      onClick: e => {
        e.stopPropagation();
        onCloseTab(t.id);
      },
      style: {
        border: "none",
        background: "transparent",
        color: "var(--ink-3)",
        cursor: "pointer",
        display: "flex",
        padding: 1,
        borderRadius: 5,
        opacity: on ? 0.6 : 0
      }
    }, /*#__PURE__*/React.createElement(Icons.X, {
      size: 13
    })));
  }), /*#__PURE__*/React.createElement("button", {
    onClick: onNewTab,
    className: "dia-icn",
    style: {
      alignSelf: "center"
    }
  }, /*#__PURE__*/React.createElement(Icons.Plus, {
    size: 16
  })))), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 8,
      padding: "8px 14px",
      background: "var(--paper-0)"
    }
  }, /*#__PURE__*/React.createElement("button", {
    className: "dia-icn"
  }, /*#__PURE__*/React.createElement(Icons.ChevronLeft, {
    size: 18
  })), /*#__PURE__*/React.createElement("button", {
    className: "dia-icn"
  }, /*#__PURE__*/React.createElement(Icons.ChevronRight, {
    size: 18
  })), /*#__PURE__*/React.createElement("button", {
    className: "dia-icn"
  }, /*#__PURE__*/React.createElement(Icons.Rotate, {
    size: 15
  })), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1,
      display: "flex",
      alignItems: "center",
      gap: 9,
      height: 36,
      padding: "0 14px",
      background: "var(--paper-2)",
      borderRadius: 999,
      color: "var(--ink-2)"
    }
  }, /*#__PURE__*/React.createElement(Icons.Lock, {
    size: 13
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: "var(--font-mono)",
      fontSize: 13
    }
  }, url)), /*#__PURE__*/React.createElement("button", {
    className: "dia-icn"
  }, /*#__PURE__*/React.createElement(Icons.Star, {
    size: 17
  })), /*#__PURE__*/React.createElement("button", {
    onClick: onToggleAI,
    style: {
      display: "inline-flex",
      alignItems: "center",
      gap: 7,
      height: 34,
      padding: "0 14px",
      border: "none",
      cursor: "pointer",
      borderRadius: 999,
      fontFamily: "var(--font-sans)",
      fontSize: 13.5,
      fontWeight: 600,
      whiteSpace: "nowrap",
      flexShrink: 0,
      background: aiOpen ? "var(--ink-1)" : "var(--grad-arc)",
      color: "#fff",
      boxShadow: aiOpen ? "none" : "var(--glow-accent)"
    }
  }, /*#__PURE__*/React.createElement(Icons.Sparkle, {
    size: 15
  }), " Ask Dia")));
}
window.DiaChrome = DiaChrome;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/dia/Chrome.jsx", error: String((e && e.message) || e) }); }

// ui_kits/dia/DiaBrowser.jsx
try { (() => {
// Dia browser shell — top chrome + page + collapsible AI assistant.
function DiaBrowser() {
  const {
    DiaChrome,
    DiaAssistant
  } = window;
  const INIT = [{
    id: "d1",
    title: "The Browser Company",
    ch: "B",
    bg: "var(--grad-arc)",
    url: "thebrowser.company",
    page: "home"
  }, {
    id: "d2",
    title: "Dia — a new kind of browser",
    ch: "D",
    bg: "var(--grad-dusk)",
    url: "diabrowser.com",
    page: "article"
  }, {
    id: "d3",
    title: "Inbox",
    ch: "M",
    bg: "#2f6bff",
    url: "mail.google.com",
    page: "article"
  }];
  const [tabs, setTabs] = React.useState(INIT);
  const [activeTab, setActiveTab] = React.useState("d1");
  const [aiOpen, setAiOpen] = React.useState(true);
  const tab = tabs.find(t => t.id === activeTab) || tabs[0];
  const closeTab = id => {
    setTabs(ts => {
      const next = ts.filter(t => t.id !== id);
      if (activeTab === id && next.length) setActiveTab(next[0].id);
      return next;
    });
  };
  const newTab = () => {
    const id = "n" + Date.now();
    setTabs(ts => [...ts, {
      id,
      title: "New tab",
      ch: "✦",
      bg: "var(--grad-sunrise)",
      url: "Search or enter URL",
      page: "blank"
    }]);
    setActiveTab(id);
  };
  return /*#__PURE__*/React.createElement("div", {
    style: {
      position: "absolute",
      inset: 0,
      display: "flex",
      flexDirection: "column",
      background: "var(--paper-0)",
      fontFamily: "var(--font-sans)",
      overflow: "hidden"
    }
  }, /*#__PURE__*/React.createElement(DiaChrome, {
    tabs: tabs,
    activeTab: activeTab,
    onSelectTab: setActiveTab,
    onCloseTab: closeTab,
    onNewTab: newTab,
    url: tab.url,
    aiOpen: aiOpen,
    onToggleAI: () => setAiOpen(o => !o)
  }), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1,
      display: "flex",
      minHeight: 0
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1,
      minWidth: 0,
      overflowY: "auto"
    }
  }, /*#__PURE__*/React.createElement(DiaPage, {
    tab: tab
  })), /*#__PURE__*/React.createElement(DiaAssistant, {
    open: aiOpen,
    onClose: () => setAiOpen(false),
    pageTitle: tab.title
  })));
}

// Page content reused from a simple inline renderer (Dia uses cleaner reading layouts).
function DiaPage({
  tab
}) {
  const {
    Icons
  } = window;
  if (tab.page === "blank") {
    return /*#__PURE__*/React.createElement("div", {
      style: {
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        height: "100%",
        gap: 16,
        background: "var(--grad-mesh), var(--paper-1)"
      }
    }, /*#__PURE__*/React.createElement("span", {
      style: {
        width: 56,
        height: 56,
        borderRadius: 16,
        background: "var(--grad-arc)",
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        color: "#fff"
      }
    }, /*#__PURE__*/React.createElement(Icons.Sparkle, {
      size: 28
    })), /*#__PURE__*/React.createElement("div", {
      style: {
        fontFamily: "var(--font-serif)",
        fontSize: 28,
        color: "var(--ink-1)"
      }
    }, "Where to next?"), /*#__PURE__*/React.createElement("div", {
      style: {
        display: "flex",
        alignItems: "center",
        gap: 10,
        width: 460,
        maxWidth: "80%",
        height: 50,
        padding: "0 18px",
        background: "var(--paper-0)",
        borderRadius: 16,
        boxShadow: "var(--shadow-md)"
      }
    }, /*#__PURE__*/React.createElement(Icons.Search, {
      size: 18,
      style: {
        color: "var(--ink-3)"
      }
    }), /*#__PURE__*/React.createElement("span", {
      style: {
        color: "var(--ink-3)",
        fontSize: 16
      }
    }, "Search or ask Dia anything\u2026")));
  }
  if (tab.page === "home") {
    return /*#__PURE__*/React.createElement("div", {
      style: {
        padding: "64px 72px",
        background: "var(--grad-mesh), var(--paper-1)",
        minHeight: "100%"
      }
    }, /*#__PURE__*/React.createElement("h1", {
      style: {
        fontFamily: "var(--font-serif)",
        fontSize: 60,
        lineHeight: 1.03,
        letterSpacing: "-0.03em",
        fontWeight: 400,
        color: "var(--ink-1)",
        margin: "0 0 18px",
        maxWidth: 720
      }
    }, "A browser that", /*#__PURE__*/React.createElement("br", null), "thinks with you."), /*#__PURE__*/React.createElement("p", {
      style: {
        fontSize: 19,
        lineHeight: 1.55,
        color: "var(--ink-2)",
        maxWidth: 520,
        margin: "0 0 28px"
      }
    }, "Dia puts an AI right beside your tabs \u2014 to summarize, answer, and help you make sense of the web as you go."), /*#__PURE__*/React.createElement("button", {
      style: {
        height: 48,
        padding: "0 26px",
        borderRadius: 999,
        border: "none",
        cursor: "pointer",
        background: "var(--grad-arc)",
        color: "#fff",
        fontSize: 16,
        fontWeight: 600,
        fontFamily: "var(--font-sans)",
        boxShadow: "var(--glow-accent)"
      }
    }, "Get early access"));
  }
  return /*#__PURE__*/React.createElement("div", {
    style: {
      maxWidth: 680,
      margin: "0 auto",
      padding: "56px 32px 80px"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      fontSize: 12,
      fontWeight: 700,
      textTransform: "uppercase",
      letterSpacing: "0.07em",
      color: "var(--ink-3)",
      marginBottom: 14
    }
  }, "From the team"), /*#__PURE__*/React.createElement("h1", {
    style: {
      fontFamily: "var(--font-serif)",
      fontSize: 42,
      lineHeight: 1.08,
      letterSpacing: "-0.02em",
      fontWeight: 400,
      color: "var(--ink-1)",
      margin: "0 0 20px"
    }
  }, tab.title), /*#__PURE__*/React.createElement("p", {
    style: {
      fontSize: 18,
      lineHeight: 1.65,
      color: "var(--ink-2)",
      margin: "0 0 18px"
    }
  }, "We built Dia because we kept asking the same question: what if the browser understood what you were doing, and quietly helped?"), /*#__PURE__*/React.createElement("p", {
    style: {
      fontSize: 18,
      lineHeight: 1.65,
      color: "var(--ink-2)",
      margin: "0 0 18px"
    }
  }, "Not another chatbot in a tab \u2014 an assistant woven into browsing itself. It reads alongside you, remembers what matters, and stays out of the way until you need it."), /*#__PURE__*/React.createElement("p", {
    style: {
      fontSize: 18,
      lineHeight: 1.65,
      color: "var(--ink-2)",
      margin: 0
    }
  }, "Try it: open the panel on the right and ask about this page."));
}
window.DiaBrowser = DiaBrowser;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/dia/DiaBrowser.jsx", error: String((e && e.message) || e) }); }

__ds_ns.Avatar = __ds_scope.Avatar;

__ds_ns.Badge = __ds_scope.Badge;

__ds_ns.Button = __ds_scope.Button;

__ds_ns.Card = __ds_scope.Card;

__ds_ns.IconButton = __ds_scope.IconButton;

__ds_ns.Input = __ds_scope.Input;

__ds_ns.Switch = __ds_scope.Switch;

__ds_ns.Tabs = __ds_scope.Tabs;

__ds_ns.Tag = __ds_scope.Tag;

__ds_ns.Tooltip = __ds_scope.Tooltip;

})();
