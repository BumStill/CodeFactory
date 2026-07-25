/** @type {import('tailwindcss').Config} */
const themeColor = (name) => `rgb(var(${name}) / <alpha-value>)`;

export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  darkMode: ["selector", '[data-theme="dark"]'],
  theme: {
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
        accent: themeColor("--accent-color"),
        "accent-hover": themeColor("--accent-hover-color"),
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
