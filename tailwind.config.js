/** @type {import('tailwindcss').Config} */
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
          0: "var(--surface-0)",
          1: "var(--surface-1)",
          2: "var(--surface-2)",
          3: "var(--surface-3)",
          4: "var(--surface-4)",
        },
        border: "var(--border-color)",
        accent: "var(--accent-color)",
        "accent-hover": "var(--accent-hover-color)",
        // Override gray shades with CSS vars so every existing text-gray-* /
        // bg-gray-* class automatically flips between light and dark themes.
        // The semantic mapping inverts the scale: in light mode gray-200 is
        // near-black (readable on white), in dark mode it's near-white.
        gray: {
          100: "var(--gray-100)",
          200: "var(--gray-200)",
          300: "var(--gray-300)",
          400: "var(--gray-400)",
          500: "var(--gray-500)",
          600: "var(--gray-600)",
          700: "var(--gray-700)",
          900: "var(--gray-900)",
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
