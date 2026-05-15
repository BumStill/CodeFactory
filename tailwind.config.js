/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
        mono: ["JetBrains Mono", "Consolas", "Menlo", "monospace"],
      },
      colors: {
        surface: {
          0: "#0d0d0d",
          1: "#111111",
          2: "#161616",
          3: "#1a1a1a",
          4: "#222222",
        },
        border: "#2a2a2a",
        accent: "#3b82f6",
        "accent-hover": "#2563eb",
      },
      keyframes: {
        blink: { "0%,100%": { opacity: 1 }, "50%": { opacity: 0 } },
      },
      animation: {
        blink: "blink 1s step-end infinite",
      },
    },
  },
  plugins: [require("@tailwindcss/typography")],
};
