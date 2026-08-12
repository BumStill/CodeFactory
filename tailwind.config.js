/** @type {import('tailwindcss').Config} */
const themeColor = (name) => `rgb(var(${name}) / <alpha-value>)`;

/**
 * A step on the type scale, authored in px and scaled by the user's text-size
 * setting.
 *
 * px, not rem, on purpose. Tailwind's stock scale is rem, which ties text to
 * the rem baseline — and that baseline also drives every spacing, radius and
 * box size. Sharing one number between "how big is the text" and "how big is
 * the grid" is what let a 14px root silently render `text-xs` at 10.5px and
 * `gap-2` at 7px. `--font-scale` moves text alone; the grid stays on rem.
 */
const step = (size, lineHeight) => [
  `calc(${size}px * var(--font-scale, 1))`,
  { lineHeight: `calc(${lineHeight}px * var(--font-scale, 1))` },
];

export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  darkMode: ["selector", '[data-theme="dark"]'],
  theme: {
    /*
     * Replaces Tailwind's stock t-shirt scale rather than extending it, so
     * `text-xs` and friends do not exist. The product had 1039 font-size
     * declarations across 6 stock steps and 8 arbitrary px values, 83% of them
     * landing between 10 and 11px, with card titles rendering smaller than
     * their own body copy. Names that say what the text IS keep that from
     * coming back; src/styles/typographyScaleAudit.test.ts holds the line.
     *
     * See docs/specs/ui-typography-and-spacing.md.
     */
    fontSize: {
      caption: step(11, 16), // timestamps, counts, paths, secondary metadata
      label:   step(12, 18), // control labels, chips, badges, button text
      note:    step(13, 20), // secondary reading text; monospace code blocks
      body:    step(14, 22), // UI body copy, list primaries, form values
      reading: step(15, 24), // chat message body — the one long-form surface
      title:   step(16, 24), // card and panel titles
      heading: step(20, 28), // page titles
      display: step(24, 32), // welcome headline, headline metrics
    },
    extend: {
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
        mono: ["JetBrains Mono", "Consolas", "Menlo", "monospace"],
      },
      colors: {
        // Surface palette — driven by CSS vars, theme-aware
        surface: {
          0: themeColor("--surface-0"),
          1: themeColor("--surface-1"),
          2: themeColor("--surface-2"),
          3: themeColor("--surface-3"),
          4: themeColor("--surface-4"),
        },
        border: themeColor("--border-color"),
        "control-border": themeColor("--control-border-color"),
        accent: themeColor("--accent-color"),
        "accent-hover": themeColor("--accent-hover-color"),
        // Semantic roles keep workflow meaning consistent across every surface.
        // `success` is intentionally separate from ordinary in-progress state.
        "status-progress": themeColor("--status-progress"),
        "status-progress-soft": themeColor("--status-progress-soft"),
        "status-success": themeColor("--status-success"),
        "status-success-soft": themeColor("--status-success-soft"),
        "status-warning": themeColor("--status-warning"),
        "status-warning-soft": themeColor("--status-warning-soft"),
        "status-danger": themeColor("--status-danger"),
        "status-danger-soft": themeColor("--status-danger-soft"),
        "status-info": themeColor("--status-info"),
        "status-info-soft": themeColor("--status-info-soft"),
        // Override gray shades with CSS vars so every existing text-gray-* /
        // bg-gray-* class automatically flips between light and dark themes.
        // The semantic mapping inverts the scale: in light mode gray-200 is
        // near-black (readable on white), in dark mode it's near-white.
        gray: {
          100: themeColor("--gray-100"),
          200: themeColor("--gray-200"),
          300: themeColor("--gray-300"),
          400: themeColor("--gray-400"),
          500: themeColor("--gray-500"),
          600: themeColor("--gray-600"),
          700: themeColor("--gray-700"),
          900: themeColor("--gray-900"),
        },
      },
      keyframes: {
        blink: { "0%,100%": { opacity: 1 }, "50%": { opacity: 0 } },
        "typing-dot": {
          "0%, 80%, 100%": { transform: "scale(0.6)", opacity: "0.4" },
          "40%":           { transform: "scale(1.0)", opacity: "1"   },
        },
      },
      animation: {
        blink: "blink 1s step-end infinite",
        "typing-dot": "typing-dot 1.2s ease-in-out infinite",
      },
    },
  },
  plugins: [require("@tailwindcss/typography")],
};
